//! Counter-Strike's third-person stance for a knife or grenade in hand, put on an MW2 body's
//! arms. The stance is read from a Counter-Strike game's own animation (CS2's `idle_knife`)
//! as joint directions in the torso's frame; posing, after the animation tree, turns each arm's
//! upper arm, forearm and hand so the MW2 joints point the same ways, keeping MW2's bone lengths
//! and the rest of the body as animated. The server's hitboxes and every client's bodies pose
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

/// One arm of a stance: unit directions in the torso frame (x forward, y left, z up) from the
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
}

/// An arm's joints in model space: shoulder, elbow, wrist, middle finger and thumb.
pub type ArmJoints = [Vec3; 5];

impl HoldStance {
    /// A stance from a posed skeleton's joints: the torso frame from its lower spine, neck and
    /// clavicles, then each arm (left, right) in it. `None` when the joints are degenerate.
    #[must_use]
    pub fn from_joints(
        lower_spine: Vec3,
        neck: Vec3,
        clavicles: [Vec3; 2],
        arms: [ArmJoints; 2],
    ) -> Option<Self> {
        let into = torso_frame(lower_spine, neck, clavicles[0], clavicles[1])?.transpose();
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
        })
    }
}

/// The torso's frame (columns forward, left, up): up from the lower spine to the neck, left
/// across the clavicles.
#[must_use]
pub fn torso_frame(lower_spine: Vec3, neck: Vec3, left: Vec3, right: Vec3) -> Option<Mat3> {
    let up = (neck - lower_spine).try_normalize()?;
    let across = left - right;
    let left = (across - up * across.dot(up)).try_normalize()?;
    Some(Mat3::from_cols(left.cross(up), left, up))
}

static STANCES: OnceLock<[Option<HoldStance>; 2]> = OnceLock::new();

/// Make the knife and grenade stances available to posing. The first call wins; later ones are
/// ignored (returns whether this one was taken).
pub fn install_hold_stances(knife: Option<HoldStance>, grenade: Option<HoldStance>) -> bool {
    STANCES.set([knife, grenade]).is_ok()
}

/// The installed stance for `kind`, if one was read.
#[must_use]
pub fn hold_stance(kind: HoldKind) -> Option<HoldStance> {
    let stances = STANCES.get()?;
    match kind {
        HoldKind::Knife => stances[0],
        HoldKind::Grenade => stances[1],
    }
}

/// MW2 body joints: lower spine, neck, clavicles, and per arm (left, right) the shoulder,
/// elbow, wrist, middle finger and thumb.
const MW2_TORSO: [&str; 4] = ["j_spinelower", "j_neck", "j_clavicle_le", "j_clavicle_ri"];
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
    let find = |name: &str| dobj.find(name).filter(|&b| b < locals.len());
    let Some(torso) = MW2_TORSO
        .iter()
        .map(|n| find(n))
        .collect::<Option<Vec<_>>>()
    else {
        return;
    };
    let Some(arms) = MW2_ARMS
        .iter()
        .map(|arm| arm.iter().map(|n| find(n)).collect::<Option<Vec<_>>>())
        .collect::<Option<Vec<_>>>()
    else {
        return;
    };
    let world = dobj.compose(locals, Mat4::IDENTITY);
    let pos = |b: usize| world[b].w_axis.truncate();
    let rot = |b: usize| world[b].to_scale_rotation_translation().1;
    let Some(frame) = torso_frame(pos(torso[0]), pos(torso[1]), pos(torso[2]), pos(torso[3]))
    else {
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
        let new_world = [
            (shoulder, (q_upper * rot(shoulder)).normalize()),
            (elbow, (arm_turn * rot(elbow)).normalize()),
            (wrist, (q_hand * arm_turn * rot(wrist)).normalize()),
        ];
        for &(bone, world_rotation) in &new_world {
            let parent_rotation = dobj.bones[bone].parent.map_or(Quat::IDENTITY, |p| {
                new_world
                    .iter()
                    .find(|(b, _)| *b == p)
                    .map_or_else(|| rot(p), |(_, q)| *q)
            });
            let local = (parent_rotation.inverse() * world_rotation).normalize();
            locals[bone].rotation = [local.x, local.y, local.z, local.w];
            locals[bone].control = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torso_frame_is_forward_left_up() {
        let frame = torso_frame(
            Vec3::new(0.0, 0.0, 40.0),
            Vec3::new(0.0, 0.0, 60.0),
            Vec3::new(0.0, 5.0, 58.0),
            Vec3::new(0.0, -5.0, 58.0),
        )
        .expect("frame");
        assert!((frame.x_axis - Vec3::X).length() < 1e-5);
        assert!((frame.y_axis - Vec3::Y).length() < 1e-5);
        assert!((frame.z_axis - Vec3::Z).length() < 1e-5);
    }

    #[test]
    fn align_pair_maps_both_directions() {
        let q = align_pair(Vec3::X, Vec3::Y, Vec3::Y, -Vec3::X);
        assert!((q * Vec3::X - Vec3::Y).length() < 1e-5);
        assert!((q * Vec3::Y + Vec3::X).length() < 1e-5);
    }
}
