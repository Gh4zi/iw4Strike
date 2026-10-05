use crate::frame::FrameWorld;
use crate::identities::MatchPhase;
use gamemode_iw4::ffa::{SCORE_LIMIT, TIME_LIMIT_MS};

/// Authority tick length: 100Hz, the CS 1.6 server rate. Must divide `SCRIPT_FRAME_MS`.
pub const MATCH_TICK_MS: u32 = 10;

/// One GSC `wait` frame. Scripts count 20 frames per second whatever the authority tick is.
pub const SCRIPT_FRAME_MS: u32 = 50;

const _: () = assert!(SCRIPT_FRAME_MS.is_multiple_of(MATCH_TICK_MS));

/// Authority ticks spanning `ms` of game time, at least one. Tick-count tunables that were
/// authored against the retail 20Hz tick are written as durations through this.
pub const fn ticks_for_ms(ms: u32) -> u32 {
    let ticks = ms / MATCH_TICK_MS;
    if ticks == 0 { 1 } else { ticks }
}

pub(crate) fn finish_prematch(world: &mut FrameWorld) {
    world.set_phase(MatchPhase::Playing);
    if world.bootstrap_ref().kind == gamemode_iw4::GameModeKind::Demolition {
        world.set_use_start_spawns(false);
    }
}

pub fn bootstrap_score_defaults() -> (i32, u32) {
    (SCORE_LIMIT, TIME_LIMIT_MS)
}
