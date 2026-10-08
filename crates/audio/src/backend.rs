use bevy::prelude::*;
use frame::ClientSet;

pub(crate) use crate::render_core::AudioScope;
use crate::runtime::AudioRuntime;

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MatchEpoch(pub u64);

impl MatchEpoch {
    pub fn bump(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }
}

pub(crate) fn register(app: &mut App) {
    app.init_resource::<MatchEpoch>()
        .init_resource::<AudioRuntime>()
        .add_systems(
            Update,
            publish_audio_context
                .after(ClientSet::Load)
                .after(ClientSet::Present)
                .before(ClientSet::Effects),
        )
        .add_systems(
            PostUpdate,
            (
                submit_presented_audio,
                cancel_audio_on_exit.after(submit_presented_audio),
                log_audio_meter,
            ),
        );
}

/// `IW4L_AUDIO_METER=1`: once a second, how loud the mix is (RMS and peak in dB of full scale)
/// and how much of it clipped — for tuning levels without listening.
fn log_audio_meter(runtime: Res<AudioRuntime>, mut last: Local<Option<std::time::Instant>>) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("IW4L_AUDIO_METER").is_some_and(|v| v != "0")) {
        return;
    }
    let now = std::time::Instant::now();
    if last.is_some_and(|last| now.duration_since(last).as_secs_f32() < 1.0) {
        return;
    }
    *last = Some(now);
    let (rms, peak, clipped, samples) = runtime.take_meter();
    let db = |level: f32| 20.0 * level.max(1e-6).log10();
    diag::info!(
        Audio,
        "audio meter: rms {:.1} dBFS peak {:.1} dBFS clipped {clipped}/{samples} ambient_volume {:.2}",
        db(rms),
        db(peak),
        crate::ambient::ambient_volume()
    );
}

fn publish_audio_context(
    epoch: Res<MatchEpoch>,
    runtime: Res<AudioRuntime>,
    clips: Option<Res<crate::ClipStore>>,
    mut mix: Option<ResMut<crate::script_mix::ScriptAudioMix>>,
    mut channels: Option<ResMut<crate::script_mix::ChannelAudioMix>>,
    listeners: Query<&Transform, With<crate::AmbientListener>>,
    generation: Option<Res<frame::WorldGeneration>>,
    presented: Option<Res<net::PresentedSnapshot>>,
    events: Option<Res<net::EntityEventCursor>>,
    local: Option<Res<net::LocalPresentClient>>,
) {
    runtime.set_match_epoch(epoch.0);
    runtime.set_event_context(
        generation
            .and_then(|generation| generation.0)
            .zip(
                presented
                    .as_ref()
                    .and_then(|presented| presented.snapshot().map(|snapshot| snapshot.tick.0)),
            )
            .zip(events.as_ref().map(|events| events.timeline()))
            .map(|((world, tick), timeline)| crate::event::EventContext {
                local_life: local.as_ref().and_then(|local| {
                    let meta = presented.as_ref()?.snapshot()?.meta.for_client(local.0)?;
                    (meta.lifecycle == sim::ClientLifecycle::Alive)
                        .then_some((local.0.0, meta.life_sequence.0))
                }),
                world,
                timeline,
                tick,
            }),
    );
    runtime.set_media_service(clips.as_ref().map(|clips| clips.service()));
    if let Some(mix) = mix.as_mut() {
        mix.reset_epoch(epoch.0);
    }
    if let Some(channels) = channels.as_mut() {
        channels.reset_epoch(epoch.0);
    }
    runtime.set_cue_mix(
        mix.zip(channels)
            .map(|(mix, channels)| crate::cue_execution::CueMix {
                epoch: epoch.0,
                gain: mix.gain.clone(),
                channels: channels.bindings(),
            }),
    );
    let mut listener = listeners.iter();
    let pose = listener.next();
    assert!(listener.next().is_none(), "more than one ambient listener");
    runtime.set_listener(pose.map(|pose| crate::spatial::ListenerSnapshot {
        origin_inches: crate::transform_inches(pose.translation),
        right: (pose.rotation * Vec3::X).to_array(),
    }));
}

fn cancel_audio_on_exit(mut exit: MessageReader<AppExit>, runtime: Res<AudioRuntime>) {
    if exit.read().count() == 0 {
        return;
    }
    runtime.cancel_all();
    diag::info!(
        Audio,
        "audio: admissions queue_full={} logical_budget={} physical_budget={} concurrency={} cancelled={} stale_scope={}",
        runtime.rejection_count(crate::AdmissionFailure::QueueFull),
        runtime.rejection_count(crate::AdmissionFailure::LogicalBudget),
        runtime.rejection_count(crate::AdmissionFailure::PhysicalBudget),
        runtime.rejection_count(crate::AdmissionFailure::Concurrency),
        runtime.rejection_count(crate::AdmissionFailure::Cancelled),
        runtime.rejection_count(crate::AdmissionFailure::StaleScope)
    );
    let stats = runtime.diagnostics();
    diag::info!(
        Audio,
        "audio: exit frames={} device_blocks={} null_blocks={} busy_blocks={} underruns={} peak={:.6} source_revision={} sources={} source_pending_layers={} source_rendered={} source_virtual={} source_dropped={}",
        stats.audio_frame,
        stats.device_blocks,
        stats.null_blocks,
        stats.busy_blocks,
        stats.device_underruns,
        stats.peak,
        stats.source_revision,
        stats.logical_sources,
        stats.pending_source_layers,
        stats.rendered_sources,
        stats.virtual_sources,
        stats.dropped_sources
    );
}

fn submit_presented_audio(
    mut runtime: ResMut<AudioRuntime>,
    epoch: Res<MatchEpoch>,
    settings: Option<Res<frame::GameSettings>>,
    listeners: Query<&Transform, With<crate::AmbientListener>>,
    destructibles: Option<Res<crate::destructible_loops::DestructibleSources>>,
    map: Option<Res<crate::ambient::MapSources>>,
    menu: Option<Res<crate::frontend::MenuSources>>,
    shellshock: Option<Res<crate::shellshock::ShellshockSources>>,
    breath: Option<Res<crate::breath::BreathSources>>,
) {
    runtime.set_match_epoch(epoch.0);
    let mut listener = listeners.iter();
    let pose = listener.next();
    assert!(listener.next().is_none(), "more than one ambient listener");
    runtime.set_listener(pose.map(|pose| crate::spatial::ListenerSnapshot {
        origin_inches: crate::transform_inches(pose.translation),
        right: (pose.rotation * Vec3::X).to_array(),
    }));
    if let Some(settings) = settings {
        runtime.set_master_volume(settings.master_volume);
        crate::ambient::set_ambient_volume(settings.ambient_volume);
        asset_audio::set_timer_warning_volume(settings.timer_warning_volume);
    }
    let mut desired = Vec::new();
    if let Some(destructibles) = destructibles {
        desired.extend(destructibles.desired.iter().cloned());
    }
    if let Some(map) = map {
        desired.extend(map.desired.iter().cloned());
    }
    if let Some(menu) = menu {
        desired.extend(menu.source.iter().cloned());
    }
    if let Some(shellshock) = shellshock {
        desired.extend(shellshock.source.iter().cloned());
    }
    if let Some(breath) = breath {
        desired.extend(breath.source.iter().cloned());
    }
    runtime.set_sources(desired);
}
