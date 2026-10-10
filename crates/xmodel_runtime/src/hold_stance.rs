//! Counter-Strike's third-person stance for a knife or grenade in hand, put on an MW2 body's
//! arms. The stance is read from a Counter-Strike game's own animation (CS2's `idle_knife`)
//! as joint directions in its torso's frame, with where that torso faces; posing, after the
//! animation tree, turns the MW2 chest to face the same way, then each arm's upper arm, forearm
//! and hand so the MW2 joints point the same ways in its torso's frame, keeping MW2's bone
//! lengths and the rest of the body as animated. The server's hitboxes and every client's bodies pose
//! through [`crate::apply_player_controller`], so both hold the same stance.

use std::sync::OnceLock;

use anim_iw4::Local;
use glam::{Mat3, Mat4, Quat, Vec3};

use crate::DObj;

/// Which stance a body holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldKind {
    Knife,
    Grenade,
}

/// One arm of a stance: unit directions in the torso's frame (x forward, y left, z up) from the
/// shoulder to the elbow, the elbow to the wrist, and the wrist to the middle finger's and the
/// thumb's first joints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArmStance {
    pub upper: Vec3,
    pub fore: Vec3,
    pub hand: Vec3,
    pub thumb: Vec3,
}

/// Both arms of a stance, left then right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoldStance {
    pub arms: [ArmStance; 2],
    /// Where the held weapon points (from its butt to its tip: a knife's blade) in the torso's
    /// frame, when the source animation says.
    pub weapon: Option<Vec3>,
    /// Where the torso faces about the up axis (radians, to the left positive): from where the
    /// body faces as read, and once installed, from where it faces holding the weapon. CS swings
    /// its chest round with a slash or a throw.
    pub yaw: f32,
    /// How far the MW2 spine eases back to its rest pose under the stance (0 to 1): through a
    /// move, so MW2's own melee and throw animations don't bend the torso as well.
    pub straighten: f32,
}

/// An arm's joints in model space: shoulder, elbow, wrist, middle finger and thumb.
pub type ArmJoints = [Vec3; 5];

/// A torso's joints in model space: lower spine, neck, left clavicle, right clavicle.
pub type TorsoJoints = [Vec3; 4];

impl HoldStance {
    /// A stance from a posed skeleton's joints in its model frame (facing +x): its torso, each
    /// arm (left, right), and the weapon's butt and tip. `None` when the joints are degenerate.
    #[must_use]
    pub fn from_joints(
        torso: TorsoJoints,
        arms: [ArmJoints; 2],
        weapon: Option<(Vec3, Vec3)>,
    ) -> Option<Self> {
        let frame = torso_frame(torso)?;
        let into = frame.transpose();
        let arm = |j: &ArmJoints| -> Option<ArmStance> {
            let dir = |from: Vec3, to: Vec3| (into * (to - from)).try_normalize();
            Some(ArmStance {
                upper: dir(j[0], j[1])?,
                fore: dir(j[1], j[2])?,
                hand: dir(j[2], j[3])?,
                thumb: dir(j[2], j[4])?,
            })
        };
        Some(Self {
            arms: [arm(&arms[0])?, arm(&arms[1])?],
            weapon: weapon.and_then(|(butt, tip)| (into * (tip - butt)).try_normalize()),
            yaw: yaw_of(&frame),
            straighten: 0.0,
        })
    }
}

/// A torso's frame (columns forward, left, up): up from the lower spine to the neck, left across
/// the clavicles. The clavicles hang off the top of the spine, so this turns with the chest.
#[must_use]
pub fn torso_frame([lower_spine, neck, left, right]: TorsoJoints) -> Option<Mat3> {
    let up = (neck - lower_spine).try_normalize()?;
    let across = left - right;
    let left = (across - up * across.dot(up)).try_normalize()?;
    Some(Mat3::from_cols(left.cross(up), left, up))
}

/// Where a torso faces about the up axis (radians from +x, to the left positive), from the line
/// of its shoulders, which stays level when the torso bends forward or back.
fn yaw_of(frame: &Mat3) -> f32 {
    // The shoulders' left is (-sin, cos) of the yaw.
    (-frame.y_axis.x).atan2(frame.y_axis.y)
}

/// The MW2 torso's frame in `world` (`dobj`'s posed bones).
fn mw2_torso_frame(dobj: &DObj, world: &[Mat4]) -> Option<Mat3> {
    let pos = |name: &str| Some(world.get(dobj.find(name)?)?.w_axis.truncate());
    torso_frame([
        pos(MW2_TORSO[0])?,
        pos(MW2_TORSO[1])?,
        pos(MW2_TORSO[2])?,
        pos(MW2_TORSO[3])?,
    ])
}

/// Where a body holding `stance` points its weapon, in the frame of `world` (`dobj`'s posed
/// bones): the stance's weapon direction in that body's torso frame.
#[must_use]
pub fn hold_weapon_direction(dobj: &DObj, world: &[Mat4], stance: &HoldStance) -> Option<Vec3> {
    Some(mw2_torso_frame(dobj, world)? * stance.weapon?)
}

/// A hand's frame from where its joints are, whatever its rig's bone axes: x from the wrist to
/// the middle finger's knuckle, y toward the thumb (square to x), z across the back of the hand.
/// A CS weapon model rides on its game's hand bone; this is how that bone lines up with MW2's hand.
#[must_use]
pub fn hand_anatomy(wrist: Vec3, middle: Vec3, thumb: Vec3) -> Option<Mat3> {
    let along = (middle - wrist).try_normalize()?;
    let toward = thumb - wrist;
    let thumb_side = (toward - along * along.dot(toward)).try_normalize()?;
    Some(Mat3::from_cols(along, thumb_side, along.cross(thumb_side)))
}

/// A posed MW2 body's right hand in the frame of `world` (`dobj`'s posed bones): its anatomy (see
/// [`hand_anatomy`]) at the wrist.
#[must_use]
pub fn mw2_right_hand(dobj: &DObj, world: &[Mat4]) -> Option<Mat4> {
    let pos = |name: &str| Some(world.get(dobj.find(name)?)?.w_axis.truncate());
    let [_, _, wrist, middle, thumb] = MW2_ARMS[1];
    let wrist = pos(wrist)?;
    let frame = hand_anatomy(wrist, pos(middle)?, pos(thumb)?)?;
    Some(Mat4::from_cols(
        frame.x_axis.extend(0.0),
        frame.y_axis.extend(0.0),
        frame.z_axis.extend(0.0),
        wrist.extend(1.0),
    ))
}

/// What the arms are doing: holding the knife or a grenade, or one of the moves CS plays with
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldAction {
    KnifeIdle,
    /// A light slash, and the other light slash it alternates with.
    KnifeSlash,
    KnifeSlashB,
    /// The heavy stab.
    KnifeStab,
    GrenadeIdle,
    GrenadeThrow,
}

impl HoldAction {
    pub const COUNT: usize = 6;

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The holding stance this move starts from and goes back to.
    #[must_use]
    pub const fn idle(self) -> Self {
        match self {
            Self::KnifeIdle | Self::KnifeSlash | Self::KnifeSlashB | Self::KnifeStab => {
                Self::KnifeIdle
            }
            Self::GrenadeIdle | Self::GrenadeThrow => Self::GrenadeIdle,
        }
    }
}

/// A stance through a move: its frames spread evenly from start to end.
#[derive(Clone, Debug, PartialEq)]
pub struct HoldClip {
    pub frames: Vec<HoldStance>,
}

impl HoldClip {
    /// The stance `fraction` (0 to 1) of the way through, between the two nearest frames.
    #[must_use]
    pub fn sample(&self, fraction: f32) -> Option<HoldStance> {
        let last = self.frames.len().checked_sub(1)?;
        let at = fraction.clamp(0.0, 1.0) * last as f32;
        let (i, t) = (at.floor() as usize, at.fract());
        let (a, b) = (&self.frames[i.min(last)], &self.frames[(i + 1).min(last)]);
        let mix = |x: Vec3, y: Vec3| x.lerp(y, t).normalize_or(x);
        let arm = |x: &ArmStance, y: &ArmStance| ArmStance {
            upper: mix(x.upper, y.upper),
            fore: mix(x.fore, y.fore),
            hand: mix(x.hand, y.hand),
            thumb: mix(x.thumb, y.thumb),
        };
        Some(HoldStance {
            arms: [arm(&a.arms[0], &b.arms[0]), arm(&a.arms[1], &b.arms[1])],
            weapon: match (a.weapon, b.weapon) {
                (Some(x), Some(y)) => Some(mix(x, y)),
                (x, y) => x.or(y),
            },
            yaw: a.yaw + angle_between(a.yaw, b.yaw) * t,
            straighten: a.straighten + (b.straighten - a.straighten) * t,
        })
    }
}

static CLIPS: OnceLock<[Option<HoldClip>; HoldAction::COUNT]> = OnceLock::new();

/// Make the stances available to posing, one clip per [`HoldAction`] (by its index). Each
/// frame's torso yaw becomes how far it turns from its holding stance's. The first call wins;
/// later ones are ignored (returns whether this one was taken).
pub fn install_hold_clips(mut clips: [Option<HoldClip>; HoldAction::COUNT]) -> bool {
    let holding = |clips: &[Option<HoldClip>], idle: HoldAction| {
        clips[idle.index()]
            .as_ref()
            .and_then(|clip| clip.frames.first())
            .map_or(0.0, |frame| frame.yaw)
    };
    let from = [HoldAction::KnifeIdle, HoldAction::GrenadeIdle].map(|idle| holding(&clips, idle));
    for (i, clip) in clips.iter_mut().enumerate() {
        let Some(clip) = clip else { continue };
        let idle = if i < HoldAction::GrenadeIdle.index() {
            from[0]
        } else {
            from[1]
        };
        for frame in &mut clip.frames {
            frame.yaw = angle_between(idle, frame.yaw);
        }
    }
    CLIPS.set(clips).is_ok()
}

fn clip(action: HoldAction) -> Option<&'static HoldClip> {
    CLIPS.get()?.get(action.index())?.as_ref()
}

/// The installed holding stance for `kind` (its idle's first frame), if one was read.
#[must_use]
pub fn hold_stance(kind: HoldKind) -> Option<HoldStance> {
    let idle = match kind {
        HoldKind::Knife => HoldAction::KnifeIdle,
        HoldKind::Grenade => HoldAction::GrenadeIdle,
    };
    clip(idle)?.sample(0.0)
}

/// Where the arms are: a move and how far through it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoldPose {
    pub action: HoldAction,
    pub fraction: f32,
}

/// The stance for `pose`: its move's clip, or the holding stance when the move wasn't read.
#[must_use]
pub fn sample_hold(pose: HoldPose) -> Option<HoldStance> {
    let mut stance = clip(pose.action)
        .or_else(|| clip(pose.action.idle()))?
        .sample(pose.fraction)?;
    if pose.action != pose.action.idle() {
        // Eased in at the move's start, so the torso doesn't snap; MW2's animation ends bent and
        // blends itself out.
        stance.straighten = (pose.fraction / STRAIGHTEN_EASE).clamp(0.0, 1.0);
    }
    Some(stance)
}

/// How much of a move's start the spine takes to ease into its rest pose.
const STRAIGHTEN_EASE: f32 = 0.15;

/// The arms of a body holding a CS knife or grenade (`kind`), from the MW2 animations it plays:
/// its legs' (whole body's) animation and, when another plays on the torso alone, that one,
/// each with how far through it is, and the torso's restart toggle (which flips with every
/// attack, so slashes alternate; the first one after holding finds it set). The knife's MW2 melee animations become CS's slashes and stab, a
/// grenade throw becomes CS's throw; through any other torso animation (a flinch, a draw) the
/// arms keep holding the weapon, as CS's do. Dying or lying down keeps MW2's arms (`None`).
#[must_use]
pub fn hold_pose_from_anims(
    kind: HoldKind,
    legs: (&str, f32),
    torso: Option<(&str, f32, bool)>,
) -> Option<HoldPose> {
    let pose = |action, fraction| Some(HoldPose { action, fraction });
    // Lying down, CS has no stance to borrow; dying, the body goes limp its own way.
    if legs.0.contains("prone") || legs.0.contains("death") {
        return None;
    }
    match kind {
        HoldKind::Grenade => {
            if let Some((name, fraction, _)) = torso
                && name.contains("grenade_throw")
            {
                return pose(HoldAction::GrenadeThrow, fraction);
            }
            if legs.0.contains("grenade_throw") {
                return pose(HoldAction::GrenadeThrow, legs.1);
            }
            pose(HoldAction::GrenadeIdle, 0.0)
        }
        HoldKind::Knife => match torso {
            Some((name, fraction, toggle)) if name.contains("melee") => {
                let action = if name.ends_with("_2") {
                    HoldAction::KnifeStab
                } else if toggle {
                    HoldAction::KnifeSlash
                } else {
                    HoldAction::KnifeSlashB
                };
                pose(action, fraction)
            }
            _ => pose(HoldAction::KnifeIdle, 0.0),
        },
    }
}

/// How far round from angle `a` to angle `b` the short way (radians).
fn angle_between(a: f32, b: f32) -> f32 {
    let d = (b - a).rem_euclid(std::f32::consts::TAU);
    if d > std::f32::consts::PI {
        d - std::f32::consts::TAU
    } else {
        d
    }
}

/// MW2 body joints: the torso (lower spine, neck, clavicles); the spine joints that turn the
/// chest, bottom up; and per arm (left, right) the shoulder, elbow, wrist, middle finger and thumb.
const MW2_TORSO: [&str; 4] = ["j_spinelower", "j_neck", "j_clavicle_le", "j_clavicle_ri"];
const MW2_SPINE: [&str; 3] = ["j_spinelower", "j_spineupper", "j_spine4"];
/// The MW2 spine, neck and head joints MW2's torso animations bend (the neck and head to keep
/// looking ahead of a bent spine), which a move eases back to rest.
const MW2_SPINE_REST: [&str; 6] = [
    "torso_stabilizer",
    "j_spinelower",
    "j_spineupper",
    "j_spine4",
    "j_neck",
    "j_head",
];
const MW2_ARMS: [[&str; 5]; 2] = [
    [
        "j_shoulder_le",
        "j_elbow_le",
        "j_wrist_le",
        "j_mid_le_1",
        "j_thumb_le_1",
    ],
    [
        "j_shoulder_ri",
        "j_elbow_ri",
        "j_wrist_ri",
        "j_mid_ri_1",
        "j_thumb_ri_1",
    ],
];

/// The rotation taking `a` onto `b` and the plane of `a` and `a2` onto that of `b` and `b2`
/// (only `a` onto `b` when either pair is near parallel).
fn align_pair(a: Vec3, a2: Vec3, b: Vec3, b2: Vec3) -> Quat {
    let frame = |x: Vec3, y: Vec3| -> Option<Mat3> {
        let x = x.try_normalize()?;
        let y = (y - x * x.dot(y)).try_normalize()?;
        Some(Mat3::from_cols(x, y, x.cross(y)))
    };
    match (frame(a, a2), frame(b, b2)) {
        (Some(from), Some(to)) => Quat::from_mat3(&(to * from.transpose())).normalize(),
        _ => Quat::from_rotation_arc(a.normalize_or_zero(), b.normalize_or_zero()),
    }
}

/// Turn the arms in `locals` (local rotations after the animation tree) into `stance`.
/// Leaves `locals` alone when the body lacks a joint the stance needs.
pub fn apply_hold_stance(dobj: &DObj, locals: &mut [Local], stance: &HoldStance) {
    let count = locals.len();
    let find = |name: &str| dobj.find(name).filter(|&b| b < count);
    let Some(arms) = MW2_ARMS
        .iter()
        .map(|arm| arm.iter().map(|n| find(n)).collect::<Option<Vec<_>>>())
        .collect::<Option<Vec<_>>>()
    else {
        return;
    };
    // Through a move the spine eases back to rest, then the chest turns from where it faces as
    // far as the stance's turns from holding the weapon, spread up the spine.
    if stance.straighten > 0.0 {
        for bone in MW2_SPINE_REST.iter().filter_map(|n| find(n)) {
            let [x, y, z, w] = locals[bone].rotation;
            let rest = dobj.bones[bone].bind_rotation;
            let q = Quat::from_xyzw(x, y, z, w)
                .slerp(rest, stance.straighten)
                .normalize();
            locals[bone].rotation = [q.x, q.y, q.z, q.w];
        }
    }
    let world = dobj.compose(locals, Mat4::IDENTITY);
    if let Some(spine) = MW2_SPINE
        .iter()
        .map(|n| find(n))
        .collect::<Option<Vec<_>>>()
        && stance.yaw != 0.0
    {
        let share = stance.yaw.clamp(-MAX_CHEST_TURN, MAX_CHEST_TURN) / spine.len() as f32;
        let turns: Vec<(usize, Quat)> = spine
            .iter()
            .enumerate()
            .map(|(i, &bone)| (bone, Quat::from_rotation_z(share * (i + 1) as f32)))
            .collect();
        turn_bones(dobj, locals, &world, &turns);
    }
    let world = dobj.compose(locals, Mat4::IDENTITY);
    let pos = |b: usize| world[b].w_axis.truncate();
    let Some(frame) = mw2_torso_frame(dobj, &world) else {
        return;
    };
    for (joints, target) in arms.iter().zip(&stance.arms) {
        let [shoulder, elbow, wrist, middle, thumb] =
            [joints[0], joints[1], joints[2], joints[3], joints[4]];
        let (upper, fore) = (pos(elbow) - pos(shoulder), pos(wrist) - pos(elbow));
        let (t_upper, t_fore) = (frame * target.upper, frame * target.fore);
        // The upper arm points at the stance's elbow, bending in the stance's plane; the forearm
        // then turns about the elbow onto the stance's wrist; the hand turns last.
        let q_upper = align_pair(upper, upper.cross(fore), t_upper, t_upper.cross(t_fore));
        let q_fore = Quat::from_rotation_arc(
            (q_upper * fore).normalize_or_zero(),
            t_fore.normalize_or_zero(),
        );
        let arm_turn = q_fore * q_upper;
        let hand = arm_turn * (pos(middle) - pos(wrist));
        let hand_thumb = arm_turn * (pos(thumb) - pos(wrist));
        let q_hand = align_pair(hand, hand_thumb, frame * target.hand, frame * target.thumb);
        turn_bones(
            dobj,
            locals,
            &world,
            &[
                (shoulder, q_upper),
                (elbow, arm_turn),
                (wrist, (q_hand * arm_turn).normalize()),
            ],
        );
    }
    // MW2's animations carry the weapon tag away from the hand to where their own weapon sits;
    // held the CS way it goes in the right palm (between the wrist and the middle finger's
    // knuckle), turned as the bind pose turns it on the wrist.
    if let (Some(tag), Some(right)) = (
        dobj.find(WEAPON_TAG).filter(|&b| b < locals.len()),
        arms.get(1),
    ) && dobj.bones[tag].parent == Some(right[2])
    {
        let posed = dobj.compose(locals, Mat4::IDENTITY);
        let (wrist, knuckle) = (posed[right[2]], posed[right[3]].w_axis.truncate());
        let palm = wrist.w_axis.truncate().lerp(knuckle, PALM_ALONG_HAND);
        let bone = &dobj.bones[tag];
        let scale = if bone.no_scale {
            1.0
        } else {
            dobj.models[bone.model].scale
        };
        let local = wrist.inverse().transform_point3(palm);
        let delta = (local - bone.bind_translation) / scale.max(1e-6);
        let q = bone.bind_rotation;
        locals[tag].rotation = [q.x, q.y, q.z, q.w];
        locals[tag].translation = delta.to_array();
        locals[tag].control = false;
    }
}

/// Turn joints of a posed body (`world`, from `locals`) in world space, each turn taking the joint
/// and everything under it: sets their `locals` to match. A bone ends up turned as its nearest
/// turned ancestor (itself included), as bones between the joints (twist bones) keep their local
/// rotations and follow.
fn turn_bones(dobj: &DObj, locals: &mut [Local], world: &[Mat4], turns: &[(usize, Quat)]) {
    let rot = |b: usize| world[b].to_scale_rotation_translation().1;
    let turned = |bone: usize| -> Quat {
        let mut at = Some(bone);
        while let Some(b) = at {
            if let Some((_, q)) = turns.iter().find(|(joint, _)| *joint == b) {
                return *q;
            }
            at = dobj.bones[b].parent;
        }
        Quat::IDENTITY
    };
    for &(bone, turn) in turns {
        let world_rotation = (turn * rot(bone)).normalize();
        let parent_rotation = dobj.bones[bone]
            .parent
            .map_or(Quat::IDENTITY, |p| (turned(p) * rot(p)).normalize());
        let local = (parent_rotation.inverse() * world_rotation).normalize();
        locals[bone].rotation = [local.x, local.y, local.z, local.w];
        locals[bone].control = false;
    }
}

/// The farthest the chest turns from the hips (radians).
const MAX_CHEST_TURN: f32 = 1.4;

/// The tag the right hand's weapon hangs off.
const WEAPON_TAG: &str = "tag_weapon_right";
/// How far from the wrist to the middle finger's knuckle the palm's grip is.
const PALM_ALONG_HAND: f32 = 0.6;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mw2_animations_pick_the_cs_move() {
        let knife = HoldKind::Knife;
        let idle = hold_pose_from_anims(knife, ("pb_stand_alert_pistol", 0.3), None);
        assert_eq!(idle.map(|p| p.action), Some(HoldAction::KnifeIdle));
        let slash = hold_pose_from_anims(
            knife,
            ("pb_stand_alert_pistol", 0.3),
            Some(("pt_melee_pistol_1", 0.4, true)),
        );
        assert_eq!(
            slash,
            Some(HoldPose {
                action: HoldAction::KnifeSlash,
                fraction: 0.4
            })
        );
        let other = hold_pose_from_anims(
            knife,
            ("pb_stand_alert_pistol", 0.3),
            Some(("pt_melee_pistol_1", 0.4, false)),
        );
        assert_eq!(other.map(|p| p.action), Some(HoldAction::KnifeSlashB));
        let stab = hold_pose_from_anims(
            knife,
            ("pb_stand_alert_pistol", 0.3),
            Some(("pt_melee_pistol_2", 0.1, false)),
        );
        assert_eq!(stab.map(|p| p.action), Some(HoldAction::KnifeStab));
        // A flinch (or a draw) keeps the knife held; dying lets go.
        let flinch = hold_pose_from_anims(
            knife,
            ("pb_stand_alert_pistol", 0.3),
            Some(("pt_flinch_pistol_back", 0.1, false)),
        );
        assert_eq!(flinch.map(|p| p.action), Some(HoldAction::KnifeIdle));
        let dying = hold_pose_from_anims(knife, ("pb_stand_death_chest_spin", 0.1), None);
        assert_eq!(dying, None);
        let grenade = HoldKind::Grenade;
        let thrown = hold_pose_from_anims(grenade, ("pb_stand_grenade_throw", 0.5), None);
        assert_eq!(thrown.map(|p| p.action), Some(HoldAction::GrenadeThrow));
        let moving = hold_pose_from_anims(
            grenade,
            ("pb_combatrun_forward_loop_grenade", 0.5),
            Some(("pt_stand_grenade_throw", 0.2, false)),
        );
        assert_eq!(moving.map(|p| p.fraction), Some(0.2));
    }

    #[test]
    fn torso_frame_faces_ahead_of_the_shoulders() {
        // Upright, shoulders across y: facing +x, no turn.
        let frame = torso_frame([
            Vec3::ZERO,
            Vec3::Z * 20.0,
            Vec3::new(0.0, 4.0, 18.0),
            Vec3::new(0.0, -4.0, 18.0),
        ])
        .expect("frame");
        assert!((frame.x_axis - Vec3::X).length() < 1e-5);
        assert!(yaw_of(&frame).abs() < 1e-5);
        // Turned a quarter to the left and bent forward: the yaw holds.
        let turn = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2) * Quat::from_rotation_y(0.6);
        let bent = torso_frame([
            Vec3::ZERO,
            turn * (Vec3::Z * 20.0),
            turn * Vec3::new(0.0, 4.0, 18.0),
            turn * Vec3::new(0.0, -4.0, 18.0),
        ])
        .expect("frame");
        assert!((yaw_of(&bent) - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    }

    #[test]
    fn angles_go_the_short_way_round() {
        assert!(
            (angle_between(170f32.to_radians(), (-170f32).to_radians()) - 20f32.to_radians()).abs()
                < 1e-5
        );
        assert!((angle_between(0.3, -0.2) + 0.5).abs() < 1e-5);
    }

    #[test]
    fn align_pair_maps_both_directions() {
        let q = align_pair(Vec3::X, Vec3::Y, Vec3::Y, -Vec3::X);
        assert!((q * Vec3::X - Vec3::Y).length() < 1e-5);
        assert!((q * Vec3::Y + Vec3::X).length() < 1e-5);
    }
}
