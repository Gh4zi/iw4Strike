//! Posing a model from its sequences the way Source layers them: a sequence's blend grid cell
//! picked by pose parameters, its animations sampled at a cycle and blended into a pose by
//! weight (per bone), a delta sequence added on top, then its autolayers played over it.
//!
//! Pose parameters are kept in their own units (`body_yaw` in degrees, `speed` 0..1), one per
//! [`StudioModel::pose_params`] entry.

use crate::studio::{
    AL_LOCAL, AL_NOBLEND, AL_POSE, AL_SPLINE, AL_XFADE, BoneFrame, SEQ_DELTA, SEQ_POST, SEQ_ROOTXFORM,
    Sequence, StudioModel,
};

/// How deep autolayers may nest (a model naming its own sequence as a layer stops here).
const MAX_LAYER_DEPTH: u8 = 4;

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

impl StudioModel {
    /// The pose parameter named `name` (case-insensitive).
    #[must_use]
    pub fn pose_param(&self, name: &str) -> Option<usize> {
        self.pose_params
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// The sequence labelled `label` (case-insensitive).
    #[must_use]
    pub fn sequence_by_label(&self, label: &str) -> Option<usize> {
        self.sequences
            .iter()
            .position(|s| s.label.eq_ignore_ascii_case(label))
    }

    /// Every pose parameter at rest: 0 where its range holds 0, else its start.
    #[must_use]
    pub fn default_pose_params(&self) -> Vec<f32> {
        self.pose_params
            .iter()
            .map(|p| 0.0f32.clamp(p.start.min(p.end), p.start.max(p.end)))
            .collect()
    }
}

/// `p · q` (Hamilton product, `[x, y, z, w]`).
#[must_use]
pub fn quat_mul(p: [f32; 4], q: [f32; 4]) -> [f32; 4] {
    [
        p[3] * q[0] + p[0] * q[3] + p[1] * q[2] - p[2] * q[1],
        p[3] * q[1] - p[0] * q[2] + p[1] * q[3] + p[2] * q[0],
        p[3] * q[2] + p[0] * q[1] - p[1] * q[0] + p[2] * q[3],
        p[3] * q[3] - p[0] * q[0] - p[1] * q[1] - p[2] * q[2],
    ]
}

fn normalize(q: [f32; 4]) -> [f32; 4] {
    let len = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    if len > 0.0 { q.map(|v| v / len) } else { IDENTITY }
}

/// Normalized linear blend along the shorter arc (Source blends delta animations this way).
fn nlerp(p: [f32; 4], q: [f32; 4], t: f32) -> [f32; 4] {
    let dot: f32 = p.iter().zip(&q).map(|(a, b)| a * b).sum();
    let q = if dot < 0.0 { q.map(|v| -v) } else { q };
    normalize(core::array::from_fn(|i| p[i] * (1.0 - t) + q[i] * t))
}

/// The rotation `q` scaled to `t` of its angle.
fn scale(q: [f32; 4], t: f32) -> [f32; 4] {
    mdl_goldsrc::slerp(IDENTITY, q, t)
}

/// `v` rotated by the unit quaternion `q`.
fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let u = [q[0], q[1], q[2]];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = cross(u, v).map(|c| 2.0 * c);
    let ut = cross(u, t);
    core::array::from_fn(|i| v[i] + q[3] * t[i] + ut[i])
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    core::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// `3s² − 2s³`.
fn spline(s: f32) -> f32 {
    (s * s * (3.0 - 2.0 * s)).clamp(0.0, 1.0)
}

/// Where a pose parameter puts one axis of a sequence's blend grid: the cell's lower index and
/// how far toward the next (`Studio_LocalPoseParameter`).
fn grid_place(model: &StudioModel, seq: &Sequence, axis: usize, params: &[f32]) -> (usize, f32) {
    let size = if axis == 0 {
        seq.blend_size.0
    } else {
        seq.blend_size.1
    };
    let Some((param, desc)) = seq.params[axis].and_then(|p| Some((p, model.pose_params.get(p)?)))
    else {
        return (0, 0.0);
    };
    if size < 2 {
        return (0, 0.0);
    }
    let mut value = params.get(param).copied().unwrap_or(0.0);
    if desc.wrap != 0.0 {
        value = desc.start + (value - desc.start).rem_euclid(desc.wrap);
    }
    if seq.pose_keys.len() >= seq.blend_size.0 + seq.blend_size.1 {
        let key = |k: usize| seq.pose_keys[axis * seq.blend_size.0 + k];
        let mut index = 0;
        loop {
            let span = key(index + 1) - key(index);
            let s = if span != 0.0 {
                (value - key(index)) / span
            } else {
                0.0
            };
            if index + 2 < size && s > 1.0 {
                index += 1;
                continue;
            }
            return (index, s.clamp(0.0, 1.0));
        }
    }
    let (from, to) = seq.param_range[axis];
    let mut s = if to != from {
        ((value - from) / (to - from)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut index = 0;
    if size > 2 {
        let scaled = s * (size - 1) as f32;
        index = (scaled as usize).min(size - 2);
        s = scaled - index as f32;
    }
    (index, s)
}

/// Animation `anim`'s local transforms at `cycle` (0..1), frames interpolated, into `out`.
fn sample_animation(model: &StudioModel, anim: usize, cycle: f32, out: &mut Vec<BoneFrame>) {
    out.clear();
    let Some(anim) = model.animations.get(anim) else {
        out.extend_from_slice(&model.rest);
        return;
    };
    let last = anim.num_frames.saturating_sub(1);
    let frame = (cycle * last as f32).clamp(0.0, last as f32);
    let first = (frame as usize).min(last);
    let t = frame - first as f32;
    let a = anim.frame(first);
    if t <= 0.0 || first == last {
        out.extend_from_slice(a);
        return;
    }
    let b = anim.frame(first + 1);
    out.extend(a.iter().zip(b).map(|(a, b)| BoneFrame {
        position: lerp3(a.position, b.position, t),
        rotation: mdl_goldsrc::slerp(a.rotation, b.rotation, t),
    }));
}

/// Blends `other` into `pose` by `t`, as a sequence's grid cells blend.
fn blend_into(pose: &mut [BoneFrame], other: &[BoneFrame], t: f32, delta: bool) {
    for (p, o) in pose.iter_mut().zip(other) {
        p.rotation = if delta {
            nlerp(p.rotation, o.rotation, t)
        } else {
            mdl_goldsrc::slerp(p.rotation, o.rotation, t)
        };
        p.position = lerp3(p.position, o.position, t);
    }
}

/// `sequence`'s local transforms at `cycle` with pose parameters `params`, into `out`
/// (`CalcPoseSingle`); false when the sequence has no data there (an all-zero animation that
/// only carries layers).
pub fn sample_sequence(
    model: &StudioModel,
    sequence: usize,
    cycle: f32,
    params: &[f32],
    out: &mut Vec<BoneFrame>,
) -> bool {
    let Some(seq) = model.sequences.get(sequence) else {
        return false;
    };
    let width = seq.blend_size.0.max(1);
    let height = seq.blend_size.1.max(1);
    let (i0, s0) = grid_place(model, seq, 0, params);
    let (i1, s1) = grid_place(model, seq, 1, params);
    let cell = |x: usize, y: usize| {
        seq.blends
            .get(y.min(height - 1) * width + x.min(width - 1))
            .copied()
    };
    // Corners whose weight counts, as Source picks them.
    let xs: &[(usize, f32)] = if s0 < 0.001 {
        &[(i0, 1.0)]
    } else if s0 > 0.999 {
        &[(i0 + 1, 1.0)]
    } else {
        &[(i0, 0.0), (i0 + 1, s0)]
    };
    let ys: &[(usize, f32)] = if s1 < 0.001 {
        &[(i1, 1.0)]
    } else if s1 > 0.999 {
        &[(i1 + 1, 1.0)]
    } else {
        &[(i1, 0.0), (i1 + 1, s1)]
    };
    let all_zeros = ys.iter().all(|&(y, _)| {
        xs.iter().all(|&(x, _)| {
            cell(x, y)
                .and_then(|a| model.animations.get(a))
                .is_none_or(|a| a.all_zeros)
        })
    });
    if all_zeros {
        return false;
    }
    let delta = seq.flags & SEQ_DELTA != 0;
    let mut row = Vec::new();
    let mut scratch = Vec::new();
    for (k, &(y, ty)) in ys.iter().enumerate() {
        let target = if k == 0 { &mut *out } else { &mut row };
        for (j, &(x, tx)) in xs.iter().enumerate() {
            let anim = cell(x, y).unwrap_or(usize::MAX);
            if j == 0 {
                sample_animation(model, anim, cycle, target);
            } else {
                sample_animation(model, anim, cycle, &mut scratch);
                blend_into(target, &scratch, tx, delta);
            }
        }
        if k > 0 {
            blend_into(out, &row, ty, delta);
        }
    }
    true
}

/// The animations of `sequence`'s grid cell at `params` and each one's weight (bilinear).
fn cell_weights(model: &StudioModel, seq: &Sequence, params: &[f32]) -> [(usize, f32); 4] {
    let width = seq.blend_size.0.max(1);
    let height = seq.blend_size.1.max(1);
    let (i0, s0) = grid_place(model, seq, 0, params);
    let (i1, s1) = grid_place(model, seq, 1, params);
    let cell = |x: usize, y: usize| {
        seq.blends
            .get(y.min(height - 1) * width + x.min(width - 1))
            .copied()
            .unwrap_or(usize::MAX)
    };
    [
        (cell(i0, i1), (1.0 - s0) * (1.0 - s1)),
        (cell(i0 + 1, i1), s0 * (1.0 - s1)),
        (cell(i0, i1 + 1), (1.0 - s0) * s1),
        (cell(i0 + 1, i1 + 1), s0 * s1),
    ]
}

/// How far `sequence` carries the model over one cycle at `params`, with its pose-driven
/// layers' share (`Studio_SeqMovement`): what a locomotion cycle's playback rate is scaled by.
#[must_use]
pub fn sequence_movement(model: &StudioModel, sequence: usize, params: &[f32]) -> [f32; 3] {
    let mut total = [0.0f32; 3];
    let Some(seq) = model.sequences.get(sequence) else {
        return total;
    };
    let mut add = |seq: &Sequence, scale: f32| {
        for (anim, weight) in cell_weights(model, seq, params) {
            if let Some(anim) = model.animations.get(anim).filter(|_| weight > 0.0) {
                for (t, m) in total.iter_mut().zip(anim.movement) {
                    *t += m * weight * scale;
                }
            }
        }
    };
    add(seq, 1.0);
    let base_weight = seq.weights.first().copied().unwrap_or(1.0);
    for layer in &seq.layers {
        if layer.flags & AL_LOCAL != 0
            || layer.flags & AL_POSE == 0
            || layer.start != 0.0
            || layer.end != 0.0
            || layer.tail == layer.peak
        {
            continue;
        }
        let value = layer.pose.and_then(|p| params.get(p).copied()).unwrap_or(0.0);
        let mut s = ((value - layer.peak) / (layer.tail - layer.peak)).clamp(0.0, 1.0);
        if layer.flags & AL_SPLINE != 0 {
            s = spline(s);
        }
        if let Some(layer_seq) = model.sequences.get(layer.sequence).filter(|_| s > 0.0) {
            add(layer_seq, base_weight * s);
        }
    }
    total
}

/// `sample` blended into `pose` by `weight` times each bone's weight in `seq` (`SlerpBones`):
/// a delta sequence adds its offsets (after the pose's own rotation when `SEQ_POST`), any other
/// moves the pose toward it.
fn slerp_bones(seq: &Sequence, pose: &mut [BoneFrame], sample: &[BoneFrame], weight: f32) {
    let delta = seq.flags & SEQ_DELTA != 0;
    let post = seq.flags & SEQ_POST != 0;
    for (b, (p, s)) in pose.iter_mut().zip(sample).enumerate() {
        let w = (weight * seq.weights.get(b).copied().unwrap_or(1.0)).min(1.0);
        if w <= 0.0 {
            continue;
        }
        if delta {
            let offset = scale(s.rotation, w);
            p.rotation = normalize(if post {
                quat_mul(p.rotation, offset)
            } else {
                quat_mul(offset, p.rotation)
            });
            p.position = core::array::from_fn(|i| p.position[i] + s.position[i] * w);
        } else {
            p.rotation = mdl_goldsrc::slerp(p.rotation, s.rotation, w);
            p.position = lerp3(p.position, s.position, w);
        }
    }
}

/// Plays `sequence` at `cycle` over `pose` with `weight`, then its autolayers
/// (`AccumulatePose`). `pose` holds one local transform per bone.
pub fn accumulate(
    model: &StudioModel,
    pose: &mut [BoneFrame],
    sequence: usize,
    cycle: f32,
    weight: f32,
    params: &[f32],
) {
    accumulate_at(model, pose, sequence, cycle, weight, params, 0);
}

fn accumulate_at(
    model: &StudioModel,
    pose: &mut [BoneFrame],
    sequence: usize,
    cycle: f32,
    weight: f32,
    params: &[f32],
    depth: u8,
) {
    let weight = weight.clamp(0.0, 1.0);
    let Some(seq) = model.sequences.get(sequence) else {
        return;
    };
    if weight <= 0.0 || depth > MAX_LAYER_DEPTH {
        return;
    }
    let cycle = if (0.0..1.0).contains(&cycle) {
        cycle
    } else if seq.looping {
        cycle.rem_euclid(1.0)
    } else {
        cycle.clamp(0.0, 1.0)
    };
    let mut sample = Vec::with_capacity(pose.len());
    if sample_sequence(model, sequence, cycle, params, &mut sample) {
        // A root re-transform (`STUDIO_ROOTXFORM`): the root it blends toward is the pose's
        // own root moved by the driver bone's transform in this sequence.
        if seq.flags & SEQ_ROOTXFORM != 0
            && let Some(driver) = seq.root_driver.and_then(|d| sample.get(d).copied())
            && let (Some(root), Some(target)) = (pose.first().copied(), sample.first_mut())
        {
            target.rotation = normalize(quat_mul(driver.rotation, root.rotation));
            let moved = rotate(driver.rotation, root.position);
            target.position = core::array::from_fn(|i| moved[i] + driver.position[i]);
        }
        slerp_bones(seq, pose, &sample, weight);
    }
    for layer in &seq.layers {
        if layer.flags & AL_LOCAL != 0 {
            continue;
        }
        let pose_value = || {
            layer
                .pose
                .and_then(|p| params.get(p).copied())
                .unwrap_or(0.0)
        };
        let mut layer_cycle = cycle;
        let mut layer_weight = weight;
        if layer.start != layer.end {
            let index = if layer.flags & AL_POSE != 0 {
                pose_value()
            } else {
                cycle
            };
            if index < layer.start || index >= layer.end {
                continue;
            }
            let mut s = 1.0;
            if index < layer.peak && layer.start != layer.peak {
                s = (index - layer.start) / (layer.peak - layer.start);
            } else if index > layer.tail && layer.end != layer.tail {
                s = (layer.end - index) / (layer.end - layer.tail);
            }
            if layer.flags & AL_SPLINE != 0 {
                s = spline(s);
            }
            layer_weight = if layer.flags & AL_XFADE != 0 && index > layer.tail {
                (s * weight) / (1.0 - weight + s * weight)
            } else if layer.flags & AL_NOBLEND != 0 {
                s
            } else {
                weight * s
            };
            if layer.flags & AL_POSE == 0 {
                layer_cycle = (cycle - layer.start) / (layer.end - layer.start);
            }
        } else if layer.start == 0.0 && layer.flags & AL_POSE != 0 {
            // A pose-driven layer with no range: its weight follows the pose parameter from
            // `peak` to `tail`.
            if layer.pose.is_none() || layer.tail == layer.peak {
                continue;
            }
            let mut s = ((pose_value() - layer.peak) / (layer.tail - layer.peak)).clamp(0.0, 1.0);
            if layer.flags & AL_SPLINE != 0 {
                s = spline(s);
            }
            layer_weight = weight * s;
        }
        accumulate_at(
            model,
            pose,
            layer.sequence,
            layer_cycle,
            layer_weight,
            params,
            depth + 1,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quarter_turn_scaled_by_half_is_an_eighth() {
        let quarter = [0.0, 0.0, (core::f32::consts::FRAC_PI_4).sin(), (core::f32::consts::FRAC_PI_4).cos()];
        let eighth = scale(quarter, 0.5);
        let expected = (core::f32::consts::PI / 8.0).sin();
        assert!((eighth[2] - expected).abs() < 1e-5, "{eighth:?}");
        let twice = quat_mul(eighth, eighth);
        assert!(twice.iter().zip(quarter).all(|(a, b)| (a - b).abs() < 1e-5), "{twice:?}");
    }

    #[test]
    fn spline_eases_in_and_out() {
        assert_eq!(spline(0.0), 0.0);
        assert_eq!(spline(1.0), 1.0);
        assert!((spline(0.5) - 0.5).abs() < 1e-6);
        assert!(spline(0.1) < 0.1);
    }
}
