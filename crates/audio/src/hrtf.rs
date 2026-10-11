//! Audio > HRTF: positional sounds through measured head-related impulse responses (MIT
//! KEMAR, `data/HRTF-KEMAR.txt`), so each ear hears what it would from where the sound is —
//! front from back, above from below, not only left from right. Off, sounds are panned.
//!
//! The control thread picks each positional sound's measured direction (the nearest of 368,
//! mirrored to the full circle) and gives it a distance-only gain; the mixer runs the sound
//! through that direction's left and right responses, crossfading over a block when the
//! direction changes.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_enabled(on: bool) {
    if ENABLED.swap(on, Ordering::Relaxed) != on {
        match data() {
            Some(hrtf) if on => diag::info!(
                Audio,
                "audio: HRTF on ({} measured directions, MIT KEMAR)",
                hrtf.directions.len()
            ),
            _ => diag::info!(
                Audio,
                "audio: HRTF {}",
                if on { "on (no data: panned)" } else { "off" }
            ),
        }
    }
}

#[must_use]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed) && data().is_some()
}

/// Response length, in samples at the mixer's rate.
pub(crate) const TAPS: usize = 128;

/// One measured direction, its responses time-reversed so a block's output is a dot product
/// with the input history, oldest sample first.
struct Direction {
    left: [f32; TAPS],
    right: [f32; TAPS],
}

/// One elevation's directions, by azimuth (degrees, 0 front, 90 right).
struct Ring {
    elevation: f32,
    azimuths: Vec<(f32, u16)>,
}

pub(crate) struct Hrtf {
    directions: Vec<Direction>,
    rings: Vec<Ring>,
}

pub(crate) fn data() -> Option<&'static Hrtf> {
    static DATA: OnceLock<Option<Hrtf>> = OnceLock::new();
    DATA.get_or_init(|| {
        let hrtf = Hrtf::parse(include_bytes!("../data/hrtf_kemar.bin"));
        if hrtf.is_none() {
            diag::warn!(
                Audio,
                "hrtf: the KEMAR responses did not load; sounds stay panned"
            );
        }
        hrtf
    })
    .as_ref()
}

impl Hrtf {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let word = |at: usize| -> Option<u32> {
            Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
        };
        let float = |at: usize| word(at).map(f32::from_bits);
        if bytes.get(..4)? != b"HRIR"
            || word(4)? != crate::render_core::SAMPLE_RATE
            || word(8)? as usize != TAPS
        {
            return None;
        }
        let count = word(12)? as usize;
        let mut directions = Vec::with_capacity(count * 2);
        let mut rings: Vec<Ring> = Vec::new();
        let mut at = 16;
        for _ in 0..count {
            let elevation = float(at)?;
            let azimuth = float(at + 4)?;
            at += 8;
            let ear = |at: &mut usize| -> Option<[f32; TAPS]> {
                let mut taps = [0.0; TAPS];
                for tap in &mut taps {
                    *tap = float(*at)?;
                    *at += 4;
                }
                taps.reverse();
                Some(taps)
            };
            let left = ear(&mut at)?;
            let right = ear(&mut at)?;
            let ring = match rings.iter().position(|ring| ring.elevation == elevation) {
                Some(index) => index,
                None => {
                    rings.push(Ring {
                        elevation,
                        azimuths: Vec::new(),
                    });
                    rings.len() - 1
                }
            };
            let mut add = |azimuth: f32, left: [f32; TAPS], right: [f32; TAPS]| {
                let index = u16::try_from(directions.len()).ok()?;
                directions.push(Direction { left, right });
                rings[ring].azimuths.push((azimuth, index));
                Some(())
            };
            add(azimuth, left, right)?;
            // The set covers the right half; the left half is its mirror image.
            if azimuth > 0.0 && azimuth < 180.0 {
                add(360.0 - azimuth, right, left)?;
            }
        }
        if directions.is_empty() {
            return None;
        }
        Some(Self { directions, rings })
    }

    /// The measured direction nearest to a sound at `right`, `up` and `forward` from the
    /// listener (any length).
    pub(crate) fn nearest(&self, right: f32, up: f32, forward: f32) -> Option<u16> {
        let level = right.hypot(forward);
        if !(level.is_finite() && up.is_finite()) || level.hypot(up) < 1e-3 {
            return None;
        }
        let elevation = up.atan2(level).to_degrees();
        let azimuth = right.atan2(forward).to_degrees().rem_euclid(360.0);
        let ring = self.rings.iter().min_by(|a, b| {
            (a.elevation - elevation)
                .abs()
                .total_cmp(&(b.elevation - elevation).abs())
        })?;
        let around = |a: f32| {
            let d = (a - azimuth).abs();
            d.min(360.0 - d)
        };
        ring.azimuths
            .iter()
            .min_by(|a, b| around(a.0).total_cmp(&around(b.0)))
            .map(|(_, index)| *index)
    }

    fn direction(&self, index: u32) -> Option<&Direction> {
        self.directions.get(index as usize)
    }
}

/// No direction: the sound is panned, not run through a response.
pub(crate) const NO_DIRECTION: u32 = u32::MAX;

/// One voice's filter state on the render thread.
pub(crate) struct HrtfVoice {
    /// The input history twice over, so the last `TAPS` samples are always one contiguous
    /// slice (oldest first) ending at `at + TAPS`.
    history: [f32; 2 * TAPS],
    at: usize,
    /// The direction the voice was last rendered from.
    direction: u32,
}

impl Default for HrtfVoice {
    fn default() -> Self {
        Self {
            history: [0.0; 2 * TAPS],
            at: 0,
            direction: NO_DIRECTION,
        }
    }
}

impl HrtfVoice {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Render a block's mono `input` from `direction` into `output`, crossfading from the
    /// direction the last block used. Returns false (nothing added) without a direction.
    pub(crate) fn render(
        &mut self,
        hrtf: &Hrtf,
        direction: u32,
        input: &[f32],
        output: &mut [[f32; 2]],
    ) -> bool {
        let Some(now) = hrtf.direction(direction) else {
            self.direction = NO_DIRECTION;
            return false;
        };
        let before = hrtf
            .direction(self.direction)
            .filter(|_| self.direction != direction);
        let fade = 1.0 / input.len().max(1) as f32;
        for (n, (&sample, target)) in input.iter().zip(output.iter_mut()).enumerate() {
            self.history[self.at] = sample;
            self.history[self.at + TAPS] = sample;
            self.at = (self.at + 1) % TAPS;
            let window = &self.history[self.at..self.at + TAPS];
            let (mut left, mut right) = (dot(&now.left, window), dot(&now.right, window));
            if let Some(before) = before {
                let t = (n + 1) as f32 * fade;
                left = left * t + dot(&before.left, window) * (1.0 - t);
                right = right * t + dot(&before.right, window) * (1.0 - t);
            }
            target[0] += left;
            target[1] += right;
        }
        self.direction = direction;
        true
    }
}

fn dot(taps: &[f32; TAPS], window: &[f32]) -> f32 {
    taps.iter()
        .zip(window)
        .map(|(tap, sample)| tap * sample)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn azimuth_elevation(hrtf: &Hrtf, index: u16) -> (f32, f32) {
        hrtf.rings
            .iter()
            .find_map(|ring| {
                ring.azimuths
                    .iter()
                    .find(|(_, at)| *at == index)
                    .map(|(azimuth, _)| (*azimuth, ring.elevation))
            })
            .unwrap()
    }

    #[test]
    fn directions_cover_the_whole_circle() {
        let hrtf = data().expect("the KEMAR responses load");
        // 368 measured, all but each ring's 0 and 180 mirrored to the left half.
        assert!(hrtf.directions.len() > 700, "{}", hrtf.directions.len());
        for ((right, up, forward), (azimuth, elevation)) in [
            ((0.0, 0.0, 1.0), (0.0, 0.0)),
            ((1.0, 0.0, 0.0), (90.0, 0.0)),
            ((0.0, 0.0, -1.0), (180.0, 0.0)),
            ((-1.0, 0.0, 0.0), (270.0, 0.0)),
            ((0.0, 1.0, 0.0), (0.0, 90.0)),
        ] {
            let index = hrtf.nearest(right, up, forward).unwrap();
            assert_eq!(
                azimuth_elevation(hrtf, index),
                (azimuth, elevation),
                "{right} {up} {forward}"
            );
        }
        assert_eq!(hrtf.nearest(0.0, 0.0, 0.0), None);
    }

    fn ears(direction: u16) -> (f32, f32) {
        let hrtf = data().unwrap();
        let mut voice = HrtfVoice::default();
        let mut input = [0.0; 256];
        input[0] = 1.0;
        let mut output = [[0.0; 2]; 256];
        assert!(voice.render(hrtf, u32::from(direction), &input, &mut output));
        output
            .iter()
            .fold((0.0, 0.0), |(l, r), o| (l + o[0] * o[0], r + o[1] * o[1]))
    }

    #[test]
    fn an_impulse_comes_out_at_each_ear_as_measured() {
        let hrtf = data().unwrap();
        let (left, right) = ears(hrtf.nearest(0.0, 0.0, 1.0).unwrap());
        assert!(
            (left - 0.5).abs() < 0.01 && (right - 0.5).abs() < 0.01,
            "{left} {right}"
        );
        let (left, right) = ears(hrtf.nearest(1.0, 0.0, 0.0).unwrap());
        assert!(right > 10.0 * left, "{left} {right}");
        let (left, right) = ears(hrtf.nearest(-1.0, 0.0, 0.0).unwrap());
        assert!(left > 10.0 * right, "{left} {right}");
    }
}
