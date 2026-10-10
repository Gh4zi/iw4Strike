//! `mdl_seqs <pak_dir.vpk> <model.mdl> [filter]`: a Source model's bones and sequences (label,
//! activity, frames, fps), those whose label holds `filter` when given.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(pak), Some(path)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: mdl_seqs <pak_dir.vpk> <model.mdl> [filter]");
        std::process::exit(2);
    };
    let vpk = mdl_source::Vpk::open(std::path::Path::new(pak)).expect("open pak");
    let mdl = vpk.read(path).expect("model in the pak");
    let model = mdl_source::StudioModel::parse(&mdl, &[], &[]).expect("parse model");
    let filter = args.get(3).map_or("", String::as_str);
    if let Some(seq) = filter.strip_prefix("desc:") {
        // desc:<sequence index>: the raw animation descriptor its first animation uses.
        let seq: usize = seq.parse().expect("sequence index");
        let i32_at = |at: usize| i32::from_le_bytes(mdl[at..at + 4].try_into().unwrap());
        let i16_at = |at: usize| i16::from_le_bytes(mdl[at..at + 2].try_into().unwrap());
        let seq_base = i32_at(192) as usize;
        let seq_at = seq_base + seq * 212;
        let groupsize = (i32_at(seq_at + 68), i32_at(seq_at + 72));
        let anim_slot = seq_at + i32_at(seq_at + 60) as usize;
        let anim = i16_at(anim_slot) as usize;
        let desc = i32_at(184) as usize + anim * 100;
        println!(
            "seq {seq} groupsize {groupsize:?} anim {anim}: flags {:#x} frames {} animblock {} animindex {} sectionindex {} sectionframes {}",
            i32_at(desc + 12),
            i32_at(desc + 16),
            i32_at(desc + 52),
            i32_at(desc + 56),
            i32_at(desc + 80),
            i32_at(desc + 84)
        );
        return;
    }
    if let Some(spec) = filter.strip_prefix("layer:") {
        // layer:<base sequence>+<delta sequence>: the base's first frame with the delta
        // sequence's centre blend composed on, as CS:S layers its aim matrix.
        let (base, delta) = spec.split_once('+').expect("base+delta");
        let find = |label: &str| {
            model
                .sequences
                .iter()
                .find(|s| s.label == label)
                .expect(label)
        };
        let base = find(base);
        let delta = find(delta);
        let mut locals = model.animations[base.blends[0]].frame(0).to_vec();
        let centre = delta.blends[delta.blends.len() / 2];
        let d = &model.animations[centre];
        println!("delta anim {centre} delta={}", d.delta);
        for (l, d) in locals.iter_mut().zip(d.frame(0)) {
            l.rotation = quat_mul(l.rotation, d.rotation);
            for i in 0..3 {
                l.position[i] += d.position[i];
            }
        }
        let world = model.world_from_locals(&locals);
        let rest = world_bones(&model, None);
        for name in [
            "ValveBiped.Bip01_Pelvis",
            "ValveBiped.Bip01_Spine4",
            "ValveBiped.Bip01_Neck1",
            "ValveBiped.Bip01_R_Clavicle",
            "ValveBiped.Bip01_R_UpperArm",
            "ValveBiped.Bip01_R_Forearm",
            "ValveBiped.Bip01_R_Hand",
            "ValveBiped.Bip01_L_Clavicle",
            "ValveBiped.Bip01_L_UpperArm",
            "ValveBiped.Bip01_L_Forearm",
            "ValveBiped.Bip01_L_Hand",
        ] {
            let Some(b) = model.bones.iter().position(|bone| bone.name == name) else {
                continue;
            };
            let p = |m: &[[f32; 4]; 3]| [m[0][3], m[1][3], m[2][3]];
            println!(
                "  {name:32} ref {:6.1?} layered {:6.1?}",
                p(&rest[b]),
                p(&world[b])
            );
        }
        return;
    }
    if let Some(probe) = filter.strip_prefix("probe:") {
        // probe:<sequence label>: rest and posed positions of the arm joints.
        let sequence = model.sequences.iter().position(|s| s.label == probe);
        let rest = world_bones(&model, None);
        let posed = world_bones(&model, sequence);
        println!("probe {probe} -> {sequence:?}");
        for name in [
            "ValveBiped.Bip01_Pelvis",
            "ValveBiped.Bip01_Spine4",
            "ValveBiped.Bip01_Head1",
            "ValveBiped.Bip01_R_Clavicle",
            "ValveBiped.Bip01_R_UpperArm",
            "ValveBiped.Bip01_R_Forearm",
            "ValveBiped.Bip01_R_Hand",
            "ValveBiped.Bip01_L_Clavicle",
            "ValveBiped.Bip01_L_UpperArm",
            "ValveBiped.Bip01_L_Forearm",
            "ValveBiped.Bip01_L_Hand",
        ] {
            let Some(b) = model.bones.iter().position(|bone| bone.name == name) else {
                continue;
            };
            let p = |m: &[[f32; 4]; 3]| [m[0][3], m[1][3], m[2][3]];
            println!(
                "  {name:32} rest {:6.1?} posed {:6.1?}",
                p(&rest[b]),
                p(&posed[b])
            );
        }
        return;
    }
    println!("{} bones", model.bones.len());
    for (i, bone) in model.bones.iter().enumerate() {
        println!("  bone {i} {} parent {:?}", bone.name, bone.parent);
    }
    for (i, seq) in model.sequences.iter().enumerate() {
        if seq.label.contains(filter) {
            println!(
                "  seq {i} {} [{}] frames {} fps {}",
                seq.label, seq.activity, seq.num_frames, seq.fps
            );
        }
    }
}

/// World (model-space) bone matrices of `model` at its rest pose, or at `sequence`'s first frame.
#[allow(dead_code)]
pub fn world_bones(model: &mdl_source::StudioModel, sequence: Option<usize>) -> Vec<[[f32; 4]; 3]> {
    let bind: Vec<[[f32; 4]; 3]> = model
        .bones
        .iter()
        .map(|b| invert(&b.pose_to_bone))
        .collect();
    let Some(sequence) = sequence else {
        return bind;
    };
    let mut skin = Vec::new();
    model.pose(sequence, 0.0, &mut skin);
    skin.iter()
        .zip(&bind)
        .map(|(s, b)| mdl_goldsrc::concat(s, b))
        .collect()
}

fn invert(m: &[[f32; 4]; 3]) -> [[f32; 4]; 3] {
    let r = |i: usize, j: usize| m[j][i];
    let t = [m[0][3], m[1][3], m[2][3]];
    let mut out = [[0.0f32; 4]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = r(i, j);
        }
        out[i][3] = -(r(i, 0) * t[0] + r(i, 1) * t[1] + r(i, 2) * t[2]);
    }
    out
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}
