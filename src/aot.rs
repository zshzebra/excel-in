use std::fmt;
use std::path::Path;
use std::process::Command;

use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine};
use inkwell::types::{ArrayType, FloatType, IntType, StructType};
use inkwell::values::{BasicValueEnum, FloatValue, FunctionValue, IntValue, PointerValue};
use inkwell::{FloatPredicate, OptimizationLevel};

use crate::codegen;
use crate::definition::{ApiScalar, ApiType, Definition, FunctionDef};
use crate::eval::{CellId, Evaluator, Idx};

#[derive(Debug)]
pub enum AotError {
    CellNotFound { cell: String, function: String },
    ArrayLengthMismatch { expected: usize, actual: usize, cells: String },
    LlvmError(String),
    LinkerError(String),
    DefinitionError(String),
}

impl fmt::Display for AotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CellNotFound { cell, function } =>
                write!(f, "cell {} not found (function {})", cell, function),
            Self::ArrayLengthMismatch { expected, actual, cells } =>
                write!(f, "array length mismatch: expected {}, got {} from {}", expected, actual, cells),
            Self::LlvmError(e) => write!(f, "LLVM error: {}", e),
            Self::LinkerError(e) => write!(f, "linker error: {}", e),
            Self::DefinitionError(e) => write!(f, "definition error: {}", e),
        }
    }
}

impl std::error::Error for AotError {}

fn resolve_cell_idx(eval: &Evaluator, col: &str, row: u32, fn_name: &str) -> Result<Idx, AotError> {
    let id = CellId::local(col.to_string(), row);
    eval.cell_index(&id).ok_or_else(|| AotError::CellNotFound {
        cell: format!("{}{}", col, row),
        function: fn_name.into(),
    })
}

fn resolve_param_indices(
    eval: &Evaluator,
    cells: &[(String, u32)],
    fn_name: &str,
) -> Result<Vec<Idx>, AotError> {
    cells.iter()
        .map(|(col, row)| resolve_cell_idx(eval, col, *row, fn_name))
        .collect()
}

pub fn validate_definition(eval: &Evaluator, def: &Definition) -> Result<(), AotError> {
    for func in &def.functions {
        for param in func.inputs.iter().chain(func.outputs.iter()) {
            resolve_param_indices(eval, &param.cells, &func.name)?;
            if let ApiType::Array(_, expected) = param.api_type {
                if param.cells.len() != expected {
                    return Err(AotError::ArrayLengthMismatch {
                        expected,
                        actual: param.cells.len(),
                        cells: format!("{}:{}", func.name, param.name),
                    });
                }
            }
        }
    }
    Ok(())
}

struct AotEmitter<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    f64_type: FloatType<'ctx>,
    i32_type: IntType<'ctx>,
    i64_type: IntType<'ctx>,
    ptr_type: inkwell::types::PointerType<'ctx>,
    state_type: StructType<'ctx>,
    values_array_type: ArrayType<'ctx>,
    tick_fn: FunctionValue<'ctx>,
    memcpy_fn: FunctionValue<'ctx>,
    lookup_cols_const: IntValue<'ctx>,
    lookup_rows_const: IntValue<'ctx>,
    value_count: usize,
}

impl<'ctx> AotEmitter<'ctx> {
    fn new(context: &'ctx Context, eval: &Evaluator) -> Self {
        let module = context.create_module("aot");
        let builder = context.create_builder();

        let f64_type = context.f64_type();
        let i32_type = context.i32_type();
        let i64_type = context.i64_type();
        let ptr_type = context.ptr_type(inkwell::AddressSpace::default());
        let i1_type = context.bool_type();

        let tick_fn = codegen::emit_tick_function(context, &module, eval.formulas());

        let initial_values = eval.initial_values();
        let value_count = initial_values.len();
        let (lookup_table, lookup_cols, lookup_rows) = eval.build_lookup_table();

        let values_array_type = f64_type.array_type(value_count as u32);
        let lookup_array_type = i32_type.array_type(lookup_table.len() as u32);

        // State struct: { [f64; N], [f64; N], [i32; L] }
        let state_type = context.struct_type(
            &[values_array_type.into(), values_array_type.into(), lookup_array_type.into()],
            false,
        );

        // Bake initial values as a global constant
        let init_vals: Vec<_> = initial_values.iter()
            .map(|v| f64_type.const_float(*v))
            .collect();
        let init_array = f64_type.const_array(&init_vals);
        let init_global = module.add_global(values_array_type, None, "initial_values");
        init_global.set_initializer(&init_array);
        init_global.set_constant(true);

        // Bake lookup table as a global constant
        let lookup_vals: Vec<_> = lookup_table.iter()
            .map(|v| i32_type.const_int(*v as u64, true))
            .collect();
        let lookup_array = i32_type.const_array(&lookup_vals);
        let lookup_global = module.add_global(lookup_array_type, None, "lookup_table");
        lookup_global.set_initializer(&lookup_array);
        lookup_global.set_constant(true);

        let lookup_cols_const = i32_type.const_int(lookup_cols as u64, false);
        let lookup_rows_const = i32_type.const_int(lookup_rows as u64, false);

        // Declare malloc, free, llvm.memcpy
        let malloc_type = ptr_type.fn_type(&[i64_type.into()], false);
        module.add_function("malloc", malloc_type, None);

        let free_type = context.void_type().fn_type(&[ptr_type.into()], false);
        module.add_function("free", free_type, None);

        let memcpy_type = context.void_type().fn_type(
            &[ptr_type.into(), ptr_type.into(), i64_type.into(), i1_type.into()],
            false,
        );
        let memcpy_fn = module.add_function("llvm.memcpy.p0.p0.i64", memcpy_type, None);

        // Emit spreadsheet_init
        let init_fn_type = ptr_type.fn_type(&[], false);
        let init_fn = module.add_function("spreadsheet_init", init_fn_type, None);
        let entry = context.append_basic_block(init_fn, "entry");
        builder.position_at_end(entry);

        let state_size = state_type.size_of().unwrap();
        let malloc_fn = module.get_function("malloc").unwrap();
        let state_ptr = builder
            .build_call(malloc_fn, &[state_size.into()], "state")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_pointer_value();

        let values_ptr = builder.build_struct_gep(state_type, state_ptr, 0, "values_ptr").unwrap();
        let prev_ptr = builder.build_struct_gep(state_type, state_ptr, 1, "prev_ptr").unwrap();
        let lookup_ptr = builder.build_struct_gep(state_type, state_ptr, 2, "lookup_ptr").unwrap();

        let values_size = i64_type.const_int((value_count * 8) as u64, false);
        let lookup_size = i64_type.const_int((lookup_table.len() * 4) as u64, false);
        let is_volatile = i1_type.const_zero();

        // Copy initial values into state
        let init_global_ptr = init_global.as_pointer_value();
        builder.build_call(memcpy_fn, &[values_ptr.into(), init_global_ptr.into(), values_size.into(), is_volatile.into()], "").unwrap();

        // Zero prev_values
        builder.build_memset(prev_ptr, 1, context.i8_type().const_zero(), values_size).unwrap();

        // Copy lookup table
        let lookup_global_ptr = lookup_global.as_pointer_value();
        builder.build_call(memcpy_fn, &[lookup_ptr.into(), lookup_global_ptr.into(), lookup_size.into(), is_volatile.into()], "").unwrap();

        builder.build_return(Some(&state_ptr)).unwrap();

        // Emit spreadsheet_free
        let free_fn_type = context.void_type().fn_type(&[ptr_type.into()], false);
        let free_fn = module.add_function("spreadsheet_free", free_fn_type, None);
        let entry = context.append_basic_block(free_fn, "entry");
        builder.position_at_end(entry);

        let state_arg = free_fn.get_nth_param(0).unwrap().into_pointer_value();
        let free_func = module.get_function("free").unwrap();
        builder.build_call(free_func, &[state_arg.into()], "").unwrap();
        builder.build_return(None).unwrap();

        Self {
            context,
            module,
            builder,
            f64_type,
            i32_type,
            i64_type,
            ptr_type,
            state_type,
            values_array_type,
            tick_fn,
            memcpy_fn,
            lookup_cols_const,
            lookup_rows_const,
            value_count,
        }
    }

    fn api_type_to_llvm(&self, t: &ApiType) -> inkwell::types::BasicTypeEnum<'ctx> {
        match t {
            ApiType::F64 => self.f64_type.into(),
            ApiType::Bool => self.context.bool_type().into(),
            ApiType::U8 => self.context.i8_type().into(),
            ApiType::I32 => self.i32_type.into(),
            ApiType::U32 => self.i32_type.into(),
            ApiType::I64 => self.i64_type.into(),
            ApiType::U64 => self.i64_type.into(),
            ApiType::Array(scalar, len) => {
                let elem = self.scalar_to_llvm(*scalar);
                match elem {
                    inkwell::types::BasicTypeEnum::FloatType(ft) => ft.array_type(*len as u32).into(),
                    inkwell::types::BasicTypeEnum::IntType(it) => it.array_type(*len as u32).into(),
                    _ => unreachable!(),
                }
            }
        }
    }

    fn scalar_to_llvm(&self, s: ApiScalar) -> inkwell::types::BasicTypeEnum<'ctx> {
        match s {
            ApiScalar::F64 => self.f64_type.into(),
            ApiScalar::Bool => self.context.bool_type().into(),
            ApiScalar::U8 => self.context.i8_type().into(),
            ApiScalar::I32 => self.i32_type.into(),
            ApiScalar::U32 => self.i32_type.into(),
            ApiScalar::I64 => self.i64_type.into(),
            ApiScalar::U64 => self.i64_type.into(),
        }
    }

    fn cast_input_to_f64(&self, val: BasicValueEnum<'ctx>, api_type: &ApiType) -> FloatValue<'ctx> {
        match api_type {
            ApiType::F64 => val.into_float_value(),
            ApiType::Bool | ApiType::U8 | ApiType::U32 | ApiType::U64 => {
                self.builder
                    .build_unsigned_int_to_float(val.into_int_value(), self.f64_type, "uitofp")
                    .unwrap()
            }
            ApiType::I32 | ApiType::I64 => {
                self.builder
                    .build_signed_int_to_float(val.into_int_value(), self.f64_type, "sitofp")
                    .unwrap()
            }
            ApiType::Array(..) => unreachable!("arrays are not scalar inputs"),
        }
    }

    fn cast_f64_to_output(&self, val: FloatValue<'ctx>, api_type: &ApiType) -> BasicValueEnum<'ctx> {
        match api_type {
            ApiType::F64 => val.into(),
            ApiType::Bool => {
                let zero = self.f64_type.const_float(0.0);
                self.builder
                    .build_float_compare(FloatPredicate::ONE, val, zero, "to_bool")
                    .unwrap()
                    .into()
            }
            ApiType::U8 => {
                self.builder
                    .build_float_to_unsigned_int(val, self.context.i8_type(), "fptoui8")
                    .unwrap()
                    .into()
            }
            ApiType::I32 => {
                self.builder
                    .build_float_to_signed_int(val, self.i32_type, "fptosi32")
                    .unwrap()
                    .into()
            }
            ApiType::U32 => {
                self.builder
                    .build_float_to_unsigned_int(val, self.i32_type, "fptoui32")
                    .unwrap()
                    .into()
            }
            ApiType::I64 => {
                self.builder
                    .build_float_to_signed_int(val, self.i64_type, "fptosi64")
                    .unwrap()
                    .into()
            }
            ApiType::U64 => {
                self.builder
                    .build_float_to_unsigned_int(val, self.i64_type, "fptoui64")
                    .unwrap()
                    .into()
            }
            ApiType::Array(scalar, _) => self.cast_f64_scalar(*scalar, val),
        }
    }

    fn cast_f64_scalar(&self, scalar: ApiScalar, val: FloatValue<'ctx>) -> BasicValueEnum<'ctx> {
        match scalar {
            ApiScalar::F64 => val.into(),
            ApiScalar::Bool => {
                let zero = self.f64_type.const_float(0.0);
                self.builder
                    .build_float_compare(FloatPredicate::ONE, val, zero, "to_bool")
                    .unwrap()
                    .into()
            }
            ApiScalar::U8 => {
                self.builder
                    .build_float_to_unsigned_int(val, self.context.i8_type(), "fptoui8")
                    .unwrap()
                    .into()
            }
            ApiScalar::I32 => {
                self.builder
                    .build_float_to_signed_int(val, self.i32_type, "fptosi32")
                    .unwrap()
                    .into()
            }
            ApiScalar::U32 => {
                self.builder
                    .build_float_to_unsigned_int(val, self.i32_type, "fptoui32")
                    .unwrap()
                    .into()
            }
            ApiScalar::I64 => {
                self.builder
                    .build_float_to_signed_int(val, self.i64_type, "fptosi64")
                    .unwrap()
                    .into()
            }
            ApiScalar::U64 => {
                self.builder
                    .build_float_to_unsigned_int(val, self.i64_type, "fptoui64")
                    .unwrap()
                    .into()
            }
        }
    }

    fn load_from_values(&self, values_ptr: PointerValue<'ctx>, idx: Idx) -> FloatValue<'ctx> {
        let gep = unsafe {
            self.builder.build_gep(
                self.f64_type,
                values_ptr,
                &[self.i32_type.const_int(idx as u64, false)],
                "val_ptr",
            )
        }
        .unwrap();
        self.builder
            .build_load(self.f64_type, gep, "val")
            .unwrap()
            .into_float_value()
    }

    fn emit_function_wrapper(
        &self,
        eval: &Evaluator,
        func: &FunctionDef,
    ) -> Result<FunctionValue<'ctx>, AotError> {
        let mut param_types: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> = vec![self.ptr_type.into()];
        for input in &func.inputs {
            param_types.push(self.api_type_to_llvm(&input.api_type).into());
        }

        // Build return struct type from all outputs
        let mut return_field_types: Vec<inkwell::types::BasicTypeEnum<'ctx>> = Vec::new();
        for output in &func.outputs {
            return_field_types.push(self.api_type_to_llvm(&output.api_type));
        }
        let return_type = self.context.struct_type(&return_field_types, false);

        let fn_type = return_type.fn_type(&param_types, false);
        let function = self.module.add_function(&func.name, fn_type, None);
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        let state_ptr = function.get_nth_param(0).unwrap().into_pointer_value();
        let values_ptr = self.builder.build_struct_gep(self.state_type, state_ptr, 0, "values").unwrap();
        let prev_ptr = self.builder.build_struct_gep(self.state_type, state_ptr, 1, "prev").unwrap();
        let lookup_ptr = self.builder.build_struct_gep(self.state_type, state_ptr, 2, "lookup").unwrap();

        // Store inputs into values array
        let mut arg_idx = 1u32;
        for input in &func.inputs {
            let indices = resolve_param_indices(eval, &input.cells, &func.name)?;
            let arg = function.get_nth_param(arg_idx).unwrap();

            match &input.api_type {
                ApiType::Array(_, _) => {
                    let arr = arg.into_array_value();
                    for (i, &cell_idx) in indices.iter().enumerate() {
                        let elem = self.builder
                            .build_extract_value(arr, i as u32, "elem")
                            .unwrap();
                        let scalar_type = match &input.api_type {
                            ApiType::Array(s, _) => match s {
                                ApiScalar::F64 => ApiType::F64,
                                ApiScalar::Bool => ApiType::Bool,
                                ApiScalar::U8 => ApiType::U8,
                                ApiScalar::I32 => ApiType::I32,
                                ApiScalar::U32 => ApiType::U32,
                                ApiScalar::I64 => ApiType::I64,
                                ApiScalar::U64 => ApiType::U64,
                            },
                            _ => unreachable!(),
                        };
                        let as_f64 = self.cast_input_to_f64(elem, &scalar_type);
                        let dest = unsafe {
                            self.builder.build_gep(
                                self.f64_type,
                                values_ptr,
                                &[self.i32_type.const_int(cell_idx as u64, false)],
                                "dest",
                            )
                        }
                        .unwrap();
                        self.builder.build_store(dest, as_f64).unwrap();
                    }
                }
                scalar_type => {
                    let as_f64 = self.cast_input_to_f64(arg, scalar_type);
                    let cell_idx = indices[0];
                    let dest = unsafe {
                        self.builder.build_gep(
                            self.f64_type,
                            values_ptr,
                            &[self.i32_type.const_int(cell_idx as u64, false)],
                            "dest",
                        )
                    }
                    .unwrap();
                    self.builder.build_store(dest, as_f64).unwrap();
                }
            }
            arg_idx += 1;
        }

        // Tick loop: copy values→prev, call tick
        let values_size = self.i64_type.const_int((self.value_count * 8) as u64, false);
        let is_volatile = self.context.bool_type().const_zero();

        for tick in 0..func.ticks {
            // Copy values → prev_values
            self.builder.build_call(
                self.memcpy_fn,
                &[prev_ptr.into(), values_ptr.into(), values_size.into(), is_volatile.into()],
                "",
            ).unwrap();

            self.builder.build_call(
                self.tick_fn,
                &[
                    values_ptr.into(),
                    prev_ptr.into(),
                    lookup_ptr.into(),
                    self.lookup_cols_const.into(),
                    self.lookup_rows_const.into(),
                ],
                &format!("tick_{}", tick),
            ).unwrap();
        }

        // Read outputs and build return struct
        let mut ret = return_type.get_undef();
        for (out_idx, output) in func.outputs.iter().enumerate() {
            let indices = resolve_param_indices(eval, &output.cells, &func.name)?;

            match &output.api_type {
                ApiType::Array(scalar, len) => {
                    let elem_type = self.scalar_to_llvm(*scalar);
                    let arr_type = match elem_type {
                        inkwell::types::BasicTypeEnum::FloatType(ft) => ft.array_type(*len as u32),
                        inkwell::types::BasicTypeEnum::IntType(it) => it.array_type(*len as u32),
                        _ => unreachable!(),
                    };
                    let mut arr = arr_type.get_undef();
                    for (i, &cell_idx) in indices.iter().enumerate() {
                        let f64_val = self.load_from_values(values_ptr, cell_idx);
                        let cast = self.cast_f64_to_output(f64_val, &output.api_type);
                        arr = self.builder
                            .build_insert_value(arr, cast, i as u32, "arr_elem")
                            .unwrap()
                            .into_array_value();
                    }
                    ret = self.builder
                        .build_insert_value(ret, arr, out_idx as u32, "ret_field")
                        .unwrap()
                        .into_struct_value();
                }
                scalar_type => {
                    let f64_val = self.load_from_values(values_ptr, indices[0]);
                    let cast = self.cast_f64_to_output(f64_val, scalar_type);
                    ret = self.builder
                        .build_insert_value(ret, cast, out_idx as u32, "ret_field")
                        .unwrap()
                        .into_struct_value();
                }
            }
        }

        self.builder.build_return(Some(&ret)).unwrap();
        Ok(function)
    }
}

fn c_type_name(t: ApiScalar) -> &'static str {
    match t {
        ApiScalar::F64 => "double",
        ApiScalar::Bool => "bool",
        ApiScalar::U8 => "uint8_t",
        ApiScalar::I32 => "int32_t",
        ApiScalar::U32 => "uint32_t",
        ApiScalar::I64 => "int64_t",
        ApiScalar::U64 => "uint64_t",
    }
}

fn c_param_type(t: &ApiType) -> String {
    match t {
        ApiType::F64 => "double".into(),
        ApiType::Bool => "bool".into(),
        ApiType::U8 => "uint8_t".into(),
        ApiType::I32 => "int32_t".into(),
        ApiType::U32 => "uint32_t".into(),
        ApiType::I64 => "int64_t".into(),
        ApiType::U64 => "uint64_t".into(),
        ApiType::Array(_, _) => unreachable!("arrays not used as input params"),
    }
}

fn sanitize_c_name(name: &str) -> String {
    const RESERVED: &[&str] = &[
        "char", "int", "long", "short", "float", "double", "void", "return",
        "if", "else", "for", "while", "do", "switch", "case", "break",
        "continue", "struct", "union", "enum", "typedef", "const", "static",
        "extern", "register", "volatile", "signed", "unsigned", "default",
        "goto", "sizeof",
    ];
    if RESERVED.contains(&name) { format!("{}_", name) } else { name.to_string() }
}

fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().chain(c).collect(),
            }
        })
        .collect()
}

fn generate_header_string(def: &Definition) -> String {
    let mut h = String::new();
    h.push_str("#pragma once\n\n");
    h.push_str("#include <stdint.h>\n");
    h.push_str("#include <stdbool.h>\n\n");
    h.push_str("typedef struct SpreadsheetState SpreadsheetState;\n\n");
    h.push_str("SpreadsheetState* spreadsheet_init(void);\n");
    h.push_str("void spreadsheet_free(SpreadsheetState* state);\n\n");

    for func in &def.functions {
        let needs_struct = func.outputs.len() > 1
            || func.outputs.iter().any(|o| matches!(o.api_type, ApiType::Array(_, _)));
        let struct_name = format!("{}Output", to_pascal_case(&func.name));

        if needs_struct {
            h.push_str("typedef struct {\n");
            for out in &func.outputs {
                match out.api_type {
                    ApiType::Array(scalar, len) => {
                        h.push_str(&format!("    {} {}[{}];\n",
                            c_type_name(scalar), out.name, len));
                    }
                    _ => {
                        h.push_str(&format!("    {} {};\n",
                            c_param_type(&out.api_type), out.name));
                    }
                }
            }
            h.push_str(&format!("}} {};\n\n", struct_name));
        }

        let return_type = if needs_struct {
            struct_name.clone()
        } else if func.outputs.len() == 1 {
            c_param_type(&func.outputs[0].api_type)
        } else {
            "void".into()
        };

        let params: Vec<String> = std::iter::once("SpreadsheetState* state".into())
            .chain(func.inputs.iter().map(|inp| {
                format!("{} {}", c_param_type(&inp.api_type), sanitize_c_name(&inp.name))
            }))
            .collect();

        h.push_str(&format!("{} {}({});\n\n", return_type, func.name, params.join(", ")));
    }
    h
}

fn link_shared_library(obj_path: &Path, output_path: &Path) -> Result<(), AotError> {
    let output = Command::new("cc")
        .arg("-shared")
        .arg("-o")
        .arg(output_path)
        .arg(obj_path)
        .output()
        .map_err(|e| AotError::LinkerError(format!("failed to invoke cc: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AotError::LinkerError(stderr.into()));
    }
    Ok(())
}

pub fn compile(
    eval: &Evaluator,
    def: &Definition,
    output_path: &Path,
    opt_level: u8,
) -> Result<(), AotError> {
    validate_definition(eval, def)?;

    let context = Context::create();
    let emitter = AotEmitter::new(&context, eval);

    for func_def in &def.functions {
        emitter.emit_function_wrapper(eval, func_def)?;
    }

    Target::initialize_native(&InitializationConfig::default())
        .map_err(|e| AotError::LlvmError(e.to_string()))?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple)
        .map_err(|e| AotError::LlvmError(e.to_string()))?;
    let level = match opt_level {
        0 => OptimizationLevel::None,
        1 => OptimizationLevel::Less,
        3 => OptimizationLevel::Aggressive,
        _ => OptimizationLevel::Default,
    };
    let machine = target
        .create_target_machine(
            &triple,
            "generic",
            "",
            level,
            RelocMode::PIC,
            CodeModel::Default,
        )
        .ok_or_else(|| AotError::LlvmError("failed to create target machine".into()))?;

    let obj_path = output_path.with_extension("o");
    machine
        .write_to_file(&emitter.module, FileType::Object, &obj_path)
        .map_err(|e| AotError::LlvmError(e.to_string()))?;

    link_shared_library(&obj_path, output_path)?;

    let header_path = output_path.with_extension("h");
    let header = generate_header_string(def);
    std::fs::write(&header_path, header)
        .map_err(|e| AotError::LlvmError(e.to_string()))?;

    let _ = std::fs::remove_file(&obj_path);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::*;

    #[test]
    fn header_single_function() {
        let def = Definition {
            functions: vec![FunctionDef {
                name: "char_to_binary".into(),
                inputs: vec![ParamDef {
                    name: "char".into(),
                    cells: vec![("A".into(), 1)],
                    api_type: ApiType::U8,
                }],
                outputs: vec![ParamDef {
                    name: "bits".into(),
                    cells: (2..=9).map(|r| ("A".into(), r)).collect(),
                    api_type: ApiType::Array(ApiScalar::Bool, 8),
                }],
                ticks: 1,
            }],
        };
        let header = generate_header_string(&def);
        assert!(header.contains("SpreadsheetState* spreadsheet_init(void)"));
        assert!(header.contains("void spreadsheet_free(SpreadsheetState* state)"));
        assert!(header.contains("typedef struct {"));
        assert!(header.contains("bool bits[8]"));
        assert!(header.contains("uint8_t char_"));
    }
}
