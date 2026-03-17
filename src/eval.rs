use crate::functions;
use crate::parser::{BinOp, CellRef, Expr, UnOp};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CellId {
    pub workbook: Option<u32>,
    pub col: String,
    pub row: u32,
}

impl CellId {
    pub fn local(col: String, row: u32) -> Self {
        Self { workbook: None, col, row }
    }

    pub fn external(wb: u32, col: String, row: u32) -> Self {
        Self { workbook: Some(wb), col, row }
    }
}

type Idx = u32;


pub struct Evaluator {
    values: Vec<f64>,
    prev_values: Vec<f64>,
    formulas: Vec<(Idx, CompiledExpr)>,
    id_to_idx: HashMap<CellId, Idx>,
    idx_to_id: Vec<CellId>,
}

#[derive(Debug)]
enum CompiledExpr {
    Number(f64),
    CellRef(Idx),
    SelfRef,
    BinaryOp { op: BinOp, left: Box<CompiledExpr>, right: Box<CompiledExpr> },
    UnaryNeg(Box<CompiledExpr>),
    FunctionCall { kind: FnKind, args: Vec<CompiledExpr> },
    IndirectAddress { row: Box<CompiledExpr>, col: Box<CompiledExpr> },
}

#[derive(Debug)]
enum FnKind {
    If,
    Mod,
    Not,
    Or,
    And,
    FloorMath,
    BitRShift,
    RowLit(u32),
    ColLit(u32),
    Generic(String),
}

impl Evaluator {
    pub fn new() -> Self {
        Self {
            values: Vec::new(),
            prev_values: Vec::new(),
            formulas: Vec::new(),
            id_to_idx: HashMap::new(),
            idx_to_id: Vec::new(),
        }
    }

    fn get_or_create_idx(&mut self, id: &CellId) -> Idx {
        if let Some(&idx) = self.id_to_idx.get(id) {
            return idx;
        }
        let idx = self.idx_to_id.len() as Idx;
        self.id_to_idx.insert(id.clone(), idx);
        self.idx_to_id.push(id.clone());
        self.values.push(0.0);
        self.prev_values.push(0.0);
        idx
    }

    pub fn add_cell(&mut self, id: CellId, formula: Expr) {
        let idx = self.get_or_create_idx(&id);
        let compiled = self.compile_expr(&formula, idx);
        self.formulas.push((idx, compiled));
    }

    pub fn set_value(&mut self, id: CellId, val: f64) {
        let idx = self.get_or_create_idx(&id);
        self.values[idx as usize] = val;
    }

    pub fn get_value(&self, id: &CellId) -> f64 {
        self.id_to_idx.get(id)
            .map(|&idx| self.values[idx as usize])
            .unwrap_or(0.0)
    }

    pub fn formula_count(&self) -> usize {
        self.formulas.len()
    }

    pub fn value_count(&self) -> usize {
        self.id_to_idx.len()
    }

    fn compile_expr(&mut self, expr: &Expr, self_idx: Idx) -> CompiledExpr {
        match expr {
            Expr::Number(n) => CompiledExpr::Number(*n),

            Expr::CellRef(r) => {
                let id = cell_ref_to_id(r);
                let idx = self.get_or_create_idx(&id);
                if idx == self_idx {
                    CompiledExpr::SelfRef
                } else {
                    CompiledExpr::CellRef(idx)
                }
            }

            Expr::BinaryOp { op, left, right } => CompiledExpr::BinaryOp {
                op: *op,
                left: Box::new(self.compile_expr(left, self_idx)),
                right: Box::new(self.compile_expr(right, self_idx)),
            },

            Expr::UnaryOp { op: UnOp::Neg, operand } =>
                CompiledExpr::UnaryNeg(Box::new(self.compile_expr(operand, self_idx))),

            Expr::FunctionCall { name, args } => {
                let upper = name.to_uppercase();
                match upper.as_str() {
                    "ROW" => {
                        if let Some(Expr::CellRef(r)) = args.first() {
                            CompiledExpr::Number(r.row as f64)
                        } else if args.is_empty() {
                            CompiledExpr::FunctionCall {
                                kind: FnKind::RowLit(self.idx_to_id[self_idx as usize].row),
                                args: vec![],
                            }
                        } else {
                            CompiledExpr::FunctionCall {
                                kind: FnKind::RowLit(0),
                                args: args.iter().map(|a| self.compile_expr(a, self_idx)).collect(),
                            }
                        }
                    }
                    "COLUMN" => {
                        if let Some(Expr::CellRef(r)) = args.first() {
                            CompiledExpr::Number(col_str_to_num(&r.col) as f64)
                        } else if args.is_empty() {
                            CompiledExpr::FunctionCall {
                                kind: FnKind::ColLit(col_str_to_num(&self.idx_to_id[self_idx as usize].col)),
                                args: vec![],
                            }
                        } else {
                            CompiledExpr::FunctionCall {
                                kind: FnKind::ColLit(0),
                                args: args.iter().map(|a| self.compile_expr(a, self_idx)).collect(),
                            }
                        }
                    }
                    "INDIRECT" => {
                        if let Some(Expr::FunctionCall { name: inner, args: inner_args }) = args.first() {
                            if inner.eq_ignore_ascii_case("ADDRESS") && inner_args.len() >= 2 {
                                return CompiledExpr::IndirectAddress {
                                    row: Box::new(self.compile_expr(&inner_args[0], self_idx)),
                                    col: Box::new(self.compile_expr(&inner_args[1], self_idx)),
                                };
                            }
                        }
                        CompiledExpr::Number(0.0)
                    }
                    "ADDRESS" => CompiledExpr::Number(0.0),
                    _ => {
                        let kind = match upper.as_str() {
                            "IF" => FnKind::If,
                            "MOD" => FnKind::Mod,
                            "NOT" => FnKind::Not,
                            "OR" => FnKind::Or,
                            "AND" => FnKind::And,
                            "FLOOR.MATH" => FnKind::FloorMath,
                            "BITRSHIFT" => FnKind::BitRShift,
                            _ => FnKind::Generic(upper),
                        };
                        CompiledExpr::FunctionCall {
                            kind,
                            args: args.iter().map(|a| self.compile_expr(a, self_idx)).collect(),
                        }
                    }
                }
            }
        }
    }

    pub fn build_eval_order(&mut self) {
        self.formulas.sort_by(|a, b| {
            let a_id = &self.idx_to_id[a.0 as usize];
            let b_id = &self.idx_to_id[b.0 as usize];
            a_id.row.cmp(&b_id.row)
                .then(col_str_to_num(&a_id.col).cmp(&col_str_to_num(&b_id.col)))
        });
        self.formulas.dedup_by(|a, b| a.0 == b.0);
    }

    pub fn tick(&mut self) {
        self.prev_values.copy_from_slice(&self.values);

        for i in 0..self.formulas.len() {
            let idx = self.formulas[i].0 as usize;
            let val = eval_compiled(
                &self.formulas[i].1,
                idx,
                &self.values,
                &self.prev_values,
                &self.id_to_idx,
            );
            self.values[idx] = val;
        }
    }
}

fn eval_compiled(
    expr: &CompiledExpr,
    self_idx: usize,
    values: &[f64],
    prev_values: &[f64],
    id_to_idx: &HashMap<CellId, Idx>,
) -> f64 {
    match expr {
        CompiledExpr::Number(n) => *n,
        CompiledExpr::CellRef(idx) => values[*idx as usize],
        CompiledExpr::SelfRef => prev_values[self_idx],

        CompiledExpr::BinaryOp { op, left, right } => {
            let l = eval_compiled(left, self_idx, values, prev_values, id_to_idx);
            let r = eval_compiled(right, self_idx, values, prev_values, id_to_idx);
            match op {
                BinOp::Add => l + r,
                BinOp::Sub => l - r,
                BinOp::Mul => l * r,
                BinOp::Div => if r != 0.0 { l / r } else { 0.0 },
                BinOp::Eq => if l == r { 1.0 } else { 0.0 },
                BinOp::Neq => if l != r { 1.0 } else { 0.0 },
                BinOp::Lt => if l < r { 1.0 } else { 0.0 },
                BinOp::Lte => if l <= r { 1.0 } else { 0.0 },
                BinOp::Gt => if l > r { 1.0 } else { 0.0 },
                BinOp::Gte => if l >= r { 1.0 } else { 0.0 },
            }
        }

        CompiledExpr::UnaryNeg(operand) =>
            -eval_compiled(operand, self_idx, values, prev_values, id_to_idx),

        CompiledExpr::IndirectAddress { row, col } => {
            let r = eval_compiled(row, self_idx, values, prev_values, id_to_idx) as u32;
            let c = eval_compiled(col, self_idx, values, prev_values, id_to_idx) as u32;
            let col_str = col_num_to_str(c);
            let id = CellId::local(col_str, r);
            id_to_idx.get(&id)
                .map(|&idx| values[idx as usize])
                .unwrap_or(0.0)
        }

        CompiledExpr::FunctionCall { kind, args } => {
            match kind {
                FnKind::RowLit(r) => {
                    if args.is_empty() { *r as f64 }
                    else { eval_compiled(&args[0], self_idx, values, prev_values, id_to_idx) }
                }
                FnKind::ColLit(c) => {
                    if args.is_empty() { *c as f64 }
                    else { eval_compiled(&args[0], self_idx, values, prev_values, id_to_idx) }
                }
                FnKind::If => {
                    let cond = eval_compiled(&args[0], self_idx, values, prev_values, id_to_idx);
                    if cond != 0.0 {
                        eval_compiled(&args[1], self_idx, values, prev_values, id_to_idx)
                    } else if args.len() > 2 {
                        eval_compiled(&args[2], self_idx, values, prev_values, id_to_idx)
                    } else {
                        0.0
                    }
                }
                FnKind::Not => {
                    let v = eval_compiled(&args[0], self_idx, values, prev_values, id_to_idx);
                    if v == 0.0 { 1.0 } else { 0.0 }
                }
                FnKind::Or => {
                    for a in args {
                        if eval_compiled(a, self_idx, values, prev_values, id_to_idx) != 0.0 {
                            return 1.0;
                        }
                    }
                    0.0
                }
                FnKind::And => {
                    if args.is_empty() { return 0.0; }
                    for a in args {
                        if eval_compiled(a, self_idx, values, prev_values, id_to_idx) == 0.0 {
                            return 0.0;
                        }
                    }
                    1.0
                }
                _ => {
                    let evaluated: Vec<f64> = args.iter()
                        .map(|a| eval_compiled(a, self_idx, values, prev_values, id_to_idx))
                        .collect();
                    match kind {
                        FnKind::Mod => functions::excel_mod(&evaluated),
                        FnKind::FloorMath => functions::excel_floor_math(&evaluated),
                        FnKind::BitRShift => functions::excel_bitrshift(&evaluated),
                        FnKind::Generic(name) => functions::call(name, &evaluated),
                        _ => unreachable!(),
                    }
                }
            }
        }
    }
}

fn cell_ref_to_id(r: &CellRef) -> CellId {
    match r.workbook {
        Some(wb) => CellId::external(wb, r.col.clone(), r.row),
        None => CellId::local(r.col.clone(), r.row),
    }
}

fn col_str_to_num(col: &str) -> u32 {
    col.bytes().fold(0u32, |acc, b| acc * 26 + (b - b'A' + 1) as u32)
}

fn col_num_to_str(mut n: u32) -> String {
    let mut result = Vec::new();
    while n > 0 {
        n -= 1;
        result.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    result.reverse();
    String::from_utf8(result).unwrap_or_default()
}
