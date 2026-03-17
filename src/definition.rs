use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ApiType {
    F64,
    Bool,
    U8,
    I32,
    U32,
    I64,
    U64,
    Array(ApiScalar, usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ApiScalar {
    F64,
    Bool,
    U8,
    I32,
    U32,
    I64,
    U64,
}

#[derive(Debug, Clone)]
pub struct ParamDef {
    pub name: String,
    pub cells: Vec<(String, u32)>,
    pub api_type: ApiType,
}

#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub name: String,
    pub inputs: Vec<ParamDef>,
    pub outputs: Vec<ParamDef>,
    pub ticks: u32,
}

#[derive(Debug, Clone)]
pub struct Definition {
    pub functions: Vec<FunctionDef>,
}

#[derive(Debug)]
pub enum DefinitionError {
    InvalidRange(String),
    ArrayLengthMismatch { expected: usize, actual: usize, range: String },
}

impl fmt::Display for DefinitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRange(r) => write!(f, "invalid cell range: {}", r),
            Self::ArrayLengthMismatch { expected, actual, range } =>
                write!(f, "array length mismatch: expected {}, got {} cells from {}", expected, actual, range),
        }
    }
}

impl std::error::Error for DefinitionError {}

fn col_to_num(col: &str) -> u32 {
    col.bytes().fold(0u32, |acc, b| acc * 26 + (b - b'A' + 1) as u32)
}

fn num_to_col(mut n: u32) -> String {
    let mut result = Vec::new();
    while n > 0 {
        n -= 1;
        result.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    result.reverse();
    String::from_utf8(result).unwrap()
}

/// Parse a cell reference like "A1" into (col_str, row).
pub fn parse_cell_ref(s: &str) -> Option<(String, u32)> {
    let col_end = s.find(|c: char| c.is_ascii_digit())?;
    if col_end == 0 { return None; }
    let col = &s[..col_end];
    let row: u32 = s[col_end..].parse().ok()?;
    Some((col.to_string(), row))
}

/// Expand "A2:A9" or "B95:Q110" into an ordered list of (col, row) pairs.
/// Order: row-major, iterating columns within each row.
pub fn expand_cell_range(range: &str) -> Result<Vec<(String, u32)>, DefinitionError> {
    let parts: Vec<&str> = range.split(':').collect();
    if parts.len() != 2 {
        return Err(DefinitionError::InvalidRange(range.into()));
    }
    let (start_col, start_row) = parse_cell_ref(parts[0])
        .ok_or_else(|| DefinitionError::InvalidRange(range.into()))?;
    let (end_col, end_row) = parse_cell_ref(parts[1])
        .ok_or_else(|| DefinitionError::InvalidRange(range.into()))?;

    let c1 = col_to_num(&start_col);
    let c2 = col_to_num(&end_col);
    let mut cells = Vec::new();
    for row in start_row..=end_row {
        for col_num in c1..=c2 {
            cells.push((num_to_col(col_num), row));
        }
    }
    Ok(cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_single_column_range() {
        let cells = expand_cell_range("A2:A9").unwrap();
        assert_eq!(cells.len(), 8);
        assert_eq!(cells[0], ("A".into(), 2));
        assert_eq!(cells[7], ("A".into(), 9));
    }

    #[test]
    fn expand_rect_range() {
        let cells = expand_cell_range("B95:Q110").unwrap();
        assert_eq!(cells.len(), 256); // 16 cols * 16 rows
        assert_eq!(cells[0], ("B".into(), 95));
        assert_eq!(cells[255], ("Q".into(), 110));
    }

    #[test]
    fn expand_single_cell() {
        let cells = expand_cell_range("A1:A1").unwrap();
        assert_eq!(cells.len(), 1);
    }

    #[test]
    fn invalid_range() {
        assert!(expand_cell_range("not_a_range").is_err());
    }
}
