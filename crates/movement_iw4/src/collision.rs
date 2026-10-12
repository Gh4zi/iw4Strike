use trace_iw4::Trace;

use crate::GroundTraceInput;
use crate::correct_solid::{CorrectSolidOutcome, correct_solid};

pub trait CollisionBackend {
    fn trace(&self, input: GroundTraceInput) -> Trace;

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
