//! `vpk_cat <pak_dir.vpk> <path> [out]`: one file out of a Source pack, to `out` or stdout.

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(pak), Some(path)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: vpk_cat <pak_dir.vpk> <path> [out]");
        std::process::exit(2);
    };
    let vpk = mdl_source::Vpk::open(std::path::Path::new(pak)).expect("open pak");
    let bytes = vpk.read(path).expect("file in the pak");
    match args.get(3) {
        Some(out) => std::fs::write(out, &bytes).expect("write"),
        None => std::io::stdout().write_all(&bytes).expect("stdout"),
    }
}
