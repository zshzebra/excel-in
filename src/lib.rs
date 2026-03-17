pub mod definition;
pub mod eval;
pub mod functions;
#[cfg(feature = "jit")]
pub mod jit;
pub mod parser;
pub mod xlsx;

use eval::{CellId, Evaluator};
use std::path::Path;

fn coord_to_cell_id(coord: &str, workbook: Option<u32>) -> Option<CellId> {
    let mut col = String::new();
    let mut row_str = String::new();
    for ch in coord.chars() {
        if ch.is_ascii_alphabetic() {
            col.push(ch);
        } else if ch.is_ascii_digit() {
            row_str.push(ch);
        }
    }
    let row: u32 = row_str.parse().ok()?;
    if col.is_empty() { return None; }
    match workbook {
        Some(wb) => Some(CellId::external(wb, col, row)),
        None => Some(CellId::local(col, row)),
    }
}

fn load_workbook_cells(path: &Path, workbook: Option<u32>, eval: &mut Evaluator) -> Result<(), Box<dyn std::error::Error>> {
    let wb = xlsx::load_xlsx(path)?;
    let mut formula_count = 0;
    let mut value_count = 0;
    let mut parse_errors = 0;

    for sheet in &wb.sheets {
        for cell in &sheet.cells {
            let Some(id) = coord_to_cell_id(&cell.coord, workbook) else { continue };

            if let Some(ref f) = cell.formula {
                match parser::parse(f) {
                    Ok(expr) => {
                        eval.add_cell(id, expr);
                        formula_count += 1;
                    }
                    Err(e) => {
                        eprintln!("parse error in {}: {} (formula: {})", cell.coord, e, f);
                        parse_errors += 1;
                    }
                }
            } else if let Some(ref v) = cell.value {
                if let Ok(n) = v.parse::<f64>() {
                    eval.set_value(id, n);
                    value_count += 1;
                }
            }
        }
    }

    let label = match workbook {
        Some(n) => format!("[{}] {}", n, path.display()),
        None => path.display().to_string(),
    };
    eprintln!("{}: {} formulas, {} values, {} parse errors", label, formula_count, value_count, parse_errors);
    Ok(())
}

fn resolve_external_workbooks(path: &Path) -> Vec<(u32, std::path::PathBuf)> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut externals = Vec::new();

    let Ok(file) = std::fs::File::open(path) else { return externals };
    let Ok(mut archive) = zip::ZipArchive::new(file) else { return externals };

    let mut xml = String::new();
    use std::io::Read;
    if let Ok(mut f) = archive.by_name("xl/externalLinks/_rels/externalLink1.xml.rels") {
        let _ = f.read_to_string(&mut xml);
        for target in extract_targets(&xml) {
            if target.starts_with("file:///") { continue; }
            let ext_path = dir.join(&target);
            if ext_path.exists() {
                externals.push((1, ext_path));
                break;
            }
        }
    }

    externals
}

fn extract_targets(xml: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut search = xml;
    while let Some(start) = search.find("Target=\"") {
        let rest = &search[start + 8..];
        if let Some(end) = rest.find('"') {
            targets.push(rest[..end].to_string());
            search = &rest[end..];
        } else {
            break;
        }
    }
    targets
}

pub fn load_spreadsheet(path: &Path) -> Result<Evaluator, Box<dyn std::error::Error>> {
    let mut eval = Evaluator::new();

    let externals = resolve_external_workbooks(path);
    for (wb_num, ext_path) in &externals {
        eprintln!("loading external workbook [{}]: {}", wb_num, ext_path.display());
        load_workbook_cells(ext_path, Some(*wb_num), &mut eval)?;
    }

    load_workbook_cells(path, None, &mut eval)?;

    eprintln!("total: {} formulas, {} values", eval.formula_count(), eval.value_count());
    eprintln!("building eval order...");
    eval.build_eval_order();

    Ok(eval)
}
