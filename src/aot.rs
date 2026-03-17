use std::fmt;

use crate::definition::{ApiType, Definition};
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
