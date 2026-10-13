//! `sv_showhitboxes 1`: the server's CS:GO hit boxes drawn over the players, as CS:GO's own
//! command does (a host sees its own server's): the head red, the rest green, as a see-through
//! grid so the body shows inside, nothing hiding them.

use std::f32::consts::PI;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use bevy::prelude::*;
use csgo_anim::hitbox::{HITGROUP_HEAD, HitShape};
use render_gpu::{
    CsViewmodelDraw, CsViewmodelModel, CsViewmodelShading, CsViewmodelVertex, CsWorldModelInstance,
};

use super::csgo_body::CsgoBodyDraws;

/// Segments around a capsule, and rings over each of its ends.
const AROUND: usize = 12;
const CAP_RINGS: usize = 3;
/// Grid cells around a capsule.
const AROUND_CELLS: f32 = 6.0;

fn vertex(position: Vec3, normal: Vec3, u: f32, v: f32) -> CsViewmodelVertex {
    CsViewmodelVertex {
        position: position.to_array(),
        normal: normal.to_array(),
        uv: [u, v],
        bones: [0; 4],
        weights: [255, 0, 0, 0],
        tangent: [0.0; 4],
    }
}

/// A capsule's surface as triangles: rings from the far pole of `a`'s end round to the far pole
/// of `b`'s.
fn capsule(out: &mut Vec<CsViewmodelVertex>, a: Vec3, b: Vec3, radius: f32) {
    let axis = (b - a).normalize_or(Vec3::Z);
    let side = axis.any_orthonormal_vector();
    let up = axis.cross(side);
    let mut rings: Vec<(Vec3, f32)> = Vec::new();
    for i in 0..=CAP_RINGS {
        rings.push((a, PI - PI / 2.0 * i as f32 / CAP_RINGS as f32));
    }
    for i in 0..=CAP_RINGS {
        rings.push((b, PI / 2.0 - PI / 2.0 * i as f32 / CAP_RINGS as f32));
    }
    let point = |(center, polar): (Vec3, f32), k: usize, v: f32| {
        let around = 2.0 * PI * k as f32 / AROUND as f32;
        let normal = axis * polar.cos() + (side * around.cos() + up * around.sin()) * polar.sin();
        (center + normal * radius, normal, AROUND_CELLS * k as f32 / AROUND as f32, v)
    };
    for (r, pair) in rings.windows(2).enumerate() {
        let (v0, v1) = (r as f32 * 0.5, (r + 1) as f32 * 0.5);
        for k in 0..AROUND {
            let p00 = point(pair[0], k, v0);
            let p01 = point(pair[0], k + 1, v0);
            let p10 = point(pair[1], k, v1);
            let p11 = point(pair[1], k + 1, v1);
            for (p, n, u, v) in [p00, p10, p11, p00, p11, p01] {
                out.push(vertex(p, n, u, v));
            }
        }
    }
}

/// A box's faces as triangles.
fn cuboid(out: &mut Vec<CsViewmodelVertex>, frame: Mat4, min: Vec3, max: Vec3) {
    let corner = |i: usize| {
        frame.transform_point3(Vec3::new(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        ))
    };
    for face in [[0, 2, 6, 4], [1, 5, 7, 3], [0, 4, 5, 1], [2, 3, 7, 6], [0, 1, 3, 2], [4, 6, 7, 5]] {
        let [p0, p1, p2, p3] = face.map(corner);
        let normal = (p1 - p0).cross(p2 - p0).normalize_or(Vec3::Z);
        for (p, u, v) in [(p0, 0.0, 0.0), (p1, 0.0, 1.0), (p2, 1.0, 1.0), (p0, 0.0, 0.0), (p2, 1.0, 1.0), (p3, 1.0, 0.0)] {
            out.push(vertex(p, normal, u, v));
        }
    }
}

/// An 8×8 grid cell: `rgb` on its first row and column, cut out inside.
fn grid_image(rgb: [u8; 3]) -> Image {
    let rgba = (0..64)
        .flat_map(|i| {
            let line = i % 8 == 0 || i / 8 == 0;
            [rgb[0], rgb[1], rgb[2], if line { 255 } else { 0 }]
        })
        .collect();
    super::csgo_body::image(8, 8, rgba)
}

/// The server's CS:GO hit boxes (this process's own server) as one overlay model.
pub fn draw_csgo_hitboxes(
    show: Res<frame::ShowHitboxes>,
    mut draws: ResMut<CsgoBodyDraws>,
    mut images: ResMut<Assets<Image>>,
    mut textures: Local<Option<[Handle<Image>; 2]>>,
) {
    sim::show_csgo_hitboxes(show.0);
    if !show.0 {
        return;
    }
    let hitboxes = sim::shown_csgo_hitboxes();
    if hitboxes.is_empty() {
        return;
    }
    let [head_image, body_image] = textures
        .get_or_insert_with(|| {
            [
                images.add(grid_image([255, 40, 40])),
                images.add(grid_image([40, 255, 80])),
            ]
        })
        .clone();
    let mut vertices = Vec::new();
    let mut draw_list = Vec::new();
    for head in [true, false] {
        let first = vertices.len();
        for hitbox in hitboxes.iter().filter(|h| (h.group == HITGROUP_HEAD) == head) {
            match hitbox.shape {
                HitShape::Capsule { a, b, radius } => capsule(&mut vertices, a, b, radius),
                HitShape::Box { frame, min, max } => cuboid(&mut vertices, frame, min, max),
            }
        }
        if vertices.len() > first {
            draw_list.push(CsViewmodelDraw {
                image: if head { head_image.clone() } else { body_image.clone() },
                first_vertex: first as u32,
                vertex_count: (vertices.len() - first) as u32,
                maps: None,
                shading: CsViewmodelShading::Fullbright,
            });
        }
    }
    let model = Arc::new(CsViewmodelModel {
        id: super::cs_viewmodel::next_model_id().fetch_add(1, Ordering::Relaxed),
        vertices,
        draws: draw_list,
    });
    draws.0.push(CsWorldModelInstance {
        model,
        world_from_model: Mat4::IDENTITY.to_cols_array_2d(),
        ambient: [1.0; 3],
        sun_dir: [0.0; 3],
        sun: [0.0; 3],
        bones: None,
        lighting_origin: None,
        overlay: true,
    });
}
