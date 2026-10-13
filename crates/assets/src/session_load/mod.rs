use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::tasks::{TaskPool, TaskPoolBuilder};

use crate::{
    PreparedGaps, PreparedMap,
    lane::{LoadedWorld, lane},
    lane_capability::PreparedCapability,
};
use asset_anim::{XAnimBuild, XAnimCatalog};
use asset_game::{
    FxCatalog, LocalizeCatalog, MP_LOCALIZED_ZONES, WeaponBuild, WeaponRegistry,
    load_localize_catalog_in_lane,
};
use asset_material::{ImageDemandPlan, MaterialCatalog};
use asset_model::{
    BodyMeshCatalog, FpvMeshBuild, FpvMeshCatalog, ProjectileMeshBuild, WorldWeaponBuild,
    WorldWeaponCatalog,
};
use asset_transport::load_jobs::{self, JobKind};
use asset_transport::{
    LoadProgress, StageHandle, StageId, find_common_mp_for_envelope, find_common_mp_for_zone,
    find_runtime_common_mp, find_runtime_zone, find_zone_file_version, find_zone_for_tree,
    games_root_from_env, open_zone_shared, peek_zone_version,
};
use asset_world::{ClipCollision, IntermissionView, WorldDraw};

pub use asset_world::WorldDrawPolicy;

mod common_cache;
mod common_walks;
mod match_walk;
mod resident_map;

use common_cache::*;
use common_walks::*;
use match_walk::*;

pub use common_cache::{CommonKey, CommonSet, ShellCommon, load_shell_common};

const KEEP_FOR_NEXT_LOAD_ENV: &str = "IW4L_KEEP_LOADED_ASSETS";
const KEEP_FOR_NEXT_LOAD_MIN_RAM_GIB: u64 = 24;

/// Whether a load keeps what it decoded for the next one: the resident copy of the prepared
/// match (a same-map load then opens no zone and decodes nothing) and the common set's
/// common_mp first-person image payloads (the next map skips ~0.8 s of parallel decode).
///
/// Both stay resident through every match: the resident copy holds every decoded image of the
/// map (~0.8 GiB on Favela), the payloads ~0.4 GiB more. On a machine without much more RAM
/// than a match uses, that is what pushed the game into page faults mid-match, so they are kept
/// only with at least `KEEP_FOR_NEXT_LOAD_MIN_RAM_GIB` installed, or when it cannot be read.
/// `IW4L_KEEP_LOADED_ASSETS=0|1` decides it either way.
fn keep_for_next_load() -> bool {
    static KEEP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *KEEP.get_or_init(|| match std::env::var(KEEP_FOR_NEXT_LOAD_ENV).as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        _ => diag::total_physical_memory_bytes()
            .is_none_or(|bytes| bytes >= KEEP_FOR_NEXT_LOAD_MIN_RAM_GIB << 30),
    })
}
pub use common_walks::{
    MatchMaterialSeed, apply_match_material_map, load_match_material_catalog,
    load_match_material_seed,
};
pub use resident_map::load_prepared_match;

static PROCESS_CPUS: std::sync::OnceLock<Vec<usize>> = std::sync::OnceLock::new();

/// Under Counter-Strike rules, whether an MW2 weapon (named without its `_mp`, as the weapon
/// registry names them) is one a CS gun wears or the bomb, so its models, sounds and animations
/// stay loaded.
pub(crate) fn cs_keeps_weapon(name: &str) -> bool {
    let mp = format!("{name}_mp");
    weapon_iw4::cs::cs_weapon_index_for(&mp).is_some()
        || matches!(mp.as_str(), "briefcase_bomb_defuse_mp" | "defaultweapon_mp")
}

pub fn publish_process_cpus(cpus: Vec<usize>) {
    let _ = PROCESS_CPUS.set(cpus);
}

pub fn load_workers() -> usize {
    PROCESS_CPUS
        .get()
        .map(Vec::len)
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .map_or(1, |n| n.saturating_sub(2).max(1))
}

pub fn load_pool() -> &'static TaskPool {
    static POOL: std::sync::OnceLock<TaskPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        TaskPoolBuilder::new()
            .num_threads(load_workers())
            .thread_name("iw4l load".to_owned())
            .on_thread_spawn(|| {
                if let Some(cpus) = PROCESS_CPUS.get() {
                    set_thread_cpus(cpus);
                }
            })
            .build()
    })
}

#[cfg(target_os = "linux")]
fn set_thread_cpus(cpus: &[usize]) {
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for cpu in cpus {
            if *cpu >= libc::CPU_SETSIZE as usize {
                return;
            }
            libc::CPU_SET(*cpu, &mut set);
        }
        libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &raw const set);
    }
}

#[cfg(not(target_os = "linux"))]
fn set_thread_cpus(_cpus: &[usize]) {}

#[derive(Default, Clone)]
pub struct PreparedWorld {
    pub draw: Option<WorldDraw>,
    pub dynamic_light: Option<asset_world::ResolvedLightDef>,
    pub static_model_meshes: Vec<asset_world::ModelMesh>,

    pub static_model_instances: Vec<Option<asset_world::StaticModelPlacement>>,

    pub map_xmodel_scene_assets: asset_world::MapXModelSceneCatalog,

    pub script_model_instances: Vec<asset_world::ScriptModelSceneInstance>,

    pub script_brush_models: Vec<asset_world::ScriptBrushModelPlacement>,

    pub flag_descriptors: Vec<asset_world::FlagDescriptor>,

    pub script_structs: Vec<asset_world::MapScriptStruct>,

    pub dyn_ents: asset_world::DynEntCatalog,

    pub smodel_lighting_samples: Vec<asset_model::SmodelLightingSample>,

    pub light_grid: Option<asset_model::OwnedLightGrid>,

    pub fx: asset_game::FxCatalog,

    pub fx_models: asset_game::FxModelCatalog,

    pub fx_glass: Option<asset_world::FxGlassReset>,

    pub impact_fx: Option<asset_game::OwnedFxImpactTable>,
    pub reflection_probe_images: Vec<Option<bevy::prelude::Image>>,
    pub intermission_view: Option<IntermissionView>,

    pub exp_fog: Option<asset_world::ExpFog>,

    pub film_vision: Option<asset_world::FilmVision>,
    pub film_visions: std::collections::BTreeMap<
        String,
        Result<asset_world::FilmVision, asset_world::FilmVisionParseError>,
    >,

    pub createart_name: Option<String>,
    pub min: [f32; 3],
    pub max: [f32; 3],

    pub world_bounds: Option<[f32; 6]>,
    pub policy: WorldDrawPolicy,
}

#[derive(Default, Clone)]
pub struct PreparedMatch {
    pub scripts: crate::ScriptSources,
    pub world: PreparedWorld,

    pub fx: asset_game::FxDefinitions,
    pub materials: crate::MatchMaterials,
    pub clip: Option<Arc<ClipCollision>>,
    pub weapons: Arc<WeaponRegistry>,
    pub fpv_meshes: FpvMeshCatalog,
    pub bodies: Arc<BodyMeshCatalog>,
    pub world_weapons: WorldWeaponCatalog,

    pub projectile_meshes: asset_model::ProjectileMeshCatalog,
    pub xanims: XAnimCatalog,
    pub destructible_death: Vec<crate::DestructibleDeathRow>,
    pub player_anim_sources: asset_anim::PlayerAnimSources,

    pub tracers: asset_game::TracerDefinitions,

    pub strings: LocalizeCatalog,
    pub report: Vec<String>,

    pub prepared_map: PreparedMap,

    pub pen_table: weapon_iw4::PenetrationDepthTable,
    pub pen_table_loaded: bool,
    pub lochit_table: Option<[f32; weapon_iw4::HITLOC_COUNT]>,

    pub xmodel_walk: crate::PreparedXModelWalkCensus,

    pub sound: Option<Result<asset_audio::SoundCatalog, String>>,
    pub sound_gaps: usize,
    pub script_sound_aliases: Option<std::collections::BTreeMap<String, Option<bool>>>,
}

pub enum MatchLoadOutcome {
    Ready(PreparedMatch),
    Canceled,
}
