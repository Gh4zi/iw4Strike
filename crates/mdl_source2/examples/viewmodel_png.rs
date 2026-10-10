//! `viewmodel_png <pak01_dir.vpk> <gun model> <clip> <seconds> <out.png>`: poses CS2's default
//! arms and a gun from a viewmodel clip and rasterises them from the eye, flat shaded, to check
//! the skeletons, the gun hanging under `wpn` and the skinning.

use mdl_source2::anim;
use mdl_source2::model;
use mdl_source2::pose::{self, Mat3x4, Pose};

const W: usize = 960;
const H: usize = 540;
const FOV_DEGREES: f32 = 68.0;

fn read(vpk: &mdl_source::Vpk, path: &str) -> Vec<u8> {
    vpk.read(path)
        .unwrap_or_else(|| panic!("{path} not in the pack"))
}

/// Skinned position and normal of every vertex of a vertex buffer.
fn posed(vb: &[model::Vertex], skin: &[Mat3x4]) -> Vec<([f32; 3], [f32; 3])> {
    vb.iter()
        .map(|v| {
            let (mut p, mut n) = ([0.0f32; 3], [0.0f32; 3]);
            for k in 0..4 {
                if v.weights[k] <= 0.0 {
                    continue;
                }
                let m = skin
                    .get(usize::from(v.bones[k]))
                    .copied()
                    .unwrap_or(Mat3x4::IDENTITY);
                let q = m.transform_point(v.position);
                let r = m.transform_vector(v.normal);
                for c in 0..3 {
                    p[c] += q[c] * v.weights[k];
                    n[c] += r[c] * v.weights[k];
                }
            }
            (p, n)
        })
        .collect()
}

struct Canvas {
    colour: Vec<[u8; 3]>,
    depth: Vec<f32>,
    focal: f32,
}

impl Canvas {
    /// Eye space is x forward, y left, z up.
    fn project(&self, p: [f32; 3]) -> Option<(f32, f32, f32)> {
        (p[0] > 1.0).then(|| {
            (
                W as f32 / 2.0 - p[1] / p[0] * self.focal,
                H as f32 / 2.0 - p[2] / p[0] * self.focal,
                p[0],
            )
        })
    }

    fn triangle(
        &mut self,
        a: (f32, f32, f32),
        b: (f32, f32, f32),
        c: (f32, f32, f32),
        rgb: [u8; 3],
    ) {
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area.abs() < 1e-6 {
            return;
        }
        let min_x = a.0.min(b.0).min(c.0).floor().max(0.0) as usize;
        let max_x = (a.0.max(b.0).max(c.0).ceil().max(0.0) as usize).min(W - 1);
        let min_y = a.1.min(b.1).min(c.1).floor().max(0.0) as usize;
        let max_y = (a.1.max(b.1).max(c.1).ceil().max(0.0) as usize).min(H - 1);
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((b.0 - px) * (c.1 - py) - (b.1 - py) * (c.0 - px)) / area;
                let w1 = ((c.0 - px) * (a.1 - py) - (c.1 - py) * (a.0 - px)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * a.2 + w1 * b.2 + w2 * c.2;
                let at = y * W + x;
                if z < self.depth[at] {
                    self.depth[at] = z;
                    self.colour[at] = rgb;
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let vpk = mdl_source::Vpk::open(std::path::Path::new(&args[0])).expect("open pack");
    let gun = model::load(&read(&vpk, &args[1])).expect("gun model");
    let arms =
        model::load(&read(&vpk, "weapons/models/shared/arms/weapon_arms.vmdl_c")).expect("arms");
    let clip = anim::load_clip(&read(&vpk, &args[2])).expect("clip");
    let seconds: f32 = args[3].parse().unwrap_or(0.0);
    let arms_skeleton =
        anim::load_skeleton(&read(&vpk, &format!("{}_c", clip.skeleton))).expect("arms skeleton");
    let mut pose = Pose::default();
    let arms_world = pose::world_matrices(&arms_skeleton, &clip.sample(seconds), Mat3x4::IDENTITY);
    pose.add(&arms_skeleton, &arms_world);
    for secondary in &clip.secondary {
        let skeleton = anim::load_skeleton(&read(&vpk, &format!("{}_c", secondary.skeleton)))
            .expect("gun skeleton");
        let attach = arms_skeleton
            .secondary
            .iter()
            .find(|(s, _)| *s == secondary.skeleton)
            .map_or("wpn", |(_, bone)| bone.as_str());
        let root = arms_skeleton
            .bone(attach)
            .map_or(Mat3x4::IDENTITY, |i| arms_world[i]);
        let world = pose::world_matrices(&skeleton, &secondary.sample(seconds), root);
        pose.add(&skeleton, &world);
    }
    let mut canvas = Canvas {
        colour: vec![[40, 44, 52]; W * H],
        depth: vec![f32::MAX; W * H],
        focal: (W as f32 / 2.0) / (FOV_DEGREES.to_radians() / 2.0).tan(),
    };
    let light = {
        let l = [-0.4f32, 0.5, 0.75];
        let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        l.map(|v| v / n)
    };
    let mut bounds = ([f32::MAX; 3], [f32::MIN; 3]);
    for (model, tint) in [
        (&arms, [230.0f32, 150.0, 110.0]),
        (&gun, [150.0, 160.0, 175.0]),
    ] {
        let skin = pose::skinning_matrices(model, &pose);
        for mesh in &model.meshes {
            let buffers: Vec<_> = mesh
                .vertex_buffers
                .iter()
                .map(|vb| posed(vb, &skin))
                .collect();
            for (p, _) in buffers.iter().flatten() {
                for (c, v) in p.iter().enumerate() {
                    bounds.0[c] = bounds.0[c].min(*v);
                    bounds.1[c] = bounds.1[c].max(*v);
                }
            }
            for draw in &mesh.draws {
                let (Some(vertices), Some(ib)) = (
                    buffers.get(draw.vertex_buffer),
                    mesh.index_buffers.get(draw.index_buffer),
                ) else {
                    continue;
                };
                let start = draw.start_index as usize;
                let end = (start + draw.index_count as usize).min(ib.len());
                for tri in ib[start..end].as_chunks::<3>().0 {
                    let v: Vec<([f32; 3], [f32; 3])> = tri
                        .iter()
                        .filter_map(|&i| {
                            vertices.get((i64::from(i) + i64::from(draw.base_vertex)) as usize)
                        })
                        .copied()
                        .collect();
                    if v.len() != 3 {
                        continue;
                    }
                    let (Some(a), Some(b), Some(c)) = (
                        canvas.project(v[0].0),
                        canvas.project(v[1].0),
                        canvas.project(v[2].0),
                    ) else {
                        continue;
                    };
                    let n = v[0].1;
                    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
                    let shade = 0.35
                        + 0.65
                            * ((n[0] * light[0] + n[1] * light[1] + n[2] * light[2]) / len)
                                .max(0.0);
                    canvas.triangle(a, b, c, tint.map(|t| (t * shade).min(255.0) as u8));
                }
            }
        }
    }
    println!("posed bounds {:?} .. {:?}", bounds.0, bounds.1);
    if let Some(m) = pose.get("wpn") {
        println!("wpn at {:?}", m.transform_point([0.0; 3]));
    }
    let file = std::fs::File::create(&args[4]).expect("out");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), W as u32, H as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png");
    writer
        .write_image_data(&canvas.colour.iter().flatten().copied().collect::<Vec<u8>>())
        .expect("png data");
}
