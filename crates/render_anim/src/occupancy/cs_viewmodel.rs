//! Counter-Strike first-person models for the CS guns, read at runtime from the local installs
//! and drawn by `render_gpu`'s viewmodel pass in place of the MW2 viewmodel: Counter-Strike:
//! Source models (`v_rif_ak47.mdl`, … from its pack) when CS:S is installed, else CS 1.6 ones.
//! Animation follows CS: draw on switching to the gun, a shoot sequence per shot, reload when a
//! reload starts (timed to the gun's reload), idle otherwise; the sounds the animations call for
//! play as they pass their frames. The view's recoil punch moves the camera, not the gun, so the
//! gun is drawn without it. Walking bobs it gently and turning leaves it trailing slightly
//! behind; `set_viewmodel_sway(false)` keeps it still.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use assets::PreparedWeapons;
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use frame::ViewSubject;
use mdl_goldsrc::Mat3x4;
use net::{LocalPresentClient, PresentedSnapshot};
use playerstate_iw4::PlayerState;
use render_gpu::{
    CsViewmodelDraw, CsViewmodelFlash, CsViewmodelFrame, CsViewmodelModel, CsViewmodelShading,
    CsViewmodelVertex,
};
use weapon_iw4::cs::CsWeapon;

use crate::occupancy::third_person::presented_is_third_person;

/// CS 1.6 draws viewmodels with a 90 degree field of view at 4:3: tan(73.74 / 2) vertically.
const GOLDSRC_TAN_HALF_FOV_Y: f32 = 0.75;
const AMBIENT: f32 = 0.65;
const SHADE: f32 = 0.45;
/// Mirror across the view's forward/up plane. CS viewmodels are modelled left-handed (CS:S's
/// weapon scripts say `BuiltRightHanded 0`) and flipped for `cl_righthand 1`; the knives of CS:S
/// and CS 1.6 are the exceptions, modelled right-handed ([`modelled_right_handed`]).
const RIGHT_HAND_MIRROR: Mat3x4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, -1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];
/// Walk bob: ground speed at which it is full, the step cycle (vertical; lateral is twice as
/// long), and how much of the cycle rises.
const BOB_FULL_SPEED: f32 = 320.0;
const BOB_CYCLE: f64 = 0.45;
const BOB_UP: f64 = 0.5;
/// How fast the bob follows the player's speed (per second), so landings and take-offs ease in.
const BOB_SPEED_EASE: f32 = 6.0;
/// The radio call on every grenade throw (CS:S `Radio.FireInTheHole`, else CS 1.6's
/// `radio/ct_fireinhole.wav`).
const FIRE_IN_THE_HOLE: &str = "css/radio.fireinthehole";
const FIRE_IN_THE_HOLE_GOLDSRC: &str = "cs/radio/ct_fireinhole";
/// How long a muzzle flash shows, seconds (CS:S flashes for about two frames).
const FLASH_SECONDS: f64 = 0.05;
/// Flash tint (warm), scaled down as it fades.
const FLASH_TINT: [f32; 3] = [1.0, 0.86, 0.62];
/// Turn sway: how fast the gun's facing catches up (per second), the lag beyond which it catches
/// up faster, and how many units one unit of facing lag moves the gun.
const SWAY_CATCH_UP: f32 = 5.0;
const SWAY_MAX_LAG: f32 = 0.5;
const SWAY_UNITS: f32 = 1.5;

static SWAY: AtomicBool = AtomicBool::new(true);

/// Whether the gun bobs when walking and trails when turning.
pub fn viewmodel_sway() -> bool {
    SWAY.load(Ordering::Relaxed)
}

pub fn set_viewmodel_sway(on: bool) {
    SWAY.store(on, Ordering::Relaxed);
}

/// Whether a CS model replaces the MW2 viewmodel this frame; `occupy_fpv_scene` reads it.
#[derive(Resource, Default)]
pub struct CsViewmodelActive(pub bool);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    GoldSrc,
    Source,
}

enum Studio {
    GoldSrc(mdl_goldsrc::StudioModel),
    Source(mdl_source::StudioModel),
}

impl Studio {
    /// Matrices that place each vertex's bind-space position in view space (before placement).
    fn pose(&self, sequence: usize, seconds: f32, out: &mut Vec<Mat3x4>) {
        match self {
            Self::GoldSrc(model) => model.pose(sequence, seconds, out),
            Self::Source(model) => model.pose(sequence, seconds, out),
        }
    }
}

/// What the player needs from one sequence.
struct SequenceInfo {
    fps: f32,
    num_frames: usize,
    duration: f32,
    /// (frame, sound alias without the `/plr` suffix).
    sounds: Vec<(f32, String)>,
}

/// The M4A1's and USP's silenced animations, and the two that screw the silencer on and off.
/// (`Roles`' own idle/draw/reload/shoot are the plain gun.)
struct Silenced {
    idle: Option<usize>,
    draw: Option<usize>,
    reload: Option<usize>,
    shoot: Vec<usize>,
    attach: Option<usize>,
    detach: Option<usize>,
}

struct Roles {
    idle: Option<usize>,
    draw: Option<usize>,
    reload: Option<usize>,
    shoot: Vec<usize>,
    silenced: Option<Silenced>,
    /// The shots of the other hand or mode (`ACT_VM_SECONDARYATTACK`: the Glock's burst, the Elites' right gun).
    shoot_alt: Vec<usize>,
    /// A shotgun's start and finish of a shell-by-shell reload (`reload` is one shell going in).
    shell_start: Option<usize>,
    shell_finish: Option<usize>,
    /// The knife's two alternating slashes, its stab and the stab that met nothing.
    slashes: Vec<usize>,
    /// A grenade's pin pull and throw.
    pullpin: Option<usize>,
    throw: Option<usize>,
    /// The C4's keypad (`pressbutton`), played while planting.
    arm: Option<usize>,
    stab: Option<usize>,
    stab_miss: Option<usize>,
}

/// A gun's idle, draw, reload and shoot animations.
type GunSet<'a> = (Option<usize>, Option<usize>, Option<usize>, &'a [usize]);

impl Roles {
    /// The animations of the gun as it is now: with its silencer on (when it has one) or plain.
    fn gun(&self, silenced: bool) -> GunSet<'_> {
        match &self.silenced {
            Some(s) if silenced => (s.idle, s.draw, s.reload, &s.shoot),
            _ => (self.idle, self.draw, self.reload, &self.shoot),
        }
    }
}

/// The knife's sequences go by the same labels in CS:S and CS 1.6.
fn knife_roles(labels: &[&str]) -> (Vec<usize>, Option<usize>, Option<usize>) {
    let find = |name: &str| labels.iter().position(|l| l.eq_ignore_ascii_case(name));
    (
        ["midslash1", "midslash2"]
            .iter()
            .filter_map(|n| find(n))
            .collect(),
        find("stab"),
        find("stab_miss"),
    )
}

/// What the viewmodel needs from the CS weapon in hand: a gun from the table, or the knife.
#[derive(Clone, Copy, Debug)]
struct ViewWeapon {
    name: &'static str,
    view_model: &'static str,
    css_view_model: &'static str,
    /// Seconds the reload animation is stretched to; 0 without one.
    reload: f32,
    knife: bool,
    grenade: bool,
    /// The bomb carrier's C4.
    c4: bool,
    /// The CS:S model is modelled right-handed already (not mirrored).
    css_right_handed: bool,
    /// `PlayerState::cs_silencers` bit of a gun with a silencer; 0 for every other weapon.
    silencer_bit: u32,
    /// Seconds the silencer takes to go on or off; the attach/detach animation is stretched to it.
    silencer_adjust: f32,
    /// `PlayerState::cs_burst_modes` bit of this gun (burst mode, or the Elites' next hand).
    mode_bit: u32,
    /// The gun has a burst mode (Glock-18, FAMAS).
    burst: bool,
    /// Two guns firing in turn (Dual Elites).
    dual: bool,
    /// Zooming hides the gun and draws the sniper scope (the AUG and SG 552 only narrow the view).
    scope_overlay: bool,
    /// Shell-by-shell reload: (start, per shell, finish) seconds (shotguns).
    shell_reload: Option<(f32, f32, f32)>,
}

impl ViewWeapon {
    fn plain(name: &'static str, view_model: &'static str, css_view_model: &'static str) -> Self {
        Self {
            name,
            view_model,
            css_view_model,
            reload: 0.0,
            knife: false,
            grenade: false,
            c4: false,
            css_right_handed: false,
            silencer_bit: 0,
            silencer_adjust: 0.0,
            mode_bit: 0,
            burst: false,
            dual: false,
            scope_overlay: false,
            shell_reload: None,
        }
    }

    fn gun(weapon: &'static CsWeapon, index: u8) -> Self {
        let bit = weapon_iw4::cs::silencer_bit(index);
        let silencer = weapon.silencer.as_ref();
        Self {
            reload: weapon.reload,
            silencer_bit: silencer.map_or(0, |_| bit),
            silencer_adjust: silencer.map_or(0.0, |s| s.adjust),
            mode_bit: bit,
            burst: weapon.burst.is_some(),
            dual: weapon.dual,
            scope_overlay: weapon.scope_overlay,
            shell_reload: weapon.shell_reload,
            ..Self::plain(weapon.name, weapon.view_model, weapon.css_view_model)
        }
    }

    fn grenade(grenade: &'static weapon_iw4::cs::CsGrenade) -> Self {
        Self {
            grenade: true,
            ..Self::plain(grenade.name, grenade.view_model, grenade.css_view_model)
        }
    }

    fn c4() -> Self {
        let c4 = &weapon_iw4::cs::CS_C4;
        Self {
            c4: true,
            // CS:S models the C4 right-handed (its keypad reads the right way round).
            css_right_handed: true,
            ..Self::plain(c4.name, c4.view_model, c4.css_view_model)
        }
    }

    fn knife() -> Self {
        let knife = &weapon_iw4::cs::CS_KNIFE;
        Self {
            knife: true,
            css_right_handed: true,
            ..Self::plain(knife.name, knife.view_model, knife.css_view_model)
        }
    }
}

struct LoadedModel {
    format: Format,
    studio: Studio,
    sequences: Vec<SequenceInfo>,
    gpu: Arc<CsViewmodelModel>,
    roles: Roles,
    /// Muzzle and shell-port attachments (bone, frame in its skinning space), CS:S models only.
    muzzle: Option<(usize, Mat3x4)>,
    eject: Option<(usize, Mat3x4)>,
    /// A GoldSrc model holding its weapon in the right hand already (not mirrored).
    goldsrc_right_handed: bool,
}

struct Playing {
    weapon: u32,
    sequence: Option<usize>,
    started: f64,
    /// Playback speed (reloads are stretched to the gun's reload time).
    rate: f32,
    /// Frame of `sequence` whose events have already played; `None` until it starts.
    events_through: Option<f32>,
    last_fire_ms: i32,
    reloading: bool,
    /// Whether the gun's silencer was on last frame.
    silenced: bool,
    /// `PlayerState::cs_grenade` last frame.
    grenade_state: u32,
    /// The C4 was being planted last frame.
    arming: bool,
    /// When the last shot's muzzle flash started, and its random spin.
    flash: Option<(f64, u32)>,
    /// The clip last frame (a shotgun reload plays a shell going in as it grows).
    clip: i32,
}

#[derive(Resource, Default)]
pub struct CsViewmodels {
    /// Looked up once, by the startup read: the CS:S pack (opened) and the GoldSrc folders
    /// (Condition Zero, then CS 1.6).
    css: Option<Option<Arc<mdl_source::Vpk>>>,
    goldsrc: Option<Option<asset_transport::GoldSrcDirs>>,
    /// The startup read (`warm_up`) while it runs off the main thread.
    warmup: Option<Task<Warmup>>,
    warmed_up: bool,
    models: HashMap<&'static str, Option<Arc<LoadedModel>>>,
    /// Viewmodels being read and decoded off the main thread, by weapon name.
    decoding: HashMap<&'static str, Task<Result<DecodedModel, String>>>,
    playing: Option<Playing>,
    bones: Vec<Mat3x4>,
    bob_time: f64,
    /// Horizontal speed the bob follows, eased.
    bob_speed: f32,
    /// World-space facing the gun trails toward (turn sway).
    lagged_forward: Option<[f32; 3]>,
    /// CS:S muzzle flash sprites (`sprites/muzzleflash4`, `effects/muzzleflashx`), looked up once.
    flash_images: Option<Option<[Handle<Image>; 2]>>,
}

static NEXT_MODEL_ID: AtomicU64 = AtomicU64::new(1);

/// Ids for CS models handed to the GPU (viewmodels and world models share them).
pub(super) fn next_model_id() -> &'static AtomicU64 {
    &NEXT_MODEL_ID
}

fn image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
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

fn pack_weights(weights: [f32; 3]) -> [u8; 4] {
    let w = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    [w(weights[0]), w(weights[1]), w(weights[2]), 0]
}

/// Whether a GoldSrc viewmodel holds its weapon in the right hand already: its weapon's meshes
/// (not the hands') sit right of the view at its first frame. CS 1.6's guns sit left and are
/// mirrored like CS:S's, but its knife sits right; Condition Zero's knife sits left.
fn modelled_right_handed(studio: &mdl_goldsrc::StudioModel) -> bool {
    const HANDS: [&str; 6] = ["view_", "hand", "glove", "finger", "arm", "sleeve"];
    let mut pose = Vec::new();
    studio.pose(0, 0.0, &mut pose);
    let (mut sum, mut count) = (0.0, 0usize);
    for mesh in &studio.meshes {
        let hands = studio.textures.get(mesh.texture).is_none_or(|texture| {
            let name = texture.name.to_ascii_lowercase();
            HANDS.iter().any(|part| name.contains(part))
        });
        if hands {
            continue;
        }
        for v in &studio.vertices[mesh.first_vertex..mesh.first_vertex + mesh.vertex_count] {
            let bone = pose
                .get(usize::from(v.bone))
                .unwrap_or(&mdl_goldsrc::IDENTITY);
            sum += mdl_goldsrc::transform_point(bone, v.position)[1];
            count += 1;
        }
    }
    count > 0 && sum < 0.0
}

fn goldsrc_model(studio: mdl_goldsrc::StudioModel, images: &mut Assets<Image>) -> LoadedModel {
    let handles: Vec<Handle<Image>> = studio
        .textures
        .iter()
        .map(|t| images.add(image(t.width, t.height, t.rgba.clone())))
        .collect();
    let vertices = studio
        .vertices
        .iter()
        .map(|v| CsViewmodelVertex {
            position: v.position,
            normal: v.normal,
            uv: v.uv,
            bones: [v.bone, 0, 0, 0],
            weights: [255, 0, 0, 0],
        })
        .collect();
    let draws = studio
        .meshes
        .iter()
        .map(|mesh| {
            let flags = studio.textures[mesh.texture].flags;
            CsViewmodelDraw {
                image: handles[mesh.texture].clone(),
                first_vertex: mesh.first_vertex as u32,
                vertex_count: mesh.vertex_count as u32,
                shading: if flags & mdl_goldsrc::TEXTURE_ADDITIVE != 0 {
                    CsViewmodelShading::Additive
                } else if flags & mdl_goldsrc::TEXTURE_FULLBRIGHT != 0 {
                    CsViewmodelShading::Fullbright
                } else {
                    CsViewmodelShading::Lit
                },
            }
        })
        .collect();
    let sequences = studio
        .sequences
        .iter()
        .map(|s| SequenceInfo {
            fps: s.fps,
            num_frames: s.num_frames,
            duration: s.duration(),
            sounds: s
                .events
                .iter()
                .filter(|e| e.event == mdl_goldsrc::EVENT_CLIENT_SOUND)
                .filter_map(|e| {
                    let name = e.options.to_ascii_lowercase();
                    let name = name.strip_prefix("weapons/")?.strip_suffix(".wav")?;
                    Some((
                        e.frame as f32,
                        format!("{}{name}", asset_audio::CS_SOUND_PREFIX),
                    ))
                })
                .collect(),
        })
        .collect();
    // The M4A1 and USP carry silenced and unsilenced sets; ours are unsilenced.
    let unsilenced = studio.sequences.iter().any(|s| s.label.ends_with("_unsil"));
    let pick = |base: &str| {
        let candidates = if unsilenced {
            vec![
                format!("{base}_unsil"),
                format!("{base}1_unsil"),
                base.to_owned(),
            ]
        } else {
            vec![base.to_owned(), format!("{base}1")]
        };
        candidates
            .iter()
            .find_map(|name| studio.sequence_index(name))
    };
    let labels: Vec<&str> = studio.sequences.iter().map(|s| s.label.as_str()).collect();
    let (slashes, stab, stab_miss) = knife_roles(&labels);
    let label = |name: &str| labels.iter().position(|l| l.eq_ignore_ascii_case(name));
    let (pullpin, throw) = (label("pullpin"), label("throw"));
    let arm = label("pressbutton");
    // With the `_unsil` set as the plain gun, the unmarked labels are the silenced one.
    let silenced = unsilenced.then(|| Silenced {
        idle: label("idle"),
        draw: label("draw"),
        reload: label("reload"),
        shoot: studio
            .sequences
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                let label = s.label.to_ascii_lowercase();
                label.starts_with("shoot")
                    && !label.contains("empty")
                    && !label.contains("last")
                    && !label.ends_with("_unsil")
            })
            .map(|(i, _)| i)
            .collect(),
        attach: label("add_silencer"),
        detach: label("detach_silencer"),
    });
    let mut roles = Roles {
        slashes,
        stab,
        stab_miss,
        pullpin,
        throw,
        arm,
        silenced,
        idle: pick("idle"),
        draw: pick("draw").or_else(|| pick("deploy")),
        reload: pick("reload").or_else(|| label("insert")),
        shoot_alt: Vec::new(),
        shell_start: label("start_reload"),
        shell_finish: label("after_reload"),
        shoot: studio
            .sequences
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                let label = s.label.to_ascii_lowercase();
                label.starts_with("shoot")
                    && !label.contains("empty")
                    && !label.contains("last")
                    && label.ends_with("_unsil") == unsilenced
            })
            .map(|(i, _)| i)
            .collect(),
    };
    // The Glock-18 fires `shoot3` a shot at a time and `shoot` in burst mode
    // (`EV_FireGlock18`); `shoot2` is unused.
    if studio.name.to_ascii_lowercase().contains("glock18")
        && let (Some(single), Some(burst)) = (label("shoot3"), label("shoot"))
    {
        roles.shoot = vec![single];
        roles.shoot_alt = vec![burst];
    }
    LoadedModel {
        format: Format::GoldSrc,
        gpu: Arc::new(CsViewmodelModel {
            id: NEXT_MODEL_ID.fetch_add(1, Ordering::Relaxed),
            vertices,
            draws,
        }),
        goldsrc_right_handed: modelled_right_handed(&studio),
        studio: Studio::GoldSrc(studio),
        sequences,
        roles,
        muzzle: None,
        eject: None,
    }
}

fn source_model(loaded: mdl_source::LoadedModel, images: &mut Assets<Image>) -> LoadedModel {
    let studio = loaded.model;
    let handles: Vec<Option<Handle<Image>>> = loaded
        .materials
        .iter()
        .map(|m| {
            m.texture.as_ref().map(|t| {
                let mut rgba = t.rgba.clone();
                // Only alpha-tested materials cut holes; others keep masks (envmap, phong) in alpha.
                if !m.alpha_test {
                    for px in rgba.chunks_exact_mut(4) {
                        px[3] = 255;
                    }
                }
                images.add(image(t.width, t.height, rgba))
            })
        })
        .collect();
    let vertices = studio
        .vertices
        .iter()
        .map(|v| CsViewmodelVertex {
            position: v.position,
            normal: v.normal,
            uv: v.uv,
            bones: [v.bones[0], v.bones[1], v.bones[2], 0],
            weights: pack_weights(v.weights),
        })
        .collect();
    let draws = studio
        .meshes
        .iter()
        .filter_map(|mesh| {
            let material = loaded.materials.get(mesh.material)?;
            Some(CsViewmodelDraw {
                image: handles.get(mesh.material)?.clone()?,
                first_vertex: mesh.first_vertex as u32,
                vertex_count: mesh.vertex_count as u32,
                shading: if material.additive {
                    CsViewmodelShading::Additive
                } else if material.fullbright {
                    CsViewmodelShading::Fullbright
                } else {
                    CsViewmodelShading::Lit
                },
            })
        })
        .collect();
    let sequences = studio
        .sequences
        .iter()
        .map(|s| SequenceInfo {
            fps: s.fps,
            num_frames: s.num_frames,
            duration: s.duration(),
            sounds: s
                .events
                .iter()
                .filter(|e| e.event == mdl_source::EVENT_CLIENT_SOUND)
                .map(|e| {
                    (
                        e.cycle * s.num_frames.saturating_sub(1) as f32,
                        format!(
                            "{}{}",
                            asset_audio::CSS_SOUND_PREFIX,
                            e.options.to_ascii_lowercase()
                        ),
                    )
                })
                .collect(),
        })
        .collect();
    // Activities name the roles; the unsilenced M4A1/USP use the plain ones.
    let labels: Vec<&str> = studio.sequences.iter().map(|s| s.label.as_str()).collect();
    let (slashes, stab, stab_miss) = knife_roles(&labels);
    let label = |name: &str| labels.iter().position(|l| l.eq_ignore_ascii_case(name));
    let (pullpin, throw) = (label("pullpin"), label("throw"));
    let arm = label("pressbutton");
    // The silenced set has its own activities; only the M4A1 and USP carry it.
    let attach = studio.sequence_for_activity("ACT_VM_ATTACH_SILENCER");
    let silenced = attach.map(|attach| Silenced {
        idle: studio.sequence_for_activity("ACT_VM_IDLE_SILENCED"),
        draw: studio.sequence_for_activity("ACT_VM_DRAW_SILENCED"),
        reload: studio.sequence_for_activity("ACT_VM_RELOAD_SILENCED"),
        shoot: studio
            .sequences_for_activity("ACT_VM_PRIMARYATTACK_SILENCED")
            .collect(),
        attach: Some(attach),
        detach: studio.sequence_for_activity("ACT_VM_DETACH_SILENCER"),
    });
    let roles = Roles {
        slashes,
        stab,
        stab_miss,
        pullpin,
        throw,
        arm,
        silenced,
        idle: studio.sequence_for_activity("ACT_VM_IDLE"),
        draw: studio.sequence_for_activity("ACT_VM_DRAW"),
        reload: studio.sequence_for_activity("ACT_VM_RELOAD"),
        shoot_alt: studio
            .sequences_for_activity("ACT_VM_SECONDARYATTACK")
            .collect(),
        shell_start: studio.sequence_for_activity("ACT_SHOTGUN_RELOAD_START"),
        shell_finish: studio.sequence_for_activity("ACT_SHOTGUN_RELOAD_FINISH"),
        shoot: studio
            .sequences_for_activity("ACT_VM_PRIMARYATTACK")
            .collect(),
    };
    let attachment = |a: &mdl_source::Attachment| (a.bone, a.in_bind);
    let muzzle = studio.muzzle().map(attachment);
    let eject = studio
        .attachments
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case("2"))
        .map(attachment);
    LoadedModel {
        format: Format::Source,
        gpu: Arc::new(CsViewmodelModel {
            id: NEXT_MODEL_ID.fetch_add(1, Ordering::Relaxed),
            vertices,
            draws,
        }),
        studio: Studio::Source(studio),
        sequences,
        roles,
        muzzle,
        eject,
        goldsrc_right_handed: false,
    }
}

/// A viewmodel read and decoded off the main thread; its textures still go into `Assets`.
enum DecodedModel {
    Source(mdl_source::LoadedModel),
    GoldSrc(mdl_goldsrc::StudioModel),
}

/// Reads and decodes a viewmodel: from the CS:S pack when there is one, else the GoldSrc folders
/// (Condition Zero's model when it has one, else CS 1.6's).
fn decode_model(
    css: Option<&mdl_source::Vpk>,
    goldsrc: Option<&asset_transport::GoldSrcDirs>,
    view_model: &str,
    css_view_model: &str,
) -> Result<DecodedModel, String> {
    if let Some(vpk) = css {
        return mdl_source::load_model(vpk, &format!("models/weapons/{css_view_model}.mdl"))
            .map(DecodedModel::Source)
            .map_err(|e| format!("{css_view_model}: {e}"));
    }
    let Some(dirs) = goldsrc else {
        return Err(format!(
            "no Counter-Strike: Source install found ({}) and no Condition Zero or \
             Counter-Strike 1.6 folder selected (Options > Game Folders)",
            asset_transport::CSS_ENV,
        ));
    };
    let relative = format!("models/{view_model}.mdl");
    let path = dirs
        .file(&relative)
        .ok_or_else(|| format!("{relative}: in none of {:?}", dirs.0))?;
    std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| mdl_goldsrc::StudioModel::parse(&bytes).map_err(|e| e.to_string()))
        .map(DecodedModel::GoldSrc)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Adds a decoded viewmodel's textures and builds what the viewmodel pass draws; `None` (the
/// MW2 model stays) when it could not be read.
fn finish_model(
    name: &str,
    decoded: Result<DecodedModel, String>,
    images: &mut Assets<Image>,
) -> Option<Arc<LoadedModel>> {
    let model = match decoded {
        Ok(DecodedModel::Source(loaded)) => source_model(loaded, images),
        Ok(DecodedModel::GoldSrc(studio)) => goldsrc_model(studio, images),
        Err(error) => {
            diag::warn!(World, "cs viewmodel: {error}; MW2 model stays");
            return None;
        }
    };
    diag::info!(
        World,
        "cs viewmodel {name} ({:?}): {} triangles, {} sequences",
        model.format,
        model.gpu.vertices.len() / 3,
        model.sequences.len()
    );
    Some(Arc::new(model))
}

/// What the startup read brings back from off the main thread.
struct Warmup {
    css: Option<Arc<mdl_source::Vpk>>,
    goldsrc: Option<asset_transport::GoldSrcDirs>,
    models: Vec<(&'static str, Result<DecodedModel, String>)>,
    flashes: Option<[mdl_source::vtf::Image; 2]>,
}

/// Opens the CS:S pack (or finds the Condition Zero / CS 1.6 folders), and decodes the viewmodels every player
/// holds at spawn (knife, starting pistols, C4, grenades) and the muzzle flash sprites. Read on
/// first use instead, each stopped the main thread for 30 ms, and the pack's first read from a
/// cold disk for most of a second.
fn warm_up() -> Warmup {
    use weapon_iw4::cs;
    let css = asset_transport::find_css_pak().and_then(|pak| match mdl_source::Vpk::open(&pak) {
        Ok(vpk) => {
            diag::info!(
                World,
                "cs viewmodels: Counter-Strike: Source from {}",
                pak.display()
            );
            Some(Arc::new(vpk))
        }
        Err(error) => {
            diag::warn!(World, "cs viewmodels: {error}");
            None
        }
    });
    let goldsrc = if css.is_some() {
        None
    } else {
        asset_transport::find_goldsrc()
    };
    if let Some(dirs) = &goldsrc {
        diag::info!(World, "cs viewmodels: GoldSrc from {:?}", dirs.0);
    }
    let mut models = Vec::new();
    if css.is_some() || goldsrc.is_some() {
        let pistols = cs::CS_WEAPONS
            .iter()
            .filter(|w| matches!(w.name, "glock" | "usp"))
            .map(|w| (w.name, w.view_model, w.css_view_model));
        let grenades = cs::CS_GRENADES
            .iter()
            .map(|g| (g.name, g.view_model, g.css_view_model));
        let (knife, c4) = (&cs::CS_KNIFE, &cs::CS_C4);
        let spawn_set = [
            (knife.name, knife.view_model, knife.css_view_model),
            (c4.name, c4.view_model, c4.css_view_model),
        ]
        .into_iter()
        .chain(pistols)
        .chain(grenades);
        for (name, view_model, css_view_model) in spawn_set {
            let decoded = decode_model(
                css.as_deref(),
                goldsrc.as_ref(),
                view_model,
                css_view_model,
            );
            models.push((name, decoded));
        }
    }
    let flashes = css.as_deref().and_then(|pack| {
        let load = |path: &str| mdl_source::vtf::decode(&pack.read(path)?).ok();
        Some([
            load("materials/sprites/muzzleflash4.vtf")?,
            load("materials/effects/muzzleflashx.vtf")?,
        ])
    });
    Warmup {
        css,
        goldsrc,
        models,
        flashes,
    }
}

/// Where a weapon's viewmodel stands.
enum ModelLoad {
    Ready(Arc<LoadedModel>),
    /// Being read off the main thread; the CS model owns the view meanwhile and nothing draws.
    Loading,
    /// Not readable: the MW2 model stays.
    Unavailable,
}

impl CsViewmodels {
    /// Starts the startup read on the first call, and lands it and every viewmodel decoded since.
    fn pump(&mut self, images: &mut Assets<Image>) {
        if !self.warmed_up && self.warmup.is_none() {
            self.warmup = Some(AsyncComputeTaskPool::get().spawn(async { warm_up() }));
        }
        if self.warmup.as_ref().is_some_and(Task::is_finished)
            && let Some(task) = self.warmup.take()
        {
            let warm = block_on(task);
            self.warmed_up = true;
            self.css = Some(warm.css);
            self.goldsrc = Some(warm.goldsrc);
            self.flash_images = Some(warm.flashes.map(|sprites| {
                sprites.map(|sprite| images.add(image(sprite.width, sprite.height, sprite.rgba)))
            }));
            for (name, decoded) in warm.models {
                let model = finish_model(name, decoded, images);
                self.models.insert(name, model);
            }
        }
        let finished: Vec<&'static str> = self
            .decoding
            .iter()
            .filter(|(_, task)| task.is_finished())
            .map(|(name, _)| *name)
            .collect();
        for name in finished {
            if let Some(task) = self.decoding.remove(name) {
                let model = finish_model(name, block_on(task), images);
                self.models.insert(name, model);
            }
        }
    }

    /// The CS:S flash sprites as additive textures, once the startup read has them.
    fn flash_images(&self) -> Option<[Handle<Image>; 2]> {
        self.flash_images.clone().flatten()
    }

    /// `weapon`'s viewmodel. One not read yet is decoded off the main thread, loading meanwhile.
    fn model_for(&mut self, weapon: ViewWeapon) -> ModelLoad {
        if let Some(model) = self.models.get(weapon.name) {
            return model
                .clone()
                .map_or(ModelLoad::Unavailable, ModelLoad::Ready);
        }
        if self.warmed_up && !self.decoding.contains_key(weapon.name) {
            let css = self.css.clone().flatten();
            let goldsrc = self.goldsrc.clone().flatten();
            let (view_model, css_view_model) = (weapon.view_model, weapon.css_view_model);
            let task = AsyncComputeTaskPool::get().spawn(async move {
                decode_model(css.as_deref(), goldsrc.as_ref(), view_model, css_view_model)
            });
            self.decoding.insert(weapon.name, task);
        }
        ModelLoad::Loading
    }
}

/// Whether the CS gun `ps` holds draws the sniper scope (and hides itself) when zoomed.
pub(super) fn held_gun_scope_overlay(ps: &PlayerState, weapons: &PreparedWeapons) -> bool {
    held_cs_weapon(ps, weapons).is_some_and(|(_, weapon)| weapon.scope_overlay)
}

/// The CS gun or knife `ps` holds, if its viewmodel weapon is one.
fn held_cs_weapon(ps: &PlayerState, weapons: &PreparedWeapons) -> Option<(u32, ViewWeapon)> {
    let viewmodel = weapon_iw4::get_viewmodel_weapon_index(ps);
    if viewmodel == 0 {
        return None;
    }
    let index = weapon_iw4::cs::cs_weapon_index_for(&weapons.0.script_name_of(viewmodel))?;
    if let Some(grenade) = weapon_iw4::cs::cs_grenade(index) {
        return Some((viewmodel, ViewWeapon::grenade(grenade)));
    }
    if weapon_iw4::cs::is_knife(index) {
        return Some((viewmodel, ViewWeapon::knife()));
    }
    if weapon_iw4::cs::is_c4(index) {
        return Some((viewmodel, ViewWeapon::c4()));
    }
    Some((
        viewmodel,
        ViewWeapon::gun(weapon_iw4::cs::cs_weapon(index)?, index),
    ))
}

/// The knife sound for the attack `ps.cs_knife` records: a swish when it met nothing, else
/// flesh or wall. Aliases without the `/plr` suffix.
fn knife_attack_sound(format: Format, knife: u32) -> &'static str {
    use playerstate_iw4::cs_knife;
    let stab = matches!(
        knife & cs_knife::ANIM_MASK,
        cs_knife::ANIM_STAB | cs_knife::ANIM_STAB_MISS
    );
    match (
        (knife & cs_knife::HIT_MASK) >> cs_knife::HIT_SHIFT,
        stab,
        format,
    ) {
        (cs_knife::HIT_PLAYER, true, Format::Source) => "css/weapon_knife.stab",
        (cs_knife::HIT_PLAYER, false, Format::Source) => "css/weapon_knife.hit",
        (cs_knife::HIT_WORLD, _, Format::Source) => "css/weapon_knife.hitwall",
        (_, _, Format::Source) => "css/weapon_knife.slash",
        (cs_knife::HIT_PLAYER, true, Format::GoldSrc) => "cs/weapons/knife_stab",
        (cs_knife::HIT_PLAYER, false, Format::GoldSrc) => "cs/weapons/knife_hit1",
        (cs_knife::HIT_WORLD, _, Format::GoldSrc) => "cs/weapons/knife_hitwall1",
        (_, _, Format::GoldSrc) => "cs/weapons/knife_slash1",
    }
}

fn knife_deploy_sound(format: Format) -> &'static str {
    match format {
        Format::Source => "css/weapon_knife.deploy",
        Format::GoldSrc => "cs/weapons/knife_deploy1",
    }
}

/// CS's zoom click when right click steps the scope (and when a reload drops it), not when a
/// shot drops it for the bolt or the bolt brings it back.
pub fn cs_zoom_sound(
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    state: Res<CsViewmodels>,
    mut last: Local<Option<(u32, u32)>>,
    mut sounds: MessageWriter<audio::AliasCommand>,
) {
    let Some(ps) = presented.viewweapon_player(local.0) else {
        *last = None;
        return;
    };
    let now = (ps.cs_zoom, ps.cs_last_zoom);
    if let Some((zoom, resume)) = *last
        && zoom != now.0
        && resume == 0
        && now.1 == 0
    {
        let alias = if matches!(state.css, Some(Some(_))) {
            "css/default.zoom"
        } else {
            "cs/weapons/zoom"
        };
        play_local(&mut sounds, alias);
    }
    *last = Some(now);
}

fn play_local(sounds: &mut MessageWriter<audio::AliasCommand>, alias: &str) {
    play_alias(sounds, alias, None);
}

/// `alias`, or `fallback` when the bank has no `alias` (the other install's sound).
fn play_local_or(sounds: &mut MessageWriter<audio::AliasCommand>, alias: &str, fallback: &str) {
    play_alias(sounds, alias, Some(fallback));
}

fn play_alias(
    sounds: &mut MessageWriter<audio::AliasCommand>,
    alias: &str,
    fallback: Option<&str>,
) {
    sounds.write(audio::AliasCommand::Play(audio::PlayAlias {
        event: None,
        namespace: asset_core::AssetNamespace::Iw4,
        alias: format!("{alias}{}", asset_audio::CS_SOUND_PLAYER_SUFFIX),
        fallback: fallback.map(|f| format!("{f}{}", asset_audio::CS_SOUND_PLAYER_SUFFIX)),
        origin_inches: None,
        snd_ent: Some(audio::SND_ENT_LOCAL),
    }));
}

/// Pitch (positive looks down), yaw and roll in degrees as a rotation in GoldSrc view axes.
fn view_rotation(pitch: f32, yaw: f32, roll: f32) -> Mat3x4 {
    let (sp, cp) = pitch.to_radians().sin_cos();
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sr, cr) = roll.to_radians().sin_cos();
    let yaw_m: Mat3x4 = [
        [cy, -sy, 0.0, 0.0],
        [sy, cy, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ];
    let pitch_m: Mat3x4 = [
        [cp, 0.0, sp, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [-sp, 0.0, cp, 0.0],
    ];
    let roll_m: Mat3x4 = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, cr, -sr, 0.0],
        [0.0, sr, cr, 0.0],
    ];
    mdl_goldsrc::concat(&mdl_goldsrc::concat(&yaw_m, &pitch_m), &roll_m)
}

/// A bob value for the phase `cycle` (0..1): rises for `BOB_UP` of it, falls for the rest,
/// scaled by speed and biased upward.
fn bob_wave(cycle: f64, amplitude: f32) -> f32 {
    let phase = if cycle < BOB_UP {
        core::f64::consts::PI * cycle / BOB_UP
    } else {
        core::f64::consts::PI + core::f64::consts::PI * (cycle - BOB_UP) / (1.0 - BOB_UP)
    };
    (amplitude * 0.3 + amplitude * 0.7 * (phase as f32).sin()).clamp(-7.0, 4.0)
}

/// Walk bob and turn sway as one view-space placement of the gun: small offsets (about a unit)
/// and tilts (well under a degree), so the gun never swings far enough to show its back.
fn sway_placement(state: &mut CsViewmodels, ps: &PlayerState, dt: f32) -> Mat3x4 {
    if !viewmodel_sway() {
        state.lagged_forward = None;
        return mdl_goldsrc::IDENTITY;
    }
    let view = [
        ps.viewangles[0] - ps.cs_punch[0],
        ps.viewangles[1] - ps.cs_punch[1],
        0.0,
    ];
    let (forward, right, up) = math_iw4::angle_vectors(view);
    let left = right.map(|v| -v);
    let to_view = |w: [f32; 3]| {
        [
            w[0] * forward[0] + w[1] * forward[1] + w[2] * forward[2],
            w[0] * left[0] + w[1] * left[1] + w[2] * left[2],
            w[0] * up[0] + w[1] * up[1] + w[2] * up[2],
        ]
    };
    // CS bobs by horizontal speed in the air too: gating it on the ground snapped the gun on
    // every bhop landing.
    let target = ps.velocity[0].hypot(ps.velocity[1]).min(BOB_FULL_SPEED);
    state.bob_speed += (target - state.bob_speed) * (BOB_SPEED_EASE * dt).min(1.0);
    let speed = state.bob_speed;
    state.bob_time += f64::from(dt * speed / BOB_FULL_SPEED);
    let amplitude = speed * 0.005;
    let vertical = bob_wave((state.bob_time % BOB_CYCLE) / BOB_CYCLE, amplitude);
    let lateral = bob_wave(
        (state.bob_time % (BOB_CYCLE * 2.0)) / (BOB_CYCLE * 2.0),
        amplitude,
    );

    let world_up = to_view([0.0, 0.0, 1.0]);
    let mut offset: [f32; 3] = core::array::from_fn(|i| world_up[i] * vertical * 0.1);
    offset[0] += vertical * 0.1;
    offset[1] -= lateral * 0.8;

    let lagged = state.lagged_forward.get_or_insert(forward);
    let mut diff: [f32; 3] = core::array::from_fn(|i| forward[i] - lagged[i]);
    let lag = diff.iter().map(|v| v * v).sum::<f32>().sqrt();
    let catch_up = if lag > SWAY_MAX_LAG {
        SWAY_CATCH_UP * lag / SWAY_MAX_LAG
    } else {
        SWAY_CATCH_UP
    };
    let step = (catch_up * dt).min(1.0);
    for i in 0..3 {
        lagged[i] += diff[i] * step;
    }
    let len = lagged.iter().map(|v| v * v).sum::<f32>().sqrt();
    if len > 0.0 {
        for v in lagged.iter_mut() {
            *v /= len;
        }
    }
    diff = core::array::from_fn(|i| forward[i] - lagged[i]);
    let drift = to_view(diff);
    for i in 0..3 {
        offset[i] -= drift[i] * SWAY_UNITS;
    }
    let mut place = view_rotation(-vertical * 0.4, -lateral * 0.3, vertical * 0.5);
    for (row, value) in place.iter_mut().zip(offset) {
        row[3] = value;
    }
    place
}

/// Rotation that undoes the view's recoil punch (pitch, yaw degrees) in GoldSrc view axes.
fn unpunch(punch: [f32; 3]) -> Mat3x4 {
    view_rotation(-punch[0], -punch[1], 0.0)
}

#[allow(clippy::too_many_arguments)]
pub fn update_cs_viewmodel(
    time: Res<Time>,
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    weapons: Option<Res<PreparedWeapons>>,
    view_settings: (Res<ViewSubject>, Res<frame::GameSettings>),
    mut images: ResMut<Assets<Image>>,
    mut state: ResMut<CsViewmodels>,
    mut frame: ResMut<CsViewmodelFrame>,
    mut active: ResMut<CsViewmodelActive>,
    mut sounds: MessageWriter<audio::AliasCommand>,
) {
    let (view, settings) = view_settings;
    frame.model = None;
    active.0 = false;
    state.pump(&mut images);
    let Some(weapons) = weapons else {
        return;
    };
    let Some(ps) = presented.viewweapon_player(local.0) else {
        state.playing = None;
        state.lagged_forward = None;
        return;
    };
    let Some((weapon_index, weapon)) = held_cs_weapon(ps, &weapons) else {
        state.playing = None;
        return;
    };
    let model = match state.model_for(weapon) {
        ModelLoad::Ready(model) => model,
        ModelLoad::Loading => {
            active.0 = true;
            return;
        }
        ModelLoad::Unavailable => return,
    };
    // From here the CS model owns the first-person view, even when it is hidden this frame.
    active.0 = true;
    if ps.pm_type >= playerstate_iw4::PM_TYPE_DEAD
        || presented_is_third_person(&presented, local.0, view.in_killcam(), settings.third_person)
        // CS hides the viewmodel while scoped.
        || (ps.cs_zoom != 0 && weapon.scope_overlay)
    {
        return;
    }

    let now = time.elapsed_secs_f64();
    let reloading = weapon_iw4::WeaponState::from_i32(ps.weaponstate_primary)
        .is_ok_and(weapon_iw4::WeaponState::is_reload_family);
    let clip = weapons.0.facts_of(weapon_index).map_or(0, |facts| {
        let key = weapon_iw4::clip_table_key(facts.clip_index, weapon_index);
        if weapon_iw4::clip_row_present(&ps.ammoclip, key) {
            weapon_iw4::get_clip_for_hand(&ps.ammoclip, key, 0)
        } else {
            0
        }
    });
    let roles = &model.roles;
    // A gun with a silencer plays its silenced or plain set by the replicated bit.
    let silenced = weapon.silencer_bit != 0 && ps.cs_silencers & weapon.silencer_bit != 0;
    let (idle, draw, gun_reload, shoot) = roles.gun(silenced);
    let start = |playing: &mut Playing, sequence: Option<usize>, rate: f32| {
        playing.sequence = sequence;
        playing.started = now;
        playing.rate = rate;
        playing.events_through = None;
    };
    let playing = match state.playing.as_mut() {
        Some(playing) if playing.weapon == weapon_index => {
            // Planting holds the player in place (the site links them to it): the C4's keypad
            // plays over the 3 s it takes, and stops if they let go.
            let arming = weapon.c4 && ps.pm_type == playerstate_iw4::PM_TYPE_NORMAL_LINKED;
            if arming && !playing.arming {
                let rate = roles
                    .arm
                    .map(|i| model.sequences[i].duration / weapon_iw4::cs::CS_C4_ARMING_SECONDS)
                    .filter(|rate| *rate > 0.0)
                    .unwrap_or(1.0);
                start(playing, roles.arm.or(idle), rate);
            } else if !arming && playing.arming {
                start(playing, idle, 1.0);
            }
            playing.arming = arming;
            if weapon.grenade {
                use playerstate_iw4::cs_grenade::{IDLE, PULLED, THROWN};
                let sequence = match (playing.grenade_state, ps.cs_grenade) {
                    (from, PULLED) if from != PULLED => Some(roles.pullpin),
                    (from, THROWN) if from != THROWN => Some(roles.throw),
                    // The next grenade comes up.
                    (THROWN, IDLE) => Some(draw),
                    _ => None,
                };
                if let Some(sequence) = sequence {
                    start(playing, sequence, 1.0);
                }
                // CS radios every grenade throw (`Radio("%!MRAD_FIREINHOLE")`).
                if playing.grenade_state != THROWN && ps.cs_grenade == THROWN {
                    play_local_or(&mut sounds, FIRE_IN_THE_HOLE, FIRE_IN_THE_HOLE_GOLDSRC);
                }
                playing.grenade_state = ps.cs_grenade;
            } else if silenced != playing.silenced
                && let Some(silencer) = &roles.silenced
            {
                // The silencer goes on or off: its animation takes as long as the sim's lockout.
                let sequence = if silenced {
                    silencer.attach
                } else {
                    silencer.detach
                };
                let rate = sequence
                    .map(|i| model.sequences[i].duration)
                    .filter(|duration| *duration > 0.0 && weapon.silencer_adjust > 0.0)
                    .map_or(1.0, |duration| duration / weapon.silencer_adjust);
                start(playing, sequence.or(idle), rate);
            } else if ps.cs_last_fire_ms != playing.last_fire_ms
                && ps.cs_last_fire_ms != 0
                && weapon.knife
            {
                use playerstate_iw4::cs_knife;
                let sequence = match ps.cs_knife & cs_knife::ANIM_MASK {
                    cs_knife::ANIM_SLASH1 => roles.slashes.first().copied(),
                    cs_knife::ANIM_SLASH2 => roles.slashes.last().copied(),
                    cs_knife::ANIM_STAB => roles.stab,
                    _ => roles.stab_miss.or(roles.stab),
                };
                if sequence.is_some() {
                    start(playing, sequence, 1.0);
                }
                play_local(&mut sounds, knife_attack_sound(model.format, ps.cs_knife));
            } else if ps.cs_last_fire_ms != playing.last_fire_ms && ps.cs_last_fire_ms != 0 {
                // A silenced shot has no muzzle flash.
                if !silenced {
                    playing.flash = Some((now, ps.cs_last_fire_ms.unsigned_abs()));
                }
                // The Glock in burst mode and the Elites' right gun shoot with the other set.
                let alt = (weapon.burst || weapon.dual)
                    && weapon.mode_bit != 0
                    && ps.cs_burst_modes & weapon.mode_bit != 0
                    && !roles.shoot_alt.is_empty();
                let set = if alt {
                    roles.shoot_alt.as_slice()
                } else {
                    shoot
                };
                if !set.is_empty() {
                    let pick = (ps.cs_last_fire_ms.unsigned_abs() / 7) as usize % set.len();
                    start(playing, Some(set[pick]), 1.0);
                }
            } else if reloading
                && !playing.reloading
                && let Some((start_time, _, _)) = weapon.shell_reload
                && let Some(begin) = roles.shell_start
            {
                // A shotgun's reload starts by opening; a shell goes in each time the clip grows.
                let duration = model.sequences[begin].duration;
                let rate = if start_time > 0.0 && duration > 0.0 {
                    duration / start_time
                } else {
                    1.0
                };
                start(playing, Some(begin), rate);
            } else if reloading
                && playing.reloading
                && clip > playing.clip
                && let Some((_, per_shell, _)) = weapon.shell_reload
                && let Some(insert) = gun_reload
            {
                let duration = model.sequences[insert].duration;
                let rate = if per_shell > 0.0 && duration > 0.0 {
                    duration / per_shell
                } else {
                    1.0
                };
                start(playing, Some(insert), rate);
            } else if !reloading
                && playing.reloading
                && let Some((_, _, finish_time)) = weapon.shell_reload
                && let Some(finish) = roles.shell_finish
            {
                let duration = model.sequences[finish].duration;
                let rate = if finish_time > 0.0 && duration > 0.0 {
                    duration / finish_time
                } else {
                    1.0
                };
                start(playing, Some(finish), rate);
            } else if reloading
                && !playing.reloading
                && let Some(reload) = gun_reload
            {
                // The reload animation ends when the gun is ready again.
                let duration = model.sequences[reload].duration;
                let rate = if weapon.reload > 0.0 && duration > 0.0 {
                    duration / weapon.reload
                } else {
                    1.0
                };
                start(playing, Some(reload), rate);
            }
            playing.last_fire_ms = ps.cs_last_fire_ms;
            playing.reloading = reloading;
            playing.clip = clip;
            playing.silenced = silenced;
            playing
        }
        _ => state.playing.insert({
            if weapon.knife {
                play_local(&mut sounds, knife_deploy_sound(model.format));
            }
            Playing {
                weapon: weapon_index,
                sequence: draw.or(idle),
                started: now,
                rate: 1.0,
                events_through: None,
                last_fire_ms: ps.cs_last_fire_ms,
                reloading,
                silenced,
                grenade_state: ps.cs_grenade,
                arming: false,
                flash: None,
                clip,
            }
        }),
    };
    let flash = playing
        .flash
        .filter(|(started, _)| now - started < FLASH_SECONDS)
        .map(|(started, seed)| (((now - started) / FLASH_SECONDS) as f32, seed));
    // A finished one-shot sequence settles into idle; a grenade holds its pulled pin or empty
    // hand until the next step.
    let holding = (weapon.grenade && ps.cs_grenade != playerstate_iw4::cs_grenade::IDLE)
        || (weapon.c4 && playing.arming);
    if let Some(sequence) = playing.sequence
        && Some(sequence) != idle
        && !holding
        && (now - playing.started) as f32 * playing.rate > model.sequences[sequence].duration
    {
        start(playing, idle, 1.0);
    }
    let Some(sequence) = playing.sequence else {
        return;
    };
    let info = &model.sequences[sequence];
    let mut seconds = (now - playing.started) as f32 * playing.rate;
    if Some(sequence) == idle && info.duration > 0.0 {
        seconds %= info.duration;
    } else if holding {
        seconds = seconds.min(info.duration);
    }
    // Sounds the animation calls for (reload clips, bolt pulls) as it passes their frames.
    let anim_frame = (seconds * info.fps).min(info.num_frames.saturating_sub(1) as f32);
    let after = playing.events_through.unwrap_or(-1.0);
    if anim_frame < after {
        playing.events_through = None;
    } else {
        for (_, alias) in info
            .sounds
            .iter()
            .filter(|(f, _)| *f > after && *f <= anim_frame)
        {
            play_local(&mut sounds, alias);
        }
        playing.events_through = Some(anim_frame);
    }

    let sway = sway_placement(&mut state, ps, time.delta_secs());
    let CsViewmodels { bones, .. } = &mut *state;
    model.studio.pose(sequence, seconds, bones);
    let mut place = mdl_goldsrc::concat(&sway, &unpunch(ps.cs_punch));
    let right_handed = match model.format {
        Format::Source => weapon.css_right_handed,
        Format::GoldSrc => model.goldsrc_right_handed,
    };
    // `cl_righthand`: mirrored when the model is built for the other hand.
    if right_handed != settings.right_hand {
        place = mdl_goldsrc::concat(&place, &RIGHT_HAND_MIRROR);
    }
    frame.bones.clear();
    frame
        .bones
        .extend(bones.iter().map(|bone| mdl_goldsrc::concat(&place, bone)));
    frame.model = Some(Arc::clone(&model.gpu));
    let (tan_half_fov_y, texture_gamma) = match model.format {
        Format::GoldSrc => (GOLDSRC_TAN_HALF_FOV_Y, 0.8),
        // Source's viewmodel fov is horizontal at 4:3 (widened for wider screens like ours).
        Format::Source => (
            (settings.viewmodel_fov * 0.5).to_radians().tan() * 0.75,
            1.0,
        ),
    };
    frame.tan_half_fov_y = tan_half_fov_y;
    frame.texture_gamma = texture_gamma;
    // From above, on the viewer's side: the faces the player sees are the lit ones.
    let light: [f32; 3] = [-0.6, 0.3, 0.75];
    let len = light.iter().map(|v| v * v).sum::<f32>().sqrt();
    frame.light_dir = light.map(|v| v / len);
    frame.ambient = [AMBIENT; 3];
    frame.shade = [SHADE; 3];
    frame.sun = [0.0; 3];

    frame.flashes.clear();
    if let (Some((age, seed)), Some((muzzle_bone, muzzle))) = (flash, model.muzzle)
        && frame.bones.len() < render_gpu::CS_VIEWMODEL_MAX_BONES
        && let Some(sprites) = state.flash_images()
    {
        let at = |bone: usize, local: &Mat3x4| {
            frame
                .bones
                .get(bone)
                .map(|m| mdl_goldsrc::concat(m, local))
                .map(|m| [m[0][3], m[1][3], m[2][3]])
        };
        if let Some(tip) = at(muzzle_bone, &muzzle) {
            let back = model.eject.and_then(|(bone, local)| at(bone, &local));
            let barrel = muzzle_flash_barrel(tip, back);
            let identity = frame.bones.len() as u8;
            frame.bones.push(mdl_goldsrc::IDENTITY);
            let (round, side) = muzzle_flash_quads(weapon.name, tip, barrel, age, seed, identity);
            frame.flashes.push(CsViewmodelFlash {
                image: sprites[0].clone(),
                vertices: round,
            });
            frame.flashes.push(CsViewmodelFlash {
                image: sprites[1].clone(),
                vertices: side,
            });
        }
    }
}

/// The barrel's direction in view space: from the shell port to the muzzle, else straight ahead.
fn muzzle_flash_barrel(tip: [f32; 3], back: Option<[f32; 3]>) -> [f32; 3] {
    let dir = back.map_or([1.0, 0.0, 0.0], |b| core::array::from_fn(|i| tip[i] - b[i]));
    let len = dir.iter().map(|v| v * v).sum::<f32>().sqrt();
    if len > 0.01 {
        dir.map(|v| v / len)
    } else {
        [1.0, 0.0, 0.0]
    }
}

/// A CS:S-style muzzle flash at `tip` (view space, GoldSrc axes: x forward, y left, z up): a round
/// flash facing the camera with a random spin, and two crossed flash cones along `barrel`.
/// `age` runs 0..1 over the flash; the tint fades with it.
fn muzzle_flash_quads(
    weapon: &str,
    tip: [f32; 3],
    barrel: [f32; 3],
    age: f32,
    seed: u32,
    bone: u8,
) -> (Vec<CsViewmodelVertex>, Vec<CsViewmodelVertex>) {
    // Half-size of the round flash, cone length and cone half-width, in model units.
    let (size, length, width) = match weapon {
        "awp" => (5.0, 16.0, 4.0),
        "deagle" => (4.0, 11.0, 3.0),
        "usp" | "glock" => (3.0, 8.0, 2.2),
        _ => (4.0, 13.0, 3.2),
    };
    let fade = 1.0 - age.clamp(0.0, 1.0) * 0.6;
    let tint = FLASH_TINT.map(|c| c * fade);
    let vertex = |p: [f32; 3], uv: [f32; 2]| CsViewmodelVertex {
        position: p,
        normal: tint,
        uv,
        bones: [bone, 0, 0, 0],
        weights: [255, 0, 0, 0],
    };
    let quad = |corners: [[f32; 3]; 4], out: &mut Vec<CsViewmodelVertex>| {
        let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for i in [0, 1, 2, 0, 2, 3] {
            out.push(vertex(corners[i], uv[i]));
        }
    };
    let add = |a: [f32; 3], b: [f32; 3], s: f32| -> [f32; 3] {
        core::array::from_fn(|i| a[i] + b[i] * s)
    };

    // Round flash in the view plane (y, z), spun by the shot.
    let spin = (seed.wrapping_mul(2_654_435_761) >> 8) as f32 / (1u32 << 24) as f32
        * core::f32::consts::TAU;
    let (s, c) = spin.sin_cos();
    let u = [0.0, c * size, s * size];
    let v = [0.0, -s * size, c * size];
    let mut round = Vec::with_capacity(6);
    quad(
        [
            add(add(tip, u, -1.0), v, -1.0),
            add(add(tip, u, 1.0), v, -1.0),
            add(add(tip, u, 1.0), v, 1.0),
            add(add(tip, u, -1.0), v, 1.0),
        ],
        &mut round,
    );

    // Two crossed cones along the barrel, starting at the tip.
    let up = if barrel[2].abs() > 0.9 {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let norm = |a: [f32; 3]| {
        let len = a.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        a.map(|x| x / len)
    };
    let side_a = norm(cross(barrel, up));
    let side_b = norm(cross(barrel, side_a));
    let end = add(tip, barrel, length);
    let mut side = Vec::with_capacity(12);
    for across in [side_a, side_b] {
        quad(
            [
                add(tip, across, -width),
                add(end, across, -width),
                add(end, across, width),
                add(tip, across, width),
            ],
            &mut side,
        );
    }
    (round, side)
}
