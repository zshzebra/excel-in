use inkwell::context::Context;
use inkwell::execution_engine::ExecutionEngine;
use inkwell::OptimizationLevel;

use crate::codegen;
use crate::eval::{CompiledExpr, Idx};

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

    codegen::emit_tick_function(context, &module, formulas);

    let fn_ptr: TickFn =
        unsafe { std::mem::transmute(engine.get_function_address("tick").unwrap()) };

    CompiledTick {
        _engine: engine,
        fn_ptr,
    }
}
