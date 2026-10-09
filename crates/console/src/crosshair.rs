//! The CS crosshair's console variables (CS:GO's `cl_crosshair*`) and `crosshair_code`, which
//! takes a CS:GO or CS2 crosshair share code.

use bevy::prelude::*;
use frame::crosshair::{CVARS, Crosshair, decode_share_code};

use crate::{ConsoleCommand, ConsoleLine, ConsoleRegistry, ConsoleSettings, ConsoleState};

const CODE_USAGE: &str = "crosshair_code <CSGO-xxxxx-xxxxx-xxxxx-xxxxx-xxxxx or CS2 code>";

pub(crate) fn register_crosshair_commands(registry: &mut ConsoleRegistry) {
    for (name, about) in CVARS {
        if registry.resolve(name).is_none() {
            registry
                .register(crate::CommandSpec::new(name).usage(format!("{name} [value] — {about}")));
        }
    }
    if registry.resolve("crosshair_code").is_none() {
        registry.register(crate::CommandSpec::new("crosshair_code").usage(format!(
            "{CODE_USAGE} — import a CS:GO or CS2 crosshair from its share code; no code prints the crosshair"
        )));
    }
}

pub(crate) fn route_crosshair_commands(
    mut events: MessageReader<ConsoleCommand>,
    mut console: ResMut<ConsoleState>,
    settings: Res<ConsoleSettings>,
    mut line: ResMut<ConsoleLine>,
    mut game: ResMut<frame::GameSettings>,
) {
    let capacity = settings.log_capacity;
    for cmd in events.read() {
        let message = if cmd.name == "crosshair_code" {
            match cmd.args.as_slice() {
                [] => describe(&game.crosshair),
                // A pasted code split by spaces is still one code.
                args => match decode_share_code(&args.concat()) {
                    Ok(crosshair) => {
                        game.crosshair = crosshair;
                        game.crosshair.sanitize();
                        game.touch();
                        describe(&game.crosshair)
                    }
                    Err(error) => format!("crosshair_code: {error}"),
                },
            }
        } else if let Some((name, about)) = CVARS.iter().find(|(name, _)| *name == cmd.name) {
            match cmd.args.as_slice() {
                [] => show(&game.crosshair, name),
                [value] => match game.crosshair.set_cvar(name, value) {
                    Ok(()) => {
                        game.touch();
                        show(&game.crosshair, name)
                    }
                    Err(error) => error,
                },
                _ => format!("usage: {name} [value] — {about}"),
            }
        } else {
            continue;
        };
        diag::info!(Console, "{message}");
        line.0 = message.clone();
        console.echo(message, capacity);
    }
}

/// `name = value`, as the console prints a variable.
fn show(crosshair: &Crosshair, name: &str) -> String {
    match crosshair.cvar(name) {
        Some(value) => format!("{name} = {value}"),
        None => format!("{name} is not a crosshair setting"),
    }
}

/// The crosshair as its `cl_crosshair*` variables, one line.
fn describe(crosshair: &Crosshair) -> String {
    CVARS
        .iter()
        .filter_map(|(name, _)| Some(format!("{name} {}", crosshair.cvar(name)?)))
        .collect::<Vec<_>>()
        .join("; ")
}
