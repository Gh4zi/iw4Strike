use bevy::prelude::Resource;
use playerstate_iw4::UserCmd;
use sim::{ClientId, Tick};
use std::collections::{HashMap, VecDeque};

use crate::client::predict::CmdSeq;

pub const AUTHORITY_HZ: f64 = 1000.0 / sim::MATCH_TICK_MS as f64;

pub const AUTHORITY_MS: i32 = sim::MATCH_TICK_MS as i32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServerTime(i32);

impl ServerTime {
    pub const fn from_ms(ms: i32) -> Self {
        Self(ms)
    }

    pub fn from_tick(tick: Tick) -> Self {
        let ms = i32::try_from(tick.0)
            .unwrap_or(0)
            .saturating_mul(AUTHORITY_MS);
        Self(ms)
    }

    pub const fn ms(self) -> i32 {
        self.0
    }
}

pub const MAX_REDUNDANT_CMDS: usize = crate::client::predict::DEFAULT_HISTORY_CAP;

pub const MAX_QUEUED_COMMANDS_PER_PEER: usize = MAX_REDUNDANT_CMDS;

pub const MAX_QUEUED_COMMAND_MS: i32 = 1000;

pub(crate) const COMMAND_TIME_STEP_MS: i32 = 200;

pub const MAX_COMMAND_STALL_MS: i32 = 1000;

pub const MAX_COMMANDS_PER_PEER_PER_FRAME: usize = 32;

/// A hole this old in a peer's command sequence (commands lost on the way and never resent) is
/// stepped over: the server runs on from the next command it has and the player is corrected
/// once, instead of the server waiting for the lost ones until [`MAX_COMMAND_STALL_MS`] drops
/// the player.
pub const COMMAND_GAP_SKIP_MS: i32 = 250;

/// The most a peer's commands are held back to even out its network jitter (see [`Playout`]).
pub const MAX_PLAYOUT_JITTER_MS: i32 = 50;

/// How long a late arrival keeps the playout delay up before it starts to shrink.
const PLAYOUT_WINDOW_MS: i32 = 1000;

/// How fast the playout delay shrinks once arrivals are steady again: a tenth of a millisecond
/// a tick, 1% of real time, so the player only moves 1% faster on the server while it does.
const PLAYOUT_RELEASE_MS_PER_TICK: f32 = 0.1;

/// Debug log of each peer's command pace, every this many ticks.
const PLAYOUT_REPORT_TICKS: u32 = 500;

/// A peer's jitter buffer. A client stamps each command with the server time of the newest
/// snapshot it had, so every command reaches the server a ping "late" and used to run the
/// tick it arrived: on an uneven connection the player stood still for a tick or two, then ran
/// the late commands at once, and everyone else saw them stutter (worst at bunnyhop speed).
/// The server now runs a peer's command `offset` ms of server time after its stamp: the
/// latest arrival of the last second, so commands run at the pace they were made. A late
/// command raises it at once; it shrinks slowly. At most [`MAX_PLAYOUT_JITTER_MS`] above the
/// peer's quickest arrival, so a bad connection stutters as before instead of lagging more.
/// The shot rewind does not change: shots carry the time the shooter saw.
#[derive(Debug, Default)]
struct Playout {
    offset: f32,
    measured_seq: Option<u32>,
    /// Per tick with new arrivals: (server time, quickest and latest arrival age, ms).
    arrivals: VecDeque<(i32, i32, i32)>,
    report: PlayoutReport,
}

#[derive(Debug, Default)]
struct PlayoutReport {
    ticks: u32,
    moved: [u32; 4],
    late: u32,
}

impl Playout {
    /// Measures the commands seen for the first time this tick and moves the offset.
    fn observe(&mut self, time_ms: i32, queue: impl Iterator<Item = (u32, i32)>) {
        let mut fresh: Option<(i32, i32)> = None;
        let mut newest = self.measured_seq;
        for (seq, stamp) in queue {
            if self.measured_seq.is_some_and(|measured| seq <= measured) {
                continue;
            }
            newest = Some(newest.map_or(seq, |newest| newest.max(seq)));
            let age = time_ms.wrapping_sub(stamp);
            if age as f32 > self.offset {
                self.report.late += 1;
            }
            fresh = Some(fresh.map_or((age, age), |(lo, hi)| (lo.min(age), hi.max(age))));
        }
        self.measured_seq = newest;
        if let Some((quickest, latest)) = fresh {
            self.arrivals.push_back((time_ms, quickest, latest));
        }
        while self
            .arrivals
            .front()
            .is_some_and(|(at, _, _)| time_ms.wrapping_sub(*at) > PLAYOUT_WINDOW_MS)
        {
            self.arrivals.pop_front();
        }
        let (Some(floor), Some(peak)) = (
            self.arrivals.iter().map(|(_, lo, _)| *lo).min(),
            self.arrivals.iter().map(|(_, _, hi)| *hi).max(),
        ) else {
            return;
        };
        if !playout_enabled() {
            return;
        }
        let target = peak.clamp(floor, floor.saturating_add(MAX_PLAYOUT_JITTER_MS)).max(0) as f32;
        self.offset = if target > self.offset {
            target
        } else if self.offset - target > MAX_PLAYOUT_JITTER_MS as f32 {
            // The connection got much quicker (a shorter route): one catch-up now, rather than
            // holding the player back that much longer for many seconds while it shrinks.
            target
        } else {
            (self.offset - PLAYOUT_RELEASE_MS_PER_TICK).max(target)
        };
    }

    /// The offset in whole ms: a command stamped `stamp` runs once `time_ms - stamp` reaches it.
    fn held_ms(&self) -> i32 {
        self.offset.round() as i32
    }

    /// Counts a tick by how much of the peer's movement it ran: none, under 8 ms, 8-12 ms
    /// (the steady 10) or more.
    fn note_tick(&mut self, id: ClientId, ran_ms: i32) {
        let report = &mut self.report;
        report.ticks += 1;
        report.moved[match ran_ms {
            ..=0 => 0,
            1..=7 => 1,
            8..=12 => 2,
            _ => 3,
        }] += 1;
        if report.ticks < PLAYOUT_REPORT_TICKS {
            return;
        }
        let floor = self.arrivals.iter().map(|(_, lo, _)| *lo).min().unwrap_or(0);
        let peak = self.arrivals.iter().map(|(_, _, hi)| *hi).max().unwrap_or(0);
        diag::debug!(
            Net,
            "cmd pace client {}: playout {:.1} ms (arrival age {floor}..{peak} ms), ticks running 0 / <8 / 8-12 / >12 ms of movement: {}/{}/{}/{}, {} late",
            id.0,
            self.offset,
            report.moved[0],
            report.moved[1],
            report.moved[2],
            report.moved[3],
            report.late
        );
        *report = PlayoutReport::default();
    }
}

/// `IW4L_NET_PLAYOUT=0` runs every command the tick it arrives, as before the jitter buffer
/// (the pace log still reports), for comparing the two.
fn playout_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("IW4L_NET_PLAYOUT").map_or(true, |value| value.trim() != "0"))
}

/// Dev switch `IW4L_NET_SIM_JITTER_MS=<ms>`: the server holds each peer's commands and lets
/// them in as clumps, the next one up to that many ms later, like an uneven internet link.
fn sim_jitter_ms() -> Option<i32> {
    static JITTER: std::sync::OnceLock<Option<i32>> = std::sync::OnceLock::new();
    *JITTER.get_or_init(|| {
        let ms = std::env::var("IW4L_NET_SIM_JITTER_MS").ok()?.trim().parse::<i32>().ok()?;
        (ms > 0).then(|| {
            diag::warn!(Net, "IW4L_NET_SIM_JITTER_MS={ms}: peer commands arrive in clumps up to {ms} ms apart (test only)");
            ms
        })
    })
}

#[derive(Debug, Default)]
struct SimJitter {
    held: Vec<(Option<CmdSeq>, UserCmd, Option<sim::ShotSampleProvenance>)>,
    open_at: i32,
    rng: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputBacklogFault {
    QueueCount { queued: usize },

    QueueDuration { queued_ms: i32 },

    NoProgress { stalled_ms: i32 },
}

impl core::fmt::Display for InputBacklogFault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::QueueCount { queued } => write!(
                f,
                "InputBacklogExceeded: {queued} unique commands queued (max \
                 {MAX_QUEUED_COMMANDS_PER_PEER})"
            ),
            Self::QueueDuration { queued_ms } => write!(
                f,
                "InputBacklogExceeded: {queued_ms} ms of command time queued (max \
                 {MAX_QUEUED_COMMAND_MS})"
            ),
            Self::NoProgress { stalled_ms } => write!(
                f,
                "InputBacklogExceeded: command queue made no progress for {stalled_ms} ms (max \
                 {MAX_COMMAND_STALL_MS})"
            ),
        }
    }
}

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuthorityClock {
    pub tick: u32,
    pub time_ms: i32,
}

impl AuthorityClock {
    pub fn advance(&mut self) -> u32 {
        self.time_ms = self.time_ms.wrapping_add(AUTHORITY_MS);
        self.tick = self.tick.wrapping_add(1);
        self.tick
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GatheredCommands {
    pub cmds: Vec<(ClientId, UserCmd)>,

    pub acks: Vec<(ClientId, CmdSeq)>,

    pub proxied: Vec<ClientId>,

    pub samples: Vec<(ClientId, i32, sim::ShotSampleProvenance)>,

    pub backlog_faults: Vec<(ClientId, InputBacklogFault)>,
}

#[derive(Resource, Debug, Default)]
pub struct ClientCommandInbox {
    queued: HashMap<ClientId, VecDeque<(Option<CmdSeq>, UserCmd, sim::ShotSampleProvenance)>>,

    last_consumed: HashMap<ClientId, UserCmd>,

    last_acked_seq: HashMap<ClientId, u32>,

    blocked_head: HashMap<ClientId, (CmdSeq, i32)>,

    playout: HashMap<ClientId, Playout>,

    sim_jitter: HashMap<ClientId, SimJitter>,

    /// A listen host's own player: its commands come in process, with no network to even out.
    local: Option<ClientId>,
}

impl ClientCommandInbox {
    pub fn push(
        &mut self,
        id: ClientId,
        seq: Option<CmdSeq>,
        cmd: UserCmd,
        sample: Option<sim::ShotSampleProvenance>,
    ) {
        if seq.is_some() && sim_jitter_ms().is_some() {
            self.sim_jitter.entry(id).or_default().held.push((seq, cmd, sample));
            return;
        }
        self.enqueue(id, seq, cmd, sample);
    }

    fn enqueue(
        &mut self,
        id: ClientId,
        seq: Option<CmdSeq>,
        cmd: UserCmd,
        sample: Option<sim::ShotSampleProvenance>,
    ) {
        if let Some(s) = seq {
            if self
                .last_acked_seq
                .get(&id)
                .is_some_and(|&last| s.0 <= last)
            {
                return;
            }
            if self
                .queued
                .get(&id)
                .is_some_and(|q| q.iter().any(|(qs, _, _)| *qs == Some(s)))
            {
                return;
            }
        }
        let queue = self.queued.entry(id).or_default();

        if queue.len() > MAX_QUEUED_COMMANDS_PER_PEER {
            return;
        }
        let row = (
            seq,
            cmd,
            sample.unwrap_or(sim::ShotSampleProvenance::NO_CLAIM),
        );
        if let Some(sequence) = seq {
            let at = queue
                .iter()
                .position(|(other, _, _)| other.is_some_and(|other| other > sequence))
                .unwrap_or(queue.len());
            queue.insert(at, row);
        } else {
            queue.push_back(row);
        }
    }

    pub fn clear(&mut self) {
        self.queued.clear();
        self.last_consumed.clear();
        self.last_acked_seq.clear();
        self.blocked_head.clear();
        self.playout.clear();
        self.sim_jitter.clear();
    }

    pub fn retire_client(&mut self, id: ClientId) {
        self.queued.remove(&id);
        self.last_consumed.remove(&id);
        self.last_acked_seq.remove(&id);
        self.blocked_head.remove(&id);
        self.playout.remove(&id);
        self.sim_jitter.remove(&id);
    }

    /// The listen host's own player, whose commands run as they come (no jitter buffer).
    pub fn set_local_client(&mut self, id: Option<ClientId>) {
        self.local = id;
    }

    /// Lets the commands the jitter switch held through, a clump at a time.
    fn release_sim_jitter(&mut self, time_ms: i32) {
        let Some(max_ms) = sim_jitter_ms() else {
            return;
        };
        let mut released = Vec::new();
        for (id, link) in &mut self.sim_jitter {
            if time_ms.wrapping_sub(link.open_at) < 0 {
                continue;
            }
            // xorshift32: the gap to the next clump, 0..=max_ms.
            let mut x = if link.rng == 0 { 0x9e37_79b9 ^ id.0 as u32 } else { link.rng };
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            link.rng = x;
            link.open_at = time_ms.wrapping_add((x % (max_ms as u32 + 1)) as i32);
            released.extend(link.held.drain(..).map(|row| (*id, row)));
        }
        for (id, (seq, cmd, sample)) in released {
            self.enqueue(id, seq, cmd, sample);
        }
    }

    /// `id`'s round trip as its commands show it (ms): the quickest age a command arrived at
    /// over the last second (the server time the client had seen when sending, to now).
    pub fn latency_ms(&self, id: ClientId) -> Option<i32> {
        self.playout
            .get(&id)?
            .arrivals
            .iter()
            .map(|(_, quickest, _)| *quickest)
            .min()
    }

    pub fn pending(&self, id: ClientId) -> usize {
        self.queued.get(&id).map_or(0, VecDeque::len)
    }

    pub fn unique_queue_depth(&self, id: ClientId) -> usize {
        self.pending(id)
    }

    pub fn backlog_fault(&mut self, id: ClientId, time_ms: i32) -> Option<InputBacklogFault> {
        let queue = self.queued.get(&id)?;
        let sequenced = queue.len() - queue.iter().filter(|(seq, _, _)| seq.is_none()).count();
        if sequenced == 0 {
            return None;
        }
        if sequenced > MAX_QUEUED_COMMANDS_PER_PEER {
            return Some(InputBacklogFault::QueueCount { queued: sequenced });
        }
        let mut queued_ms: i32 = 0;
        let mut previous: Option<i32> = self
            .last_consumed
            .get(&id)
            .map(|cmd| cmd.server_time)
            .or_else(|| {
                queue
                    .iter()
                    .find(|(seq, _, _)| seq.is_some())
                    .map(|(_, cmd, _)| time_ms.min(cmd.server_time))
            });
        for (_, cmd, _) in queue.iter().filter(|(seq, _, _)| seq.is_some()) {
            if let Some(previous) = previous {
                queued_ms = queued_ms.saturating_add(
                    cmd.server_time
                        .saturating_sub(previous)
                        .clamp(0, COMMAND_TIME_STEP_MS),
                );
            }
            previous = Some(cmd.server_time);
        }
        if queued_ms > MAX_QUEUED_COMMAND_MS {
            return Some(InputBacklogFault::QueueDuration { queued_ms });
        }
        let head = queue.iter().find_map(|(seq, _, _)| *seq)?;
        let blocked = self.blocked_head.entry(id).or_insert((head, time_ms));
        if blocked.0 != head {
            *blocked = (head, time_ms);
        }
        let stalled_ms = time_ms.saturating_sub(blocked.1);
        if stalled_ms > MAX_COMMAND_STALL_MS {
            return Some(InputBacklogFault::NoProgress { stalled_ms });
        }
        None
    }

    /// Steps over a hole at the head of a peer's queue that has blocked it for
    /// [`COMMAND_GAP_SKIP_MS`]: the commands before the first one queued count as run. Returns
    /// how many were stepped over.
    fn skip_stale_gap(&mut self, id: ClientId, time_ms: i32) -> Option<u32> {
        let expected = self.last_acked_seq.get(&id)?.wrapping_add(1);
        let head = self.queued.get(&id)?.iter().find_map(|(seq, _, _)| *seq)?;
        let missing = head.0.wrapping_sub(expected);
        if missing == 0 || missing > MAX_REDUNDANT_CMDS as u32 * 4 {
            return None;
        }
        let (blocked, since) = self.blocked_head.get(&id)?;
        if *blocked != head || time_ms.wrapping_sub(*since) < COMMAND_GAP_SKIP_MS {
            return None;
        }
        self.last_acked_seq.insert(id, head.0.wrapping_sub(1));
        self.blocked_head.remove(&id);
        Some(missing)
    }

    pub fn last_acked_seq(&self, id: ClientId) -> Option<CmdSeq> {
        self.last_acked_seq.get(&id).copied().map(CmdSeq)
    }

    pub fn last_consumed(&self, id: ClientId) -> Option<UserCmd> {
        self.last_consumed.get(&id).copied()
    }

    fn known_clients(&self) -> Vec<ClientId> {
        let mut ids: Vec<ClientId> = self
            .queued
            .keys()
            .chain(self.last_consumed.keys())
            .copied()
            .collect();
        ids.sort_by_key(|id| id.0);
        ids.dedup();
        ids
    }

    pub fn take_for_tick(&mut self, time_ms: i32) -> GatheredCommands {
        let mut out = GatheredCommands::default();
        self.release_sim_jitter(time_ms);
        for id in self.known_clients() {
            let playout = self.playout.entry(id).or_default();
            if let Some(queue) = self.queued.get(&id) {
                playout.observe(
                    time_ms,
                    queue
                        .iter()
                        .filter_map(|(seq, cmd, _)| seq.map(|seq| (seq.0, cmd.server_time))),
                );
            }
            let held_ms = if self.local == Some(id) {
                0
            } else {
                self.playout[&id].held_ms()
            };
            // A datagram burst can contain more than one frame of runnable work.
            // Consume the bounded frame budget before measuring remaining debt.
            // Count and blocked-head limits still apply before any simulation.
            if let Some(fault) = self.backlog_fault(id, time_ms)
                && !matches!(fault, InputBacklogFault::QueueDuration { .. })
            {
                out.backlog_faults.push((id, fault));
                continue;
            }
            if let Some(missing) = self.skip_stale_gap(id, time_ms) {
                diag::warn!(
                    Net,
                    "client {}: {missing} commands never arrived; stepped over them after {COMMAND_GAP_SKIP_MS} ms",
                    id.0
                );
            }

            let mut expected = self
                .last_acked_seq
                .get(&id)
                .copied()
                .unwrap_or(0)
                .wrapping_add(1);
            let take = self
                .queued
                .get(&id)
                .map(|queue| {
                    queue
                        .iter()
                        .take_while(|(seq, cmd, _)| {
                            if let Some(seq) = seq {
                                if seq.0 != expected || time_ms.wrapping_sub(cmd.server_time) < held_ms {
                                    return false;
                                }
                                expected = expected.wrapping_add(1);
                            }
                            true
                        })
                        .take(MAX_COMMANDS_PER_PEER_PER_FRAME)
                        .count()
                })
                .unwrap_or(0);
            let mut drained: Vec<_> = self
                .queued
                .get_mut(&id)
                .map(|queue| queue.drain(..take).collect())
                .unwrap_or_default();
            if drained.iter().all(|(seq, _, _)| seq.is_none()) && drained.len() > 1 {
                drained.drain(..drained.len() - 1);
            }
            drained.sort_by_key(|(seq, _, _)| *seq);
            if let Some(playout) = self.playout.get_mut(&id)
                && let Some(previous) = self.last_consumed.get(&id)
                && self.last_acked_seq.contains_key(&id)
            {
                let ran_ms = drained
                    .iter()
                    .filter(|(seq, _, _)| seq.is_some())
                    .map(|(_, cmd, _)| cmd.server_time.wrapping_sub(previous.server_time))
                    .max()
                    .unwrap_or(0);
                playout.note_tick(id, ran_ms);
            }
            if drained.is_empty() {
                if let Some(fault) = self.backlog_fault(id, time_ms) {
                    out.backlog_faults.push((id, fault));
                    continue;
                }
                if let Some(&acked) = self.last_acked_seq.get(&id) {
                    out.acks.push((id, CmdSeq(acked)));
                    continue;
                }
                if let Some(mut cmd) = self.last_consumed.get(&id).copied() {
                    cmd.server_time = time_ms;
                    self.last_consumed.insert(id, cmd);
                    out.proxied.push(id);
                    out.cmds.push((id, cmd));
                }
                continue;
            }
            self.blocked_head.remove(&id);
            let mut ack = None;
            for (seq, mut cmd, sample) in drained {
                cmd.server_time = if seq.is_some() {
                    cmd.server_time
                } else {
                    time_ms
                };
                self.last_consumed.insert(id, cmd);
                if let Some(seq) = seq {
                    self.last_acked_seq.insert(id, seq.0);
                    ack = Some(seq);
                }

                out.samples.push((id, cmd.server_time, sample));
                out.cmds.push((id, cmd));
            }
            if let Some(seq) = ack {
                out.acks.push((id, seq));
            }
            if let Some(fault) = self.backlog_fault(id, time_ms) {
                out.backlog_faults.push((id, fault));
            }
        }
        out
    }
}

pub const MAX_PENDING_ACTIONS_PER_CLIENT: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionEnqueueError {
    QueueFull { pending: usize },

    DuplicateRequestId { request_id: sim::ActionRequestId },
}

impl core::fmt::Display for ActionEnqueueError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::QueueFull { pending } => {
                write!(f, "action queue full ({pending} unresolved requests)")
            }
            Self::DuplicateRequestId { request_id } => {
                write!(
                    f,
                    "request_id {request_id} is already pending with another payload"
                )
            }
        }
    }
}

#[derive(Resource, Debug, Default)]
pub struct ClientActionInbox {
    pending: Vec<(ClientId, sim::ClientAction)>,
    started: HashMap<(ClientId, sim::ActionRequestId), std::time::Instant>,

    received: Vec<(ClientId, sim::ClientAction)>,
    overflowed: std::collections::HashSet<ClientId>,
}

impl ClientActionInbox {
    pub fn push(
        &mut self,
        id: ClientId,
        action: sim::ClientAction,
    ) -> Result<(), ActionEnqueueError> {
        let request_id = sim::action_request_id(&action);
        for (owner, existing) in &self.pending {
            if *owner != id || sim::action_request_id(existing) != request_id {
                continue;
            }
            if *existing == action {
                return Ok(());
            }
            return Err(ActionEnqueueError::DuplicateRequestId { request_id });
        }
        let held = self
            .pending
            .iter()
            .filter(|(owner, _)| *owner == id)
            .count();
        if held >= MAX_PENDING_ACTIONS_PER_CLIENT {
            return Err(ActionEnqueueError::QueueFull { pending: held });
        }
        self.pending.push((id, action));
        Ok(())
    }

    pub fn mark_dispatched(&mut self, id: ClientId, request_id: sim::ActionRequestId) {
        if self
            .pending
            .iter()
            .any(|(owner, action)| *owner == id && sim::action_request_id(action) == request_id)
        {
            self.started
                .entry((id, request_id))
                .or_insert_with(std::time::Instant::now);
        }
    }

    pub fn push_from_peer(&mut self, id: ClientId, action: sim::ClientAction) {
        if self.received.contains(&(id, action)) {
            return;
        }
        if self
            .received
            .iter()
            .filter(|(owner, _)| *owner == id)
            .count()
            >= MAX_PENDING_ACTIONS_PER_CLIENT
        {
            self.overflowed.insert(id);
            return;
        }
        self.received.push((id, action));
    }

    pub fn take_overflowed(&mut self) -> impl Iterator<Item = ClientId> + '_ {
        self.overflowed.drain()
    }

    pub fn timed_out(&self) -> bool {
        self.started
            .values()
            .any(|at| at.elapsed() > std::time::Duration::from_secs(8))
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn pending(&self) -> &[(ClientId, sim::ClientAction)] {
        &self.pending
    }

    pub fn pending_for(&self, id: ClientId) -> impl Iterator<Item = sim::ClientAction> + '_ {
        self.pending
            .iter()
            .filter(move |(owner, _)| *owner == id)
            .map(|(_, action)| *action)
    }

    pub fn retire(&mut self, id: ClientId, request_id: sim::ActionRequestId) -> bool {
        self.started.remove(&(id, request_id));
        let before = self.pending.len();
        self.pending
            .retain(|(owner, action)| *owner != id || sim::action_request_id(action) != request_id);
        self.pending.len() != before
    }

    pub fn retire_client(&mut self, id: ClientId) {
        self.started.retain(|(owner, _), _| *owner != id);
        self.pending.retain(|(owner, _)| *owner != id);
        self.received.retain(|(owner, _)| *owner != id);
        self.overflowed.remove(&id);
    }

    pub fn clear(&mut self) {
        self.started.clear();
        self.pending.clear();
        self.received.clear();
        self.overflowed.clear();
    }

    pub fn gather(&mut self) -> Vec<(ClientId, sim::ClientAction)> {
        let mut out = self.pending.clone();
        for (id, action) in &out {
            self.mark_dispatched(*id, sim::action_request_id(action));
        }
        out.append(&mut self.received);
        out
    }
}

pub fn run_fixed_authority_stream(
    world: &mut sim::SimWorld,
    ticks: u32,
    mut sample: impl FnMut(u32, i32) -> Vec<(ClientId, UserCmd)>,
) -> Vec<sim::Snapshot> {
    let mut clock = AuthorityClock::default();
    let mut out = Vec::with_capacity(ticks as usize);
    for _ in 0..ticks {
        let tick = clock.advance();
        let cmds = sample(tick, clock.time_ms);
        out.push(sim::step(
            world,
            sim::Tick(tick),
            &sim::TickInput::from_cmds(cmds),
            AUTHORITY_MS,
            sim::StepReason::AuthorityFrame,
        ));
    }
    out
}
