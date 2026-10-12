//! `mdl_probe <pak_dir.vpk> <model.mdl> [arms.mdl]`: loads a model the way the game does (with its
//! `.ani`, and arms bone-merged onto it when given), then prints its meshes, bones and sequences,
//! and how many bones each sequence moves.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(pak), Some(path)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: mdl_probe <pak_dir.vpk> <model.mdl>");
        std::process::exit(2);
    };
    let vpk = mdl_source::Vpk::open(std::path::Path::new(pak)).expect("open pak");
    let mut loaded = mdl_source::load_model(&vpk, path).expect("load model");
    if let Some(arms) = args.get(3) {
        let arms = mdl_source::load_model(&vpk, arms).expect("load arms");
        loaded.bone_merge(&arms, 128).expect("bone merge");
        let mut palette = Vec::new();
        loaded.model.pose(0, 0.0, &mut palette);
        println!("merged {}: {} extra slots, palette {}", arms.model.name, loaded.model.merged.len(), palette.len());
    }
    let model = &loaded.model;
    let textured = loaded.materials.iter().filter(|m| m.texture.is_some()).count();
    println!(
        "{}: {} meshes, {} vertices, {} materials ({textured} with a texture), {} attachments",
        model.name,
        model.meshes.len(),
        model.vertices.len(),
        model.materials.len(),
        model.attachments.len()
    );
    for (index, part) in model.body_parts.iter().enumerate() {
        let meshes: Vec<String> = model.meshes.iter().filter(|m| m.body_part == index).map(|m| format!("{}:{}", m.body_model, model.materials[m.material])).collect();
        println!("body part {index} {}: {:?} meshes {meshes:?}", part.name, part.models);
        let mut used = std::collections::BTreeMap::new();
        for mesh in model.meshes.iter().filter(|m| m.body_part == index) {
            for v in &model.vertices[mesh.first_vertex..mesh.first_vertex + mesh.vertex_count] {
                for (b, w) in v.bones.iter().zip(v.weights) {
                    if w > 0.0 { *used.entry(model.bones.get(usize::from(*b)).map_or("?", |b| b.name.as_str())).or_insert(0usize) += 1; }
                }
            }
        }
        println!("  bones used: {used:?}");
    }
    let attachments: Vec<&str> = model.attachments.iter().map(|a| a.name.as_str()).collect();
    println!("attachments: {attachments:?}");
    let names: Vec<&str> = model.bones.iter().map(|b| b.name.as_str()).collect();
    println!("{} bones: {}", names.len(), names.join(", "));
    for (index, seq) in model.sequences.iter().enumerate() {
        // Bones whose skinning matrix changes between the start and the middle of the sequence.
        let (mut start, mut middle) = (Vec::new(), Vec::new());
        model.pose(index, 0.0, &mut start);
        model.pose(index, seq.duration() * 0.5, &mut middle);
        let moving = start
            .iter()
            .zip(&middle)
            .filter(|(a, b)| a.iter().flatten().zip(b.iter().flatten()).any(|(x, y)| (x - y).abs() > 0.01))
            .count();
        println!(
            "{index:3} {:<28} {:<32} {:4} frames @ {:5.1} fps, {} events, {moving} bones move",
            seq.label,
            seq.activity,
            seq.num_frames,
            seq.fps,
            seq.events.len()
        );
    }
}
