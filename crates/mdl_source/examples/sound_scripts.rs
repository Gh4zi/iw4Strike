//! `sound_scripts <pak_dir.vpk> [filter]`: a Source pack's sound script entries (those holding
//! `filter` when given) and their waves.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(pak) = args.get(1) else {
        eprintln!("usage: sound_scripts <pak_dir.vpk> [filter]");
        std::process::exit(2);
    };
    let vpk = mdl_source::Vpk::open(std::path::Path::new(pak)).expect("open pak");
    let scripts = mdl_source::SoundScripts::load(&vpk);
    let filter = args.get(2).map_or(String::new(), |f| f.to_ascii_lowercase());
    let mut names: Vec<&str> = scripts.names().filter(|n| n.contains(&filter)).collect();
    names.sort_unstable();
    for name in names {
        println!("{name}: {}", scripts.waves(name).join(" "));
    }
}
