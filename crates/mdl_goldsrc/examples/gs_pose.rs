//! `gs_pose <model.mdl> [sequence] [seconds]`: a GoldSrc model's bones and sequences, or the
//! model-space positions of every bone in `sequence` at `seconds`.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("usage: gs_pose <model.mdl> [sequence] [seconds]");
        std::process::exit(2);
    };
    let data = std::fs::read(path).expect("read model");
    let model = mdl_goldsrc::StudioModel::parse(&data).expect("parse model");
    let Some(label) = args.get(2) else {
        for (i, seq) in model.sequences.iter().enumerate() {
            println!(
                "seq {i} {} frames {} fps {}",
                seq.label, seq.num_frames, seq.fps
            );
        }
        return;
    };
    let seconds: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let sequence = model.sequence_index(label).expect("sequence");
    let mut world = Vec::new();
    model.pose(sequence, seconds, &mut world);
    for (bone, m) in model.bones.iter().zip(&world) {
        println!(
            "  {:24} [{:6.1}, {:6.1}, {:6.1}]",
            bone.name, m[0][3], m[1][3], m[2][3]
        );
    }
}
