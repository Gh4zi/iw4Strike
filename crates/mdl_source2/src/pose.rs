//! Posing models from animation clips: bone matrices from local transforms, skeletons attached
//! to a bone of another (a gun under the arms' `wpn`), and skinning matrices for a model whose
//! bones are matched to the animated ones by name.

use crate::anim::{Skeleton, Transform};
use crate::model::Model;

/// An affine transform, 3×4 row-major: `[r00 r01 r02 tx, r10 r11 r12 ty, r20 r21 r22 tz]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat3x4(pub [f32; 12]);

impl Default for Mat3x4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat3x4 {
    pub const IDENTITY: Self = Self([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);

    #[must_use]
    pub fn from_transform(t: &Transform) -> Self {
        let [x, y, z, w] = t.rotation;
        let s = t.scale;
        let r = [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ];
        Self([
            r[0] * s,
            r[1] * s,
            r[2] * s,
            t.translation[0],
            r[3] * s,
            r[4] * s,
            r[5] * s,
            t.translation[1],
            r[6] * s,
            r[7] * s,
            r[8] * s,
            t.translation[2],
        ])
    }

    #[must_use]
    pub fn mul(&self, o: &Self) -> Self {
        let (a, b) = (&self.0, &o.0);
        let mut out = [0.0; 12];
        for row in 0..3 {
            for col in 0..4 {
                let mut v =
                    a[row * 4] * b[col] + a[row * 4 + 1] * b[4 + col] + a[row * 4 + 2] * b[8 + col];
                if col == 3 {
                    v += a[row * 4 + 3];
                }
                out[row * 4 + col] = v;
            }
        }
        Self(out)
    }

    #[must_use]
    pub fn transform_point(&self, p: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        core::array::from_fn(|r| {
            m[r * 4] * p[0] + m[r * 4 + 1] * p[1] + m[r * 4 + 2] * p[2] + m[r * 4 + 3]
        })
    }

    #[must_use]
    pub fn transform_vector(&self, v: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        core::array::from_fn(|r| m[r * 4] * v[0] + m[r * 4 + 1] * v[1] + m[r * 4 + 2] * v[2])
    }

    /// Inverse of an affine transform.
    #[must_use]
    pub fn inverse(&self) -> Self {
        let m = &self.0;
        let (a, b, c) = (m[0], m[1], m[2]);
        let (d, e, f) = (m[4], m[5], m[6]);
        let (g, h, i) = (m[8], m[9], m[10]);
        let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
        if det.abs() < 1e-12 {
            return Self::IDENTITY;
        }
        let inv = 1.0 / det;
        let r = [
            (e * i - f * h) * inv,
            (c * h - b * i) * inv,
            (b * f - c * e) * inv,
            (f * g - d * i) * inv,
            (a * i - c * g) * inv,
            (c * d - a * f) * inv,
            (d * h - e * g) * inv,
            (b * g - a * h) * inv,
            (a * e - b * d) * inv,
        ];
        let t = [m[3], m[7], m[11]];
        let mut out = [0.0; 12];
        for row in 0..3 {
            out[row * 4] = r[row * 3];
            out[row * 4 + 1] = r[row * 3 + 1];
            out[row * 4 + 2] = r[row * 3 + 2];
            out[row * 4 + 3] = -(r[row * 3] * t[0] + r[row * 3 + 1] * t[1] + r[row * 3 + 2] * t[2]);
        }
        Self(out)
    }
}

/// Bone matrices of a skeleton from its bones' local transforms (parents come first), under
/// `root` (the bone a secondary skeleton hangs from, identity for a primary one).
#[must_use]
pub fn world_matrices(skeleton: &Skeleton, locals: &[Transform], root: Mat3x4) -> Vec<Mat3x4> {
    let mut out: Vec<Mat3x4> = Vec::with_capacity(skeleton.bones.len());
    for (i, parent) in skeleton.parents.iter().enumerate() {
        let local = Mat3x4::from_transform(
            locals
                .get(i)
                .or_else(|| skeleton.reference.get(i))
                .unwrap_or(&Transform::default()),
        );
        let base = parent.and_then(|p| out.get(p).copied()).unwrap_or(root);
        out.push(base.mul(&local));
    }
    out
}

/// Animated bone matrices by name, from one or more posed skeletons.
#[derive(Clone, Debug, Default)]
pub struct Pose {
    bones: Vec<(String, Mat3x4)>,
}

impl Pose {
    pub fn add(&mut self, skeleton: &Skeleton, matrices: &[Mat3x4]) {
        for (name, m) in skeleton.bones.iter().zip(matrices) {
            self.bones.push((name.clone(), *m));
        }
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Mat3x4> {
        self.bones
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, m)| *m)
    }
}

/// Skinning matrices for every bone of `model`: the animated bone of the same name, or the
/// model's own rest offset from its parent when the pose has no such bone, times the bone's
/// inverse bind pose.
#[must_use]
pub fn skinning_matrices(model: &Model, pose: &Pose) -> Vec<Mat3x4> {
    let mut world: Vec<Mat3x4> = Vec::with_capacity(model.bones.len());
    let mut rest: Vec<Mat3x4> = Vec::with_capacity(model.bones.len());
    for bone in &model.bones {
        let local = Mat3x4::from_transform(&Transform {
            translation: bone.position,
            rotation: bone.rotation,
            scale: 1.0,
        });
        let parent_world = bone
            .parent
            .and_then(|p| world.get(p).copied())
            .unwrap_or(Mat3x4::IDENTITY);
        let parent_rest = bone
            .parent
            .and_then(|p| rest.get(p).copied())
            .unwrap_or(Mat3x4::IDENTITY);
        world.push(
            pose.get(&bone.name)
                .unwrap_or_else(|| parent_world.mul(&local)),
        );
        rest.push(parent_rest.mul(&local));
    }
    world
        .iter()
        .zip(&rest)
        .enumerate()
        .map(|(i, (w, r))| {
            let inverse = model
                .inverse_bind
                .get(i)
                .copied()
                .flatten()
                .map_or_else(|| r.inverse(), Mat3x4);
            w.mul(&inverse)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_undoes_a_transform() {
        let m = Mat3x4::from_transform(&Transform {
            translation: [1.0, -2.0, 3.0],
            rotation: [0.2, 0.3, -0.1, 0.927_361_85],
            scale: 1.5,
        });
        let p = m
            .inverse()
            .transform_point(m.transform_point([4.0, 5.0, -6.0]));
        for (a, b) in p.iter().zip([4.0, 5.0, -6.0]) {
            assert!((a - b).abs() < 1e-4, "{p:?}");
        }
    }
}
