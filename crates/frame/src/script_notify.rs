use bevy::prelude::*;

#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitLevelCalled;

/// The map the end-of-match vote picked, for the session to play next when the match scripts
/// call `exitLevel` (none: back to the menu).
#[derive(Resource, Default, Clone, Debug, PartialEq, Eq)]
pub struct NextMatchZone(pub Option<String>);

pub fn register_script_notify(app: &mut App) {
    app.add_message::<ExitLevelCalled>();
    app.init_resource::<NextMatchZone>();
}
