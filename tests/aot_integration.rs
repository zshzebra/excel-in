#[cfg(feature = "jit")]
#[test]
fn aot_compile_produces_shared_library() {
    use std::path::Path;
    use excel_in::definition::*;

    let xlsx_path = Path::new(".reference/excelRISC-CPU/RISC-CPU.xlsx");
    if !xlsx_path.exists() {
        eprintln!("skipping AOT test: xlsx not found");
        return;
    }

    let eval = excel_in::load_spreadsheet(xlsx_path).unwrap();

    let def = Definition {
        functions: vec![FunctionDef {
            name: "cpu_tick".into(),
            inputs: vec![
                ParamDef {
                    name: "key_input".into(),
                    cells: vec![("F".into(), 2)],
                    api_type: ApiType::F64,
                },
                ParamDef {
                    name: "reset".into(),
                    cells: vec![("L".into(), 2)],
                    api_type: ApiType::Bool,
                },
            ],
            outputs: vec![ParamDef {
                name: "framebuffer".into(),
                cells: expand_cell_range("B95:Q110").unwrap(),
                api_type: ApiType::Array(ApiScalar::U8, 256),
            }],
            ticks: 1,
        }],
    };

    let output_path = std::env::temp_dir().join("test_spreadsheet.so");
    excel_in::aot::compile(&eval, &def, &output_path, 0).unwrap();

    assert!(output_path.exists(), ".so was not created");
    assert!(output_path.with_extension("h").exists(), ".h was not created");

    let metadata = std::fs::metadata(&output_path).unwrap();
    assert!(metadata.len() > 0, ".so is empty");

    let _ = std::fs::remove_file(&output_path);
    let _ = std::fs::remove_file(output_path.with_extension("h"));
}
