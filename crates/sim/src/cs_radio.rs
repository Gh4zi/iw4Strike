//! Counter-Strike radio calls the game makes for a player (`CCSPlayer::Radio`): heard by their
//! team wherever they are.

use crate::frame::FrameWorld;
use crate::world::{ClientId, Tick};
use entity_iw4::{TEAM_ALLIES, TEAM_AXIS};

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
    let teams = matches!(team, TEAM_AXIS | TEAM_ALLIES);
    let audience: Vec<ClientId> = world
        .client_ids_sorted()
        .into_iter()
        .filter(|other| {
            if *other == id {
                csgo
            } else {
                teams && world.client_meta(*other).is_some_and(|m| m.client_state_team == team)
            }
        })
        .collect();
    if audience.is_empty() {
        return;
    }
    diag::info!(Sim, "cs radio: client {} threw {grenade}: {alias} to {} players", id.0, audience.len());
    let origin = world.player(id).map_or([0.0; 3], |ps| ps.origin);
    let index = world.sound_alias_index(&alias);
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
