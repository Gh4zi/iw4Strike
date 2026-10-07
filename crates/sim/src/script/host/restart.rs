use super::entities::{EntityKind, KeyType};
use crate::frame::FrameWorld;
use crate::script::runtime::{install_level, reset, run_now};
use crate::script::{
    Arc, ArrayKey, BTreeMap, Fault, Location, NativeRegistry, Runtime, StringTable, Value,
};
use bevy_ecs::prelude::World;

#[derive(Clone)]
pub(crate) struct RestartPlan {
    pub natives: NativeRegistry,
    pub entities: Arc<Vec<Vec<(String, String)>>>,
    pub tables: Arc<BTreeMap<String, StringTable>>,
    pub keys: Arc<BTreeMap<String, KeyType>>,
    pub entries: Vec<String>,
    pub schemas: BTreeMap<String, Arc<structured_data_iw4::DefinitionSet>>,
    pub player_data_defaults: Option<Arc<crate::PlayerDataDefaults>>,
}

impl std::fmt::Debug for RestartPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestartPlan")
            .field("entries", &self.entries)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Detached {
    Value(Value),
    Array(Vec<(ArrayKey, Detached)>),
}

const MAX_DEPTH: usize = 64;

/// A string field of the saved `game` array.
fn game_string<'a>(game: &'a [(Arc<str>, Detached)], name: &str) -> Option<&'a str> {
    game.iter().find_map(|(field, value)| match value {
        Detached::Value(Value::String(text)) if &**field == name => Some(&**text),
        _ => None,
    })
}

/// `game["roundsWon"]` as `[_, axis, allies]`.
fn round_wins(game: &[(Arc<str>, Detached)]) -> [i32; 3] {
    let mut won = [0; 3];
    let Some(Detached::Array(rows)) = game
        .iter()
        .find(|(field, _)| &**field == "roundsWon")
        .map(|(_, value)| value)
    else {
        return won;
    };
    for (key, value) in rows {
        let team = match key {
            ArrayKey::String(text) if &**text == "axis" => entity_iw4::TEAM_AXIS,
            ArrayKey::String(text) if &**text == "allies" => entity_iw4::TEAM_ALLIES,
            _ => continue,
        };
        if let Detached::Value(Value::Int(n)) = value {
            won[team as usize] = i32::try_from(*n).unwrap_or(0);
        }
    }
    won
}

/// `game["switchedsides"]` at the last round restart, to see the sides switch (it outlives the
/// script runtime, which every restart replaces).
#[derive(bevy_ecs::prelude::Resource, Default)]
struct CsSwitchedSides(bool);

/// GSC's `game` global (object 1; `level` is 0, `anim` 2), whose fields `map_restart(true)`
/// keeps: rounds won and played, which side attacks, the match state.
const GAME_OBJECT: u64 = 1;

fn detach(runtime: &Runtime, value: &Value, depth: usize) -> Option<Detached> {
    match value {
        Value::Undefined
        | Value::Int(_)
        | Value::Float(_)
        | Value::String(_)
        | Value::LocalizedString(_)
        | Value::Vector(_) => Some(Detached::Value(value.clone())),
        Value::Array(id) if depth < MAX_DEPTH => Some(Detached::Array(
            runtime
                .arrays
                .get(id)?
                .iter()
                .filter_map(|(key, value)| Some((key.clone(), detach(runtime, value, depth + 1)?)))
                .collect(),
        )),
        _ => None,
    }
}

pub(crate) fn attach(runtime: &mut Runtime, value: Detached) -> Result<Value, String> {
    match value {
        Detached::Value(value) => Ok(value),
        Detached::Array(rows) => {
            let mut array = BTreeMap::new();
            for (key, value) in rows {
                array.insert(key, attach(runtime, value)?);
            }
            let id = runtime.next_object;
            runtime.next_object = id.checked_add(1).ok_or("object identifier exhausted")?;
            runtime.arrays.insert(id, array);
            Ok(Value::Array(id))
        }
    }
}

fn symbol_name(runtime: &Runtime, id: u32) -> Option<Arc<str>> {
    let program = runtime.program.as_ref()?;
    program.symbols.get(id as usize).cloned().or_else(|| {
        runtime
            .dynamic_symbols
            .iter()
            .find(|(_, symbol)| **symbol == id)
            .map(|(name, _)| name.clone())
    })
}

/// Field `name` of script object `object` (`0` is `level`). Field names are case-insensitive;
/// the compiler keeps them lowercase.
pub(crate) fn field(runtime: &Runtime, object: u64, name: &str) -> Option<Value> {
    let program = runtime.program.as_ref()?;
    let name = name.to_ascii_lowercase();
    let id = program
        .symbol_ids
        .get(name.as_str())
        .or_else(|| runtime.dynamic_symbols.get(name.as_str()))?;
    runtime.objects.get(&object)?.get(id).cloned()
}

pub(crate) fn restart_level(world: &mut World, tick: crate::Tick) {
    let runtime = world.resource::<Runtime>();
    let Some(persist) = runtime.pending_restart else {
        return;
    };
    if runtime.fault.is_some() {
        return;
    }
    let (Some(program), Some(plan)) = (runtime.program.clone(), runtime.restart.clone()) else {
        return;
    };
    let game: Vec<(Arc<str>, Detached)> = runtime
        .objects
        .get(&GAME_OBJECT)
        .into_iter()
        .flatten()
        .filter_map(|(id, value)| Some((symbol_name(runtime, *id)?, detach(runtime, value, 0)?)))
        .collect();
    let pers: BTreeMap<u32, Detached> = runtime
        .players
        .iter()
        .filter(|_| persist)
        .filter_map(|(client, slot)| {
            let pers = field(runtime, slot.object, "pers")?;
            Some((*client, detach(runtime, &pers, 0)?))
        })
        .collect();
    // How the bomb mode's round ended, for the CS round money: the `level` flags its script set.
    let level_flag = |name: &str| matches!(field(runtime, 0, name), Some(Value::Int(n)) if n != 0);
    let round_end = crate::cs_economy::RoundEnd {
        won: round_wins(&game),
        attackers: match game_string(&game, "attackers") {
            Some("axis") => entity_iw4::TEAM_AXIS,
            Some("allies") => entity_iw4::TEAM_ALLIES,
            _ => 0,
        },
        bomb_planted: level_flag("bombPlanted"),
        bomb_exploded: level_flag("bombExploded"),
        bomb_defused: level_flag("bombDefused"),
    };
    let dvars = runtime.dvars.clone();
    let local_presentation_dvars = runtime.local_presentation_dvars;
    let local_presentation_client = runtime.local_presentation_client;
    let pending_local_dvars = runtime.pending_local_dvars.clone();
    let weapon_bridge = runtime.weapon_bridge.clone();
    let personal_classes = runtime.personal_classes.clone();
    let next_presence = runtime.next_spawned_presence;
    let huds: Vec<u64> = runtime.hud_slots.keys().copied().collect();
    let clients: Vec<u32> = runtime.players.keys().copied().collect();
    let spawned: Vec<crate::ScriptModelId> = runtime
        .entities
        .values()
        .filter(|e| matches!(e.kind, EntityKind::Spawned | EntityKind::Vehicle))
        .filter_map(|e| e.presence)
        .collect();

    // CS: survivors carry their loadout into the next round; nobody does once the sides switch
    // (`game["switchedsides"]` flipped by the round switch) or at a halftime.
    let halftime = game.iter().any(|(name, value)| {
        &**name == "status"
            && matches!(value, Detached::Value(Value::String(status)) if &**status == "halftime")
    });
    let switched = game.iter().any(|(name, value)| {
        &**name == "switchedsides" && matches!(value, Detached::Value(Value::Int(n)) if *n != 0)
    });
    let sides_changed = {
        let mut last = world.get_resource_or_insert_with(CsSwitchedSides::default);
        std::mem::replace(&mut last.0, switched) != switched
    };
    {
        let mut frame = FrameWorld::from_world(world);
        crate::script_player::capture_cs_carry(&mut frame, halftime || sides_changed);
        if round_end.attackers != 0 {
            crate::cs_economy::settle_round(&mut frame, round_end, halftime || sides_changed);
        }
    }
    for id in huds {
        super::hud::destroy(world, id);
    }
    for client in clients {
        super::players::unlink_player(world, client);
    }
    {
        let mut frame = FrameWorld::from_world(world);
        for mover in crate::frame::collect_script_movers(frame.ecs()) {
            if spawned.contains(&mover.id) {
                frame.remove_script_mover_by_number(mover.state.number);
                frame.remove_collision_owner(mover.id);
            }
        }
        for projectile in crate::frame::collect_projectiles(frame.ecs()) {
            frame.remove_projectile_by_number(projectile.entnum);
        }
        for number in frame.dropped_item_numbers_sorted() {
            frame.remove_dropped_item_by_number(number);
        }
        crate::t5_destructible::restart(&mut frame);
        frame.restart_level_phase();
        for id in frame.client_ids_sorted() {
            let (origin, angles) = frame
                .player(id)
                .map_or(([0.0; 3], [0.0; 3]), |ps| (ps.origin, ps.viewangles));
            crate::script_player::spawn(&mut frame, tick, id, origin, angles, "spectator");
        }
    }

    reset(world);
    {
        let mut runtime = world.resource_mut::<Runtime>();
        runtime.dvars = dvars;
        runtime.local_presentation_dvars = local_presentation_dvars;
        runtime.local_presentation_client = local_presentation_client;
        runtime.pending_local_dvars = pending_local_dvars;
    }
    // The entries stay in the plan the next restart reads: they are recorded only at the first
    // start, so taking them out here left every later restart without the level's `main` (the
    // script runtime stopped after the second round restart and the match stalled).
    let plan = (*plan).clone();
    let entries = plan.entries.clone();
    if let Err(fault) = install_level(world, program, plan) {
        world.resource_mut::<Runtime>().fault = Some(fault);
        return;
    }
    let mut runtime = world.resource_mut::<Runtime>();
    runtime.last_tick = Some(tick);
    runtime.weapon_bridge = weapon_bridge;
    runtime.personal_classes = personal_classes;
    runtime.next_spawned_presence = next_presence;
    runtime.restored_pers = pers;
    for (name, value) in game {
        match attach(&mut runtime, value) {
            Ok(value) => runtime.set_object_field(GAME_OBJECT, &name, value),
            Err(message) => {
                runtime.fault = Some(Fault::at(
                    &Location {
                        module: "<engine>".into(),
                        function: "map_restart".into(),
                        line: 0,
                        column: 0,
                    },
                    message,
                ));
                return;
            }
        }
    }
    drop(runtime);
    for entry in entries {
        if let Err(fault) = run_now(
            world,
            &entry,
            Value::level(),
            Vec::new(),
            i64::from(crate::level_time_ms(tick)),
        ) {
            world.resource_mut::<Runtime>().fault = Some(fault);
            return;
        }
    }
}
