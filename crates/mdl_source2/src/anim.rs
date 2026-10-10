//! Animation skeletons (`.vnmskel_c`) and clips (`.vnmclip_c`) of CS2's animation system.
//!
//! A clip samples one skeleton (a viewmodel clip: the arms' `viewmodel.vnmskel`) and carries
//! secondary clips for skeletons attached to it (the gun's, hung under the arms' `wpn` bone).
//! Each frame stores, per bone that moves, a "smallest three" quaternion (three 16-bit words),
//! a translation quantised into the bone's range (three words) and maybe a scale (one word).

use crate::Resource;
use crate::kv3::Value;

/// A bone's local transform: translation, rotation (`[x, y, z, w]`), uniform scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: 1.0,
        }
    }
}

impl Transform {
    /// Blend toward `other` by `t` (translations and scale linearly, rotations by normalised
    /// lerp along the short way).
    #[must_use]
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        let dot: f32 = (0..4).map(|i| self.rotation[i] * other.rotation[i]).sum();
        let sign = if dot < 0.0 { -1.0 } else { 1.0 };
        let mut rotation: [f32; 4] =
            core::array::from_fn(|i| self.rotation[i] * (1.0 - t) + other.rotation[i] * sign * t);
        let length = rotation
            .iter()
            .map(|v| v * v)
            .sum::<f32>()
            .sqrt()
            .max(1e-12);
        rotation = rotation.map(|v| v / length);
        Self {
            translation: core::array::from_fn(|i| {
                self.translation[i] + (other.translation[i] - self.translation[i]) * t
            }),
            rotation,
            scale: self.scale + (other.scale - self.scale) * t,
        }
    }
}

/// `[tx, ty, tz, scale, qx, qy, qz, qw]` as skeletons and root motion store transforms.
fn transform_of(value: &Value) -> Transform {
    let v = value.as_f32s();
    if v.len() < 8 {
        return Transform::default();
    }
    Transform {
        translation: [v[0], v[1], v[2]],
        scale: v[3],
        rotation: [v[4], v[5], v[6], v[7]],
    }
}

/// An animation skeleton.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skeleton {
    pub id: String,
    pub bones: Vec<String>,
    pub parents: Vec<Option<usize>>,
    /// Each bone's reference pose relative to its parent.
    pub reference: Vec<Transform>,
    /// Skeletons attached to one of this skeleton's bones (guns under `wpn`).
    pub secondary: Vec<(String, String)>,
}

impl Skeleton {
    #[must_use]
    pub fn bone(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b == name)
    }
}

/// Load an animation skeleton.
pub fn load_skeleton(bytes: &[u8]) -> Result<Skeleton, String> {
    let data = Resource::parse(bytes)?.data_kv3()?;
    let list = |key: &str| data.get(key).map(Value::as_array).unwrap_or(&[]);
    Ok(Skeleton {
        id: data.str_of("m_ID").to_owned(),
        bones: list("m_boneIDs")
            .iter()
            .map(|b| b.as_str().unwrap_or("").to_owned())
            .collect(),
        parents: list("m_parentIndices")
            .iter()
            .map(|p| p.as_i64().and_then(|p| usize::try_from(p).ok()))
            .collect(),
        reference: list("m_parentSpaceReferencePose")
            .iter()
            .map(transform_of)
            .collect(),
        secondary: list("m_secondarySkeletons")
            .iter()
            .map(|s| {
                (
                    s.str_of("m_skeleton").to_owned(),
                    s.str_of("m_attachToBoneID").to_owned(),
                )
            })
            .collect(),
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Track {
    range_start: [f32; 3],
    range_length: [f32; 3],
    scale_start: f32,
    scale_length: f32,
    constant_rotation: [f32; 4],
    rotation_static: bool,
    translation_static: bool,
    scale_static: bool,
    /// Where the track's words start within a frame, when the clip says.
    read_offset: Option<usize>,
}

/// Something a clip fires at a time: a sound (`Weapon_AK47.Clipout`) or a named marker.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipEvent {
    /// Seconds into the clip.
    pub time: f32,
    pub duration: f32,
    /// `Sound` for sound events, `ID` for markers.
    pub kind: String,
    /// The sound event name, or the marker's id.
    pub name: String,
}

/// An animation clip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clip {
    pub skeleton: String,
    pub frames: usize,
    pub duration: f32,
    pub additive: bool,
    data: Vec<u16>,
    offsets: Vec<usize>,
    tracks: Vec<Track>,
    /// Clips for skeletons attached to this one's (the gun's).
    pub secondary: Vec<Clip>,
    pub events: Vec<ClipEvent>,
}

const RANGE_MIN: f32 = -std::f32::consts::FRAC_1_SQRT_2;
const RANGE_LENGTH: f32 = std::f32::consts::SQRT_2;

fn decode_unorm(word: u16, start: f32, length: f32) -> f32 {
    f32::from(word) / 65535.0 * length + start
}

fn decode_quaternion(words: &[u16]) -> [f32; 4] {
    let scale = RANGE_LENGTH / 32767.0;
    let a = f32::from(words[0] & 0x7FFF) * scale + RANGE_MIN;
    let b = f32::from(words[1] & 0x7FFF) * scale + RANGE_MIN;
    let c = f32::from(words[2]) * scale + RANGE_MIN;
    let w = (1.0 - (a * a + b * b + c * c)).max(0.0).sqrt();
    // The two high bits say which component was dropped (the largest).
    match ((words[0] >> 14) & 2) | (words[1] >> 15) {
        0 => [w, a, b, c],
        1 => [a, w, b, c],
        2 => [a, b, w, c],
        _ => [a, b, c, w],
    }
}

fn parse_clip(data: &Value) -> Clip {
    let list = |key: &str| data.get(key).map(Value::as_array).unwrap_or(&[]);
    let blob = data
        .get("m_compressedPoseData")
        .and_then(Value::as_blob)
        .unwrap_or(&[]);
    let words = blob
        .as_chunks::<2>()
        .0
        .iter()
        .map(|w| u16::from_le_bytes(*w))
        .collect();
    let range = |s: &Value, key: &str| {
        let r = s.get(key);
        (
            r.map(|r| r.float_of("m_flRangeStart")).unwrap_or(0.0),
            r.map(|r| r.float_of("m_flRangeLength")).unwrap_or(0.0),
        )
    };
    let tracks = list("m_trackCompressionSettings")
        .iter()
        .map(|s| {
            let (x, y, z) = (
                range(s, "m_translationRangeX"),
                range(s, "m_translationRangeY"),
                range(s, "m_translationRangeZ"),
            );
            let (scale_start, scale_length) = range(s, "m_scaleRange");
            let q = s
                .get("m_constantRotation")
                .map_or_else(Vec::new, Value::as_f32s);
            Track {
                range_start: [x.0, y.0, z.0],
                range_length: [x.1, y.1, z.1],
                scale_start,
                scale_length,
                constant_rotation: if q.len() >= 4 {
                    [q[0], q[1], q[2], q[3]]
                } else {
                    [0.0, 0.0, 0.0, 1.0]
                },
                rotation_static: s
                    .get("m_bIsRotationStatic")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                translation_static: s
                    .get("m_bIsTranslationStatic")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                scale_static: s
                    .get("m_bIsScaleStatic")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                read_offset: s
                    .get("m_nTrackReadOffset")
                    .and_then(Value::as_i64)
                    .and_then(|v| usize::try_from(v).ok()),
            }
        })
        .collect();
    // Event times are stored as fractions of the clip.
    let clip_seconds = data.float_of("m_flDuration");
    let events = list("m_events")
        .iter()
        .map(|e| {
            let class = e.str_of("_class");
            let kind = class
                .trim_start_matches("CNm")
                .trim_end_matches("Event")
                .to_owned();
            let name = if class == "CNmSoundEvent" {
                e.str_of("m_name")
            } else {
                e.str_of("m_ID")
            };
            ClipEvent {
                time: e
                    .path("m_flStartTime/m_flValue")
                    .and_then(Value::as_f32)
                    .unwrap_or(0.0)
                    * clip_seconds,
                duration: e
                    .path("m_flDuration/m_flValue")
                    .and_then(Value::as_f32)
                    .unwrap_or(0.0)
                    * clip_seconds,
                kind,
                name: name.to_owned(),
            }
        })
        .collect();
    Clip {
        skeleton: data.str_of("m_skeleton").to_owned(),
        frames: data.int_of("m_nNumFrames").max(1) as usize,
        duration: data.float_of("m_flDuration"),
        additive: data
            .get("m_bIsAdditive")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        data: words,
        offsets: list("m_compressedPoseOffsets")
            .iter()
            .filter_map(Value::as_i64)
            .map(|v| v.max(0) as usize)
            .collect(),
        tracks,
        secondary: list("m_secondaryAnimations")
            .iter()
            .map(parse_clip)
            .collect(),
        events,
    }
}

/// Load an animation clip.
pub fn load_clip(bytes: &[u8]) -> Result<Clip, String> {
    let data = Resource::parse(bytes)?.data_kv3()?;
    Ok(parse_clip(&data))
}

impl Clip {
    /// Number of bones the clip animates (its skeleton's).
    #[must_use]
    pub fn bone_count(&self) -> usize {
        self.tracks.len()
    }

    /// Every bone's local transform at a frame.
    #[must_use]
    pub fn frame(&self, index: usize) -> Vec<Transform> {
        let index = index.min(self.frames.saturating_sub(1));
        let frame = self
            .offsets
            .get(index)
            .and_then(|&at| self.data.get(at..))
            .unwrap_or(&[]);
        let mut words = frame;
        self.tracks
            .iter()
            .map(|track| {
                // Tracks are packed one after another; the clip also records where each starts.
                if let Some(at) = track.read_offset {
                    words = frame.get(at..).unwrap_or(&[]);
                }
                let mut take = |n: usize| -> Option<&[u16]> {
                    let (head, rest) = (words.get(..n)?, words.get(n..)?);
                    words = rest;
                    Some(head)
                };
                let mut out = Transform {
                    translation: track.range_start,
                    rotation: track.constant_rotation,
                    scale: track.scale_start,
                };
                if !track.rotation_static
                    && let Some(w) = take(3)
                {
                    out.rotation = decode_quaternion(w);
                }
                if !track.translation_static
                    && let Some(w) = take(3)
                {
                    out.translation = core::array::from_fn(|k| {
                        decode_unorm(w[k], track.range_start[k], track.range_length[k])
                    });
                }
                if !track.scale_static
                    && let Some(w) = take(1)
                {
                    out.scale = decode_unorm(w[0], track.scale_start, track.scale_length);
                }
                out
            })
            .collect()
    }

    /// Every bone's local transform at `seconds`, between the two nearest frames.
    #[must_use]
    pub fn sample(&self, seconds: f32) -> Vec<Transform> {
        if self.frames <= 1 || self.duration <= 0.0 {
            return self.frame(0);
        }
        let position = (seconds / self.duration).clamp(0.0, 1.0) * (self.frames - 1) as f32;
        let first = position.floor() as usize;
        let t = position - first as f32;
        let a = self.frame(first);
        if t <= 1e-4 || first + 1 >= self.frames {
            return a;
        }
        let b = self.frame(first + 1);
        a.iter().zip(&b).map(|(a, b)| a.lerp(b, t)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quaternion_words_round_trip_identity() {
        // Identity: x, y, z at the middle of the range (0) with w dropped as the largest.
        let mid = ((0.0 - RANGE_MIN) / RANGE_LENGTH * 32767.0).round() as u16;
        let q = decode_quaternion(&[mid | 0x8000, mid | 0x8000, mid]);
        assert!((q[3] - 1.0).abs() < 1e-3, "{q:?}");
        assert!(q[0].abs() < 1e-3 && q[1].abs() < 1e-3 && q[2].abs() < 1e-3);
    }

    #[test]
    fn transforms_blend_halfway() {
        let a = Transform::default();
        let b = Transform {
            translation: [2.0, 0.0, 0.0],
            rotation: [
                0.0,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
            scale: 1.0,
        };
        let h = a.lerp(&b, 0.5);
        assert!((h.translation[0] - 1.0).abs() < 1e-6);
        let angle = 2.0 * h.rotation[3].acos();
        assert!(
            (angle - std::f32::consts::FRAC_PI_4).abs() < 1e-3,
            "{angle}"
        );
    }
}
