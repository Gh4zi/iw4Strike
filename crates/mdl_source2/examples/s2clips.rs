//! `s2clips <pak01_dir.vpk> <gun model> <viewmodel graph>`: loads a CS2 viewmodel the way the
//! game does and lists its clips (frames, duration, secondary animations), with the largest bone
//! offset and any non-finite skinning matrix at the start, middle and end of each.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, pak, model, graph, ..] = args.as_slice() else {
        eprintln!("usage: s2clips <pak01_dir.vpk> <gun model> <viewmodel graph>");
        std::process::exit(2);
    };
    let vpk = mdl_source::Vpk::open(std::path::Path::new(pak)).expect("open pak");
    let viewmodel = mdl_source2::viewmodel::load(&vpk, model, graph).expect("load viewmodel");
    println!(
        "arms {} bones, weapon {} bones, {} secondary skeletons",
        viewmodel.arms.bones.len(),
        viewmodel.weapon.bones.len(),
        viewmodel.secondary_skeletons.len()
    );
    meshes(&viewmodel.weapon);
    if let Some(prefix) = args.get(4) {
        match prefix.split_once(':') {
            Some((clip, bone)) => weapon_bone(&viewmodel, clip, bone),
            None => root_motion(&viewmodel, prefix),
        }
        return;
    }
    for (index, named) in viewmodel.clips.iter().enumerate() {
        let clip = &named.clip;
        print!(
            "[{index}] {} frames {} duration {:.3} additive {} secondary {} |",
            named.name,
            clip.frames,
            clip.duration,
            clip.additive,
            clip.secondary.len()
        );
        for t in [0.0, clip.duration * 0.5, clip.duration] {
            let skin = viewmodel.skin(index, t);
            let bad = skin
                .iter()
                .filter(|m| m.0.iter().any(|v| !v.is_finite()))
                .count();
            let far = skin
                .iter()
                .map(|m| m.0[3].abs().max(m.0[7].abs()).max(m.0[11].abs()))
                .fold(0.0f32, f32::max);
            print!(" t{t:.2}: far {far:.0} bad {bad}");
        }
        println!();
    }
}

/// Each mesh of the weapon: vertices, materials and the bones its vertices ride.
#[allow(dead_code)]
fn meshes(model: &mdl_source2::model::Model) {
    for (index, mesh) in model.meshes.iter().enumerate() {
        let mut bones: Vec<u16> = mesh
            .vertex_buffers
            .iter()
            .flatten()
            .flat_map(|v| {
                v.bones
                    .iter()
                    .zip(&v.weights)
                    .filter(|(_, w)| **w > 0.0)
                    .map(|(b, _)| *b)
            })
            .collect();
        bones.sort_unstable();
        bones.dedup();
        let names: Vec<&str> = bones
            .iter()
            .filter_map(|b| model.bones.get(usize::from(*b)).map(|b| b.name.as_str()))
            .collect();
        let materials: Vec<&str> = mesh.draws.iter().map(|d| d.material.as_str()).collect();
        println!(
            "mesh {index} {}: {} vertices, materials {materials:?}, bones {names:?}",
            mesh.name,
            mesh.vertex_buffers.iter().map(Vec::len).sum::<usize>()
        );
    }
}

/// The arms skeleton's root bone (\`root_motion\`) through a clip: how far the whole viewmodel moves.
#[allow(dead_code)]
fn root_motion(viewmodel: &mdl_source2::viewmodel::Viewmodel, prefix: &str) {
    let Some(index) = viewmodel.find(prefix, |_| true) else {
        return;
    };
    let clip = &viewmodel.clips[index].clip;
    println!("root_motion through {}:", viewmodel.clips[index].name);
    let steps = 12;
    for step in 0..=steps {
        let t = clip.duration * step as f32 / steps as f32;
        let locals = clip.sample(t);
        if let Some(root) = locals.first() {
            println!(
                "  t{t:.2} pos [{:.2}, {:.2}, {:.2}] rot [{:.3}, {:.3}, {:.3}, {:.3}]",
                root.translation[0],
                root.translation[1],
                root.translation[2],
                root.rotation[0],
                root.rotation[1],
                root.rotation[2],
                root.rotation[3]
            );
        }
    }
}

/// A weapon skeleton bone through a clip (`clip-prefix:bone`).
#[allow(dead_code)]
fn weapon_bone(viewmodel: &mdl_source2::viewmodel::Viewmodel, prefix: &str, bone: &str) {
    let Some(index) = viewmodel.find(prefix, |_| true) else {
        return;
    };
    let clip = &viewmodel.clips[index].clip;
    for secondary in &clip.secondary {
        let Some(skeleton) = viewmodel
            .secondary_skeletons
            .iter()
            .find(|s| s.id == secondary.skeleton)
        else {
            continue;
        };
        let Some(b) = skeleton.bone(bone) else {
            continue;
        };
        println!("{bone} through {}:", viewmodel.clips[index].name);
        let steps = 12;
        for step in 0..=steps {
            let t = clip.duration * step as f32 / steps as f32;
            if let Some(local) = secondary.sample(t).get(b) {
                println!(
                    "  t{t:.2} pos [{:.2}, {:.2}, {:.2}] scale {:.3}",
                    local.translation[0], local.translation[1], local.translation[2], local.scale
                );
            }
        }
    }
}
