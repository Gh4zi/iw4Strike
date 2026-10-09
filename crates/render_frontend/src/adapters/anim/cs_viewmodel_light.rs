//! Lights the Counter-Strike viewmodel from the map, as MW2 lights its own: the light grid
//! sampled at the viewmodel lighting origin sets how bright the gun is (ambient plus a soft fill
//! from the viewer's side), and the sun adds its own light when nothing solid stands between the
//! gun and the sky.

use bevy::prelude::*;
use net::{LocalPresentClient, PresentedSnapshot};
use render_gpu::CsViewmodelFrame;

use crate::adapters::anim::dyn_ent::DynEntPhysClip;
use crate::prepare::scene::world::{MapDirPrimaryLight, WorldScene};

/// Linear light-grid colour → ambient and fill light on the gun.
const GRID_AMBIENT: f32 = 2.4;
const GRID_FILL: f32 = 3.2;
/// The sun's colour (already scaled by the map) → its light on the gun.
const SUN_SCALE: f32 = 0.55;
/// How far toward the sun the shadow ray looks, and how fast the gun fades in and out of sun.
const SUN_TRACE: f32 = 8192.0;
const SUN_FADE_PER_SECOND: f32 = 6.0;
const CONTENTS_SOLID: u32 = 0x1;
const SURF_SKY: u32 = 0x4;

/// Linear light-grid colour → ambient light on a CS world model (third-person guns).
const WORLD_GRID_AMBIENT: f32 = 3.0;

/// Sun visibility is cached per cell of this many units: the world the ray crosses does not move,
/// and an 8192-unit trace per gun per frame was most of these systems' time with a dozen players.
const SUN_CACHE_CELL: f32 = 4.0;
const SUN_CACHE_MAX: usize = 1 << 16;

/// Answers [`sees_sun`] once per cell for the map's sun.
#[derive(Default)]
struct SunCache {
    dir: [f32; 3],
    cells: bevy::platform::collections::HashMap<[i32; 3], bool>,
}

impl SunCache {
    fn sees_sun(
        &mut self,
        clip: &Res<DynEntPhysClip>,
        origin: [f32; 3],
        dir: [f32; 3],
    ) -> Option<bool> {
        if clip.is_changed() || self.dir != dir || self.cells.len() > SUN_CACHE_MAX {
            self.cells.clear();
            self.dir = dir;
        }
        let key = origin.map(|v| (v / SUN_CACHE_CELL).floor() as i32);
        if let Some(&seen) = self.cells.get(&key) {
            return Some(seen);
        }
        let seen = sees_sun(clip, origin, dir)?;
        self.cells.insert(key, seen);
        Some(seen)
    }
}

pub fn register_cs_viewmodel_light(app: &mut App) {
    app.add_systems(
        Update,
        (
            light_cs_viewmodel.after(render_anim::occupancy::cs_viewmodel::update_cs_viewmodel),
            light_cs_world_models
                .after(render_anim::occupancy::cs_world_model::update_cs_world_models),
        ),
    );
}

/// Lights each CS world model (a gun in a player's hand) from the light grid where it is, and
/// the sun when the sky is open above it.
fn light_cs_world_models(
    scene: Option<Res<WorldScene>>,
    sun: Option<Res<MapDirPrimaryLight>>,
    clip: Res<DynEntPhysClip>,
    mut frame: ResMut<render_gpu::CsWorldModelsFrame>,
    mut sun_cache: Local<SunCache>,
) {
    let Some(grid) = scene.as_ref().and_then(|s| s.light_grid.as_ref()) else {
        return;
    };
    let sun = sun.filter(|s| s.direction.iter().any(|v| *v != 0.0));
    for instance in &mut frame.instances {
        let origin = [
            instance.world_from_model[3][0],
            instance.world_from_model[3][1],
            instance.world_from_model[3][2],
        ];
        if let Ok(sample) = asset_model::sample_light_grid(&grid.view(), origin) {
            instance.ambient = sample
                .compressed
                .map(|c| srgb_to_linear(f32::from(c) / 255.0) * WORLD_GRID_AMBIENT);
        }
        if let Some(sun) = sun.as_ref()
            && sun_cache.sees_sun(&clip, origin, sun.direction) == Some(true)
        {
            instance.sun_dir = sun.direction;
            instance.sun = sun.color.map(|c| c * sun.diffuse_color_scale * SUN_SCALE);
        }
    }
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Whether the sky is visible from `origin` toward `dir` (unit, toward the sun).
fn sees_sun(clip: &DynEntPhysClip, origin: [f32; 3], dir: [f32; 3]) -> Option<bool> {
    let clip = clip.0.as_ref()?;
    let end = core::array::from_fn(|i| origin[i] + dir[i] * SUN_TRACE);
    let hit = clip.sweep_box(origin, end, [0.0; 3], [0.0; 3], CONTENTS_SOLID);
    Some(!hit.startsolid && (hit.fraction >= 1.0 || hit.surface_flags & SURF_SKY != 0))
}

#[allow(clippy::too_many_arguments)]
fn light_cs_viewmodel(
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    scene: Option<Res<WorldScene>>,
    sun: Option<Res<MapDirPrimaryLight>>,
    clip: Res<DynEntPhysClip>,
    time: Res<Time>,
    mut sun_visible: Local<f32>,
    mut frame: ResMut<CsViewmodelFrame>,
    mut sun_cache: Local<SunCache>,
) {
    if frame.model.is_none() {
        return;
    }
    let Some(ps) = presented.viewweapon_player(local.0) else {
        return;
    };
    let Some(grid) = scene.as_ref().and_then(|s| s.light_grid.as_ref()) else {
        return;
    };
    let origin = render_anim::viewmodel_lighting_origin(
        ps.origin,
        ps.view_height_current,
        ps.viewangles[1],
        ps.leanf,
    );
    let Ok(sample) = asset_model::sample_light_grid(&grid.view(), origin) else {
        return;
    };
    let grid = sample
        .compressed
        .map(|c| srgb_to_linear(f32::from(c) / 255.0));
    frame.ambient = grid.map(|c| c * GRID_AMBIENT);
    frame.shade = grid.map(|c| c * GRID_FILL);

    let Some(sun) = sun.filter(|s| s.direction.iter().any(|v| *v != 0.0)) else {
        return;
    };
    let target = match sun_cache.sees_sun(&clip, origin, sun.direction) {
        Some(true) => 1.0,
        Some(false) => 0.0,
        None => return,
    };
    if (target > 0.5) != (*sun_visible > 0.5) {
        diag::debug!(
            World,
            "cs viewmodel light: sun {} at [{:.0}, {:.0}, {:.0}]",
            if target > 0.5 { "visible" } else { "hidden" },
            origin[0],
            origin[1],
            origin[2]
        );
    }
    let step = (SUN_FADE_PER_SECOND * time.delta_secs()).min(1.0);
    *sun_visible += (target - *sun_visible) * step;
    // The gun is drawn in the view's frame without the recoil punch; so is its sun.
    let recoil = weapon_iw4::csgo::view_offset(ps.cs_shooting_mode, ps.cs_punch, ps.cs_view_punch);
    let (forward, right, up) = math_iw4::angle_vectors([
        ps.viewangles[0] - recoil[0],
        ps.viewangles[1] - recoil[1],
        0.0,
    ]);
    let d = sun.direction;
    let dot = |a: [f32; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
    frame.sun_dir = [dot(forward), -dot(right), dot(up)];
    let strength = *sun_visible * sun.diffuse_color_scale * SUN_SCALE;
    frame.sun = sun.color.map(|c| c * strength);
}
