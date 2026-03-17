use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::execution_engine::ExecutionEngine;
use inkwell::module::Module;
use inkwell::types::{FloatType, IntType};
use inkwell::values::{FloatValue, FunctionValue, IntValue, PointerValue};
use inkwell::{FloatPredicate, IntPredicate, OptimizationLevel};

use crate::eval::{CompiledExpr, FnKind, Idx};
use crate::parser::BinOp;

type TickFn = unsafe extern "C" fn(*mut f64, *const f64, *const i32, i32, i32);

pub struct CompiledTick {
    _engine: ExecutionEngine<'static>,
    fn_ptr: TickFn,
}

impl CompiledTick {
    pub unsafe fn call(
        &self,
        values: *mut f64,
        prev_values: *const f64,
        lookup: *const i32,
        lookup_cols: i32,
        lookup_rows: i32,
    ) {
        unsafe { (self.fn_ptr)(values, prev_values, lookup, lookup_cols, lookup_rows) }
    }
}

pub(crate) fn compile_tick(
    formulas: &[(Idx, CompiledExpr)],
    _value_count: usize,
    _lookup_table: &[i32],
    _lookup_cols: usize,
    _lookup_rows: usize,
    opt_level: u8,
) -> CompiledTick {
    let context = Box::leak(Box::new(Context::create()));
    let module = context.create_module("tick");
    let level = match opt_level {
        0 => OptimizationLevel::None,
        1 => OptimizationLevel::Less,
        3 => OptimizationLevel::Aggressive,
        _ => OptimizationLevel::Default,
    };
    let engine = module
        .create_jit_execution_engine(level)
        .expect("failed to create JIT engine");

    let f64_type = context.f64_type();
    let i32_type = context.i32_type();
    let ptr_type = context.ptr_type(inkwell::AddressSpace::default());
    let void_type = context.void_type();

    let fn_type = void_type.fn_type(
        &[
            ptr_type.into(), // values: *mut f64
            ptr_type.into(), // prev_values: *const f64
            ptr_type.into(), // lookup: *const i32
            i32_type.into(), // lookup_cols
            i32_type.into(), // lookup_rows
        ],
        false,
    );
    let function = module.add_function("tick", fn_type, None);
    let entry = context.append_basic_block(function, "entry");
    let builder = context.create_builder();
    builder.position_at_end(entry);

    let values_ptr = function.get_nth_param(0).unwrap().into_pointer_value();
    let prev_ptr = function.get_nth_param(1).unwrap().into_pointer_value();
    let lookup_ptr = function.get_nth_param(2).unwrap().into_pointer_value();
    let lookup_cols = function.get_nth_param(3).unwrap().into_int_value();
    let lookup_rows = function.get_nth_param(4).unwrap().into_int_value();

    let mut emitter = Emitter {
        context,
        builder: &builder,
        module: &module,
        function,
        f64_type,
        i32_type,
        values_ptr,
        prev_ptr,
        lookup_ptr,
        lookup_cols,
        lookup_rows,
        bb_counter: 0,
    };

    for (idx, expr) in formulas {
        let val = emitter.emit_expr(expr, *idx as usize);
        let dest = unsafe {
            emitter.builder.build_gep(
                f64_type,
                values_ptr,
                &[i32_type.const_int(*idx as u64, false)],
                "dest",
            )
        }
        .unwrap();
        emitter.builder.build_store(dest, val).unwrap();
    }

    builder.build_return(None).unwrap();

    let fn_ptr: TickFn = unsafe { std::mem::transmute(engine.get_function_address("tick").unwrap()) };

    CompiledTick {
        _engine: engine,
        fn_ptr,
    }
}

struct Emitter<'ctx, 'a> {
    context: &'ctx Context,
    builder: &'a Builder<'ctx>,
    module: &'a Module<'ctx>,
    function: FunctionValue<'ctx>,
    f64_type: FloatType<'ctx>,
    i32_type: IntType<'ctx>,
    values_ptr: PointerValue<'ctx>,
    prev_ptr: PointerValue<'ctx>,
    lookup_ptr: PointerValue<'ctx>,
    lookup_cols: IntValue<'ctx>,
    lookup_rows: IntValue<'ctx>,
    bb_counter: u32,
}

impl<'ctx, 'a> Emitter<'ctx, 'a> {
    fn next_bb(&mut self, name: &str) -> inkwell::basic_block::BasicBlock<'ctx> {
        self.bb_counter += 1;
        self.context
            .append_basic_block(self.function, &format!("{}_{}", name, self.bb_counter))
    }

    fn load_f64(&self, ptr: PointerValue<'ctx>, idx: usize, name: &str) -> FloatValue<'ctx> {
        let gep = unsafe {
            self.builder.build_gep(
                self.f64_type,
                ptr,
                &[self.i32_type.const_int(idx as u64, false)],
                &format!("{}_ptr", name),
            )
        }
        .unwrap();
        self.builder
            .build_load(self.f64_type, gep, name)
            .unwrap()
            .into_float_value()
    }

    fn emit_expr(&mut self, expr: &CompiledExpr, self_idx: usize) -> FloatValue<'ctx> {
        match expr {
            CompiledExpr::Number(n) => self.f64_type.const_float(*n),

            CompiledExpr::CellRef(idx) => self.load_f64(self.values_ptr, *idx as usize, "cell"),

            CompiledExpr::SelfRef => self.load_f64(self.prev_ptr, self_idx, "self_prev"),

            CompiledExpr::UnaryNeg(operand) => {
                let v = self.emit_expr(operand, self_idx);
                self.builder.build_float_neg(v, "neg").unwrap()
            }

            CompiledExpr::BinaryOp { op, left, right } => {
                let l = self.emit_expr(left, self_idx);
                let r = self.emit_expr(right, self_idx);
                self.emit_binop(*op, l, r)
            }

            CompiledExpr::FunctionCall { kind, args } => self.emit_fn_call(kind, args, self_idx),

            CompiledExpr::IndirectAddress { row, col } => {
                self.emit_indirect(row, col, self_idx)
            }
        }
    }

    fn emit_binop(&self, op: BinOp, l: FloatValue<'ctx>, r: FloatValue<'ctx>) -> FloatValue<'ctx> {
        match op {
            BinOp::Add => self.builder.build_float_add(l, r, "add").unwrap(),
            BinOp::Sub => self.builder.build_float_sub(l, r, "sub").unwrap(),
            BinOp::Mul => self.builder.build_float_mul(l, r, "mul").unwrap(),
            BinOp::Div => {
                let zero = self.f64_type.const_float(0.0);
                let is_zero = self
                    .builder
                    .build_float_compare(FloatPredicate::OEQ, r, zero, "div_zero")
                    .unwrap();
                let div = self.builder.build_float_div(l, r, "div").unwrap();
                self.builder
                    .build_select(is_zero, zero, div, "div_safe")
                    .unwrap()
                    .into_float_value()
            }
            BinOp::Eq => self.emit_cmp(FloatPredicate::OEQ, l, r),
            BinOp::Neq => self.emit_cmp(FloatPredicate::ONE, l, r),
            BinOp::Lt => self.emit_cmp(FloatPredicate::OLT, l, r),
            BinOp::Lte => self.emit_cmp(FloatPredicate::OLE, l, r),
            BinOp::Gt => self.emit_cmp(FloatPredicate::OGT, l, r),
            BinOp::Gte => self.emit_cmp(FloatPredicate::OGE, l, r),
        }
    }

    fn emit_cmp(
        &self,
        pred: FloatPredicate,
        l: FloatValue<'ctx>,
        r: FloatValue<'ctx>,
    ) -> FloatValue<'ctx> {
        let cmp = self
            .builder
            .build_float_compare(pred, l, r, "cmp")
            .unwrap();
        let one = self.f64_type.const_float(1.0);
        let zero = self.f64_type.const_float(0.0);
        self.builder
            .build_select(cmp, one, zero, "cmp_result")
            .unwrap()
            .into_float_value()
    }

    fn emit_fn_call(
        &mut self,
        kind: &FnKind,
        args: &[CompiledExpr],
        self_idx: usize,
    ) -> FloatValue<'ctx> {
        match kind {
            FnKind::RowLit(r) => {
                if args.is_empty() {
                    self.f64_type.const_float(*r as f64)
                } else {
                    self.emit_expr(&args[0], self_idx)
                }
            }
            FnKind::ColLit(c) => {
                if args.is_empty() {
                    self.f64_type.const_float(*c as f64)
                } else {
                    self.emit_expr(&args[0], self_idx)
                }
            }
            FnKind::Not => {
                let v = self.emit_expr(&args[0], self_idx);
                let zero = self.f64_type.const_float(0.0);
                let is_zero = self
                    .builder
                    .build_float_compare(FloatPredicate::OEQ, v, zero, "not_cmp")
                    .unwrap();
                let one = self.f64_type.const_float(1.0);
                self.builder
                    .build_select(is_zero, one, zero, "not_result")
                    .unwrap()
                    .into_float_value()
            }
            FnKind::If => self.emit_if(args, self_idx),
            FnKind::Or => self.emit_or(args, self_idx),
            FnKind::And => self.emit_and(args, self_idx),
            FnKind::Mod => self.emit_mod(args, self_idx),
            FnKind::FloorMath => self.emit_floor_math(args, self_idx),
            FnKind::BitRShift => self.emit_bitrshift(args, self_idx),
            FnKind::Generic(_) => self.f64_type.const_float(0.0),
        }
    }

    fn emit_if(&mut self, args: &[CompiledExpr], self_idx: usize) -> FloatValue<'ctx> {
        let cond = self.emit_expr(&args[0], self_idx);
        let zero = self.f64_type.const_float(0.0);
        let is_true = self
            .builder
            .build_float_compare(FloatPredicate::ONE, cond, zero, "if_cond")
            .unwrap();

        let then_bb = self.next_bb("if_then");
        let else_bb = self.next_bb("if_else");
        let merge_bb = self.next_bb("if_merge");

        self.builder
            .build_conditional_branch(is_true, then_bb, else_bb)
            .unwrap();

        self.builder.position_at_end(then_bb);
        let then_val = self.emit_expr(&args[1], self_idx);
        let then_exit = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(else_bb);
        let else_val = if args.len() > 2 {
            self.emit_expr(&args[2], self_idx)
        } else {
            zero
        };
        let else_exit = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(merge_bb);
        let phi = self.builder.build_phi(self.f64_type, "if_result").unwrap();
        phi.add_incoming(&[(&then_val, then_exit), (&else_val, else_exit)]);
        phi.as_basic_value().into_float_value()
    }

    fn emit_or(&mut self, args: &[CompiledExpr], self_idx: usize) -> FloatValue<'ctx> {
        if args.is_empty() {
            return self.f64_type.const_float(0.0);
        }

        let merge_bb = self.next_bb("or_merge");
        let one = self.f64_type.const_float(1.0);
        let zero_const = self.f64_type.const_float(0.0);
        let mut incoming: Vec<(FloatValue<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)> = Vec::new();

        for (i, arg) in args.iter().enumerate() {
            let v = self.emit_expr(arg, self_idx);
            let is_true = self
                .builder
                .build_float_compare(FloatPredicate::ONE, v, zero_const, "or_cmp")
                .unwrap();
            let current_bb = self.builder.get_insert_block().unwrap();

            if i == args.len() - 1 {
                let result = self
                    .builder
                    .build_select(is_true, one, zero_const, "or_last")
                    .unwrap()
                    .into_float_value();
                incoming.push((result, current_bb));
                self.builder.build_unconditional_branch(merge_bb).unwrap();
            } else {
                let next_bb = self.next_bb("or_next");
                incoming.push((one, current_bb));
                self.builder
                    .build_conditional_branch(is_true, merge_bb, next_bb)
                    .unwrap();
                self.builder.position_at_end(next_bb);
            }
        }

        self.builder.position_at_end(merge_bb);
        let phi = self.builder.build_phi(self.f64_type, "or_result").unwrap();
        let refs: Vec<(&dyn inkwell::values::BasicValue<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)> =
            incoming.iter().map(|(v, bb)| (v as &dyn inkwell::values::BasicValue<'ctx>, *bb)).collect();
        for (v, bb) in &refs {
            phi.add_incoming(&[(*v, *bb)]);
        }
        phi.as_basic_value().into_float_value()
    }

    fn emit_and(&mut self, args: &[CompiledExpr], self_idx: usize) -> FloatValue<'ctx> {
        if args.is_empty() {
            return self.f64_type.const_float(0.0);
        }

        let merge_bb = self.next_bb("and_merge");
        let one = self.f64_type.const_float(1.0);
        let zero_const = self.f64_type.const_float(0.0);
        let mut incoming: Vec<(FloatValue<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)> = Vec::new();

        for (i, arg) in args.iter().enumerate() {
            let v = self.emit_expr(arg, self_idx);
            let is_false = self
                .builder
                .build_float_compare(FloatPredicate::OEQ, v, zero_const, "and_cmp")
                .unwrap();
            let current_bb = self.builder.get_insert_block().unwrap();

            if i == args.len() - 1 {
                let result = self
                    .builder
                    .build_select(is_false, zero_const, one, "and_last")
                    .unwrap()
                    .into_float_value();
                incoming.push((result, current_bb));
                self.builder.build_unconditional_branch(merge_bb).unwrap();
            } else {
                let next_bb = self.next_bb("and_next");
                incoming.push((zero_const, current_bb));
                self.builder
                    .build_conditional_branch(is_false, merge_bb, next_bb)
                    .unwrap();
                self.builder.position_at_end(next_bb);
            }
        }

        self.builder.position_at_end(merge_bb);
        let phi = self.builder.build_phi(self.f64_type, "and_result").unwrap();
        let refs: Vec<(&dyn inkwell::values::BasicValue<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)> =
            incoming.iter().map(|(v, bb)| (v as &dyn inkwell::values::BasicValue<'ctx>, *bb)).collect();
        for (v, bb) in &refs {
            phi.add_incoming(&[(*v, *bb)]);
        }
        phi.as_basic_value().into_float_value()
    }

    fn emit_mod(&mut self, args: &[CompiledExpr], self_idx: usize) -> FloatValue<'ctx> {
        let num = self.emit_expr(&args[0], self_idx);
        let div = self.emit_expr(&args[1], self_idx);
        let zero = self.f64_type.const_float(0.0);

        // if div == 0.0, return 0.0
        let div_is_zero = self
            .builder
            .build_float_compare(FloatPredicate::OEQ, div, zero, "mod_div_zero")
            .unwrap();

        let compute_bb = self.next_bb("mod_compute");
        let merge_bb = self.next_bb("mod_merge");
        let entry_bb = self.builder.get_insert_block().unwrap();
        self.builder
            .build_conditional_branch(div_is_zero, merge_bb, compute_bb)
            .unwrap();

        self.builder.position_at_end(compute_bb);
        let rem = self.builder.build_float_rem(num, div, "mod_rem").unwrap();
        // sign adjustment: if rem != 0 && sign(rem) != sign(div), add div
        let rem_nonzero = self
            .builder
            .build_float_compare(FloatPredicate::ONE, rem, zero, "rem_nz")
            .unwrap();
        let rem_sign = self
            .builder
            .build_float_compare(FloatPredicate::OLT, rem, zero, "rem_neg")
            .unwrap();
        let div_sign = self
            .builder
            .build_float_compare(FloatPredicate::OLT, div, zero, "div_neg")
            .unwrap();
        let signs_differ = self
            .builder
            .build_xor(rem_sign, div_sign, "signs_diff")
            .unwrap();
        let need_adjust = self
            .builder
            .build_and(rem_nonzero, signs_differ, "need_adj")
            .unwrap();
        let adjusted = self.builder.build_float_add(rem, div, "adjusted").unwrap();
        let result = self
            .builder
            .build_select(need_adjust, adjusted, rem, "mod_result")
            .unwrap()
            .into_float_value();
        let compute_exit = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(merge_bb);
        let phi = self.builder.build_phi(self.f64_type, "mod_final").unwrap();
        phi.add_incoming(&[(&zero, entry_bb), (&result, compute_exit)]);
        phi.as_basic_value().into_float_value()
    }

    fn emit_floor_math(&mut self, args: &[CompiledExpr], self_idx: usize) -> FloatValue<'ctx> {
        let num = self.emit_expr(&args[0], self_idx);
        let sig = if args.len() > 1 {
            self.emit_expr(&args[1], self_idx)
        } else {
            self.f64_type.const_float(1.0)
        };
        let zero = self.f64_type.const_float(0.0);

        let sig_is_zero = self
            .builder
            .build_float_compare(FloatPredicate::OEQ, sig, zero, "sig_zero")
            .unwrap();

        let compute_bb = self.next_bb("floor_compute");
        let merge_bb = self.next_bb("floor_merge");
        let entry_bb = self.builder.get_insert_block().unwrap();
        self.builder
            .build_conditional_branch(sig_is_zero, merge_bb, compute_bb)
            .unwrap();

        self.builder.position_at_end(compute_bb);
        let divided = self.builder.build_float_div(num, sig, "fdiv").unwrap();
        let floored = self.emit_floor_intrinsic(divided);
        let result = self.builder.build_float_mul(floored, sig, "fmul").unwrap();
        let compute_exit = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(merge_bb);
        let phi = self
            .builder
            .build_phi(self.f64_type, "floor_result")
            .unwrap();
        phi.add_incoming(&[(&zero, entry_bb), (&result, compute_exit)]);
        phi.as_basic_value().into_float_value()
    }

    fn emit_floor_intrinsic(&self, val: FloatValue<'ctx>) -> FloatValue<'ctx> {
        let floor_fn = self.module.get_function("llvm.floor.f64").unwrap_or_else(|| {
            let ft = self.f64_type.fn_type(&[self.f64_type.into()], false);
            self.module.add_function("llvm.floor.f64", ft, None)
        });
        self.builder
            .build_call(floor_fn, &[val.into()], "floor")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_float_value()
    }

    fn emit_bitrshift(&mut self, args: &[CompiledExpr], self_idx: usize) -> FloatValue<'ctx> {
        let num = self.emit_expr(&args[0], self_idx);
        let shift = self.emit_expr(&args[1], self_idx);
        let i64_type = self.context.i64_type();
        let num_i = self
            .builder
            .build_float_to_signed_int(num, i64_type, "num_i64")
            .unwrap();
        let shift_u = self
            .builder
            .build_float_to_unsigned_int(shift, i64_type, "shift_u64")
            .unwrap();
        let shifted = self
            .builder
            .build_right_shift(num_i, shift_u, true, "rshift")
            .unwrap();
        self.builder
            .build_signed_int_to_float(shifted, self.f64_type, "rshift_f64")
            .unwrap()
    }

    fn emit_indirect(
        &mut self,
        row_expr: &CompiledExpr,
        col_expr: &CompiledExpr,
        self_idx: usize,
    ) -> FloatValue<'ctx> {
        let row_f = self.emit_expr(row_expr, self_idx);
        let col_f = self.emit_expr(col_expr, self_idx);
        let zero = self.f64_type.const_float(0.0);
        let one_i32 = self.i32_type.const_int(1, false);
        let row_i = self
            .builder
            .build_float_to_signed_int(row_f, self.i32_type, "row_i")
            .unwrap();
        let col_i = self
            .builder
            .build_float_to_signed_int(col_f, self.i32_type, "col_i")
            .unwrap();

        // bounds check: row >= 1 && row <= lookup_rows && col >= 1 && col <= lookup_cols
        let row_ge1 = self
            .builder
            .build_int_compare(IntPredicate::SGE, row_i, one_i32, "row_ge1")
            .unwrap();
        let row_le_max = self
            .builder
            .build_int_compare(IntPredicate::SLE, row_i, self.lookup_rows, "row_le")
            .unwrap();
        let col_ge1 = self
            .builder
            .build_int_compare(IntPredicate::SGE, col_i, one_i32, "col_ge1")
            .unwrap();
        let col_le_max = self
            .builder
            .build_int_compare(IntPredicate::SLE, col_i, self.lookup_cols, "col_le")
            .unwrap();
        let row_ok = self.builder.build_and(row_ge1, row_le_max, "row_ok").unwrap();
        let col_ok = self.builder.build_and(col_ge1, col_le_max, "col_ok").unwrap();
        let in_bounds = self.builder.build_and(row_ok, col_ok, "bounds").unwrap();

        let lookup_bb = self.next_bb("ind_lookup");
        let oob_bb = self.next_bb("ind_oob");
        let merge_bb = self.next_bb("ind_merge");

        self.builder
            .build_conditional_branch(in_bounds, lookup_bb, oob_bb)
            .unwrap();

        // lookup path
        self.builder.position_at_end(lookup_bb);
        let row_off = self
            .builder
            .build_int_sub(row_i, one_i32, "row_off")
            .unwrap();
        let col_off = self
            .builder
            .build_int_sub(col_i, one_i32, "col_off")
            .unwrap();
        let row_x_cols = self
            .builder
            .build_int_mul(row_off, self.lookup_cols, "rxc")
            .unwrap();
        let offset = self
            .builder
            .build_int_add(row_x_cols, col_off, "offset")
            .unwrap();

        let idx_ptr = unsafe {
            self.builder
                .build_gep(self.i32_type, self.lookup_ptr, &[offset], "idx_ptr")
        }
        .unwrap();
        let idx_val = self
            .builder
            .build_load(self.i32_type, idx_ptr, "idx_val")
            .unwrap()
            .into_int_value();

        let neg_one = self.i32_type.const_int(u64::MAX, true); // -1 as i32
        let is_valid = self
            .builder
            .build_int_compare(IntPredicate::NE, idx_val, neg_one, "valid")
            .unwrap();

        let load_bb = self.next_bb("ind_load");
        let invalid_bb = self.next_bb("ind_invalid");
        self.builder
            .build_conditional_branch(is_valid, load_bb, invalid_bb)
            .unwrap();

        self.builder.position_at_end(load_bb);
        let cell_ptr = unsafe {
            self.builder
                .build_gep(self.f64_type, self.values_ptr, &[idx_val], "cell_ptr")
        }
        .unwrap();
        let cell_val = self
            .builder
            .build_load(self.f64_type, cell_ptr, "cell_val")
            .unwrap()
            .into_float_value();
        let load_exit = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(invalid_bb);
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(oob_bb);
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(merge_bb);
        let phi = self
            .builder
            .build_phi(self.f64_type, "ind_result")
            .unwrap();
        phi.add_incoming(&[
            (&cell_val, load_exit),
            (&zero, invalid_bb),
            (&zero, oob_bb),
        ]);
        phi.as_basic_value().into_float_value()
    }
}
