//! What a server's admin layer reaches into the match through: chat lines that are commands
//! (`!kick`, `/ban`, `@admins…`), players to slay, and lines to print to one player or all.

use bevy_ecs::prelude::*;

use crate::ClientId;

#[derive(Resource, Debug, Default)]
pub(crate) struct AdminHooks {
    /// Chat lines starting `!`, `/` or `@` are commands: kept for the admin layer. A `!` line is
    /// said as well (everyone sees the command, as SourceMod's public triggers); `/` and `@` are
    /// not (silent triggers, admin chat).
    pub(crate) chat_commands: bool,
    pub(crate) lines: Vec<(ClientId, String)>,
    /// Players an admin slays, at the next step.
    pub(crate) slays: Vec<ClientId>,
    /// Players whose chat is not said (their command lines still are commands).
    pub(crate) gagged: std::collections::HashSet<ClientId>,
    /// Every line said, for the server's chat log.
    pub(crate) said: Vec<(ClientId, String)>,
}

impl AdminHooks {
    /// Takes a chat line `id` sent: keeps a command line, and whether the line is said.
    pub(crate) fn take_line(&mut self, id: ClientId, text: &str) -> bool {
        let command = chat_command(text);
        if command.is_some() {
            self.lines.push((id, text.trim().to_owned()));
        }
        let said = command.unwrap_or(true) && !self.gagged.contains(&id);
        if said {
            self.said.push((id, text.trim().to_owned()));
        }
        said
    }
}

/// A player in the match as a server's admins list them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RosterRow {
    pub id: ClientId,
    pub name: String,
    /// 0 none, 1 axis, 2 allies, 3 spectator.
    pub team: i32,
    pub alive: bool,
    pub kills: i32,
    pub deaths: i32,
    pub score: i32,
}

/// Whether a chat line is a command the admin layer takes, and whether it is also said.
pub(crate) fn chat_command(text: &str) -> Option<bool> {
    match text.trim_start().chars().next()? {
        '!' => Some(true),
        '/' | '@' => Some(false),
        _ => None,
    }
}
