use bevy::prelude::Resource;

use crate::asset_graph::DestructibleDeathRow;
use asset_anim::XAnimCatalog;
use asset_game::WeaponRegistry;
use asset_model::{BodyMeshCatalog, FpvMeshCatalog, WorldWeaponCatalog};

/// The one material population a prepared match owns, and the map-zone-local
/// index space that resolves into it.
///
/// Geometry keeps references — local material indices and, after the merge,
/// nothing else. The pool itself is never nested inside an optional product.
#[derive(Clone, Default)]
pub struct MatchMaterials {
    pub population: std::sync::Arc<asset_material::MaterialDefinitions>,

    pub common_profile_id: u64,

    pub products_id: u64,

    /// map-zone-local material index -> row in `population`
    pub map_ids: Vec<Option<usize>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreparedGaps {
    pub lines: Vec<String>,
}

/// What the map itself declares about the match, captured once by the lane and
/// moved from there to its single owner in [`PreparedMap`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapFacts {
    pub minimap_corners: Option<asset_world::MinimapCorners>,

    pub north_yaw: Option<f32>,

    pub airstrike_height: Option<f32>,

    pub compass: asset_world::MapCompassDeclaration,

    pub script_sound: asset_audio::MapScriptSoundFacts,

    pub team_settings: asset_game::MapTeamSettings,

    pub t5_teamset: Option<String>,

    pub objective_visuals: asset_game::ObjectiveVisuals,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreparedMap {
    pub zone: String,

    pub namespace: Option<asset_core::AssetNamespace>,
    pub spawns: Vec<asset_world::SpawnPoint>,

    pub facts: MapFacts,
    pub gaps: PreparedGaps,
}

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedKillstreaks(pub Vec<String>);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedWeapons(pub std::sync::Arc<WeaponRegistry>);

#[derive(Clone, Debug, Default, Resource)]
pub struct MatchType10SoundHints(pub Vec<String>);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedFpvMeshes(pub FpvMeshCatalog);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedBodies(pub std::sync::Arc<BodyMeshCatalog>);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedWorldWeapons(pub WorldWeaponCatalog);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedProjectileMeshes(pub asset_model::ProjectileMeshCatalog);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedXModelWalkCensus {
    pub walked_n: usize,

    pub unclassified_n: usize,

    pub projectile_names: Vec<String>,
}

impl PreparedXModelWalkCensus {
    pub fn projectile_walked_n(&self) -> usize {
        self.projectile_names.len()
    }

    pub fn projectile_names_csv(&self) -> String {
        self.projectile_names.join(",")
    }

    pub fn report_line(&self, source: &str) -> String {
        format!(
            "{source} XModels: {} unique names, {} unclassified (model_kind None); projectile_* walked: {} ({})",
            self.walked_n,
            self.unclassified_n,
            self.projectile_walked_n(),
            if self.projectile_names.is_empty() {
                "none".to_owned()
            } else {
                self.projectile_names_csv()
            }
        )
    }
}

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedXAnims(pub XAnimCatalog);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedDestructibleDeath(pub Vec<DestructibleDeathRow>);

#[derive(Clone, Debug, Default, Resource)]
pub struct PreparedLocalizedStrings(pub asset_game::LocalizeCatalog);

#[derive(Clone, Debug, Default, Resource)]
pub struct SessionCompass {
    pub corners: Option<asset_world::MinimapCorners>,

    pub north_yaw: Option<f32>,

    pub declaration: asset_world::MapCompassDeclaration,
}

#[derive(Clone, Copy, Debug, Default, Resource, PartialEq, Eq)]
pub struct PreparedBodyClips(pub bool);
