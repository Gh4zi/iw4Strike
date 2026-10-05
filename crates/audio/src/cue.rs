use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use asset_audio::{SoundCatalog, lerp_range, unit_random};
use asset_core::AssetNamespace;

use crate::admission::{AdmissionPolicy, AliasLimit, ChannelRule, LimitMode};
use crate::clip_store::{ClipKey, MediaService, clip_key_for_variant};
use crate::render_core::AudioScope;
use crate::start::StartFailure;

const SELECTOR_CAPACITY: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CueFailure {
    QueueFull,
    SelectorBudget,
    CompositionBudget,
    PendingBudget,
    MissingAlias,
    NoMedia,
    Cancelled,
    StaleScope,
    DuplicateEvent,
    StaleEvent,
    EventBudget,
}

#[derive(Clone)]
pub(crate) struct ResolvedCue {
    pub bank: Arc<SoundCatalog>,
    pub media: Option<MediaService>,
    pub policy: CueExecutionPolicy,
    pub variant: usize,
    pub clip: Option<ClipKey>,
    pub volume: f32,
    pub pitch: f32,
    pub layer: Option<String>,
}

#[derive(Clone)]
pub(crate) struct CueExecutionPolicy {
    pub looping: bool,
    pub channel: Option<u32>,
    pub priority: Option<asset_audio::VoicePriority>,
    pub spatial: Option<Result<CueSpatialPolicy, StartFailure>>,
    admission: AdmissionPolicy,
}

#[derive(Clone)]
pub(crate) struct CueSpatialPolicy {
    pub dist_min: f32,
    pub dist_max: f32,
    pub knots: Arc<[[f32; 2]]>,
    pub near_knots: Option<Arc<[[f32; 2]]>>,
}

impl CueExecutionPolicy {
    fn lower(bank: &SoundCatalog, namespace: AssetNamespace, index: usize, variant: usize) -> Self {
        let sound = bank.sound_at(index).expect("resolved cue");
        let row = sound.aliases.get(variant);
        let channel = sound.ent_channel(variant);
        let positional = if namespace == AssetNamespace::T5 {
            row.and_then(|row| row.flags)
                .is_some_and(|flags| flags & 2 != 0)
        } else {
            channel
                .and_then(|id| bank.ent_channel(id))
                .is_none_or(|info| info.is_3d)
        };
        let spatial = positional.then(|| {
            let row = row.ok_or(StartFailure::NoPcm)?;
            let curve = row
                .volume_falloff
                .as_ref()
                .ok_or(StartFailure::NoFalloffCurve)?;
            let pack = |knots: &[(f32, f32)]| -> Arc<[[f32; 2]]> {
                knots
                    .iter()
                    .take(asset_iw4::SND_CURVE_MAX_KNOTS)
                    .map(|&(x, y)| [x, y])
                    .collect::<Vec<_>>()
                    .into()
            };
            Ok(CueSpatialPolicy {
                dist_min: row.dist_min,
                dist_max: row.dist_max,
                knots: pack(&curve.knots),
                near_knots: row.near_falloff.as_ref().map(|curve| pack(&curve.knots)),
            })
        });
        let admission = alias_admission(bank, namespace, index, row, channel);
        Self {
            looping: row
                .and_then(|row| row.decoded_flags())
                .is_some_and(|flags| flags.looping()),
            channel,
            priority: row.and_then(|row| row.voice_priority.clone()),
            spatial,
            admission,
        }
    }

    pub(crate) fn admission_for(&self, emitter: Option<u32>, priority: f32) -> AdmissionPolicy {
        AdmissionPolicy {
            emitter,
            priority,
            ..self.admission
        }
    }
}

fn alias_admission(
    bank: &SoundCatalog,
    namespace: AssetNamespace,
    index: usize,
    row: Option<&asset_audio::CapturedAlias>,
    channel: Option<u32>,
) -> AdmissionPolicy {
    let channel = channel.and_then(|id| {
        bank.ent_channel(id).map(|info| ChannelRule {
            id,
            maximum: info.max_voices,
            restricted: info.is_restricted,
        })
    });
    let limit = |shift: u32, count: Option<u8>, per_emitter| {
        if namespace != AssetNamespace::T5 {
            return AliasLimit::default();
        }
        let Some(count) = count else {
            return AliasLimit::default();
        };
        let mode = match (row.and_then(|row| row.flags).unwrap_or(0) >> shift) & 3u32 {
            1 => LimitMode::Oldest,
            2 => LimitMode::Reject,
            3 => LimitMode::Priority,
            _ => LimitMode::Unlimited,
        };
        AliasLimit {
            mode,
            count: if mode == LimitMode::Oldest {
                count.max(1)
            } else {
                count
            },
            per_emitter,
        }
    };
    AdmissionPolicy {
        bank_revision: bank.revision(),
        alias: Some(index),
        emitter: None,
        channel,
        limits: [
            limit(25, row.and_then(|row| row.limit_count), false),
            limit(27, row.and_then(|row| row.entity_limit_count), true),
        ],
        priority: 0.0,
    }
}

pub(crate) struct CueState {
    pub(crate) release: Arc<crate::media::CueRelease>,
    result: OnceLock<Result<ResolvedCue, CueFailure>>,
    completion: OnceLock<crate::StartDecision>,
    pub(crate) playback: OnceLock<Arc<crate::render_core::InstanceState>>,
    children: Mutex<Vec<(String, Arc<CueState>)>>,
}

pub(crate) struct CueHandle(pub Arc<CueState>);

impl CueHandle {
    pub(crate) fn release(&self, frame: u64, frames: u64) {
        self.0.release.release(frame, frames);
    }
    pub(crate) fn active(&self) -> bool {
        self.0.active()
    }
    pub(crate) fn fade_to(&self, frame: u64, to: f32, frames: u64) -> bool {
        self.0.release.fade_to(frame, to, frames)
    }
    pub fn new() -> Self {
        Self(CueState::new())
    }

    pub(crate) fn completion(&self) -> Option<crate::StartDecision> {
        let mut decision = self.0.completion()?;
        if let Some(Ok(cue)) = self.result() {
            decision.detail = Some(self.0.playback.get().map_or_else(
                || format!("bank_revision={}", cue.bank.revision()),
                |instance| {
                    format!(
                        "instance={} bank_revision={}",
                        instance.id,
                        cue.bank.revision()
                    )
                },
            ));
        }
        Some(decision)
    }

    pub fn result(&self) -> Option<Result<ResolvedCue, CueFailure>> {
        self.0.result.get().cloned()
    }
}

impl CueState {
    fn active(&self) -> bool {
        self.playback.get().map_or_else(
            || self.completion.get().is_none(),
            |instance| !instance.has_reached(crate::render_core::InstanceStatus::Retired),
        ) || self
            .children
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
            .any(|(_, child)| child.active())
    }
    fn completion(&self) -> Option<crate::StartDecision> {
        let mut decision = self.completion.get()?.clone();
        for (alias, child) in self
            .children
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
        {
            let secondary = child.completion()?;
            decision.secondary = Some((alias.clone(), secondary.outcome));
        }
        Some(decision)
    }

    pub(crate) fn child(&self, alias: &str) -> Arc<Self> {
        let child = Arc::new(Self {
            release: self.release.clone(),
            result: OnceLock::new(),
            completion: OnceLock::new(),
            playback: OnceLock::new(),
            children: Mutex::new(Vec::new()),
        });
        self.children
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push((alias.into(), child.clone()));
        child
    }

    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            release: Arc::new(crate::media::CueRelease::default()),
            result: OnceLock::new(),
            completion: OnceLock::new(),
            playback: OnceLock::new(),
            children: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn complete(&self, decision: crate::StartDecision) {
        let _ = self.completion.set(decision);
    }
    pub(crate) fn resolved(&self, result: Result<ResolvedCue, CueFailure>) {
        let _ = self.result.set(result);
    }
}

pub(crate) struct CueRequest {
    pub state: Arc<CueState>,
    pub execution: crate::cue_execution::CueIntent,
    pub bank: Arc<SoundCatalog>,
    pub media: Option<MediaService>,
    pub namespace: AssetNamespace,
    pub alias: String,
    pub bound: Option<usize>,
    pub scope: AudioScope,
    pub epoch: u64,
    pub pitch_scale: f32,
}

impl CueRequest {
    pub(crate) fn reject(self, reason: CueFailure) {
        self.state.resolved(Err(reason));
        self.state.complete(crate::StartDecision {
            event: self.execution.event,
            namespace: self.namespace,
            alias: self.alias,
            variant: None,
            outcome: crate::StartOutcome::Failed(crate::StartFailure::CueRefused(reason)),
            secondary: None,
            detail: None,
        });
    }
}

pub(crate) struct CueResolver {
    lcg: u32,
    history: HashMap<(AudioScope, u64, u64, usize), usize>,
}

impl CueResolver {
    pub fn new() -> Self {
        Self {
            lcg: 0x00a5_5a5a,
            history: HashMap::new(),
        }
    }

    pub fn resolve(&mut self, request: &CueRequest) -> Result<ResolvedCue, CueFailure> {
        let bank = &request.bank;
        let index = request
            .bound
            .or_else(|| bank.index_in(request.namespace, &request.alias))
            .ok_or(CueFailure::MissingAlias)?;
        let sound = bank.sound_at(index).ok_or(CueFailure::MissingAlias)?;
        let epoch = if request.scope == AudioScope::Match {
            request.epoch
        } else {
            0
        };
        let key = (request.scope, epoch, bank.revision(), index);
        if self.history.len() == SELECTOR_CAPACITY && !self.history.contains_key(&key) {
            return Err(CueFailure::SelectorBudget);
        }
        let outcome = bank
            .pick_loaded_outcome_at(index, &mut self.lcg, self.history.get(&key).copied())
            .ok_or(CueFailure::MissingAlias)?;
        let variant = outcome.variant_index;
        let row = sound.aliases.get(variant);
        let clip = clip_key_for_variant(
            bank,
            request.namespace,
            &request.alias,
            Some(index),
            variant,
            outcome
                .picked
                .as_ref()
                .map(|picked| picked.sound.name.as_str()),
            outcome
                .picked
                .as_ref()
                .map(|picked| AssetNamespace::from_zone_game(picked.sound.game)),
        );
        let (volume, pitch, layer) = match outcome.picked {
            Some(picked) => (picked.volume, picked.pitch, picked.layer),
            None => {
                let row = row.ok_or(CueFailure::NoMedia)?;
                let volume_random = unit_random(&mut self.lcg);
                let pitch_random = unit_random(&mut self.lcg);
                let volume = bank.alias_volume(request.namespace, row, volume_random);
                let pitch = if row.pitch_min == 0.0 && row.pitch_max == 0.0 {
                    1.0
                } else {
                    lerp_range(row.pitch_min, row.pitch_max, pitch_random)
                };
                (volume, pitch, row.secondary.clone())
            }
        };
        let layer = if request.namespace == AssetNamespace::T5 {
            row.and_then(|row| row.secondary.clone())
        } else {
            layer
        };
        if clip.is_none()
            && (request.namespace != AssetNamespace::T5
                || layer.as_deref().is_none_or(str::is_empty))
        {
            return Err(CueFailure::NoMedia);
        }
        let scale = if request.pitch_scale.is_finite() && request.pitch_scale > 0.0 {
            request.pitch_scale
        } else {
            1.0
        };
        self.history.insert(key, variant);
        Ok(ResolvedCue {
            policy: CueExecutionPolicy::lower(bank, request.namespace, index, variant),
            bank: bank.clone(),
            media: request
                .media
                .clone()
                .filter(|media| media.bank_revision() == bank.revision()),
            variant,
            clip,
            volume,
            pitch: pitch * scale,
            layer,
        })
    }

    pub fn retain_epoch(&mut self, epoch: u64) {
        self.history
            .retain(|(scope, old, _, _), _| *scope != AudioScope::Match || *old == epoch);
    }
}
