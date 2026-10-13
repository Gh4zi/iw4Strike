//! CS:GO's player bodies without the GPU: the animation state that poses them
//! ([`AnimState`]), the agents' skeletons and hit boxes ([`Agent`], [`hitbox`]), and the CS:GO
//! pack they come from, read once per process ([`pack`]). The client draws the bodies with
//! them; the server poses the same bodies to trace shots against their hit boxes.

pub mod agent;
pub mod hitbox;
mod state;

pub use agent::{Agent, Pack, pack, placement};
pub use state::{AnimState, Animset, Event, Input, Layer, Observation, agent_model};
