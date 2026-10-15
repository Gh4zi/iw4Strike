//! CS:GO player bodies in third person: with CS:GO mounted and CS:GO's shooting, each living
//! player is drawn as the CS:GO agent of their side's MW2 faction (TF141 the SAS, the Rangers and
//! SEALs SEAL Team 6, and so on, as the radio voices go) instead of the MW2 body, animated by
//! CS:GO's own player animations (`animset_ct.mdl` / `animset_t.mdl`) the way its player
//! animation state layers them each frame:
//!
//! - the aim matrix (`rifle_aim`: full-body poses blended by body yaw and pitch, standing,
//!   walking, running, crouched), with the feet turning to the eyes past the matrix's width or
//!   once moving;
//! - the weapon action (fire, reload, deploy, pin pull) and its re-crouch;
//! - jumping, falling and landing;
//! - the 8-way locomotion (`move`: crouched, walking and running blends by move yaw), played at
//!   the rate its stride covers the ground;
//! - the idle breathing loop and the lean into acceleration.
//!
//! The MW2 body still poses underneath (hit boxes, head, effects) but is not drawn; the body is
//! skinned on the GPU through the CS model pass. The held weapon is CS:GO's own world model
//! (`w_rif_ak47.mdl`), which carries the player skeleton and is bone-merged onto the body as
//! CS:GO does: each of its bones follows the body's bone of the same name.
//!
//! A player who dies stays the CS:GO agent: CS:GO's corpses are ragdolls, ours fall the way the
//! MW2 corpse's death animation does. The agent's main bones follow the MW2 corpse's matching
//! joints, each keeping the turn it had from that joint in the last living pose (so the fall
//! starts from where the agent stood), with the agent's own limb lengths.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use assets::PreparedWeapons;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use entity_iw4::ET_PLAYER_CORPSE;
use csgo_anim::{AnimState, Animset, Observation, Pack, agent_model};
use mdl_source::{BoneFrame, StudioModel};
use net::{CEntity, CEntityRuntime, LocalPresentClient, PresentedSnapshot};
use render_gpu::{
    CsViewmodelDraw, CsViewmodelModel, CsViewmodelShading, CsViewmodelVertex, CsWorldModelInstance,
};

use super::cs_viewmodel::{cs_factions, terrorist_team};
use super::cs_world_model::CsHeldWeaponTags;
use super::remote_body::{RemoteFxBolts, RemotePlayer};

/// Entities whose MW2 body is not drawn this frame: a CS:GO body stands in for it.
#[derive(Resource, Default)]
pub struct CsgoBodyHidden(pub HashSet<u32>);

/// This frame's CS:GO bodies, for the CS model pass.
#[derive(Resource, Default)]
pub struct CsgoBodyDraws(pub Vec<CsWorldModelInstance>);

/// The MW2 joints a CS:GO corpse follows, and the CS:GO bone each one drives.
const RETARGET: [(&str, &str); 20] = [
    ("j_mainroot", "pelvis"),
    ("j_spinelower", "spine_0"),
    ("j_spineupper", "spine_2"),
    ("j_spine4", "spine_3"),
    ("j_neck", "neck_0"),
    ("j_head", "head_0"),
    ("j_clavicle_le", "clavicle_L"),
    ("j_shoulder_le", "arm_upper_L"),
    ("j_elbow_le", "arm_lower_L"),
    ("j_wrist_le", "hand_L"),
    ("j_clavicle_ri", "clavicle_R"),
    ("j_shoulder_ri", "arm_upper_R"),
    ("j_elbow_ri", "arm_lower_R"),
    ("j_wrist_ri", "hand_R"),
    ("j_hip_le", "leg_upper_L"),
    ("j_knee_le", "leg_lower_L"),
    ("j_ankle_le", "ankle_L"),
    ("j_hip_ri", "leg_upper_R"),
    ("j_knee_ri", "leg_lower_R"),
    ("j_ankle_ri", "ankle_R"),
];

/// Where the MW2 bodies' [`RETARGET`] joints were last posed (world space), by entity.
#[derive(Resource, Default)]
pub struct Mw2BodyPoses(pub HashMap<u32, [Option<Mat4>; RETARGET.len()]>);

/// Records MW2 body `entity`'s joints as remote body posing places them: `world` in the body's
/// frame (`world_from_body` places it), its joints found by name with `find`.
pub fn record_mw2_pose(
    poses: &mut Mw2BodyPoses,
    entity: u32,
    find: impl Fn(&str) -> Option<usize>,
    world: &[Mat4],
    world_from_body: Mat4,
) {
    let joints = RETARGET.map(|(joint, _)| {
        find(joint)
            .and_then(|i| world.get(i))
            .map(|m| world_from_body * *m)
    });
    poses.0.insert(entity, joints);
}

/// A living body's last pose, kept to start its corpse from.
struct AliveRecord {
    character: Arc<Character>,
    /// The agent's bones in world space.
    world: Vec<Mat4>,
    /// Its local transforms.
    locals: Vec<BoneFrame>,
    /// The hidden MW2 body's joints at the same moment.
    mw2: [Option<Mat4>; RETARGET.len()],
}

/// The agent standing as `seen` shows (its idle and aim poses), placed there, with the MW2 joints
/// `mw2` of the same moment: a last pose for a body never posed alive.
fn standing_record(
    character: &Arc<Character>,
    animset: &Animset,
    seen: &Observation<'_>,
    mw2: [Option<Mat4>; RETARGET.len()],
    entity: u32,
    locals: &mut Vec<BoneFrame>,
) -> AliveRecord {
    let mut state = AnimState::new(animset, character.ct, entity);
    // A few frames, for the idle and aim layers to come in.
    for _ in 0..4 {
        state.observe(animset, seen, 0.1);
    }
    state.pose(animset, locals);
    let agent_locals =
        csgo_anim::agent::agent_locals(&character.from_animset, &character.studio.rest, locals);
    let world = character.studio.world_from_locals(&agent_locals);
    let world_from_model = csgo_anim::placement(&state, seen.position);
    AliveRecord {
        character: character.clone(),
        world: world.iter().map(|m| world_from_model * mat4(m)).collect(),
        locals: agent_locals,
        mw2,
    }
}

/// A CS:GO corpse: the agent's last living pose and, for each bone a MW2 joint drives, which
/// joint and the turn between them.
struct Corpse {
    character: Arc<Character>,
    world: Vec<Mat4>,
    locals: Vec<Mat4>,
    follow: Vec<Option<(usize, Mat4)>>,
}

impl Corpse {
    fn new(alive: &AliveRecord) -> Self {
        let follow = alive
            .character
            .studio
            .bones
            .iter()
            .enumerate()
            .map(|(b, bone)| {
                let slot = RETARGET
                    .iter()
                    .position(|(_, name)| bone.name.eq_ignore_ascii_case(name))?;
                let joint = alive.mw2[slot]?;
                Some((slot, joint.inverse() * alive.world[b]))
            })
            .collect();
        Self {
            character: alive.character.clone(),
            world: alive.world.clone(),
            locals: alive
                .locals
                .iter()
                .map(|l| mat4(&l.matrix()))
                .collect(),
            follow,
        }
    }

    /// The agent's bones (world space) with the MW2 corpse's joints at `mw2`.
    fn pose(&self, mw2: &[Option<Mat4>; RETARGET.len()]) -> Vec<Mat4> {
        let bones = &self.character.studio.bones;
        let mut world: Vec<Mat4> = Vec::with_capacity(bones.len());
        for (b, bone) in bones.iter().enumerate() {
            // Rigid on its parent as it was, unless a joint turns it.
            let base = match bone.parent.and_then(|p| world.get(p)) {
                Some(parent) => *parent * self.locals[b],
                None => self.world[b],
            };
            let followed = self.follow[b].and_then(|(slot, offset)| Some(mw2[slot]? * offset));
            world.push(match followed {
                // The root goes where the joint takes it; the rest keep the agent's lengths.
                Some(m) if bone.parent.is_none() => m,
                Some(m) => {
                    let (_, rotation, _) = m.to_scale_rotation_translation();
                    Mat4::from_rotation_translation(rotation, base.w_axis.truncate())
                }
                None => base,
            });
        }
        world
    }
}

fn mat4(m: &mdl_goldsrc::Mat3x4) -> Mat4 {
    Mat4::from_cols_array(&[
        m[0][0], m[1][0], m[2][0], 0.0, m[0][1], m[1][1], m[2][1], 0.0, m[0][2], m[1][2], m[2][2],
        0.0, m[0][3], m[1][3], m[2][3], 1.0,
    ])
}

fn mat3x4(m: &Mat4) -> mdl_goldsrc::Mat3x4 {
    let c = m.to_cols_array_2d();
    [
        [c[0][0], c[1][0], c[2][0], c[3][0]],
        [c[0][1], c[1][1], c[2][1], c[3][1]],
        [c[0][2], c[1][2], c[2][2], c[3][2]],
    ]
}

/// Ambient light on a body (as on the CS weapons drawn in hands).
const AMBIENT: [f32; 3] = [0.6; 3];

/// A CS:GO agent model ready to draw.
struct Character {
    gpu: Arc<CsViewmodelModel>,
    /// Its skeleton (geometry dropped once on the GPU).
    studio: StudioModel,
    /// Which animation set drives it (the Counter-Terrorists' or the Terrorists').
    ct: bool,
    /// For each of its bones, the animation set's bone of the same name.
    from_animset: Vec<Option<usize>>,
}

/// A weapon's CS:GO world model, bone-merged onto the body holding it.
struct HeldModel {
    gpu: Arc<CsViewmodelModel>,
    /// The gun without its silencer (the M4A1-S and USP-S).
    unsilenced: Option<Arc<CsViewmodelModel>>,
    studio: StudioModel,
}

enum Slot<T> {
    Loading(Task<T>),
}

pub(super) fn image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    use bevy::asset::RenderAssetUsages;
    use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

/// A model on the GPU (bind-space vertices, skinned by up to three bones), its skeleton kept for
/// posing (the geometry and animations dropped), and for a gun with a silencer part the same
/// gun without it.
fn gpu_model(
    loaded: mdl_source::LoadedModel,
    images: &mut Assets<Image>,
) -> (Arc<CsViewmodelModel>, Option<Arc<CsViewmodelModel>>, StudioModel) {
    let mut studio = loaded.model;
    let textures: Vec<Option<Handle<Image>>> = loaded
        .materials
        .iter()
        .map(|m| {
            m.texture.as_ref().map(|t| {
                let mut rgba = t.rgba.clone();
                // Only alpha-tested materials cut holes; others keep masks in alpha.
                if !m.alpha_test {
                    for px in rgba.chunks_exact_mut(4) {
                        px[3] = 255;
                    }
                }
                images.add(image(t.width, t.height, rgba))
            })
        })
        .collect();
    let weight = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let vertices: Vec<CsViewmodelVertex> = studio
        .vertices
        .iter()
        .map(|v| CsViewmodelVertex {
            position: v.position,
            normal: v.normal,
            uv: v.uv,
            bones: [v.bones[0], v.bones[1], v.bones[2], 0],
            weights: [weight(v.weights[0]), weight(v.weights[1]), weight(v.weights[2]), 0],
            tangent: [0.0; 4],
        })
        .collect();
    let silencer = studio
        .body_parts
        .iter()
        .position(|part| part.name.eq_ignore_ascii_case("silencer"));
    let draws: Vec<(usize, CsViewmodelDraw)> = studio
        .meshes
        .iter()
        // Each body part's first model (the others are alternatives).
        .filter(|mesh| mesh.body_model == 0)
        .filter_map(|mesh| {
            let material = loaded.materials.get(mesh.material)?;
            (!material.additive).then_some(())?;
            Some((
                mesh.body_part,
                CsViewmodelDraw {
                    image: textures.get(mesh.material)?.clone()?,
                    first_vertex: mesh.first_vertex as u32,
                    vertex_count: mesh.vertex_count as u32,
                    maps: None,
                    shading: if material.fullbright {
                        CsViewmodelShading::Fullbright
                    } else {
                        CsViewmodelShading::Lit
                    },
                },
            ))
        })
        .collect();
    studio.vertices = Vec::new();
    studio.meshes = Vec::new();
    studio.sequences = Vec::new();
    studio.animations = Vec::new();
    let unsilenced = silencer.map(|part| {
        Arc::new(CsViewmodelModel {
            id: super::cs_viewmodel::next_model_id().fetch_add(1, Ordering::Relaxed),
            vertices: vertices.clone(),
            draws: draws
                .iter()
                .filter(|(p, _)| *p != part)
                .map(|(_, draw)| draw.clone())
                .collect(),
        })
    });
    let gpu = Arc::new(CsViewmodelModel {
        id: super::cs_viewmodel::next_model_id().fetch_add(1, Ordering::Relaxed),
        vertices,
        draws: draws.into_iter().map(|(_, draw)| draw).collect(),
    });
    (gpu, unsilenced, studio)
}

/// An agent ready to draw, its bones matched to the animation set's.
fn character(
    loaded: mdl_source::LoadedModel,
    ct: bool,
    animset: &Animset,
    images: &mut Assets<Image>,
) -> Character {
    let (gpu, _, studio) = gpu_model(loaded, images);
    let from_animset = csgo_anim::agent::from_animset(&studio, animset);
    Character {
        gpu,
        studio,
        ct,
        from_animset,
    }
}

/// For each bone of `held`, the bone of `body` with its name (`None`: it keeps its own place
/// under its parent).
fn bone_merge(held: &StudioModel, body: &StudioModel) -> Vec<Option<usize>> {
    held.bones
        .iter()
        .map(|bone| {
            body.bones
                .iter()
                .position(|b| b.name.eq_ignore_ascii_case(&bone.name))
        })
        .collect()
}

/// A bone-merged model's skinning palette on a body posed at `body_world` (model space).
fn merged_palette(
    held: &StudioModel,
    merge: &[Option<usize>],
    body_world: &[mdl_goldsrc::Mat3x4],
) -> Vec<mdl_goldsrc::Mat3x4> {
    let mut world: Vec<mdl_goldsrc::Mat3x4> = Vec::with_capacity(held.bones.len());
    for (index, bone) in held.bones.iter().enumerate() {
        let followed = merge
            .get(index)
            .copied()
            .flatten()
            .and_then(|b| body_world.get(b).copied());
        let own = || {
            let local = held.rest.get(index).map_or(mdl_goldsrc::IDENTITY, BoneFrame::matrix);
            match bone.parent.and_then(|p| world.get(p)) {
                Some(parent) => mdl_goldsrc::concat(parent, &local),
                None => local,
            }
        };
        world.push(followed.unwrap_or_else(own));
    }
    world
        .iter()
        .zip(&held.bones)
        .map(|(w, bone)| mdl_goldsrc::concat(w, &bone.pose_to_bone))
        .collect()
}

/// The CS:GO world model a body holding CS weapon `index` shows (`w_rif_ak47`); for the knife,
/// the one the player carries (`knife_skin`, `PlayerState::cs_knife_skin`), else their side's.
fn held_model_name(index: u8, ct: bool, knife_skin: u32) -> Option<String> {
    use weapon_iw4::cs;
    if cs::is_knife(index)
        && let Some(view) = cs::csgo_knife_view_model_of_skin(knife_skin)
    {
        return Some(format!("w_{}", view.strip_prefix("v_")?));
    }
    let name = if cs::is_knife(index) {
        "knife"
    } else if let Some(grenade) = cs::cs_grenade(index) {
        grenade.name
    } else if cs::is_c4(index) {
        "c4"
    } else {
        cs::cs_weapon(index)?.name
    };
    let view = cs::csgo_view_model(name)?[usize::from(ct)];
    Some(format!("w_{}", view.strip_prefix("v_")?))
}

/// The CS:GO bodies: the pack, the agents read so far, each player's animation state.
#[derive(Resource, Default)]
pub struct CsgoBodies {
    characters: HashMap<&'static str, Option<Slot<Result<mdl_source::LoadedModel, String>>>>,
    ready: HashMap<&'static str, Arc<Character>>,
    held: HashMap<String, Option<Slot<Result<mdl_source::LoadedModel, String>>>>,
    held_ready: HashMap<String, Arc<HeldModel>>,
    /// Bone merges by (agent, weapon) model id.
    merges: HashMap<(u64, u64), Arc<Vec<Option<usize>>>>,
    /// Each living player's last pose (by client), and the corpses (by entity).
    alive: HashMap<sim::ClientId, AliveRecord>,
    corpses: HashMap<u32, Corpse>,
    states: HashMap<u32, AnimState>,
    locals: Vec<BoneFrame>,
}

/// Hides the local player's own living MW2 body (seen in third person) this frame.
fn hide_own_body(
    roots: &Query<(&CEntity, &CEntityRuntime, &RemotePlayer, &mut RemoteFxBolts)>,
    local: sim::ClientId,
    hidden: &mut CsgoBodyHidden,
) {
    for (identity, runtime, ..) in roots {
        let corpse = runtime.next_state.e_type == ET_PLAYER_CORPSE
            || runtime.pose_e_type == ET_PLAYER_CORPSE as u8;
        if !corpse && identity.client() == Some(local) {
            hidden.0.insert(u32::from(identity.number()));
        }
    }
}

impl CsgoBodies {
    /// Agent `name` is still being read.
    fn loading(&self, name: &str) -> bool {
        matches!(self.characters.get(name), Some(Some(Slot::Loading(_))))
    }

    /// Agent `name`, read off the main thread on first use.
    fn character(
        &mut self,
        pack: &Arc<Pack>,
        name: &'static str,
        images: &mut Assets<Image>,
    ) -> Option<Arc<Character>> {
        if let Some(ready) = self.ready.get(name) {
            return Some(ready.clone());
        }
        let ct = name.starts_with("ctm_");
        let animset = pack.animsets[usize::from(!ct)].clone()?;
        let slot = self.characters.entry(name).or_insert_with(|| {
            let vpk = pack.vpk.clone();
            Some(Slot::Loading(AsyncComputeTaskPool::get().spawn(async move {
                mdl_source::load_model_with(
                    &vpk,
                    &format!("models/player/custom_player/legacy/{name}.mdl"),
                    false,
                )
            })))
        });
        let Some(Slot::Loading(task)) = slot else {
            return None;
        };
        if !task.is_finished() {
            return None;
        }
        let Some(Slot::Loading(task)) = slot.take() else {
            return None;
        };
        match block_on(task) {
            Ok(loaded) => {
                let character = Arc::new(character(loaded, ct, &animset, images));
                diag::info!(
                    World,
                    "csgo bodies: {name}: {} triangles, {} bones ({} driven)",
                    character.gpu.vertices.len() / 3,
                    character.studio.bones.len(),
                    character.from_animset.iter().flatten().count()
                );
                self.ready.insert(name, character.clone());
                Some(character)
            }
            Err(error) => {
                diag::warn!(World, "csgo bodies: {name}: {error}; the MW2 body stays");
                None
            }
        }
    }
}

/// `IW4L_CSGO_BODY_DEBUG`: each body's animation state logged once a second.
fn debug_log() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("IW4L_CSGO_BODY_DEBUG").is_some())
}

impl CsgoBodies {
    /// Weapon world model `name`, read off the main thread on first use.
    fn held_model(
        &mut self,
        pack: &Arc<Pack>,
        name: &str,
        images: &mut Assets<Image>,
    ) -> Option<Arc<HeldModel>> {
        if let Some(ready) = self.held_ready.get(name) {
            return Some(ready.clone());
        }
        let slot = self.held.entry(name.to_owned()).or_insert_with(|| {
            let vpk = pack.vpk.clone();
            let path = format!("models/weapons/{name}.mdl");
            Some(Slot::Loading(AsyncComputeTaskPool::get().spawn(async move {
                mdl_source::load_model_with(&vpk, &path, false)
            })))
        });
        let Some(Slot::Loading(task)) = slot else {
            return None;
        };
        if !task.is_finished() {
            return None;
        }
        let Some(Slot::Loading(task)) = slot.take() else {
            return None;
        };
        match block_on(task) {
            Ok(loaded) => {
                let (gpu, unsilenced, studio) = gpu_model(loaded, images);
                diag::info!(
                    World,
                    "csgo bodies: {name}: {} triangles, {} bones",
                    gpu.vertices.len() / 3,
                    studio.bones.len()
                );
                let held = Arc::new(HeldModel {
                    gpu,
                    unsilenced,
                    studio,
                });
                self.held_ready.insert(name.to_owned(), held.clone());
                Some(held)
            }
            Err(error) => {
                diag::warn!(World, "csgo bodies: {name}: {error}");
                None
            }
        }
    }

    /// `held` bone-merged onto `character`.
    fn merge(&mut self, character: &Character, held: &HeldModel) -> Arc<Vec<Option<usize>>> {
        self.merges
            .entry((character.gpu.id, held.gpu.id))
            .or_insert_with(|| Arc::new(bone_merge(&held.studio, &character.studio)))
            .clone()
    }
}

/// Poses and hands over every living player's CS:GO body this frame, hiding their MW2 body and
/// moving their held weapon onto the CS:GO hand.
#[allow(clippy::too_many_arguments)]
pub fn update_csgo_bodies(
    time: Res<Time>,
    settings: Res<frame::GameSettings>,
    local: Res<LocalPresentClient>,
    presented: Res<PresentedSnapshot>,
    weapons: Option<Res<PreparedWeapons>>,
    mut bodies: ResMut<CsgoBodies>,
    mut hidden: ResMut<CsgoBodyHidden>,
    mut draws: ResMut<CsgoBodyDraws>,
    mut held: ResMut<CsHeldWeaponTags>,
    mut mw2_poses: ResMut<Mw2BodyPoses>,
    mut images: ResMut<Assets<Image>>,
    mut roots: Query<(&CEntity, &CEntityRuntime, &RemotePlayer, &mut RemoteFxBolts)>,
) {
    let bodies = &mut *bodies;
    hidden.0.clear();
    draws.0.clear();
    let wanted = movement_iw4::rules::CS_RULES
        && settings.player_models == "csgo"
        && presented
            .player(local.0)
            .is_some_and(|ps| ps.cs_shooting_mode == weapon_iw4::csgo::SHOOTING_CSGO);
    let (Some(snapshot), Some(weapons)) = (presented.snapshot(), weapons) else {
        return;
    };
    if !wanted {
        bodies.states.clear();
        return;
    }
    let Some(pack) = csgo_anim::pack() else {
        // Your own body waits for its agent hidden rather than showing as MW2's for a moment
        // (the others stay MW2's until theirs is read: they must be seen).
        hide_own_body(&roots, local.0, &mut hidden);
        return;
    };
    let factions = cs_factions(snapshot);
    let factions_known = factions.is_some();
    let (allies, axis) = factions.unwrap_or_default();
    // Both sides' agents are read as soon as the match's factions are known, not when a body
    // first needs one: until then that body (your own in third person, or the first player of a
    // side you saw) was MW2's.
    if factions_known {
        for terrorist in [false, true] {
            let name = agent_model(weapon_iw4::cs::csgo_side_voice(&allies, &axis, terrorist));
            let _ = bodies.character(&pack, name, &mut images);
        }
    }
    let dt = time.delta_secs().min(0.1);
    let mut live = HashSet::new();
    for (identity, runtime, remote, mut bolts) in &mut roots {
        let is_corpse = runtime.next_state.e_type == ET_PLAYER_CORPSE
            || runtime.pose_e_type == ET_PLAYER_CORPSE as u8;
        let Some(client) = identity.client() else {
            continue;
        };
        if is_corpse {
            // The victim's agent falls with the MW2 corpse.
            let entity = u32::from(identity.number());
            let Some(mw2) = mw2_poses.0.get(&entity) else {
                continue;
            };
            if !bodies.corpses.contains_key(&entity) {
                // Your own body isn't posed while you see through its eyes, so it has no last
                // pose: its corpse starts from the agent standing where it fell, against the MW2
                // corpse's first frame (MW2's death animations start standing too).
                if client == local.0
                    && !bodies.alive.contains_key(&client)
                    && let Some(ps) = presented.player(client)
                {
                    let terrorist = terrorist_team(snapshot, remote.client_state_team);
                    let name =
                        agent_model(weapon_iw4::cs::csgo_side_voice(&allies, &axis, terrorist));
                    if let Some(character) = bodies.character(&pack, name, &mut images)
                        && let Some(animset) = pack.animsets[usize::from(!character.ct)].clone()
                    {
                        let weapon_name = weapons.0.script_name_of(ps.weapon);
                        let record = standing_record(
                            &character,
                            &animset,
                            &Observation {
                                ps,
                                position: Vec3::from(runtime.origin),
                                eye_yaw: runtime.angles[1],
                                eye_pitch: 0.0,
                                weapon_name: &weapon_name,
                            },
                            *mw2,
                            entity,
                            &mut bodies.locals,
                        );
                        bodies.alive.insert(client, record);
                    }
                }
                let Some(alive) = bodies.alive.get(&client) else {
                    continue;
                };
                bodies.corpses.insert(entity, Corpse::new(alive));
            }
            let corpse = &bodies.corpses[&entity];
            let world = corpse.pose(mw2);
            // Placed at its first bone (the pelvis), where the map's light is read.
            let at = world.first().map_or(Vec3::ZERO, |m| m.w_axis.truncate());
            let model_from_world = Mat4::from_translation(-at);
            let skin: Vec<mdl_goldsrc::Mat3x4> = world
                .iter()
                .zip(&corpse.character.studio.bones)
                .map(|(w, bone)| mdl_goldsrc::concat(&mat3x4(&(model_from_world * *w)), &bone.pose_to_bone))
                .collect();
            draws.0.push(CsWorldModelInstance {
                model: corpse.character.gpu.clone(),
                world_from_model: Mat4::from_translation(at).to_cols_array_2d(),
                ambient: AMBIENT,
                sun_dir: [0.0; 3],
                sun: [0.0; 3],
                bones: Some(Arc::new(skin)),
                lighting_origin: None,
                overlay: false,
            });
            hidden.0.insert(entity);
            live.insert(entity);
            continue;
        }
        let Some(ps) = presented.player(client) else {
            continue;
        };
        // The agent of the side they play (as the server's hit boxes take it), not of their MW2
        // team: some maps attack with the allies, and the teams swap sides at halftime.
        let terrorist = terrorist_team(snapshot, remote.client_state_team);
        let name = agent_model(weapon_iw4::cs::csgo_side_voice(&allies, &axis, terrorist));
        let Some(character) = bodies.character(&pack, name, &mut images) else {
            if client == local.0 && bodies.loading(name) {
                hidden.0.insert(u32::from(identity.number()));
            }
            continue;
        };
        let Some(animset) = pack.animsets[usize::from(!character.ct)].clone() else {
            continue;
        };
        let entity = u32::from(identity.number());
        live.insert(entity);

        let weapon = ps.weapon;
        let weapon_name = weapons.0.script_name_of(weapon);
        let cs_index = weapon_iw4::cs::cs_weapon_index_for(&weapon_name);
        let class = cs_index.map_or("knife", weapon_iw4::cs::csgo_anim_class);
        let state = bodies
            .states
            .entry(entity)
            .or_insert_with(|| AnimState::new(&animset, character.ct, entity));
        if state.ct != character.ct {
            *state = AnimState::new(&animset, character.ct, entity);
        }
        state.observe(
            &animset,
            &Observation {
                ps,
                position: Vec3::from(runtime.origin),
                eye_yaw: runtime.angles[1],
                eye_pitch: runtime.angles[0],
                weapon_name: &weapon_name,
            },
            dt,
        );
        if debug_log() && (state.time / 1.0).floor() != ((state.time - dt) / 1.0).floor() {
            let layers: Vec<String> = state
                .layers
                .iter()
                .filter_map(|l| {
                    let seq = animset.model.sequences.get(l.sequence?)?;
                    Some(format!("{}:{:.2}@{:.2}", seq.label, l.weight, l.cycle))
                })
                .collect();
            diag::info!(
                World,
                "csgo body {entity}: eye {:.0}/{:.0} foot {:.0} ground {} duck {:.2} speed {:.0} vel_z {:.0} weapon {weapon} {class} layers [{}]",
                state.eye_yaw,
                state.eye_pitch,
                state.foot_yaw,
                state.on_ground,
                state.duck,
                state.speed,
                state.velocity_z,
                layers.join(" ")
            );
        }

        // The animation set's pose, carried over to the agent's own skeleton.
        let locals = &mut bodies.locals;
        let state = &bodies.states[&entity];
        state.pose(&animset, locals);
        let agent_locals =
            csgo_anim::agent::agent_locals(&character.from_animset, &character.studio.rest, locals);
        let world = character.studio.world_from_locals(&agent_locals);
        let skin: Vec<mdl_goldsrc::Mat3x4> = world
            .iter()
            .zip(&character.studio.bones)
            .map(|(w, bone)| mdl_goldsrc::concat(w, &bone.pose_to_bone))
            .collect();
        let world_from_model = csgo_anim::placement(state, Vec3::from(runtime.origin));
        // Lit where its pelvis is (its first bone), as a corpse is: the origin is at the feet,
        // where the light grid and the sun ray start at (or in) the ground.
        let lighting_origin = world
            .first()
            .map_or(Vec3::from(runtime.origin) + Vec3::Z * 36.0, |m| {
                (world_from_model * mat4(m)).w_axis.truncate()
            })
            .to_array();
        draws.0.push(CsWorldModelInstance {
            model: character.gpu.clone(),
            world_from_model: world_from_model.to_cols_array_2d(),
            ambient: AMBIENT,
            sun_dir: [0.0; 3],
            sun: [0.0; 3],
            bones: Some(Arc::new(skin)),
            lighting_origin: Some(lighting_origin),
            overlay: false,
        });
        hidden.0.insert(entity);
        // Kept to start a corpse from.
        if let Some(mw2) = mw2_poses.0.get(&entity) {
            bodies.alive.insert(
                client,
                AliveRecord {
                    character: character.clone(),
                    world: world.iter().map(|m| world_from_model * mat4(m)).collect(),
                    locals: agent_locals.clone(),
                    mw2: *mw2,
                },
            );
        }
        // The weapon: CS:GO's world model on the body's bones, not the MW2 body's CS:S one.
        held.0.retain(|tag| tag.entity != entity);
        let held_model = cs_index
            .and_then(|index| {
                held_model_name(index, character.ct, ps.cs_knife_skin)
                    .and_then(|name| bodies.held_model(&pack, &name, &mut images))
                    // A knife CS:GO lacks a world model for: the side's own.
                    .or_else(|| {
                        held_model_name(index, character.ct, 0)
                            .and_then(|name| bodies.held_model(&pack, &name, &mut images))
                    })
            });
        if let Some(model) = held_model {
            let merge = bodies.merge(&character, &model);
            let palette = merged_palette(&model.studio, &merge, &world);
            // Muzzle flashes and brass come out of the CS:GO gun, not the hidden MW2 one.
            let attachment = |name: &str| {
                let a = model
                    .studio
                    .attachments
                    .iter()
                    .find(|a| a.name.eq_ignore_ascii_case(name))?;
                let at = mdl_goldsrc::concat(palette.get(a.bone)?, &a.in_bind);
                Some(world_from_model * mat4(&at))
            };
            let orientation = |m: Mat4| fx::FxBoltOrientation {
                origin: m.w_axis.truncate().to_array(),
                axis: [
                    m.x_axis.truncate().to_array(),
                    m.y_axis.truncate().to_array(),
                    m.z_axis.truncate().to_array(),
                ],
            };
            if let (Some(flash), Some(m)) = (bolts.flash.as_mut(), attachment("muzzle_flash")) {
                flash.orientation = orientation(m);
            }
            if let (Some(brass), Some(m)) = (bolts.brass.as_mut(), attachment("shell_eject")) {
                brass.orientation = orientation(m);
            }
            let silencer_off = cs_index.is_some_and(|index| {
                !weapon_iw4::cs::silencer_on(
                    ps.cs_silencers,
                    weapon_iw4::cs::silencer_bit(index),
                    ps.cs_shooting_mode,
                )
            });
            let gun = model
                .unsilenced
                .as_ref()
                .filter(|_| silencer_off)
                .unwrap_or(&model.gpu);
            draws.0.push(CsWorldModelInstance {
                model: gun.clone(),
                world_from_model: world_from_model.to_cols_array_2d(),
                ambient: AMBIENT,
                sun_dir: [0.0; 3],
                sun: [0.0; 3],
                bones: Some(Arc::new(palette)),
                lighting_origin: Some(lighting_origin),
                overlay: false,
            });
        }
    }
    bodies.states.retain(|entity, _| live.contains(entity));
    bodies.corpses.retain(|entity, _| live.contains(entity));
    let present: HashSet<u32> = roots
        .iter()
        .map(|(identity, ..)| u32::from(identity.number()))
        .collect();
    mw2_poses.0.retain(|entity, _| present.contains(entity));
}
