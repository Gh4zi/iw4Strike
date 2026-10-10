//! `s2pose <pak01_dir.vpk> <clip.vnmclip_c> [seconds]`: model-space positions of a character
//! clip's arm and spine joints, to see a third-person stance.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(pak), Some(path)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: s2pose <pak01_dir.vpk> <clip.vnmclip_c> [seconds]");
        std::process::exit(2);
    };
    let seconds: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let vpk = mdl_source::Vpk::open(std::path::Path::new(pak)).expect("open pak");
    let clip = mdl_source2::anim::load_clip(&vpk.read(path).expect("clip")).expect("decode clip");
    let skel_path = format!("{}_c", clip.skeleton);
    let skeleton = mdl_source2::anim::load_skeleton(&vpk.read(&skel_path).expect("skeleton"))
        .expect("decode skeleton");
    let world = mdl_source2::pose::world_matrices(
        &skeleton,
        &clip.sample(seconds),
        mdl_source2::pose::Mat3x4::IDENTITY,
    );
    println!(
        "{} on {} ({} bones)",
        path,
        clip.skeleton,
        skeleton.bones.len()
    );
    for name in [
        "pelvis",
        "spine_0",
        "spine_3",
        "neck_0",
        "head_0",
        "clavicle_L",
        "arm_upper_L",
        "arm_lower_L",
        "hand_L",
        "finger_middle_0_L",
        "finger_thumb_0_L",
        "clavicle_R",
        "arm_upper_R",
        "arm_lower_R",
        "hand_R",
        "finger_middle_0_R",
        "finger_thumb_0_R", "wpnPivot", "wpn", "wpnHand_R", "wpnHand_L", "wpnTip", "wpnEnd",
    ] {
        if let Some(b) = skeleton.bone(name) {
            let m = world[b].0;
            println!("  {name:20} [{:6.1}, {:6.1}, {:6.1}]", m[3], m[7], m[11]);
        }
    }
}
