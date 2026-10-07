use bevy::prelude::*;

use crate::{ConsoleCommand, ConsoleLine, ConsoleRegistry, ConsoleSettings, ConsoleState};

pub(crate) fn register_movement_commands(registry: &mut ConsoleRegistry) {
    if registry.resolve("mv_stamina").is_none() {
        registry.register(
            crate::CommandSpec::new("mv_stamina").usage(
                "mv_stamina [scale] — jump stamina strength: 1 = profile, 0.5 = half, 0 = off",
            ),
        );
    }
    if registry.resolve("cl_dynamiccrosshair").is_none() {
        registry.register(
            crate::CommandSpec::new("cl_dynamiccrosshair")
                .usage("cl_dynamiccrosshair [0|1] — CS crosshair follows movement and shots"),
        );
    }
    if registry.resolve("mv_mode").is_none() {
        registry.register(crate::CommandSpec::new("mv_mode").usage(
            "mv_mode [csgo|surf|mmod|cs16] — movement preset: CS:GO, surf, Momentum bhop, CS 1.6",
        ));
    }
    if registry.resolve("sv_destructibles").is_none() {
        registry.register(crate::CommandSpec::new("sv_destructibles").usage(
            "sv_destructibles [0|1] — cars, barrels and breakable walls take damage (default 0)",
        ));
    }
    if registry.resolve("snd_ambient_volume").is_none() {
        registry.register(crate::CommandSpec::new("snd_ambient_volume").usage(
            "snd_ambient_volume [0-1] — how loud the map's own ambience plays (wind, engines, hum)",
        ));
    }
    if registry.resolve("viewmodel_fov").is_none() {
        registry.register(crate::CommandSpec::new("viewmodel_fov").usage(
            "viewmodel_fov [54-90] — how wide the CS gun is drawn (bigger = gun further away)",
        ));
    }
    if registry.resolve("cl_wpn_sway").is_none() {
        registry.register(
            crate::CommandSpec::new("cl_wpn_sway")
                .usage("cl_wpn_sway [0|1] — CS gun bobs when walking and trails when turning"),
        );
    }
}

/// Keeps the movement preset on what the settings say (loaded from settings.cfg or set by
/// `mv_mode`).
pub(crate) fn sync_movement_mode(game: Res<frame::GameSettings>) {
    if !game.is_changed() {
        return;
    }
    if sim::cs_settings::destructibles_enabled() != game.destructibles {
        sim::cs_settings::set_destructibles(game.destructibles);
        diag::info!(Console, "sv_destructibles = {}", u8::from(game.destructibles));
    }
    if let Some(wanted) = movement_iw4::rules::MovementMode::from_name(&game.mv_mode)
        && wanted != movement_iw4::rules::mode()
    {
        movement_iw4::rules::set_mode(wanted);
        diag::info!(Console, "mv_mode = {} — {}", wanted.name(), wanted.describe());
    }
}

pub(crate) fn route_movement_commands(
    mut events: MessageReader<ConsoleCommand>,
    mut console: ResMut<ConsoleState>,
    settings: Res<ConsoleSettings>,
    mut line: ResMut<ConsoleLine>,
    mut game: ResMut<frame::GameSettings>,
) {
    let capacity = settings.log_capacity;
    for cmd in events.read() {
        let message = match cmd.name.as_str() {
            "mv_stamina" => match cmd.args.as_slice() {
                [] => format!("mv_stamina = {}", movement_iw4::source::stamina_scale()),
                [arg] => match arg.parse::<f32>() {
                    Ok(scale) => {
                        movement_iw4::source::set_stamina_scale(scale);
                        format!("mv_stamina = {}", movement_iw4::source::stamina_scale())
                    }
                    Err(_) => "usage: mv_stamina [scale]".to_owned(),
                },
                _ => "usage: mv_stamina [scale]".to_owned(),
            },
            "cl_dynamiccrosshair" => match cmd.args.as_slice() {
                [] => format!(
                    "cl_dynamiccrosshair = {}",
                    u8::from(hud::dynamic_crosshair())
                ),
                [arg] if arg == "0" || arg == "1" => {
                    hud::set_dynamic_crosshair(arg == "1");
                    format!("cl_dynamiccrosshair = {arg}")
                }
                _ => "usage: cl_dynamiccrosshair [0|1]".to_owned(),
            },
            "mv_mode" => {
                use movement_iw4::rules::{MovementMode, mode, set_mode};
                match cmd.args.as_slice() {
                    [] => format!("mv_mode = {} — {}", mode().name(), mode().describe()),
                    [arg] => match MovementMode::from_name(arg) {
                        Some(next) => {
                            set_mode(next);
                            next.name().clone_into(&mut game.mv_mode);
                            game.touch();
                            format!("mv_mode = {} — {}", next.name(), next.describe())
                        }
                        None => "usage: mv_mode [csgo|surf|mmod|cs16]".to_owned(),
                    },
                    _ => "usage: mv_mode [csgo|surf|mmod|cs16]".to_owned(),
                }
            }
            "sv_destructibles" => match cmd.args.as_slice() {
                [] => format!(
                    "sv_destructibles = {}",
                    u8::from(sim::cs_settings::destructibles_enabled())
                ),
                [arg] if arg == "0" || arg == "1" => {
                    sim::cs_settings::set_destructibles(arg == "1");
                    game.destructibles = arg == "1";
                    game.touch();
                    format!("sv_destructibles = {arg}")
                }
                _ => "usage: sv_destructibles [0|1]".to_owned(),
            },
            "snd_ambient_volume" => match cmd.args.as_slice() {
                [] => format!("snd_ambient_volume = {:.2}", game.ambient_volume),
                [arg] => match arg.parse::<f32>() {
                    Ok(volume) if volume.is_finite() => {
                        game.ambient_volume = volume.clamp(0.0, 1.0);
                        game.touch();
                        format!("snd_ambient_volume = {:.2}", game.ambient_volume)
                    }
                    _ => "usage: snd_ambient_volume [0-1]".to_owned(),
                },
                _ => "usage: snd_ambient_volume [0-1]".to_owned(),
            },
            "viewmodel_fov" => match cmd.args.as_slice() {
                [] => format!("viewmodel_fov = {:.0}", game.viewmodel_fov),
                [arg] => match arg.parse::<f32>() {
                    Ok(fov) if fov.is_finite() => {
                        game.viewmodel_fov = fov.clamp(
                            frame::GameSettings::VIEWMODEL_FOV_MIN,
                            frame::GameSettings::VIEWMODEL_FOV_MAX,
                        );
                        game.touch();
                        format!("viewmodel_fov = {:.0}", game.viewmodel_fov)
                    }
                    _ => "usage: viewmodel_fov [54-90]".to_owned(),
                },
                _ => "usage: viewmodel_fov [54-90]".to_owned(),
            },
            "cl_wpn_sway" => {
                use render_anim::occupancy::cs_viewmodel::{set_viewmodel_sway, viewmodel_sway};
                match cmd.args.as_slice() {
                    [] => format!("cl_wpn_sway = {}", u8::from(viewmodel_sway())),
                    [arg] if arg == "0" || arg == "1" => {
                        set_viewmodel_sway(arg == "1");
                        format!("cl_wpn_sway = {arg}")
                    }
                    _ => "usage: cl_wpn_sway [0|1]".to_owned(),
                }
            }
            _ => continue,
        };
        diag::info!(Console, "{message}");
        line.0 = message.clone();
        console.echo(message, capacity);
    }
}
