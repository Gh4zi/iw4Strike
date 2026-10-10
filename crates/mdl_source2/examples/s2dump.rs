//! `s2dump <pak01_dir.vpk> <path in the pack> [block] [depth]`: a compiled Source 2 file's
//! blocks, and one block (default `DATA`) as KeyValues3.

use mdl_source2::Resource;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(pack), Some(path)) = (args.first(), args.get(1)) else {
        eprintln!("usage: s2dump <pak01_dir.vpk> <path> [block] [depth]");
        return;
    };
    let block = args.get(2).map_or("DATA", String::as_str);
    let depth = args.get(3).and_then(|d| d.parse().ok()).unwrap_or(4);
    let vpk = mdl_source::Vpk::open(std::path::Path::new(pack)).expect("open pack");
    let bytes = vpk.read(path).expect("not in the pack");
    let resource = Resource::parse(&bytes).expect("resource");
    println!(
        "{path}: version {}, {} bytes",
        resource.version,
        bytes.len()
    );
    for (i, b) in resource.blocks.iter().enumerate() {
        println!("  [{i}] {} @{} {} bytes", b.kind_str(), b.offset, b.size);
    }
    for (id, name) in resource.external_refs() {
        println!("  ref {id:016x} {name}");
    }
    summary(path, &bytes);
    let kind: [u8; 4] = block.as_bytes().try_into().expect("four-letter block");
    if let Some((i, b)) = resource.blocks_of(&kind).next() {
        match mdl_source2::kv3::parse(resource.bytes(b)) {
            Ok(value) => {
                let value = match args.get(4) {
                    Some(path) => value
                        .path(path)
                        .cloned()
                        .unwrap_or(mdl_source2::Value::Null),
                    None => value,
                };
                println!("[{i}] {block} =\n{}", value.dump(depth));
            }
            Err(e) => println!("[{i}] {block}: {e}"),
        }
    }
}

/// What `load` makes of models and textures, printed after the raw dump.
#[allow(dead_code)]
fn summary(path: &str, bytes: &[u8]) {
    if path.ends_with(".vmdl_c") {
        match mdl_source2::model::load(bytes) {
            Ok(model) => {
                println!(
                    "model {}: {} bones, skeleton {:?}",
                    model.name,
                    model.bones.len(),
                    model.skeleton
                );
                for bone in &model.bones {
                    println!(
                        "  bone {} parent {:?} pos {:?} rot {:?}",
                        bone.name, bone.parent, bone.position, bone.rotation
                    );
                }
                for a in &model.attachments {
                    println!("  attachment {} on {} at {:?}", a.name, a.bone, a.offset);
                }
                for mesh in &model.meshes {
                    let verts: Vec<usize> = mesh.vertex_buffers.iter().map(Vec::len).collect();
                    let inds: Vec<usize> = mesh.index_buffers.iter().map(Vec::len).collect();
                    println!("  mesh {}: vertices {verts:?} indices {inds:?}", mesh.name);
                    for d in &mesh.draws {
                        println!(
                            "    draw {} start {} count {} vb {}",
                            d.material, d.start_index, d.index_count, d.vertex_buffer
                        );
                    }
                    if let Some(v) = mesh.vertex_buffers.first().and_then(|b| b.get(100)) {
                        println!("    vertex 100: {v:?}");
                    }
                    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                    for v in mesh.vertex_buffers.first().into_iter().flatten() {
                        for k in 0..3 {
                            lo[k] = lo[k].min(v.position[k]);
                            hi[k] = hi[k].max(v.position[k]);
                        }
                    }
                    println!("    bounds {lo:?} .. {hi:?}");
                }
            }
            Err(e) => println!("model: {e}"),
        }
    } else if path.ends_with(".vnmclip_c") {
        clip_summary(bytes);
        worst_step(bytes);
    } else if path.ends_with(".vmat_c") {
        material_summary(bytes);
    } else if path.ends_with(".vtex_c") {
        match mdl_source2::texture::load(bytes) {
            Ok(t) => println!(
                "texture {}x{} {:?} {} mips, level 0 {} bytes",
                t.width,
                t.height,
                t.format,
                t.mips.len(),
                t.mips[0].len()
            ),
            Err(e) => println!("texture: {e}"),
        }
    }
}

/// A material's parameters.
#[allow(dead_code)]
fn material_summary(bytes: &[u8]) {
    match mdl_source2::material::load(bytes) {
        Ok(m) => {
            println!("material {} shader {}", m.name, m.shader);
            for (k, v) in &m.textures {
                println!("  tex {k} = {v}");
            }
            for (k, v) in &m.vectors {
                println!("  vec {k} = {v:?}");
            }
            for (k, v) in &m.floats {
                println!("  float {k} = {v}");
            }
            for (k, v) in &m.ints {
                println!("  int {k} = {v}");
            }
        }
        Err(e) => println!("material: {e}"),
    }
}

/// A clip's frames: how smooth bones move between frames (a bad decode jumps).
#[allow(dead_code)]
fn clip_summary(bytes: &[u8]) {
    let clip = match mdl_source2::anim::load_clip(bytes) {
        Ok(c) => c,
        Err(e) => return println!("clip: {e}"),
    };
    for (label, c) in
        std::iter::once(("primary", &clip)).chain(clip.secondary.iter().map(|c| ("secondary", c)))
    {
        println!(
            "{label} clip on {}: {} frames, {:.3}s, {} bones",
            c.skeleton,
            c.frames,
            c.duration,
            c.bone_count()
        );
        let (mut worst_q, mut worst_t, mut worst_norm) = (0.0f32, 0.0f32, 0.0f32);
        let mut previous = c.frame(0);
        for f in 1..c.frames {
            let frame = c.frame(f);
            for (a, b) in previous.iter().zip(&frame) {
                let dot: f32 = (0..4).map(|i| a.rotation[i] * b.rotation[i]).sum();
                worst_q = worst_q.max(1.0 - dot.abs());
                let d: f32 = (0..3)
                    .map(|i| (a.translation[i] - b.translation[i]).powi(2))
                    .sum::<f32>()
                    .sqrt();
                worst_t = worst_t.max(d);
                let n: f32 = b.rotation.iter().map(|v| v * v).sum::<f32>().sqrt();
                worst_norm = worst_norm.max((n - 1.0).abs());
            }
            previous = frame;
        }
        println!(
            "  largest frame step: rotation 1-dot {worst_q:.5}, translation {worst_t:.3}; quaternion norm off by {worst_norm:.5}"
        );
        let mid = c.frame(c.frames / 2);
        for (i, t) in mid.iter().enumerate().take(6) {
            println!(
                "  bone {i} mid: t {:?} q {:?} s {}",
                t.translation, t.rotation, t.scale
            );
        }
    }
    for e in &clip.events {
        println!("  event {:.3}s {} {}", e.time, e.kind, e.name);
    }
}

/// The single largest rotation step of a clip: which bone, which frame, and the quaternions.
#[allow(dead_code)]
pub fn worst_step(bytes: &[u8]) {
    let Ok(clip) = mdl_source2::anim::load_clip(bytes) else {
        return;
    };
    let mut worst = (0.0f32, 0, 0);
    for f in 1..clip.frames {
        let (a, b) = (clip.frame(f - 1), clip.frame(f));
        for (i, (a, b)) in a.iter().zip(&b).enumerate() {
            let dot: f32 = (0..4).map(|k| a.rotation[k] * b.rotation[k]).sum();
            if 1.0 - dot.abs() > worst.0 {
                worst = (1.0 - dot.abs(), i, f);
            }
        }
    }
    let (_, bone, f) = worst;
    println!("worst: bone {bone} frame {f}");
    for g in f.saturating_sub(2)..(f + 2).min(clip.frames) {
        println!("  frame {g}: {:?}", clip.frame(g)[bone].rotation);
    }
}
