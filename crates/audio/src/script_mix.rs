use crate::backend::MatchEpoch;
use crate::media::LiveGain;
use bevy::prelude::*;

#[derive(Resource, Default)]
pub(crate) struct ScriptAudioMix {
    epoch: u64,
    pub(crate) gain: LiveGain,
}

impl ScriptAudioMix {
    pub(crate) fn fade(&mut self, frame: u64, to: f32, duration: i32) {
        self.gain.fade(
            frame,
            to,
            duration.max(0) as u64 * u64::from(crate::render_core::SAMPLE_RATE) / 1000,
        );
    }
    pub(crate) fn reset_epoch(&mut self, epoch: u64) {
        if self.epoch != epoch {
            self.epoch = epoch;
            self.gain.set(1.0);
        }
    }
}

struct ChannelGroup {
    active: bool,
    goal: [f32; 64],
}

impl Default for ChannelGroup {
    fn default() -> Self {
        Self {
            active: false,
            goal: [1.0; 64],
        }
    }
}

#[derive(Resource)]
pub(crate) struct ChannelAudioMix {
    epoch: u64,
    groups: [ChannelGroup; 4],
    selected: usize,
    gains: [LiveGain; 64],
    pending: std::collections::VecDeque<sim::ScriptAudioCommand>,
}

impl Default for ChannelAudioMix {
    fn default() -> Self {
        let mut groups = std::array::from_fn(|_| ChannelGroup::default());
        groups[0].active = true;
        Self {
            epoch: 0,
            groups,
            selected: 0,
            gains: std::array::from_fn(|_| LiveGain::default()),
            pending: Default::default(),
        }
    }
}

impl ChannelAudioMix {
    pub(crate) fn bindings(&self) -> [LiveGain; 64] {
        self.gains.clone()
    }
    pub(crate) fn reset_epoch(&mut self, epoch: u64) {
        if self.epoch != epoch {
            self.epoch = epoch;
            self.groups = std::array::from_fn(|_| ChannelGroup::default());
            self.groups[0].active = true;
            self.selected = 0;
            self.pending.clear();
            for gain in &self.gains {
                gain.set(1.0);
            }
        }
    }
    fn apply(&self, frame: u64, fade_ms: i32) {
        let frames = fade_ms.max(0) as u64 * u64::from(crate::render_core::SAMPLE_RATE) / 1000;
        for (gain, goal) in self.gains.iter().zip(self.groups[self.selected].goal) {
            gain.fade(frame, goal, frames);
        }
    }
    fn set(&mut self, frame: u64, priority: u8, goals: &[f32], fade_ms: i32) {
        let group = &mut self.groups[usize::from(priority)];
        group.active = true;
        group.goal[..goals.len()].copy_from_slice(goals);
        self.selected = self
            .groups
            .iter()
            .rposition(|group| group.active)
            .unwrap_or(0);
        if self.selected == usize::from(priority) {
            self.apply(frame, fade_ms);
        }
    }
    fn deactivate(&mut self, frame: u64, priority: u8, fade_ms: i32) {
        self.groups[usize::from(priority)].active = false;
        if self.selected == usize::from(priority) {
            self.selected = self
                .groups
                .iter()
                .rposition(|group| group.active)
                .unwrap_or(0);
            self.apply(frame, fade_ms);
        }
    }
}

fn update_channel_mix(
    mut mix: ResMut<ChannelAudioMix>,
    mut events: MessageReader<net::SvcScriptAudio>,
    bank: Option<Res<crate::SoundBank>>,
    local: Option<Res<net::LocalPresentClient>>,
    epoch: Res<MatchEpoch>,
    runtime: Res<crate::AudioRuntime>,
) {
    let now = runtime.audio_frame();
    mix.reset_epoch(epoch.0);
    for event in events.read() {
        if event.0.target().is_some() {
            mix.pending.push_back(event.0.clone());
        }
    }
    let Some(local) = local else {
        return;
    };
    while let Some(command) = mix.pending.front() {
        if command.target() != Some(local.0) || !command.valid() {
            mix.pending.pop_front();
            continue;
        }
        if let sim::ScriptAudioCommand::ChannelVolumes { .. } = command
            && bank.is_none()
        {
            break;
        }
        let command = mix.pending.pop_front().expect("front command");
        match command {
            sim::ScriptAudioCommand::ChannelVolumes {
                priority,
                volumes,
                fade_ms,
                ..
            } => {
                let Some(bank) = bank.as_ref() else {
                    continue;
                };
                let goals: Option<Vec<_>> = match volumes {
                    Some(volumes) => bank
                        .0
                        .ent_channels
                        .iter()
                        .map(|channel| volumes.get(&channel.name.to_ascii_lowercase()).copied())
                        .collect(),
                    None => Some(vec![0.0; bank.0.ent_channels.len()]),
                };
                let Some(goals) = goals.filter(|goals| !goals.is_empty() && goals.len() <= 64)
                else {
                    diag::warn!(
                        Audio,
                        "audio: channel volume profile is incomplete for the sound bank"
                    );
                    continue;
                };
                mix.set(now, priority, &goals, fade_ms);
            }
            sim::ScriptAudioCommand::DeactivateChannelVolumes {
                priority, fade_ms, ..
            } => mix.deactivate(now, priority, fade_ms),
            _ => {}
        }
    }
}

pub(crate) fn register(app: &mut App) {
    app.init_resource::<ScriptAudioMix>()
        .init_resource::<ChannelAudioMix>()
        .add_systems(Update, update_channel_mix.in_set(net::ClientSet::Effects));
}
