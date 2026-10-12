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
use playerstate_iw4::{ENTITYNUM_NONE, cs_grenade};
use mdl_source::{BoneFrame, StudioModel};
use net::{CEntity, CEntityRuntime, LocalPresentClient, PresentedSnapshot};
use render_gpu::{
    CsViewmodelDraw, CsViewmodelModel, CsViewmodelShading, CsViewmodelVertex, CsWorldModelInstance,
};

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

// CS:GO's movement numbers the animations follow (`cs_shareddefs.cpp`).
const SPEED_RUN: f32 = 260.0;
const SPEED_WALK_MODIFIER: f32 = 0.52;
const SPEED_DUCK_MODIFIER: f32 = 0.34;

// CS:GO's player animation state (`csgo_playeranimstate.cpp`).
const MAX_VEL_LIMIT: f32 = 1.2;
const DUCK_APPROACH_SPEED: f32 = 6.0;
const AIM_YAW_MAX: f32 = 58.0;
const AIM_YAW_MIN: f32 = -58.0;
const SPEED_TO_CHANGE_AIM_MATRIX: f32 = 0.8;
const SPEED_TO_CHANGE_AIM_MATRIX_SCOPED: f32 = 4.2;
const LOWER_CATCHUP_IDLE: f32 = 100.0;
const AIM_NARROW_WALK: f32 = 0.8;
const AIM_NARROW_RUN: f32 = 0.5;
const AIM_NARROW_CROUCHMOVING: f32 = 0.5;
const LOWER_REALIGN_DELAY: f32 = 1.1;
const WALK_TO_RUN_TRANSITION_SPEED: f32 = 2.0;
const ONGROUND_FUZZY_APPROACH: f32 = 8.0;
const ONGROUND_FUZZY_APPROACH_CROUCH: f32 = 16.0;
const RUN_ANIM_PLAYBACK_MULTIPLIER: f32 = 0.85;

/// The animation layers, in the order they play over each other (`animstate_layer_t`).
const AIMMATRIX: usize = 0;
const WEAPON_ACTION: usize = 1;
const WEAPON_ACTION_RECROUCH: usize = 2;
const JUMP_OR_FALL: usize = 4;
const LAND_OR_CLIMB: usize = 5;
const MOVE: usize = 6;
const ALIVELOOP: usize = 11;
const LEAN: usize = 12;
const LAYER_COUNT: usize = 13;

/// Ambient light on a body (as on the CS weapons drawn in hands).
const AMBIENT: [f32; 3] = [0.6; 3];

/// The CS:GO agent a faction is drawn as, by its radio voice (`cs::csgo_radio_voice`).
fn agent_model(voice: &str) -> &'static str {
    match voice {
        "sas" => "ctm_sas",
        "seal" => "ctm_st6",
        "balkan" => "tm_balkan_varianta",
        "leet" => "tm_leet_varianta",
        "separatist" => "tm_separatist",
        _ => "tm_phoenix",
    }
}

fn approach(target: f32, value: f32, speed: f32) -> f32 {
    let delta = target - value;
    if delta > speed {
        value + speed
    } else if delta < -speed {
        value - speed
    } else {
        target
    }
}

fn approach_vec(target: Vec3, value: Vec3, speed: f32) -> Vec3 {
    let delta = target - value;
    let len = delta.length();
    if len > speed { value + delta / len * speed } else { target }
}

/// An angle in (-180, 180].
fn angle_normalize(angle: f32) -> f32 {
    let a = angle.rem_euclid(360.0);
    if a > 180.0 { a - 360.0 } else { a }
}

/// How far `dest` is from `src`, the short way round.
fn angle_diff(dest: f32, src: f32) -> f32 {
    angle_normalize(dest - src)
}

fn approach_angle(target: f32, value: f32, speed: f32) -> f32 {
    let delta = angle_diff(target, value);
    let speed = speed.abs();
    if delta > speed {
        value + speed
    } else if delta < -speed {
        value - speed
    } else {
        target
    }
}

/// Schlick's bias: `x` bent toward 0 or 1 (`amount` 0.5 leaves it).
fn bias(x: f32, amount: f32) -> f32 {
    x.max(0.0).powf(amount.ln() / 0.5f32.ln())
}

fn smoothstep_bounds(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + (b - a) * t
}

fn remap_clamped(value: f32, a: f32, b: f32, c: f32, d: f32) -> f32 {
    let t = ((value - a) / (b - a)).clamp(0.0, 1.0);
    c + (d - c) * t
}

/// A cycle wrapped into 0..1.
fn clamp_cycle(cycle: f32) -> f32 {
    cycle - cycle.floor()
}

/// A pose parameter of the animation set, by the role CS:GO gives it.
#[derive(Clone, Copy, Default)]
struct Params {
    move_yaw: Option<usize>,
    body_yaw: Option<usize>,
    body_pitch: Option<usize>,
    speed: Option<usize>,
    stand: Option<usize>,
    jump_fall: Option<usize>,
    lean_yaw: Option<usize>,
    aim_stand_idle: Option<usize>,
    aim_stand_walk: Option<usize>,
    aim_stand_run: Option<usize>,
    aim_crouch_idle: Option<usize>,
    aim_crouch_walk: Option<usize>,
    move_blend_walk: Option<usize>,
    move_blend_run: Option<usize>,
    move_blend_crouch: Option<usize>,
}

/// A CS:GO animation set and the sequences the animation state looks up.
struct Animset {
    model: StudioModel,
    params: Params,
    lean: Option<usize>,
    recrouch: Option<usize>,
}

impl Animset {
    fn new(model: StudioModel) -> Self {
        let p = |name: &str| model.pose_param(name);
        let params = Params {
            move_yaw: p("move_yaw"),
            body_yaw: p("body_yaw"),
            body_pitch: p("body_pitch"),
            speed: p("speed"),
            stand: p("stand"),
            jump_fall: p("jump_fall"),
            lean_yaw: p("lean_yaw"),
            aim_stand_idle: p("aim_blend_stand_idle"),
            aim_stand_walk: p("aim_blend_stand_walk"),
            aim_stand_run: p("aim_blend_stand_run"),
            aim_crouch_idle: p("aim_blend_crouch_idle"),
            aim_crouch_walk: p("aim_blend_crouch_walk"),
            move_blend_walk: p("move_blend_walk"),
            move_blend_run: p("move_blend_run"),
            move_blend_crouch: p("move_blend_crouch"),
        };
        Self {
            lean: model.sequence_by_label("lean"),
            recrouch: model.sequence_by_label("recrouch_generic"),
            params,
            model,
        }
    }

    /// Playback rate of `sequence` in cycles per second (`Studio_CPS`).
    fn cycle_rate(&self, sequence: usize) -> f32 {
        let duration = self
            .model
            .sequences
            .get(sequence)
            .map_or(0.0, mdl_source::studio::Sequence::duration);
        if duration > 0.0 { 1.0 / duration } else { 10.0 }
    }

    fn activity(&self, sequence: Option<usize>) -> &str {
        sequence
            .and_then(|s| self.model.sequences.get(s))
            .map_or("", |s| s.activity.as_str())
    }

    fn has_act_mod(&self, sequence: Option<usize>, name: &str) -> bool {
        sequence
            .and_then(|s| self.model.sequences.get(s))
            .is_some_and(|s| s.act_mods.iter().any(|m| m == name))
    }

    /// The sequence playing `activity` that best fits `mods` (`SelectWeightedSequenceFromModifiers`):
    /// each modifier it has that is wanted counts two, each one not wanted costs one; a tie is
    /// drawn at random.
    fn select(&self, activity: &str, mods: &[&str], roll: u32) -> Option<usize> {
        let mut top = i32::MIN;
        let mut best = Vec::new();
        for (index, seq) in self.model.sequences.iter().enumerate() {
            if !seq.activity.eq_ignore_ascii_case(activity) {
                continue;
            }
            let score: i32 = seq
                .act_mods
                .iter()
                .map(|m| if mods.contains(&m.as_str()) { 2 } else { -1 })
                .sum();
            if score > top {
                top = score;
                best.clear();
            }
            if score == top {
                best.push(index);
            }
        }
        (!best.is_empty()).then(|| best[roll as usize % best.len()])
    }
}

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
    Ready(T),
}

/// The CS:GO pack and both animation sets, read off the main thread.
struct Pack {
    vpk: Arc<mdl_source::Vpk>,
    /// The Counter-Terrorists' and the Terrorists'.
    animsets: [Option<Arc<Animset>>; 2],
}

fn open_pack() -> Option<Arc<Pack>> {
    let path = asset_transport::find_csgo_pak()?;
    let vpk = match mdl_source::Vpk::open(&path) {
        Ok(vpk) => Arc::new(vpk),
        Err(error) => {
            diag::warn!(World, "csgo bodies: {error}");
            return None;
        }
    };
    let animset = |name: &str| {
        let path = format!("models/player/custom_player/{name}.mdl");
        let mdl = vpk.read(&path)?;
        let ani = vpk.read(&path.replace(".mdl", ".ani"));
        match StudioModel::parse_with_ani(&mdl, &[], &[], ani.as_deref()) {
            Ok(model) => {
                diag::info!(
                    World,
                    "csgo bodies: {name}: {} bones, {} sequences, {} animations",
                    model.bones.len(),
                    model.sequences.len(),
                    model.animations.len()
                );
                Some(Arc::new(Animset::new(model)))
            }
            Err(error) => {
                diag::warn!(World, "csgo bodies: {name}: {error}");
                None
            }
        }
    };
    let animsets = [animset("animset_ct"), animset("animset_t")];
    Some(Arc::new(Pack { vpk, animsets }))
}

fn image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
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
    let from_animset = studio
        .bones
        .iter()
        .map(|bone| {
            animset
                .model
                .bones
                .iter()
                .position(|b| b.name.eq_ignore_ascii_case(&bone.name))
        })
        .collect();
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

/// The CS:GO world model a body holding CS weapon `index` shows (`w_rif_ak47`), the side's
/// own knife for the knife.
fn held_model_name(index: u8, ct: bool) -> Option<String> {
    use weapon_iw4::cs;
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

/// One animation layer (`CAnimationLayer`).
#[derive(Clone, Copy, Default)]
struct Layer {
    sequence: Option<usize>,
    cycle: f32,
    rate: f32,
    weight: f32,
    weight_rate: f32,
}

/// How long an aim matrix's state has held, and its blend (`aimmatrix_transition_t`).
#[derive(Clone, Copy, Default)]
struct Transition {
    valid_for: f32,
    invalid_for: f32,
    blend_in_wait: f32,
    blend_out_wait: f32,
    blend: f32,
}

impl Transition {
    fn new(blend_in_wait: f32, blend_out_wait: f32) -> Self {
        Self {
            blend_in_wait,
            blend_out_wait,
            ..Self::default()
        }
    }

    fn update(&mut self, valid: bool, dt: f32, speed: f32) {
        if valid {
            self.invalid_for = 0.0;
            self.valid_for += dt;
            if self.valid_for >= self.blend_in_wait {
                self.blend = approach(1.0, self.blend, speed);
            }
        } else {
            self.valid_for = 0.0;
            self.invalid_for += dt;
            if self.invalid_for >= self.blend_out_wait {
                self.blend = approach(0.0, self.blend, speed);
            }
        }
    }
}

/// What the game says about a player this frame.
struct Input {
    position: Vec3,
    eye_yaw: f32,
    eye_pitch: f32,
    velocity: Vec3,
    duck: f32,
    on_ground: bool,
    walking: bool,
    scoped: bool,
    max_speed: f32,
    class: &'static str,
}

/// Something the player did that starts an animation (`PlayerAnimEvent_t`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Event {
    Fire,
    Operate,
    SilencerAttach,
    SilencerDetach,
    Reload,
    ReloadStart,
    ReloadLoop,
    ReloadEnd,
    Deploy,
    Jump,
}

/// One player's CS:GO animation state (`CCSGOPlayerAnimState`).
struct AnimState {
    ct: bool,
    first_run: bool,
    time: f32,
    dt: f32,
    seed: u32,
    params: Vec<f32>,
    layers: [Layer; LAYER_COUNT],
    class: &'static str,
    eye_yaw: f32,
    eye_pitch: f32,
    foot_yaw: f32,
    move_yaw: f32,
    move_yaw_ideal: f32,
    lower_body_target: f32,
    lower_body_realign_at: f32,
    duck: f32,
    duck_additional: f32,
    position: Vec3,
    velocity: Vec3,
    velocity_dir: Vec3,
    velocity_z: f32,
    speed: f32,
    portion_run: f32,
    portion_walk: f32,
    portion_crouch: f32,
    duration_moving: f32,
    walk_to_run: f32,
    to_run: bool,
    stand_walk: Transition,
    stand_run: Transition,
    crouch_walk: Transition,
    primary_cycle: f32,
    move_weight: f32,
    on_ground: bool,
    landing: bool,
    jumping: bool,
    in_air_smooth: f32,
    duration_in_air: f32,
    left_ground_height: f32,
    land_multiplier: f32,
    velocity_last: Vec3,
    velocity_test_at: f32,
    accel: Vec3,
    target_accel: Vec3,
    // What the game showed last frame, to see events in.
    seen_weapon: u32,
    seen_fire_ms: i32,
    seen_weaponstate: i32,
    seen_grenade: u32,
    seen_silencers: u32,
    /// The weapon class the idle loop was chosen for.
    alive_class: &'static str,
}

impl AnimState {
    fn new(animset: &Animset, ct: bool, seed: u32) -> Self {
        Self {
            ct,
            first_run: true,
            time: 0.0,
            dt: 0.0,
            seed: seed.wrapping_mul(2_654_435_761) | 1,
            params: animset.model.default_pose_params(),
            layers: [Layer::default(); LAYER_COUNT],
            class: "rifle",
            eye_yaw: 0.0,
            eye_pitch: 0.0,
            foot_yaw: 0.0,
            move_yaw: 0.0,
            move_yaw_ideal: 0.0,
            lower_body_target: 0.0,
            lower_body_realign_at: 0.0,
            duck: 0.0,
            duck_additional: 0.0,
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            velocity_dir: Vec3::X,
            velocity_z: 0.0,
            speed: 0.0,
            portion_run: 0.0,
            portion_walk: 0.0,
            portion_crouch: 0.0,
            duration_moving: 0.0,
            walk_to_run: 0.0,
            to_run: true,
            stand_walk: Transition::new(0.4, 0.2),
            stand_run: Transition::new(0.2, 0.4),
            crouch_walk: Transition::new(0.3, 0.3),
            primary_cycle: 0.0,
            move_weight: 0.0,
            on_ground: true,
            landing: false,
            jumping: false,
            in_air_smooth: 0.0,
            duration_in_air: 0.0,
            left_ground_height: 0.0,
            land_multiplier: 1.0,
            velocity_last: Vec3::ZERO,
            velocity_test_at: 0.0,
            accel: Vec3::ZERO,
            target_accel: Vec3::ZERO,
            seen_weapon: 0,
            seen_fire_ms: 0,
            seen_weaponstate: 0,
            seen_grenade: 0,
            seen_silencers: 0,
            alive_class: "",
        }
    }

    fn roll(&mut self) -> u32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed
    }

    fn random(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * (self.roll() % 10_000) as f32 / 10_000.0
    }

    fn set_param(&mut self, animset: &Animset, param: Option<usize>, value: f32) {
        let Some(index) = param else {
            return;
        };
        let Some(desc) = animset.model.pose_params.get(index) else {
            return;
        };
        let (low, high) = (desc.start.min(desc.end), desc.start.max(desc.end));
        if let Some(slot) = self.params.get_mut(index) {
            *slot = value.clamp(low, high);
        }
    }

    fn set_sequence(&mut self, animset: &Animset, layer: usize, sequence: Option<usize>) {
        if let Some(sequence) = sequence {
            self.layers[layer] = Layer {
                sequence: Some(sequence),
                cycle: 0.0,
                rate: animset.cycle_rate(sequence),
                weight: 0.0,
                weight_rate: self.layers[layer].weight_rate,
            };
        }
    }

    fn increment_cycle(&mut self, layer: usize, allow_loop: bool) {
        let l = &mut self.layers[layer];
        if l.rate.abs() <= 0.0 {
            return;
        }
        let mut cycle = l.cycle + self.dt * l.rate;
        if !allow_loop && cycle >= 1.0 {
            cycle = 0.999;
        }
        l.cycle = clamp_cycle(cycle);
    }

    fn increment_weight(&mut self, layer: usize) {
        let l = &mut self.layers[layer];
        if l.weight_rate.abs() <= 0.0 {
            return;
        }
        l.weight = (l.weight + self.dt * l.weight_rate).clamp(0.0, 1.0);
    }

    fn set_weight(&mut self, layer: usize, weight: f32) {
        self.layers[layer].weight = weight.clamp(0.0, 1.0);
    }

    fn set_weight_rate(&mut self, layer: usize, previous: f32) {
        if self.dt > 0.0 {
            self.layers[layer].weight_rate = (self.layers[layer].weight - previous) / self.dt;
        }
    }

    fn completed(&self, layer: usize) -> bool {
        let l = &self.layers[layer];
        l.cycle + self.dt * l.rate >= 1.0
    }

    /// The weight a layer's sequence wants at its cycle, easing in and out (`fadein`/`fadeout`).
    fn ideal_weight(&self, animset: &Animset, layer: usize) -> f32 {
        let l = &self.layers[layer];
        let Some(seq) = l.sequence.and_then(|s| animset.model.sequences.get(s)) else {
            return 0.0;
        };
        let cycle = if l.cycle >= 0.999 { 1.0 } else { l.cycle };
        let mut weight = 1.0;
        if seq.fade_in > 0.0 && cycle < seq.fade_in {
            weight = smoothstep_bounds(0.0, seq.fade_in, cycle);
        } else if seq.fade_out < 1.0 && cycle > seq.fade_out {
            weight = smoothstep_bounds(1.0, seq.fade_out, cycle);
        }
        if weight < 0.0015 { 0.0 } else { weight.clamp(0.0, 1.0) }
    }

    fn update_layer(&mut self, layer: usize, sequence: Option<usize>, rate: f32, weight: f32, cycle: f32) {
        if let Some(sequence) = sequence {
            self.layers[layer] = Layer {
                sequence: Some(sequence),
                rate,
                cycle: cycle.clamp(0.0, 1.0),
                weight: weight.clamp(0.0, 1.0),
                weight_rate: self.layers[layer].weight_rate,
            };
        }
    }

    /// The activity modifiers events choose sequences by (`UpdateActivityModifiers`).
    fn mods(&self) -> Vec<&'static str> {
        let mut mods = vec![self.class];
        if self.portion_walk > 0.25 {
            mods.push("moving");
        }
        if self.duck > 0.55 {
            mods.push("crouch");
        }
        mods
    }

    fn select(&mut self, animset: &Animset, activity: &str) -> Option<usize> {
        let mods = self.mods();
        let roll = self.roll();
        animset.select(activity, &mods, roll)
    }

    /// `DoAnimationEvent`.
    fn event(&mut self, animset: &Animset, event: Event) {
        let (layer, activity) = match event {
            Event::Fire => (WEAPON_ACTION, "ACT_CSGO_FIRE_PRIMARY"),
            Event::Operate => (WEAPON_ACTION, "ACT_CSGO_OPERATE"),
            Event::SilencerAttach => (WEAPON_ACTION, "ACT_CSGO_SILENCER_ATTACH"),
            Event::SilencerDetach => (WEAPON_ACTION, "ACT_CSGO_SILENCER_DETACH"),
            // Shotguns load shell by shell instead.
            Event::Reload if self.class == "shotgun" => return,
            Event::Reload => (WEAPON_ACTION, "ACT_CSGO_RELOAD"),
            Event::ReloadStart => (WEAPON_ACTION, "ACT_CSGO_RELOAD_START"),
            Event::ReloadLoop => (WEAPON_ACTION, "ACT_CSGO_RELOAD_LOOP"),
            Event::ReloadEnd => (WEAPON_ACTION, "ACT_CSGO_RELOAD_END"),
            Event::Deploy => (WEAPON_ACTION, "ACT_CSGO_DEPLOY"),
            Event::Jump => {
                self.jumping = true;
                (JUMP_OR_FALL, "ACT_CSGO_JUMP")
            }
        };
        let sequence = self.select(animset, activity);
        self.set_sequence(animset, layer, sequence);
    }

    /// One frame (`Update`): every layer's sequence, cycle and weight, and the pose parameters.
    fn update(&mut self, animset: &Animset, input: &Input, dt: f32) {
        self.dt = dt.max(0.0);
        self.time += self.dt;
        self.eye_yaw = angle_normalize(input.eye_yaw);
        self.eye_pitch = angle_normalize(input.eye_pitch);
        self.position = input.position;
        if self.class != input.class {
            self.class = input.class;
        }
        self.duck = approach(
            (input.duck + self.duck_additional).clamp(0.0, 1.0),
            self.duck,
            self.dt * DUCK_APPROACH_SPEED,
        )
        .clamp(0.0, 1.0);
        self.velocity_phase(animset, input);
        self.aim_matrix(animset, input);
        self.weapon_action(animset);
        self.movement(animset, input);
        self.alive_loop(animset);
        self.lean(animset, input);
        self.first_run = false;
    }

    /// `SetUpVelocity`: speed, the feet's yaw, move yaw and the aim matrix's yaw and pitch.
    fn velocity_phase(&mut self, animset: &Animset, input: &Input) {
        let p = animset.params;
        let dt = self.dt;
        let mut velocity = input.velocity;
        let limit = SPEED_RUN * MAX_VEL_LIMIT;
        if velocity.length_squared() > limit * limit {
            velocity = velocity.normalize() * limit;
        }
        self.velocity_z = velocity.z;
        velocity.z = 0.0;
        self.velocity = approach_vec(velocity, self.velocity, dt * 2000.0);
        self.speed = self.velocity.length().min(SPEED_RUN);
        if self.speed > 0.0 {
            self.velocity_dir = self.velocity.normalize_or(self.velocity_dir);
        }
        let max_run = input.max_speed.max(0.001);
        self.portion_run = (self.speed / max_run).clamp(0.0, 1.0);
        self.portion_walk = self.speed / (max_run * SPEED_WALK_MODIFIER);
        self.portion_crouch = self.speed / (max_run * SPEED_DUCK_MODIFIER);
        let started_moving = self.speed > 0.0 && self.duration_moving <= 0.0;
        if self.speed > 0.0 {
            self.duration_moving += dt;
        } else {
            self.duration_moving = 0.0;
        }

        // The feet stay put while the eyes look around within the aim matrix's width (narrower
        // on the move), and turn to them once moving or after a while.
        self.foot_yaw = self.foot_yaw.clamp(-360.0, 360.0);
        let eye_foot = angle_diff(self.eye_yaw, self.foot_yaw);
        let mut width = lerp(
            self.portion_walk.clamp(0.0, 1.0),
            1.0,
            lerp(self.walk_to_run, AIM_NARROW_WALK, AIM_NARROW_RUN),
        );
        if self.duck > 0.0 {
            width = lerp(
                self.duck * self.portion_crouch.clamp(0.0, 1.0),
                width,
                AIM_NARROW_CROUCHMOVING,
            );
        }
        let (yaw_max, yaw_min) = (AIM_YAW_MAX * width, AIM_YAW_MIN * width);
        if eye_foot > yaw_max {
            self.foot_yaw = self.eye_yaw - yaw_max.abs();
        } else if eye_foot < yaw_min {
            self.foot_yaw = self.eye_yaw + yaw_min.abs();
        }
        self.foot_yaw = angle_normalize(self.foot_yaw);
        if self.speed > 0.1 || self.velocity_z.abs() > 100.0 {
            self.foot_yaw = approach_angle(
                self.eye_yaw,
                self.foot_yaw,
                dt * (30.0 + 20.0 * self.walk_to_run),
            );
            self.lower_body_realign_at = self.time + LOWER_REALIGN_DELAY * 0.2;
            self.lower_body_target = self.eye_yaw;
        } else {
            self.foot_yaw = approach_angle(self.lower_body_target, self.foot_yaw, dt * LOWER_CATCHUP_IDLE);
            if self.time > self.lower_body_realign_at
                && angle_diff(self.foot_yaw, self.eye_yaw).abs() > 35.0
            {
                self.lower_body_realign_at = self.time + LOWER_REALIGN_DELAY;
                self.lower_body_target = self.eye_yaw;
            }
        }

        // Which way the legs move, relative to the feet (180 forward).
        if self.speed > 0.0 {
            let mut raw = (-self.velocity.y).atan2(-self.velocity.x).to_degrees();
            if raw < 0.0 {
                raw += 360.0;
            }
            self.move_yaw_ideal = angle_normalize(angle_diff(raw, self.foot_yaw));
        }
        let to_ideal = angle_normalize(angle_diff(self.move_yaw_ideal, self.move_yaw));
        if started_moving && self.move_weight <= 0.0 {
            self.move_yaw = self.move_yaw_ideal;
        } else {
            let weight = lerp(
                self.duck,
                self.portion_walk.clamp(0.0, 1.0),
                self.portion_crouch.clamp(0.0, 1.0),
            );
            let ratio = bias(weight, 0.18) + 0.1;
            self.move_yaw = angle_normalize(self.move_yaw + to_ideal * ratio);
        }
        self.set_param(animset, p.move_yaw, self.move_yaw);

        // The aim matrix's yaw and pitch (its limits are CS:GO's defaults, ±58° and ±90°, so
        // no rescaling).
        self.set_param(animset, p.body_yaw, angle_diff(self.eye_yaw, self.foot_yaw));
        self.set_param(animset, p.body_pitch, angle_diff(self.eye_pitch, 0.0));
        self.set_param(animset, p.speed, self.portion_walk);
        self.set_param(animset, p.stand, 1.0 - self.duck * self.in_air_smooth);
    }

    /// `SetUpAimMatrix`: the full-body aim poses, cross-faded between standing, walking,
    /// running and crouched as the player's speed holds for a moment.
    fn aim_matrix(&mut self, animset: &Animset, input: &Input) {
        let p = animset.params;
        if self.duck <= 0.0 || self.duck >= 1.0 {
            let speed = self.dt
                * if input.scoped {
                    SPEED_TO_CHANGE_AIM_MATRIX_SCOPED
                } else {
                    SPEED_TO_CHANGE_AIM_MATRIX
                };
            if input.scoped {
                for t in [&mut self.stand_walk, &mut self.stand_run, &mut self.crouch_walk] {
                    t.invalid_for = t.blend_out_wait;
                }
            }
            let dt = self.dt;
            self.stand_walk.update(
                input.walking && !input.scoped && self.portion_walk > 0.7 && self.portion_run < 0.7,
                dt,
                speed,
            );
            self.stand_run
                .update(!input.scoped && self.portion_run >= 0.7, dt, speed);
            self.crouch_walk
                .update(!input.scoped && self.portion_crouch >= 0.5, dt, speed);
        }
        let mut stand_idle = 1.0;
        let mut stand_walk = self.stand_walk.blend;
        let mut stand_run = self.stand_run.blend;
        let mut crouch_idle = 1.0;
        let mut crouch_walk = self.crouch_walk.blend;
        if stand_walk >= 1.0 {
            stand_idle = 0.0;
        }
        if stand_run >= 1.0 {
            stand_idle = 0.0;
            stand_walk = 0.0;
        }
        if crouch_walk >= 1.0 {
            crouch_idle = 0.0;
        }
        if self.duck >= 1.0 {
            stand_idle = 0.0;
            stand_walk = 0.0;
            stand_run = 0.0;
        } else if self.duck <= 0.0 {
            crouch_idle = 0.0;
            crouch_walk = 0.0;
        }
        crouch_idle *= self.duck;
        crouch_walk *= self.duck;
        stand_walk *= 1.0 - self.duck;
        stand_run *= 1.0 - self.duck;
        if crouch_idle < 1.0 && crouch_walk < 1.0 && stand_walk < 1.0 && stand_run < 1.0 {
            stand_idle = 1.0;
        }
        self.set_param(animset, p.aim_stand_idle, stand_idle);
        self.set_param(animset, p.aim_stand_walk, stand_walk);
        self.set_param(animset, p.aim_stand_run, stand_run);
        self.set_param(animset, p.aim_crouch_idle, crouch_idle);
        self.set_param(animset, p.aim_crouch_walk, crouch_walk);
        let aim = animset.model.sequence_by_label(&format!("{}_aim", self.class));
        self.update_layer(AIMMATRIX, aim, 0.0, 1.0, 0.0);
    }

    /// `SetUpWeaponAction`: firing, reloading, deploying; re-crouching a standing action.
    fn weapon_action(&mut self, animset: &Animset) {
        let action = self.layers[WEAPON_ACTION];
        let mut recrouch = 0.0;
        if action.weight > 0.0 {
            if self.layers[WEAPON_ACTION_RECROUCH].sequence.is_none() {
                self.layers[WEAPON_ACTION_RECROUCH].sequence = animset.recrouch;
            }
            if animset.has_act_mod(action.sequence, "crouch") {
                if self.duck < 1.0 {
                    recrouch = action.weight * (1.0 - self.duck);
                }
            } else if self.duck > 0.0 {
                recrouch = action.weight * self.duck;
            }
        } else if self.layers[WEAPON_ACTION_RECROUCH].weight > 0.0 {
            recrouch = approach(0.0, self.layers[WEAPON_ACTION_RECROUCH].weight, self.dt * 4.0);
        }
        self.set_weight(WEAPON_ACTION_RECROUCH, recrouch);
        // CS:GO steps the action's cycle twice a frame.
        self.increment_cycle(WEAPON_ACTION, false);
        let previous = self.layers[WEAPON_ACTION].weight;
        let ideal = self.ideal_weight(animset, WEAPON_ACTION);
        self.set_weight(WEAPON_ACTION, ideal);
        self.set_weight_rate(WEAPON_ACTION, previous);
        if self.layers[WEAPON_ACTION].weight > 0.0 {
            self.increment_cycle(WEAPON_ACTION, false);
            self.increment_weight(WEAPON_ACTION);
        }
    }

    /// `SetUpMovement`: the locomotion cycle and its weight, jumping, falling, landing.
    fn movement(&mut self, animset: &Animset, input: &Input) {
        let p = animset.params;
        let dt = self.dt;
        if self.walk_to_run > 0.0 && self.walk_to_run < 1.0 {
            let step = dt * WALK_TO_RUN_TRANSITION_SPEED;
            self.walk_to_run = (self.walk_to_run + if self.to_run { step } else { -step }).clamp(0.0, 1.0);
        }
        let walk_threshold = SPEED_RUN * SPEED_WALK_MODIFIER;
        if self.speed > walk_threshold && !self.to_run {
            self.to_run = true;
            self.walk_to_run = self.walk_to_run.max(0.01);
        } else if self.speed < walk_threshold && self.to_run {
            self.to_run = false;
            self.walk_to_run = self.walk_to_run.min(0.99);
        }
        self.set_param(animset, p.move_blend_walk, (1.0 - self.walk_to_run) * (1.0 - self.duck));
        self.set_param(animset, p.move_blend_run, self.walk_to_run * (1.0 - self.duck));
        self.set_param(animset, p.move_blend_crouch, self.duck);

        let move_seq = animset
            .model
            .sequence_by_label(&format!("move_{}", self.class))
            .or_else(|| animset.model.sequence_by_label("move"));
        let target = lerp(
            self.duck,
            self.portion_walk.clamp(0.0, 1.0),
            self.portion_crouch.clamp(0.0, 1.0),
        );
        if self.move_weight <= target {
            self.move_weight = target;
        } else {
            self.move_weight = approach(target, self.move_weight, dt * 2.0);
        }
        let yaw = (self.foot_yaw + self.move_yaw + 180.0).to_radians();
        let move_dir = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
        self.move_weight *= bias(self.velocity_dir.dot(move_dir).abs(), 0.2);
        let mut weight = self.move_weight * self.in_air_smooth;
        weight *= (1.0 - self.layers[LAND_OR_CLIMB].weight).max(0.55);
        let mut rate = 0.0;
        if let Some(seq) = move_seq.filter(|_| self.speed > 0.0) {
            // The cycle covers as much ground as the player does.
            let stride = Vec3::from(mdl_source::blend::sequence_movement(&animset.model, seq, &self.params))
                .length();
            rate = if stride > 0.001 {
                self.speed / stride
            } else {
                animset.cycle_rate(seq)
            };
            rate *= lerp(self.walk_to_run, 1.0, RUN_ANIM_PLAYBACK_MULTIPLIER);
        }
        let increment = rate * dt;
        self.primary_cycle = clamp_cycle(self.primary_cycle + increment);
        self.update_layer(MOVE, move_seq, increment, weight.clamp(0.0, 1.0), self.primary_cycle);

        let was_on_ground = self.on_ground;
        self.on_ground = input.on_ground;
        let landed = !self.first_run && !was_on_ground && self.on_ground;
        let left_ground = was_on_ground && !self.on_ground;
        if left_ground {
            self.left_ground_height = self.position.z;
        }
        if landed {
            let fell = (self.left_ground_height - self.position.z).abs();
            let fall = bias(remap_clamped(fell, 12.0, 72.0, 0.0, 1.0), 0.4);
            self.land_multiplier = bias(self.duration_in_air, 0.3).clamp(0.1, 1.0);
            self.duck_additional = self.land_multiplier.max(fall);
        } else {
            self.duck_additional = approach(0.0, self.duck_additional, dt * 2.0);
        }
        self.in_air_smooth = approach(
            if self.on_ground { 1.0 } else { 0.0 },
            self.in_air_smooth,
            lerp(self.duck, ONGROUND_FUZZY_APPROACH, ONGROUND_FUZZY_APPROACH_CROUCH) * dt,
        )
        .clamp(0.0, 1.0);

        if self.on_ground {
            if !self.landing && landed {
                let activity = if self.duration_in_air > 1.0 {
                    "ACT_CSGO_LAND_HEAVY"
                } else {
                    "ACT_CSGO_LAND_LIGHT"
                };
                let seq = self.select(animset, activity);
                self.set_sequence(animset, LAND_OR_CLIMB, seq);
                self.layers[LAND_OR_CLIMB].cycle = 0.0;
                self.landing = true;
            }
            self.duration_in_air = 0.0;
            if self.landing {
                self.jumping = false;
                self.increment_cycle(LAND_OR_CLIMB, false);
                self.increment_cycle(JUMP_OR_FALL, false);
                self.set_param(animset, p.jump_fall, 0.0);
                if self.completed(LAND_OR_CLIMB) {
                    self.landing = false;
                    self.set_weight(LAND_OR_CLIMB, 0.0);
                    self.set_weight(JUMP_OR_FALL, 0.0);
                    self.land_multiplier = 1.0;
                } else {
                    let land = self.ideal_weight(animset, LAND_OR_CLIMB)
                        * self.land_multiplier
                        * (1.0 - self.duck).clamp(0.2, 1.0);
                    self.set_weight(LAND_OR_CLIMB, land);
                    let jump = self.layers[JUMP_OR_FALL].weight;
                    if jump > 0.0 {
                        self.set_weight(JUMP_OR_FALL, approach(0.0, jump, dt * 10.0));
                    }
                }
            }
            if !self.landing && !self.jumping {
                self.set_weight(LAND_OR_CLIMB, 0.0);
            }
        } else {
            self.landing = false;
            if left_ground {
                if !self.jumping {
                    let seq = self.select(animset, "ACT_CSGO_FALL");
                    self.set_sequence(animset, JUMP_OR_FALL, seq);
                }
                self.duration_in_air = 0.0;
            }
            self.duration_in_air += dt;
            self.increment_cycle(JUMP_OR_FALL, false);
            let next = self.ideal_weight(animset, JUMP_OR_FALL);
            if next > self.layers[JUMP_OR_FALL].weight {
                self.set_weight(JUMP_OR_FALL, next);
            }
            let lingering = self.layers[LAND_OR_CLIMB].weight;
            if lingering > 0.0 {
                self.set_weight(
                    LAND_OR_CLIMB,
                    lingering * smoothstep_bounds(0.2, 0.0, self.duration_in_air),
                );
            }
            self.set_param(
                animset,
                p.jump_fall,
                smoothstep_bounds(0.72, 1.52, self.duration_in_air).clamp(0.0, 1.0),
            );
        }
    }

    /// `SetUpAliveloop`: breathing and fidgeting, faded out when running.
    fn alive_loop(&mut self, animset: &Animset) {
        if animset.activity(self.layers[ALIVELOOP].sequence) != "ACT_CSGO_ALIVE_LOOP" {
            let seq = self.select(animset, "ACT_CSGO_ALIVE_LOOP");
            self.set_sequence(animset, ALIVELOOP, seq);
            self.layers[ALIVELOOP].cycle = self.random(0.0, 1.0);
            if let Some(seq) = self.layers[ALIVELOOP].sequence {
                self.layers[ALIVELOOP].rate = animset.cycle_rate(seq) * self.random(0.8, 1.1);
            }
            self.alive_class = self.class;
        } else if self.alive_class != self.class {
            // A new weapon's own loop, carrying on where the old one was.
            let cycle = self.layers[ALIVELOOP].cycle;
            let seq = self.select(animset, "ACT_CSGO_ALIVE_LOOP");
            self.set_sequence(animset, ALIVELOOP, seq);
            self.layers[ALIVELOOP].cycle = cycle;
            self.alive_class = self.class;
        } else if self.completed(ALIVELOOP) {
            if let Some(seq) = self.layers[ALIVELOOP].sequence {
                self.layers[ALIVELOOP].rate = animset.cycle_rate(seq) * self.random(0.8, 1.1);
            }
        } else {
            let weight = remap_clamped(self.portion_run, 0.55, 0.9, 1.0, 0.0);
            self.set_weight(ALIVELOOP, weight);
        }
        self.increment_cycle(ALIVELOOP, true);
    }

    /// `SetUpLean`: the body leans into acceleration.
    fn lean(&mut self, animset: &Animset, input: &Input) {
        let interval = self.time - self.velocity_test_at;
        if interval > 0.025 {
            let interval = interval.min(0.1);
            self.velocity_test_at = self.time;
            self.target_accel = (input.velocity - self.velocity_last) / interval;
            self.target_accel.z = 0.0;
            self.velocity_last = input.velocity;
        }
        self.accel = approach_vec(self.target_accel, self.accel, self.dt * 800.0);
        let accel_yaw = self.accel.y.atan2(self.accel.x).to_degrees();
        let weight = (self.accel.length() / SPEED_RUN * self.portion_run).clamp(0.0, 1.0);
        self.set_param(animset, animset.params.lean_yaw, angle_normalize(self.foot_yaw - accel_yaw));
        if self.layers[LEAN].sequence.is_none() {
            self.set_sequence(animset, LEAN, animset.lean);
        }
        self.set_weight(LEAN, weight);
    }

    /// The animation set's local transforms for this frame: the rest pose with every layer
    /// played over it in order.
    fn pose(&self, animset: &Animset, out: &mut Vec<BoneFrame>) {
        out.clear();
        out.extend_from_slice(&animset.model.rest);
        for layer in &self.layers {
            let Some(sequence) = layer.sequence.filter(|_| layer.weight > 0.0) else {
                continue;
            };
            mdl_source::blend::accumulate(
                &animset.model,
                out,
                sequence,
                layer.cycle,
                layer.weight,
                &self.params,
            );
        }
    }
}

/// The CS:GO bodies: the pack, the agents read so far, each player's animation state.
#[derive(Resource, Default)]
pub struct CsgoBodies {
    pack: Option<Slot<Option<Arc<Pack>>>>,
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

impl CsgoBodies {
    fn pack(&mut self) -> Option<Arc<Pack>> {
        match self.pack.get_or_insert_with(|| {
            Slot::Loading(AsyncComputeTaskPool::get().spawn(async { open_pack() }))
        }) {
            Slot::Ready(pack) => pack.clone(),
            Slot::Loading(task) => {
                if !task.is_finished() {
                    return None;
                }
                let pack = block_on(task);
                self.pack = Some(Slot::Ready(pack.clone()));
                pack
            }
        }
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

/// The server's `cs_factions` ("<allies> <axis>"): each side's MW2 faction.
fn factions(snapshot: &sim::Snapshot) -> Option<(String, String)> {
    let value = snapshot
        .meta
        .objectives
        .server_info
        .iter()
        .find(|(name, _)| name == "cs_factions")?
        .1
        .clone();
    let (allies, axis) = value.split_once(' ')?;
    Some((allies.to_owned(), axis.to_owned()))
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
    let Some(pack) = bodies.pack() else {
        return;
    };
    let (allies, axis) = factions(snapshot).unwrap_or_default();
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
            });
            hidden.0.insert(entity);
            live.insert(entity);
            continue;
        }
        let Some(ps) = presented.player(client) else {
            continue;
        };
        let is_allies = remote.client_state_team == entity_iw4::TEAM_ALLIES;
        let faction = if is_allies { &allies } else { &axis };
        let name = agent_model(weapon_iw4::cs::csgo_radio_voice(faction, is_allies));
        let Some(character) = bodies.character(&pack, name, &mut images) else {
            continue;
        };
        let Some(animset) = pack.animsets[usize::from(!character.ct)].clone() else {
            continue;
        };
        let entity = u32::from(identity.number());
        live.insert(entity);

        let weapon = ps.weapon;
        let cs_index = weapon_iw4::cs::cs_weapon_index_for(&weapons.0.script_name_of(weapon));
        let class = cs_index.map_or("knife", weapon_iw4::cs::csgo_anim_class);
        let max_speed = cs_index
            .and_then(weapon_iw4::cs::cs_weapon)
            .map_or(250.0, |gun| gun.max_speed);
        let input = Input {
            position: Vec3::from(runtime.origin),
            eye_yaw: runtime.angles[1],
            eye_pitch: runtime.angles[0],
            velocity: Vec3::from(ps.velocity),
            duck: ps.cs_duck_amount.clamp(0.0, 1.0),
            on_ground: ps.ground_entity_num != ENTITYNUM_NONE,
            walking: ps.cs_move_buttons & 0x2 != 0,
            scoped: ps.cs_zoom != 0,
            max_speed,
            class,
        };
        let state = bodies
            .states
            .entry(entity)
            .or_insert_with(|| AnimState::new(&animset, character.ct, entity));
        if state.ct != character.ct {
            *state = AnimState::new(&animset, character.ct, entity);
        }
        // What happened since the last frame.
        let mut events = Vec::new();
        if !state.first_run {
            if weapon != state.seen_weapon {
                events.push(Event::Deploy);
            } else if ps.cs_last_fire_ms != state.seen_fire_ms && ps.cs_last_fire_ms != 0 {
                events.push(Event::Fire);
            }
            let reloading = |s: i32| s == weapon_iw4::WeaponState::Reloading as i32;
            let state_now = ps.weaponstate_primary;
            if state_now != state.seen_weaponstate {
                if state_now == weapon_iw4::WeaponState::ReloadStart as i32 {
                    events.push(Event::ReloadStart);
                } else if reloading(state_now) {
                    events.push(if class == "shotgun" { Event::ReloadLoop } else { Event::Reload });
                } else if state_now == weapon_iw4::WeaponState::ReloadEnd as i32 {
                    events.push(Event::ReloadEnd);
                }
            }
            if ps.cs_grenade == cs_grenade::PULLED && state.seen_grenade != cs_grenade::PULLED {
                events.push(Event::Operate);
            }
            // The silencer screwed on or off the gun in hand.
            if let Some(index) = cs_index.filter(|_| weapon == state.seen_weapon) {
                let bit = weapon_iw4::cs::silencer_bit(index);
                if (ps.cs_silencers ^ state.seen_silencers) & bit != 0 {
                    let on = weapon_iw4::cs::silencer_on(ps.cs_silencers, bit, ps.cs_shooting_mode);
                    events.push(if on { Event::SilencerAttach } else { Event::SilencerDetach });
                }
            }
            // A take-off: rising fast just after leaving the ground (the first airborne state
            // can come before the jump's speed does).
            if !input.on_ground
                && !state.jumping
                && input.velocity.z > 100.0
                && (state.on_ground || state.duration_in_air < 0.2)
            {
                events.push(Event::Jump);
            }
        }
        state.seen_weapon = weapon;
        state.seen_fire_ms = ps.cs_last_fire_ms;
        state.seen_weaponstate = ps.weaponstate_primary;
        state.seen_grenade = ps.cs_grenade;
        state.seen_silencers = ps.cs_silencers;
        state.class = class;
        // Events see this frame's speed and crouch, as CS:GO's do.
        for event in events {
            state.event(&animset, event);
        }
        state.update(&animset, &input, dt);
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
        let agent_locals: Vec<BoneFrame> = character
            .from_animset
            .iter()
            .zip(&character.studio.rest)
            .map(|(from, rest)| from.and_then(|b| locals.get(b).copied()).unwrap_or(*rest))
            .collect();
        let world = character.studio.world_from_locals(&agent_locals);
        let skin: Vec<mdl_goldsrc::Mat3x4> = world
            .iter()
            .zip(&character.studio.bones)
            .map(|(w, bone)| mdl_goldsrc::concat(w, &bone.pose_to_bone))
            .collect();
        let world_from_model = Mat4::from_rotation_translation(
            Quat::from_rotation_z(state.foot_yaw.to_radians()),
            Vec3::from(runtime.origin),
        );
        draws.0.push(CsWorldModelInstance {
            model: character.gpu.clone(),
            world_from_model: world_from_model.to_cols_array_2d(),
            ambient: AMBIENT,
            sun_dir: [0.0; 3],
            sun: [0.0; 3],
            bones: Some(Arc::new(skin)),
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
            .and_then(|index| held_model_name(index, character.ct))
            .and_then(|name| bodies.held_model(&pack, &name, &mut images));
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
