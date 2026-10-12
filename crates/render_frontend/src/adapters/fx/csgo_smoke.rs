//! CS:GO's smoke (`smoke_mode csgo`): a still cloud of big smoke sprites packed into a dome
//! where the grenade popped. It blooms out in about a second, stands without moving and thins
//! out over its last seconds; it does not flow round corners, and bullets and blasts leave it
//! as it is. The sprites are MW2's own smoke, lit as MW2's smoke grenade lights its puffs and
//! drawn with the other effect sprites, so every player sees the same cloud whichever CS game
//! they have installed.

use std::sync::Arc;

use bevy::prelude::*;
use fx::{FxPackedLightingSrc, FxSpriteInstance};

/// The sprite material: MW2's white smoke flipbook, feathered over its last 3 units where it
/// meets the world, then the plain one. Both are part of every map's common effects. (The
/// 128-unit feather faded a cloud standing on the ground to a haze seen from above.)
pub(crate) const MATERIALS: [&str; 2] = ["gfx_smk_white_atlas_z3", "gfx_smk_white_atlas"];
/// The flipbook as MW2's effects use it: 8 columns, 4 rows. Its early frames are the densest.
const ATLAS_COL_BITS: u8 = 3;
const ATLAS_ROW_BITS: u8 = 2;
const ATLAS_FRAMES: u16 = 32;
const ATLAS_FRAME_FIRST: u32 = 0;
const ATLAS_FRAME_SPAN: u32 = 10;

/// CS:GO's visible smoke: a fat ring of smoke, its centre line 61 units out and its tube 88 units
/// thick, centred 68 units above where the grenade lies (so it reaches 149 units out and 156 up).
/// It is full in about a second and gone 18 s after the pop, thinning over the last 3.
const CENTER_HEIGHT: f32 = 68.0;
const RING_RADIUS: f32 = 61.0;
const TUBE_RADIUS: f32 = 88.0;
/// Puff centres stay this far inside the tube: a sprite's soft edge reaches about that far past
/// its centre, so the cloud ends near the ring's surface.
const PUFF_INSET: f32 = 30.0;
/// Puff centres stay this far above the ground; the soft material takes the sprites down to the
/// floor without a seam.
const PUFF_FLOOR: f32 = 12.0;
const PUFFS: usize = 120;
/// A puff's half-width.
const PUFF_SIZE_MIN: f32 = 72.0;
const PUFF_SIZE_MAX: f32 = 108.0;
const BLOOM_MS: i32 = 1000;
/// How far out the cloud starts when it pops.
const BLOOM_START: f32 = 0.3;
const FADE_START_MS: i32 = 15_000;
const LIFE_MS: i32 = 18_000;
/// The grey of MW2's smoke grenade puffs (its colour graph, ~0.6) and the share of the light at
/// the cloud they take (its lightingFrac, 0.349).
const GREY: u8 = 153;
const LIGHTING_FRAC: u8 = 89;
/// Pops this close and this near in time are one smoke.
const SAME_POP_DISTANCE: f32 = 64.0;
const SAME_POP_MS: i32 = 2000;

struct CsgoSmoke {
    origin: [f32; 3],
    popped_ms: i32,
    seed: u32,
}

#[derive(Resource, Default)]
pub(crate) struct CsgoSmokes {
    smokes: Vec<CsgoSmoke>,
    /// The sprite material's asset index in a material generation.
    material: Option<(u64, Option<usize>)>,
}

impl CsgoSmokes {
    /// A smoke popped at `origin` (FX clock ms).
    pub(crate) fn pop(&mut self, origin: [f32; 3], now_ms: i32) {
        self.smokes.retain(|smoke| live(smoke.popped_ms, now_ms));
        if self.smokes.iter().any(|smoke| {
            distance_sq(smoke.origin, origin) < SAME_POP_DISTANCE * SAME_POP_DISTANCE
                && now_ms.wrapping_sub(smoke.popped_ms).abs() < SAME_POP_MS
        }) {
            return;
        }
        let seed = origin.iter().fold(0x9e37_79b9u32, |h, v| {
            (h ^ v.to_bits()).wrapping_mul(0x0100_0193)
        });
        self.smokes.push(CsgoSmoke {
            origin,
            popped_ms: now_ms,
            seed,
        });
    }

    pub(crate) fn clear(&mut self) {
        self.smokes.clear();
    }

    /// The sprite material's asset index in `catalog`, looked up once per material generation.
    pub(crate) fn material_index(
        &mut self,
        catalog: &render_material::RuntimeMaterialCatalog,
    ) -> Option<usize> {
        let generation = catalog.generation_id.0;
        if let Some((cached, index)) = self.material
            && cached == generation
        {
            return index;
        }
        let index = MATERIALS.iter().find_map(|name| {
            catalog
                .materials
                .iter()
                .position(|material| material.name == *name)
        });
        self.material = Some((generation, index));
        index
    }

    /// Every standing smoke's puffs as effect sprites, farthest from `eye` first so they blend
    /// back to front.
    pub(crate) fn sprites(
        &mut self,
        now_ms: i32,
        eye: [f32; 3],
        material_index: usize,
        light_at: impl Fn([f32; 3]) -> Option<[u8; 3]>,
    ) -> Vec<FxSpriteInstance> {
        self.smokes.retain(|smoke| live(smoke.popped_ms, now_ms));
        if self.smokes.is_empty() {
            return Vec::new();
        }
        let def_name: Arc<str> = Arc::from("csgo_smoke");
        let material_name: Arc<str> = Arc::from(MATERIALS[0]);
        let mut sprites = Vec::with_capacity(self.smokes.len() * PUFFS);
        for smoke in &self.smokes {
            let age = now_ms - smoke.popped_ms;
            let bloom = (age as f32 / BLOOM_MS as f32).clamp(0.0, 1.0);
            let bloom = 1.0 - (1.0 - bloom) * (1.0 - bloom);
            let spread = BLOOM_START + (1.0 - BLOOM_START) * bloom;
            let fade = 1.0
                - ((age - FADE_START_MS) as f32 / (LIFE_MS - FADE_START_MS) as f32).clamp(0.0, 1.0);
            let alpha = (255.0 * bloom.max(0.15) * fade) as u8;
            if alpha == 0 {
                continue;
            }
            let light = light_at([
                smoke.origin[0],
                smoke.origin[1],
                smoke.origin[2] + CENTER_HEIGHT,
            ]);
            let [b, g, r, a] = match light {
                Some(packed) => fx_iw4::apply_lighting_frac_bgra(
                    [GREY, GREY, GREY, alpha],
                    packed,
                    LIGHTING_FRAC,
                ),
                None => [GREY, GREY, GREY, alpha],
            };
            let mut random = smoke.seed;
            let mut next = move || {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (random >> 8) as f32 / (1u32 << 24) as f32
            };
            let tube = TUBE_RADIUS - PUFF_INSET;
            let reach = RING_RADIUS + tube;
            for _ in 0..PUFFS {
                // A point inside the ring's tube, above the floor (relative to the cloud's centre).
                let (x, y, z) = loop {
                    let x = (next() * 2.0 - 1.0) * reach;
                    let y = (next() * 2.0 - 1.0) * reach;
                    let z = (next() * 2.0 - 1.0) * tube;
                    let out = (x * x + y * y).sqrt() - RING_RADIUS;
                    if out * out + z * z <= tube * tube && CENTER_HEIGHT + z >= PUFF_FLOOR {
                        break (x, y, z);
                    }
                };
                let size = PUFF_SIZE_MIN + (PUFF_SIZE_MAX - PUFF_SIZE_MIN) * next();
                let frame = ATLAS_FRAME_FIRST + (next() * ATLAS_FRAME_SPAN as f32) as u32;
                let roll = next() * std::f32::consts::TAU;
                sprites.push(FxSpriteInstance {
                    viewmodel: false,
                    origin: [
                        smoke.origin[0] + x * spread,
                        smoke.origin[1] + y * spread,
                        smoke.origin[2] + CENTER_HEIGHT + z * spread,
                    ],
                    size0: size * spread,
                    size1: 0.0,
                    color_rgba: [r, g, b, a],
                    elem_type: 0,
                    def_name: Arc::clone(&def_name),
                    catalog_index: 0,
                    def_index: 0,
                    material_name: Arc::clone(&material_name),
                    material_index: Some(material_index),
                    axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                    rotation_rad: roll,
                    vel_dir: [0.0; 3],
                    atlas: fx_iw4::sprite_atlas_cell(
                        frame,
                        ATLAS_COL_BITS,
                        ATLAS_ROW_BITS,
                        ATLAS_FRAMES,
                    ),
                    flags: 0,
                    lighting_sample: [0; 3],
                    lighting_frac: 0,
                    packed_lighting_src: FxPackedLightingSrc::Missing,
                });
            }
        }
        sprites.sort_by(|a, b| distance_sq(b.origin, eye).total_cmp(&distance_sq(a.origin, eye)));
        sprites
    }
}

/// A clock that went back (a new map) leaves no smoke standing.
fn live(popped_ms: i32, now_ms: i32) -> bool {
    now_ms >= popped_ms && now_ms - popped_ms < LIFE_MS
}

fn distance_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum()
}
