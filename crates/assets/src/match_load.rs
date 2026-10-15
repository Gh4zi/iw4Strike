use std::path::PathBuf;

use bevy::prelude::*;
use bevy::tasks::{Task, futures_lite::future};
use frame::{ClientSet, LaunchIdentity, MapLoadApproved, MapLoadFailed};

use crate::map_load_process::MapLoadProcess;
use crate::session_load::{MatchLoadOutcome, PreparedMatch, load_prepared_match};
use asset_transport::progress::LoadProgress;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MapLoadApproval;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MatchLoadDispatch;

#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MatchLoadBusy(pub bool);

#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchLoadAbort(pub u64);

#[derive(Resource)]
pub struct MatchLoadRequest {
    pub request_id: u64,
    pub load_key: frame::LocalLoadKey,

    pub zone: String,
    pub zone_ff: Result<PathBuf, String>,
    pub common_mp: Result<PathBuf, String>,
    pub progress: LoadProgress,
}

#[derive(Resource)]
struct MatchLoadTask {
    task: Task<Option<PreparedMatchReady>>,
    request_id: u64,
    load_key: frame::LocalLoadKey,
    zone: String,
    progress: LoadProgress,
}

#[derive(Resource, Clone, Debug)]
pub struct MatchLoadAccepted {
    pub request_id: u64,
    pub load_key: frame::LocalLoadKey,
    pub zone: String,
}

#[derive(Resource)]
pub struct PreparedMatchReady {
    pub request_id: u64,
    pub load_key: frame::LocalLoadKey,
    pub zone: String,
    pub prepared: PreparedMatch,
}

#[derive(Resource)]
pub struct PreparedMatchSound {
    pub load_key: frame::LocalLoadKey,
    pub zone: String,
    pub common_profile_id: u64,
    pub products_id: u64,
    pub sound: Result<asset_audio::SoundCatalog, String>,
    pub gaps: usize,
}

fn start_match_load(
    mut commands: Commands,
    identity: Option<Res<LaunchIdentity>>,
    request: Option<Res<MatchLoadRequest>>,
    busy: Option<Res<MatchLoadTask>>,
    retiring: Option<Res<frame::Retiring>>,
    abort: Option<Res<MatchLoadAbort>>,
    headless: Option<Res<frame::Headless>>,
    mut waiting_for_retirement: Local<Option<u64>>,
    mut inflight: ResMut<MatchLoadBusy>,
) {
    let headless = headless.is_some();
    let Some(request) = request else {
        return;
    };
    if abort
        .as_ref()
        .is_some_and(|abort| abort.0 == request.request_id)
    {
        *waiting_for_retirement = None;
        commands.remove_resource::<MatchLoadRequest>();
        if busy.is_none() {
            inflight.0 = false;
        }
        diag::info!(
            World,
            "match load: abort request #{} before the walk started",
            request.request_id
        );
        return;
    }
    if busy.is_some() {
        return;
    }
    if retiring.as_ref().is_some_and(|retiring| retiring.busy()) {
        if *waiting_for_retirement != Some(request.request_id) {
            diag::info!(
                World,
                "match load: request #{} waits for retired resources",
                request.request_id
            );
            *waiting_for_retirement = Some(request.request_id);
        }
        return;
    }
    if *waiting_for_retirement == Some(request.request_id) {
        diag::info!(
            World,
            "match load: request #{} resumes after retirement",
            request.request_id
        );
    }
    *waiting_for_retirement = None;
    let zone_ff = request.zone_ff.clone();
    let common_mp = request.common_mp.clone();
    let progress = request.progress.clone();
    let request_id = request.request_id;
    let load_key = request.load_key;
    let zone = request.zone.clone();
    let zone_accepted = zone.clone();
    let zone_task = zone.clone();
    let cancel_handle = progress.clone();
    // The process outlives the overlay and every task this spawns: it is what
    // the table reads, what admission finishes, and what the bench still has
    // once the screen is gone.
    commands.insert_resource(MapLoadProcess::new(
        request_id,
        load_key,
        zone.clone(),
        cancel_handle.clone(),
    ));

    let games = identity.map(|identity| asset_transport::GamesRoot(identity.games_root.clone()));
    let task = crate::session_load::load_pool().spawn(async move {
        let sound_path = zone_ff.clone();
        match load_prepared_match(zone_ff, common_mp, progress.clone()).await {
            MatchLoadOutcome::Ready(mut prepared) => {
                if progress.is_canceled() {
                    return None;
                }
                prepared.sound = Some(match (games, sound_path) {
                    (Some(games), Ok(path)) => {
                        let sources = asset_audio::gather_sound_sources(&games, &path);
                        let map = prepared
                            .sound
                            .take()
                            .unwrap_or_else(|| Err("the map zone never opened".into()));
                        let bank = asset_audio::compose_sound_bank(
                            sources,
                            &zone,
                            asset_audio::namespace_for_zone(&games, &zone),
                            map,
                        );
                        prepared.sound_gaps = bank.gaps.len();
                        let mut catalog = bank.catalog;
                        if movement_iw4::rules::CS_RULES {
                            release_dropped_weapon_sounds(&prepared.weapons, &mut catalog);
                        }
                        prepared.script_sound_aliases = Some(catalog.script_alias_looping());
                        if headless {
                            // A dedicated server plays nothing: the names stay (the scripts and
                            // the sounds it tells clients to play use them), the audio goes.
                            let freed = catalog.release_audio();
                            diag::info!(
                                Audio,
                                "dedicated: {:.1} MiB of loaded sound freed",
                                freed as f64 / 1048576.0
                            );
                        }
                        Ok(catalog)
                    }
                    _ => Err("no launch identity or map path to compose the sound bank".into()),
                });
                if progress.is_canceled() {
                    return None;
                }
                Some(PreparedMatchReady {
                    request_id,
                    load_key,
                    zone,
                    prepared,
                })
            }
            MatchLoadOutcome::Canceled => None,
        }
    });
    inflight.0 = true;
    commands.insert_resource(MatchLoadAccepted {
        request_id,
        load_key,
        zone: zone_accepted,
    });
    commands.insert_resource(MatchLoadTask {
        task,
        request_id,
        load_key,
        zone: zone_task,
        progress: cancel_handle,
    });
    commands.remove_resource::<MatchLoadRequest>();
}

/// Counter-Strike plays only the MW2 weapons its guns wear, so the `weap_` sounds only the
/// others play (fire, reload, handling) are freed. Other sounds a dropped weapon names,
/// explosions or bounces, can be heard from the map too and stay.
fn release_dropped_weapon_sounds(
    weapons: &asset_game::WeaponRegistry,
    catalog: &mut asset_audio::SoundCatalog,
) {
    let (_, dropped) = weapons.sound_aliases_split(crate::session_load::cs_keeps_weapon);
    let (unbound, freed) = catalog.release_aliases(|alias| {
        let alias = alias.to_ascii_lowercase();
        alias.starts_with("weap_") && dropped.contains(&alias)
    });
    diag::info!(
        Audio,
        "CS weapon sounds: {unbound} MW2-only aliases unbound, {:.1} MiB of loaded sound freed",
        freed as f64 / 1048576.0
    );
}

fn approve_map_load(
    mut commands: Commands,
    mut approved: MessageReader<MapLoadApproved>,
    mut failed: MessageWriter<MapLoadFailed>,
    identity: Res<LaunchIdentity>,
    abort: Option<Res<MatchLoadAbort>>,
) {
    for request in approved.read() {
        if abort
            .as_ref()
            .is_some_and(|abort| abort.0 == request.request_id)
        {
            diag::info!(
                World,
                "match load: abort approved request #{} (`{}`) — no walk",
                request.request_id,
                request.zone
            );
            continue;
        }
        let games = asset_transport::GamesRoot(identity.games_root.clone());
        let found = asset_transport::find_zone_file(&games, &request.zone);
        let zone = found
            .as_ref()
            .map(|zone| zone.zone_name.clone())
            .unwrap_or_else(|_| request.zone.clone());
        let zone_ff = found.map(|zone| zone.path);
        let common_mp = match &zone_ff {
            Ok(path) => asset_transport::find_runtime_common_mp(&games, path).map(|zone| zone.path),
            Err(error) => Err(error.clone()),
        };
        let error = zone_ff
            .as_ref()
            .err()
            .or_else(|| common_mp.as_ref().err())
            .cloned();
        if let Some(error) = error {
            failed.write(MapLoadFailed {
                request_id: request.request_id,
                load_key: request.load_key,
                zone,
                error,
            });
            continue;
        }
        // Its own state, never the live map's: tasks from the request before
        // this one still hold handles, and they must not land in these rows.
        let progress = LoadProgress::new(request.request_id);
        commands.insert_resource(MatchLoadRequest {
            request_id: request.request_id,
            load_key: request.load_key,
            zone,
            zone_ff,
            common_mp,
            progress,
        });
    }
}

fn poll_match_load(
    mut commands: Commands,
    mut task: Option<ResMut<MatchLoadTask>>,
    mut inflight: ResMut<MatchLoadBusy>,
    abort: Option<Res<MatchLoadAbort>>,
    mut failed: MessageWriter<MapLoadFailed>,
) {
    let Some(task) = task.as_deref_mut() else {
        return;
    };

    if abort
        .as_ref()
        .is_some_and(|abort| abort.0 == task.request_id)
        && !task.progress.is_canceled()
    {
        task.progress.cancel();
        diag::info!(
            World,
            "match load: cancel in-flight walk request #{} — the walk returns at its next checkpoint",
            task.request_id
        );
    }
    let polled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        future::block_on(future::poll_once(&mut task.task))
    }));
    let outcome = match polled {
        Ok(None) => return,
        Ok(Some(outcome)) => Some(outcome),
        Err(panic) => {
            failed.write(walk_panic_failure(task, panic.as_ref()));
            None
        }
    };
    let request_id = task.request_id;
    inflight.0 = false;
    commands.remove_resource::<MatchLoadTask>();
    commands.remove_resource::<MatchLoadAccepted>();
    let Some(outcome) = outcome else {
        return;
    };
    let Some(mut ready) = outcome else {
        diag::info!(
            World,
            "match load: walk request #{request_id} returned canceled — nothing prepared"
        );
        return;
    };
    if abort.is_some_and(|abort| abort.0 == ready.request_id) {
        diag::info!(
            World,
            "match load: discarded aborted walk request #{} (`{}`)",
            ready.request_id,
            ready.zone
        );
        return;
    }
    commands.insert_resource(PreparedMatchSound {
        load_key: ready.load_key,
        zone: ready.zone.clone(),
        common_profile_id: ready.prepared.materials.common_profile_id,
        products_id: ready.prepared.materials.products_id,
        gaps: ready.prepared.sound_gaps,
        sound: ready
            .prepared
            .sound
            .take()
            .unwrap_or_else(|| Err("the map zone never opened".to_owned())),
    });
    commands.insert_resource(ready);
}

fn walk_panic_failure(task: &MatchLoadTask, panic: &(dyn std::any::Any + Send)) -> MapLoadFailed {
    let message = panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("no message");
    diag::error!(
        World,
        "match load: walk request #{} (`{}`) panicked: {message} — reported as a failed map load",
        task.request_id,
        task.zone
    );
    MapLoadFailed {
        request_id: task.request_id,
        load_key: task.load_key,
        zone: task.zone.clone(),
        error: format!("The load stopped on an internal error: {message}"),
    }
}

fn expire_match_load_abort(
    mut commands: Commands,
    abort: Option<Res<MatchLoadAbort>>,
    task: Option<Res<MatchLoadTask>>,
    request: Option<Res<MatchLoadRequest>>,
    ready: Option<Res<PreparedMatchReady>>,
) {
    if abort.is_none() {
        return;
    }
    if task.is_some() || request.is_some() || ready.is_some() {
        return;
    }
    commands.remove_resource::<MatchLoadAbort>();
}

pub fn register_match_load_systems(app: &mut App) {
    app.init_resource::<MatchLoadBusy>().add_systems(
        Update,
        (
            approve_map_load.in_set(MapLoadApproval),
            (
                start_match_load,
                poll_match_load.after(start_match_load),
                expire_match_load_abort.after(poll_match_load),
            )
                .in_set(MatchLoadDispatch)
                .after(MapLoadApproval),
        )
            .after(frame::SessionSwapApplied)
            .in_set(ClientSet::Load),
    );
}
