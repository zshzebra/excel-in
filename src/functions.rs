pub fn call(name: &str, args: &[f64]) -> f64 {
    match name {
        "IF" => excel_if(args),
        "MOD" => excel_mod(args),
        "NOT" => excel_not(args),
        "OR" => excel_or(args),
        "AND" => excel_and(args),
        "ROW" => excel_row(args),
        "COLUMN" => excel_column(args),
        "FLOOR.MATH" => excel_floor_math(args),
        "BITRSHIFT" => excel_bitrshift(args),
        "ADDRESS" | "INDIRECT" => 0.0,
        _ => 0.0,
    }
}

pub fn excel_if(args: &[f64]) -> f64 {
    let cond = args.first().copied().unwrap_or(0.0);
    let then_val = args.get(1).copied().unwrap_or(0.0);
    let else_val = args.get(2).copied().unwrap_or(0.0);
    if cond != 0.0 { then_val } else { else_val }
}

pub fn excel_mod(args: &[f64]) -> f64 {
    let num = args.first().copied().unwrap_or(0.0);
    let div = args.get(1).copied().unwrap_or(1.0);
    if div == 0.0 {
        return 0.0;
    }
    let r = num % div;
    if r != 0.0 && r.signum() != div.signum() { r + div } else { r }
}

pub fn excel_not(args: &[f64]) -> f64 {
    let val = args.first().copied().unwrap_or(0.0);
    if val == 0.0 { 1.0 } else { 0.0 }
}

pub fn excel_or(args: &[f64]) -> f64 {
    if args.iter().any(|&v| v != 0.0) { 1.0 } else { 0.0 }
}

pub fn excel_and(args: &[f64]) -> f64 {
    if args.is_empty() {
        return 0.0;
    }
    if args.iter().all(|&v| v != 0.0) { 1.0 } else { 0.0 }
}

pub fn excel_row(args: &[f64]) -> f64 {
    args.first().copied().unwrap_or(0.0)
}

pub fn excel_column(args: &[f64]) -> f64 {
    args.first().copied().unwrap_or(0.0)
}

pub fn excel_floor_math(args: &[f64]) -> f64 {
    let num = args.first().copied().unwrap_or(0.0);
    let sig = args.get(1).copied().unwrap_or(1.0);
    if sig == 0.0 {
        return 0.0;
    }
    (num / sig).floor() * sig
}

pub fn excel_bitrshift(args: &[f64]) -> f64 {
    let num = args.first().copied().unwrap_or(0.0) as i64;
    let shift = args.get(1).copied().unwrap_or(0.0) as u32;
    (num >> shift) as f64
}

pub fn excel_address(_args: &[f64]) -> f64 {
    0.0
}

pub fn excel_indirect(_args: &[f64]) -> f64 {
    0.0
}
