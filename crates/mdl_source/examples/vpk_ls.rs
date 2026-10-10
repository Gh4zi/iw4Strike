//! `vpk_ls <pak_dir.vpk> [filter]`: lists a Source pack's files, those holding `filter` when
//! given.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(pak) = args.get(1) else {
        eprintln!("usage: vpk_ls <pak_dir.vpk> [filter]");
        std::process::exit(2);
    };
    let vpk = mdl_source::Vpk::open(std::path::Path::new(pak)).expect("open pak");
    let filter = args.get(2).map_or("", String::as_str);
    let mut paths: Vec<&str> = vpk.paths().filter(|p| p.contains(filter)).collect();
    paths.sort_unstable();
    for path in paths {
        println!("{path}");
    }
}
