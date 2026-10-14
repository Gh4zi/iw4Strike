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
    use weapon_iw4::cs::{CSGO_RADIO_ALIAS_PREFIX, csgo_grenade_radio};
    let Some(team) = world.client_meta(id).map(|meta| meta.client_state_team) else {
        return;
    };
    let csgo = crate::cs_settings::shooting_mode() != weapon_iw4::csgo::SHOOTING_CS16;
    let alias = if csgo {
        format!(
            "{CSGO_RADIO_ALIAS_PREFIX}{}/{}",
            csgo_voice(world, team),
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

/// Tells clients each side's MW2 faction (the server info `cs_factions` = "<allies> <axis>"),
/// which CS:GO agents they draw the players as.
pub(crate) fn publish_factions(world: &mut FrameWorld) {
    let (Some(allies), Some(axis)) = (game_faction(world, true), game_faction(world, false)) else {
        return;
    };
    let value = format!("{allies} {axis}");
    if let Some(mut runtime) = world.ecs().get_resource_mut::<crate::script::Runtime>()
        && (runtime.dvars.get("cs_factions") != Some(&value)
            || !runtime.server_info.contains("cs_factions"))
    {
        runtime.server_info.insert("cs_factions".to_owned());
        runtime.dvars.insert("cs_factions".to_owned(), value);
    }
}

/// The CS:GO voice a player on MW2 team `team` speaks in, which is also the agent they are
/// drawn as: their side's ([`weapon_iw4::cs::csgo_side_voice`]), by the side they play
/// ([`weapon_iw4::cs::cs_plays_terrorist`]) as the clients take it from `cs_attackers`.
pub(crate) fn csgo_voice(world: &mut FrameWorld, team: i32) -> &'static str {
    let attackers = world
        .ecs()
        .get_resource::<crate::script::Runtime>()
        .and_then(|runtime| runtime.dvars.get("cs_attackers").cloned());
    let terrorist = weapon_iw4::cs::cs_plays_terrorist(attackers.as_deref(), team == TEAM_ALLIES);
    let allies = game_faction(world, true).unwrap_or_default();
    let axis = game_faction(world, false).unwrap_or_default();
    weapon_iw4::cs::csgo_side_voice(&allies, &axis, terrorist)
}

/// The MW2 faction a team wears on this map (`game["allies"]` / `game["axis"]`).
pub(crate) fn game_faction(world: &mut FrameWorld, allies: bool) -> Option<String> {
    let runtime = world.ecs().get_resource::<crate::script::Runtime>()?;
    let name = if allies { "allies" } else { "axis" };
    match crate::script::host::restart::game_field(runtime, name)? {
        crate::script::Value::String(faction) => {
            Some(String::from_utf8_lossy(faction.as_bytes()).to_ascii_lowercase())
        }
        _ => None,
    }
}
