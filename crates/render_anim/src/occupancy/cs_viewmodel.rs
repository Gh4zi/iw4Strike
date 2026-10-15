//! Counter-Strike first-person models for the CS guns, read at runtime from the local installs
//! and drawn by `render_gpu`'s viewmodel pass in place of the MW2 viewmodel: Counter-Strike 2's
//! guns (with CS2's default arms and the clips of each gun's viewmodel graph) when CS2 is
//! installed, then CS:GO's (its `v_` models with the team's arms bone-merged on), then
//! Counter-Strike: Source models (`v_rif_ak47.mdl`, … from its pack) for the rest when CS:S is,
//! else CS 1.6 ones.
//! Animation follows CS: draw on switching to the gun, a shoot sequence per shot, reload when a
//! reload starts (timed to the gun's reload), idle otherwise; the sounds the animations call for
//! play as they pass their frames. The view's recoil punch moves the camera, not the gun, so the
//! gun is drawn without it. Walking bobs it gently and turning leaves it trailing slightly
//! behind; `set_viewmodel_sway(false)` keeps it still.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

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
    CsAimsightFrame, CsViewmodelDraw, CsViewmodelFlash, CsViewmodelFrame, CsViewmodelMaps,
    CsViewmodelModel, CsViewmodelShading, CsViewmodelVertex,
};
use weapon_iw4::cs::CsWeapon;

use crate::occupancy::cs2_assets::{self, Cs2Decoded};
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
/// CS:GO's default `viewmodel_offset_x/y/z` (1 right, 1 forward, 1 down), in view space
/// (x forward, y left, z up).
const CSGO_VIEWMODEL_OFFSET: Mat3x4 = [
    [1.0, 0.0, 0.0, 1.0],
    [0.0, 1.0, 0.0, -1.0],
    [0.0, 0.0, 1.0, -1.0],
];
/// CS:GO's default `viewmodel_fov` (horizontal degrees at 4:3): its guns are drawn as CS:GO draws
/// them by default, whatever `viewmodel_fov` (tuned for CS2's and CS:S's) says.
const CSGO_VIEWMODEL_FOV: f32 = 60.0;
/// CS's view `fov` unzoomed (`default_fov`). Source narrows the viewmodel's fov by as much as a
/// zoom narrows the view's (`fovViewmodel = viewmodel_fov - (default_fov - fov)`), so a gun aimed
/// down its scope at 45 is drawn at 15.
const CS_DEFAULT_FOV: f32 = 90.0;
/// A side's CS:GO look in first person: whether it plays the Terrorists (their knife and guns)
/// and the arms of the agent it wears.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CsgoSide {
    terrorist: bool,
    arms: &'static str,
}

impl CsgoSide {
    /// The Terrorists (`terrorist`) or the Counter-Terrorists on a map whose MW2 teams wear
    /// `factions` (allies, axis): the arms of the side's agent, as their body wears it.
    fn of(terrorist: bool, factions: Option<&(String, String)>) -> Self {
        let (allies, axis) =
            factions.map_or(("", ""), |(allies, axis)| (allies.as_str(), axis.as_str()));
        Self {
            terrorist,
            arms: csgo_arms(weapon_iw4::cs::csgo_side_voice(allies, axis, terrorist)),
        }
    }
}

/// CS:GO's arms of the agent who speaks in `voice` (`csgo_anim::agent_model`).
fn csgo_arms(voice: &str) -> &'static str {
    match voice {
        "sas" => "models/weapons/ct_arms_sas.mdl",
        "seal" => "models/weapons/ct_arms_st6.mdl",
        "balkan" => "models/weapons/t_arms_balkan.mdl",
        "leet" => "models/weapons/t_arms_leet.mdl",
        "separatist" => "models/weapons/t_arms_separatist.mdl",
        _ => "models/weapons/t_arms_phoenix.mdl",
    }
}
/// CS2's default `viewmodel_offset_x/y/z` (2.5 right, 0 forward, 1.5 down), in view space
/// (x forward, y left, z up).
const CS2_VIEWMODEL_OFFSET: Mat3x4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, -2.5],
    [0.0, 0.0, 1.0, -1.5],
];
/// CS:GO's viewmodel bob (`CalcViewModelBobHelper` at its defaults: `cl_bobcycle` 0.98,
/// `cl_bobamt_vert` 0.25, `cl_bobamt_lat` 0.4, `cl_bob_lower_amt` 21, `cl_bobup` 0.5).
const BOB_CYCLE_SCALE: f32 = 0.98;
const BOB_VERT: f32 = 0.25;
const BOB_LAT: f32 = 0.4;
const BOB_LOWER: f32 = 21.0;
const BOB_UP: f64 = 0.5;
/// The bob's speed follows the player's at most this fast (u/s per second) and tops out here.
const BOB_SPEED_SLEW: f32 = 640.0;
const BOB_MAX_SPEED: f32 = 320.0;
/// How far the gun shifts as the shooting gets steadier or shakier (`cl_viewmodel_shift_left_amt`
/// 1.5, `cl_viewmodel_shift_right_amt` 0.75).
const SHIFT_LEFT: f32 = 1.5;
const SHIFT_RIGHT: f32 = 0.75;
/// The radio call on every grenade throw (CS:S `Radio.FireInTheHole`, else CS 1.6's
/// `radio/ct_fireinhole.wav`).
const FIRE_IN_THE_HOLE: &str = "css/radio.fireinthehole";
const FIRE_IN_THE_HOLE_GOLDSRC: &str = "cs/radio/ct_fireinhole";
/// How long a muzzle flash shows, seconds (CS:S flashes for about two frames).
const FLASH_SECONDS: f64 = 0.05;
/// Flash tint (warm), scaled down as it fades.
const FLASH_TINT: [f32; 3] = [1.0, 0.86, 0.62];
/// Turn lag (`CalcViewModelLag`): how fast the gun's facing catches up (per second), the lag
/// beyond which it catches up faster, and how many units one unit of facing lag moves the gun.
const SWAY_CATCH_UP: f32 = 5.0;
const SWAY_MAX_LAG: f32 = 1.5;
const SWAY_UNITS: f32 = 5.0;
/// CS:S's and CS:GO's turn lag (their predicted viewmodel's): how far back the view's angles are
/// looked at (`cl_wpn_sway_interp`, seconds), and the units one unit of the forward's turn since
/// then moves the gun (`cl_wpn_sway_scale`: 1 in CS:S, 1.6 in CS:GO).
const SWAY_INTERP: f64 = 0.1;
const CSS_SWAY_SCALE: f32 = 1.0;
const CSGO_SWAY_SCALE: f32 = 1.6;
/// The scope's reticle dot, as a share of the lens's radius (a few pixels at 1080p).
const AIM_DOT: f32 = 0.015;

/// CS:GO's aim down a rifle's scope (`aimsight` in its items_game: the AUG's and SG 553's).
/// Zoomed, the gun turns by `pivot_angle` (pitch, yaw, roll) about the point `pivot_forward`
/// ahead of the eye and moves by minus `eye` (forward, left, up), which puts its scope's lens on
/// the middle of the view, square to it; it goes in at `speed_up` of the way a second and back at
/// `speed_down`, drawn at `fov` once in.
#[derive(Clone, Copy, Debug)]
struct Aimsight {
    eye: [f32; 3],
    pivot_angle: [f32; 3],
    pivot_forward: f32,
    speed_up: f32,
    speed_down: f32,
    fov: f32,
}

/// The aim of CS weapon `name`, the rifles that have one.
fn aimsight(name: &str) -> Option<Aimsight> {
    match name {
        "aug" => Some(Aimsight {
            eye: [-1.56, -3.6, -0.07],
            pivot_angle: [0.78, -0.1, -0.03],
            pivot_forward: 10.0,
            speed_up: 10.0,
            speed_down: 8.0,
            fov: 45.0,
        }),
        "sg552" => Some(Aimsight {
            eye: [0.72, -5.12, -1.33],
            pivot_angle: [0.52, 0.04, 0.72],
            pivot_forward: 8.0,
            speed_up: 10.0,
            speed_down: 8.0,
            fov: 45.0,
        }),
        _ => None,
    }
}

/// The gun `amount` (eased, 0 to 1) of the way into `aim`, in view space: turned about the
/// pivot, then moved. `mirrored` (`cl_righthand 0`) mirrors it across the view as the gun is.
fn aim_placement(aim: &Aimsight, amount: f32, mirrored: bool) -> Mat3x4 {
    let side = if mirrored { -1.0 } else { 1.0 };
    let turn = view_rotation(
        aim.pivot_angle[0] * amount,
        aim.pivot_angle[1] * amount * side,
        aim.pivot_angle[2] * amount * side,
    );
    let pivot = [aim.pivot_forward, 0.0, 0.0];
    let eye = [aim.eye[0], aim.eye[1] * side, aim.eye[2]];
    let mut place = turn;
    for (r, row) in place.iter_mut().enumerate() {
        let turned = row[0] * pivot[0] + row[1] * pivot[1] + row[2] * pivot[2];
        row[3] = pivot[r] - turned - eye[r] * amount;
    }
    place
}

/// Where a CS:GO rifle's scope lens is (its `aimsight lens mask` model): the bone it rides, its
/// centre in the gun's bind space (the bone's skinning matrix places it) and its radius.
#[derive(Clone, Copy, Debug)]
struct Lens {
    bone: usize,
    center: [f32; 3],
    radius: f32,
}

fn transform(m: &Mat3x4, p: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3])
}

/// Inverse of a rotation and translation.
fn rigid_inverse(m: &Mat3x4) -> Mat3x4 {
    let mut out = [[0.0; 4]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().take(3).enumerate() {
            *v = m[c][r];
        }
        row[3] = -(row[0] * m[0][3] + row[1] * m[1][3] + row[2] * m[2][3]);
    }
    out
}

/// The lens of CS:GO viewmodel `model` (`v_rif_aug`) on `gun`, from its scope's lens mask
/// (`models/weapons/v_rif_aug_scopelensmask.mdl`, a disc on one of the gun's bones), when the
/// gun has one.
fn scope_lens(vpk: &mdl_source::Vpk, gun: &mdl_source::StudioModel, model: &str) -> Option<Lens> {
    let stem = format!("models/weapons/{model}_scopelensmask");
    let mask = mdl_source::StudioModel::parse(
        &vpk.read(&format!("{stem}.mdl"))?,
        &vpk.read(&format!("{stem}.vvd"))?,
        &vpk.read(&format!("{stem}.dx90.vtx"))?,
    )
    .ok()?;
    let mask_bone = mask.bones.first()?;
    let bone = gun
        .bones
        .iter()
        .position(|b| b.name.eq_ignore_ascii_case(&mask_bone.name))?;
    // The mask's bind space, to its bone's, to the gun's bind space.
    let to_gun = mdl_goldsrc::concat(
        &rigid_inverse(&gun.bones[bone].pose_to_bone),
        &mask_bone.pose_to_bone,
    );
    let points: Vec<[f32; 3]> = mask
        .vertices
        .iter()
        .map(|v| transform(&to_gun, v.position))
        .collect();
    let count = points.len().max(1) as f32;
    let center = [0, 1, 2].map(|k| points.iter().map(|p| p[k]).sum::<f32>() / count);
    let radius = points
        .iter()
        .map(|p| (0..3).map(|k| (p[k] - center[k]).powi(2)).sum::<f32>().sqrt())
        .fold(0.0, f32::max);
    (radius > 0.0).then_some(Lens {
        bone,
        center,
        radius,
    })
}

static SWAY: AtomicBool = AtomicBool::new(true);
/// How far into a rifle's scope the gun is (eased, as f32 bits) and the view's zoom there.
static AIM_AMOUNT: AtomicU32 = AtomicU32::new(0);
static AIM_FOV: AtomicU32 = AtomicU32::new(0);

/// While the gun goes into (or out of) a CS:GO rifle's scope: how far in it is (0 to 1) and the
/// zoom (CS fov) it brings, which the view eases to with it, as CS:GO's ironsight does.
pub fn aimsight_view() -> Option<(f32, f32)> {
    let amount = f32::from_bits(AIM_AMOUNT.load(Ordering::Relaxed));
    (amount > 0.0).then(|| (amount, f32::from_bits(AIM_FOV.load(Ordering::Relaxed))))
}

/// Set once a frame by the viewmodel's update: [`aimsight_view`]'s amount and zoom.
fn publish_aim(amount: f32, fov: f32) {
    AIM_FOV.store(fov.to_bits(), Ordering::Relaxed);
    AIM_AMOUNT.store(amount.to_bits(), Ordering::Relaxed);
}

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
    /// CS:GO: Source models built right-handed, with their arms bone-merged on.
    Csgo,
    /// Counter-Strike 2.
    Source2,
}

enum Studio {
    GoldSrc(mdl_goldsrc::StudioModel),
    Source(mdl_source::StudioModel),
    /// Arms and gun posed together; "sequences" are stretches of the gun's clips.
    Source2(Box<mdl_source2::viewmodel::Viewmodel>, Vec<Cs2Segment>),
}

/// A stretch of a CS2 clip played as one sequence: each whole clip, and a shotgun's reload cut at
/// the intro, per-shell loop and outro markers CS2 puts in its one reload clip.
struct Cs2Segment {
    clip: usize,
    start: f32,
    end: f32,
}

impl Studio {
    /// Matrices that place each vertex's bind-space position in view space (before placement).
    /// `fading`: the sequences still fading out over it (Source models only; see [`Fade`]).
    fn pose(
        &self,
        sequence: usize,
        seconds: f32,
        fading: &[(usize, f32, f32)],
        out: &mut Vec<Mat3x4>,
    ) {
        match self {
            Self::GoldSrc(model) => model.pose(sequence, seconds, out),
            Self::Source(model) => model.pose_fading(sequence, seconds, fading, out),
            Self::Source2(viewmodel, segments) => {
                let Some(segment) = segments.get(sequence) else {
                    return;
                };
                let seconds = segment.start + seconds.clamp(0.0, segment.end - segment.start);
                out.clear();
                out.extend(viewmodel.skin(segment.clip, seconds).iter().map(rows));
            }
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
    /// Seconds a Source sequence fades in over the one before it and out under the one after
    /// (`fadeintime`, `fadeouttime`); 0 for the formats that cut between sequences.
    fade_in: f32,
    fade_out: f32,
    /// Cuts in with no fade (`STUDIO_SNAP`: CS:GO's shots, draws and reloads).
    snap: bool,
}

/// A sequence still fading out under the one that followed it (Source's
/// `CSequenceTransitioner`): it keeps playing at its rate while its share of the pose eases from
/// all of it to none.
#[derive(Clone, Copy)]
struct Fade {
    sequence: usize,
    /// When the sequence started playing and its speed, to know where in it the pose is.
    started: f64,
    rate: f32,
    /// When the fade began and how long it lasts (seconds).
    from: f64,
    over: f32,
}

/// At most this many sequences fade at once; an older one is dropped.
const MAX_FADES: usize = 4;

impl Fade {
    /// Its share of the pose at `now` (`GetFadeout`: 1 down to 0 on a smooth spline).
    fn weight(&self, now: f64) -> f32 {
        let left = 1.0 - (now - self.from) as f32 / self.over;
        if left <= 0.0 {
            0.0
        } else if left >= 1.0 {
            1.0
        } else {
            left * left * (3.0 - 2.0 * left)
        }
    }
}

/// Changing from the sequence `playing` plays to `next`: unless `next` snaps in, the old one
/// fades out under it over the shorter of its own fade-out and `next`'s fade-in.
fn fade_out(playing: &mut Playing, next: Option<usize>, sequences: &[SequenceInfo], now: f64) {
    let next = next.and_then(|n| sequences.get(n));
    if next.is_none_or(|next| next.snap) {
        playing.fading.clear();
        return;
    }
    let (Some(next), Some((current, info))) = (
        next,
        playing
            .sequence
            .and_then(|c| sequences.get(c).map(|info| (c, info))),
    ) else {
        return;
    };
    let over = info.fade_out.min(next.fade_in);
    if over <= 0.0 {
        playing.fading.clear();
        return;
    }
    playing.fading.retain(|fade| fade.weight(now) > 0.0);
    if playing.fading.len() >= MAX_FADES {
        playing.fading.remove(0);
    }
    playing.fading.push(Fade {
        sequence: current,
        started: playing.started,
        rate: playing.rate,
        from: now,
        over,
    });
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
    /// CS2 keeps the silencer on the gun's mesh, on its own bone, and hides that bone while the
    /// silencer is off: the bone's index and its origin in bind space.
    hidden_bone: Option<(usize, [f32; 3])>,
}

/// A CS2 pistol with its slide locked back: the last shot, the empty idle and the empty reload.
struct Empty {
    idle: Option<usize>,
    reload: Option<usize>,
    shoot: Option<usize>,
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
    empty: Option<Empty>,
    /// CS:GO's inspects (`lookat01`…`lookat03`), played on `+lookatweapon`, each with how
    /// often it is picked (the Deagle's spin 1 in 11).
    inspect: Vec<usize>,
    inspect_weights: Vec<u32>,
    /// Every plain idle and draw of a CS:S or CS:GO model with how often each is picked (its
    /// activity weight): CS:GO's knives have two idles, and the Butterfly and Ursus two draws.
    idles: Vec<(usize, u32)>,
    draws: Vec<(usize, u32)>,
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

    /// The idle to play: CS:GO picks one of a weapon's plain idles by weight each time one
    /// starts; `idle` itself when it is another (a silenced or empty pistol's) or the only one.
    fn pick_idle(&self, idle: Option<usize>, roll: u32) -> Option<usize> {
        pick_weighted(&self.idles, idle, self.idle, roll)
    }

    /// The draw to play, picked as [`pick_idle`](Self::pick_idle) picks the idle.
    fn pick_draw(&self, draw: Option<usize>, roll: u32) -> Option<usize> {
        pick_weighted(&self.draws, draw, self.draw, roll)
    }

    /// Whether `sequence` is an idle while `idle` is the gun's: it, or any plain one when it is
    /// the plain one.
    fn is_idle(&self, sequence: usize, idle: Option<usize>) -> bool {
        Some(sequence) == idle
            || (idle == self.idle && self.idles.iter().any(|(s, _)| *s == sequence))
    }

    /// Whether `sequence` draws the gun while `draw` is the gun's draw: it, or any plain one.
    fn is_draw(&self, sequence: usize, draw: Option<usize>) -> bool {
        Some(sequence) == draw
            || Some(sequence) == self.draw
            || self.draws.iter().any(|(s, _)| *s == sequence)
    }

    /// Whether each idle that ends makes way for another pick (the weapon has several).
    fn idle_varies(&self, idle: Option<usize>) -> bool {
        idle == self.idle && self.idles.len() > 1
    }
}

/// CS:GO's `SelectWeightedSequence` among `set` (sequence, weight) by `roll`, when `wanted` is
/// its `plain` one and it has a choice; else `wanted`.
fn pick_weighted(
    set: &[(usize, u32)],
    wanted: Option<usize>,
    plain: Option<usize>,
    roll: u32,
) -> Option<usize> {
    if wanted.is_none() || wanted != plain || set.len() < 2 {
        return wanted;
    }
    let total: u32 = set.iter().map(|(_, weight)| (*weight).max(1)).sum();
    let mut left = roll % total;
    for &(sequence, weight) in set {
        let weight = weight.max(1);
        if left < weight {
            return Some(sequence);
        }
        left -= weight;
    }
    wanted
}

/// The sequences of `studio` playing `activity`, each with its weight.
fn weighted(studio: &mdl_source::StudioModel, activity: &str) -> Vec<(usize, u32)> {
    studio
        .sequences_for_activity(activity)
        .map(|i| (i, studio.sequences[i].activity_weight.unsigned_abs()))
        .collect()
}

/// The knife's sequences go by the same labels in CS:S and CS 1.6; CS:GO's knives slash with
/// their light swings and stab with their heavy one.
fn knife_roles(labels: &[&str]) -> (Vec<usize>, Option<usize>, Option<usize>) {
    let find = |name: &str| labels.iter().position(|l| l.eq_ignore_ascii_case(name));
    let slashes: Vec<usize> = ["midslash1", "midslash2"]
        .iter()
        .filter_map(|n| find(n))
        .collect();
    if slashes.is_empty() && find("light_miss1").is_some() {
        return (
            ["light_miss1", "light_miss2"]
                .iter()
                .filter_map(|n| find(n))
                .collect(),
            find("heavy_hit1"),
            find("heavy_miss1"),
        );
    }
    (slashes, find("stab"), find("stab_miss"))
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
            // CS:S builds most guns left-handed and mirrors them; these three it builds
            // right-handed (`BuiltRightHanded`).
            css_right_handed: matches!(weapon.name, "m249" | "galil" | "famas"),
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
    /// A CS:GO rifle's scope lens, for its aim down the scope.
    lens: Option<Lens>,
}

struct Playing {
    weapon: u32,
    /// The model `sequence` and `fading` index into: a new knife or side swaps it under the
    /// same weapon.
    model: Arc<LoadedModel>,
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
    /// Sequences played before `sequence`, still fading out under it (oldest first).
    fading: Vec<Fade>,
}

#[derive(Resource, Default)]
pub struct CsViewmodels {
    /// Looked up once, by the startup read: CS2's pack and the CS:S pack (opened) and the
    /// GoldSrc folders (Condition Zero, then CS 1.6).
    cs2: Option<Option<Arc<mdl_source::Vpk>>>,
    csgo: Option<Option<Arc<mdl_source::Vpk>>>,
    css: Option<Option<Arc<mdl_source::Vpk>>>,
    goldsrc: Option<Option<asset_transport::GoldSrcDirs>>,
    /// The startup read (`warm_up`) while it runs off the main thread.
    warmup: Option<Task<Warmup>>,
    warmed_up: bool,
    models: HashMap<ModelKey, Option<Arc<LoadedModel>>>,
    /// Viewmodels being read and decoded off the main thread, by weapon name and side.
    decoding: HashMap<ModelKey, Task<Result<DecodedModel, String>>>,
    playing: Option<Playing>,
    bones: Vec<Mat3x4>,
    bob_time: f64,
    /// Horizontal speed the bob follows, eased.
    bob_speed: f32,
    /// How far the gun has shifted for the shooting's accuracy (`m_flGunAccuracyPosition`).
    gun_accuracy: f32,
    /// World-space facing the gun trails toward (turn sway).
    lagged_forward: Option<[f32; 3]>,
    /// The view's (pitch, yaw) over the last [`SWAY_INTERP`] seconds and when, oldest first
    /// (CS:S's and CS:GO's turn sway).
    sway_history: std::collections::VecDeque<(f64, [f32; 2])>,
    /// How far into a rifle's scope the gun is (0 to 1, not eased).
    aim: f32,
    /// CS:GO's `dev/scope_vignette`, darkening the blur around an aimed scope.
    vignette: Option<Handle<Image>>,
    /// CS:S muzzle flash sprites (`sprites/muzzleflash4`, `effects/muzzleflashx`), looked up once.
    flash_images: Option<Option<[Handle<Image>; 2]>>,
    /// CS2 textures on the GPU, shared by every gun that draws them (the arms).
    cs2_images: cs2_assets::Cs2Images,
    /// When `+lookatweapon` was last taken (CS:GO takes it at most every
    /// [`INSPECT_COMMAND_RATE`]).
    inspect_taken: Option<f64>,
    /// The two sides' looks on the map being played (Terrorists, Counter-Terrorists).
    sides: Option<[CsgoSide; 2]>,
    /// Since when `+reload` has been held (none while it is up).
    reload_held_since: Option<f64>,
}

/// How long `+reload` is held, with nothing to reload, before an inspect is put away (a tap
/// leaves it playing). Picked by feel, not a CS:GO number.
const INSPECT_RELOAD_HOLD: f64 = 0.2;

/// CS:GO's `CS_COMMAND_MAX_RATE`: a rate-limited command (`+lookatweapon` among them) pressed
/// again sooner than this after the last one taken is dropped.
const INSPECT_COMMAND_RATE: f64 = 0.3;

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
            tangent: [0.0; 4],
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
                maps: None,
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
            fade_in: 0.0,
            fade_out: 0.0,
            snap: true,
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
        hidden_bone: None,
    });
    let mut roles = Roles {
        inspect: Vec::new(),
        inspect_weights: Vec::new(),
        idles: Vec::new(),
        draws: Vec::new(),
        slashes,
        stab,
        stab_miss,
        empty: None,
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
        lens: None,
    }
}

/// Where a bone sits in bind space: the inverse of its bind-to-bone matrix applied to its origin.
fn bind_origin(pose_to_bone: &Mat3x4) -> [f32; 3] {
    let m = pose_to_bone;
    [0, 1, 2].map(|i| -(m[0][i] * m[0][3] + m[1][i] * m[1][3] + m[2][i] * m[2][3]))
}

fn source_model(
    loaded: mdl_source::LoadedModel,
    images: &mut Assets<Image>,
    format: Format,
) -> LoadedModel {
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
            tangent: [0.0; 4],
        })
        .collect();
    let draws = studio
        .meshes
        .iter()
        // CS:GO models carry alternatives in their body parts (the R8's speedloader, its fired
        // rounds); like the engine, only each part's first model draws.
        .filter(|mesh| format != Format::Csgo || mesh.body_model == 0)
        .filter_map(|mesh| {
            let material = loaded.materials.get(mesh.material)?;
            Some(CsViewmodelDraw {
                image: handles.get(mesh.material)?.clone()?,
                first_vertex: mesh.first_vertex as u32,
                vertex_count: mesh.vertex_count as u32,
                maps: None,
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
            fade_in: s.fade_in,
            fade_out: s.fade_out,
            snap: s.flags & mdl_source::studio::SEQ_SNAP != 0,
            sounds: s
                .events
                .iter()
                .filter(|e| e.event == mdl_source::EVENT_CLIENT_SOUND)
                .map(|e| {
                    (
                        e.cycle * s.num_frames.saturating_sub(1) as f32,
                        format!(
                            "{}{}",
                            if format == Format::Csgo {
                                asset_audio::CSGO_SOUND_PREFIX
                            } else {
                                asset_audio::CSS_SOUND_PREFIX
                            },
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
    let silenced = attach.map(|attach| {
        let mut silenced = Silenced {
            idle: studio.sequence_for_activity("ACT_VM_IDLE_SILENCED"),
            draw: studio.sequence_for_activity("ACT_VM_DRAW_SILENCED"),
            reload: studio.sequence_for_activity("ACT_VM_RELOAD_SILENCED"),
            shoot: studio
                .sequences_for_activity("ACT_VM_PRIMARYATTACK_SILENCED")
                .collect(),
            attach: Some(attach),
            detach: studio.sequence_for_activity("ACT_VM_DETACH_SILENCER"),
            hidden_bone: None,
        };
        // CS:GO's M4A1-S and USP-S share one set but the draw, and keep the silencer on its own
        // bone (hidden while it is off).
        if format == Format::Csgo {
            silenced.idle = silenced.idle.or(studio.sequence_for_activity("ACT_VM_IDLE"));
            silenced.reload = silenced.reload.or(studio.sequence_for_activity("ACT_VM_RELOAD"));
            if silenced.shoot.is_empty() {
                silenced.shoot = studio
                    .sequences_for_activity("ACT_VM_PRIMARYATTACK")
                    .collect();
            }
            silenced.hidden_bone = studio
                .bones
                .iter()
                .position(|b| b.name.to_ascii_lowercase().ends_with("silencer"))
                .map(|bone| (bone, bind_origin(&studio.bones[bone].pose_to_bone)));
        }
        silenced
    });
    let roles = Roles {
        // As CS:GO's `LookAtHeldWeapon`: the sequences playing `ACT_VM_IDLE_LOWERED`, else
        // `lookat01`.
        inspect: {
            let lowered: Vec<usize> = studio
                .sequences_for_activity("ACT_VM_IDLE_LOWERED")
                .collect();
            if lowered.is_empty() {
                label("lookat01").into_iter().collect()
            } else {
                lowered
            }
        },
        inspect_weights: {
            let lowered: Vec<u32> = studio
                .sequences
                .iter()
                .filter(|s| s.activity.eq_ignore_ascii_case("ACT_VM_IDLE_LOWERED"))
                .map(|s| s.activity_weight.unsigned_abs())
                .collect();
            if lowered.is_empty() { vec![1] } else { lowered }
        },
        slashes,
        stab,
        stab_miss,
        empty: None,
        pullpin,
        throw,
        arm,
        silenced,
        idles: weighted(&studio, "ACT_VM_IDLE"),
        draws: weighted(&studio, "ACT_VM_DRAW"),
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
        format,
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
        lens: None,
    }
}

/// A CS2 skinning matrix as the viewmodel pass takes it (three rows).
fn rows(m: &mdl_source2::pose::Mat3x4) -> Mat3x4 {
    let m = &m.0;
    [
        [m[0], m[1], m[2], m[3]],
        [m[4], m[5], m[6], m[7]],
        [m[8], m[9], m[10], m[11]],
    ]
}

/// A CS2 gun's clips in the roles the player needs, by their names (`idle_ak`, `draw_ak`,
/// `reload_ak`, `shoot1_ak`; guns sharing CS2's default set play `idle_rifle`...).
fn cs2_roles(viewmodel: &mdl_source2::viewmodel::Viewmodel) -> Roles {
    // `idle_ak`, not `idle_slide_back_glock` or `idle_from_activity_m249`.
    let single = |prefix: &str| viewmodel.find(prefix, |name| !name[prefix.len()..].contains('_'));
    let every = |prefix: &str, skip: &[&str]| -> Vec<usize> {
        viewmodel
            .clips
            .iter()
            .enumerate()
            .filter(|(_, c)| c.name.starts_with(prefix) && !skip.iter().any(|s| c.name.starts_with(s)))
            .map(|(i, _)| i)
            .collect()
    };
    // The G3SG1 and M249 name theirs `idle1_…`.
    let idle = single("idle_").or_else(|| single("idle1_"));
    let (draw, reload) = (single("draw_"), single("reload_"));
    // The Dual Berettas shoot left and right in turn.
    let left = every("shoot_left", &["shoot_leftlast"]);
    let (shoot, shoot_alt) = if left.is_empty() {
        (
            every("shoot", &["shoot_empty", "shoot_left", "shoot_right"]),
            Vec::new(),
        )
    } else {
        (left, every("shoot_right", &["shoot_rightlast"]))
    };
    // A pistol's slide locks back on its last shot.
    let empty = viewmodel.find("idle_slide_back_", |_| true).map(|idle| Empty {
        idle: Some(idle),
        reload: viewmodel.find("reload_empty_", |_| true),
        shoot: viewmodel.find("shoot_empty_", |_| true),
    });
    // CS2 keeps one set of clips for a gun with or without its silencer.
    let silenced = viewmodel.find("silencer_attach", |_| true).map(|attach| Silenced {
        idle,
        draw,
        reload,
        shoot: shoot.clone(),
        attach: Some(attach),
        detach: viewmodel.find("silencer_detach", |_| true),
        hidden_bone: viewmodel.weapon.bone("silencer").and_then(|bone| {
            let bind = viewmodel.weapon.inverse_bind.get(bone).copied().flatten()?;
            let origin = mdl_source2::pose::Mat3x4(bind).inverse().0;
            Some((viewmodel.arms.bones.len() + bone, [origin[3], origin[7], origin[11]]))
        }),
    });
    Roles {
        idle,
        draw,
        reload,
        shoot,
        silenced,
        shoot_alt,
        shell_start: None,
        shell_finish: None,
        // The knife's light swings, and its heavy one landing or not.
        slashes: every("light_miss", &[]),
        pullpin: viewmodel.find("pullpin_", |_| true),
        throw: viewmodel.find("throw_overhand_", |_| true),
        // The C4's whole plant: keypad, then down on the ground.
        arm: viewmodel.find("plant_", |_| true),
        stab: viewmodel.find("heavy_hit", |_| true),
        stab_miss: viewmodel.find("heavy_miss", |_| true),
        empty,
        inspect: Vec::new(),
        inspect_weights: Vec::new(),
        idles: Vec::new(),
        draws: Vec::new(),
    }
}

/// What the viewmodel pass draws of a CS2 gun: the arms' meshes then the gun's as triangle lists
/// (the gun's bones numbered after the arms'), one draw per material, the clips as sequences
/// with the sounds their events call for (CS:S's sound of the same name).
fn cs2_model(
    decoded: Cs2Decoded,
    images: &mut Assets<Image>,
    cache: &mut cs2_assets::Cs2Images,
) -> LoadedModel {
    let Cs2Decoded {
        viewmodel,
        materials,
    } = decoded;
    let handles: HashMap<String, (Handle<Image>, CsViewmodelShading, Option<CsViewmodelMaps>)> =
        materials
            .iter()
            .map(|(name, material)| {
                let colour = cache.get(&material.colour, true, images);
                let maps = match (&material.normal, &material.metal) {
                    (Some(normal), Some(metal)) if !material.alpha_test => {
                        let ambient_occlusion = match &material.ambient_occlusion {
                            Some(ao) => cache.get(ao, false, images),
                            None => cache.no_occlusion(images),
                        };
                        Some(CsViewmodelMaps {
                            normal: cache.get(normal, false, images),
                            metal: cache.get(metal, false, images),
                            ambient_occlusion,
                        })
                    }
                    _ => None,
                };
                let shading = match (material.shading, &maps) {
                    (cs2_assets::Cs2Shading::Weapon, Some(_)) => CsViewmodelShading::PbrWeapon,
                    (cs2_assets::Cs2Shading::Character, Some(_)) => {
                        CsViewmodelShading::PbrCharacter
                    }
                    _ if material.alpha_test => CsViewmodelShading::Lit,
                    _ => CsViewmodelShading::Opaque,
                };
                let maps = maps.filter(|_| {
                    matches!(
                        shading,
                        CsViewmodelShading::PbrWeapon | CsViewmodelShading::PbrCharacter
                    )
                });
                (name.clone(), (colour, shading, maps))
            })
            .collect();
    let mut vertices: Vec<CsViewmodelVertex> = Vec::new();
    let mut draws = Vec::new();
    let mut bone_base = 0usize;
    for model in [&viewmodel.arms, &viewmodel.weapon] {
        for mesh in &model.meshes {
            for draw in &mesh.draws {
                let (Some(vb), Some(ib), Some((image, shading, maps))) = (
                    mesh.vertex_buffers.get(draw.vertex_buffer),
                    mesh.index_buffers.get(draw.index_buffer),
                    handles.get(&draw.material),
                ) else {
                    continue;
                };
                let first = vertices.len() as u32;
                let start = (draw.start_index as usize).min(ib.len());
                let end = (start + draw.index_count as usize).min(ib.len());
                for triangle in ib[start..end].as_chunks::<3>().0 {
                    let corners: Option<Vec<_>> = triangle
                        .iter()
                        .map(|&i| vb.get((i64::from(i) + i64::from(draw.base_vertex)) as usize))
                        .collect();
                    let Some(corners) = corners else { continue };
                    vertices.extend(corners.into_iter().map(|v| CsViewmodelVertex {
                        position: v.position,
                        normal: v.normal,
                        uv: v.uv,
                        bones: v.bones.map(|b| {
                            (usize::from(b) + bone_base).min(render_gpu::CS_VIEWMODEL_MAX_BONES - 2) as u8
                        }),
                        weights: v.weights.map(|w| (w.clamp(0.0, 1.0) * 255.0 + 0.5) as u8),
                        tangent: v.tangent,
                    }));
                }
                draws.push(CsViewmodelDraw {
                    image: image.clone(),
                    first_vertex: first,
                    vertex_count: vertices.len() as u32 - first,
                    shading: *shading,
                    maps: maps.clone(),
                });
            }
        }
        bone_base += model.bones.len();
    }
    let mut segments: Vec<Cs2Segment> = viewmodel
        .clips
        .iter()
        .enumerate()
        .map(|(clip, named)| Cs2Segment {
            clip,
            start: 0.0,
            end: named.clip.duration,
        })
        .collect();
    let mut roles = cs2_roles(&viewmodel);
    // A shotgun reloads shell by shell: its one reload clip marks where the per-shell loop and
    // the closing pump begin.
    if let Some(reload) = roles.reload {
        let clip = &viewmodel.clips[reload].clip;
        let marker = |id: &str| {
            clip.events
                .iter()
                .find(|e| e.kind == "ID" && e.name == id)
                .map(|e| e.time)
        };
        if let (Some(insert), Some(outro)) = (marker("WPN_RELOAD_LOOP"), marker("WPN_RELOAD_OUTRO"))
            && 0.0 < insert
            && insert < outro
            && outro < clip.duration
        {
            let first = segments.len();
            for (start, end) in [(0.0, insert), (insert, outro), (outro, clip.duration)] {
                segments.push(Cs2Segment { clip: reload, start, end });
            }
            roles.shell_start = Some(first);
            roles.reload = Some(first + 1);
            roles.shell_finish = Some(first + 2);
        }
    }
    let sequences = segments
        .iter()
        .map(|segment| {
            let clip = &viewmodel.clips[segment.clip].clip;
            let fps = if clip.frames > 1 && clip.duration > 0.0 {
                (clip.frames - 1) as f32 / clip.duration
            } else {
                30.0
            };
            let duration = segment.end - segment.start;
            let whole = segment.end >= clip.duration;
            SequenceInfo {
                fps,
                num_frames: (duration * fps).round() as usize + 1,
                duration,
                fade_in: 0.0,
                fade_out: 0.0,
                snap: true,
                sounds: clip
                    .events
                    .iter()
                    .filter(|e| {
                        e.kind == "Sound"
                            && e.time >= segment.start
                            && (e.time < segment.end || whole)
                    })
                    .map(|e| {
                        (
                            (e.time - segment.start) * fps,
                            format!(
                                "{}{}",
                                asset_audio::CSS_SOUND_PREFIX,
                                e.name.to_ascii_lowercase()
                            ),
                        )
                    })
                    .collect(),
            }
        })
        .collect();
    let attachment = |name: &str| {
        viewmodel
            .weapon_attachment(name)
            .map(|(bone, frame)| (bone, rows(&frame)))
    };
    let (muzzle, eject) = (attachment("muzzle_flash"), attachment("shell_eject"));
    LoadedModel {
        format: Format::Source2,
        gpu: Arc::new(CsViewmodelModel {
            id: NEXT_MODEL_ID.fetch_add(1, Ordering::Relaxed),
            vertices,
            draws,
        }),
        studio: Studio::Source2(Box::new(viewmodel), segments),
        sequences,
        roles,
        muzzle,
        eject,
        goldsrc_right_handed: false,
        lens: None,
    }
}

/// A viewmodel read and decoded off the main thread; its textures still go into `Assets`.
enum DecodedModel {
    Source(mdl_source::LoadedModel),
    Csgo(mdl_source::LoadedModel, Option<Lens>),
    GoldSrc(mdl_goldsrc::StudioModel),
    Source2(Box<Cs2Decoded>),
}

/// Reads a CS:GO viewmodel and bone-merges the arms on (it keeps none of its own).
fn decode_csgo(vpk: &mdl_source::Vpk, model: &str, arms: &str) -> Result<DecodedModel, String> {
    let mut gun = mdl_source::load_model(vpk, &format!("models/weapons/{model}.mdl"))
        .map_err(|e| format!("CS:GO {model}: {e}"))?;
    let lens = scope_lens(vpk, &gun.model, model);
    // One slot stays free for the muzzle flash's.
    match mdl_source::load_model(vpk, arms)
        .and_then(|arms| gun.bone_merge(&arms, render_gpu::CS_VIEWMODEL_MAX_BONES - 1))
    {
        Ok(()) => {}
        Err(error) => diag::warn!(World, "cs viewmodel: CS:GO {model} without arms: {error}"),
    }
    Ok(DecodedModel::Csgo(gun, lens))
}

/// Reads and decodes a viewmodel: CS2's model when CS2 is installed and the weapon has one, else
/// CS:GO's (with the team's arms) when CS:GO is, else from the CS:S pack when there is one, else
/// the GoldSrc folders (Condition Zero's model when it has one, else CS 1.6's).
fn decode_model(
    cs2: Option<(&mdl_source::Vpk, weapon_iw4::cs::Cs2View)>,
    csgo: Option<(&mdl_source::Vpk, &str, &str)>,
    css: Option<&mdl_source::Vpk>,
    goldsrc: Option<&asset_transport::GoldSrcDirs>,
    view_model: &str,
    css_view_model: &str,
) -> Result<DecodedModel, String> {
    if let Some((vpk, view)) = cs2 {
        match cs2_assets::decode_viewmodel(vpk, view) {
            Ok(decoded) => return Ok(DecodedModel::Source2(Box::new(decoded))),
            Err(error) => diag::warn!(World, "cs viewmodel: CS2 {error}; trying the others"),
        }
    }
    if let Some((vpk, model, arms)) = csgo {
        match decode_csgo(vpk, model, arms) {
            Ok(decoded) => return Ok(decoded),
            Err(error) => diag::warn!(World, "cs viewmodel: {error}; trying the others"),
        }
    }
    if let Some(vpk) = css {
        return mdl_source::load_model(vpk, &format!("models/weapons/{css_view_model}.mdl"))
            .map(DecodedModel::Source)
            .map_err(|e| format!("{css_view_model}: {e}"));
    }
    let Some(dirs) = goldsrc else {
        return Err(format!(
            "no Counter-Strike: Source install found ({}) and no Condition Zero or \
             Counter-Strike 1.6 folder selected (Manage Game Paths on the main menu)",
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
    cs2_images: &mut cs2_assets::Cs2Images,
) -> Option<Arc<LoadedModel>> {
    let model = match decoded {
        Ok(DecodedModel::Source(loaded)) => source_model(loaded, images, Format::Source),
        Ok(DecodedModel::Csgo(loaded, lens)) => LoadedModel {
            lens,
            ..source_model(loaded, images, Format::Csgo)
        },
        Ok(DecodedModel::GoldSrc(studio)) => goldsrc_model(studio, images),
        Ok(DecodedModel::Source2(decoded)) => cs2_model(*decoded, images, cs2_images),
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
    cs2: Option<Arc<mdl_source::Vpk>>,
    csgo: Option<Arc<mdl_source::Vpk>>,
    css: Option<Arc<mdl_source::Vpk>>,
    goldsrc: Option<asset_transport::GoldSrcDirs>,
    models: Vec<(ModelKey, Result<DecodedModel, String>)>,
    flashes: Option<[mdl_source::vtf::Image; 2]>,
    vignette: Option<mdl_source::vtf::Image>,
}

/// Opens the CS:S pack (or finds the Condition Zero / CS 1.6 folders), and decodes the viewmodels every player
/// holds at spawn (knife, starting pistols, C4, grenades) and the muzzle flash sprites. Read on
/// first use instead, each stopped the main thread for 30 ms, and the pack's first read from a
/// cold disk for most of a second.
fn warm_up() -> Warmup {
    use weapon_iw4::cs;
    let cs2 = asset_transport::find_cs2_pak().and_then(|pak| match mdl_source::Vpk::open(&pak) {
        Ok(vpk) => {
            diag::info!(World, "cs viewmodels: Counter-Strike 2 from {}", pak.display());
            Some(Arc::new(vpk))
        }
        Err(error) => {
            diag::warn!(World, "cs viewmodels: CS2: {error}");
            None
        }
    });
    let csgo = asset_transport::find_csgo_pak().and_then(|pak| match mdl_source::Vpk::open(&pak) {
        Ok(vpk) => {
            diag::info!(World, "cs viewmodels: CS:GO from {}", pak.display());
            Some(Arc::new(vpk))
        }
        Err(error) => {
            diag::warn!(World, "cs viewmodels: CS:GO: {error}");
            None
        }
    });
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
    if css.is_some() || goldsrc.is_some() || csgo.is_some() {
        for (name, view_model, css_view_model) in spawn_set() {
            // Both sides (with no map's factions known yet): only CS:GO's models differ by side
            // (their arms), the others are read once and shared.
            for terrorist in [true, false] {
                let side = CsgoSide::of(terrorist, None);
                let cs2_gun = cs2.as_deref().zip(cs::cs2_view(name));
                let csgo_gun = csgo_source(csgo.as_deref(), name, side, None);
                if !terrorist && csgo_gun.is_none() {
                    continue;
                }
                let decoded = decode_model(
                    cs2_gun,
                    csgo_gun,
                    css.as_deref(),
                    goldsrc.as_ref(),
                    view_model,
                    css_view_model,
                );
                models.push(((name, side), decoded));
            }
        }
    }
    let flashes = css.as_deref().and_then(|pack| {
        let load = |path: &str| mdl_source::vtf::decode(&pack.read(path)?).ok();
        Some([
            load("materials/sprites/muzzleflash4.vtf")?,
            load("materials/effects/muzzleflashx.vtf")?,
        ])
    });
    let vignette = csgo.as_deref().and_then(|pack| {
        mdl_source::vtf::decode(&pack.read("materials/dev/scope_vignette.vtf")?).ok()
    });
    Warmup {
        cs2,
        csgo,
        css,
        goldsrc,
        models,
        flashes,
        vignette,
    }
}

/// A viewmodel's cache key: the weapon, and the side holding it (CS:GO's arms differ).
type ModelKey = (&'static str, CsgoSide);

/// CS:GO's model of `name` and the side's arms, when CS:GO is installed and has the weapon.
fn csgo_source<'a>(
    csgo: Option<&'a mdl_source::Vpk>,
    name: &str,
    side: CsgoSide,
    knife: Option<&'static str>,
) -> Option<(&'a mdl_source::Vpk, &'static str, &'static str)> {
    csgo.zip(weapon_iw4::cs::csgo_view_model(name)).map(|(vpk, models)| {
        // `cl_knife`: the chosen CS:GO knife instead of the side's own.
        let model = knife
            .filter(|_| name == "knife")
            .unwrap_or(models[usize::from(!side.terrorist)]);
        (vpk, model, side.arms)
    })
}

/// The viewmodels every player holds at spawn (knife, C4, starting pistols, grenades), by name,
/// with their GoldSrc and CS:S models.
fn spawn_set() -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    use weapon_iw4::cs;
    let pistols = cs::CS_WEAPONS
        .iter()
        .filter(|w| matches!(w.name, "glock" | "usp"))
        .map(|w| (w.name, w.view_model, w.css_view_model));
    let grenades = cs::CS_GRENADES
        .iter()
        .map(|g| (g.name, g.view_model, g.css_view_model));
    let (knife, c4) = (&cs::CS_KNIFE, &cs::CS_C4);
    [
        (knife.name, knife.view_model, knife.css_view_model),
        (c4.name, c4.view_model, c4.css_view_model),
    ]
    .into_iter()
    .chain(pistols)
    .chain(grenades)
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
            self.cs2 = Some(warm.cs2);
            self.csgo = Some(warm.csgo);
            self.css = Some(warm.css);
            self.goldsrc = Some(warm.goldsrc);
            self.flash_images = Some(warm.flashes.map(|sprites| {
                sprites.map(|sprite| images.add(image(sprite.width, sprite.height, sprite.rgba)))
            }));
            self.vignette = warm
                .vignette
                .map(|vignette| images.add(image(vignette.width, vignette.height, vignette.rgba)));
            for (key, decoded) in warm.models {
                let model = finish_model(key.0, decoded, images, &mut self.cs2_images);
                // A model that is not CS:GO's serves both sides.
                if key.1.terrorist && !model.as_ref().is_some_and(|m| m.format == Format::Csgo) {
                    self.models
                        .insert((key.0, CsgoSide::of(false, None)), model.clone());
                }
                self.models.insert(key, model);
            }
        }
        let finished: Vec<ModelKey> = self
            .decoding
            .iter()
            .filter(|(_, task)| task.is_finished())
            .map(|(key, _)| *key)
            .collect();
        for key in finished {
            if let Some(task) = self.decoding.remove(&key) {
                let model = finish_model(key.0, block_on(task), images, &mut self.cs2_images);
                self.models.insert(key, model);
            }
        }
    }

    /// The CS:S flash sprites as additive textures, once the startup read has them.
    fn flash_images(&self) -> Option<[Handle<Image>; 2]> {
        self.flash_images.clone().flatten()
    }

    /// `weapon`'s viewmodel for a player on `side`. One not read yet is decoded off the main
    /// thread, loading meanwhile.
    fn model_for(
        &mut self,
        weapon: ViewWeapon,
        side: CsgoSide,
        knife: Option<&'static str>,
    ) -> ModelLoad {
        self.model_by_name(
            (weapon.name, weapon.view_model, weapon.css_view_model),
            weapon.knife,
            side,
            knife,
        )
    }

    /// [`model_for`](Self::model_for) by the weapon's name and its GoldSrc and CS:S models.
    fn model_by_name(
        &mut self,
        (name, view_model, css_view_model): (&'static str, &'static str, &'static str),
        is_knife: bool,
        side: CsgoSide,
        knife: Option<&'static str>,
    ) -> ModelLoad {
        // A chosen CS:GO knife is its own model (by its viewmodel's name).
        let knife = knife.filter(|_| is_knife && self.csgo.clone().flatten().is_some());
        let key = (knife.unwrap_or(name), side);
        if let Some(model) = self.models.get(&key) {
            return model
                .clone()
                .map_or(ModelLoad::Unavailable, ModelLoad::Ready);
        }
        if self.warmed_up && !self.decoding.contains_key(&key) {
            let cs2 = self.cs2.clone().flatten();
            let csgo = self.csgo.clone().flatten();
            let css = self.css.clone().flatten();
            let goldsrc = self.goldsrc.clone().flatten();
            let cs2_view = weapon_iw4::cs::cs2_view(name);
            let task = AsyncComputeTaskPool::get().spawn(async move {
                decode_model(
                    cs2.as_deref().zip(cs2_view),
                    csgo_source(csgo.as_deref(), name, side, knife),
                    css.as_deref(),
                    goldsrc.as_ref(),
                    view_model,
                    css_view_model,
                )
            });
            self.decoding.insert(key, task);
        }
        ModelLoad::Loading
    }

    /// On a map whose two sides wear `sides` (Terrorists, Counter-Terrorists): drops the
    /// viewmodels of any other side and starts reading the spawn set (with the chosen knife) for
    /// both, as the startup read does before a map is known, so none is read at a spawn.
    fn follow_sides(&mut self, sides: [CsgoSide; 2], knife: Option<&'static str>) {
        if !self.warmed_up || self.sides == Some(sides) {
            return;
        }
        self.sides = Some(sides);
        self.models.retain(|(_, side), _| sides.contains(side));
        for side in sides {
            for weapon in spawn_set() {
                let is_knife = weapon.0 == weapon_iw4::cs::CS_KNIFE.name;
                let _ = self.model_by_name(weapon, is_knife, side, knife);
            }
        }
    }
}

/// Whether `team` plays the Terrorists in this match: the bomb mode's attackers (`cs_attackers`),
/// everyone but MW2's allies when it names none, as the server decides
/// ([`weapon_iw4::cs::cs_plays_terrorist`]).
pub(super) fn terrorist_team(snap: &sim::Snapshot, team: i32) -> bool {
    let attackers = snap
        .meta
        .objectives
        .server_info
        .iter()
        .find(|(name, _)| name == "cs_attackers")
        .map(|(_, value)| value.as_str());
    weapon_iw4::cs::cs_plays_terrorist(attackers, team == entity_iw4::TEAM_ALLIES)
}

/// The server's `cs_factions` ("<allies> <axis>"): each side's MW2 faction.
pub(super) fn cs_factions(snapshot: &sim::Snapshot) -> Option<(String, String)> {
    let value = &snapshot
        .meta
        .objectives
        .server_info
        .iter()
        .find(|(name, _)| name == "cs_factions")?
        .1;
    let (allies, axis) = value.split_once(' ')?;
    Some((allies.to_owned(), axis.to_owned()))
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
        (cs_knife::HIT_PLAYER, true, Format::Csgo) => "csgo/weapon_knife.stab",
        (cs_knife::HIT_PLAYER, false, Format::Csgo) => "csgo/weapon_knife.hit",
        (cs_knife::HIT_WORLD, _, Format::Csgo) => "csgo/weapon_knife.hitwall",
        (_, _, Format::Csgo) => "csgo/weapon_knife.slash",
        (cs_knife::HIT_PLAYER, true, Format::Source | Format::Source2) => "css/weapon_knife.stab",
        (cs_knife::HIT_PLAYER, false, Format::Source | Format::Source2) => "css/weapon_knife.hit",
        (cs_knife::HIT_WORLD, _, Format::Source | Format::Source2) => "css/weapon_knife.hitwall",
        (_, _, Format::Source | Format::Source2) => "css/weapon_knife.slash",
        (cs_knife::HIT_PLAYER, true, Format::GoldSrc) => "cs/weapons/knife_stab",
        (cs_knife::HIT_PLAYER, false, Format::GoldSrc) => "cs/weapons/knife_hit1",
        (cs_knife::HIT_WORLD, _, Format::GoldSrc) => "cs/weapons/knife_hitwall1",
        (_, _, Format::GoldSrc) => "cs/weapons/knife_slash1",
    }
}

fn knife_deploy_sound(format: Format) -> &'static str {
    match format {
        Format::Csgo => "csgo/weapon_knife.deploy",
        Format::Source | Format::Source2 => "css/weapon_knife.deploy",
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

/// A bob value of `amplitude` at `time` into a `period`: rises for `BOB_UP` of it, falls for
/// the rest, biased upward.
fn bob_wave(time: f64, period: f64, amplitude: f32) -> f32 {
    let cycle = (time % period) / period;
    let phase = if cycle < BOB_UP {
        core::f64::consts::PI * cycle / BOB_UP
    } else {
        core::f64::consts::PI + core::f64::consts::PI * (cycle - BOB_UP) / (1.0 - BOB_UP)
    };
    amplitude * 0.3 + amplitude * 0.7 * (phase as f32).sin()
}

/// The run speed CS weapon `index` allows (scoped when `zoomed`), which sets the bob's pace.
fn bob_max_speed(index: Option<u8>, zoomed: bool) -> f32 {
    use weapon_iw4::cs;
    let Some(index) = index else {
        return 250.0;
    };
    if let Some(gun) = cs::cs_weapon(index) {
        if zoomed { gun.max_speed_zoomed } else { gun.max_speed }
    } else if cs::is_knife(index) {
        cs::CS_KNIFE.max_speed
    } else if cs::is_c4(index) {
        cs::CS_C4.max_speed
    } else {
        250.0
    }
}

/// Where the shooting's accuracy moves the gun (CS:GO `CalcViewModelBobHelper`): left as it
/// steadies below standing still, right (a little) as moving, jumping or firing shakes it; never
/// for the Dual Berettas, nor while reloading.
fn accuracy_shift(ps: &PlayerState, index: Option<u8>, reloading: bool) -> f32 {
    use weapon_iw4::{cs, csgo};
    let Some((gun, index)) = index.and_then(|i| cs::cs_weapon(i).map(|gun| (gun, i))) else {
        return 0.0;
    };
    if reloading || gun.dual || ps.cs_shooting_mode == csgo::SHOOTING_CS16 {
        return 0.0;
    }
    let Some(csgo_gun) = csgo::csgo_gun(gun.name) else {
        return 0.0;
    };
    let bit = cs::silencer_bit(index);
    let mode = csgo::gun_mode(
        ps.cs_zoom != 0,
        gun.silencer.is_some() && cs::silencer_on(ps.cs_silencers, bit, ps.cs_shooting_mode),
        gun.burst.is_some() && ps.cs_burst_modes & bit != 0,
    );
    let ducked = ps.pm_flags & playerstate_iw4::pm_flags::CROUCH != 0;
    let base = csgo_gun.inaccuracy_stand[mode] * 0.001;
    let accuracy = if ducked {
        csgo_gun.inaccuracy_crouch[mode] * 0.001
    } else {
        csgo::inaccuracy(
            csgo_gun,
            mode,
            ps.cs_accuracy,
            csgo::CsgoShooter {
                on_ground: ps.ground_entity_num != playerstate_iw4::ENTITYNUM_NONE,
                ducked,
                on_ladder: false,
                walking: false,
                reloading,
                speed: ps.velocity[0].hypot(ps.velocity[1]),
                vertical_speed: ps.velocity[2],
            },
        )
    };
    let (accuracy, scale) = if accuracy < base {
        (accuracy, if gun.scope_overlay { 0.15 } else { 18.0 } * SHIFT_LEFT)
    } else {
        (accuracy.min(0.082), SHIFT_RIGHT)
    };
    ((accuracy - base) * scale).max(-0.1) * 80.0
}

/// Walk bob and turn sway as one view-space placement of the gun: small offsets (about a unit)
/// and tilts (well under a degree), so the gun never swings far enough to show its back.
/// `steady` scales it all (0 holds the gun still, as aiming down a scope does), `now` is the
/// clock the turn sway's history keeps.
#[allow(clippy::too_many_arguments)]
fn sway_placement(
    state: &mut CsViewmodels,
    ps: &PlayerState,
    index: Option<u8>,
    reloading: bool,
    format: Format,
    now: f64,
    steady: f32,
    dt: f32,
) -> Mat3x4 {
    if !viewmodel_sway() {
        state.lagged_forward = None;
        state.sway_history.clear();
        return mdl_goldsrc::IDENTITY;
    }
    let recoil = recoil_view(ps);
    let view = [
        ps.viewangles[0] - recoil[0],
        ps.viewangles[1] - recoil[1],
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
    // CS:GO `CalcViewModelBobHelper`: the horizontal speed, followed at 640 u/s per second up to
    // 320, paces the cycle and sizes the bob; in the air it bobs a fifth as much.
    let slew = BOB_SPEED_SLEW * dt;
    let speed = ps.velocity[0]
        .hypot(ps.velocity[1])
        .clamp(state.bob_speed - slew, state.bob_speed + slew)
        .clamp(-BOB_MAX_SPEED, BOB_MAX_SPEED);
    state.bob_speed = speed;
    state.bob_time += f64::from(dt * speed / BOB_MAX_SPEED);
    let zoomed = ps.cs_zoom != 0;
    // Running lowers the gun (not scoped).
    let lowered = if zoomed {
        0.0
    } else {
        BOB_LOWER * 0.2 * speed * 0.006
    };
    let period = f64::from((1000.0 - bob_max_speed(index, zoomed)) / 3.5 * 0.001 * BOB_CYCLE_SCALE);
    // The accuracy shift eases in (`Approach` at four times the gap per second).
    let target = accuracy_shift(ps, index, reloading);
    let step = (target - state.gun_accuracy).abs() * dt * 4.0;
    state.gun_accuracy += (target - state.gun_accuracy).clamp(-step, step);
    let shift = if index.and_then(weapon_iw4::cs::cs_weapon).is_some() {
        state.gun_accuracy
    } else {
        0.0
    };
    let on_ground = ps.ground_entity_num != playerstate_iw4::ENTITYNUM_NONE;
    let size = speed * if on_ground { 0.00625 } else { 0.00125 };
    let vertical = (bob_wave(state.bob_time, period, size * BOB_VERT) - (lowered - shift * 0.2))
        .clamp(-7.0, 4.0);
    let lateral =
        (bob_wave(state.bob_time, period * 2.0, size * BOB_LAT) + shift * 0.25).clamp(-8.0, 8.0);

    // `AddViewModelBobHelper`: 40% of the vertical bob forward, 10% up, a fifth of the lateral
    // to the right.
    let world_up = to_view([0.0, 0.0, 1.0]);
    let mut offset: [f32; 3] = core::array::from_fn(|i| world_up[i] * vertical * 0.1);
    offset[0] += vertical * 0.4;
    offset[1] -= lateral * 0.2;

    let source_scale = match format {
        Format::Csgo => Some(CSGO_SWAY_SCALE),
        Format::Source => Some(CSS_SWAY_SCALE),
        Format::GoldSrc | Format::Source2 => None,
    };
    if let Some(scale) = source_scale {
        // CS:S's and CS:GO's (`CPredictedViewModel::CalcViewModelLag`): the gun moves by how far
        // the view's forward has turned since `cl_wpn_sway_interp` ago, against the turn. It
        // follows the view's angles over time, so it trails a turn the same at any frame rate
        // and turning speed, and settles as soon as the turning stops.
        state.lagged_forward = None;
        let turned = turned_since(&mut state.sway_history, now, [view[0], view[1]]);
        let (turned, _, _) = math_iw4::angle_vectors([turned[0], turned[1], 0.0]);
        offset[0] += (1.0 - turned[0]) * scale;
        offset[1] -= turned[1] * scale;
        offset[2] -= turned[2] * scale;
    } else {
        // `CalcViewModelLag`: the gun trails the facing it is catching up to, and looking up or
        // down slides it (back, left and down as the view dips).
        state.sway_history.clear();
        let lagged = state.lagged_forward.get_or_insert(forward);
        let diff: [f32; 3] = core::array::from_fn(|i| forward[i] - lagged[i]);
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
        let drift = to_view(diff);
        for i in 0..3 {
            offset[i] -= drift[i] * SWAY_UNITS;
        }
        let pitch = (view[0] + 180.0).rem_euclid(360.0) - 180.0;
        offset[0] -= pitch * 0.035;
        offset[1] += pitch * 0.03;
        offset[2] -= pitch * 0.02;
    }
    let mut place = view_rotation(
        -vertical * 0.4 * steady,
        -lateral * 0.3 * steady,
        vertical * 0.5 * steady,
    );
    for (row, value) in place.iter_mut().zip(offset) {
        row[3] = value * steady;
    }
    place
}

/// How far the view (`angles`: pitch, yaw) has turned since [`SWAY_INTERP`] before `now`, by
/// the angles `history` holds (taking these in): the angles then are found between the two
/// taken around it, or are the oldest while the history is shorter.
fn turned_since(
    history: &mut std::collections::VecDeque<(f64, [f32; 2])>,
    now: f64,
    angles: [f32; 2],
) -> [f32; 2] {
    // A clock that went back (a new match) starts over.
    if history.back().is_some_and(|(at, _)| *at > now) {
        history.clear();
    }
    history.push_back((now, angles));
    let then = now - SWAY_INTERP;
    while history.len() > 2 && history[1].0 <= then {
        history.pop_front();
    }
    let (from_at, from) = history[0];
    let earlier = match history.get(1) {
        Some(&(to_at, to)) if from_at < then && to_at > from_at => {
            let t = ((then - from_at) / (to_at - from_at)).clamp(0.0, 1.0) as f32;
            [0, 1].map(|i| from[i] + math_iw4::angle_subtract(to[i], from[i]) * t)
        }
        _ => from,
    };
    [0, 1].map(|i| math_iw4::angle_subtract(angles[i], earlier[i]))
}

/// Rotation that undoes the view's recoil punch (pitch, yaw degrees) in GoldSrc view axes.
/// How far the recoil turned the camera (CS 1.6 punch or CS:GO view offset).
fn recoil_view(ps: &PlayerState) -> [f32; 3] {
    weapon_iw4::csgo::view_offset(ps.cs_shooting_mode, ps.cs_punch, ps.cs_view_punch)
}

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
    mut actions: Option<ResMut<net::ClientActionInput>>,
    mut aim_frame: ResMut<CsAimsightFrame>,
) {
    let (view, settings) = view_settings;
    aim_frame.amount = 0.0;
    // `+lookatweapon` this frame (taken whatever is held, so it never waits for a gun).
    let inspect = actions
        .as_mut()
        .is_some_and(|a| std::mem::take(&mut a.client.inspect));
    let reload_down = actions
        .as_ref()
        .is_some_and(|a| a.client.kb.reload.active);
    frame.model = None;
    active.0 = false;
    state.pump(&mut images);
    let Some(weapons) = weapons else {
        publish_aim(0.0, 0.0);
        return;
    };
    let Some(ps) = presented.viewweapon_player(local.0) else {
        state.playing = None;
        state.lagged_forward = None;
        state.sway_history.clear();
        state.aim = 0.0;
        publish_aim(0.0, 0.0);
        return;
    };
    let Some((weapon_index, weapon)) = held_cs_weapon(ps, &weapons) else {
        state.playing = None;
        state.aim = 0.0;
        publish_aim(0.0, 0.0);
        return;
    };
    // The side of whoever's gun this is (the killer's in a killcam), with their agent's arms.
    let viewed = match *view {
        ViewSubject::Seat {
            focus: Some(focus), ..
        } => sim::ClientId(focus),
        _ => local.0,
    };
    // Your knife as you set it; anyone else's (spectated, a killcam) as the server says they
    // carry it.
    let knife = if viewed == local.0 {
        weapon_iw4::cs::csgo_knife_view_model(&settings.knife)
    } else {
        weapon_iw4::cs::csgo_knife_view_model_of_skin(ps.cs_knife_skin)
    };
    let side = match presented.snapshot() {
        Some(snap) => {
            let factions = cs_factions(snap);
            state.follow_sides(
                [true, false].map(|terrorist| CsgoSide::of(terrorist, factions.as_ref())),
                knife,
            );
            let team = snap
                .meta
                .for_client(viewed)
                .map_or(entity_iw4::TEAM_AXIS, |meta| meta.client_state_team);
            CsgoSide::of(terrorist_team(snap, team), factions.as_ref())
        }
        None => CsgoSide::of(true, None),
    };
    let model = match state.model_for(weapon, side, knife) {
        ModelLoad::Ready(model) => model,
        ModelLoad::Loading => {
            active.0 = true;
            publish_aim(0.0, 0.0);
            return;
        }
        ModelLoad::Unavailable => {
            publish_aim(0.0, 0.0);
            return;
        }
    };
    // CS:GO's aim down the AUG's or SG 553's scope while zoomed: in at its `aimsight speed up`
    // of the way a second, back out at its `speed down`; the view's zoom eases with it.
    let aim = aimsight(weapon.name).filter(|_| model.lens.is_some());
    let aiming = aim.is_some() && ps.cs_zoom != 0;
    let dt = time.delta_secs();
    state.aim = match aim {
        Some(aim) if aiming => (state.aim + aim.speed_up * dt).min(1.0),
        Some(aim) => (state.aim - aim.speed_down * dt).max(0.0),
        None => 0.0,
    };
    let aim_in = state.aim * state.aim * (3.0 - 2.0 * state.aim);
    publish_aim(aim_in, aim.map_or(0.0, |aim| aim.fov));
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
    let inspect = inspect
        && state
            .inspect_taken
            .is_none_or(|taken| now - taken >= INSPECT_COMMAND_RATE);
    if inspect {
        state.inspect_taken = Some(now);
    }
    let reload_held = if reload_down {
        now - *state.reload_held_since.get_or_insert(now) >= INSPECT_RELOAD_HOLD
    } else {
        state.reload_held_since = None;
        false
    };
    let reloading = weapon_iw4::WeaponState::from_i32(ps.weaponstate_primary)
        .is_ok_and(weapon_iw4::WeaponState::is_reload_family);
    // Rounds in the clip, read as the HUD's ammo counter reads them: a gun whose clip row is not
    // in the replicated table (the P250's, the Deagle's) counts from the snapshot.
    let clip = weapons.0.facts_of(weapon_index).map_or(0, |facts| {
        let key = weapon_iw4::clip_table_key(facts.clip_index, weapon_index);
        if weapon_iw4::clip_row_present(&ps.ammoclip, key) {
            weapon_iw4::get_clip_for_hand(&ps.ammoclip, key, 0)
        } else {
            presented
                .snapshot()
                .and_then(|s| s.meta.for_client(local.0))
                .map_or(0, |meta| meta.ammo_clip)
        }
    });
    let roles = &model.roles;
    // A gun with a silencer plays its silenced or plain set by the replicated bit.
    let silenced =
        weapon_iw4::cs::silencer_on(ps.cs_silencers, weapon.silencer_bit, ps.cs_shooting_mode);
    let (idle, draw, gun_reload, shoot) = roles.gun(silenced);
    // An empty pistol idles and reloads with its slide back, and its last shot leaves it there.
    let (idle, gun_reload, shoot) = match &roles.empty {
        Some(empty) if clip == 0 => (
            empty.idle.or(idle),
            empty.reload.or(gun_reload),
            empty.shoot.as_ref().map_or(shoot, core::slice::from_ref),
        ),
        _ => (idle, gun_reload, shoot),
    };
    // What a weighted pick of an idle or draw (or an inspect) draws by.
    let roll = ((now * 1000.0) as u64).wrapping_mul(2_654_435_761) as u32 >> 8;
    let start = |playing: &mut Playing, sequence: Option<usize>, rate: f32| {
        fade_out(playing, sequence, &model.sequences, now);
        playing.sequence = sequence;
        playing.started = now;
        playing.rate = rate;
        playing.events_through = None;
    };
    let playing = match state.playing.as_mut() {
        Some(playing) if playing.weapon == weapon_index && Arc::ptr_eq(&playing.model, &model) => {
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
                // CS 1.6 radios every grenade throw (`Radio("%!MRAD_FIREINHOLE")`); under CS:GO
                // the server sends the call to the whole team (`cs_radio`).
                if playing.grenade_state != THROWN
                    && ps.cs_grenade == THROWN
                    && ps.cs_shooting_mode == weapon_iw4::csgo::SHOOTING_CS16
                {
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
                // The Glock in burst mode and the Elites' right gun shoot with the other set, as
                // do the AUG and SG 553 aimed down their scope (CS:GO's `ironsight_fire`).
                let other_mode = (weapon.burst || weapon.dual)
                    && weapon.mode_bit != 0
                    && ps.cs_burst_modes & weapon.mode_bit != 0;
                let alt = (other_mode || aiming) && !roles.shoot_alt.is_empty();
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
            let inspecting = playing.sequence.is_some_and(|s| roles.inspect.contains(&s));
            if inspecting && (ps.cs_zoom != 0 || reload_held) {
                // Zooming in puts the weapon back up, as does holding reload with nothing to
                // reload (a reload that starts plays over the inspect anyway).
                start(playing, roles.pick_idle(idle, roll), 1.0);
            } else if inspect
                && !roles.inspect.is_empty()
                && !reloading
                && ps.cs_zoom == 0
                && !weapon.grenade
                && !playing.arming
                // As CS:GO's `LookAtHeldWeapon`: an inspect still playing is not started over, and
                // one can cut the draw short (the knife out and inspected at once).
                && (playing
                    .sequence
                    .is_some_and(|s| roles.is_idle(s, idle) || roles.is_draw(s, draw))
                    || playing.sequence.is_none_or(|s| {
                        (now - playing.started) as f32 * playing.rate
                            >= model.sequences[s].duration
                    }))
            {
                // CS:GO's `SelectWeightedSequence`: a draw below the weights' total walks the
                // inspects, each taking up its weight (one weighted 0 still taking one).
                let total: u32 = roles.inspect_weights.iter().sum();
                let mut draw = (now * 1000.0) as u32 % total.max(1);
                let mut pick = 0;
                while pick + 1 < roles.inspect.len() {
                    let weight = roles.inspect_weights.get(pick).copied().unwrap_or(1).max(1);
                    if draw < weight {
                        break;
                    }
                    draw -= weight;
                    pick += 1;
                }
                start(playing, Some(roles.inspect[pick]), 1.0);
            }
            playing.last_fire_ms = ps.cs_last_fire_ms;
            playing.reloading = reloading;
            playing.clip = clip;
            playing.silenced = silenced;
            playing
        }
        _ => state.playing.insert({
            let drawn = roles.pick_draw(draw, roll).or(idle);
            // The knife's deploy sound, unless its draw calls its own (every CS:GO knife's does:
            // the Butterfly's flips, the Ursus's catch).
            if weapon.knife && drawn.is_none_or(|s| model.sequences[s].sounds.is_empty()) {
                play_local(&mut sounds, knife_deploy_sound(model.format));
            }
            Playing {
                weapon: weapon_index,
                model: Arc::clone(&model),
                sequence: drawn,
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
                fading: Vec::new(),
            }
        }),
    };
    let flash = playing
        .flash
        .filter(|(started, _)| now - started < FLASH_SECONDS)
        .map(|(started, seed)| (((now - started) / FLASH_SECONDS) as f32, seed));
    // A finished one-shot sequence settles into idle, and a finished idle makes way for the next
    // one picked when the weapon has several (CS:GO's knives); a grenade holds its pulled pin or
    // empty hand until the next step.
    let holding = (weapon.grenade && ps.cs_grenade != playerstate_iw4::cs_grenade::IDLE)
        || (weapon.c4 && playing.arming);
    if let Some(sequence) = playing.sequence
        && (!roles.is_idle(sequence, idle) || roles.idle_varies(idle))
        && !holding
        && (now - playing.started) as f32 * playing.rate > model.sequences[sequence].duration
    {
        start(playing, roles.pick_idle(idle, roll), 1.0);
    }
    let Some(sequence) = playing.sequence else {
        return;
    };
    let info = &model.sequences[sequence];
    let mut seconds = (now - playing.started) as f32 * playing.rate;
    if roles.is_idle(sequence, idle) && info.duration > 0.0 {
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
    // The sequences fading out over this one, the latest first as Source blends them.
    playing.fading.retain(|fade| fade.weight(now) > 0.0);
    let fading: Vec<(usize, f32, f32)> = playing
        .fading
        .iter()
        .rev()
        .map(|fade| {
            let at = (now - fade.started) as f32 * fade.rate;
            (fade.sequence, at, fade.weight(now))
        })
        .collect();

    let cs_index = weapon_iw4::cs::cs_weapon_index_for(&weapons.0.script_name_of(weapon_index));
    let sway = sway_placement(
        &mut state,
        ps,
        cs_index,
        reloading,
        model.format,
        now,
        1.0 - aim_in,
        dt,
    );
    let CsViewmodels { bones, .. } = &mut *state;
    model.studio.pose(sequence, seconds, &fading, bones);
    // A CS2 silencer that is off shrinks to a point, except while it goes on or comes off.
    if let Some(silencer) = &roles.silenced
        && let Some((bone, origin)) = silencer.hidden_bone
        && !silenced
        && silencer.attach != Some(sequence)
        && silencer.detach != Some(sequence)
        && let Some(m) = bones.get_mut(bone)
    {
        let at = [0, 1, 2].map(|r| {
            m[r][0] * origin[0] + m[r][1] * origin[1] + m[r][2] * origin[2] + m[r][3]
        });
        *m = at.map(|t| [0.0, 0.0, 0.0, t]);
    }
    let right_handed = match model.format {
        Format::Source => weapon.css_right_handed,
        Format::GoldSrc => model.goldsrc_right_handed,
        // CS:GO and CS2 build their guns right-handed.
        Format::Csgo | Format::Source2 => true,
    };
    let mut place = mdl_goldsrc::concat(&sway, &unpunch(recoil_view(ps)));
    if let Some(aim) = aim.filter(|_| aim_in > 0.0) {
        let aimed = aim_placement(&aim, aim_in, right_handed != settings.right_hand);
        place = mdl_goldsrc::concat(&place, &aimed);
    }
    match model.format {
        Format::Source2 => place = mdl_goldsrc::concat(&CS2_VIEWMODEL_OFFSET, &place),
        Format::Csgo => {
            // `viewmodel_offset_*` gives way to the aim.
            let mut offset = CSGO_VIEWMODEL_OFFSET;
            for row in &mut offset {
                row[3] *= 1.0 - aim_in;
            }
            place = mdl_goldsrc::concat(&offset, &place);
        }
        Format::Source | Format::GoldSrc => {}
    }
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
        // Source's viewmodel fov is horizontal at 4:3 (widened for wider screens like ours),
        // narrowed by as much as the view's zoom narrows it (`CS_DEFAULT_FOV`): an aimed CS:GO
        // rifle's view eases to its `aimsight fov`.
        Format::Csgo => {
            let zoom = aim.map_or(0.0, |aim| (CS_DEFAULT_FOV - aim.fov) * aim_in);
            let fov = (CSGO_VIEWMODEL_FOV - zoom).max(1.0);
            ((fov * 0.5).to_radians().tan() * 0.75, 1.0)
        }
        Format::Source | Format::Source2 => (
            (settings.viewmodel_fov * 0.5).to_radians().tan() * 0.75,
            1.0,
        ),
    };
    frame.tan_half_fov_y = tan_half_fov_y;
    frame.texture_gamma = texture_gamma;
    // The scope's lens on screen, for the blur around it and the reticle dot.
    if aim_in > 0.0
        && let Some(lens) = model.lens
        && let Some(bone) = frame.bones.get(lens.bone)
    {
        let at = transform(bone, lens.center);
        if at[0] > 1.0 {
            let scale = 1.0 / (at[0] * tan_half_fov_y);
            let [r, g, b, _] = settings.crosshair.color;
            *aim_frame = CsAimsightFrame {
                amount: aim_in,
                lens_center: [-at[1] * scale, at[2] * scale],
                lens_radius: lens.radius * scale,
                dot_color: [r, g, b].map(|c| f32::from(c) / 255.0),
                dot_radius: AIM_DOT,
                vignette: state.vignette.clone(),
            };
        }
    }
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
        tangent: [0.0; 4],
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
