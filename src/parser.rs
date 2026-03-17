use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Number(f64),
    CellRef(CellRef),
    BinaryOp { op: BinOp, left: Box<Expr>, right: Box<Expr> },
    UnaryOp { op: UnOp, operand: Box<Expr> },
    FunctionCall { name: String, args: Vec<Expr> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CellRef {
    pub col: String,
    pub row: u32,
    pub abs_col: bool,
    pub abs_row: bool,
    pub sheet: Option<String>,
    pub workbook: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div,
    Eq, Neq, Lt, Gt, Lte, Gte,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp {
    Neg,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub msg: String,
    pub pos: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "parse error at position {}: {}", self.pos, self.msg)
    }
}

impl std::error::Error for ParseError {}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn new(input: &str) -> Self {
        Self { chars: input.chars().collect(), pos: 0 }
    }

    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError { msg: msg.into(), pos: self.pos }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.chars.get(self.pos).copied();
        if ch.is_some() { self.pos += 1; }
        ch
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.advance();
        }
    }

    fn expect(&mut self, ch: char) -> Result<(), ParseError> {
        self.skip_whitespace();
        match self.peek() {
            Some(c) if c == ch => { self.advance(); Ok(()) }
            Some(c) => Err(self.err(format!("expected '{}', found '{}'", ch, c))),
            None => Err(self.err(format!("expected '{}', found end of input", ch))),
        }
    }

    fn expression(&mut self) -> Result<Expr, ParseError> {
        self.skip_whitespace();
        let left = self.additive()?;
        self.skip_whitespace();

        let op = match self.peek() {
            Some('=') => { self.advance(); BinOp::Eq }
            Some('<') => {
                self.advance();
                match self.peek() {
                    Some('>') => { self.advance(); BinOp::Neq }
                    Some('=') => { self.advance(); BinOp::Lte }
                    _ => BinOp::Lt,
                }
            }
            Some('>') => {
                self.advance();
                if self.peek() == Some('=') { self.advance(); BinOp::Gte }
                else { BinOp::Gt }
            }
            _ => return Ok(left),
        };

        self.skip_whitespace();
        let right = self.additive()?;
        Ok(Expr::BinaryOp { op, left: Box::new(left), right: Box::new(right) })
    }

    fn additive(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.multiplicative()?;
        loop {
            self.skip_whitespace();
            let op = match self.peek() {
                Some('+') => BinOp::Add,
                Some('-') => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.multiplicative()?;
            left = Expr::BinaryOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn multiplicative(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.unary()?;
        loop {
            self.skip_whitespace();
            let op = match self.peek() {
                Some('*') => BinOp::Mul,
                Some('/') => BinOp::Div,
                _ => break,
            };
            self.advance();
            let right = self.unary()?;
            left = Expr::BinaryOp { op, left: Box::new(left), right: Box::new(right) };
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        self.skip_whitespace();
        if self.peek() == Some('-') {
            self.advance();
            let operand = self.unary()?;
            return Ok(Expr::UnaryOp { op: UnOp::Neg, operand: Box::new(operand) });
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        self.skip_whitespace();
        match self.peek() {
            Some('(') => {
                self.advance();
                let expr = self.expression()?;
                self.expect(')')?;
                Ok(expr)
            }
            Some(c) if c.is_ascii_digit() || c == '.' => self.number(),
            Some('[') => self.external_ref(),
            Some('$') => self.identifier_or_cell(None, None),
            Some(c) if c.is_ascii_alphabetic() || c == '_' => self.identifier_or_cell(None, None),
            _ => Err(self.err(format!("unexpected character: {:?}", self.peek()))),
        }
    }

    fn external_ref(&mut self) -> Result<Expr, ParseError> {
        self.expect('[')?;
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.advance();
        }
        let wb_num: u32 = self.chars[start..self.pos].iter().collect::<String>()
            .parse().map_err(|_| self.err("expected workbook number"))?;
        self.expect(']')?;

        let sheet_start = self.pos;
        while self.peek().is_some_and(|c| c != '!') {
            self.advance();
        }
        let sheet: String = self.chars[sheet_start..self.pos].iter().collect();
        self.expect('!')?;

        self.identifier_or_cell(Some(sheet), Some(wb_num))
    }

    fn number(&mut self) -> Result<Expr, ParseError> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.advance();
        }
        if self.peek() == Some('.') {
            self.advance();
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.advance();
            }
        }
        let s: String = self.chars[start..self.pos].iter().collect();
        let val: f64 = s.parse().map_err(|_| self.err(format!("invalid number: {}", s)))?;
        Ok(Expr::Number(val))
    }

    fn identifier_or_cell(&mut self, sheet: Option<String>, workbook: Option<u32>) -> Result<Expr, ParseError> {
        let start = self.pos;
        let abs_col = self.peek() == Some('$');
        if abs_col { self.advance(); }

        if !self.peek().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') {
            return Err(self.err("expected identifier"));
        }

        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
            self.advance();
        }

        let raw: String = self.chars[start..self.pos].iter().collect();

        self.skip_whitespace();
        if self.peek() == Some('(') && sheet.is_none() {
            let name = if let Some(stripped) = raw.strip_prefix("_xlfn.") {
                stripped.to_string()
            } else {
                raw
            };
            self.advance();
            let args = self.arg_list()?;
            self.expect(')')?;
            return Ok(Expr::FunctionCall { name, args });
        }

        self.pos = start;
        if let Some(cell) = self.try_cell_ref(sheet, workbook) {
            return Ok(Expr::CellRef(cell));
        }

        Err(self.err(format!("unexpected identifier: {}", raw)))
    }

    fn try_cell_ref(&mut self, sheet: Option<String>, workbook: Option<u32>) -> Option<CellRef> {
        let save = self.pos;

        let abs_col = self.peek() == Some('$');
        if abs_col { self.advance(); }

        let col_start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_uppercase()) {
            self.advance();
        }
        if self.pos == col_start {
            self.pos = save;
            return None;
        }
        let col: String = self.chars[col_start..self.pos].iter().collect();

        let abs_row = self.peek() == Some('$');
        if abs_row { self.advance(); }

        let row_start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.advance();
        }
        if self.pos == row_start {
            self.pos = save;
            return None;
        }
        let row: u32 = self.chars[row_start..self.pos].iter().collect::<String>().parse().ok()?;

        Some(CellRef { col, row, abs_col, abs_row, sheet, workbook })
    }

    fn arg_list(&mut self) -> Result<Vec<Expr>, ParseError> {
        self.skip_whitespace();
        if self.peek() == Some(')') {
            return Ok(vec![]);
        }
        let mut args = vec![self.expression()?];
        loop {
            self.skip_whitespace();
            if self.peek() != Some(',') { break; }
            self.advance();
            args.push(self.expression()?);
        }
        Ok(args)
    }
}

pub fn parse(input: &str) -> Result<Expr, ParseError> {
    let mut parser = Parser::new(input);
    let expr = parser.expression()?;
    parser.skip_whitespace();
    if parser.pos != parser.chars.len() {
        return Err(parser.err(format!("unexpected trailing characters at position {}", parser.pos)));
    }
    Ok(expr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(v: f64) -> Expr { Expr::Number(v) }
    fn cell(col: &str, row: u32) -> Expr {
        Expr::CellRef(CellRef { col: col.to_string(), row, abs_col: false, abs_row: false, sheet: None, workbook: None })
    }
    fn abs_cell(col: &str, row: u32) -> Expr {
        Expr::CellRef(CellRef { col: col.to_string(), row, abs_col: true, abs_row: true, sheet: None, workbook: None })
    }
    fn binop(op: BinOp, l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp { op, left: Box::new(l), right: Box::new(r) }
    }
    fn func(name: &str, args: Vec<Expr>) -> Expr {
        Expr::FunctionCall { name: name.to_string(), args }
    }

    #[test]
    fn simple_if() {
        let expr = parse("IF(B2=0, 1, 0)").unwrap();
        assert_eq!(expr, func("IF", vec![
            binop(BinOp::Eq, cell("B", 2), num(0.0)),
            num(1.0),
            num(0.0),
        ]));
    }

    #[test]
    fn arithmetic_with_comparisons() {
        let expr = parse("(B2)*(F2=0)*B5 + (B2)*(F2)*D5 + (B2=0)*C8").unwrap();
        assert!(matches!(expr, Expr::BinaryOp { op: BinOp::Add, .. }));
    }

    #[test]
    fn complex_comparison_chain() {
        let expr = parse("(G8=3)*1 + (G8=0)*2 + (G8=2)*3 + OR(G8=1, G8=5, G8=6)*4 + (G8=8)*5 + (G8=9)*6").unwrap();
        assert!(matches!(expr, Expr::BinaryOp { op: BinOp::Add, .. }));
    }

    #[test]
    fn mod_function() {
        let expr = parse("MOD(B47, 8)").unwrap();
        assert_eq!(expr, func("MOD", vec![cell("B", 47), num(8.0)]));
    }

    #[test]
    fn xlfn_bitrshift() {
        let expr = parse("_xlfn.BITRSHIFT(C8, 8)").unwrap();
        assert_eq!(expr, func("BITRSHIFT", vec![cell("C", 8), num(8.0)]));
    }

    #[test]
    fn xlfn_floor_math() {
        let expr = parse("_xlfn.FLOOR.MATH(G40/16)").unwrap();
        assert_eq!(expr, func("FLOOR.MATH", vec![
            binop(BinOp::Div, cell("G", 40), num(16.0)),
        ]));
    }

    #[test]
    fn indirect_address() {
        let expr = parse("INDIRECT(ADDRESS(R11+13, 18))").unwrap();
        assert_eq!(expr, func("INDIRECT", vec![
            func("ADDRESS", vec![
                binop(BinOp::Add, cell("R", 11), num(13.0)),
                num(18.0),
            ]),
        ]));
    }

    #[test]
    fn absolute_refs() {
        let expr = parse("$R$5 + $R$11").unwrap();
        assert_eq!(expr, binop(BinOp::Add, abs_cell("R", 5), abs_cell("R", 11)));
    }

    #[test]
    fn not_function() {
        let expr = parse("NOT(D12=2)*J12").unwrap();
        assert_eq!(expr, binop(BinOp::Mul,
            func("NOT", vec![binop(BinOp::Eq, cell("D", 12), num(2.0))]),
            cell("J", 12),
        ));
    }

    #[test]
    fn and_function() {
        let expr = parse("AND(G8>=3, G8<=7, NOT(G8=4))*1").unwrap();
        if let Expr::BinaryOp { op: BinOp::Mul, left, right } = expr {
            assert_eq!(*right, num(1.0));
            if let Expr::FunctionCall { name, args } = *left {
                assert_eq!(name, "AND");
                assert_eq!(args.len(), 3);
            } else { panic!("expected function call"); }
        } else { panic!("expected mul"); }
    }

    #[test]
    fn or_function() {
        let expr = parse("OR(G8=1, G8=3, G8=5, G8=6)*1").unwrap();
        assert!(matches!(expr, Expr::BinaryOp { op: BinOp::Mul, .. }));
    }

    #[test]
    fn nested_not_mul() {
        let expr = parse("NOT((P7=1)*(P12=1))*NOT((P7=0)*(P10=1))*(R11)").unwrap();
        assert!(matches!(expr, Expr::BinaryOp { op: BinOp::Mul, .. }));
    }

    #[test]
    fn complex_additive_with_parens() {
        let expr = parse("(B19=0)*G22 + B19*(D19=0)*G19 + B19*(D19=1)*(G19+I19+M23)").unwrap();
        assert!(matches!(expr, Expr::BinaryOp { op: BinOp::Add, .. }));
    }

    #[test]
    fn deeply_nested_indirect() {
        let input = "INDIRECT(ADDRESS((L12<128)*(_xlfn.FLOOR.MATH(L12/16)+47) + (L12>=128)*(_xlfn.FLOOR.MATH((L12-128)/16)+47+(F40*8)), MOD(L12,16)+2))";
        let expr = parse(input).unwrap();
        assert!(matches!(expr, Expr::FunctionCall { .. }));
    }

    #[test]
    fn big_store_formula() {
        let input = "($B$36)*($N$36=0)*($J$36)*(ROW(B47)-47 = $H$40)*(COLUMN(B47)-2 = $I$40)*($J$40) + NOT(($B$36)*($N$36=0)*($J$36)*(ROW(B47)-47 = $H$40)*(COLUMN(B47)-2 = $I$40))*B47";
        let expr = parse(input).unwrap();
        assert!(matches!(expr, Expr::BinaryOp { op: BinOp::Add, .. }));
    }

    #[test]
    fn double_indirect_with_arithmetic() {
        let input = "INDIRECT(ADDRESS(_xlfn.FLOOR.MATH(D36/16)+114,MOD(D36,16)+2))*256+INDIRECT(ADDRESS(_xlfn.FLOOR.MATH((D36+1)/16)+114,MOD((D36+1),16)+2))";
        let expr = parse(input).unwrap();
        assert!(matches!(expr, Expr::BinaryOp { op: BinOp::Add, .. }));
    }

    #[test]
    fn row_column_complex() {
        let input = "($R$5 + ($R$11>(ROW(R13)-13)))*0 + (($R$5=0)*($P$7=0)*($R$11=(ROW(R13)-13))*($P$10))*$P$29";
        let expr = parse(input).unwrap();
        assert!(matches!(expr, Expr::BinaryOp { op: BinOp::Add, .. }));
    }

    #[test]
    fn unary_neg() {
        let expr = parse("-B2").unwrap();
        assert_eq!(expr, Expr::UnaryOp { op: UnOp::Neg, operand: Box::new(cell("B", 2)) });
    }

    #[test]
    fn number_decimal() {
        let expr = parse("0.5").unwrap();
        assert_eq!(expr, num(0.5));
    }

    #[test]
    fn not_equal() {
        let expr = parse("A1<>B1").unwrap();
        assert_eq!(expr, binop(BinOp::Neq, cell("A", 1), cell("B", 1)));
    }

    #[test]
    fn mixed_abs_ref() {
        let expr = parse("$B2 + B$2").unwrap();
        let left = Expr::CellRef(CellRef { col: "B".to_string(), row: 2, abs_col: true, abs_row: false, sheet: None, workbook: None });
        let right = Expr::CellRef(CellRef { col: "B".to_string(), row: 2, abs_col: false, abs_row: true, sheet: None, workbook: None });
        assert_eq!(expr, binop(BinOp::Add, left, right));
    }

    #[test]
    fn external_workbook_ref() {
        let expr = parse("[1]Sheet1!A3").unwrap();
        assert_eq!(expr, Expr::CellRef(CellRef {
            col: "A".to_string(), row: 3, abs_col: false, abs_row: false,
            sheet: Some("Sheet1".to_string()), workbook: Some(1),
        }));
    }
}
