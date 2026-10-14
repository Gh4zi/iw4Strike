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
    if registry.resolve("always_edgebug").is_none() {
        registry.register(crate::CommandSpec::new("always_edgebug").usage(
            "always_edgebug [0|1] — server cheat, off by default and at every launch: land at any edge, close enough to go off it, and you edgebug: no landing, no fall damage, your speed kept; landings elsewhere hurt as usual (not in mv_mode cs16). The host's applies to everyone on the server",
        ));
    }
    if registry.resolve("always_pixelsurf").is_none() {
        registry.register(crate::CommandSpec::new("always_pixelsurf").usage(
            "always_pixelsurf [0|1] — server cheat, off by default and at every launch: hold a key into any wall while falling and moving along it to pixelsurf: your height holds until you let go or something stops you (not in mv_mode cs16). The host's applies to everyone on the server",
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
            "smoke_mode [csgo|mw2|cs2] — how smoke grenades look on the server you host: CS:GO's static smoke (default), MW2's particle smoke, or CS2's volumetric smoke that fills the room (work in progress)",
        ));
    }
    if registry.resolve("snd_device").is_none() {
        registry.register(crate::CommandSpec::new("snd_device").usage(
            "snd_device [default|<number>|<name>] — the sound output device: with no argument, lists them; switches at once, mid-match included",
        ));
    }
    if registry.resolve("r_fsr").is_none() {
        registry.register(crate::CommandSpec::new("r_fsr").usage(
            "r_fsr [0-5] — AMD FSR 3 upscaling: 0 off, 1 Native AA, 2 Quality, 3 Balanced, 4 Performance, 5 Ultra Performance",
        ));
    }
    if registry.resolve("r_fsr_sharpness").is_none() {
        registry.register(crate::CommandSpec::new("r_fsr_sharpness").usage(
            "r_fsr_sharpness [0-1] — FSR 3's sharpening (default 0.8)",
        ));
    }
    if registry.resolve("console_style").is_none() {
        registry.register(crate::CommandSpec::new("console_style").usage(
            "console_style [0|1] — the console's look: 0 the original panel, 1 the CS2-style window",
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
    if registry.resolve("cl_knife").is_none() {
        registry.register(crate::CommandSpec::new("cl_knife").usage(
            "cl_knife [default|bayonet|m9_bayonet|karambit|butterfly|flip|gut|huntsman|falchion|bowie|shadow_daggers|navaja|stiletto|ursus|talon|classic|paracord|survival|nomad|skeleton|golden] — the CS:GO knife you carry (with CS:GO installed)",
        ));
    }
    if registry.resolve("cl_player_ping_mute").is_none() {
        registry.register(crate::CommandSpec::new("cl_player_ping_mute").usage(
            "cl_player_ping_mute [0|1|2] — teammates' pings with their sound, without it, or not at all",
        ));
    }
    if registry.resolve("cl_radar").is_none() {
        registry.register(
            crate::CommandSpec::new("cl_radar")
                .usage("cl_radar [csgo|mw2] — CS:GO's round radar (with CS:GO installed) or MW2's minimap"),
        );
    }
    if registry.resolve("cl_playermodels").is_none() {
        registry.register(crate::CommandSpec::new("cl_playermodels").usage(
            "cl_playermodels [csgo|mw2] — players as CS:GO's agents with its animations (with CS:GO installed) or MW2's soldiers",
        ));
    }
    if registry.resolve("cl_loadout_pistol").is_none() {
        registry.register(crate::CommandSpec::new("cl_loadout_pistol").usage(
            "cl_loadout_pistol [usp|p2000] — the Counter-Terrorists' starting pistol (USP-S or P2000)",
        ));
    }
    if registry.resolve("cl_loadout_m4").is_none() {
        registry.register(crate::CommandSpec::new("cl_loadout_m4").usage(
            "cl_loadout_m4 [m4a4|m4a1] — the Counter-Terrorists' M4 in the buy wheel (M4A4 or M4A1-S)",
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

/// The movement cheats in play (`always_edgebug`, `always_pixelsurf`): server settings, off
/// unless the host turns them on. Hosting, they are this player's own; on another's server, the
/// server's, which it tells its clients in the server info, so their prediction moves as it does
/// and none turns them on for themselves.
pub(crate) fn follow_server_movement_cheats(
    game: Res<frame::GameSettings>,
    presented: Res<net::PresentedSnapshot>,
    authority: Option<Res<net::AuthorityWorld>>,
) {
    let server = |name: &str| {
        presented.snapshot().is_some_and(|snap| {
            snap.meta
                .objectives
                .server_info
                .iter()
                .any(|(key, value)| key == name && value == "1")
        })
    };
    let (edgebug, pixelsurf) = if authority.is_some() {
        (game.always_edgebug, game.always_pixelsurf)
    } else {
        (server("always_edgebug"), server("always_pixelsurf"))
    };
    if movement_iw4::rules::always_edgebug() != edgebug {
        movement_iw4::rules::set_always_edgebug(edgebug);
        diag::info!(Console, "always_edgebug = {}", u8::from(edgebug));
    }
    if movement_iw4::rules::always_pixelsurf() != pixelsurf {
        movement_iw4::rules::set_always_pixelsurf(pixelsurf);
        diag::info!(Console, "always_pixelsurf = {}", u8::from(pixelsurf));
    }
}

/// `snd_device`: list the output devices, or pick one by its number in the list, by name, or
/// `default` to follow the system's.
fn snd_device(
    args: &[String],
    game: &mut frame::GameSettings,
    audio: Option<&audio::AudioRuntime>,
) -> String {
    let devices = audio.map(audio::AudioRuntime::output_devices).unwrap_or_default();
    let wanted = args.join(" ");
    let picked = match wanted.trim() {
        "" => None,
        "default" | "0" => Some(String::new()),
        arg => match arg.parse::<usize>() {
            Ok(number) => match devices.get(number.wrapping_sub(1)) {
                Some(name) => Some(name.clone()),
                None => return format!("snd_device: no device {number} (snd_device lists them)"),
            },
            Err(_) => Some(arg.to_owned()),
        },
    };
    if let Some(picked) = picked
        && game.audio_device != picked
    {
        game.audio_device = picked;
        game.touch();
    }
    let playing = audio
        .and_then(audio::AudioRuntime::output_device)
        .unwrap_or_else(|| "no device yet".to_owned());
    let listed = devices
        .iter()
        .enumerate()
        .map(|(index, name)| format!("{}: {name}", index + 1))
        .collect::<Vec<_>>()
        .join(" | ");
    let message = format!(
        "snd_device = {} (playing on {playing}) | 0: default | {listed}",
        if game.audio_device.is_empty() {
            "default"
        } else {
            &game.audio_device
        }
    );
    diag::info!(Audio, "{message}");
    message
}

pub(crate) fn route_movement_commands(
    mut events: MessageReader<ConsoleCommand>,
    mut console: ResMut<ConsoleState>,
    settings: Res<ConsoleSettings>,
    mut line: ResMut<ConsoleLine>,
    mut game: ResMut<frame::GameSettings>,
    audio: Option<Res<audio::AudioRuntime>>,
) {
    let capacity = settings.log_capacity;
    for cmd in events.read() {
        let message = match cmd.name.as_str() {
            "snd_device" => snd_device(&cmd.args, &mut game, audio.as_deref()),
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
            "r_fsr" => match cmd.args.as_slice() {
                [] => format!(
                    "r_fsr = {} (0 off, 1 Native AA, 2 Quality, 3 Balanced, 4 Performance, 5 Ultra \
                     Performance)",
                    game.fsr_mode
                ),
                [arg] if matches!(arg.parse::<u8>(), Ok(0..=5)) => {
                    game.fsr_mode = arg.parse().unwrap_or(0);
                    game.touch();
                    format!("r_fsr = {}", game.fsr_mode)
                }
                _ => "usage: r_fsr [0-5]".to_owned(),
            },
            "r_fsr_sharpness" => match cmd.args.as_slice() {
                [] => format!("r_fsr_sharpness = {:.2}", game.fsr_sharpness),
                [arg] if arg.parse::<f32>().is_ok_and(f32::is_finite) => {
                    game.fsr_sharpness = arg.parse::<f32>().unwrap_or(0.8).clamp(0.0, 1.0);
                    game.touch();
                    format!("r_fsr_sharpness = {:.2}", game.fsr_sharpness)
                }
                _ => "usage: r_fsr_sharpness [0-1]".to_owned(),
            },
            "console_style" => match cmd.args.as_slice() {
                [] => format!(
                    "console_style = {} (0: the original panel; 1: the CS2-style window)",
                    game.console_style
                ),
                [arg] if arg == "0" || arg == "1" => {
                    game.console_style = u8::from(arg == "1");
                    game.touch();
                    format!("console_style = {arg}")
                }
                _ => "usage: console_style [0|1]".to_owned(),
            },
            "smoke_mode" => match cmd.args.as_slice() {
                [] => format!(
                    "smoke_mode = {} (csgo: CS:GO's static smoke; mw2: MW2's particle smoke; cs2: CS2's volumetric smoke, work in progress)",
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
                _ => "usage: smoke_mode [csgo|mw2|cs2]".to_owned(),
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
            "always_edgebug" => match cmd.args.as_slice() {
                [] => format!(
                    "always_edgebug = {}",
                    u8::from(movement_iw4::rules::always_edgebug())
                ),
                [arg] if arg == "0" || arg == "1" => {
                    movement_iw4::rules::set_always_edgebug(arg == "1");
                    game.always_edgebug = arg == "1";
                    game.touch();
                    format!("always_edgebug = {arg}")
                }
                _ => "usage: always_edgebug [0|1]".to_owned(),
            },
            "always_pixelsurf" => match cmd.args.as_slice() {
                [] => format!(
                    "always_pixelsurf = {}",
                    u8::from(movement_iw4::rules::always_pixelsurf())
                ),
                [arg] if arg == "0" || arg == "1" => {
                    movement_iw4::rules::set_always_pixelsurf(arg == "1");
                    game.always_pixelsurf = arg == "1";
                    game.touch();
                    format!("always_pixelsurf = {arg}")
                }
                _ => "usage: always_pixelsurf [0|1]".to_owned(),
            },
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
                _ => "usage: cl_roundbanner [csgo|css|cs16|mw2]".to_owned(),
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
            "cl_knife" => match cmd.args.as_slice() {
                [] => format!("cl_knife = {}", game.knife),
                [arg] if arg.trim() == "default"
                    || weapon_iw4::cs::csgo_knife_view_model(arg).is_some() =>
                {
                    game.knife = arg.trim().to_ascii_lowercase();
                    game.touch();
                    format!("cl_knife = {}", game.knife)
                }
                _ => format!(
                    "cl_knife: default, {}",
                    weapon_iw4::cs::CSGO_KNIVES
                        .iter()
                        .map(|(name, _, _)| *name)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            },
            "cl_player_ping_mute" => match cmd.args.as_slice() {
                [] => format!("cl_player_ping_mute = {}", game.ping_mute),
                [arg] if matches!(arg.trim(), "0" | "1" | "2") => {
                    game.ping_mute = arg.trim().parse().unwrap_or(0);
                    game.touch();
                    format!("cl_player_ping_mute = {}", game.ping_mute)
                }
                _ => "usage: cl_player_ping_mute [0|1|2]".to_owned(),
            },
            "cl_radar" => match cmd.args.as_slice() {
                [] => format!("cl_radar = {}", game.radar),
                [arg] if matches!(arg.trim(), "csgo" | "mw2") => {
                    game.radar = arg.trim().to_owned();
                    game.touch();
                    format!("cl_radar = {}", game.radar)
                }
                _ => "usage: cl_radar [csgo|mw2]".to_owned(),
            },
            "cl_playermodels" => match cmd.args.as_slice() {
                [] => format!("cl_playermodels = {}", game.player_models),
                [arg] if matches!(arg.trim(), "csgo" | "mw2") => {
                    game.player_models = arg.trim().to_owned();
                    game.touch();
                    format!("cl_playermodels = {}", game.player_models)
                }
                _ => "usage: cl_playermodels [csgo|mw2]".to_owned(),
            },
            "cl_loadout_pistol" => match cmd.args.as_slice() {
                [] => format!("cl_loadout_pistol = {}", game.loadout_pistol),
                [arg] if matches!(arg.trim(), "usp" | "p2000") => {
                    game.loadout_pistol = arg.trim().to_owned();
                    game.touch();
                    format!("cl_loadout_pistol = {}", game.loadout_pistol)
                }
                _ => "usage: cl_loadout_pistol [usp|p2000]".to_owned(),
            },
            "cl_loadout_m4" => match cmd.args.as_slice() {
                [] => format!("cl_loadout_m4 = {}", game.loadout_m4),
                [arg] if matches!(arg.trim(), "m4a4" | "m4a1") => {
                    game.loadout_m4 = arg.trim().to_owned();
                    game.touch();
                    format!("cl_loadout_m4 = {}", game.loadout_m4)
                }
                _ => "usage: cl_loadout_m4 [m4a4|m4a1]".to_owned(),
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
