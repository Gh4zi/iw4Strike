//! Counter-Strike money, by CS 1.6's rules (ReGameDLL `CHalfLifeMultiplay`): $800 to start,
//! $16000 at most, $300 per enemy killed (a team kill costs $3300), and at every round restart
//! each team's round money — the winners' reward by how the round was won, the losers' bonus,
//! which grows $500 with every round they lose in a row. When the sides switch everyone starts
//! over with $800, as CS2's halftime does.
//!
//! Money exists in the bomb mode alone, where rounds earn it. Every other mode (free-for-all,
//! team deathmatch) has no round economy: everyone holds $16000 and buys for free, like CS:GO's
//! deathmatch.

use bevy_ecs::prelude::Resource;

use crate::ClientId;
use crate::frame::FrameWorld;

/// Whether this match has CS money: the bomb mode, whose script names the attackers.
pub(crate) fn economy(world: &mut FrameWorld) -> bool {
    world
        .ecs()
        .get_resource::<crate::script::Runtime>()
        .is_some_and(|runtime| runtime.dvars.contains_key("cs_attackers"))
}

pub const START_MONEY: i32 = 800;
pub const MAX_MONEY: i32 = 16000;
pub const KILL_REWARD: i32 = 300;
pub const TEAM_KILL_PENALTY: i32 = 3300;
/// Terrorists, the target bombed (`REWARD_TARGET_BOMB`).
const BOMB_EXPLODED: i32 = 3500;
/// Terrorists, every Counter-Terrorist dead (`REWARD_BOMB_EXPLODED` on a bomb map).
const TERRORISTS_ELIMINATED: i32 = 3250;
/// Counter-Terrorists: bomb defused, every Terrorist dead or time run out.
const COUNTER_TERRORISTS_WON: i32 = 3250;
/// Terrorists whose planted bomb was defused (`REWARD_BOMB_PLANTED`), on top of the loss bonus.
const BOMB_PLANTED: i32 = 800;
const LOSER_BONUS_DEFAULT: i32 = 1400;
const LOSER_BONUS_MIN: i32 = 1500;
const LOSER_BONUS_MAX: i32 = 3000;
const LOSER_BONUS_ADD: i32 = 500;

/// A player's money ($16000 outside the bomb mode).
pub(crate) fn money(world: &mut FrameWorld, id: ClientId) -> i32 {
    if !economy(world) {
        return MAX_MONEY;
    }
    world
        .client_meta(id)
        .and_then(|meta| meta.cs_money)
        .unwrap_or(START_MONEY)
}

/// Sets a player's money, kept between $0 and $16000, and shows it on their HUD.
pub(crate) fn set_money(world: &mut FrameWorld, id: ClientId, amount: i32) {
    let amount = amount.clamp(0, MAX_MONEY);
    world.client_meta_mut(id).cs_money = Some(amount);
    show_money(world, id);
}

pub(crate) fn add_money(world: &mut FrameWorld, id: ClientId, amount: i32) {
    let now = money(world, id);
    set_money(world, id, now.saturating_add(amount));
}

/// Copies the account into the player state the HUD reads (a spawn rebuilds it).
pub(crate) fn show_money(world: &mut FrameWorld, id: ClientId) {
    let amount = money(world, id);
    if let Some(ps) = world.player_mut(id) {
        ps.cs_money = amount as u32;
    }
}

/// Takes `price` if the player can pay it (anything is free outside the bomb mode).
pub(crate) fn pay(world: &mut FrameWorld, id: ClientId, price: i32) -> bool {
    if !economy(world) {
        return true;
    }
    let now = money(world, id);
    if now < price {
        return false;
    }
    set_money(world, id, now - price);
    true
}

/// A kill's money: $300 for an enemy, minus $3300 for a teammate, nothing for yourself.
pub(crate) fn reward_kill(world: &mut FrameWorld, victim: ClientId, attacker: Option<ClientId>) {
    let Some(attacker) = attacker.filter(|attacker| *attacker != victim) else {
        return;
    };
    if !economy(world) {
        return;
    }
    let team = |world: &FrameWorld, id: ClientId| world.client_meta(id).map(|m| m.client_state_team);
    let teammate = world.game_mode_kind().is_team() && team(world, victim) == team(world, attacker);
    let amount = if teammate {
        -TEAM_KILL_PENALTY
    } else {
        KILL_REWARD
    };
    add_money(world, attacker, amount);
    diag::info!(
        Sim,
        "cs money: client {} {} client {}: {amount:+}",
        attacker.0,
        if teammate { "team-killed" } else { "killed" },
        victim.0
    );
}

/// Seconds of buying once the freeze is over (`scr_cs_buytime`; CS2's `mp_buytime 20`).
const BUY_TIME_DEFAULT_SECONDS: f32 = 20.0;
/// A buy zone: this close to any of the player's side's spawn points (CS 1.6 on a map without
/// `func_buyzone`, ReGameDLL `CBasePlayer::HandleSignals`).
const BUY_ZONE_RADIUS: f32 = 200.0;

/// The bomb mode's attacking (Terrorist) MW2 team, from its script.
pub(crate) fn attackers(world: &mut FrameWorld) -> Option<i32> {
    let runtime = world.ecs().get_resource::<crate::script::Runtime>()?;
    match runtime.dvars.get("cs_attackers").map(String::as_str) {
        Some("axis") => Some(entity_iw4::TEAM_AXIS),
        Some("allies") => Some(entity_iw4::TEAM_ALLIES),
        _ => None,
    }
}

/// Whether the round still sells: all through the freeze (the script's `level.startTime` is set
/// when it ends) and for the buy time after it.
fn buy_time_open(world: &mut FrameWorld, tick: crate::Tick) -> bool {
    let Some(runtime) = world.ecs().get_resource::<crate::script::Runtime>() else {
        return true;
    };
    let Some(crate::script::Value::Int(start)) =
        crate::script::host::restart::field(runtime, 0, "startTime")
    else {
        return true;
    };
    let seconds = runtime
        .dvars
        .get("scr_cs_buytime")
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(BUY_TIME_DEFAULT_SECONDS);
    let now = i64::from(tick.0) * i64::from(crate::MATCH_TICK_MS);
    now - i64::from(start) < (seconds * 1000.0) as i64
}

/// Where and when `id` may buy now (`playerstate_iw4::cs_buy` bits). Outside the bomb mode
/// everything sells everywhere.
pub(crate) fn buy_bits(world: &mut FrameWorld, tick: crate::Tick, id: ClientId) -> u32 {
    use playerstate_iw4::cs_buy::{TIME, ZONE};
    if !economy(world) {
        return ZONE | TIME;
    }
    let time = if buy_time_open(world, tick) { TIME } else { 0 };
    let attackers = attackers(world).unwrap_or(entity_iw4::TEAM_AXIS);
    let Some(team) = world.client_meta(id).map(|meta| meta.client_state_team) else {
        return time;
    };
    let class = if team == attackers {
        "mp_sd_spawn_attacker"
    } else if team == entity_iw4::TEAM_AXIS || team == entity_iw4::TEAM_ALLIES {
        "mp_sd_spawn_defender"
    } else {
        return time;
    };
    let Some(origin) = world.player(id).map(|ps| ps.origin) else {
        return time;
    };
    let near = world
        .bootstrap_ref()
        .spawns
        .iter()
        .filter(|spawn| spawn.classname.eq_ignore_ascii_case(class))
        .any(|spawn| {
            let d = [0, 1, 2].map(|i| spawn.origin[i] - origin[i]);
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < BUY_ZONE_RADIUS * BUY_ZONE_RADIUS
        });
    time | if near { ZONE } else { 0 }
}

/// Refreshes every player's buy zone and buy time bits, for their HUD and buy menu.
pub(crate) fn update_buy_bits(world: &mut FrameWorld, tick: crate::Tick) {
    for id in world.client_ids_sorted() {
        let bits = buy_bits(world, tick, id);
        if let Some(ps) = world.player_mut(id)
            && ps.cs_buy != bits
        {
            ps.cs_buy = bits;
        }
    }
}

/// Why `id` can't buy right now, if they can't: out of buy time or out of a buy zone (CS
/// checks the time first).
pub(crate) fn buy_refusal(world: &mut FrameWorld, tick: crate::Tick, id: ClientId) -> Option<&'static str> {
    use playerstate_iw4::cs_buy::{TIME, ZONE};
    let bits = buy_bits(world, tick, id);
    if bits & TIME == 0 {
        Some("the buy time is over")
    } else if bits & ZONE == 0 {
        Some("not in a buy zone")
    } else {
        None
    }
}

/// Whether `id`'s side may buy `name` (a buy name): team weapons are kept to their side in the
/// bomb mode, where the sides are Terrorists and Counter-Terrorists.
pub(crate) fn side_may_buy(world: &mut FrameWorld, id: ClientId, name: &str) -> bool {
    use weapon_iw4::cs::BuyTeam;
    let team = weapon_iw4::cs::buy_team(name);
    if team == BuyTeam::Both || !economy(world) {
        return true;
    }
    let attackers = attackers(world).unwrap_or(entity_iw4::TEAM_AXIS);
    let terrorist = world
        .client_meta(id)
        .is_some_and(|meta| meta.client_state_team == attackers);
    (team == BuyTeam::Terrorists) == terrorist
}

/// How the bomb mode's round ended, as its script left it at the restart.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RoundEnd {
    /// Rounds won so far, by MW2 team (`[_, axis, allies]`).
    pub won: [i32; 3],
    /// The attacking (Terrorist) MW2 team.
    pub attackers: i32,
    pub bomb_planted: bool,
    pub bomb_exploded: bool,
    pub bomb_defused: bool,
}

/// What the round money remembers from round to round.
#[derive(Resource, Default)]
pub(crate) struct RoundLedger {
    won: [i32; 3],
    /// Rounds lost in a row: Terrorists, Counter-Terrorists.
    losses: [u32; 2],
    loser_bonus: i32,
}

/// Pays the round that just ended (CS 1.6 `RestartRound`), or after a side switch (`reset`)
/// starts everyone over with $800.
pub(crate) fn settle_round(world: &mut FrameWorld, end: RoundEnd, reset: bool) {
    let mut ledger = world
        .ecs()
        .remove_resource::<RoundLedger>()
        .unwrap_or(RoundLedger {
            loser_bonus: LOSER_BONUS_DEFAULT,
            ..RoundLedger::default()
        });
    let previous = std::mem::replace(&mut ledger.won, end.won);
    let clients = world.client_ids_sorted();
    if reset {
        ledger.losses = [0; 2];
        ledger.loser_bonus = LOSER_BONUS_DEFAULT;
        for id in clients {
            set_money(world, id, START_MONEY);
        }
        world.ecs().insert_resource(ledger);
        return;
    }
    let winner = [entity_iw4::TEAM_AXIS, entity_iw4::TEAM_ALLIES]
        .into_iter()
        .find(|&team| end.won[team as usize] > previous[team as usize]);
    let Some(winner) = winner else {
        world.ecs().insert_resource(ledger);
        return;
    };
    let terrorists_won = winner == end.attackers;
    // Loss streaks, then the bonus they earn (ReGameDLL order).
    let (winners, losers) = if terrorists_won { (0, 1) } else { (1, 0) };
    if ledger.losses[winners] > 1 {
        ledger.loser_bonus = LOSER_BONUS_MIN;
    }
    ledger.losses[winners] = 0;
    ledger.losses[losers] += 1;
    if ledger.losses[losers] > 1 && ledger.loser_bonus < LOSER_BONUS_MAX {
        ledger.loser_bonus += LOSER_BONUS_ADD;
    }
    let (terrorist_money, counter_terrorist_money) = if terrorists_won {
        let reward = if end.bomb_exploded {
            BOMB_EXPLODED
        } else {
            TERRORISTS_ELIMINATED
        };
        (reward, ledger.loser_bonus)
    } else {
        let planted = if end.bomb_defused && end.bomb_planted {
            BOMB_PLANTED
        } else {
            0
        };
        (ledger.loser_bonus + planted, COUNTER_TERRORISTS_WON)
    };
    for id in clients {
        let Some(team) = world.client_meta(id).map(|m| m.client_state_team) else {
            continue;
        };
        if team == end.attackers {
            add_money(world, id, terrorist_money);
        } else if team == entity_iw4::TEAM_AXIS || team == entity_iw4::TEAM_ALLIES {
            add_money(world, id, counter_terrorist_money);
        }
    }
    diag::info!(
        Sim,
        "cs money: {} won the round (bomb planted {} exploded {} defused {}); Terrorists +${} \
         Counter-Terrorists +${}",
        if terrorists_won {
            "Terrorists"
        } else {
            "Counter-Terrorists"
        },
        end.bomb_planted,
        end.bomb_exploded,
        end.bomb_defused,
        terrorist_money,
        counter_terrorist_money
    );
    world.ecs().insert_resource(ledger);
}
