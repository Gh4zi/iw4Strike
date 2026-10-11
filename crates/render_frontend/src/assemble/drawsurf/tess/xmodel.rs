use render_anim::geometry::install_retained_packed;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

use bevy::prelude::*;

use super::smodel::{PackedVertexRefusal, SmodelPassMaterial, SmodelVertex};

pub use render_anim::{
    BODY_PACKED_UNAVAILABLE, DynEntAssetDraw, DynEntDrawPlan, DynEntOwnerDraw,
    FPV_PACKED_EMPTY_PLAN, FPV_PACKED_UNAVAILABLE, FpvDrawPlan, FpvSurfaceDraw, ItemAssetDraw,
    ItemDrawPlan, ItemOwnerDraw, MissileDrawPlan, MissileOwnerDraw, RemoteBodyDrawPlan,
    RemoteBodySurfaceDraw, ScriptModelAssetDraw, ScriptModelDrawPlan, ScriptModelOwnerDraw,
    XMODEL_OBJECT_ID_BODY_BASE, XMODEL_OBJECT_ID_DYNENT_BASE, XMODEL_OBJECT_ID_ITEM_BASE,
    XMODEL_OBJECT_ID_MISSILE_BASE,
};
pub use render_anim::{
    BodyPackedSession, append_dynent_asset, append_dynent_surfaces, append_item_surfaces,
    append_missile_surfaces, append_remote_body_cpu_blob, append_script_model_asset,
    authored_lit_xmodel_pass_material, body_lit_pass_material, bound_lit_xmodel_pass_material,
    finish_remote_body_draw_plan, install_body_packed_session, overwrite_script_model_asset,
    push_remote_body_cpu_draw, retain_script_model_assets, take_body_packed_session,
};
pub use render_fx::append_fx_model_asset;

pub use render_frame::XMODEL_OBJECT_ID_VIEWMODEL;
pub use render_fx::{FxModelAssetDraw, FxModelDrawPlan, XMODEL_OBJECT_ID_FX_BASE};
pub use render_scene::{XModelColourRefusal, XModelSurfaceDraw};

#[derive(Resource, Clone, Debug, Default)]
pub struct XModelDrawPlan {
    pub vertices: Vec<SmodelVertex>,

    pub decoded_n: usize,
    pub indices: Vec<u32>,
    pub surface_ranges: Vec<(u32, u32)>,
    pub materials: Vec<SmodelPassMaterial>,
    pub draws: Vec<XModelSurfaceDraw>,

    pub fx_object_id_exhausted: u32,
    pub revision: u64,

    pub topology_revision: u64,

    pub packed_vertices: asset_world::PackedVertexPayload,

    pub packed_share: Option<Arc<Vec<[u8; asset_iw4::size::GFX_PACKED_VERTEX]>>>,

    pub packed_segments: render_frame::PackedSegments,

    published_packed: Option<PackedContentKey>,

    pub index_share: Option<Arc<Vec<u32>>>,
    pub range_share: Option<Arc<Vec<(u32, u32)>>>,
    pub decoded_share: Option<Arc<Vec<SmodelVertex>>>,
    packed_banks: super::ShareBanks<[u8; asset_iw4::size::GFX_PACKED_VERTEX]>,
    index_banks: super::ShareBanks<u32>,
    range_banks: super::ShareBanks<(u32, u32)>,

    last_input: Option<XModelMergeStamp>,
    last_topology: Option<XModelTopologyKey>,
    concat_layout: bool,
    last_admitted: u8,

    packed_owner_vertices: [[u64; 8]; super::SHARE_BANKS],

    /// Where the last full merge put each producer (producer order as in
    /// `concat_packed_owners`), so a merge whose topology and materials held can redo just
    /// the draws and the packed rows of a compacted layout.
    owner_layouts: [OwnerLayout; 8],
    owner_draws: [Vec<XModelSurfaceDraw>; 8],
    compact_ready: bool,
    /// Bumped by every full merge; a bank is patched only if it was written under the
    /// current layout.
    layout_epoch: u64,
    bank_layout: [u64; super::SHARE_BANKS],
}

const UNMAPPED: u32 = u32::MAX;

#[derive(Clone, Debug, Default)]
struct OwnerLayout {
    appended: bool,
    /// Every surface was drawn: the producer went in whole at these bases.
    whole: bool,
    range_base: u32,
    mat_base: u32,
    /// Packed rows copied whole, or the source rows gathered in order (compacted).
    whole_rows: usize,
    rows: Vec<u32>,
    /// One past the highest gathered source row.
    need: usize,
    /// Compacted: source surface / material -> merged index, `UNMAPPED` where unused.
    surf_map: Vec<u32>,
    mat_map: Vec<u32>,
}

impl OwnerLayout {
    fn packed_len(&self) -> usize {
        self.whole_rows + self.rows.len()
    }

    fn reset(&mut self) {
        self.appended = false;
        self.whole = false;
        self.range_base = 0;
        self.mat_base = 0;
        self.whole_rows = 0;
        self.rows.clear();
        self.need = 0;
        self.surf_map.clear();
        self.mat_map.clear();
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct XModelMergeStamps {
    pub append_ms: f32,

    pub packed_ms: f32,

    pub concatenated: bool,
}

pub const XMODEL_PACKED_UNAVAILABLE: &str =
    "xmodel merge GfxPackedVertex missing or count-mismatched; decoded float is not packed VB";
pub const XMODEL_PACKED_EMPTY_PLAN: &str = "xmodel plan has no vertices";

impl XModelDrawPlan {
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.decoded_n = 0;
        self.indices.clear();
        self.surface_ranges.clear();
        self.materials.clear();
        self.draws.clear();
        self.fx_object_id_exhausted = 0;
        self.packed_share = None;
        self.packed_segments.forget();
        self.index_share = None;
        self.range_share = None;
        self.decoded_share = None;
        self.last_input = None;
        self.last_topology = None;
        self.concat_layout = false;
        self.last_admitted = 0;
        self.packed_owner_vertices = [[0; 8]; super::SHARE_BANKS];
        self.compact_ready = false;
        self.packed_vertices = asset_world::PackedVertexPayload::Unavailable {
            source_layout: XMODEL_PACKED_UNAVAILABLE,
        };
    }

    pub fn packed_rows(&self) -> Option<&[[u8; asset_iw4::size::GFX_PACKED_VERTEX]]> {
        if let Some(share) = self.packed_share.as_ref() {
            return Some(share.as_slice());
        }
        match &self.packed_vertices {
            asset_world::PackedVertexPayload::Iw4(rows) => Some(rows.as_slice()),
            asset_world::PackedVertexPayload::Unavailable { .. } => None,
        }
    }

    pub fn index_rows(&self) -> &[u32] {
        super::published_rows(&self.index_share)
    }

    pub fn range_rows(&self) -> &[(u32, u32)] {
        super::published_rows(&self.range_share)
    }

    pub fn exact_packed_vertices(
        &self,
    ) -> Result<&[[u8; asset_iw4::size::GFX_PACKED_VERTEX]], PackedVertexRefusal> {
        let table_stride =
            asset_iw4::vertex_decl::stream_extent(asset_iw4::vertex_decl::PACKED_VERTEX_TYPE, 0);
        if table_stride != Some(asset_iw4::size::GFX_PACKED_VERTEX as u16) {
            return Err(PackedVertexRefusal::StrideMismatch { table_stride });
        }
        let vertices = match self.packed_rows() {
            Some(vertices) => vertices,
            None => {
                let source_layout = match &self.packed_vertices {
                    asset_world::PackedVertexPayload::Unavailable { source_layout } => {
                        *source_layout
                    }
                    asset_world::PackedVertexPayload::Iw4(_) => XMODEL_PACKED_UNAVAILABLE,
                };
                return Err(PackedVertexRefusal::ForeignLayout { source_layout });
            }
        };
        let decoded = if self.vertices.is_empty() {
            self.decoded_n
        } else {
            self.vertices.len()
        };
        if vertices.len() != decoded {
            return Err(PackedVertexRefusal::VertexCountMismatch {
                packed: vertices.len(),
                decoded,
            });
        }
        Ok(vertices)
    }
}

pub const HOST_XMODEL_RIGID_TESS_INFO_PACKED_ARM: u8 = 1;

pub fn xmodel_rigid_vert_decl_type(tess_info_byte_10: u8) -> u8 {
    render_frame::xmodel_tess_info_vert_decl_type(tess_info_byte_10)
}

#[must_use]
fn fpv_scene_colour(fpv: &FpvDrawPlan, scene: Option<&crate::GfxScene>) -> bool {
    fpv.admits_colour() && scene.is_some_and(|s| s.scene_dobj_live(crate::SCENE_VIEWMODEL_ENTNUM))
}

#[must_use]
fn scene_ent_admitted(scene: Option<&crate::GfxScene>, entnum: Option<u32>) -> bool {
    match entnum {
        None => true,
        Some(n) => scene.is_some_and(|s| s.scene_ent_live(n) && !s.scene_ent_skips_draw(n)),
    }
}

/// Whether a remote body's surfaces are drawn, and with what colour refusal. A body out of the
/// camera's walk is posed only while it is inside a sun partition, and is drawn only into the
/// sun shadow.
#[must_use]
fn body_admission(
    scene: Option<&crate::GfxScene>,
    entnum: Option<u32>,
) -> Option<Option<XModelColourRefusal>> {
    if scene_ent_admitted(scene, entnum) {
        return Some(None);
    }
    entnum
        .is_some_and(|n| scene.is_some_and(|s| s.scene_ent_hidden(n)))
        .then_some(Some(XModelColourRefusal::CameraFrustum))
}

/// A script model out of the camera's walk is drawn only into the sun shadow: its producer
/// keeps it, colour refused, while it is inside a sun partition.
#[must_use]
fn script_draw_admitted(scene: Option<&crate::GfxScene>, draw: &XModelSurfaceDraw) -> bool {
    scene_ent_admitted(scene, draw.scene_entnum)
        || (draw.colour_refusal.is_some()
            && draw
                .scene_entnum
                .is_some_and(|n| scene.is_some_and(|s| s.scene_ent_hidden(n))))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct XModelTopologyKey {
    generation: [u64; 8],
    topology: [u64; 8],
    admission: [u64; 8],
    occupancy: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct XModelMergeStamp {
    topology: XModelTopologyKey,
    vertices: [u64; 8],
    materials: [u64; 8],
    draws: [u64; 8],
    producer_revision: [u64; 8],
    fpv_world: [u32; 16],
    fpv_lighting: u32,
    fpv_scene_light: u8,
    fpv_probe: u8,
    fpv_visible: bool,
    fpv_drawgun: Option<i32>,
    fpv_colour: bool,
    sky_eye: Option<[u32; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PackedContentKey {
    generation: [u64; 8],
    admission: [u64; 8],
    occupancy: u64,
    admitted: u8,
    rows: usize,
    vertices: [u64; 8],
}

impl PackedContentKey {
    fn new(stamp: &XModelMergeStamp, admitted: u8, rows: usize) -> Self {
        Self {
            generation: stamp.topology.generation,
            admission: stamp.topology.admission,
            occupancy: stamp.topology.occupancy,
            admitted,
            rows,
            vertices: stamp.vertices,
        }
    }
}

const ADMIT_BODY: u8 = 1 << 0;
const ADMIT_FPV: u8 = 1 << 1;
const ADMIT_SCRIPT: u8 = 1 << 2;
const ADMIT_MISSILE: u8 = 1 << 3;
const ADMIT_ITEM: u8 = 1 << 4;
const ADMIT_FX: u8 = 1 << 5;
const ADMIT_DYNENT: u8 = 1 << 6;
const ADMIT_SKY: u8 = 1 << 7;

struct ProducerKeys {
    generation: [u64; 8],
    topology: [u64; 8],
    admission: [u64; 8],
    vertices: [u64; 8],
    materials: [u64; 8],
    draws: [u64; 8],
    producer_revision: [u64; 8],
}

fn producer_keys(
    fpv: &FpvDrawPlan,
    bodies: &RemoteBodyDrawPlan,
    scripts: &ScriptModelDrawPlan,
    missiles: &MissileDrawPlan,
    items: &ItemDrawPlan,
    fx_models: &FxModelDrawPlan,
    dynents: &DynEntDrawPlan,
    sky: Option<&super::sky::SkyModelDrawPlan>,
) -> ProducerKeys {
    let sky_rev = sky.map(|sky| sky.revisions).unwrap_or_default();
    let sky_gen = sky.map(|sky| sky.generation).unwrap_or(0);
    ProducerKeys {
        generation: [
            fpv.generation,
            bodies.generation,
            scripts.generation,
            missiles.generation,
            items.generation,
            fx_models.generation,
            dynents.generation,
            sky_gen,
        ],
        topology: [
            fpv.revisions.topology,
            bodies.revisions.topology,
            scripts.revisions.topology,
            missiles.revisions.topology,
            items.revisions.topology,
            fx_models.revisions.topology,
            dynents.revisions.topology,
            sky_rev.topology,
        ],
        admission: [
            fpv.revisions.admission,
            bodies.revisions.admission,
            scripts.revisions.admission,
            missiles.revisions.admission,
            items.revisions.admission,
            fx_models.revisions.admission,
            dynents.revisions.admission,
            sky_rev.admission,
        ],
        vertices: [
            fpv.revisions.vertices,
            bodies.revisions.vertices,
            scripts.revisions.vertices,
            missiles.revisions.vertices,
            items.revisions.vertices,
            fx_models.revisions.vertices,
            dynents.revisions.vertices,
            sky_rev.vertices,
        ],
        materials: [
            fpv.revisions.materials,
            bodies.revisions.materials,
            scripts.revisions.materials,
            missiles.revisions.materials,
            items.revisions.materials,
            fx_models.revisions.materials,
            dynents.revisions.materials,
            sky_rev.materials,
        ],
        draws: [
            fpv.revisions.draws,
            bodies.revisions.draws,
            scripts.revisions.draws,
            missiles.revisions.draws,
            items.revisions.draws,
            fx_models.revisions.draws,
            dynents.revisions.draws,
            sky_rev.draws,
        ],
        producer_revision: [
            fpv.revision,
            bodies.revision,
            scripts.revision,
            missiles.revision,
            items.revision,
            fx_models.revision,
            dynents.revision,
            0,
        ],
    }
}

fn xmodel_merge_stamp(
    fpv: &FpvDrawPlan,
    bodies: &RemoteBodyDrawPlan,
    scripts: &ScriptModelDrawPlan,
    missiles: &MissileDrawPlan,
    items: &ItemDrawPlan,
    fx_models: &FxModelDrawPlan,
    dynents: &DynEntDrawPlan,
    scene: Option<&crate::GfxScene>,
    sky: Option<(&super::sky::SkyModelDrawPlan, Vec3)>,
) -> XModelMergeStamp {
    // The one thing still read out of the producers' rows here is which scene
    // entity each draw belongs to, because whether that entity is admitted this
    // frame is the scene's answer, not the producer's — a frame input, not a
    // guess at whether the payload changed. The payload fingerprint that used
    // to sit next to it is gone: `vertices`, `draws` and `topology` below are
    // the producers' own published verdicts now.
    let mut occupancy = DefaultHasher::new();
    fpv_scene_colour(fpv, scene).hash(&mut occupancy);
    for draw in bodies.draws() {
        body_admission(scene, draw.scene_entnum)
            .map(|refusal| refusal.is_some())
            .hash(&mut occupancy);
    }
    for draw in scripts.draws() {
        script_draw_admitted(scene, draw).hash(&mut occupancy);
    }
    for draw in missiles.draws() {
        scene_ent_admitted(scene, draw.scene_entnum).hash(&mut occupancy);
    }
    for draw in items.draws() {
        scene_ent_admitted(scene, draw.scene_entnum).hash(&mut occupancy);
    }
    let sky_eye = sky.map(|(_, eye)| [eye.x.to_bits(), eye.y.to_bits(), eye.z.to_bits()]);
    let keys = producer_keys(
        fpv,
        bodies,
        scripts,
        missiles,
        items,
        fx_models,
        dynents,
        sky.map(|(sky, _)| sky),
    );
    XModelMergeStamp {
        topology: XModelTopologyKey {
            generation: keys.generation,
            topology: keys.topology,
            admission: keys.admission,
            occupancy: occupancy.finish(),
        },
        vertices: keys.vertices,
        materials: keys.materials,
        draws: keys.draws,
        producer_revision: keys.producer_revision,
        fpv_world: mat4_bits(fpv.world_from_local),
        fpv_lighting: fpv.lighting_handle,
        fpv_scene_light: fpv.scene_light_index,
        fpv_probe: fpv.reflection_probe_index,
        fpv_visible: fpv.visible,
        fpv_drawgun: fpv.drawgun,
        fpv_colour: fpv_scene_colour(fpv, scene),
        sky_eye,
    }
}

fn mat4_bits(m: Mat4) -> [u32; 16] {
    let cols = m.to_cols_array();
    std::array::from_fn(|i| cols[i].to_bits())
}

pub fn merge_xmodel_draw_plan(
    merged: &mut XModelDrawPlan,
    fpv: &FpvDrawPlan,
    bodies: &RemoteBodyDrawPlan,
    scripts: &ScriptModelDrawPlan,
    missiles: &MissileDrawPlan,
    items: &ItemDrawPlan,
    fx_models: &FxModelDrawPlan,
    dynents: &DynEntDrawPlan,
    scene: Option<&crate::GfxScene>,
    sky: Option<(&super::sky::SkyModelDrawPlan, Vec3)>,
) -> XModelMergeStamps {
    let stamp = xmodel_merge_stamp(
        fpv, bodies, scripts, missiles, items, fx_models, dynents, scene, sky,
    );
    if merged.last_input == Some(stamp) {
        return XModelMergeStamps {
            concatenated: false,
            ..XModelMergeStamps::default()
        };
    }
    if merged.last_topology == Some(stamp.topology) && merged.concat_layout {
        let prev = merged.last_input;
        let vertices_changed = prev.is_none_or(|p| p.vertices != stamp.vertices);
        let draws_changed = prev.is_none_or(|p| concat_draws_changed(p, &stamp));
        let mut packed_ms = 0.0;
        if vertices_changed {
            let packed_started = Instant::now();
            let prev_packed = merged.packed_share.take();
            let (mut packed, fresh) = merged.packed_banks.take_write_keep();
            let write = merged.packed_banks.write_index();
            if fresh {
                merged.packed_owner_vertices[write] = [0; 8];
            }
            let mut packed_ok = true;
            recopy_concat_packed_bank(
                &mut packed,
                &mut packed_ok,
                fresh,
                &mut merged.packed_owner_vertices[write],
                stamp.vertices,
                merged.last_admitted,
                concat_packed_owners(
                    fpv, bodies, scripts, missiles, items, fx_models, dynents, sky,
                ),
                &mut merged.packed_segments,
            );
            let key = packed_ok
                .then(|| PackedContentKey::new(&stamp, merged.last_admitted, packed.len()));
            let payload = install_retained_packed(
                packed_ok,
                packed,
                merged.decoded_n,
                XMODEL_PACKED_EMPTY_PLAN,
                XMODEL_PACKED_UNAVAILABLE,
            );
            publish_packed_payload(merged, prev_packed, payload, key);
            packed_ms = packed_started.elapsed().as_secs_f32() * 1000.0;
        }
        if draws_changed {
            refresh_concat_draws(
                merged, fpv, bodies, scripts, missiles, items, fx_models, dynents, scene, sky,
            );
        }
        merged.last_input = Some(stamp);
        return XModelMergeStamps {
            concatenated: true,
            packed_ms,
            append_ms: 0.0,
        };
    }
    // A compacted layout (some producer only partly drawn) whose topology and materials
    // held: indices, ranges and materials are last frame's, the draws go back through the
    // last merge's surface and material maps, and the packed rows are gathered again.
    let owners = concat_packed_owners(
        fpv, bodies, scripts, missiles, items, fx_models, dynents, sky,
    );
    if merged.compact_ready
        && !merged.concat_layout
        && merged.last_topology == Some(stamp.topology)
        && let Some(prev) = merged.last_input
        && prev.materials == stamp.materials
        && packed_layouts_fit(&merged.owner_layouts, &owners)
        && (!concat_draws_changed(prev, &stamp)
            || refresh_compact_draws(
                merged, &owners, fpv, bodies, scripts, missiles, items, fx_models, dynents, scene,
                sky,
            ))
    {
        let packed_started = Instant::now();
        let prev_packed = merged.packed_share.take();
        let (mut packed, fresh) = merged.packed_banks.take_write_keep();
        let write = merged.packed_banks.write_index();
        let patch = !fresh && merged.bank_layout[write] == merged.layout_epoch;
        regather_packed_bank(
            &mut packed,
            patch,
            &mut merged.packed_owner_vertices[write],
            stamp.vertices,
            &merged.owner_layouts,
            &owners,
            &mut merged.packed_segments,
        );
        merged.bank_layout[write] = merged.layout_epoch;
        let key = PackedContentKey::new(&stamp, merged.last_admitted, packed.len());
        let payload = install_retained_packed(
            true,
            packed,
            merged.decoded_n,
            XMODEL_PACKED_EMPTY_PLAN,
            XMODEL_PACKED_UNAVAILABLE,
        );
        publish_packed_payload(merged, prev_packed, payload, Some(key));
        merged.last_input = Some(stamp);
        return XModelMergeStamps {
            concatenated: false,
            packed_ms: packed_started.elapsed().as_secs_f32() * 1000.0,
            append_ms: 0.0,
        };
    }
    let prev_packed = merged.packed_share.take();
    let prev_index = merged.index_share.take();
    let prev_range = merged.range_share.take();
    let mut packed = merged.packed_banks.take_write();
    let packed_write = merged.packed_banks.write_index();
    merged.packed_owner_vertices[packed_write] = [0; 8];
    merged.packed_segments.forget();
    packed.clear();
    let indices = merged.index_banks.take_write();
    let ranges = merged.range_banks.take_write();
    merged.clear();
    merged.indices = indices;
    merged.surface_ranges = ranges;
    merged.indices.clear();
    merged.surface_ranges.clear();
    merged.concat_layout = true;

    packed.reserve(
        packed_row_count(bodies.packed_vertices())
            + packed_row_count(fpv.packed_vertices())
            + packed_row_count(scripts.packed_vertices())
            + packed_row_count(missiles.packed_vertices())
            + packed_row_count(items.packed_vertices())
            + packed_row_count(fx_models.packed_vertices())
            + packed_row_count(dynents.packed_vertices()),
    );
    let mut packed_ok = true;
    let append_started = Instant::now();
    merged.layout_epoch = merged.layout_epoch.wrapping_add(1);
    let mut layouts = std::mem::take(&mut merged.owner_layouts);
    let mut lists = std::mem::take(&mut merged.owner_draws);
    merged.fx_object_id_exhausted = owner_draw_lists(
        &mut lists, fpv, bodies, scripts, missiles, items, fx_models, dynents, scene, sky,
    );
    for ((layout, owner), draws) in layouts.iter_mut().zip(&owners).zip(lists.iter_mut()) {
        layout.reset();
        append_admitted_source(merged, &mut packed, &mut packed_ok, layout, owner, draws);
    }
    merged.owner_layouts = layouts;
    merged.owner_draws = lists;
    let append_ms = append_started.elapsed().as_secs_f32() * 1000.0;
    let packed_started = Instant::now();
    if packed_ok {
        merged.packed_owner_vertices[packed_write] = stamp.vertices;
    }
    merged.compact_ready = packed_ok && !merged.concat_layout;
    merged.bank_layout[packed_write] = merged.layout_epoch;
    merged.last_admitted = admitted_mask(
        fpv, bodies, scripts, missiles, items, fx_models, dynents, scene, sky,
    );
    let key = packed_ok.then(|| PackedContentKey::new(&stamp, merged.last_admitted, packed.len()));
    let payload = install_retained_packed(
        packed_ok,
        packed,
        merged.decoded_n,
        XMODEL_PACKED_EMPTY_PLAN,
        XMODEL_PACKED_UNAVAILABLE,
    );
    publish_packed_payload(merged, prev_packed, payload, key);
    publish_topology_shares(
        prev_index,
        prev_range,
        &mut merged.indices,
        &mut merged.surface_ranges,
        &mut merged.index_share,
        &mut merged.range_share,
        &mut merged.topology_revision,
        &mut merged.index_banks,
        &mut merged.range_banks,
    );
    publish_decoded_share(merged);
    merged.last_input = Some(stamp);
    merged.last_topology = Some(stamp.topology);
    let packed_ms = packed_started.elapsed().as_secs_f32() * 1000.0;
    XModelMergeStamps {
        append_ms,
        packed_ms,
        concatenated: true,
    }
}

fn dense_fx_object_ids(
    draws: &[XModelSurfaceDraw],
    max_non_fx: Option<u16>,
) -> (Vec<XModelSurfaceDraw>, u32) {
    let mut next = match max_non_fx {
        Some(max_id) => max_id
            .checked_add(1)
            .map(|id| id.max(XMODEL_OBJECT_ID_FX_BASE)),
        None => Some(XMODEL_OBJECT_ID_FX_BASE),
    };
    let mut previous_source = None;
    let mut current = None;
    let mut exhausted = 0u32;
    let mut dense = Vec::with_capacity(draws.len());
    for draw in draws {
        if previous_source != Some(draw.object_id) {
            previous_source = Some(draw.object_id);
            current = next;
            next = next.and_then(|id| id.checked_add(1));
        }
        let Some(object_id) = current else {
            exhausted = exhausted.saturating_add(1);
            continue;
        };
        dense.push(XModelSurfaceDraw { object_id, ..*draw });
    }
    (dense, exhausted)
}

fn concat_draws_changed(prev: XModelMergeStamp, stamp: &XModelMergeStamp) -> bool {
    prev.draws != stamp.draws
        || prev.materials != stamp.materials
        || prev.fpv_world != stamp.fpv_world
        || prev.fpv_lighting != stamp.fpv_lighting
        || prev.fpv_scene_light != stamp.fpv_scene_light
        || prev.fpv_probe != stamp.fpv_probe
        || prev.fpv_visible != stamp.fpv_visible
        || prev.fpv_drawgun != stamp.fpv_drawgun
        || prev.fpv_colour != stamp.fpv_colour
        || prev.sky_eye != stamp.sky_eye
}

struct ConcatPackedOwner<'a> {
    admit: u8,
    rev_i: usize,
    payload: &'a asset_world::PackedVertexPayload,
    decoded_n: usize,
    indices: &'a [u32],
    surface_ranges: &'a [(u32, u32)],
    materials: &'a [SmodelPassMaterial],
}

fn concat_packed_owners<'a>(
    fpv: &'a FpvDrawPlan,
    bodies: &'a RemoteBodyDrawPlan,
    scripts: &'a ScriptModelDrawPlan,
    missiles: &'a MissileDrawPlan,
    items: &'a ItemDrawPlan,
    fx_models: &'a FxModelDrawPlan,
    dynents: &'a DynEntDrawPlan,
    sky: Option<(&'a super::sky::SkyModelDrawPlan, Vec3)>,
) -> [ConcatPackedOwner<'a>; 8] {
    let sky_geom = sky.map(|(sky, _)| &sky.geometry);
    [
        ConcatPackedOwner {
            admit: ADMIT_BODY,
            rev_i: 1,
            payload: bodies.packed_vertices(),
            decoded_n: bodies.decoded_n(),
            indices: bodies.indices(),
            surface_ranges: bodies.surface_ranges(),
            materials: bodies.materials(),
        },
        ConcatPackedOwner {
            admit: ADMIT_FPV,
            rev_i: 0,
            payload: fpv.packed_vertices(),
            decoded_n: fpv.decoded_n(),
            indices: fpv.indices(),
            surface_ranges: fpv.surface_ranges(),
            materials: fpv.materials(),
        },
        ConcatPackedOwner {
            admit: ADMIT_SCRIPT,
            rev_i: 2,
            payload: scripts.packed_vertices(),
            decoded_n: scripts.vertices().len(),
            indices: scripts.indices(),
            surface_ranges: scripts.surface_ranges(),
            materials: scripts.materials(),
        },
        ConcatPackedOwner {
            admit: ADMIT_MISSILE,
            rev_i: 3,
            payload: missiles.packed_vertices(),
            decoded_n: missiles.vertices().len(),
            indices: missiles.indices(),
            surface_ranges: missiles.surface_ranges(),
            materials: missiles.materials(),
        },
        ConcatPackedOwner {
            admit: ADMIT_ITEM,
            rev_i: 4,
            payload: items.packed_vertices(),
            decoded_n: items.vertices().len(),
            indices: items.indices(),
            surface_ranges: items.surface_ranges(),
            materials: items.materials(),
        },
        ConcatPackedOwner {
            admit: ADMIT_FX,
            rev_i: 5,
            payload: fx_models.packed_vertices(),
            decoded_n: fx_models.vertices().len(),
            indices: fx_models.indices(),
            surface_ranges: fx_models.surface_ranges(),
            materials: fx_models.materials(),
        },
        ConcatPackedOwner {
            admit: ADMIT_DYNENT,
            rev_i: 6,
            payload: dynents.packed_vertices(),
            decoded_n: dynents.vertices().len(),
            indices: dynents.indices(),
            surface_ranges: dynents.surface_ranges(),
            materials: dynents.materials(),
        },
        ConcatPackedOwner {
            admit: ADMIT_SKY,
            rev_i: 7,
            payload: sky_geom
                .map(|g| &g.packed_vertices)
                .unwrap_or(bodies.packed_vertices()),
            decoded_n: sky_geom.map(|g| g.decoded_vertices().len()).unwrap_or(0),
            indices: sky_geom.map(|g| g.indices()).unwrap_or(&[]),
            surface_ranges: sky_geom.map(|g| g.surface_ranges()).unwrap_or(&[]),
            materials: sky_geom.map(|g| g.materials.as_slice()).unwrap_or(&[]),
        },
    ]
}

fn concat_packed_layout(admitted: u8, owners: &[ConcatPackedOwner<'_>; 8]) -> ([usize; 8], usize) {
    let mut counts = [0usize; 8];
    let mut expected = 0usize;
    for (i, owner) in owners.iter().enumerate() {
        if admitted & owner.admit == 0 || owner.decoded_n == 0 {
            continue;
        }
        counts[i] = packed_row_count(owner.payload);
        expected = expected.saturating_add(counts[i]);
    }
    (counts, expected)
}

fn concat_packed_segments(
    counts: [usize; 8],
    stamp_vertices: [u64; 8],
    owners: &[ConcatPackedOwner<'_>; 8],
) -> [render_frame::PackedSegment; render_frame::PACKED_SEGMENT_OWNERS] {
    let mut start = 0u32;
    std::array::from_fn(|i| {
        let rows = u32::try_from(counts[i]).unwrap_or(u32::MAX);
        let segment = render_frame::PackedSegment {
            start,
            rows,
            revision: stamp_vertices[owners[i].rev_i],
        };
        start = start.saturating_add(rows);
        segment
    })
}

fn recopy_concat_packed_bank(
    packed: &mut Vec<[u8; asset_iw4::size::GFX_PACKED_VERTEX]>,
    packed_ok: &mut bool,
    fresh: bool,
    bank_vertices: &mut [u64; 8],
    stamp_vertices: [u64; 8],
    admitted: u8,
    owners: [ConcatPackedOwner<'_>; 8],
    published: &mut render_frame::PackedSegments,
) {
    let (counts, expected) = concat_packed_layout(admitted, &owners);
    let can_patch = !fresh && packed.len() == expected && *packed_ok;
    if !can_patch {
        packed.clear();
        packed.reserve(expected);
        for owner in &owners {
            if admitted & owner.admit != 0 {
                append_packed_source(packed, packed_ok, owner.payload, owner.decoded_n);
            }
        }
        *bank_vertices = if *packed_ok { stamp_vertices } else { [0; 8] };
        published.forget();
        return;
    }
    let mut off = 0usize;
    for (i, owner) in owners.iter().enumerate() {
        if admitted & owner.admit == 0 || owner.decoded_n == 0 {
            continue;
        }
        let n = counts[i];
        if n != owner.decoded_n {
            *packed_ok = false;
            packed.clear();
            *bank_vertices = [0; 8];
            published.forget();
            return;
        }
        if bank_vertices[owner.rev_i] != stamp_vertices[owner.rev_i] {
            match owner.payload {
                asset_world::PackedVertexPayload::Iw4(rows) if rows.len() == n => {
                    packed[off..off + n].copy_from_slice(rows);
                }
                _ => {
                    *packed_ok = false;
                    packed.clear();
                    *bank_vertices = [0; 8];
                    published.forget();
                    return;
                }
            }
        }
        off = off.saturating_add(n);
    }
    *bank_vertices = stamp_vertices;
    published.publish(concat_packed_segments(counts, stamp_vertices, &owners));
}

/// Each producer's admitted draws as the full merge appends them (producer order as in
/// `concat_packed_owners`), before any remap. Returns how many FX draws ran out of object
/// ids.
fn owner_draw_lists(
    lists: &mut [Vec<XModelSurfaceDraw>; 8],
    fpv: &FpvDrawPlan,
    bodies: &RemoteBodyDrawPlan,
    scripts: &ScriptModelDrawPlan,
    missiles: &MissileDrawPlan,
    items: &ItemDrawPlan,
    fx_models: &FxModelDrawPlan,
    dynents: &DynEntDrawPlan,
    scene: Option<&crate::GfxScene>,
    sky: Option<(&super::sky::SkyModelDrawPlan, Vec3)>,
) -> u32 {
    for list in lists.iter_mut() {
        list.clear();
    }
    let [
        body_list,
        fpv_list,
        script_list,
        missile_list,
        item_list,
        fx_list,
        dynent_list,
        sky_list,
    ] = lists;
    let mut next_body_object = XMODEL_OBJECT_ID_BODY_BASE;
    body_list.extend(bodies.draws().iter().filter_map(|d| {
        body_admission(scene, d.scene_entnum).map(|colour_refusal| {
            let object_id = next_body_object;
            next_body_object = next_body_object.saturating_add(1);
            XModelSurfaceDraw {
                surface: d.surface,
                material: d.material,
                world_from_local: d.world_from_local,
                lighting_handle: d.lighting_handle,

                pending_lighting: None,
                colour_refusal,
                object_id,
                scene_light_index: d.scene_light_index,
                reflection_probe_index: d.reflection_probe_index,
                packed_lighting: None,
                is_scope: false,
                scene_entnum: d.scene_entnum,
                body_client: d.scene_entnum,
                // Remote bodies carry no radius here; unbounded keeps
                // every player caster in both partitions.
                caster_bound: None,
            }
        })
    }));
    if fpv_scene_colour(fpv, scene) {
        fpv_list.extend(fpv.draws().iter().map(|d| XModelSurfaceDraw {
            surface: d.surface,
            material: d.material,
            world_from_local: fpv.world_from_local,
            lighting_handle: fpv.lighting_handle,
            pending_lighting: None,
            colour_refusal: None,
            object_id: XMODEL_OBJECT_ID_VIEWMODEL,
            scene_light_index: fpv.scene_light_index,
            reflection_probe_index: fpv.reflection_probe_index,
            packed_lighting: None,
            is_scope: d.is_scope,
            scene_entnum: Some(crate::SCENE_VIEWMODEL_ENTNUM),
            body_client: None,
            caster_bound: None,
        }));
    }
    script_list.extend(
        scripts
            .draws()
            .iter()
            .copied()
            .filter(|d| script_draw_admitted(scene, d)),
    );
    for (list, draws) in [
        (missile_list, missiles.draws()),
        (item_list, items.draws()),
    ] {
        list.extend(
            draws
                .iter()
                .copied()
                .filter(|d| scene_ent_admitted(scene, d.scene_entnum)),
        );
    }
    let (fx_draws, fx_object_id_exhausted) = dense_fx_object_ids(
        fx_models.draws(),
        dynents.draws().iter().map(|draw| draw.object_id).max(),
    );
    *fx_list = fx_draws;
    dynent_list.extend_from_slice(dynents.draws());
    if let Some((sky, eye)) = sky {
        sky_list.extend(sky.draws.iter().map(|draw| XModelSurfaceDraw {
            world_from_local: Mat4::from_translation(eye),
            ..*draw
        }));
    }
    fx_object_id_exhausted
}

/// Rebuilds the draws of a compacted layout through the last full merge's maps. Refuses
/// (leaving the plan as it was) when any producer now draws a different set of surfaces or
/// materials, since that changes the compaction itself.
fn refresh_compact_draws(
    merged: &mut XModelDrawPlan,
    owners: &[ConcatPackedOwner<'_>; 8],
    fpv: &FpvDrawPlan,
    bodies: &RemoteBodyDrawPlan,
    scripts: &ScriptModelDrawPlan,
    missiles: &MissileDrawPlan,
    items: &ItemDrawPlan,
    fx_models: &FxModelDrawPlan,
    dynents: &DynEntDrawPlan,
    scene: Option<&crate::GfxScene>,
    sky: Option<(&super::sky::SkyModelDrawPlan, Vec3)>,
) -> bool {
    let mut lists = std::mem::take(&mut merged.owner_draws);
    let fx_object_id_exhausted = owner_draw_lists(
        &mut lists, fpv, bodies, scripts, missiles, items, fx_models, dynents, scene, sky,
    );
    let fits = merged
        .owner_layouts
        .iter()
        .zip(owners)
        .zip(lists.iter())
        .all(|((layout, owner), draws)| owner_draws_fit(layout, owner, draws));
    if fits {
        merged.draws.clear();
        for (layout, draws) in merged.owner_layouts.iter().zip(lists.iter_mut()) {
            if layout.whole {
                merged.draws.extend(draws.drain(..).map(|mut d| {
                    d.surface = d.surface.saturating_add(layout.range_base);
                    d.material = d.material.saturating_add(layout.mat_base);
                    d
                }));
            } else {
                merged.draws.extend(
                    draws
                        .drain(..)
                        .map(|d| compact_draw(d, &layout.surf_map, &layout.mat_map)),
                );
            }
        }
        merged.fx_object_id_exhausted = fx_object_id_exhausted;
    }
    merged.owner_draws = lists;
    fits
}

/// Whether a producer's current draws use exactly the surfaces and materials its layout
/// was built for.
fn owner_draws_fit(
    layout: &OwnerLayout,
    owner: &ConcatPackedOwner<'_>,
    draws: &[XModelSurfaceDraw],
) -> bool {
    if layout.appended == draws.is_empty() {
        return false;
    }
    if !layout.appended {
        return true;
    }
    let surface_n = owner.surface_ranges.len();
    let mut used = vec![false; surface_n];
    for d in draws {
        if let Some(slot) = used.get_mut(d.surface as usize) {
            *slot = true;
        }
    }
    if layout.whole {
        return surface_n > 0 && used.iter().all(|&live| live);
    }
    if layout.surf_map.len() != surface_n || layout.mat_map.len() != owner.materials.len() {
        return false;
    }
    if used
        .iter()
        .zip(&layout.surf_map)
        .any(|(&live, &mapped)| live != (mapped != UNMAPPED))
    {
        return false;
    }
    let mut used_mat = vec![false; owner.materials.len()];
    for d in draws {
        if let Some(slot) = used_mat.get_mut(d.material as usize) {
            *slot = true;
        }
    }
    used_mat
        .iter()
        .zip(&layout.mat_map)
        .all(|(&live, &mapped)| live == (mapped != UNMAPPED))
}

fn packed_layouts_fit(layouts: &[OwnerLayout; 8], owners: &[ConcatPackedOwner<'_>; 8]) -> bool {
    owners.iter().zip(layouts).all(|(owner, layout)| {
        if layout.packed_len() == 0 {
            return true;
        }
        match owner.payload {
            asset_world::PackedVertexPayload::Iw4(rows) => {
                rows.len() == owner.decoded_n
                    && (layout.whole_rows == 0 || layout.whole_rows == rows.len())
                    && layout.need <= rows.len()
            }
            asset_world::PackedVertexPayload::Unavailable { .. } => false,
        }
    })
}

/// Gathers a compacted layout's packed rows again from the producers' payloads (checked by
/// `packed_layouts_fit`). A bank already holding this layout only takes the producers whose
/// vertices moved since it was written.
fn regather_packed_bank(
    packed: &mut Vec<[u8; asset_iw4::size::GFX_PACKED_VERTEX]>,
    patch: bool,
    bank_vertices: &mut [u64; 8],
    stamp_vertices: [u64; 8],
    layouts: &[OwnerLayout; 8],
    owners: &[ConcatPackedOwner<'_>; 8],
    published: &mut render_frame::PackedSegments,
) {
    let total: usize = layouts.iter().map(OwnerLayout::packed_len).sum();
    let patch = patch && packed.len() == total;
    if !patch {
        packed.clear();
        packed.reserve(total);
    }
    let mut off = 0usize;
    for (owner, layout) in owners.iter().zip(layouts) {
        let n = layout.packed_len();
        if n == 0 {
            continue;
        }
        if patch && bank_vertices[owner.rev_i] == stamp_vertices[owner.rev_i] {
            off += n;
            continue;
        }
        let asset_world::PackedVertexPayload::Iw4(rows) = owner.payload else {
            unreachable!("packed_layouts_fit admits only Iw4 payloads");
        };
        if patch {
            let dst = &mut packed[off..off + n];
            if layout.whole_rows > 0 {
                dst.copy_from_slice(rows);
            } else {
                for (dst, &src) in dst.iter_mut().zip(&layout.rows) {
                    *dst = rows[src as usize];
                }
            }
        } else if layout.whole_rows > 0 {
            packed.extend_from_slice(rows);
        } else {
            packed.extend(layout.rows.iter().map(|&src| rows[src as usize]));
        }
        off += n;
    }
    *bank_vertices = stamp_vertices;
    let counts = std::array::from_fn(|i| layouts[i].packed_len());
    published.publish(concat_packed_segments(counts, stamp_vertices, owners));
}

fn append_concat_owner_draws(
    merged: &mut XModelDrawPlan,
    range_base: &mut u32,
    surface_n: usize,
    materials: &[SmodelPassMaterial],
    draws: impl IntoIterator<Item = XModelSurfaceDraw>,
) {
    let mat_base = merged.materials.len() as u32;
    let mut any = false;
    for mut d in draws {
        if !any {
            any = true;
            merged.materials.extend_from_slice(materials);
        }
        d.surface = d.surface.saturating_add(*range_base);
        d.material = d.material.saturating_add(mat_base);
        merged.draws.push(d);
    }
    if !any {
        return;
    }
    *range_base = range_base.saturating_add(surface_n as u32);
}

fn refresh_concat_draws(
    merged: &mut XModelDrawPlan,
    fpv: &FpvDrawPlan,
    bodies: &RemoteBodyDrawPlan,
    scripts: &ScriptModelDrawPlan,
    missiles: &MissileDrawPlan,
    items: &ItemDrawPlan,
    fx_models: &FxModelDrawPlan,
    dynents: &DynEntDrawPlan,
    scene: Option<&crate::GfxScene>,
    sky: Option<(&super::sky::SkyModelDrawPlan, Vec3)>,
) {
    merged.draws.clear();
    merged.materials.clear();
    let mut range_base = 0u32;
    let mut next_body_object = XMODEL_OBJECT_ID_BODY_BASE;
    let (fx_draws, fx_object_id_exhausted) = dense_fx_object_ids(
        fx_models.draws(),
        dynents.draws().iter().map(|draw| draw.object_id).max(),
    );
    merged.fx_object_id_exhausted = fx_object_id_exhausted;
    append_concat_owner_draws(
        merged,
        &mut range_base,
        bodies.surface_ranges().len(),
        bodies.materials(),
        bodies.draws().iter().filter_map(|d| {
            body_admission(scene, d.scene_entnum).map(|colour_refusal| {
                let object_id = next_body_object;
                next_body_object = next_body_object.saturating_add(1);
                XModelSurfaceDraw {
                    surface: d.surface,
                    material: d.material,
                    world_from_local: d.world_from_local,
                    lighting_handle: d.lighting_handle,
                    pending_lighting: None,
                    colour_refusal,
                    object_id,
                    scene_light_index: d.scene_light_index,
                    reflection_probe_index: d.reflection_probe_index,
                    packed_lighting: None,
                    is_scope: false,
                    scene_entnum: d.scene_entnum,
                    body_client: d.scene_entnum,
                    // Remote bodies carry no radius here; unbounded keeps
                    // every player caster in both partitions.
                    caster_bound: None,
                }
            })
        }),
    );
    if fpv_scene_colour(fpv, scene) {
        append_concat_owner_draws(
            merged,
            &mut range_base,
            fpv.surface_ranges().len(),
            fpv.materials(),
            fpv.draws().iter().map(|d| XModelSurfaceDraw {
                surface: d.surface,
                material: d.material,
                world_from_local: fpv.world_from_local,
                lighting_handle: fpv.lighting_handle,
                pending_lighting: None,
                colour_refusal: None,
                object_id: XMODEL_OBJECT_ID_VIEWMODEL,
                scene_light_index: fpv.scene_light_index,
                reflection_probe_index: fpv.reflection_probe_index,
                packed_lighting: None,
                is_scope: d.is_scope,
                scene_entnum: Some(crate::SCENE_VIEWMODEL_ENTNUM),
                body_client: None,
                caster_bound: None,
            }),
        );
    }
    append_concat_owner_draws(
        merged,
        &mut range_base,
        scripts.surface_ranges().len(),
        scripts.materials(),
        scripts
            .draws()
            .iter()
            .copied()
            .filter(|d| script_draw_admitted(scene, d)),
    );
    append_concat_owner_draws(
        merged,
        &mut range_base,
        missiles.surface_ranges().len(),
        missiles.materials(),
        missiles
            .draws()
            .iter()
            .copied()
            .filter(|d| scene_ent_admitted(scene, d.scene_entnum)),
    );
    append_concat_owner_draws(
        merged,
        &mut range_base,
        items.surface_ranges().len(),
        items.materials(),
        items
            .draws()
            .iter()
            .copied()
            .filter(|d| scene_ent_admitted(scene, d.scene_entnum)),
    );
    append_concat_owner_draws(
        merged,
        &mut range_base,
        fx_models.surface_ranges().len(),
        fx_models.materials(),
        fx_draws,
    );
    append_concat_owner_draws(
        merged,
        &mut range_base,
        dynents.surface_ranges().len(),
        dynents.materials(),
        dynents.draws().iter().copied(),
    );
    if let Some((sky, eye)) = sky {
        append_concat_owner_draws(
            merged,
            &mut range_base,
            sky.geometry.surface_ranges().len(),
            &sky.geometry.materials,
            sky.draws.iter().map(|draw| XModelSurfaceDraw {
                world_from_local: Mat4::from_translation(eye),
                ..*draw
            }),
        );
    }
}

fn admitted_mask(
    fpv: &FpvDrawPlan,
    bodies: &RemoteBodyDrawPlan,
    scripts: &ScriptModelDrawPlan,
    missiles: &MissileDrawPlan,
    items: &ItemDrawPlan,
    fx_models: &FxModelDrawPlan,
    dynents: &DynEntDrawPlan,
    scene: Option<&crate::GfxScene>,
    sky: Option<(&super::sky::SkyModelDrawPlan, Vec3)>,
) -> u8 {
    let mut admitted = 0u8;
    if bodies
        .draws()
        .iter()
        .any(|d| body_admission(scene, d.scene_entnum).is_some())
    {
        admitted |= ADMIT_BODY;
    }
    if fpv_scene_colour(fpv, scene) {
        admitted |= ADMIT_FPV;
    }
    if scripts
        .draws()
        .iter()
        .any(|d| script_draw_admitted(scene, d))
    {
        admitted |= ADMIT_SCRIPT;
    }
    if missiles
        .draws()
        .iter()
        .any(|d| scene_ent_admitted(scene, d.scene_entnum))
    {
        admitted |= ADMIT_MISSILE;
    }
    if items
        .draws()
        .iter()
        .any(|d| scene_ent_admitted(scene, d.scene_entnum))
    {
        admitted |= ADMIT_ITEM;
    }
    if !fx_models.draws().is_empty() {
        admitted |= ADMIT_FX;
    }
    if !dynents.draws().is_empty() {
        admitted |= ADMIT_DYNENT;
    }
    if sky.is_some() {
        admitted |= ADMIT_SKY;
    }
    admitted
}

fn publish_packed_payload(
    merged: &mut XModelDrawPlan,
    prev: Option<Arc<Vec<[u8; asset_iw4::size::GFX_PACKED_VERTEX]>>>,
    payload: asset_world::PackedVertexPayload,
    key: Option<PackedContentKey>,
) {
    match payload {
        asset_world::PackedVertexPayload::Iw4(rows) => {
            let resident = key.is_some() && merged.published_packed == key;
            merged.packed_banks.put_write(rows);
            match prev.as_ref().filter(|_| resident) {
                Some(prev) => merged.packed_share = Some(Arc::clone(prev)),
                None => {
                    merged.revision = merged.revision.wrapping_add(1);
                    merged.packed_share = Some(merged.packed_banks.published());
                    merged.published_packed = key;
                }
            }
            merged.packed_vertices = asset_world::PackedVertexPayload::Unavailable {
                source_layout: XMODEL_PACKED_UNAVAILABLE,
            };
        }
        other => {
            if prev.is_some() {
                merged.revision = merged.revision.wrapping_add(1);
            }
            merged.packed_share = None;
            merged.published_packed = None;
            merged.packed_vertices = other;
        }
    }
}

fn publish_topology_shares(
    prev_index: Option<Arc<Vec<u32>>>,
    prev_range: Option<Arc<Vec<(u32, u32)>>>,
    indices: &mut Vec<u32>,
    ranges: &mut Vec<(u32, u32)>,
    index_share: &mut Option<Arc<Vec<u32>>>,
    range_share: &mut Option<Arc<Vec<(u32, u32)>>>,
    topology_revision: &mut u64,
    index_banks: &mut super::ShareBanks<u32>,
    range_banks: &mut super::ShareBanks<(u32, u32)>,
) {
    let indices_same = prev_index
        .as_ref()
        .is_some_and(|prev| prev.as_slice() == indices.as_slice());
    let ranges_same = prev_range
        .as_ref()
        .is_some_and(|prev| prev.as_slice() == ranges.as_slice());
    if indices_same && ranges_same {
        *index_share = prev_index;
        *range_share = prev_range;
        index_banks.put_write(std::mem::take(indices));
        range_banks.put_write(std::mem::take(ranges));
        return;
    }
    *topology_revision = topology_revision.wrapping_add(1);
    index_banks.put_write(std::mem::take(indices));
    range_banks.put_write(std::mem::take(ranges));
    *index_share = Some(index_banks.published());
    *range_share = Some(range_banks.published());
}

fn publish_decoded_share(merged: &mut XModelDrawPlan) {
    if merged.vertices.is_empty() {
        merged.decoded_share = None;
        return;
    }
    merged.decoded_share = Some(Arc::new(std::mem::take(&mut merged.vertices)));
}

fn packed_row_count(payload: &asset_world::PackedVertexPayload) -> usize {
    match payload {
        asset_world::PackedVertexPayload::Iw4(rows) => rows.len(),
        asset_world::PackedVertexPayload::Unavailable { .. } => 0,
    }
}

fn append_packed_source(
    packed: &mut Vec<[u8; asset_iw4::size::GFX_PACKED_VERTEX]>,
    packed_ok: &mut bool,
    payload: &asset_world::PackedVertexPayload,
    decoded_count: usize,
) {
    if !*packed_ok || decoded_count == 0 {
        return;
    }
    match payload {
        asset_world::PackedVertexPayload::Iw4(vertices) if vertices.len() == decoded_count => {
            packed.extend_from_slice(vertices);
        }
        _ => {
            *packed_ok = false;
            packed.clear();
        }
    }
}

fn append_admitted_source(
    merged: &mut XModelDrawPlan,
    packed: &mut Vec<[u8; asset_iw4::size::GFX_PACKED_VERTEX]>,
    packed_ok: &mut bool,
    layout: &mut OwnerLayout,
    owner: &ConcatPackedOwner<'_>,
    draws: &mut Vec<XModelSurfaceDraw>,
) {
    if draws.is_empty() {
        return;
    }
    layout.appended = true;
    let mut used = vec![false; owner.surface_ranges.len()];
    for d in draws.iter() {
        if let Some(slot) = used.get_mut(d.surface as usize) {
            *slot = true;
        }
    }
    let all_surfaces_live = !used.is_empty() && used.iter().all(|&live| live);
    if all_surfaces_live {
        layout.whole = true;
        layout.range_base = merged.surface_ranges.len() as u32;
        layout.mat_base = merged.materials.len() as u32;
        layout.whole_rows = owner.decoded_n;
        append_source_plan(
            merged,
            owner.decoded_n,
            owner.indices,
            owner.surface_ranges,
            owner.materials,
            draws.drain(..),
        );
        append_packed_source(packed, packed_ok, owner.payload, owner.decoded_n);
        return;
    }
    compact_admitted_source(merged, packed, packed_ok, layout, owner, &used, draws);
}

fn compact_admitted_source(
    merged: &mut XModelDrawPlan,
    packed: &mut Vec<[u8; asset_iw4::size::GFX_PACKED_VERTEX]>,
    packed_ok: &mut bool,
    layout: &mut OwnerLayout,
    owner: &ConcatPackedOwner<'_>,
    used_surfaces: &[bool],
    draws: &mut Vec<XModelSurfaceDraw>,
) {
    let decoded_n = owner.decoded_n;
    let indices = owner.indices;
    let surface_ranges = owner.surface_ranges;
    let materials = owner.materials;
    let source_rows = match owner.payload {
        asset_world::PackedVertexPayload::Iw4(rows) if rows.len() == decoded_n => {
            Some(rows.as_slice())
        }
        _ => {
            if decoded_n != 0 {
                *packed_ok = false;
                packed.clear();
            }
            None
        }
    };
    let mut vert_remap = vec![UNMAPPED; decoded_n];
    let surf_remap = &mut layout.surf_map;
    surf_remap.clear();
    surf_remap.resize(surface_ranges.len(), UNMAPPED);
    let vert_base = merged.decoded_n as u32;
    let mut new_verts = 0u32;
    for (si, &(start, count)) in surface_ranges.iter().enumerate() {
        if !used_surfaces.get(si).copied().unwrap_or(false) {
            continue;
        }
        let start = start as usize;
        let end = start.saturating_add(count as usize);
        let Some(span) = indices.get(start..end) else {
            continue;
        };
        let new_start = merged.indices.len() as u32;
        for &old in span {
            let old = old as usize;
            let Some(slot) = vert_remap.get_mut(old) else {
                continue;
            };
            if *slot == UNMAPPED {
                *slot = vert_base.saturating_add(new_verts);
                if *packed_ok {
                    if let Some(rows) = source_rows {
                        packed.push(rows[old]);
                    }
                }
                layout.rows.push(old as u32);
                layout.need = layout.need.max(old + 1);
                new_verts = new_verts.saturating_add(1);
            }
            merged.indices.push(*slot);
        }
        surf_remap[si] = merged.surface_ranges.len() as u32;
        merged.surface_ranges.push((new_start, count));
    }
    let mat_remap = &mut layout.mat_map;
    mat_remap.clear();
    mat_remap.resize(materials.len(), UNMAPPED);
    let mut used_mat = vec![false; materials.len()];
    for d in draws.iter() {
        if let Some(slot) = used_mat.get_mut(d.material as usize) {
            *slot = true;
        }
    }
    for (mi, live) in used_mat.iter().enumerate() {
        if !*live {
            continue;
        }
        mat_remap[mi] = merged.materials.len() as u32;
        merged.materials.push(materials[mi].clone());
    }
    merged.decoded_n = merged.decoded_n.saturating_add(new_verts as usize);
    merged.concat_layout = false;
    for d in draws.drain(..) {
        merged
            .draws
            .push(compact_draw(d, &layout.surf_map, &layout.mat_map));
    }
}

fn compact_draw(mut d: XModelSurfaceDraw, surf_map: &[u32], mat_map: &[u32]) -> XModelSurfaceDraw {
    if let Some(&mapped) = surf_map.get(d.surface as usize)
        && mapped != UNMAPPED
    {
        d.surface = mapped;
    }
    if let Some(&mapped) = mat_map.get(d.material as usize)
        && mapped != UNMAPPED
    {
        d.material = mapped;
    }
    d
}

fn append_source_plan(
    merged: &mut XModelDrawPlan,
    decoded_n: usize,
    indices: &[u32],
    surface_ranges: &[(u32, u32)],
    materials: &[SmodelPassMaterial],
    draws: impl Iterator<Item = XModelSurfaceDraw>,
) {
    let vert_base = merged.decoded_n as u32;
    let index_base = merged.indices.len() as u32;
    let range_base = merged.surface_ranges.len() as u32;
    let mat_base = merged.materials.len() as u32;

    merged.decoded_n = merged.decoded_n.saturating_add(decoded_n);
    merged.indices.reserve(indices.len());
    for &i in indices {
        merged.indices.push(i.saturating_add(vert_base));
    }
    for &(start, count) in surface_ranges {
        merged
            .surface_ranges
            .push((start.saturating_add(index_base), count));
    }
    merged.materials.extend_from_slice(materials);
    for mut d in draws {
        d.surface = d.surface.saturating_add(range_base);
        d.material = d.material.saturating_add(mat_base);
        merged.draws.push(d);
    }
}

pub(crate) fn apply_resolved_xmodel_lighting(
    resolved: Res<crate::prepare::scene::model_lighting_cache::ResolvedModelLightingTable>,
    mut script: ResMut<ScriptModelDrawPlan>,
    mut item: ResMut<ItemDrawPlan>,
    mut missile: ResMut<MissileDrawPlan>,
    mut dynent: ResMut<DynEntDrawPlan>,
    mut fpv: ResMut<FpvDrawPlan>,
    mut focus: ResMut<crate::assemble::drawsurf::RenderFocus>,
) {
    let script = script.as_mut();
    let script_failed = script.finalize_lighting(&resolved);
    let item = item.as_mut();
    item.finalize_lighting(&resolved);
    let missile = missile.as_mut();
    missile.finalize_lighting(&resolved);
    let dynent = dynent.as_mut();
    dynent.finalize_lighting(&resolved);
    fpv.finalize_lighting(&resolved);
    if let Some(frame) = focus.frame.as_mut()
        && let Some(object_id) = frame.object_id
    {
        if script_failed.contains(&object_id) {
            frame.outcome = "lighting_failed";
            frame.lighting_handle = None;
        } else if let Some(handle) = script
            .draws()
            .iter()
            .find(|draw| draw.object_id == object_id)
            .map(|draw| draw.lighting_handle)
        {
            frame.lighting_handle = Some(handle);
        }
    }
}

pub(crate) fn apply_resolved_fx_model_lighting(
    resolved: Res<crate::prepare::scene::model_lighting_cache::ResolvedModelLightingTable>,
    mut plan: ResMut<FxModelDrawPlan>,
) {
    plan.finalize_lighting(&resolved);
}
