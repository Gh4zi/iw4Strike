use trace_iw4::Trace;

use crate::GroundTraceInput;
use crate::correct_solid::{CorrectSolidOutcome, correct_solid};

/// A player someone stands on, as Source movement sees them: their velocity is given or taken at
/// the instant of leaving or landing on them (a runboost), and CS:GO jumps and stacks follow
/// what they stand on.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerGround {
    pub velocity: [f32; 3],
    /// That player is in the air.
    pub airborne: bool,
    /// That player stands on another player.
    pub on_player: bool,
}

pub trait CollisionBackend {
    fn trace(&self, input: GroundTraceInput) -> Trace;

    /// The player with entity number `entity`, if that entity is one.
    fn player_ground(&self, entity: u16) -> Option<PlayerGround> {
        let _ = entity;
        None
    }

    /// `trace` with axis-aligned box brushes deciding hits as CS:GO does (exact touch times, no
    /// epsilon); the world's other geometry as `trace`. CS:GO movement traces this way, which is
    /// what makes its edgebugs land where CS:GO's do.
    fn trace_exact_boxes(&self, input: GroundTraceInput) -> Trace {
        self.trace(input)
    }

    fn correct_solid(
        &self,
        origin: [f32; 3],
        mins: [f32; 3],
        maxs: [f32; 3],
        tracemask: u32,
    ) -> Option<CorrectSolidOutcome> {
        correct_solid(origin, mins, maxs, tracemask, self)
    }

    fn touch_entity(&self, _entity: i32) {}
}
