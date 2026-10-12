//! Counter-Strike radio calls the game makes for a player (`CCSPlayer::Radio`): heard by their
//! team wherever they are.

use crate::frame::FrameWorld;
use crate::match_state::ClientLifecycle;
use crate::world::{ClientId, Tick};
use entity_iw4::{TEAM_ALLIES, TEAM_AXIS};

/// How far a ping reaches along the crosshair.
const PING_RANGE: f32 = 8192.0;
/// A second ping this soon after one is a double tap: a danger ping.
const PING_DOUBLE_TAP_MS: i32 = 350;
/// The ping's sound (`UI.PlayerPing`): at the spot, carrying as far as a gunshot, quiet.
const PING_SOUND: &str = "csgo/ui.playerping";

/// A ping (`player_ping`, middle mouse): the spot the crosshair is on, for the living player's
/// team to see for [`weapon_iw4::cs::CS_PING_SECONDS`] on screen and on the radar, with CS:GO's
/// ping sound there. A double tap makes it a danger ping.
pub(crate) fn player_ping(world: &mut FrameWorld, tick: Tick, id: ClientId) {
    let Some(meta) = world.client_meta(id) else {
        return;
    };
    let team = meta.client_state_team;
    if meta.lifecycle != ClientLifecycle::Alive || !matches!(team, TEAM_AXIS | TEAM_ALLIES) {
        return;
    }
    let previous = meta.cs_ping;
    let Some(ps) = world.player(id).copied() else {
        return;
    };
    let eye = [ps.origin[0], ps.origin[1], ps.origin[2] + ps.view_height_current];
    let (forward, _, _) = math_iw4::angle_vectors(ps.viewangles);
    let end: [f32; 3] = core::array::from_fn(|i| eye[i] + forward[i] * PING_RANGE);
    let trace = world.trace_static_world(eye, end, [0.0; 3], [0.0; 3], crate::MASK_BULLET_WORLD);
    if trace.fraction >= 1.0 {
        return;
    }
    let now = crate::level_time_ms(tick);
    let danger = previous
        .is_some_and(|ping| !ping.danger && now - ping.placed_ms <= PING_DOUBLE_TAP_MS);
    let origin = trace.endpos;
    world.client_meta_mut(id).cs_ping = Some(crate::CsPing {
        origin,
        placed_ms: now,
        danger,
    });
    let audience = team_audience(world, id, team, true);
    diag::info!(
        Sim,
        "cs ping: client {} pinged {origin:.0?}{} for {} players",
        id.0,
        if danger { " (danger)" } else { "" },
        audience.len()
    );
    send_sound(world, tick, audience, PING_SOUND, origin);
}

/// The players on `team` (with `id` or not); nobody else when `team` is not a side.
fn team_audience(world: &FrameWorld, id: ClientId, team: i32, with_self: bool) -> Vec<ClientId> {
    let teams = matches!(team, TEAM_AXIS | TEAM_ALLIES);
    world
        .client_ids_sorted()
        .into_iter()
        .filter(|other| {
            if *other == id {
                with_self
            } else {
                teams && world.client_meta(*other).is_some_and(|m| m.client_state_team == team)
            }
        })
        .collect()
}

/// Sends sound alias `alias` at `origin` to `audience`.
fn send_sound(world: &mut FrameWorld, tick: Tick, audience: Vec<ClientId>, alias: &str, origin: [f32; 3]) {
    if audience.is_empty() {
        return;
    }
    let index = world.sound_alias_index(alias);
    world.push_entity_event(
        tick,
        crate::EventAudience::Clients(audience),
        entity_iw4::EntityEventKind::SOUND_ALIAS,
        crate::EntityEventPayload {
            number: i32::from(trace_iw4::ENTITYNUM_WORLD),
            event_parm: i32::from(index),
            origin,
            ..Default::default()
        },
    );
}

/// A grenade throw's call (`CBaseCSGrenade::ThrowGrenade`). CS:GO: "Fire in the hole!",
/// "Flashbang out!" or "Smoke out!" in the voice of the faction the thrower's team wears, to the
/// whole team. CS 1.6: "Fire in the hole!" for any grenade to the teammates (the thrower plays it
/// for themselves).
pub(crate) fn grenade_call(world: &mut FrameWorld, tick: Tick, id: ClientId, grenade: &str) {
    use weapon_iw4::cs::{CSGO_RADIO_ALIAS_PREFIX, csgo_grenade_radio, csgo_radio_voice};
    let Some(team) = world.client_meta(id).map(|meta| meta.client_state_team) else {
        return;
    };
    let csgo = crate::cs_settings::shooting_mode() != weapon_iw4::csgo::SHOOTING_CS16;
    let alias = if csgo {
        let allies = team == TEAM_ALLIES;
        let faction = game_faction(world, allies).unwrap_or_default();
        format!(
            "{CSGO_RADIO_ALIAS_PREFIX}{}/{}",
            csgo_radio_voice(&faction, allies),
            csgo_grenade_radio(grenade)
        )
    } else {
        "css/radio.fireinthehole".to_owned()
    };
    let audience = team_audience(world, id, team, csgo);
    if audience.is_empty() {
        return;
    }
    diag::info!(Sim, "cs radio: client {} threw {grenade}: {alias} to {} players", id.0, audience.len());
    let origin = world.player(id).map_or([0.0; 3], |ps| ps.origin);
    send_sound(world, tick, audience, &alias, origin);
}

/// The MW2 faction a team wears on this map (`game["allies"]` / `game["axis"]`).
fn game_faction(world: &mut FrameWorld, allies: bool) -> Option<String> {
    let runtime = world.ecs().get_resource::<crate::script::Runtime>()?;
    let name = if allies { "allies" } else { "axis" };
    match crate::script::host::restart::game_field(runtime, name)? {
        crate::script::Value::String(faction) => {
            Some(String::from_utf8_lossy(faction.as_bytes()).to_ascii_lowercase())
        }
        _ => None,
    }
}
