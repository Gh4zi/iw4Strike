use bevy::prelude::*;
use net::AuthorityWorld;

use crate::ConsoleCommand;

use super::echo::ConsoleEcho;

pub(crate) fn route_hitvol_commands(
    mut events: MessageReader<ConsoleCommand>,
    mut echo: ConsoleEcho,
    mut authority: Option<ResMut<AuthorityWorld>>,
) {
    for cmd in events.read() {
        if !matches!(
            cmd.name.as_str(),
            "hitvol" | "ladders" | "smodels" | "cylinders" | "destructibles"
        ) {
            continue;
        }
        let Some(authority) = authority.as_deref_mut() else {
            echo.write(format!("{}: no authority world on this client", cmd.name));
            continue;
        };
        let lines = match cmd.name.as_str() {
            "ladders" => ladder_report(&authority.0),
            "smodels" => smodel_report(&authority.0, cmd.args.first().map_or("", String::as_str)),
            "cylinders" => cylinder_report(&authority.0),
            "destructibles" => {
                let spots = authority.0.destructible_spots();
                let mut lines = vec![format!(
                    "destructibles: {} (damage {})",
                    spots.len(),
                    if sim::cs_settings::destructibles_enabled() { "on" } else { "off" }
                )];
                lines.extend(spots.iter().map(|(name, model, o)| {
                    format!("destructible {name} `{model}` at ({:.0} {:.0} {:.0})", o[0], o[1], o[2])
                }));
                lines
            }
            _ => hitvol_report(&authority.0),
        };
        for line in lines {
            echo.write(line);
        }
    }
}

/// Big pipes in the collision mesh: long triangles sharing an axis direction whose normals turn
/// around it, grouped per axis line. Prints each group's extent and how far its normals spread.
fn cylinder_report(world: &sim::SimWorld) -> Vec<String> {
    let mesh = &world.clip_mesh().tables;
    let vert = |i: usize| mesh.tri_indices.get(i).and_then(|&v| mesh.verts.get(usize::from(v)));
    struct Group {
        dir: [f32; 3],
        mins: [f32; 3],
        maxs: [f32; 3],
        tris: usize,
        nz_min: f32,
        nz_max: f32,
        contents: u32,
    }
    let mut groups: Vec<Group> = Vec::new();
    for tri in 0..mesh.tri_indices.len() / 3 {
        let (Some(a), Some(b), Some(c)) = (vert(tri * 3), vert(tri * 3 + 1), vert(tri * 3 + 2)) else {
            continue;
        };
        let edges = [(a, b), (b, c), (c, a)];
        let (p, q) = edges
            .iter()
            .copied()
            .max_by(|x, y| {
                let l = |e: (&[f32; 3], &[f32; 3])| (0..3).map(|i| (e.1[i] - e.0[i]).powi(2)).sum::<f32>();
                l(*x).total_cmp(&l(*y))
            })
            .expect("three edges");
        let d: [f32; 3] = core::array::from_fn(|i| q[i] - p[i]);
        let len = d.iter().map(|v| v * v).sum::<f32>().sqrt();
        if len < 64.0 {
            continue;
        }
        let mut dir = d.map(|v| v / len);
        if dir[0] < 0.0 || (dir[0] == 0.0 && dir[1] < 0.0) {
            dir = dir.map(|v| -v);
        }
        let u: [f32; 3] = core::array::from_fn(|i| b[i] - a[i]);
        let v: [f32; 3] = core::array::from_fn(|i| c[i] - a[i]);
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let nl = n.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        let nz = n[2] / nl;
        let contents = mesh.tri_content_flags.get(tri).copied().unwrap_or(0);
        let mins: [f32; 3] = core::array::from_fn(|i| a[i].min(b[i]).min(c[i]));
        let maxs: [f32; 3] = core::array::from_fn(|i| a[i].max(b[i]).max(c[i]));
        let near = |g: &Group| {
            g.dir.iter().zip(&dir).map(|(x, y)| x * y).sum::<f32>() > 0.995
                && (0..3).all(|i| mins[i] <= g.maxs[i] + 48.0 && maxs[i] >= g.mins[i] - 48.0)
        };
        match groups.iter_mut().find(|g| near(g)) {
            Some(g) => {
                for i in 0..3 {
                    g.mins[i] = g.mins[i].min(mins[i]);
                    g.maxs[i] = g.maxs[i].max(maxs[i]);
                }
                g.tris += 1;
                g.nz_min = g.nz_min.min(nz);
                g.nz_max = g.nz_max.max(nz);
                g.contents |= contents;
            }
            None => groups.push(Group {
                dir,
                mins,
                maxs,
                tris: 1,
                nz_min: nz,
                nz_max: nz,
                contents,
            }),
        }
    }
    // A pipe's normals sweep from the sides up over the top: many tris, a wide spread.
    // Sloped pipes (the ones players climb) first, then the biggest.
    groups.retain(|g| g.tris >= 8 && g.nz_max - g.nz_min > 0.8);
    groups.sort_by(|a, b| {
        let sloped = |g: &Group| g.dir[2].abs() > 0.15;
        sloped(b).cmp(&sloped(a)).then(b.tris.cmp(&a.tris))
    });
    let mut out = vec![format!("cylinders: {}", groups.len())];
    for g in groups.iter().take(20) {
        out.push(format!(
            "cylinder tris {} dir ({:.2} {:.2} {:.2}) mins ({:.0} {:.0} {:.0}) maxs ({:.0} {:.0} {:.0}) nz {:.2}..{:.2} contents {:#x}",
            g.tris, g.dir[0], g.dir[1], g.dir[2], g.mins[0], g.mins[1], g.mins[2], g.maxs[0], g.maxs[1],
            g.maxs[2], g.nz_min, g.nz_max, g.contents
        ));
    }
    out
}

/// Solid static models whose name contains `filter`: where they stand and how big they are.
fn smodel_report(world: &sim::SimWorld, filter: &str) -> Vec<String> {
    let filter = filter.to_ascii_lowercase();
    let mut rows: Vec<String> = world
        .clip_mesh()
        .static_models
        .iter()
        .filter(|sm| sm.name.to_ascii_lowercase().contains(&filter))
        .filter(|sm| sm.model.coll.surfs.iter().any(|s| !s.tris.is_empty()))
        .map(|sm| {
            let m = &sm.model;
            format!(
                "smodel {} `{}` origin ({:.0} {:.0} {:.0}) mid ({:.0} {:.0} {:.0}) half ({:.0} {:.0} {:.0})",
                sm.index,
                sm.name,
                m.origin[0],
                m.origin[1],
                m.origin[2],
                m.bounds_mid[0],
                m.bounds_mid[1],
                m.bounds_mid[2],
                m.bounds_half[0],
                m.bounds_half[1],
                m.bounds_half[2]
            )
        })
        .collect();
    rows.insert(0, format!("smodels matching `{filter}`: {}", rows.len()));
    rows
}

/// Where the map's ladders are: brushes with a ladder side and ladder mesh triangles, merged
/// into one box per ladder.
fn ladder_report(world: &sim::SimWorld) -> Vec<String> {
    let ladder = movement_iw4::SURF_LADDER;
    let mut boxes: Vec<([f32; 3], [f32; 3])> = Vec::new();
    let mut add = |mins: [f32; 3], maxs: [f32; 3]| {
        let near = |b: &([f32; 3], [f32; 3])| {
            (0..3).all(|i| mins[i] <= b.1[i] + 16.0 && maxs[i] >= b.0[i] - 16.0)
        };
        match boxes.iter_mut().find(|b| near(b)) {
            Some(b) => {
                for i in 0..3 {
                    b.0[i] = b.0[i].min(mins[i]);
                    b.1[i] = b.1[i].max(maxs[i]);
                }
            }
            None => boxes.push((mins, maxs)),
        }
    };
    for brush in world.clip_brushes() {
        if !brush.plane_surface_flags.iter().any(|f| f & ladder != 0) {
            continue;
        }
        let mut mins = [f32::NAN; 3];
        let mut maxs = [f32::NAN; 3];
        for plane in &brush.planes {
            for axis in 0..3 {
                if plane[axis] == 1.0 {
                    maxs[axis] = plane[3];
                } else if plane[axis] == -1.0 {
                    mins[axis] = -plane[3];
                }
            }
        }
        if mins.iter().chain(&maxs).all(|v| v.is_finite()) {
            add(mins, maxs);
        }
    }
    let mesh = &world.clip_mesh().tables;
    for (tri, flags) in mesh.tri_surface_flags.iter().enumerate() {
        if flags & ladder == 0 {
            continue;
        }
        let mut mins = [f32::MAX; 3];
        let mut maxs = [f32::MIN; 3];
        for corner in 0..3 {
            let Some(vert) = mesh
                .tri_indices
                .get(tri * 3 + corner)
                .and_then(|&i| mesh.verts.get(usize::from(i)))
            else {
                continue;
            };
            for i in 0..3 {
                mins[i] = mins[i].min(vert[i]);
                maxs[i] = maxs[i].max(vert[i]);
            }
        }
        if mins[0] <= maxs[0] {
            add(mins, maxs);
        }
    }
    let mut out = vec![format!("ladders: {}", boxes.len())];
    for (i, (mins, maxs)) in boxes.iter().enumerate() {
        out.push(format!(
            "ladder {i}: centre ({:.0} {:.0}) z {:.0}..{:.0} size ({:.0} {:.0})",
            (mins[0] + maxs[0]) * 0.5,
            (mins[1] + maxs[1]) * 0.5,
            mins[2],
            maxs[2],
            maxs[0] - mins[0],
            maxs[1] - mins[1]
        ));
    }
    out
}

pub(super) fn hitvol_report(world: &sim::SimWorld) -> Vec<String> {
    let census = world.collision_census();
    let w = &census.world;
    let p = &census.players;
    let e = &census.entities;
    let mut out = vec![
        format!(
            "hitvol world: brushes {} leaves {} leafbrushes {} meshtris {} cmodels {} smodels {} (with tris {}) pen_table {}",
            w.brushes,
            w.bsp_leaves,
            w.leafbrushes,
            w.mesh_tris,
            w.cmodels,
            w.static_models,
            w.static_models_with_tris,
            w.pen_table_loaded
        ),
        format!(
            "hitvol players: poses {} bones {} aabb-only {} bone-count {}..{} with-head-bone {}",
            p.poses, p.with_bones, p.aabb_only, p.min_bones, p.max_bones, p.with_head_bone
        ),
        format!(
            "hitvol entities: rows {} colltris {} boxes {} brush {} authored-no-collision {} not-bullet-solid {} no-dobj {} no-capability {} materialize-failed {} linked-brushes {}",
            e.rows,
            e.colltris,
            e.boxes_only,
            e.brush_only,
            e.no_collision_authored,
            e.not_bullet_solid,
            e.no_dobj,
            e.no_capability,
            e.materialize_failed,
            e.linked_brushes
        ),
    ];
    if !e.no_clip_sample.is_empty() {
        out.push(format!(
            "hitvol entities with no model clip: {}",
            e.no_clip_sample.join(", ")
        ));
    }
    if let Some(error) = &p.materialize_error {
        out.push(format!("hitvol players: materialize error {error}"));
    }
    for kit in &census.kits {
        out.push(format!(
            "hitvol kit `{}`: {} bones {} boxes {} collsurfs {} colltris {} lod {}",
            kit.key,
            kit.clip(),
            kit.bones,
            kit.bone_boxes,
            kit.coll_surfs,
            kit.coll_tris,
            kit.coll_lod
        ));
    }
    for row in world.hitvol_dump() {
        out.push(format!(
            "hitvol client {:?}: geom {} bones {} pose {} body `{}` head `{}` controller {}",
            row.client.map(|c| c.0),
            row.geom,
            row.bone_count,
            row.pose_kind,
            row.body_key,
            row.head_key,
            row.controller
        ));
    }
    out
}
