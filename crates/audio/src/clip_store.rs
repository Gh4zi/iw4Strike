use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, SyncSender, TrySendError, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use asset_audio::SoundCatalog;
use asset_core::AssetNamespace;
use assets::NamespaceSoundIwd;
use bevy::prelude::*;

use crate::media::PcmBuffer;
use crate::pcm::decode_audio_bytes;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ClipKey {
    Loaded(usize),
    Streamed {
        ns: AssetNamespace,
        dir: String,
        name: String,
    },
}

/// A queued clip and the instant it was queued, so a worker can say how long
/// the job sat before anyone picked it up. The wait is the queue's, not the
/// clip's: it is what the store owed and could not pay yet.
struct ClipJob {
    key: ClipKey,
    queued_at: Instant,
}

/// Which decoder a clip went through. Every clip takes exactly one of these,
/// and they cost wildly different things — an external process per XWMA clip,
/// a loop over bytes for everything else — so the report separates them rather
/// than averaging one number over all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipPath {
    /// Linear PCM read in place from the zone.
    Pcm,
    /// T5 ADPCM, decoded in process.
    Adpcm,
    /// T5 XWMA, decoded by an external `ffmpeg` or read from the artifact cache.
    Xwma,
    /// A clip read out of an IWD rather than out of the zone.
    Streamed,
    /// No decoder claims the key: the bank holds nothing at that index, or it
    /// holds a clip in a format none of the four above reads. Counted here so
    /// it cannot hide inside another path's failures.
    Unresolved,
}

impl ClipPath {
    pub const COUNT: usize = Self::Unresolved as usize + 1;

    pub const ALL: [Self; Self::COUNT] = [
        Self::Pcm,
        Self::Adpcm,
        Self::Xwma,
        Self::Streamed,
        Self::Unresolved,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Pcm => "pcm",
            Self::Adpcm => "t5 adpcm",
            Self::Xwma => "t5 xwma",
            Self::Streamed => "streamed",
            Self::Unresolved => "unresolved",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ClipError {
    Decode,
    InvalidPcm(crate::media::PcmError),
    Read,
    QueueClosed,
    QueueFull,
    RequestLimit,
}

impl From<crate::pcm::DecodeError> for ClipError {
    fn from(error: crate::pcm::DecodeError) -> Self {
        match error {
            crate::pcm::DecodeError::Decode => Self::Decode,
            crate::pcm::DecodeError::Pcm(error) => Self::InvalidPcm(error),
        }
    }
}

pub(crate) enum MediaRequest {
    Existing,
    Resident,
    Submitted,
    Deferred,
    Refused,
}

// Bump when PCM decode, layout or trim rules change.
const PCM_CONVERSION_REVISION: u32 = 1;

#[derive(Clone, PartialEq, Eq, Hash)]
struct LoadedClipKey {
    content: [u8; 32],
    conversion_revision: u32,
    format: i32,
    rate: u32,
    bits: i32,
    channels: i32,
    samples: u32,
    block_size: u32,
    seek_table: Vec<u32>,
}

fn resident_clip_key(bank: &SoundCatalog, key: &ClipKey) -> Option<(LoadedClipKey, bool)> {
    let ClipKey::Loaded(index) = key else {
        return None;
    };
    let sound = bank.pcm_at(*index)?;
    let common = matches!(
        sound.zone.as_str(),
        "code_post_gfx_mp"
            | "localized_code_post_gfx_mp"
            | "patch_mp"
            | "common_mp"
            | "localized_common_mp"
    );
    Some((
        LoadedClipKey {
            content: sound.encoded_content_id(),
            conversion_revision: PCM_CONVERSION_REVISION,
            format: sound.format(),
            rate: sound.rate,
            bits: sound.bits(),
            channels: sound.channels(),
            samples: sound.samples,
            block_size: sound.block_size,
            seek_table: sound.seek_table.clone(),
        },
        common,
    ))
}

struct PreparedClip {
    pcm: PcmBuffer,
    common: bool,
}

#[derive(Default)]
struct PreparedClipCacheInner {
    profile_id: u64,
    bank: std::sync::Weak<SoundCatalog>,
    prepared: HashMap<LoadedClipKey, PreparedClip>,
}

#[derive(Clone, Default)]
struct PreparedClipCache(Arc<Mutex<PreparedClipCacheInner>>);

#[derive(Resource, Clone, Default)]
pub(crate) struct ResidentClipCache {
    prepared: PreparedClipCache,
}

impl ResidentClipCache {
    fn use_profile(&self, profile_id: u64) {
        self.prepared.use_profile(profile_id);
    }
    fn use_bank(&self, bank: &Arc<SoundCatalog>) {
        self.prepared.use_bank(bank);
    }
}

impl PreparedClipCache {
    fn use_profile(&self, profile_id: u64) {
        let mut inner = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        if inner.profile_id != profile_id {
            inner.prepared.clear();
            inner.profile_id = profile_id;
        }
    }

    fn use_bank(&self, bank: &Arc<SoundCatalog>) {
        let mut inner = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        if std::sync::Weak::ptr_eq(&inner.bank, &Arc::downgrade(bank)) {
            return;
        }
        inner.prepared.retain(|_, entry| entry.common);
        inner.bank = Arc::downgrade(bank);
    }

    fn ready(&self, profile_id: u64, key: &LoadedClipKey, common: bool) -> Option<PcmBuffer> {
        let mut inner = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        if profile_id == 0 || inner.profile_id != profile_id {
            return None;
        }
        let entry = inner.prepared.get_mut(key)?;
        entry.common |= common;
        Some(entry.pcm.clone())
    }

    fn remember(&self, profile_id: u64, key: LoadedClipKey, common: bool, pcm: PcmBuffer) {
        let mut inner = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        if profile_id != 0 && inner.profile_id == profile_id {
            inner
                .prepared
                .entry(key)
                .and_modify(|entry| entry.common |= common)
                .or_insert(PreparedClip { pcm, common });
        }
    }
}

/// How many queued jobs a worker takes at once. It bounds how long the store
/// can hold a clip's outcome back — every job in a batch is published when the
/// last of them is prepared — and it is the XWMA decoder's batch, which is the
/// only decoder here that gains anything from the grouping.
pub const PREP_BATCH: usize = 64;

pub(crate) const MEDIA_REQUEST_LIMIT: usize = 4096;
const MEDIA_QUEUE_CAPACITY: usize = 256;

struct ClipWorkers {
    handles: Vec<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

impl Drop for ClipWorkers {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let mut joined = 0;
        for handle in self.handles.drain(..) {
            if handle.join().is_err() {
                diag::warn!(Audio, "audio: clip prep worker panicked during retirement");
            }
            joined += 1;
        }
        if joined != 0 {
            diag::info!(Audio, "audio: clip prep workers joined={joined}");
        }
    }
}

#[derive(Resource)]
pub struct ClipStore {
    service: MediaService,
}

#[derive(Clone)]
pub(crate) struct MediaService(Arc<MediaServiceInner>);

struct MediaServiceInner {
    bank: Arc<SoundCatalog>,
    // Closing the job sender releases workers waiting in recv during retirement.
    tx: SyncSender<ClipJob>,
    requests: Arc<Mutex<MediaRequests>>,
    outcomes: Arc<Mutex<Outcomes>>,
    workers: Option<ClipWorkers>,
    retirement: Sender<ClipWorkers>,
    clip_cache: Option<PreparedClipCache>,
    common_profile_id: u64,
}

impl Drop for MediaServiceInner {
    fn drop(&mut self) {
        if let Some(workers) = self.workers.take() {
            workers.stop.store(true, Ordering::Release);
            if let Err(error) = self.retirement.send(workers) {
                let mut workers = error.0;
                // Joining here would stall control; sender closure lets stopped workers exit.
                workers.handles.clear();
            }
        }
    }
}

type Outcomes = HashMap<ClipKey, (Result<PcmBuffer, ClipError>, u64)>;

static USE_TICK: AtomicU64 = AtomicU64::new(0);

fn evict_idle_streamed(requests: &Mutex<MediaRequests>, outcomes: &Mutex<Outcomes>) -> usize {
    let mut requests = requests.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut outcomes = outcomes.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut idle: Vec<(u64, ClipKey, usize)> = outcomes
        .iter()
        .filter_map(|(key, (result, used))| match (key, result) {
            (ClipKey::Streamed { .. }, Ok(pcm)) if !pcm.shared() => {
                Some((*used, key.clone(), pcm.resident_bytes()))
            }
            _ => None,
        })
        .collect();
    idle.sort_unstable_by_key(|(used, ..)| *used);
    let goal = crate::pcm_memory().limit_bytes / 4;
    let mut freed = 0;
    for (_, key, bytes) in idle {
        if freed >= goal {
            break;
        }
        outcomes.remove(&key);
        requests.queued.remove(&key);
        freed += bytes;
    }
    EVICTED_BYTES.fetch_add(freed as u64, Ordering::Relaxed);
    freed
}

#[derive(Default)]
struct MediaRequests {
    queued: HashSet<ClipKey>,
    reused_clips: usize,
    reused_bytes: u64,
    match_live: bool,
    late_prepares: u32,
}

impl ClipStore {
    pub fn start(bank: Arc<SoundCatalog>, iwd: Option<Arc<NamespaceSoundIwd>>) -> Self {
        Self {
            service: MediaService::start_with_common(bank, iwd, 0, None),
        }
    }

    pub(crate) fn start_with_common(
        bank: Arc<SoundCatalog>,
        iwd: Option<Arc<NamespaceSoundIwd>>,
        common_profile_id: u64,
        clip_cache: Option<ResidentClipCache>,
    ) -> Self {
        if let Some(cache) = &clip_cache {
            cache.use_profile(common_profile_id);
            cache.use_bank(&bank);
        }
        let prepared = clip_cache.as_ref().map(|cache| cache.prepared.clone());
        Self {
            service: MediaService::start_with_common(bank, iwd, common_profile_id, prepared),
        }
    }

    pub(crate) fn service(&self) -> MediaService {
        self.service.clone()
    }
    pub fn workers(&self) -> usize {
        self.service.workers()
    }
    pub fn reused_resident(&self) -> (usize, u64) {
        self.service.reused_resident()
    }
    pub fn arm_match_live(&mut self) {
        self.service.arm_match_live();
    }
    pub fn late_prepares(&self) -> u32 {
        self.service.late_prepares()
    }
    pub(crate) fn prefetch(&mut self, key: ClipKey) -> MediaRequest {
        self.service.request_with_policy(key, true)
    }
    pub(crate) fn ready(&self, key: &ClipKey) -> Option<Result<PcmBuffer, ClipError>> {
        self.service.ready(key)
    }
}

impl MediaService {
    fn start_with_common(
        bank: Arc<SoundCatalog>,
        iwd: Option<Arc<NamespaceSoundIwd>>,
        common_profile_id: u64,
        clip_cache: Option<PreparedClipCache>,
    ) -> Self {
        if let Some(cache) = &clip_cache {
            cache.use_profile(common_profile_id);
            cache.use_bank(&bank);
        }
        let (retirement, retired_workers) = channel::<ClipWorkers>();
        if let Err(error) = std::thread::Builder::new()
            .name("audio-media-retire".into())
            .spawn(move || {
                if let Ok(workers) = retired_workers.recv() {
                    drop(workers);
                }
            })
        {
            diag::warn!(
                Audio,
                "audio: media retirement thread not started ({error})"
            );
        }
        let workers = std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(2).clamp(1, 4))
            .unwrap_or(1);
        let (tx, rx) = sync_channel::<ClipJob>(MEDIA_QUEUE_CAPACITY);
        let rx = Arc::new(Mutex::new(rx));
        let outcomes = Arc::new(Mutex::new(Outcomes::new()));
        let requests = Arc::new(Mutex::new(MediaRequests::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::with_capacity(workers);
        for slot in 0..workers {
            let rx = Arc::clone(&rx);
            let bank = Arc::clone(&bank);
            let iwd = iwd.clone();
            let outcomes = Arc::clone(&outcomes);
            let requests = Arc::clone(&requests);
            let clip_cache = clip_cache.clone();
            let stop_worker = Arc::clone(&stop);

            let spawned = std::thread::Builder::new()
                .name(format!("clip-prep-{slot}"))
                .spawn(move || {
                    loop {
                        if stop_worker.load(Ordering::Acquire) {
                            return;
                        }
                        // A match queues every clip it needs at once, and one
                        // of the decoders below is an external process whose
                        // startup costs more than the decode. So a worker takes
                        // what is already waiting rather than one job at a time:
                        // the queue is where the batch comes from. A clip asked
                        // for on its own — a late prepare, a reload — still
                        // arrives alone and is prepared alone.
                        let jobs = {
                            let guard = rx.lock().unwrap_or_else(|poison| poison.into_inner());
                            let Ok(first) = guard.recv() else {
                                return;
                            };
                            let mut jobs = vec![first];
                            while jobs.len() < PREP_BATCH {
                                let Ok(next) = guard.try_recv() else { break };
                                jobs.push(next);
                            }
                            jobs
                        };
                        if stop_worker.load(Ordering::Acquire) {
                            return;
                        }
                        for job in &jobs {
                            QUEUE_WAIT_NS.fetch_add(
                                job.queued_at.elapsed().as_nanos() as u64,
                                Ordering::Relaxed,
                            );
                        }
                        let mut prepared = prepare_jobs(&bank, iwd.as_deref(), &jobs);
                        for (job, result) in jobs.iter().zip(prepared.iter_mut()) {
                            if matches!(
                                result,
                                Err(ClipError::InvalidPcm(crate::media::PcmError::MemoryLimit))
                            ) && evict_idle_streamed(&requests, &outcomes) != 0
                            {
                                let (path, retry) =
                                    prepare_clip_now(&bank, iwd.as_deref(), &job.key);
                                note_outcome(path, retry.as_ref());
                                *result = retry;
                            }
                        }
                        if stop_worker.load(Ordering::Acquire) {
                            return;
                        }
                        if let Some(cache) = &clip_cache {
                            for (job, result) in jobs.iter().zip(&prepared) {
                                if let Ok(pcm) = result
                                    && let Some((key, common)) = resident_clip_key(&bank, &job.key)
                                {
                                    cache.remember(common_profile_id, key, common, pcm.clone());
                                }
                            }
                        }
                        let mut guard =
                            outcomes.lock().unwrap_or_else(|poison| poison.into_inner());
                        let used = USE_TICK.fetch_add(1, Ordering::Relaxed);
                        for (job, result) in jobs.into_iter().zip(prepared) {
                            guard.insert(job.key, (result, used));
                        }
                    }
                });
            match spawned {
                Ok(handle) => handles.push(handle),
                Err(e) => diag::warn!(Audio, "audio: clip prep worker {slot} not started ({e})"),
            }
        }
        WORKERS.fetch_add(handles.len() as u64, Ordering::Relaxed);
        Self(Arc::new(MediaServiceInner {
            bank,
            tx,
            requests,
            outcomes,
            workers: Some(ClipWorkers { handles, stop }),
            retirement,
            clip_cache,
            common_profile_id,
        }))
    }

    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn bank_revision(&self) -> u64 {
        self.0.bank.revision()
    }

    fn workers(&self) -> usize {
        self.0
            .workers
            .as_ref()
            .map_or(0, |workers| workers.handles.len())
    }

    fn reused_resident(&self) -> (usize, u64) {
        let requests = self
            .0
            .requests
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        (requests.reused_clips, requests.reused_bytes)
    }

    fn arm_match_live(&self) {
        self.0
            .requests
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .match_live = true;
    }

    fn late_prepares(&self) -> u32 {
        self.0
            .requests
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .late_prepares
    }

    pub(crate) fn request(&self, key: ClipKey) -> bool {
        matches!(
            self.request_with_policy(key, false),
            MediaRequest::Submitted
        )
    }

    fn request_with_policy(&self, key: ClipKey, defer_full: bool) -> MediaRequest {
        REQUESTS.fetch_add(1, Ordering::Relaxed);
        let mut requests = self
            .0
            .requests
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if requests.queued.contains(&key) {
            return MediaRequest::Existing;
        }
        if requests.queued.len() == MEDIA_REQUEST_LIMIT {
            REQUEST_LIMIT.fetch_add(1, Ordering::Relaxed);
            return MediaRequest::Refused;
        }
        requests.queued.insert(key.clone());
        if let Some(pcm) = self.0.clip_cache.as_ref().and_then(|cache| {
            resident_clip_key(&self.0.bank, &key)
                .and_then(|(source, common)| cache.ready(self.0.common_profile_id, &source, common))
        }) {
            requests.reused_clips += 1;
            requests.reused_bytes += pcm.resident_bytes() as u64;
            self.0
                .outcomes
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .insert(key, (Ok(pcm), USE_TICK.fetch_add(1, Ordering::Relaxed)));
            return MediaRequest::Resident;
        }
        if requests.match_live && matches!(key, ClipKey::Loaded(_)) {
            requests.late_prepares = requests.late_prepares.saturating_add(1);
            LATE.fetch_add(1, Ordering::Relaxed);
            diag::warn!(Audio, "audio: clip prepare after AudioReady ({key:?})");
        }
        let job = ClipJob {
            key: key.clone(),
            queued_at: Instant::now(),
        };
        match self.0.tx.try_send(job) {
            Ok(()) => {
                QUEUED.fetch_add(1, Ordering::Relaxed);
                MediaRequest::Submitted
            }
            Err(error) => {
                let reason = match error {
                    TrySendError::Full(_) if defer_full => {
                        requests.queued.remove(&key);
                        QUEUE_DEFERRED.fetch_add(1, Ordering::Relaxed);
                        return MediaRequest::Deferred;
                    }
                    TrySendError::Full(_) => {
                        QUEUE_FULL.fetch_add(1, Ordering::Relaxed);
                        ClipError::QueueFull
                    }
                    TrySendError::Disconnected(_) => ClipError::QueueClosed,
                };
                self.0
                    .outcomes
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert(key, (Err(reason), 0));
                MediaRequest::Refused
            }
        }
    }

    pub(crate) fn ready(&self, key: &ClipKey) -> Option<Result<PcmBuffer, ClipError>> {
        let requests = self
            .0
            .requests
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !requests.queued.contains(key) && requests.queued.len() == MEDIA_REQUEST_LIMIT {
            return Some(Err(ClipError::RequestLimit));
        }
        let mut outcomes = self
            .0
            .outcomes
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (result, used) = outcomes.get_mut(key)?;
        *used = USE_TICK.fetch_add(1, Ordering::Relaxed);
        Some(result.clone())
    }
}

pub(crate) struct CueFeedbackEntry {
    pub handle: crate::cue::CueHandle,
}

#[derive(Resource, Default)]
pub(crate) struct CueFeedback {
    pub cues: Vec<CueFeedbackEntry>,
    pub dropped: u64,
}

impl CueFeedback {
    pub(crate) fn push(&mut self, handle: crate::cue::CueHandle) {
        if self.cues.len() == crate::runtime::LOGICAL_INSTANCES {
            self.cues.remove(0);
            self.dropped = self.dropped.saturating_add(1);
        }
        self.cues.push(CueFeedbackEntry { handle });
    }
    pub fn clear(&mut self) {
        self.cues.clear();
    }
}

pub(crate) fn clip_keys_for_alias(
    bank: &SoundCatalog,
    ns: AssetNamespace,
    alias: &str,
) -> Vec<ClipKey> {
    let mut seen_alias = HashSet::new();
    let mut seen_key = HashSet::new();
    let mut out = Vec::new();
    collect_clip_keys(bank, ns, alias, 0, &mut seen_alias, &mut seen_key, &mut out);
    out
}

fn collect_clip_keys(
    bank: &SoundCatalog,
    ns: AssetNamespace,
    alias: &str,
    depth: u8,
    seen_alias: &mut HashSet<String>,
    seen_key: &mut HashSet<ClipKey>,
    out: &mut Vec<ClipKey>,
) {
    if depth > 10 || !seen_alias.insert(alias.to_owned()) {
        return;
    }
    let Some(sound) = bank.sound_in(ns, alias) else {
        return;
    };
    for (vi, row) in sound.aliases.iter().enumerate() {
        if let Some(idx) = row.loaded.bound_index() {
            let key = ClipKey::Loaded(idx);
            if seen_key.insert(key.clone()) {
                out.push(key);
            }
        } else if let Some((sns, dir, name)) = bank.streamed_for_variant(ns, alias, vi) {
            let key = ClipKey::Streamed { ns: sns, dir, name };
            if seen_key.insert(key.clone()) {
                out.push(key);
            }
        }
        if let Some(sec) = row.secondary.as_deref()
            && !sec.is_empty()
        {
            collect_clip_keys(bank, ns, sec, depth + 1, seen_alias, seen_key, out);
        }
    }
}

pub(crate) fn clip_key_for_variant(
    bank: &SoundCatalog,
    ns: AssetNamespace,
    alias: &str,
    bound: Option<usize>,
    variant: usize,
    loaded_name: Option<&str>,
    loaded_ns: Option<AssetNamespace>,
) -> Option<ClipKey> {
    if let (Some(name), Some(loaded_ns)) = (loaded_name, loaded_ns)
        && let Some(idx) = bank.loaded_index_in(loaded_ns, name)
    {
        return Some(ClipKey::Loaded(idx));
    }
    let sound = match bound {
        Some(index) => bank.sound_at(index),
        None => bank.sound_in(ns, alias),
    };
    if let Some(idx) = sound
        .and_then(|s| s.aliases.get(variant))
        .and_then(|row| row.loaded.bound_index())
    {
        return Some(ClipKey::Loaded(idx));
    }
    match bound {
        Some(index) => bank.streamed_for_variant_at(index, variant),
        None => bank.streamed_for_variant(ns, alias, variant),
    }
    .map(|(sns, dir, name)| ClipKey::Streamed { ns: sns, dir, name })
}

static WORKERS: AtomicU64 = AtomicU64::new(0);
static REQUESTS: AtomicU64 = AtomicU64::new(0);
static QUEUE_DEFERRED: AtomicU64 = AtomicU64::new(0);
static QUEUE_FULL: AtomicU64 = AtomicU64::new(0);
static REQUEST_LIMIT: AtomicU64 = AtomicU64::new(0);
static QUEUED: AtomicU64 = AtomicU64::new(0);
static QUEUE_WAIT_NS: AtomicU64 = AtomicU64::new(0);
static LATE: AtomicU64 = AtomicU64::new(0);
static EVICTED_BYTES: AtomicU64 = AtomicU64::new(0);
static PREPARED: [AtomicU64; ClipPath::COUNT] = [const { AtomicU64::new(0) }; ClipPath::COUNT];
static FAILED: [AtomicU64; ClipPath::COUNT] = [const { AtomicU64::new(0) }; ClipPath::COUNT];
static WALL_NS: [AtomicU64; ClipPath::COUNT] = [const { AtomicU64::new(0) }; ClipPath::COUNT];
static SAMPLE_BYTES: [AtomicU64; ClipPath::COUNT] = [const { AtomicU64::new(0) }; ClipPath::COUNT];

/// One decoder's share of the clip preparation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClipPathCost {
    pub prepared: u64,
    pub failed: u64,
    /// Worker time inside this decoder, summed over the prep threads.
    pub wall_ms: f64,
    pub sample_bytes: u64,
}

/// What preparing the match's clips cost this process.
///
/// `requests` counts asks and `queued` counts jobs: the walk reaches one clip
/// from several aliases and several weapons, and the difference between the
/// two is what the store's own dedupe already saves. Every duration is summed
/// over the prep workers, which run several at a time, so none of them is a
/// stretch of the load.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipPrepCost {
    /// Prep threads that actually started, across every store this process
    /// opened. A match teardown closes one store and a reload opens another.
    pub workers: u64,
    pub requests: u64,
    pub queued: u64,
    pub queue_full: u64,
    pub queue_deferred: u64,
    pub request_limit: u64,
    pub queue_wait_ms: f64,
    pub late: u64,
    pub evicted_bytes: u64,
    pub paths: Vec<(ClipPath, ClipPathCost)>,
}

pub fn clip_prep_cost() -> ClipPrepCost {
    ClipPrepCost {
        workers: WORKERS.load(Ordering::Relaxed),
        requests: REQUESTS.load(Ordering::Relaxed),
        queued: QUEUED.load(Ordering::Relaxed),
        queue_full: QUEUE_FULL.load(Ordering::Relaxed),
        queue_deferred: QUEUE_DEFERRED.load(Ordering::Relaxed),
        request_limit: REQUEST_LIMIT.load(Ordering::Relaxed),
        queue_wait_ms: QUEUE_WAIT_NS.load(Ordering::Relaxed) as f64 / 1.0e6,
        late: LATE.load(Ordering::Relaxed),
        evicted_bytes: EVICTED_BYTES.load(Ordering::Relaxed),
        paths: ClipPath::ALL
            .into_iter()
            .map(|path| {
                let slot = path as usize;
                (
                    path,
                    ClipPathCost {
                        prepared: PREPARED[slot].load(Ordering::Relaxed),
                        failed: FAILED[slot].load(Ordering::Relaxed),
                        wall_ms: WALL_NS[slot].load(Ordering::Relaxed) as f64 / 1.0e6,
                        sample_bytes: SAMPLE_BYTES[slot].load(Ordering::Relaxed),
                    },
                )
            })
            .collect(),
    }
}

fn note_prepared(path: ClipPath, prepare_at: Instant, result: Result<&PcmBuffer, &ClipError>) {
    note_wall(path, prepare_at.elapsed());
    note_outcome(path, result);
}

/// Worker time this decoder spent, whether it spent it on one clip or on the
/// sixty-four it was handed together. A batched decoder has no per-clip time to
/// report — what it has is a total and a count, and the report divides them.
fn note_wall(path: ClipPath, wall: std::time::Duration) {
    WALL_NS[path as usize].fetch_add(wall.as_nanos() as u64, Ordering::Relaxed);
}

fn note_outcome(path: ClipPath, result: Result<&PcmBuffer, &ClipError>) {
    let slot = path as usize;
    match result {
        Ok(prepared) => {
            PREPARED[slot].fetch_add(1, Ordering::Relaxed);
            SAMPLE_BYTES[slot].fetch_add(prepared.resident_bytes() as u64, Ordering::Relaxed);
        }
        Err(_) => {
            FAILED[slot].fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// One worker's drained jobs, prepared together.
///
/// The XWMA clips among them go to their decoder in one call, because that
/// decoder leaves the process: one `ffmpeg` for the batch instead of one per
/// clip. Every other path is a loop over bytes in this process and is prepared
/// one clip at a time, exactly as before.
fn prepare_jobs(
    bank: &SoundCatalog,
    iwd: Option<&NamespaceSoundIwd>,
    jobs: &[ClipJob],
) -> Vec<Result<PcmBuffer, ClipError>> {
    let mut out: Vec<Option<Result<PcmBuffer, ClipError>>> = vec![None; jobs.len()];

    let asked: Vec<(usize, &asset_audio::LoadedSoundPcm)> = jobs
        .iter()
        .enumerate()
        .filter_map(|(i, job)| match job.key {
            ClipKey::Loaded(index) => {
                let sound = bank.pcm_at(index)?;
                (loaded_path(sound) == Some(ClipPath::Xwma)).then_some((i, sound))
            }
            ClipKey::Streamed { .. } => None,
        })
        .collect();
    if !asked.is_empty() {
        let clips: Vec<asset_audio::XwmaClip<'_>> = asked
            .iter()
            .map(|(_, sound)| asset_audio::XwmaClip {
                packets: sound.encoded_bytes(),
                seek_table: &sound.seek_table,
                channels: sound.channels().max(0) as u32,
                rate: sound.rate,
            })
            .collect();
        let decode_at = Instant::now();
        let decoded = asset_audio::decode_t5_xwma_batch(&clips);
        note_wall(ClipPath::Xwma, decode_at.elapsed());
        for ((i, sound), pcm) in asked.into_iter().zip(decoded) {
            let result = pcm
                .map_err(|_| ClipError::Decode)
                .and_then(|bytes| pcm_from_s16(&bytes, sound.channels(), sound.rate));
            note_outcome(ClipPath::Xwma, result.as_ref());
            out[i] = Some(result);
        }
    }

    for (i, job) in jobs.iter().enumerate() {
        if out[i].is_some() {
            continue;
        }
        let prepare_at = Instant::now();
        let (path, result) = prepare_clip_now(bank, iwd, &job.key);
        note_prepared(path, prepare_at, result.as_ref());
        out[i] = Some(result);
    }
    out.into_iter()
        .map(|result| result.unwrap_or(Err(ClipError::Decode)))
        .collect()
}

fn prepare_clip_now(
    bank: &SoundCatalog,
    iwd: Option<&NamespaceSoundIwd>,
    key: &ClipKey,
) -> (ClipPath, Result<PcmBuffer, ClipError>) {
    match key {
        ClipKey::Loaded(index) => {
            let Some(sound) = bank.pcm_at(*index) else {
                return (ClipPath::Unresolved, Err(ClipError::Decode));
            };
            let Some(path) = loaded_path(sound) else {
                return (ClipPath::Unresolved, Err(ClipError::Decode));
            };
            (path, prepare_loaded(sound))
        }
        ClipKey::Streamed { ns, dir, name } => {
            (ClipPath::Streamed, prepare_streamed(iwd, *ns, dir, name))
        }
    }
}

/// Which decoder `prepare_loaded` will reach for, decided the same way it
/// decides — the two read the same fields in the same order, so a clip cannot
/// be counted under one path and decoded by another.
fn loaded_path(sound: &asset_audio::LoadedSoundPcm) -> Option<ClipPath> {
    if sound.t5_adpcm_bytes().is_some() {
        Some(ClipPath::Adpcm)
    } else if sound.is_t5_xwma() {
        Some(ClipPath::Xwma)
    } else if sound.format() == asset_audio::MSS_PCM {
        Some(ClipPath::Pcm)
    } else {
        None
    }
}

fn prepare_loaded(sound: &asset_audio::LoadedSoundPcm) -> Result<PcmBuffer, ClipError> {
    if let Some(bytes) = sound.t5_adpcm_bytes() {
        let channels = u16::try_from(sound.channels().max(1)).map_err(|_| ClipError::Decode)?;
        let pcm = crate::pcm::t5_stream::decode_adpcm(
            bytes,
            sound.samples,
            sound.rate,
            u32::from(channels),
        )
        .map_err(ClipError::from)?;
        return Ok(pcm);
    }
    if sound.is_t5_xwma() {
        let decoded = asset_audio::decode_t5_xwma(
            sound.encoded_bytes(),
            &sound.seek_table,
            sound.channels().max(0) as u32,
            sound.rate,
        )
        .map_err(|_| ClipError::Decode)?;
        return pcm_from_s16(&decoded, sound.channels(), sound.rate);
    }
    if sound.format() != 1 {
        return Err(ClipError::Decode);
    }
    let channels = u16::try_from(sound.channels().max(1)).map_err(|_| ClipError::Decode)?;
    PcmBuffer::from_zone(
        sound.encoded_shared(),
        sound.bits(),
        channels,
        sound.rate.max(1),
    )
    .map_err(|error| match error {
        crate::media::PcmError::Empty => ClipError::Decode,
        error => ClipError::InvalidPcm(error),
    })
}

/// Decoder output as the store keeps it: 16-bit interleaved, a whole number of
/// frames, and never empty — a clip with no samples is a failed decode and not
/// a silent clip.
fn pcm_from_s16(bytes: &[u8], channels: i32, rate: u32) -> Result<PcmBuffer, ClipError> {
    let lanes = channels.max(1) as usize;
    let count = bytes.len() / 2 / lanes * lanes;
    if count == 0 {
        return Err(ClipError::Decode);
    }
    let channels = u16::try_from(channels.max(1)).map_err(|_| ClipError::Decode)?;
    let samples = bytes
        .chunks_exact(2)
        .take(count)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    PcmBuffer::from_i16(samples, channels, rate.max(1)).map_err(ClipError::InvalidPcm)
}

fn prepare_streamed(
    iwd: Option<&NamespaceSoundIwd>,
    ns: AssetNamespace,
    dir: &str,
    name: &str,
) -> Result<PcmBuffer, ClipError> {
    let iwd = iwd.ok_or(ClipError::Read)?;
    let rel = format!("{dir}/{name}");
    let bytes: Vec<u8> = match iwd.read_sound(ns, &rel) {
        Some(Ok(bytes)) => bytes,
        Some(Err(e)) => {
            diag::warn!(Audio, "audio: IWD read `{rel}` failed: {e}");
            return Err(ClipError::Read);
        }
        None => {
            diag::warn!(
                Audio,
                "audio: streamed IWD miss `{}:{rel}` (typed gap)",
                ns.as_str()
            );
            return Err(ClipError::Read);
        }
    };
    let pcm = if ns == AssetNamespace::T5 && !bytes.starts_with(b"RIFF") {
        crate::pcm::t5_stream::decode(&bytes)
    } else {
        decode_audio_bytes(&bytes)
    };
    pcm.map_err(ClipError::from)
}
