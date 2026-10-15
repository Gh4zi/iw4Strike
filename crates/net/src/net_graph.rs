//! What `net_graph` shows: the round trip of a command to its acknowledgement (ping), the
//! packets and commands sent each second, the snapshots lost on the way, and on a listen host
//! the server's frame time.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bevy::prelude::*;

/// The window the per-second rates count over.
const RATE_WINDOW: Duration = Duration::from_secs(1);
/// How many sent commands wait for their acknowledgement (a second and a bit at 128 a second).
const SENT_CAP: usize = 192;
/// How much of each new round trip the shown ping takes (CS's is an average too).
const PING_BLEND: f32 = 0.1;

/// The listen host's authority steps since the last read: how many, their durations' sum and
/// sum of squares, and the gaps between their starts (µs).
static STEP_N: AtomicU64 = AtomicU64::new(0);
static STEP_SUM_US: AtomicU64 = AtomicU64::new(0);
static STEP_SQ_US: AtomicU64 = AtomicU64::new(0);
static GAP_N: AtomicU64 = AtomicU64::new(0);
static GAP_SUM_US: AtomicU64 = AtomicU64::new(0);
static GAP_SQ_US: AtomicU64 = AtomicU64::new(0);
static LAST_START_US: AtomicU64 = AtomicU64::new(0);

fn process_us() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

/// One authority step that started at `started` and took `took`.
pub(crate) fn note_authority_step(started: Instant, took: Duration) {
    let us = took.as_micros() as u64;
    STEP_N.fetch_add(1, Ordering::Relaxed);
    STEP_SUM_US.fetch_add(us, Ordering::Relaxed);
    STEP_SQ_US.fetch_add(us * us, Ordering::Relaxed);
    let start = process_us().saturating_sub(started.elapsed().as_micros() as u64);
    let last = LAST_START_US.swap(start, Ordering::Relaxed);
    if last != 0 && start > last {
        let gap = start - last;
        GAP_N.fetch_add(1, Ordering::Relaxed);
        GAP_SUM_US.fetch_add(gap, Ordering::Relaxed);
        GAP_SQ_US.fetch_add(gap * gap, Ordering::Relaxed);
    }
}

fn take_mean_dev(n: &AtomicU64, sum: &AtomicU64, sq: &AtomicU64) -> Option<(f32, f32)> {
    let n = n.swap(0, Ordering::Relaxed);
    let sum = sum.swap(0, Ordering::Relaxed) as f64;
    let sq = sq.swap(0, Ordering::Relaxed) as f64;
    if n == 0 {
        return None;
    }
    let mean = sum / n as f64;
    let dev = (sq / n as f64 - mean * mean).max(0.0).sqrt();
    Some(((mean / 1000.0) as f32, (dev / 1000.0) as f32))
}

/// The server's frames since the last read, on a listen host: their time and its spread (ms),
/// and the spread of the gaps between them (ms). `None` with no local server.
#[must_use]
pub fn take_server_frames() -> Option<((f32, f32), f32)> {
    let step = take_mean_dev(&STEP_N, &STEP_SUM_US, &STEP_SQ_US)?;
    let gap = take_mean_dev(&GAP_N, &GAP_SUM_US, &GAP_SQ_US).map_or(0.0, |(_, dev)| dev);
    Some((step, gap))
}

/// The client's side of `net_graph`.
#[derive(Resource, Default)]
pub struct NetGraphStats {
    /// Commands sent and not yet acknowledged: their number and when they left.
    sent: VecDeque<(u32, Instant)>,
    newest_sent: Option<u32>,
    newest_acked: Option<u32>,
    /// The averaged round trip (ms), once a command came back.
    pub ping_ms: Option<f32>,
    packets_up: VecDeque<Instant>,
    commands_up: VecDeque<Instant>,
}

impl NetGraphStats {
    /// A send carrying commands up to `newest` (none: a control packet).
    pub(crate) fn note_send(&mut self, newest: Option<u32>) {
        let now = Instant::now();
        self.packets_up.push_back(now);
        if let Some(newest) = newest
            && self
                .newest_sent
                .is_none_or(|sent| newest.wrapping_sub(sent) as i32 > 0)
        {
            let first = self.newest_sent.map_or(newest, |sent| sent.wrapping_add(1));
            let count = newest.wrapping_sub(first).min(SENT_CAP as u32);
            for offset in 0..=count {
                self.sent.push_back((first.wrapping_add(offset), now));
                self.commands_up.push_back(now);
            }
            self.newest_sent = Some(newest);
            while self.sent.len() > SENT_CAP {
                self.sent.pop_front();
            }
        }
        Self::trim(&mut self.packets_up, now);
        Self::trim(&mut self.commands_up, now);
    }

    /// A snapshot acknowledging commands up to `acked`.
    pub(crate) fn note_ack(&mut self, acked: u32) {
        if self
            .newest_acked
            .is_some_and(|newest| acked.wrapping_sub(newest) as i32 <= 0)
        {
            return;
        }
        self.newest_acked = Some(acked);
        let mut left = None;
        while let Some(&(seq, at)) = self.sent.front() {
            if seq.wrapping_sub(acked) as i32 > 0 {
                break;
            }
            self.sent.pop_front();
            if seq == acked {
                left = Some(at);
            }
        }
        if let Some(at) = left {
            let rtt = at.elapsed().as_secs_f32() * 1000.0;
            self.ping_ms = Some(self.ping_ms.map_or(rtt, |ping| ping + (rtt - ping) * PING_BLEND));
        }
    }

    fn trim(times: &mut VecDeque<Instant>, now: Instant) {
        while times
            .front()
            .is_some_and(|at| now.duration_since(*at) > RATE_WINDOW)
        {
            times.pop_front();
        }
    }

    /// Packets and commands sent over the last second.
    #[must_use]
    pub fn rates(&mut self) -> (usize, usize) {
        let now = Instant::now();
        Self::trim(&mut self.packets_up, now);
        Self::trim(&mut self.commands_up, now);
        (self.packets_up.len(), self.commands_up.len())
    }
}
