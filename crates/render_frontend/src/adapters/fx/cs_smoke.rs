//! CS2's volumetric smoke on the client. When the server pops a smoke under `smoke_mode cs2`
//! (`CS_SMOKE_VOLUME_PARM`), the smoke fills the space around it: a flood through the map's
//! collision on a voxel grid, cheapest voxels first, until it holds a CS2 smoke's volume. It
//! spreads along corridors and stops at walls. Bullets leave tunnels through it and blasts
//! clear it for a moment, then it rolls back. render_gpu's `cs_smoke` pass draws it.
//!
//! The flood and its lighting run on their own thread (a few thousand short traces), so popping
//! a smoke costs a frame nothing; the cloud starts growing once it is done, a few frames later.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use std::thread::JoinHandle;

use bevy::prelude::*;
use render_fx::HostFxSystem;
use render_gpu::{
    CS_SMOKE_DIM, CS_SMOKE_HOLES, CS_SMOKE_SLOTS, CsSmokeFrame, CsSmokeHole, CsSmokeVolume,
};

use crate::adapters::anim::dyn_ent::DynEntPhysClip;
use crate::prepare::scene::world::{MapDirPrimaryLight, WorldScene};

/// Side of one voxel.
const VOXEL: f32 = 16.0;
/// Voxels along x, y and z.
const DIMS: [usize; 3] = [
    CS_SMOKE_DIM[0] as usize,
    CS_SMOKE_DIM[1] as usize,
    CS_SMOKE_DIM[2] as usize,
];
const CELLS: usize = DIMS[0] * DIMS[1] * DIMS[2];
/// The voxel the smoke starts from: the grid reaches 6 voxels below the grenade and 17 above.
const START: [usize; 3] = [16, 16, 6];
/// How far above the grenade the start voxel's centre sits.
const GRID_LIFT: f32 = 2.0;
/// Voxels one smoke fills: in the open, a flat-topped mass some 340 units across standing its
/// full body height.
const FILL: usize = 3600;
/// The smoke's body height above the floor beneath it (voxels, 160 units): it covers a player
/// standing or jumping anywhere inside.
const BODY_HEIGHT: u8 = 10;
/// Flood cost of a step up or down against one sideways: within its body height the smoke fills
/// a column before it spreads on.
const COST_VERTICAL: f32 = 0.6;
/// How much dearer a step into air above the body height is: the smoke goes there only when
/// walls leave it nowhere else.
const COST_ABOVE_BODY: f32 = 6.0;
/// How far out (voxels) the body reaches in the open before it has rounded down to the floor:
/// with [`FILL`] it is about filled on open ground.
const BODY_RADIUS: f32 = 11.0;
/// The smoke thins out over its last voxels before open air (walls and floors keep it dense),
/// where the drifting noise eats into it.
const EDGE_STEPS: u8 = 3;
/// Voxels on the floor are at least this deep, so the smoke stays thick down to the ground.
const FLOOR_STEPS: u8 = 2;
/// After the pop: the smoke spreads to its full size, holds, then clears. It lasts 18 seconds,
/// as CS2's: the clearing thins it linearly, and the noise eats the thin smoke first, so it is
/// gone to the eye at about 18 s and dropped at the end.
const GROW_MS: f32 = 1200.0;
const CLEAR_START_MS: i32 = 17_250;
const CLEAR_END_MS: i32 = 18_500;
/// A bullet's tunnel and how long it takes to close.
const BULLET_RADIUS: f32 = 10.0;
const BULLET_CLOSE_MS: i32 = 1500;
/// A blast: how far it pushes the smoke back, how long that stays clear, and how long the smoke
/// takes to roll back in.
const BLAST_RADIUS: f32 = 170.0;
const BLAST_HOLD_MS: i32 = 900;
const BLAST_CLOSE_MS: i32 = 2200;
/// Two pops this close in place and time are the same smoke.
const SAME_POP_DISTANCE: f32 = 4.0;
const SAME_POP_MS: i32 = 500;
const CONTENTS_SOLID: u32 = 0x1;
/// What stops a bullet (`MASK_SHOT`) stops the smoke at a placed model (a hut, a container).
const PLACED_MODEL_MASK: u32 = 0x0280_6831;
const SURF_SKY: u32 = 0x4;
/// How far toward the sun the sky check looks.
const SUN_TRACE: f32 = 8192.0;
/// The light grid is read every this many voxels across the smoke's box.
const LIGHT_STRIDE: usize = 4;
const LIGHT_DIMS: [usize; 3] = [
    DIMS[0] / LIGHT_STRIDE + 1,
    DIMS[1] / LIGHT_STRIDE + 1,
    DIMS[2] / LIGHT_STRIDE + 1,
];
/// Linear light-grid luminance → the light on the smoke from all around (linear, as on a white
/// wall): `GRID_BRIGHTNESS * luminance^GRID_RANGE`, kept between a floor in the dark and a
/// ceiling (the texels store it over the ceiling). The power narrows MW2's range between shade
/// and sun (about 3:1 on the light grid) to about 1.5:1: the sun itself adds the rest, and the
/// map's film grading brightens everything after.
const GRID_BRIGHTNESS: f32 = 0.55;
const GRID_RANGE: f32 = 0.35;
const AMBIENT_FLOOR: f32 = 0.08;
const AMBIENT_MAX: f32 = 1.5;
/// How much of the light grid's colour the smoke takes; the rest is grey. CS2's smoke is white
/// (`g_vSmokeColor`) and takes all its colour from the light where it is: tan in a sunlit sandy
/// alley, blue-grey in shade.
const TINT: f32 = 1.0;
/// The sun's light on the smoke (linear, as on a white wall facing it) at the map's luminance,
/// in the map sun's colour: it lights the sunlit side, the far side stays in shade.
const SUN_LIGHT: f32 = 0.4;
/// Voxels this deep in from open air are the darkest inside (CS2's `g_flInteriorDarkening`).
const INTERIOR_STEPS: u8 = 6;
const INTERIOR_DARKENING: f32 = 0.5;

/// One flooded smoke, ready to draw.
struct Fill {
    min: [f32; 3],
    texels: Arc<Vec<u8>>,
    /// The light grid's colour around the smoke, at luminance 1.
    tint: [f32; 3],
}

struct Smoke {
    id: u64,
    slot: u32,
    origin: [f32; 3],
    popped_ms: i32,
    job: Option<JoinHandle<Fill>>,
    fill: Option<Fill>,
}

#[derive(Clone, Copy)]
struct Hole {
    a: [f32; 3],
    b: [f32; 3],
    made_ms: i32,
    blast: bool,
}

/// The smokes on screen and the holes in them.
#[derive(Resource, Default)]
pub(crate) struct CsSmokes {
    generation: frame::WorldGeneration,
    smokes: Vec<Smoke>,
    holes: Vec<Hole>,
    next_id: u64,
}

impl CsSmokes {
    /// A smoke popped at `origin`.
    pub(crate) fn pop(&mut self, origin: [f32; 3], now_ms: i32) {
        if self.smokes.iter().any(|s| {
            distance(s.origin, origin) < SAME_POP_DISTANCE
                && now_ms.wrapping_sub(s.popped_ms).abs() < SAME_POP_MS
        }) {
            return;
        }
        if self.smokes.len() >= CS_SMOKE_SLOTS {
            // Every slot is taken: the oldest smoke gives way.
            if let Some(oldest) = self
                .smokes
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.popped_ms)
                .map(|(i, _)| i)
            {
                self.smokes.remove(oldest);
            }
        }
        let slot = (0..CS_SMOKE_SLOTS as u32)
            .find(|slot| self.smokes.iter().all(|s| s.slot != *slot))
            .unwrap_or(0);
        self.next_id += 1;
        diag::debug!(
            World,
            "cs2 smoke: pop {} at {:?} slot {}",
            self.next_id,
            origin,
            slot
        );
        self.smokes.push(Smoke {
            id: self.next_id,
            slot,
            origin,
            popped_ms: now_ms,
            job: None,
            fill: None,
        });
    }

    /// A bullet flew from `start` to `end`: it tunnels through any smoke on its way.
    pub(crate) fn bullet(&mut self, start: [f32; 3], end: [f32; 3], now_ms: i32) {
        if self
            .smokes
            .iter()
            .any(|s| segment_hits_box(start, end, s.bounds(), BULLET_RADIUS))
        {
            self.holes.push(Hole {
                a: start,
                b: end,
                made_ms: now_ms,
                blast: false,
            });
        }
    }

    /// Something blew up at `origin`: it clears the smoke around it.
    pub(crate) fn blast(&mut self, origin: [f32; 3], now_ms: i32) {
        if self
            .smokes
            .iter()
            .any(|s| segment_hits_box(origin, origin, s.bounds(), BLAST_RADIUS))
        {
            self.holes.push(Hole {
                a: origin,
                b: origin,
                made_ms: now_ms,
                blast: true,
            });
        }
    }

    pub(crate) fn clear(&mut self) {
        self.smokes.clear();
        self.holes.clear();
    }
}

impl Smoke {
    /// World box the smoke's voxels can fill.
    fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        Self::bounds_at(self.origin)
    }

    /// World box the voxels of a smoke popped at `origin` can fill.
    fn bounds_at(origin: [f32; 3]) -> ([f32; 3], [f32; 3]) {
        let min = grid_min(origin);
        let max = core::array::from_fn(|i| min[i] + DIMS[i] as f32 * VOXEL);
        (min, max)
    }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    Vec3::from_array(a).distance(Vec3::from_array(b))
}

fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// World corner of voxel (0, 0, 0) for a smoke popped at `origin`: the start voxel's centre
/// sits just above the grenade, so the bottom layer reaches down to the floor it lies on
/// (filtering fades the smoke over the half voxel below the last centres).
fn grid_min(origin: [f32; 3]) -> [f32; 3] {
    let lifted = [origin[0], origin[1], origin[2] + GRID_LIFT];
    core::array::from_fn(|i| lifted[i] - (START[i] as f32 + 0.5) * VOXEL)
}

fn index(c: [usize; 3]) -> usize {
    (c[2] * DIMS[1] + c[1]) * DIMS[0] + c[0]
}

fn cell_of(i: usize) -> [usize; 3] {
    [
        i % DIMS[0],
        (i / DIMS[0]) % DIMS[1],
        i / (DIMS[0] * DIMS[1]),
    ]
}

fn centre(min: [f32; 3], c: [usize; 3]) -> [f32; 3] {
    core::array::from_fn(|i| min[i] + (c[i] as f32 + 0.5) * VOXEL)
}

/// The outermost voxels stay empty, so smokes side by side in the atlas never bleed.
fn inside(c: [i64; 3]) -> bool {
    (0..3).all(|i| c[i] >= 1 && c[i] < DIMS[i] as i64 - 1)
}

/// Whether the segment passes within `pad` of the box.
fn segment_hits_box(a: [f32; 3], b: [f32; 3], (min, max): ([f32; 3], [f32; 3]), pad: f32) -> bool {
    let (mut t0, mut t1) = (0.0_f32, 1.0_f32);
    for i in 0..3 {
        let (lo, hi) = (min[i] - pad, max[i] + pad);
        let d = b[i] - a[i];
        if d.abs() < 1e-6 {
            if a[i] < lo || a[i] > hi {
                return false;
            }
            continue;
        }
        let (mut near, mut far) = ((lo - a[i]) / d, (hi - a[i]) / d);
        if near > far {
            core::mem::swap(&mut near, &mut far);
        }
        t0 = t0.max(near);
        t1 = t1.min(far);
        if t0 > t1 {
            return false;
        }
    }
    true
}

/// Edge state between a voxel and its + neighbour on one axis.
const EDGE_UNKNOWN: u8 = 0;
const EDGE_OPEN: u8 = 1;
const EDGE_SHUT: u8 = 2;

/// A smoke's flood: which voxels it filled, in order with their cost, and which voxel faces the
/// map closes.
struct Flood {
    min: [f32; 3],
    order: Vec<([usize; 3], f32)>,
    filled: Vec<bool>,
    edges: Vec<u8>,
}

impl Flood {
    /// Whether the face between `cell` and its neighbour `step` along `axis` is a wall.
    fn shut(&self, cell: [usize; 3], axis: usize, step: i64) -> bool {
        let mut lo = cell;
        if step < 0 {
            lo[axis] -= 1;
        }
        self.edges[index(lo) * 3 + axis] == EDGE_SHUT
    }
}

/// Floods a smoke's voxels from `origin`. `trace(a, b)` is how far from `a` toward `b` the map
/// lets a point go (1 all the way, 0 stuck). Voxels fill cheapest first (26 neighbours; a
/// diagonal step needs an open path along its axes). As CS2's, the smoke fills each column up
/// to its body height above the floor beneath before spreading on, so it stands flat-topped and
/// tall enough to hide a player anywhere in it; it climbs higher only where walls box it in,
/// and drops off ledges and down stairs instead of hanging in the air.
fn flood(origin: [f32; 3], mut trace: impl FnMut([f32; 3], [f32; 3]) -> f32) -> Flood {
    let min = grid_min(origin);
    // The start voxel, or the nearest one the grenade can reach when that is in a wall.
    let mut starts: Vec<[usize; 3]> = (0..27)
        .map(|i| [i % 3, (i / 3) % 3, i / 9])
        .map(|d| core::array::from_fn(|k| START[k] + d[k] - 1))
        .collect();
    starts.sort_by(|a, b| {
        distance(centre(min, *a), origin).total_cmp(&distance(centre(min, *b), origin))
    });
    let start = starts
        .iter()
        .copied()
        .find(|c| trace(origin, centre(min, *c)) >= 1.0)
        .unwrap_or(START);

    // Each axis edge (from a voxel to its + neighbour) is traced once, and each voxel's height
    // above the floor beneath it (in voxels, 255 not yet known).
    let mut edges = vec![EDGE_UNKNOWN; CELLS * 3];
    let mut heights = vec![u8::MAX; CELLS];
    let mut probe = |query: Probe| -> u8 {
        match query {
            Probe::Edge(from, axis, step) => {
                let mut lo = from;
                if step < 0 {
                    lo[axis] -= 1;
                }
                let mut hi = lo;
                hi[axis] += 1;
                let slot = index(lo) * 3 + axis;
                if edges[slot] == EDGE_UNKNOWN {
                    edges[slot] = if trace(centre(min, lo), centre(min, hi)) < 1.0 {
                        EDGE_SHUT
                    } else {
                        EDGE_OPEN
                    };
                }
                u8::from(edges[slot] == EDGE_OPEN)
            }
            Probe::Sight(from, to) => u8::from(trace(centre(min, from), centre(min, to)) >= 1.0),
            Probe::Height(cell) => {
                let at = index(cell);
                if heights[at] == u8::MAX {
                    let top = centre(min, cell);
                    let reach = f32::from(BODY_HEIGHT + 2) * VOXEL;
                    let bottom = [top[0], top[1], top[2] - reach];
                    heights[at] = ((trace(top, bottom) * reach) / VOXEL).floor() as u8;
                }
                heights[at]
            }
        }
    };

    // Lazy Theta*: a voxel's cost is the length of the shortest path to it that may cut across
    // open space at any angle, so in the open the smoke fills in order of straight-line distance
    // (round, not the octagon voxel steps grow) while walls still make it go the long way: it
    // fills a room before it pours out of the room's doors and windows. Each voxel has two
    // candidates: straight from the voxel its parent could see (taken when the sight line holds),
    // and a step from the neighbour it was reached from.
    let mut cost = vec![f32::INFINITY; CELLS];
    let mut parent = vec![usize::MAX; CELLS];
    let mut sight_cost = vec![f32::INFINITY; CELLS];
    let mut sight_from = vec![usize::MAX; CELLS];
    let mut step_cost = vec![f32::INFINITY; CELLS];
    let mut step_from = vec![usize::MAX; CELLS];
    let mut filled = vec![false; CELLS];
    let mut order = Vec::with_capacity(FILL);
    let mut heap = BinaryHeap::new();
    let first = index(start);
    sight_cost[first] = 0.0;
    sight_from[first] = first;
    heap.push(Reverse((0.0_f32.to_bits(), first)));
    let moves: Vec<[i64; 3]> = (0..27)
        .map(|i| [i % 3 - 1, (i / 3) % 3 - 1, i / 9 - 1])
        .filter(|d| *d != [0, 0, 0])
        .collect();
    let metric = |a: usize, b: usize| {
        let (a, b) = (cell_of(a), cell_of(b));
        let d: [f32; 3] = core::array::from_fn(|k| b[k] as f32 - a[k] as f32);
        (d[0] * d[0] + d[1] * d[1] + (d[2] * COST_VERTICAL).powi(2)).sqrt()
    };
    while order.len() < FILL {
        let Some(Reverse((bits, at))) = heap.pop() else {
            break;
        };
        if filled[at] {
            continue;
        }
        let key = f32::from_bits(bits);
        // Take the straight candidate when its sight line holds; otherwise the stepped one
        // (queued again when that is dearer than this pop, so voxels still fill in order).
        let seen = sight_cost[at] <= step_cost[at]
            && (sight_from[at] == at
                || probe(Probe::Sight(cell_of(sight_from[at]), cell_of(at))) == 1);
        if seen {
            cost[at] = sight_cost[at];
            parent[at] = sight_from[at];
        } else {
            sight_cost[at] = f32::INFINITY;
            if step_cost[at] > key + 1e-3 {
                heap.push(Reverse((step_cost[at].to_bits(), at)));
                continue;
            }
            cost[at] = step_cost[at];
            parent[at] = step_from[at];
        }
        filled[at] = true;
        let here = cell_of(at);
        order.push((here, cost[at]));
        for d in &moves {
            let next: [i64; 3] = core::array::from_fn(|k| here[k] as i64 + d[k]);
            if !inside(next) {
                continue;
            }
            let next_cell = next.map(|v| v as usize);
            let to = index(next_cell);
            if filled[to] {
                continue;
            }
            // Above its body (full height through the middle, rounding off to the rim) the
            // smoke only climbs or spreads when boxed in, one dear step at a time; falling stays
            // cheap.
            let out = [
                next[0] as f32 - start[0] as f32,
                next[1] as f32 - start[1] as f32,
            ];
            let above = d[2] >= 0
                && f32::from(probe(Probe::Height(next_cell))) >= body_at(out[0].hypot(out[1]));
            let mut step = metric(at, to);
            if above {
                step *= COST_ABOVE_BODY;
            }
            let stepped = cost[at] + step;
            let straight = if above {
                f32::INFINITY
            } else {
                cost[parent[at]] + metric(parent[at], to)
            };
            if stepped >= step_cost[to] && straight >= sight_cost[to] {
                continue;
            }
            // Some order of the axis steps must stay in open voxels.
            let axes: Vec<usize> = (0..3).filter(|&k| d[k] != 0).collect();
            let passable = permutations(&axes).iter().any(|path| {
                let mut cur = here;
                path.iter().all(|&axis| {
                    let ok = probe(Probe::Edge(cur, axis, d[axis])) == 1;
                    cur[axis] = (cur[axis] as i64 + d[axis]) as usize;
                    ok
                })
            });
            if !passable {
                continue;
            }
            if stepped < step_cost[to] {
                step_cost[to] = stepped;
                step_from[to] = at;
            }
            if straight < sight_cost[to] {
                sight_cost[to] = straight;
                sight_from[to] = parent[at];
            }
            let best = step_cost[to].min(sight_cost[to]);
            heap.push(Reverse((best.to_bits(), to)));
        }
    }
    Flood {
        min,
        order,
        filled,
        edges,
    }
}

/// The smoke's body height (voxels above the floor) `r` voxels out from where it popped: full
/// through most of it, rounding off to nothing at [`BODY_RADIUS`] (a squircle, flat on top
/// with round shoulders, as CS2's).
fn body_at(r: f32) -> f32 {
    let edge = (r / BODY_RADIUS).min(1.0);
    f32::from(BODY_HEIGHT + 1) * (1.0 - edge.powi(4)).powf(0.25)
}

/// What the flood asks the map: whether a voxel face is open, or whether a voxel lies within
/// the smoke's body height of the floor beneath it.
enum Probe {
    Edge([usize; 3], usize, i64),
    /// Whether the straight line between two voxel centres is open.
    Sight([usize; 3], [usize; 3]),
    Height([usize; 3]),
}

fn permutations(axes: &[usize]) -> Vec<Vec<usize>> {
    match axes {
        [] => vec![Vec::new()],
        [a] => vec![vec![*a]],
        [a, b] => vec![vec![*a, *b], vec![*b, *a]],
        _ => {
            let mut all = Vec::new();
            for (i, &first) in axes.iter().enumerate() {
                let rest: Vec<usize> = axes
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, &a)| a)
                    .collect();
                for mut tail in permutations(&rest) {
                    tail.insert(0, first);
                    all.push(tail);
                }
            }
            all
        }
    }
}

/// How deep each filled voxel lies: 1 next to open air, up to `steps` further in (and for
/// everything deeper). A voxel against a wall or the floor is not next to open air.
fn edge_depth(flood: &Flood, steps: u8) -> Vec<u8> {
    let mut depth = vec![0u8; CELLS];
    let neighbours = |c: [usize; 3]| {
        (0..3).flat_map(move |axis| {
            [-1_i64, 1].into_iter().map(move |step| {
                let mut n: [i64; 3] = c.map(|v| v as i64);
                n[axis] += step;
                (axis, step, n)
            })
        })
    };
    for (cell, _) in &flood.order {
        let exposed = neighbours(*cell).any(|(axis, step, n)| {
            !inside(n)
                || (!flood.filled[index(n.map(|v| v as usize))] && !flood.shut(*cell, axis, step))
        });
        if exposed {
            depth[index(*cell)] = 1;
        }
    }
    for level in 2..=steps {
        for (cell, _) in &flood.order {
            let at = index(*cell);
            if depth[at] == 0
                && neighbours(*cell)
                    .any(|(_, _, n)| inside(n) && depth[index(n.map(|v| v as usize))] == level - 1)
            {
                depth[at] = level;
            }
        }
    }
    for (cell, _) in &flood.order {
        let at = index(*cell);
        if depth[at] == 0 {
            depth[at] = steps;
        }
    }
    depth
}

/// Averages a per-voxel value with its filled neighbours, so light baked per voxel shows no
/// voxel steps.
fn smooth(flood: &Flood, values: &mut [f32]) {
    for _ in 0..2 {
        let before = values.to_vec();
        for (cell, _) in &flood.order {
            let (mut sum, mut count) = (before[index(*cell)], 1.0);
            for axis in 0..3 {
                for step in [-1_i64, 1] {
                    let mut n: [i64; 3] = cell.map(|v| v as i64);
                    n[axis] += step;
                    if inside(n) && flood.filled[index(n.map(|v| v as usize))] {
                        sum += before[index(n.map(|v| v as usize))];
                        count += 1.0;
                    }
                }
            }
            values[index(*cell)] = sum / count;
        }
    }
}

/// What [`bake`] makes of a flood.
struct Baked {
    texels: Vec<u8>,
    /// The colour of the light around the smoke, at luminance 1.
    tint: [f32; 3],
    /// Mean linear luminance of the light around it.
    luma: f32,
}

/// The flood's texels for [`CsSmokeVolume::texels`]. Each filled voxel gets its density
/// (thinning toward open air), its flood order (0 first, 1 last), whether it sees the sun
/// (`sees_sun` traces a point to the sky; the shader shades it by the smoke in front) and the
/// light from all around (`light`, linear, at a world point), darker deep inside the smoke as
/// CS2's (`g_flInteriorDarkening`). All but density are stored multiplied by it, so they
/// filter right at the edges.
fn bake(
    flood: &Flood,
    mut sees_sun: impl FnMut([f32; 3]) -> bool,
    light: impl Fn([f32; 3]) -> [f32; 3],
) -> Baked {
    let edge = edge_depth(flood, EDGE_STEPS);
    let depth = edge_depth(flood, INTERIOR_STEPS);
    let last = flood.order.last().map_or(1.0, |(_, c)| c.max(1.0));
    // Sky checks are shared by blocks of 2×2×2 voxels.
    let mut sky_blocks: HashMap<[usize; 3], bool> = HashMap::new();
    let mut sky = vec![0.0_f32; CELLS];
    let mut around = vec![0.0_f32; CELLS];
    let mut colour = [0.0_f32; 3];
    for (cell, _) in &flood.order {
        let at = index(*cell);
        let point = centre(flood.min, *cell);
        let block = cell.map(|v| v & !1);
        sky[at] = f32::from(u8::from(
            *sky_blocks.entry(block).or_insert_with(|| sees_sun(point)),
        ));
        let light = light(point);
        for (sum, c) in colour.iter_mut().zip(light) {
            *sum += c;
        }
        let interior = f32::from(depth[at] - 1) / f32::from(INTERIOR_STEPS - 1);
        around[at] = (GRID_BRIGHTNESS * luminance(light).max(0.0).powf(GRID_RANGE))
            .clamp(AMBIENT_FLOOR, AMBIENT_MAX)
            * (1.0 - INTERIOR_DARKENING * interior);
    }
    smooth(flood, &mut sky);
    smooth(flood, &mut around);
    let mut texels = vec![0u8; CELLS * 4];
    for (cell, cost) in &flood.order {
        let at = index(*cell);
        // Smoke resting on the floor keeps its body out to its rim: it covers the ground, and
        // only its upper edges billow away.
        let on_floor = flood.shut(*cell, 2, -1);
        let depth = if on_floor { edge[at].max(FLOOR_STEPS) } else { edge[at] };
        let density = f32::from(depth) / f32::from(EDGE_STEPS);
        let byte = |v: f32| (v.clamp(0.0, 1.0) * density * 255.0).round() as u8;
        texels[at * 4] = (density * 255.0).round() as u8;
        texels[at * 4 + 1] = byte(cost / last);
        texels[at * 4 + 2] = byte(sky[at]);
        texels[at * 4 + 3] = byte(around[at] / AMBIENT_MAX);
    }
    let count = flood.order.len().max(1) as f32;
    let luma = luminance(colour);
    let tint = if luma > 1e-6 {
        colour.map(|c| (1.0 - TINT) + TINT * c / luma)
    } else {
        [1.0; 3]
    };
    Baked {
        texels,
        tint,
        luma: luma / count,
    }
}

/// The light grid's linear colour at the lattice points across a smoke's box, read on the main
/// thread for the flood's.
fn light_lattice(scene: Option<&WorldScene>, min: [f32; 3]) -> Option<Vec<[f32; 3]>> {
    let grid = scene?.light_grid.as_ref()?;
    let view = grid.view();
    let mut lattice = Vec::with_capacity(LIGHT_DIMS[0] * LIGHT_DIMS[1] * LIGHT_DIMS[2]);
    for z in 0..LIGHT_DIMS[2] {
        for y in 0..LIGHT_DIMS[1] {
            for x in 0..LIGHT_DIMS[0] {
                let point: [f32; 3] =
                    core::array::from_fn(|k| min[k] + ([x, y, z][k] * LIGHT_STRIDE) as f32 * VOXEL);
                let colour = asset_model::sample_light_grid(&view, point).map_or([0.1; 3], |s| {
                    s.compressed.map(|c| srgb_to_linear(f32::from(c) / 255.0))
                });
                lattice.push(colour);
            }
        }
    }
    Some(lattice)
}

/// The lattice's colour at a world point, trilinear.
fn lattice_light(lattice: &[[f32; 3]], min: [f32; 3], point: [f32; 3]) -> [f32; 3] {
    let at: [f32; 3] = core::array::from_fn(|k| {
        ((point[k] - min[k]) / (VOXEL * LIGHT_STRIDE as f32)).clamp(0.0, (LIGHT_DIMS[k] - 1) as f32)
    });
    let base: [usize; 3] = core::array::from_fn(|k| (at[k] as usize).min(LIGHT_DIMS[k] - 2));
    let frac: [f32; 3] = core::array::from_fn(|k| at[k] - base[k] as f32);
    let mut out = [0.0; 3];
    for corner in 0..8 {
        let offset = [corner & 1, (corner >> 1) & 1, corner >> 2];
        let weight: f32 = (0..3)
            .map(|k| {
                if offset[k] == 1 {
                    frac[k]
                } else {
                    1.0 - frac[k]
                }
            })
            .product();
        let c = lattice[((base[2] + offset[2]) * LIGHT_DIMS[1] + base[1] + offset[1])
            * LIGHT_DIMS[0]
            + base[0]
            + offset[0]];
        for k in 0..3 {
            out[k] += c[k] * weight;
        }
    }
    out
}

fn start_fill(
    origin: [f32; 3],
    clip: Option<Arc<asset_world::ClipCollision>>,
    sun_dir: Option<[f32; 3]>,
    lattice: Option<Vec<[f32; 3]>>,
) -> JoinHandle<Fill> {
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let mut traces = 0u32;
        // Placed models (huts, containers, crates) collide apart from the map's brushes; the
        // flood only tests the ones overlapping its own box.
        let (box_min, box_max) = Smoke::bounds_at(origin);
        let nearby: Vec<&clipmap_iw4::ClipStaticModel> = clip
            .as_deref()
            .map(|clip| {
                clip.static_models
                    .iter()
                    .map(|placed| &placed.model)
                    .filter(|model| {
                        (0..3).all(|k| {
                            model.bounds_mid[k] + model.bounds_half[k] >= box_min[k]
                                && model.bounds_mid[k] - model.bounds_half[k] <= box_max[k]
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let nearby_count = nearby.len();
        let flood = flood(origin, |a, b| {
            traces += 1;
            clip.as_ref().map_or(1.0, |clip| {
                let hit = clip.sweep_box(a, b, [-1.0; 3], [1.0; 3], CONTENTS_SOLID);
                if hit.startsolid {
                    return 0.0;
                }
                let mut best = trace_iw4::Trace {
                    fraction: hit.fraction,
                    endpos: hit.endpos,
                    ..trace_iw4::Trace::default()
                };
                if best.fraction > 0.0 && !nearby.is_empty() {
                    clipmap_iw4::point_trace_static_models(
                        nearby.iter().copied(),
                        a,
                        b,
                        PLACED_MODEL_MASK,
                        &mut best,
                    );
                }
                if best.startsolid != 0 {
                    0.0
                } else {
                    best.fraction
                }
            })
        });
        let (mut sky_traces, mut sky_open) = (0u32, 0u32);
        let sees_sun = |point: [f32; 3]| {
            sky_traces += 1;
            let (Some(clip), Some(dir)) = (clip.as_ref(), sun_dir) else {
                return false;
            };
            let end = core::array::from_fn(|i| point[i] + dir[i] * SUN_TRACE);
            let hit = clip.sweep_box(point, end, [0.0; 3], [0.0; 3], CONTENTS_SOLID);
            // A placed model (a hut's roof) between the smoke and the sun shades it too.
            let mut blocker = trace_iw4::Trace {
                fraction: hit.fraction,
                endpos: hit.endpos,
                ..trace_iw4::Trace::default()
            };
            let shaded = clipmap_iw4::point_trace_static_models(
                clip.static_models.iter().map(|placed| &placed.model),
                point,
                end,
                PLACED_MODEL_MASK,
                &mut blocker,
            )
            .is_some();
            let open = !hit.startsolid
                && !shaded
                && (hit.fraction >= 1.0 || hit.surface_flags & SURF_SKY != 0);
            sky_open += u32::from(open);
            open
        };
        let min = flood.min;
        let light = |point: [f32; 3]| {
            lattice
                .as_deref()
                .map_or([0.1; 3], |lattice| lattice_light(lattice, min, point))
        };
        let baked = bake(&flood, sees_sun, light);
        diag::debug!(
            World,
            "cs2 smoke: filled {} voxels at {:?} in {:.1} ms ({} traces, {} placed models by it, sun seen from {} of \
             {} sky blocks toward {:?}), light grid luminance {:.4}, tint {:?}",
            flood.order.len(),
            origin,
            started.elapsed().as_secs_f32() * 1000.0,
            traces,
            nearby_count,
            sky_open,
            sky_traces,
            sun_dir,
            baked.luma,
            baked.tint
        );
        Fill {
            min,
            texels: Arc::new(baked.texels),
            tint: baked.tint,
        }
    })
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Floods new smokes, ages smokes and holes, and hands the frame to the smoke pass.
pub(crate) fn update_cs_smokes(
    mut smokes: ResMut<CsSmokes>,
    host: Res<HostFxSystem>,
    generation: Res<frame::WorldGeneration>,
    clip: Res<DynEntPhysClip>,
    scene: Option<Res<WorldScene>>,
    sun: Option<Res<MapDirPrimaryLight>>,
    settings: Res<frame::GameSettings>,
    mut frame: ResMut<CsSmokeFrame>,
) {
    if smokes.generation != *generation {
        smokes.clear();
        smokes.generation = *generation;
    }
    let now = host.0.msec_now;
    let sun = sun.filter(|s| s.direction.iter().any(|v| *v != 0.0));
    let sun_dir = sun.as_ref().map(|s| s.direction);
    // The map's sun colour at a set brightness: maps scale their suns very differently.
    let sun_light = sun.as_ref().map_or([0.0; 3], |s| {
        let luma = luminance(s.color);
        if luma > 1e-6 {
            s.color.map(|c| c / luma * SUN_LIGHT)
        } else {
            [0.0; 3]
        }
    });
    // Time running backwards (a killcam, a new round's clock) or past the end drops the smoke.
    smokes.smokes.retain(|s| {
        let age = now.wrapping_sub(s.popped_ms);
        (-1000..CLEAR_END_MS).contains(&age)
    });
    smokes.holes.retain(|h| {
        let age = now.wrapping_sub(h.made_ms);
        let life = if h.blast {
            BLAST_HOLD_MS + BLAST_CLOSE_MS
        } else {
            BULLET_CLOSE_MS
        };
        (0..life).contains(&age)
    });
    let excess = smokes.holes.len().saturating_sub(CS_SMOKE_HOLES);
    smokes.holes.drain(..excess);

    let mut volumes = Vec::with_capacity(smokes.smokes.len());
    for smoke in &mut smokes.smokes {
        if smoke.fill.is_none() && smoke.job.is_none() {
            let lattice = light_lattice(scene.as_deref(), grid_min(smoke.origin));
            diag::debug!(
                World,
                "cs2 smoke: map sun colour {:?} x {} (clip {})",
                sun.as_ref().map(|s| s.color),
                sun.as_ref().map_or(0.0, |s| s.diffuse_color_scale),
                clip.0.is_some()
            );
            smoke.job = Some(start_fill(smoke.origin, clip.0.clone(), sun_dir, lattice));
        }
        if smoke.job.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(job) = smoke.job.take()
        {
            match job.join() {
                Ok(fill) => smoke.fill = Some(fill),
                Err(_) => diag::warn!(World, "cs2 smoke: flood at {:?} failed", smoke.origin),
            }
        }
        let Some(fill) = smoke.fill.as_ref() else {
            continue;
        };
        let age = now.wrapping_sub(smoke.popped_ms);
        volumes.push(CsSmokeVolume {
            id: smoke.id,
            slot: smoke.slot,
            texels: fill.texels.clone(),
            min: fill.min,
            voxel: VOXEL,
            ambient: fill.tint,
            sun: sun_light,
            grow: 0.12 + ease_out(age as f32 / GROW_MS),
            alpha: 1.0
                - ((age - CLEAR_START_MS) as f32 / (CLEAR_END_MS - CLEAR_START_MS) as f32)
                    .clamp(0.0, 1.0),
        });
    }
    let holes = smokes
        .holes
        .iter()
        .map(|h| {
            let age = now.wrapping_sub(h.made_ms) as f32;
            let (radius, strength) = if h.blast {
                let closing =
                    ((age - BLAST_HOLD_MS as f32) / BLAST_CLOSE_MS as f32).clamp(0.0, 1.0);
                (
                    BLAST_RADIUS * (1.0 - closing).sqrt(),
                    1.0 - closing * closing,
                )
            } else {
                let closing = age / BULLET_CLOSE_MS as f32;
                (BULLET_RADIUS * (1.0 - 0.5 * closing), 1.0 - closing)
            };
            CsSmokeHole {
                a: h.a,
                b: h.b,
                radius: radius.max(1.0),
                strength: strength.clamp(0.0, 1.0),
            }
        })
        .collect();
    *frame = CsSmokeFrame {
        volumes,
        holes,
        seconds: now as f32 / 1000.0,
        sun_dir: sun_dir.unwrap_or([0.0; 3]),
        quality: frame::settings::SmokeQuality::from_name(&settings.smoke_quality)
            .unwrap_or_default(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled_points(flood: &Flood) -> Vec<[f32; 3]> {
        flood
            .order
            .iter()
            .map(|(cell, _)| centre(flood.min, *cell))
            .collect()
    }

    /// How far from `a` toward `b` a point gets before it drops below the floor at `floor_z(x)`
    /// or passes `half_width` from the corridor's centre line.
    fn walk(a: [f32; 3], b: [f32; 3], floor_z: impl Fn(f32) -> f32, half_width: f32) -> f32 {
        let blocked = |p: [f32; 3]| p[2] < floor_z(p[0]) || p[1].abs() > half_width;
        if blocked(a) {
            return 0.0;
        }
        const STEPS: usize = 256;
        (1..=STEPS)
            .map(|i| i as f32 / STEPS as f32)
            .find(|&t| blocked(core::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)))
            .map_or(1.0, |t| t - 1.0 / STEPS as f32)
    }

    fn floor(a: [f32; 3], b: [f32; 3]) -> f32 {
        walk(a, b, |_| 0.0, f32::INFINITY)
    }

    #[test]
    fn open_ground_fills_a_flat_topped_body() {
        let flood = flood([0.0, 0.0, 2.0], floor);
        assert_eq!(flood.order.len(), FILL);
        let points = filled_points(&flood);
        assert!(
            points.iter().all(|p| p[2] > 0.0),
            "no smoke under the floor"
        );
        let body = f32::from(BODY_HEIGHT) * VOXEL;
        let width = points.iter().map(|p| p[0].abs()).fold(0.0, f32::max);
        let height = points.iter().map(|p| p[2]).fold(0.0, f32::max);
        assert!(
            (body - VOXEL..=body + VOXEL * 2.5).contains(&height),
            "stands its body height, a little domed ({height})"
        );
        assert!((130.0..=230.0).contains(&width), "radius {width}");
        // A jumping player is covered most of the way out, not only in the middle.
        for column in points.iter().filter(|p| p[0].hypot(p[1]) < width * 0.6) {
            let top = points
                .iter()
                .filter(|p| p[0] == column[0] && p[1] == column[1])
                .map(|p| p[2])
                .fold(0.0, f32::max);
            assert!(top >= 128.0, "column at {column:?} only {top} high");
        }
    }

    /// A hut 200 × 120 inside and 146 high, its walls 4 thick, a doorway 48 wide and 110 high in
    /// its +x wall: how far from `a` toward `b` a point gets.
    fn hut(a: [f32; 3], b: [f32; 3]) -> f32 {
        let solid = |p: [f32; 3]| {
            let [x, y, z] = p;
            let in_walls = x.abs() <= 104.0 && y.abs() <= 64.0 && z <= 150.0;
            let doorway = x >= 100.0 && y.abs() < 24.0 && z < 110.0;
            z < 0.0
                || (in_walls
                    && !doorway
                    && (x.abs() >= 100.0 || y.abs() >= 60.0 || z >= 146.0))
        };
        if solid(a) {
            return 0.0;
        }
        const STEPS: usize = 512;
        (1..=STEPS)
            .map(|i| i as f32 / STEPS as f32)
            .find(|&t| solid(core::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)))
            .map_or(1.0, |t| t - 1.0 / STEPS as f32)
    }

    #[test]
    fn a_room_fills_before_smoke_pours_out_of_its_door() {
        let flood = flood([-40.0, 0.0, 2.0], hut);
        let points = filled_points(&flood);
        // Nothing behind the hut, where it has no opening, and what comes out of the doorway
        // stays mostly in front of it rather than wrapping round the walls.
        for p in &points {
            assert!(p[0] > -100.0, "smoke behind the back wall at {p:?}");
        }
        let outside = |p: &&[f32; 3]| p[0].abs() > 104.0 || p[1].abs() > 64.0 || p[2] > 150.0;
        let out: Vec<_> = points.iter().filter(outside).collect();
        let round_the_side = out.iter().filter(|p| p[0] < 40.0).count();
        assert!(
            round_the_side * 5 < out.len(),
            "{round_the_side} of {} outside voxels wrapped round the hut",
            out.len()
        );
        // The room is full, and what does not fit has come out of the doorway.
        let inside = points
            .iter()
            .filter(|p| p[0].abs() < 100.0 && p[1].abs() < 60.0 && p[2] < 146.0)
            .count();
        assert!(inside >= 12 * 7 * 9, "room only {inside} voxels full");
        assert!(points.iter().any(|p| p[0] > 140.0), "some pours out of the door");
    }

    #[test]
    fn walls_keep_the_smoke_in_a_corridor() {
        // A corridor 64 wide along x: the smoke runs along it.
        let flood = flood([0.0, 0.0, 2.0], |a, b| walk(a, b, |_| 0.0, 32.0));
        let points = filled_points(&flood);
        assert!(
            points.iter().all(|p| p[1].abs() <= 32.0),
            "smoke through the wall"
        );
        let reach = points.iter().map(|p| p[0].abs()).fold(0.0, f32::max);
        assert!(reach > 200.0, "runs down the corridor ({reach})");
    }

    #[test]
    fn smoke_runs_down_off_a_ledge() {
        // The ground drops 80 units past x = 64.
        let ledge = |x: f32| if x < 64.0 { 0.0 } else { -80.0 };
        let flood = flood([0.0, 0.0, 2.0], |a, b| walk(a, b, ledge, f32::INFINITY));
        let points = filled_points(&flood);
        assert!(
            points.iter().any(|p| p[0] > 120.0 && p[2] < -40.0),
            "smoke spills down to the lower ground"
        );
    }

    #[test]
    fn smoke_thins_toward_open_air_but_not_against_the_floor() {
        let flood = flood([0.0, 0.0, 2.0], floor);
        let Baked { texels, tint, .. } = bake(&flood, |_| false, |_| [0.1; 3]);
        // The start voxel lies on the floor in the middle: full density.
        let start = index(flood.order[0].0);
        assert_eq!(texels[start * 4], 255);
        // The top of the dome touches open air: the thinnest.
        let top = flood
            .order
            .iter()
            .max_by(|a, b| a.0[2].cmp(&b.0[2]))
            .map(|(cell, _)| index(*cell))
            .unwrap_or(start);
        assert_eq!(texels[top * 4], 85);
        assert!(
            tint.iter().all(|c| (c - 1.0).abs() < 1e-4),
            "grey light: grey smoke"
        );
    }

    #[test]
    fn bullets_only_mark_smokes_they_cross() {
        let mut smokes = CsSmokes::default();
        smokes.pop([0.0, 0.0, 0.0], 0);
        smokes.bullet([-1000.0, 0.0, 50.0], [1000.0, 0.0, 50.0], 10);
        smokes.bullet([-1000.0, 2000.0, 50.0], [1000.0, 2000.0, 50.0], 10);
        assert_eq!(smokes.holes.len(), 1);
        smokes.pop([0.5, 0.0, 0.0], 100);
        assert_eq!(smokes.smokes.len(), 1, "the same pop twice is one smoke");
    }
}
