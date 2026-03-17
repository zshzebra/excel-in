use excel_in::eval::{CellId, Evaluator};
use minifb::{Key, Window, WindowOptions};
use std::path::Path;
use std::time::{Duration, Instant};

const SCALE: usize = 20;
const WIDTH: usize = 16 * SCALE;
const HEIGHT: usize = 16 * SCALE;
const BLOCK6_ROW: u32 = 95;
const COLS: [&str; 16] = ["B","C","D","E","F","G","H","I","J","K","L","M","N","O","P","Q"];

const PALETTE: [u32; 64] = [
    0x000000, 0x222222, 0x444444, 0x666666, 0x999999, 0xbbbbbb, 0xdddddd, 0xffffff,
    0x220000, 0x440000, 0x660000, 0x780000, 0x9a0000, 0xbc0000, 0xde0000, 0xff0000,
    0x221100, 0x442200, 0x663300, 0x783c00, 0x9a4d00, 0xbc5e00, 0xde6f00, 0xff7f00,
    0x222200, 0x444400, 0x666600, 0x787800, 0x999a00, 0xbbbc00, 0xddde00, 0xffff00,
    0x002200, 0x004400, 0x006600, 0x007800, 0x009a00, 0x00bc00, 0x00de00, 0x00ff00,
    0x002222, 0x004444, 0x006666, 0x007878, 0x00999a, 0x00bbbc, 0x00ddde, 0x00feff,
    0x000022, 0x000044, 0x000066, 0x000078, 0x00009a, 0x0000bc, 0x0000de, 0x0000ff,
    0x220022, 0x440044, 0x660066, 0x780078, 0x9a0099, 0xbc00bb, 0xde00dd, 0xff00fe,
];

fn read_framebuffer(eval: &Evaluator) -> [u8; 256] {
    let mut fb = [0u8; 256];
    for block in 0..2u32 {
        for row_idx in 0..8u32 {
            for (col_idx, col) in COLS.iter().enumerate() {
                let cell_row = BLOCK6_ROW + block * 8 + row_idx;
                let id = CellId::local(col.to_string(), cell_row);
                let val = eval.get_value(&id) as i64;
                let color = (val.rem_euclid(64)) as u8;
                let px = (block * 8 + row_idx) as usize * 16 + col_idx;
                if px < 256 {
                    fb[px] = color;
                }
            }
        }
    }
    fb
}

fn render(fb: &[u8; 256], buffer: &mut [u32]) {
    for y in 0..16 {
        for x in 0..16 {
            let color = PALETTE[fb[y * 16 + x] as usize];
            for sy in 0..SCALE {
                for sx in 0..SCALE {
                    buffer[(y * SCALE + sy) * WIDTH + x * SCALE + sx] = color;
                }
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: excel-in-viewer <file.xlsx> [-O0|-O1|-O2|-O3]");
        std::process::exit(1);
    }

    let path = Path::new(&args[1]);
    let mut eval = excel_in::load_spreadsheet(path)?;

    let jit_opt: Option<u8> = args.iter()
        .find(|a| a.starts_with("-O"))
        .and_then(|a| a[2..].parse().ok());

    if let Some(level) = jit_opt {
        eprintln!("compiling JIT (-O{})...", level);
        let t = Instant::now();
        eval.compile_jit(level);
        eprintln!("JIT compiled in {:?}", t.elapsed());
    }

    eval.set_value(CellId::local("F".into(), 2), 0.0);
    eval.set_value(CellId::local("L".into(), 2), 1.0);
    for _ in 0..10 {
        if jit_opt.is_some() { eval.jit_tick(); } else { eval.tick(); }
    }
    eval.set_value(CellId::local("L".into(), 2), 0.0);

    let mut window = Window::new(
        "excel-in viewer",
        WIDTH, HEIGHT,
        WindowOptions::default(),
    )?;

    let mut buffer = vec![0u32; WIDTH * HEIGHT];
    let mut tick_count: u64 = 0;
    let start = Instant::now();
    let mut last_log = Instant::now();
    let mut last_render = Instant::now();
    let frame_interval = Duration::from_secs_f64(1.0 / 60.0);

    while window.is_open() && !window.is_key_down(Key::Escape) {
        if jit_opt.is_some() { eval.jit_tick(); } else { eval.tick(); }
        tick_count += 1;

        if last_render.elapsed() >= frame_interval {
            let fb = read_framebuffer(&eval);
            render(&fb, &mut buffer);
            window.update_with_buffer(&buffer, WIDTH, HEIGHT)?;
            last_render = Instant::now();
        }

        if last_log.elapsed() > Duration::from_secs(5) {
            let elapsed = start.elapsed().as_secs_f64();
            eprintln!("tick {} ({:.0} ticks/sec)", tick_count, tick_count as f64 / elapsed);
            last_log = Instant::now();
        }
    }

    Ok(())
}
