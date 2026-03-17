use excel_in::definition::*;
use std::path::Path;

fn parse_kdl_definition(input: &str) -> Result<Definition, String> {
    let doc: kdl::KdlDocument = input.parse().map_err(|e| format!("KDL parse error: {}", e))?;
    let mut functions = Vec::new();

    for node in doc.nodes() {
        if node.name().value() != "function" {
            continue;
        }
        let fn_name = node.entries().first()
            .and_then(|e| e.value().as_string())
            .ok_or("function node missing name")?
            .to_string();

        let children = node.children()
            .ok_or(format!("function '{}' has no children", fn_name))?;

        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let mut ticks = 1u32;

        for child in children.nodes() {
            match child.name().value() {
                "input" | "output" => {
                    let param_name = child.entries().first()
                        .and_then(|e| e.value().as_string())
                        .ok_or("param missing name")?
                        .to_string();

                    let type_str = child.get("type")
                        .and_then(|v| v.as_string())
                        .ok_or("param missing type")?;

                    let api_type = parse_api_type(type_str)?;

                    let cells = if let Some(cell) = child.get("cell").and_then(|v| v.as_string()) {
                        let (col, row) = parse_cell_ref(cell)
                            .ok_or(format!("invalid cell ref: {}", cell))?;
                        vec![(col, row)]
                    } else if let Some(range) = child.get("cells").and_then(|v| v.as_string()) {
                        expand_cell_range(range).map_err(|e| e.to_string())?
                    } else {
                        return Err(format!("param '{}' missing cell or cells", param_name));
                    };

                    if let ApiType::Array(_, expected) = api_type {
                        if cells.len() != expected {
                            return Err(format!(
                                "param '{}': array length {} doesn't match {} cells",
                                param_name, expected, cells.len()
                            ));
                        }
                    }

                    let param = ParamDef { name: param_name, cells, api_type };
                    if child.name().value() == "input" {
                        inputs.push(param);
                    } else {
                        outputs.push(param);
                    }
                }
                "ticks" => {
                    ticks = child.entries().first()
                        .and_then(|e| e.value().as_integer())
                        .ok_or("ticks missing value")? as u32;
                }
                _ => {}
            }
        }

        functions.push(FunctionDef { name: fn_name, inputs, outputs, ticks });
    }

    Ok(Definition { functions })
}

fn parse_api_type(s: &str) -> Result<ApiType, String> {
    match s {
        "f64" => Ok(ApiType::F64),
        "bool" => Ok(ApiType::Bool),
        "u8" => Ok(ApiType::U8),
        "i32" => Ok(ApiType::I32),
        "u32" => Ok(ApiType::U32),
        "i64" => Ok(ApiType::I64),
        "u64" => Ok(ApiType::U64),
        s if s.starts_with('[') => {
            let inner = s.trim_start_matches('[').trim_end_matches(']');
            let parts: Vec<&str> = inner.split(';').map(str::trim).collect();
            if parts.len() != 2 {
                return Err(format!("invalid array type: {}", s));
            }
            let scalar = match parts[0] {
                "f64" => ApiScalar::F64,
                "bool" => ApiScalar::Bool,
                "u8" => ApiScalar::U8,
                "i32" => ApiScalar::I32,
                "u32" => ApiScalar::U32,
                "i64" => ApiScalar::I64,
                "u64" => ApiScalar::U64,
                _ => return Err(format!("unknown scalar type: {}", parts[0])),
            };
            let len: usize = parts[1].parse()
                .map_err(|_| format!("invalid array length: {}", parts[1]))?;
            Ok(ApiType::Array(scalar, len))
        }
        _ => Err(format!("unknown type: {}", s)),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: excel-in-compile <file.xlsx> --def <api.kdl> [-o <output.so>] [-O0|-O1|-O2|-O3]");
        std::process::exit(1);
    }

    let xlsx_path = Path::new(&args[1]);

    let def_path = args.iter()
        .position(|a| a == "--def")
        .and_then(|i| args.get(i + 1))
        .map(Path::new)
        .unwrap_or_else(|| { eprintln!("--def required"); std::process::exit(1) });

    let output_path = args.iter()
        .position(|a| a == "-o")
        .and_then(|i| args.get(i + 1))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| xlsx_path.with_extension("so"));

    let opt_level: u8 = args.iter()
        .find(|a| a.starts_with("-O"))
        .and_then(|a| a[2..].parse().ok())
        .unwrap_or(2);

    eprintln!("loading {}...", xlsx_path.display());
    let eval = excel_in::load_spreadsheet(xlsx_path)?;

    eprintln!("parsing {}...", def_path.display());
    let kdl_text = std::fs::read_to_string(def_path)?;
    let def = parse_kdl_definition(&kdl_text)?;

    eprintln!("compiling to {} (-O{})...", output_path.display(), opt_level);
    excel_in::aot::compile(&eval, &def, &output_path, opt_level)?;

    let header_path = output_path.with_extension("h");
    eprintln!("wrote {} and {}", output_path.display(), header_path.display());

    Ok(())
}
