//! A CS:GO player's hit boxes in the world and a shot traced against them as CS:GO's server
//! traces one (`TraceToStudioCsgoHitgroupsPriority`): each hit box a capsule (or, without a
//! radius, a box) on its bone, the nearest hit kept per kind of hit group, and the kind that
//! counts first taking the shot, so a bullet through an arm into the head is a headshot.

use glam::{Mat3, Mat4, Vec3};
use mdl_goldsrc::Mat3x4;

use crate::agent::{Agent, mat4};

// `HITGROUP_*` (`shareddefs.h`).
pub const HITGROUP_GENERIC: i32 = 0;
pub const HITGROUP_HEAD: i32 = 1;
pub const HITGROUP_CHEST: i32 = 2;
pub const HITGROUP_STOMACH: i32 = 3;
pub const HITGROUP_LEFTARM: i32 = 4;
pub const HITGROUP_RIGHTARM: i32 = 5;
pub const HITGROUP_LEFTLEG: i32 = 6;
pub const HITGROUP_RIGHTLEG: i32 = 7;
/// Counts as the chest but does not stop a headshot.
pub const HITGROUP_NECK: i32 = 8;

/// A hit box placed in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HitShape {
    /// The points within `radius` of the segment `a`-`b`.
    Capsule { a: Vec3, b: Vec3, radius: f32 },
    /// The box `min`..`max` in the frame `frame` (rotation and origin in the world).
    Box { frame: Mat4, min: Vec3, max: Vec3 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldHitbox {
    /// `HITGROUP_*`.
    pub group: i32,
    pub shape: HitShape,
}

/// A rotation from pitch, yaw and roll in degrees (`AngleMatrix`: yaw, then pitch, then roll).
#[must_use]
pub fn angle_matrix(angles: [f32; 3]) -> Mat3 {
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sy, cy) = angles[1].to_radians().sin_cos();
    let (sr, cr) = angles[2].to_radians().sin_cos();
    Mat3::from_cols(
        Vec3::new(cp * cy, cp * sy, -sp),
        Vec3::new(sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, sr * cp),
        Vec3::new(cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp),
    )
}

/// `agent`'s hit boxes on its bones `bones` (model space, as `StudioModel::world_from_locals`
/// gives them), the model placed at `world_from_model`. Each box's frame is its bone's turned
/// by the box's own angles.
#[must_use]
pub fn world_hitboxes(agent: &Agent, bones: &[Mat3x4], world_from_model: Mat4) -> Vec<WorldHitbox> {
    agent
        .model
        .hitboxes
        .iter()
        .filter_map(|hitbox| {
            let bone = world_from_model * mat4(bones.get(hitbox.bone)?);
            let frame = bone * Mat4::from_mat3(angle_matrix(hitbox.angle_offset));
            let shape = if hitbox.capsule_radius > 0.0 {
                HitShape::Capsule {
                    a: frame.transform_point3(Vec3::from(hitbox.min)),
                    b: frame.transform_point3(Vec3::from(hitbox.max)),
                    radius: hitbox.capsule_radius,
                }
            } else {
                HitShape::Box {
                    frame,
                    min: Vec3::from(hitbox.min),
                    max: Vec3::from(hitbox.max),
                }
            };
            Some(WorldHitbox {
                group: hitbox.group,
                shape,
            })
        })
        .collect()
}

/// The box around every hit box, for a quick miss before tracing each.
#[must_use]
pub fn bounds(hitboxes: &[WorldHitbox]) -> Option<(Vec3, Vec3)> {
    let mut out: Option<(Vec3, Vec3)> = None;
    let mut grow = |lo: Vec3, hi: Vec3| {
        out = Some(out.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))));
    };
    for hitbox in hitboxes {
        match hitbox.shape {
            HitShape::Capsule { a, b, radius } => {
                grow(a.min(b) - Vec3::splat(radius), a.max(b) + Vec3::splat(radius));
            }
            HitShape::Box { frame, min, max } => {
                for corner in 0..8 {
                    let p = Vec3::new(
                        if corner & 1 == 0 { min.x } else { max.x },
                        if corner & 2 == 0 { min.y } else { max.y },
                        if corner & 4 == 0 { min.z } else { max.z },
                    );
                    let p = frame.transform_point3(p);
                    grow(p, p);
                }
            }
        }
    }
    out
}

/// Where a shot meets a hit box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitboxHit {
    /// Which hit box (an index into the traced list).
    pub index: usize,
    pub group: i32,
    /// How far along the shot (0 at its start, 1 at its end).
    pub fraction: f32,
    pub normal: Vec3,
    pub start_solid: bool,
}

/// The first time (0..1 of `delta`) the segment from `start` meets the capsule, and the surface
/// normal there; time 0 when it starts inside.
fn ray_capsule(start: Vec3, delta: Vec3, a: Vec3, b: Vec3, radius: f32) -> Option<(f32, Vec3, bool)> {
    let axis = b - a;
    let length = axis.length();
    let dir = if length > 0.0 { axis / length } else { Vec3::Z };
    let closest = |p: Vec3| a + dir * (p - a).dot(dir).clamp(0.0, length);
    let normal_at = |p: Vec3| (p - closest(p)).normalize_or(-delta.normalize_or(Vec3::Z));
    let rr = radius * radius;
    if (start - closest(start)).length_squared() < rr {
        return Some((0.0, normal_at(start), true));
    }
    let mut best: Option<f32> = None;
    let mut keep = |t: f32| {
        if (0.0..=1.0).contains(&t) && best.is_none_or(|b| t < b) {
            best = Some(t);
        }
    };
    // The side: a cylinder around the segment, between its ends.
    let m = start - a;
    let d_perp = delta - dir * delta.dot(dir);
    let m_perp = m - dir * m.dot(dir);
    let qa = d_perp.length_squared();
    if qa > f32::EPSILON {
        let qb = 2.0 * m_perp.dot(d_perp);
        let qc = m_perp.length_squared() - rr;
        let disc = qb * qb - 4.0 * qa * qc;
        if disc >= 0.0 {
            let t = (-qb - disc.sqrt()) / (2.0 * qa);
            let along = (m + delta * t).dot(dir);
            if (0.0..=length).contains(&along) {
                keep(t);
            }
        }
    }
    // The ends: a sphere at each.
    let qa = delta.length_squared();
    if qa > f32::EPSILON {
        for center in [a, b] {
            let m = start - center;
            let qb = 2.0 * m.dot(delta);
            let qc = m.length_squared() - rr;
            let disc = qb * qb - 4.0 * qa * qc;
            if disc >= 0.0 {
                keep((-qb - disc.sqrt()) / (2.0 * qa));
            }
        }
    }
    let t = best?;
    Some((t, normal_at(start + delta * t), false))
}

/// The first time (0..1 of `delta`) the segment from `start` enters the box, and the face's
/// normal; time 0 when it starts inside.
fn ray_box(start: Vec3, delta: Vec3, frame: Mat4, min: Vec3, max: Vec3) -> Option<(f32, Vec3, bool)> {
    let inverse = frame.inverse();
    let s = inverse.transform_point3(start);
    let d = inverse.transform_vector3(delta);
    let mut enter = f32::NEG_INFINITY;
    let mut leave = f32::INFINITY;
    let mut face = Vec3::ZERO;
    for axis in 0..3 {
        if d[axis].abs() < f32::EPSILON {
            if s[axis] < min[axis] || s[axis] > max[axis] {
                return None;
            }
            continue;
        }
        let (mut t0, mut t1) = ((min[axis] - s[axis]) / d[axis], (max[axis] - s[axis]) / d[axis]);
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        if t0 > enter {
            enter = t0;
            face = Vec3::ZERO;
            face[axis] = -d[axis].signum();
        }
        leave = leave.min(t1);
    }
    if enter > leave || leave < 0.0 || enter > 1.0 {
        return None;
    }
    let inside = (0..3).all(|axis| s[axis] >= min[axis] && s[axis] <= max[axis]);
    if inside {
        return Some((0.0, frame.transform_vector3(face).normalize_or(Vec3::Z), true));
    }
    Some((enter.max(0.0), frame.transform_vector3(face).normalize_or(Vec3::Z), false))
}

/// The kinds of hit group, in the order a shot counts them (`EHitGroupType_t`).
const HEAD: usize = 0;
const STOMACH: usize = 1;
const CHEST: usize = 2;
const NECK: usize = 3;
const ARMS: usize = 4;
const GENERAL: usize = 5;
const LEGS: usize = 6;
const KINDS: usize = 7;

fn kind(group: i32) -> usize {
    match group {
        HITGROUP_HEAD => HEAD,
        HITGROUP_NECK => NECK,
        HITGROUP_CHEST => CHEST,
        HITGROUP_STOMACH => STOMACH,
        HITGROUP_LEFTARM | HITGROUP_RIGHTARM => ARMS,
        HITGROUP_LEFTLEG | HITGROUP_RIGHTLEG => LEGS,
        _ => GENERAL,
    }
}

/// A shot from `start` to `end` against `hitboxes`, as CS:GO's server traces it: the nearest hit
/// of each kind of hit group, a headshot dropped when the shot met the stomach or chest first,
/// then the first kind hit in the order head, stomach, chest, neck, arms, the rest, legs.
#[must_use]
pub fn trace(hitboxes: &[WorldHitbox], start: Vec3, end: Vec3) -> Option<HitboxHit> {
    let delta = end - start;
    let mut kinds: [Option<HitboxHit>; KINDS] = [None; KINDS];
    for (index, hitbox) in hitboxes.iter().enumerate() {
        let hit = match hitbox.shape {
            HitShape::Capsule { a, b, radius } => ray_capsule(start, delta, a, b, radius),
            HitShape::Box { frame, min, max } => ray_box(start, delta, frame, min, max),
        };
        let Some((fraction, normal, start_solid)) = hit else {
            continue;
        };
        let slot = &mut kinds[kind(hitbox.group)];
        if slot.is_none_or(|kept| fraction < kept.fraction) {
            *slot = Some(HitboxHit {
                index,
                group: hitbox.group,
                fraction,
                normal,
                start_solid,
            });
        }
    }
    if let Some(head) = kinds[HEAD]
        && kinds[STOMACH..=CHEST]
            .iter()
            .flatten()
            .any(|body| body.fraction < head.fraction)
    {
        kinds[HEAD] = None;
    }
    kinds.into_iter().flatten().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capsule_hit_from_outside_and_inside() {
        let (a, b) = (Vec3::ZERO, Vec3::new(0.0, 0.0, 10.0));
        let hit = ray_capsule(Vec3::new(-10.0, 0.0, 5.0), Vec3::new(20.0, 0.0, 0.0), a, b, 2.0);
        let (t, normal, inside) = hit.expect("crosses the side");
        assert!((t - 0.4).abs() < 1e-5, "{t}");
        assert!((normal - Vec3::NEG_X).length() < 1e-5);
        assert!(!inside);
        let cap = ray_capsule(Vec3::new(0.0, 0.0, 20.0), Vec3::new(0.0, 0.0, -20.0), a, b, 2.0);
        assert!((cap.expect("meets the top").0 - 0.4).abs() < 1e-5);
        assert!(ray_capsule(Vec3::new(1.0, 0.0, 5.0), Vec3::X, a, b, 2.0).is_some_and(|h| h.2));
        assert!(ray_capsule(Vec3::new(-10.0, 5.0, 5.0), Vec3::new(20.0, 0.0, 0.0), a, b, 2.0).is_none());
    }

    #[test]
    fn box_hit_turned() {
        let frame = Mat4::from_rotation_translation(glam::Quat::from_rotation_z(0.5), Vec3::new(5.0, 0.0, 0.0));
        let hit = ray_box(
            Vec3::new(-10.0, 0.0, 0.0),
            Vec3::new(30.0, 0.0, 0.0),
            frame,
            Vec3::splat(-1.0),
            Vec3::splat(1.0),
        );
        assert!(hit.is_some_and(|(t, _, inside)| t > 0.0 && t < 0.5 && !inside));
    }

    #[test]
    fn head_behind_an_arm_counts_but_not_behind_the_chest() {
        let capsule = |x: f32, group: i32| WorldHitbox {
            group,
            shape: HitShape::Capsule {
                a: Vec3::new(x, 0.0, -1.0),
                b: Vec3::new(x, 0.0, 1.0),
                radius: 1.0,
            },
        };
        let start = Vec3::new(-10.0, 0.0, 0.0);
        let end = Vec3::new(10.0, 0.0, 0.0);
        let arm_then_head = [capsule(0.0, HITGROUP_LEFTARM), capsule(4.0, HITGROUP_HEAD)];
        assert_eq!(trace(&arm_then_head, start, end).map(|h| h.group), Some(HITGROUP_HEAD));
        let chest_then_head = [capsule(0.0, HITGROUP_CHEST), capsule(4.0, HITGROUP_HEAD)];
        assert_eq!(trace(&chest_then_head, start, end).map(|h| h.group), Some(HITGROUP_CHEST));
        let neck_then_head = [capsule(0.0, HITGROUP_NECK), capsule(4.0, HITGROUP_HEAD)];
        assert_eq!(trace(&neck_then_head, start, end).map(|h| h.group), Some(HITGROUP_HEAD));
    }
}
