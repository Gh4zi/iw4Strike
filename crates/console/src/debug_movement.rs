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
            "mv_mode [csgo|csgo64|csgo128|css|surf|mmod|cs16] — movement preset: CS:GO (100, 64 or 128 tick), CS:S style, surf, Momentum bhop, CS 1.6",
        ));
    }
    if registry.resolve("shooting_mode").is_none() {
        registry.register(crate::CommandSpec::new("shooting_mode").usage(
            "shooting_mode [csgo|cs16] — how CS guns shoot on the server you host: CS:GO's spray, inaccuracy and recoil (default) or CS 1.6's",
        ));
    }
    if registry.resolve("smoke_quality").is_none() {
        registry.register(crate::CommandSpec::new("smoke_quality").usage(
            "smoke_quality [high|medium|low] — how finely CS2 smoke is drawn: high every pixel (sharpest), medium half resolution (about a quarter of the cost), low half resolution with coarser steps",
        ));
    }
    if registry.resolve("smoke_mode").is_none() {
        registry.register(crate::CommandSpec::new("smoke_mode").usage(
            "smoke_mode [cs2|csgo] — how smoke grenades look on the server you host: CS2's volumetric smoke that fills the room (default) or the particle smoke",
        ));
    }
    if registry.resolve("sv_destructibles").is_none() {
        registry.register(crate::CommandSpec::new("sv_destructibles").usage(
            "sv_destructibles [0|1] — cars, barrels and breakable walls take damage (default 0)",
        ));
    }
    if registry.resolve("_vgui_menus").is_none() {
        registry.register(crate::CommandSpec::new("_vgui_menus").usage(
            "_vgui_menus [0|1] — buy menu: 1 the window you click (CS:S / CS 1.6 VGUI), 0 CS 1.6's old numbered text menu",
        ));
    }
    if registry.resolve("cl_roundbanner").is_none() {
        registry.register(crate::CommandSpec::new("cl_roundbanner").usage(
            "cl_roundbanner [css|cs16|mw2] — round-end banner: CS:S win panel, CS 1.6 centre message or MW2's round outcome",
        ));
    }
    if registry.resolve("snd_timer_warning_volume").is_none() {
        registry.register(crate::CommandSpec::new("snd_timer_warning_volume").usage(
            "snd_timer_warning_volume [0-1] — how loud the round timer ticks in its last 10 seconds",
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
    for (name, usage) in [
        (
            "sensitivity",
            "sensitivity [value] — mouse speed, as CS's (a count turns value × 0.022°)",
        ),
        (
            "sensitivity_fov_match",
            "sensitivity_fov_match [0|1] — 1: CS's sensitivity at fov 90, converted to your fov \
             (same feel on screen); 0: the same turn per count at any fov",
        ),
        (
            "zoom_sensitivity_ratio",
            "zoom_sensitivity_ratio [value] — scoped mouse speed (1 CS:GO / CS2, 1.2 CS 1.6)",
        ),
        (
            "m_rawinput",
            "m_rawinput [0|1] — 1: the mouse's raw counts; 0: Windows' pointer (its speed and \
             acceleration)",
        ),
    ] {
        if registry.resolve(name).is_none() {
            registry.register(crate::CommandSpec::new(name).usage(usage));
        }
    }
    if registry.resolve("cl_righthand").is_none() {
        registry.register(
            crate::CommandSpec::new("cl_righthand").usage(
                "cl_righthand [0|1] — CS gun in the right hand (1, default) or the left (0)",
            ),
        );
    }
    if registry.resolve("cl_camera_anim").is_none() {
        registry.register(crate::CommandSpec::new("cl_camera_anim").usage(
            "cl_camera_anim [0|1] — with a CS gun in hand, let the MW2 weapon animations move your view (the head shake on draws and reloads); 0 by default",
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
    if let Some(wanted) = weapon_iw4::csgo::shooting_mode_from_name(&game.shooting_mode)
        && wanted != sim::cs_settings::shooting_mode()
    {
        sim::cs_settings::set_shooting_mode(wanted);
        diag::info!(Console, "shooting_mode = {}", game.shooting_mode);
    }
    if let Some(wanted) = weapon_iw4::cs::smoke_mode_from_name(&game.smoke_mode)
        && wanted != sim::cs_settings::smoke_mode()
    {
        sim::cs_settings::set_smoke_mode(wanted);
        diag::info!(Console, "smoke_mode = {}", game.smoke_mode);
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
                [] => format!("cl_dynamiccrosshair = {}", u8::from(game.crosshair.dynamic)),
                [arg] if arg == "0" || arg == "1" => {
                    game.crosshair.dynamic = arg == "1";
                    game.touch();
                    format!("cl_dynamiccrosshair = {arg}")
                }
                _ => "usage: cl_dynamiccrosshair [0|1]".to_owned(),
            },
            "shooting_mode" => match cmd.args.as_slice() {
                [] => format!(
                    "shooting_mode = {} (csgo: CS:GO's spray and inaccuracy; cs16: CS 1.6's)",
                    game.shooting_mode
                ),
                [arg] if weapon_iw4::csgo::shooting_mode_from_name(arg).is_some() => {
                    game.shooting_mode = arg.trim().to_ascii_lowercase();
                    game.touch();
                    format!(
                        "shooting_mode = {} — for the server you host; on someone else's, theirs \
                         applies",
                        game.shooting_mode
                    )
                }
                _ => "usage: shooting_mode [csgo|cs16]".to_owned(),
            },
            "smoke_quality" => match cmd.args.as_slice() {
                [] => format!("smoke_quality = {} (high|medium|low)", game.smoke_quality),
                [arg] if frame::settings::SmokeQuality::from_name(arg).is_some() => {
                    game.smoke_quality = arg.trim().to_ascii_lowercase();
                    game.touch();
                    format!("smoke_quality = {}", game.smoke_quality)
                }
                _ => "usage: smoke_quality [high|medium|low]".to_owned(),
            },
            "smoke_mode" => match cmd.args.as_slice() {
                [] => format!(
                    "smoke_mode = {} (cs2: CS2's volumetric smoke; csgo: the particle smoke)",
                    game.smoke_mode
                ),
                [arg] if weapon_iw4::cs::smoke_mode_from_name(arg).is_some() => {
                    game.smoke_mode = arg.trim().to_ascii_lowercase();
                    game.touch();
                    format!(
                        "smoke_mode = {} — from the next smoke on the server you host; on \
                         someone else's, theirs applies",
                        game.smoke_mode
                    )
                }
                _ => "usage: smoke_mode [cs2|csgo]".to_owned(),
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
                        None => "usage: mv_mode [csgo|csgo64|csgo128|css|surf|mmod|cs16]".to_owned(),
                    },
                    _ => "usage: mv_mode [csgo|csgo64|csgo128|css|surf|mmod|cs16]".to_owned(),
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
            "_vgui_menus" => match cmd.args.as_slice() {
                [] => format!("_vgui_menus = {}", u8::from(game.vgui_menus)),
                [arg] if arg == "0" || arg == "1" => {
                    game.vgui_menus = arg == "1";
                    game.touch();
                    format!("_vgui_menus = {arg}")
                }
                _ => "usage: _vgui_menus [0|1]".to_owned(),
            },
            "cl_roundbanner" => match cmd.args.as_slice() {
                [] => format!("cl_roundbanner = {}", game.round_banner),
                [arg] if hud::RoundBanner::parse(arg).is_some() => {
                    game.round_banner = arg.trim().to_ascii_lowercase();
                    game.touch();
                    format!("cl_roundbanner = {}", game.round_banner)
                }
                _ => "usage: cl_roundbanner [css|cs16|mw2]".to_owned(),
            },
            "snd_timer_warning_volume" => match cmd.args.as_slice() {
                [] => format!("snd_timer_warning_volume = {:.2}", game.timer_warning_volume),
                [arg] => match arg.parse::<f32>() {
                    Ok(volume) if volume.is_finite() => {
                        game.timer_warning_volume = volume.clamp(0.0, 1.0);
                        game.touch();
                        format!("snd_timer_warning_volume = {:.2}", game.timer_warning_volume)
                    }
                    _ => "usage: snd_timer_warning_volume [0-1]".to_owned(),
                },
                _ => "usage: snd_timer_warning_volume [0-1]".to_owned(),
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
            "sensitivity" | "zoom_sensitivity_ratio" => {
                let slot = if cmd.name == "sensitivity" {
                    &mut game.sensitivity
                } else {
                    &mut game.zoom_sensitivity_ratio
                };
                match cmd.args.as_slice() {
                    [] => format!("{} = {}", cmd.name, *slot),
                    [arg] => match arg.parse::<f32>() {
                        Ok(value) if value.is_finite() && value > 0.0 => {
                            *slot = value;
                            game.sanitize();
                            game.touch();
                            let now = if cmd.name == "sensitivity" {
                                game.sensitivity
                            } else {
                                game.zoom_sensitivity_ratio
                            };
                            format!("{} = {now}", cmd.name)
                        }
                        _ => format!("usage: {} [value]", cmd.name),
                    },
                    _ => format!("usage: {} [value]", cmd.name),
                }
            }
            "sensitivity_fov_match" | "m_rawinput" => {
                let raw = cmd.name == "m_rawinput";
                match cmd.args.as_slice() {
                    [] => format!(
                        "{} = {}",
                        cmd.name,
                        u8::from(if raw {
                            game.raw_input
                        } else {
                            game.sensitivity_fov_match
                        })
                    ),
                    [arg] if arg == "0" || arg == "1" => {
                        if raw {
                            game.raw_input = arg == "1";
                        } else {
                            game.sensitivity_fov_match = arg == "1";
                        }
                        game.touch();
                        format!("{} = {arg}", cmd.name)
                    }
                    _ => format!("usage: {} [0|1]", cmd.name),
                }
            }
            "cl_righthand" => match cmd.args.as_slice() {
                [] => format!("cl_righthand = {}", u8::from(game.right_hand)),
                [arg] if arg == "0" || arg == "1" => {
                    game.right_hand = arg == "1";
                    game.touch();
                    format!("cl_righthand = {arg}")
                }
                _ => "usage: cl_righthand [0|1]".to_owned(),
            },
            "cl_camera_anim" => match cmd.args.as_slice() {
                [] => format!("cl_camera_anim = {}", u8::from(game.camera_anim)),
                [arg] if arg == "0" || arg == "1" => {
                    game.camera_anim = arg == "1";
                    game.touch();
                    format!("cl_camera_anim = {arg}")
                }
                _ => "usage: cl_camera_anim [0|1]".to_owned(),
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
