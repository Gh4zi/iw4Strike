//! CS:GO's end-of-match map vote (`mp_endmatch_votenextmap`): as the bomb mode's match ends the
//! server offers the maps the session picked (`cs_mapvote_maps`: the one just played and a few
//! others), each player votes for one (`ClientAction::MapVote`), and the vote closes when a map
//! has a majority (`votes_to_succeed`) or after [`VOTE_SECONDS`]; the most voted map (a tie
//! drawn among the tied) is the next one. Clients read it as the server info `cs_mapvote` =
//! "<maps>|<votes each>|<votes to win>|<the winner's place, or ->".

use std::collections::BTreeMap;

use bevy_ecs::prelude::Resource;

use crate::frame::FrameWorld;
use crate::identities::MatchPhase;
use crate::world::{ClientId, Tick};

/// How long the vote stays open.
const VOTE_SECONDS: i32 = 20;

#[derive(Resource, Default)]
pub(crate) struct MapVote {
    maps: Vec<String>,
    votes: BTreeMap<ClientId, u8>,
    opened_ms: i32,
    winner: Option<u8>,
}

impl MapVote {
    fn counts(&self) -> Vec<u32> {
        let mut counts = vec![0; self.maps.len()];
        for &index in self.votes.values() {
            if let Some(count) = counts.get_mut(usize::from(index)) {
                *count += 1;
            }
        }
        counts
    }
}

/// Opens the vote as the match ends, closes it on a majority or when time is up, and keeps the
/// clients' `cs_mapvote` current.
pub(crate) fn update(world: &mut FrameWorld, tick: Tick) {
    // The match is over once its script sends everyone to the intermission (`level.intermission`),
    // some 20 s before the level ends.
    let intermission = world
        .ecs()
        .get_resource::<crate::script::Runtime>()
        .and_then(|runtime| crate::script::host::restart::field(runtime, 0, "intermission"))
        .is_some_and(|value| matches!(value, crate::script::Value::Int(n) if n != 0));
    let over = intermission || matches!(world.phase(), MatchPhase::PostGame | MatchPhase::Intermission);
    if !over || !crate::cs_economy::economy(world) {
        return;
    }
    let now = crate::level_time_ms(tick);
    let mut vote = match world.ecs().remove_resource::<MapVote>() {
        Some(vote) => vote,
        None => {
            let maps: Vec<String> = world
                .ecs()
                .get_resource::<crate::script::Runtime>()
                .and_then(|runtime| runtime.dvars.get("cs_mapvote_maps").cloned())
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_owned)
                .collect();
            if maps.len() < 2 {
                return;
            }
            diag::info!(Sim, "cs map vote: open for {}", maps.join(", "));
            MapVote {
                maps,
                opened_ms: now,
                ..MapVote::default()
            }
        }
    };
    let needed = world.client_ids_sorted().len() / 2 + 1;
    if vote.winner.is_none() {
        let counts = vote.counts();
        let majority = counts.iter().position(|&count| count as usize >= needed);
        if majority.is_some() || now - vote.opened_ms >= VOTE_SECONDS * 1000 {
            let top = counts.iter().copied().max().unwrap_or(0);
            let tied: Vec<usize> = (0..counts.len()).filter(|&i| counts[i] == top).collect();
            let pick = majority.unwrap_or_else(|| tied[now.unsigned_abs() as usize % tied.len()]);
            vote.winner = u8::try_from(pick).ok();
            diag::info!(Sim, "cs map vote: {} next ({counts:?})", vote.maps[pick]);
        }
    }
    let counts: Vec<String> = vote.counts().iter().map(u32::to_string).collect();
    let value = format!(
        "{}|{}|{needed}|{}",
        vote.maps.join(" "),
        counts.join(" "),
        vote.winner.map_or_else(|| "-".to_owned(), |w| w.to_string())
    );
    if let Some(mut runtime) = world.ecs().get_resource_mut::<crate::script::Runtime>()
        && (runtime.dvars.get("cs_mapvote") != Some(&value)
            || !runtime.server_info.contains("cs_mapvote"))
    {
        runtime.server_info.insert("cs_mapvote".to_owned());
        runtime.dvars.insert("cs_mapvote".to_owned(), value);
    }
    world.ecs().insert_resource(vote);
}

/// `id` votes for the map at `index` while the vote is open (a new vote replaces the old).
pub(crate) fn vote(world: &mut FrameWorld, id: ClientId, index: u8) {
    if let Some(mut vote) = world.ecs().get_resource_mut::<MapVote>()
        && vote.winner.is_none()
        && usize::from(index) < vote.maps.len()
    {
        vote.votes.insert(id, index);
    }
}

/// The map a `cs_mapvote` value picks: its winner, or while it is still open the most voted
/// (the first of the tied).
#[must_use]
pub fn mapvote_next_map(value: &str) -> Option<String> {
    let mut parts = value.split('|');
    let maps: Vec<&str> = parts.next()?.split_whitespace().collect();
    let counts: Vec<u32> = parts
        .next()?
        .split_whitespace()
        .filter_map(|c| c.parse().ok())
        .collect();
    let _needed = parts.next()?;
    let index = match parts.next()?.parse::<usize>() {
        Ok(winner) => winner,
        Err(_) => {
            let top = counts.iter().copied().max()?;
            counts.iter().position(|&c| c == top)?
        }
    };
    maps.get(index).map(|map| (*map).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_winner_or_the_leader_is_next() {
        assert_eq!(mapvote_next_map("mp_rust mp_terminal|0 2|3|1").as_deref(), Some("mp_terminal"));
        assert_eq!(mapvote_next_map("mp_rust mp_terminal mp_derail|1 0 3|3|-").as_deref(), Some("mp_derail"));
        assert_eq!(mapvote_next_map("mp_rust mp_terminal|0 0|2|-").as_deref(), Some("mp_rust"));
    }
}
