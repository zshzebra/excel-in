use excel_in::eval::CellId;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: excel-in <file.xlsx> [--ticks N] [--set COL ROW VAL]...");
        std::process::exit(1);
    }

    let path = Path::new(&args[1]);
    let ticks: u64 = args.iter()
        .position(|a| a == "--ticks")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let mut eval = excel_in::load_spreadsheet(path)?;

    let mut i = 1;
    while i < args.len() {
        if args[i] == "--set" && i + 3 < args.len() {
            let col = args[i + 1].clone();
            let row: u32 = args[i + 2].parse().unwrap();
            let val: f64 = args[i + 3].parse().unwrap();
            eval.set_value(CellId::local(col, row), val);
            i += 4;
        } else {
            i += 1;
        }
    }

    eprintln!("running {} ticks...", ticks);
    let start = std::time::Instant::now();
    for _ in 0..ticks {
        eval.tick();
    }
    let elapsed = start.elapsed();

    eprintln!("done in {:?} ({:.0} ticks/sec)", elapsed, ticks as f64 / elapsed.as_secs_f64());

    Ok(())
}
