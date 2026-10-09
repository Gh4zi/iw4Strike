use std::{fs, path::PathBuf};

use bevy::{
    input::{
        ButtonInput,
        keyboard::KeyCode,
        mouse::{MouseButton, MouseWheel},
    },
    prelude::*,
};

use crate::{BindButton, KeyBinds, binds::wheel_button, display_button};

#[derive(Resource, Default)]
pub(crate) struct PendingMenuBinding {
    id: Option<u32>,
    armed: bool,
}

#[derive(Resource, Default)]
pub(crate) struct UserSettingsPersistence {
    path: Option<PathBuf>,
    last_payload: Option<String>,
    /// The save in flight. Writes leave the frame: a Windows file write and rename can stall for
    /// hundreds of milliseconds.
    writer: Option<std::thread::JoinHandle<()>>,
}

pub(crate) fn load_user_settings(
    identity: Res<ui::LaunchIdentity>,
    mut settings: ResMut<frame::GameSettings>,
    mut binds: ResMut<KeyBinds>,
    mut persistence: ResMut<UserSettingsPersistence>,
) {
    let Some(path) = settings_path(&identity.artifacts) else {
        warn!("no HOME or XDG_CONFIG_HOME; user settings are session-only");
        return;
    };
    persistence.path = Some(path.clone());
    match fs::read_to_string(&path) {
        Ok(source) => parse_settings(&source, &mut settings, &mut binds),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            // Do not replace settings we could not read with session defaults.
            persistence.path = None;
            warn!(
                "could not read {}: {error}; user settings are session-only, file preserved",
                path.display()
            );
        }
    }
    settings.sanitize();
    persistence.last_payload = Some(serialize_settings(&settings, &binds));
}

pub(crate) fn consume_menu_binding(
    mut intents: MessageReader<frame::UiBindRequest>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut wheel: MessageReader<MouseWheel>,
    (gamepads, active, mut settings): (
        Query<&bevy::input::gamepad::Gamepad>,
        Res<frame::ActivePad>,
        ResMut<frame::GameSettings>,
    ),
    mut pending: ResMut<PendingMenuBinding>,
    mut capture: ResMut<frame::UiBindingCapture>,
    mut binds: ResMut<KeyBinds>,
    mut view: ResMut<ui::BindingView>,
) {
    capture.consumed_input = false;
    if capture.command.is_none() {
        pending.id = None;
        view.listening = None;
    }
    let wheel_direction = wheel
        .read()
        .fold(None, |first, event| first.or_else(|| wheel_button(event.y)));
    let mut began = false;
    for intent in intents.read() {
        if let Some(id) = input_iw4::command_id_lookup(&intent.command) {
            capture.command = Some(intent.command.clone());
            pending.id = Some(id);
            view.listening = Some(id);
            view.revision = view.revision.wrapping_add(1);
            pending.armed = false;
            began = true;
        } else {
            capture.command = None;
            pending.id = None;
            view.listening = None;
        }
    }
    let Some(id) = pending.id else { return };
    if began || !pending.armed {
        pending.armed = true;
        return;
    }
    let pad = active.0.and_then(|entity| gamepads.get(entity).ok());
    let pad_start =
        pad.is_some_and(|pad| pad.just_pressed(bevy::input::gamepad::GamepadButton::Start));
    let pad_button = pad.and_then(|pad| {
        pad.get_just_pressed()
            .copied()
            .filter_map(crate::PadButton::from_gamepad_button)
            .find(|button| *button != crate::PadButton::Start)
    });
    if let Some(button) = pad_button.filter(|_| !pad_start) {
        capture.command = None;
        capture.consumed_input = true;
        binds.clear_command_on(id, true);
        binds.set(BindButton::Pad(button), id);
        if settings.pad_layout != frame::GameSettings::PAD_LAYOUT_CUSTOM {
            settings.pad_layout = frame::GameSettings::PAD_LAYOUT_CUSTOM;
            settings.touch();
        }
        pending.id = None;
        pending.armed = false;
        view.listening = None;
        view.revision = view.revision.wrapping_add(1);
        return;
    }
    if keys.just_pressed(KeyCode::Escape) || pad_start {
        capture.command = None;
        capture.consumed_input = true;
        pending.id = None;
        pending.armed = false;
        view.listening = None;
        view.revision = view.revision.wrapping_add(1);
        return;
    }
    let button = keys
        .get_just_pressed()
        .copied()
        .map(BindButton::Key)
        .next()
        .or_else(|| {
            mouse
                .get_just_pressed()
                .copied()
                .map(BindButton::Mouse)
                .next()
        })
        .or(wheel_direction);
    let Some(button) = button else { return };
    capture.command = None;
    capture.consumed_input = true;
    binds.clear_command_on(id, false);
    binds.set_both_sides(button, id);
    pending.id = None;
    pending.armed = false;
    view.listening = None;
    view.revision = view.revision.wrapping_add(1);
}

pub(crate) fn sync_binding_view(
    binds: Res<KeyBinds>,
    devices: Res<frame::InputDevices>,
    mut view: ResMut<ui::BindingView>,
    mut dvars: ResMut<frame::UiMenuDvars>,
    mut style: Local<Option<frame::PromptStyle>>,
) {
    if !binds.is_changed() && *style == Some(devices.style) {
        return;
    }
    *style = Some(devices.style);
    let mut chords = std::collections::BTreeMap::<u32, (Vec<String>, Vec<String>)>::new();
    for (button, id) in binds.iter() {
        let (keys, buttons) = chords.entry(id).or_default();
        match button {
            BindButton::Pad(pad) => buttons.push(pad.prompt(devices.style).to_owned()),
            _ => keys.push(display_button(button)),
        }
    }
    view.chords.clear();
    for (id, (mut keys, mut buttons)) in chords {
        keys.sort();
        keys.dedup();
        buttons.sort();
        buttons.dedup();
        let text = match (keys.is_empty(), buttons.is_empty()) {
            (false, false) => format!("{} | {}", keys.join(" OR "), buttons.join(" OR ")),
            (false, true) => keys.join(" OR "),
            (true, false) => buttons.join(" OR "),
            (true, true) => continue,
        };
        view.chords.insert(id, text);
    }
    // Publish every command so removing its last binding clears the old label.
    for (id, command) in input_iw4::INPUT_COMMAND_NAMES.iter().enumerate().skip(1) {
        dvars.set(&format!("ui_bind_{command}"), view.chord(id as u32));
    }
    view.revision = view.revision.wrapping_add(1);
}

pub(crate) fn apply_master_volume(
    settings: Res<frame::GameSettings>,
    audio: Option<Res<audio::AudioRuntime>>,
) {
    if settings.is_changed()
        && let Some(audio) = audio
    {
        audio.set_master_volume(settings.master_volume);
    }
}

pub(crate) fn sync_player_name(
    settings: Res<frame::GameSettings>,
    generation: Res<frame::WorldGeneration>,
    has_world: Res<frame::HasWorld>,
    role: Res<frame::RuntimeRole>,
    local: Option<Res<net::LocalPresentClient>>,
    link: Option<Res<net::UdpClientLink>>,
    mut inbox: Option<ResMut<net::ClientActionInbox>>,
    mut seq: ResMut<net::ActionRequestIds>,
    mut sent: Local<Option<(frame::WorldGeneration, sim::ClientId, [u8; 16])>>,
) {
    if !has_world.0 || *role == frame::RuntimeRole::Replay {
        *sent = None;
        return;
    }
    let (Some(local), Some(inbox)) = (local, inbox.as_deref_mut()) else {
        return;
    };
    if let Some(link) = link
        && (link.connection.is_none()
            || link.assigned_client != Some(local.0)
            || !link.has_entered_match())
    {
        *sent = None;
        return;
    }
    let name = entity_iw4::pack_client_state_name(&settings.player_name);
    let next = (*generation, local.0, name);
    if sent.as_ref() == Some(&next) {
        return;
    }
    let request_id = seq.allocate();
    if let Err(error) = inbox.push(local.0, sim::ClientAction::SetName { request_id, name }) {
        diag::warn!(
            Console,
            "name: request_id={request_id} not queued — {error}"
        );
    } else {
        *sent = Some(next);
    }
}

pub(crate) fn save_user_settings(
    settings: Res<frame::GameSettings>,
    binds: Res<KeyBinds>,
    mut persistence: ResMut<UserSettingsPersistence>,
) {
    if !settings.is_changed() && !binds.is_changed() {
        return;
    }
    let payload = serialize_settings(&settings, &binds);
    if persistence.last_payload.as_deref() == Some(payload.as_str()) {
        return;
    }
    let Some(path) = persistence.path.clone() else {
        return;
    };
    // One write at a time, so an older payload can never land after a newer one.
    if let Some(previous) = persistence.writer.take() {
        let _ = previous.join();
    }
    let contents = payload.clone();
    persistence.writer = std::thread::Builder::new()
        .name("iw4l-settings-save".into())
        .spawn(move || write_settings_file(&path, &contents))
        .ok();
    persistence.last_payload = Some(payload);
}

impl UserSettingsPersistence {
    /// Wait for a save still on its way to disk; the process is about to exit.
    pub(crate) fn finish_pending_save(&mut self) {
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

fn write_settings_file(path: &std::path::Path, payload: &str) {
    let Some(parent) = path.parent() else { return };
    if let Err(error) = fs::create_dir_all(parent) {
        warn!("could not create {}: {error}", parent.display());
        return;
    }
    let temporary = path.with_extension("cfg.tmp");
    if let Err(error) =
        fs::write(&temporary, payload.as_bytes()).and_then(|()| fs::rename(&temporary, path))
    {
        warn!("could not atomically save {}: {error}", path.display());
    }
}

pub(crate) fn settings_path(artifacts: &std::path::Path) -> Option<PathBuf> {
    asset_transport::settings_file(artifacts)
}

pub(crate) fn apply_pad_layout_setting(
    settings: Res<frame::GameSettings>,
    mut binds: ResMut<KeyBinds>,
    mut seen: Local<Option<u8>>,
) {
    let layout = settings.pad_layout;
    let previous = seen.replace(layout);
    if previous.is_none()
        || previous == Some(layout)
        || layout == frame::GameSettings::PAD_LAYOUT_CUSTOM
    {
        return;
    }
    binds.apply_pad_layout(usize::from(layout));
}

fn parse_into<T: std::str::FromStr>(value: &str, slot: &mut T) {
    if let Ok(value) = value.parse() {
        *slot = value;
    }
}

fn serialize_settings(settings: &frame::GameSettings, binds: &KeyBinds) -> String {
    let safe_name = settings.player_name.replace(['\n', '\r', '='], " ");
    let mut lines = vec![
        "// IW4L user settings v2".to_owned(),
        format!(
            "resolution={}x{}",
            settings.resolution.width, settings.resolution.height
        ),
        format!("fullscreen={}", settings.fullscreen),
        format!("vsync={}", settings.vsync),
        format!("master_volume={:.3}", settings.master_volume),
        format!("snd_ambient_volume={:.2}", settings.ambient_volume),
        format!("snd_timer_warning_volume={:.2}", settings.timer_warning_volume),
        format!("_vgui_menus={}", u8::from(settings.vgui_menus)),
        format!("cl_roundbanner={}", settings.round_banner),
        format!("brightness={:.3}", settings.brightness),
        format!("fov={:.0}", settings.fov),
        format!("viewmodel_fov={:.0}", settings.viewmodel_fov),
        format!("cl_righthand={}", u8::from(settings.right_hand)),
        format!("mv_mode={}", settings.mv_mode),
        format!("sv_destructibles={}", u8::from(settings.destructibles)),
        format!("third_person={}", settings.third_person),
        format!("shadows={}", settings.shadows),
        format!("depth_of_field={}", settings.depth_of_field),
        format!("bloom={}", settings.bloom),
        format!("max_frames_ahead={}", settings.max_frames_ahead),
        format!("sensitivity={:.3}", settings.sensitivity),
        format!("invert_mouse={}", settings.invert_mouse),
        format!("player_name={safe_name}"),
        format!("pad_layout={}", settings.pad_layout),
        format!("pad_stick_layout={}", settings.pad_stick_layout),
        format!("pad_sensitivity_preset={}", settings.pad_sensitivity_preset),
        format!(
            "pad_custom_sensitivity={:.3}",
            settings.pad_custom_sensitivity
        ),
        format!("pad_ads_sensitivity={:.2}", settings.pad_ads_sensitivity),
        format!("pad_invert={}", settings.pad_invert),
        format!("pad_curve={}", settings.pad_curve),
        format!("pad_acceleration={}", settings.pad_acceleration),
        format!("pad_aim_assist={}", settings.pad_aim_assist),
        format!("pad_prompts={}", settings.pad_prompts),
        format!("pad_vibration={}", settings.pad_vibration),
        format!("pad_deadzone_left={:.2}", settings.pad_deadzone_left),
        format!("pad_deadzone_right={:.2}", settings.pad_deadzone_right),
    ];
    lines.extend(
        frame::crosshair::CVARS
            .iter()
            .filter_map(|(name, _)| Some(format!("{name}={}", settings.crosshair.cvar(name)?))),
    );
    if let Some(paths) = &settings.game_paths {
        for folder in asset_transport::GameFolder::ALL {
            lines.push(format!("{}={}", folder.key(), paths[folder.index()]));
        }
    }
    // The game folders window's "Use" boxes, written back as read.
    if let Some(used) = &settings.game_used {
        for folder in asset_transport::GameFolder::ALL {
            if let Some(key) = folder.use_key() {
                lines.push(format!("{key}={}", u8::from(used[folder.index()])));
            }
        }
    }
    lines.push("unbindall".to_owned());
    lines.extend(binds.list_lines());
    lines.push(String::new());
    lines.join("\n")
}

fn parse_settings(source: &str, settings: &mut frame::GameSettings, binds: &mut KeyBinds) {
    let mut bind_script = String::new();
    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line.starts_with("bind ") || line == "unbindall" {
            bind_script.push_str(line);
            bind_script.push('\n');
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warn!("ignored malformed setting line: {line}");
            continue;
        };
        if let Some(folder) = asset_transport::GameFolder::ALL
            .into_iter()
            .find(|folder| folder.key() == key)
        {
            settings.game_paths.get_or_insert_default()[folder.index()] = value.trim().to_owned();
            continue;
        }
        if let Some(folder) = asset_transport::GameFolder::ALL
            .into_iter()
            .find(|folder| folder.use_key() == Some(key))
        {
            settings.game_used.get_or_insert([true; 4])[folder.index()] = value.trim() != "0";
            continue;
        }
        match key {
            "resolution" => {
                if let Some((w, h)) = value.split_once('x')
                    && let (Ok(width), Ok(height)) = (w.parse(), h.parse())
                {
                    settings.resolution = frame::DisplayResolution::new(width, height);
                }
            }
            "fullscreen" => {
                if let Ok(value) = value.parse() {
                    settings.fullscreen = value;
                }
            }
            "vsync" => {
                if let Ok(value) = value.parse() {
                    settings.vsync = value;
                }
            }
            "third_person" => parse_into(value, &mut settings.third_person),
            "fov" => {
                if let Ok(v) = value.parse() {
                    settings.fov = v;
                }
            }
            "viewmodel_fov" => parse_into(value, &mut settings.viewmodel_fov),
            "cl_righthand" => settings.right_hand = value.trim() != "0",
            "snd_timer_warning_volume" => {
                if let Ok(volume) = value.trim().parse::<f32>()
                    && volume.is_finite()
                {
                    settings.timer_warning_volume = volume.clamp(0.0, 1.0);
                }
            }
            "snd_ambient_volume" => {
                if let Ok(volume) = value.trim().parse::<f32>()
                    && volume.is_finite()
                {
                    settings.ambient_volume = volume.clamp(0.0, 1.0);
                }
            }
            "sv_destructibles" => settings.destructibles = value.trim() == "1",
            "_vgui_menus" => settings.vgui_menus = value.trim() != "0",
            "cl_roundbanner" => {
                if hud::RoundBanner::parse(value).is_some() {
                    settings.round_banner = value.trim().to_ascii_lowercase();
                }
            }
            "mv_mode" => {
                if movement_iw4::rules::MovementMode::from_name(value).is_some() {
                    value.trim().clone_into(&mut settings.mv_mode);
                }
            }
            "brightness" => {
                if let Ok(v) = value.parse() {
                    settings.brightness = v;
                }
            }
            "shadows" => {
                // Older files saved on/off; their "off" still drew the sun's shadow, but off
                // now means none.
                settings.shadows = match value.trim() {
                    "true" => frame::GameSettings::SHADOWS_ALL,
                    "false" => frame::GameSettings::SHADOWS_OFF,
                    other => other.parse().unwrap_or(settings.shadows),
                };
            }
            "depth_of_field" => {
                if let Ok(v) = value.parse() {
                    settings.depth_of_field = v;
                }
            }
            "bloom" => {
                if let Ok(v) = value.parse() {
                    settings.bloom = v;
                }
            }
            "max_frames_ahead" => {
                if let Ok(v @ 1..=2) = value.trim().parse::<u8>() {
                    settings.max_frames_ahead = v;
                }
            }
            "master_volume" => {
                if let Ok(value) = value.parse() {
                    settings.master_volume = value;
                }
            }
            "sensitivity" => {
                if let Ok(value) = value.parse() {
                    settings.sensitivity = value;
                }
            }
            "invert_mouse" => {
                if let Ok(value) = value.parse() {
                    settings.invert_mouse = value;
                }
            }
            "player_name" => settings.player_name = value.to_owned(),
            "pad_layout" => parse_into(value, &mut settings.pad_layout),
            "pad_stick_layout" => parse_into(value, &mut settings.pad_stick_layout),
            "pad_sensitivity" => {
                if let Ok(old) = value.parse::<f32>() {
                    settings.pad_custom_sensitivity = old.clamp(1.0, 10.0) / 3.0;
                    settings.pad_sensitivity_preset = 0;
                }
            }
            "pad_sensitivity_preset" => parse_into(value, &mut settings.pad_sensitivity_preset),
            "pad_custom_sensitivity" => parse_into(value, &mut settings.pad_custom_sensitivity),
            "pad_ads_sensitivity" => parse_into(value, &mut settings.pad_ads_sensitivity),
            "pad_invert" => parse_into(value, &mut settings.pad_invert),
            "pad_curve" => parse_into(value, &mut settings.pad_curve),
            "pad_acceleration" => parse_into(value, &mut settings.pad_acceleration),
            "pad_aim_assist" => parse_into(value, &mut settings.pad_aim_assist),
            "pad_prompts" => parse_into(value, &mut settings.pad_prompts),
            "pad_vibration" => parse_into(value, &mut settings.pad_vibration),
            "pad_deadzone_left" => parse_into(value, &mut settings.pad_deadzone_left),
            "pad_deadzone_right" => parse_into(value, &mut settings.pad_deadzone_right),
            key if key.starts_with("cl_crosshair") => {
                if let Err(error) = settings.crosshair.set_cvar(key, value) {
                    warn!("ignored setting: {error}");
                }
            }
            _ => warn!("ignored unknown setting `{key}`"),
        }
    }
    if !bind_script.is_empty() {
        let warnings = binds.apply_config_script(&bind_script);
        for warning in warnings {
            warn!("settings bind: {warning}");
        }
    }
    if !binds.has_pad_binds() {
        binds.apply_pad_layout(usize::from(settings.pad_layout.min(4)));
    }
    if source
        .lines()
        .next()
        .is_some_and(|line| line.trim() == "// IW4L user settings v1")
        && binds.get(BindButton::Key(KeyCode::Digit4)).is_none()
        && !binds.iter().any(|(_, id)| id == 21)
    {
        binds.set(BindButton::Key(KeyCode::Digit4), 21);
    }
    // Counter-Strike: B opens the buy menu, 6-9 and 0 pick its items (slot6..slot10), F1 / F2
    // autobuy / rebuy. Settings
    // saved before those commands existed get them on keys nothing else uses.
    for (key, command) in [
        (KeyCode::KeyB, "buymenu"),
        (KeyCode::F1, "autobuy"),
        (KeyCode::F2, "rebuy"),
        (KeyCode::Digit6, "slot6"),
        (KeyCode::Digit7, "slot7"),
        (KeyCode::Digit8, "slot8"),
        (KeyCode::Digit9, "slot9"),
        (KeyCode::Digit0, "slot10"),
    ] {
        let Some(id) = input_iw4::command_id_from_name(command) else {
            continue;
        };
        if binds.get(BindButton::Key(key)).is_none() && !binds.iter().any(|(_, bound)| bound == id) {
            binds.set(BindButton::Key(key), id);
        }
    }
}

// `frame::GameSettings::game_paths` / `game_used` hold one entry per game folder.
const _: () = assert!(asset_transport::GameFolder::COUNT == 4);

/// The options menu's crosshair rows and the `cl_crosshair*` variable each sets.
const MENU_CROSSHAIR: [(&str, &str); 8] = [
    ("ui_crosshair_style", "cl_crosshairstyle"),
    ("ui_crosshair_size", "cl_crosshairsize"),
    ("ui_crosshair_gap", "cl_crosshairgap"),
    ("ui_crosshair_thickness", "cl_crosshairthickness"),
    ("ui_crosshair_color", "cl_crosshaircolor"),
    ("ui_crosshair_dot", "cl_crosshairdot"),
    ("ui_crosshair_outline", "cl_crosshair_drawoutline"),
    ("ui_crosshair_t", "cl_crosshair_t"),
];

#[allow(clippy::too_many_arguments)]
pub(crate) fn native_menu_settings(
    mut events: MessageReader<crate::ConsoleCommand>,
    mut settings: ResMut<frame::GameSettings>,
    mut dvars: ResMut<frame::UiMenuDvars>,
    mut shadows: ResMut<render_frontend::prepare::scene::view_parms::SmEnableDvar>,
    mut dof: ResMut<render_frontend::assemble::drawsurf::dof::DofDvars>,
    mut glow: ResMut<render_frontend::assemble::drawsurf::dof::GlowDvars>,
    mut test_rumble: MessageWriter<frame::TestControllerRumble>,
    mut code_status: Local<Option<(String, frame::Crosshair)>>,
) {
    for command in events.read() {
        if !matches!(command.name.as_str(), "set" | "seta") {
            continue;
        }
        let [name, value, ..] = command.args.as_slice() else {
            continue;
        };
        match name.as_str() {
            "ui_r_mode" => {
                if let Some((w, h)) = value.split_once('x')
                    && let (Ok(w), Ok(h)) = (w.parse(), h.parse())
                {
                    settings.resolution = frame::DisplayResolution::new(w, h);
                }
            }
            "ui_r_displayMode" => settings.fullscreen = value == "1",
            "ui_r_vsync" => settings.vsync = value == "1",
            "ui_volume" => {
                if let Ok(v) = value.parse::<f32>()
                    && v.is_finite()
                {
                    settings.master_volume = v;
                }
            }
            "ui_player_name" => settings.player_name = value.clone(),
            "ui_timer_warning_volume" => {
                if let Ok(v) = value.parse::<f32>()
                    && v.is_finite()
                {
                    settings.timer_warning_volume = v.clamp(0.0, 1.0);
                }
            }
            "ui_round_banner" => {
                if hud::RoundBanner::parse(value).is_some() {
                    settings.round_banner = value.trim().to_ascii_lowercase();
                }
            }
            "ui_sensitivity" => {
                if let Ok(v) = value.parse::<f32>()
                    && v.is_finite()
                {
                    settings.sensitivity = v;
                }
            }
            "ui_fov" => {
                if let Ok(v) = value.parse::<f32>() {
                    settings.fov = v;
                }
            }
            "ui_brightness" => {
                if let Ok(v) = value.parse::<f32>()
                    && v.is_finite()
                {
                    settings.brightness = v;
                }
            }
            "ui_third_person" | "cg_thirdPerson" => settings.third_person = value == "1",
            "ui_shadows" => parse_into(value, &mut settings.shadows),
            "ui_dof" => settings.depth_of_field = value == "1",
            "ui_bloom" => settings.bloom = value == "1",
            "ui_max_frames_ahead" => parse_into(value, &mut settings.max_frames_ahead),
            "ui_pad_layout" => parse_into(value, &mut settings.pad_layout),
            "ui_pad_stick_layout" => parse_into(value, &mut settings.pad_stick_layout),
            "ui_pad_sensitivity_preset" => parse_into(value, &mut settings.pad_sensitivity_preset),
            "ui_pad_custom_sensitivity" => {
                parse_into(value, &mut settings.pad_custom_sensitivity);
                settings.pad_sensitivity_preset = 0;
            }
            "ui_pad_ads_sensitivity" => parse_into(value, &mut settings.pad_ads_sensitivity),
            "ui_pad_invert" => settings.pad_invert = value == "1",
            "ui_pad_curve" => parse_into(value, &mut settings.pad_curve),
            "ui_pad_acceleration" => settings.pad_acceleration = value == "1",
            "ui_pad_aim_assist" => parse_into(value, &mut settings.pad_aim_assist),
            "ui_pad_prompts" => parse_into(value, &mut settings.pad_prompts),
            "ui_pad_test_rumble" => {
                if value == "1" {
                    test_rumble.write(frame::TestControllerRumble);
                }
            }
            "ui_pad_vibration" => settings.pad_vibration = value == "1",
            "ui_pad_deadzone_left" => parse_into(value, &mut settings.pad_deadzone_left),
            "ui_pad_deadzone_right" => parse_into(value, &mut settings.pad_deadzone_right),
            "ui_crosshair_code" => {
                if value.trim().is_empty() {
                    continue;
                }
                *code_status = match frame::crosshair::decode_share_code(value) {
                    Ok(crosshair) => {
                        settings.crosshair = crosshair;
                        settings.crosshair.sanitize();
                        Some(("Crosshair code imported".to_owned(), settings.crosshair))
                    }
                    Err(error) => {
                        let mut chars = error.chars();
                        let first = chars.next().map(|c| c.to_ascii_uppercase());
                        let error = first.into_iter().chain(chars).collect();
                        Some((error, settings.crosshair))
                    }
                };
            }
            name => {
                let Some((_, cvar)) = MENU_CROSSHAIR.iter().find(|(dvar, _)| *dvar == name) else {
                    continue;
                };
                if let Err(error) = settings.crosshair.set_cvar(cvar, value) {
                    warn!("menu setting: {error}");
                }
            }
        }
        settings.sanitize();
        settings.touch();
    }
    // The code field starts empty; the line under it says how the last code went until the
    // crosshair changes another way, and how to paste one before that.
    if code_status
        .as_ref()
        .is_some_and(|(_, applied)| *applied != settings.crosshair)
    {
        *code_status = None;
    }
    dvars.set("ui_crosshair_code", "");
    dvars.set(
        "ui_crosshair_code_status",
        code_status.as_ref().map_or_else(
            || "Select the line above, Ctrl+V your code, press Enter to import".to_owned(),
            |(status, _)| status.clone(),
        ),
    );
    for (dvar, cvar) in MENU_CROSSHAIR {
        let value = if cvar == "cl_crosshaircolor"
            && settings.crosshair.color_preset() == frame::Crosshair::CUSTOM_COLOR
        {
            Some("Custom".to_owned())
        } else {
            settings.crosshair.cvar(cvar)
        };
        if let Some(value) = value {
            dvars.set(dvar, value);
        }
    }
    if settings.is_changed() {
        shadows.enabled = Some(settings.shadows != frame::GameSettings::SHADOWS_OFF);
        shadows.spot_enabled = Some(settings.shadows == frame::GameSettings::SHADOWS_ALL);
        dof.enable = settings.depth_of_field;
        glow.enable = settings.bloom;
    }
    dvars.set("ui_volume", settings.master_volume.to_string());
    dvars.set("ui_brightness", settings.brightness.to_string());
    dvars.set("ui_fov", settings.fov.to_string());
    dvars.set("ui_sensitivity", settings.sensitivity.to_string());
    for name in ["ui_third_person", "cg_thirdPerson"] {
        dvars.set(name, if settings.third_person { "1" } else { "0" });
    }
    dvars.set("ui_player_name", settings.player_name.clone());
    dvars.set("ui_round_banner", settings.round_banner.clone());
    dvars.set(
        "ui_timer_warning_volume",
        format!("{:.1}", settings.timer_warning_volume),
    );
    dvars.set("ui_shadows", settings.shadows.to_string());
    dvars.set("ui_dof", if settings.depth_of_field { "1" } else { "0" });
    dvars.set("ui_bloom", if settings.bloom { "1" } else { "0" });
    dvars.set("ui_max_frames_ahead", settings.max_frames_ahead.to_string());
    dvars.set("ui_r_mode", settings.resolution.to_string());
    dvars.set(
        "ui_r_displayMode",
        if settings.fullscreen { "1" } else { "0" },
    );
    dvars.set("ui_r_vsync", if settings.vsync { "1" } else { "0" });
    dvars.set("ui_pad_layout", settings.pad_layout.to_string());
    dvars.set("ui_pad_stick_layout", settings.pad_stick_layout.to_string());
    dvars.set(
        "ui_pad_sensitivity_preset",
        settings.pad_sensitivity_preset.to_string(),
    );
    dvars.set(
        "ui_pad_custom_sensitivity",
        settings.pad_custom_sensitivity.to_string(),
    );
    dvars.set(
        "ui_pad_ads_sensitivity",
        settings.pad_ads_sensitivity.to_string(),
    );
    dvars.set("ui_pad_invert", if settings.pad_invert { "1" } else { "0" });
    dvars.set("ui_pad_curve", settings.pad_curve.to_string());
    dvars.set(
        "ui_pad_acceleration",
        if settings.pad_acceleration { "1" } else { "0" },
    );
    dvars.set("ui_pad_aim_assist", settings.pad_aim_assist.to_string());
    dvars.set("ui_pad_prompts", settings.pad_prompts.to_string());
    dvars.set(
        "ui_pad_vibration",
        if settings.pad_vibration { "1" } else { "0" },
    );
    dvars.set(
        "ui_pad_deadzone_left",
        settings.pad_deadzone_left.to_string(),
    );
    dvars.set(
        "ui_pad_deadzone_right",
        settings.pad_deadzone_right.to_string(),
    );
}
