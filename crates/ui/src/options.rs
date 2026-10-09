use bevy::{
    prelude::*,
    window::{
        Monitor, MonitorSelection, PresentMode, PrimaryMonitor, PrimaryWindow, VideoModeSelection,
    },
};
use std::collections::BTreeMap;

#[derive(Resource, Clone, Debug, Default)]
pub struct BindingView {
    pub chords: BTreeMap<u32, String>,
    pub listening: Option<u32>,
    pub revision: u64,
}

impl BindingView {
    pub fn chord(&self, command_id: u32) -> &str {
        self.chords
            .get(&command_id)
            .map(String::as_str)
            .unwrap_or("UNBOUND")
    }
}

#[derive(Resource)]
pub struct PresentModeOverride(pub PresentMode);

pub(crate) fn apply_window_settings(
    settings: Res<frame::GameSettings>,
    present_override: Option<Res<PresentModeOverride>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    monitors: Query<&Monitor, With<PrimaryMonitor>>,
    mut applied: Local<Option<(frame::DisplayResolution, bool, bool, PresentMode)>>,
    mut listed: Local<bool>,
) {
    let present_mode = present_override.map_or_else(
        || {
            if settings.vsync {
                PresentMode::AutoVsync
            } else {
                PresentMode::AutoNoVsync
            }
        },
        |mode| mode.0,
    );
    let display = (
        settings.resolution,
        settings.fullscreen,
        settings.exclusive_fullscreen,
        present_mode,
    );
    if applied.as_ref() == Some(&display) {
        return;
    }
    // Which 4:3 sizes exclusive fullscreen can take (the GPU panel can add more), once.
    if !*listed && let Some(monitor) = monitors.iter().next() {
        *listed = true;
        let mut best: std::collections::BTreeMap<(u32, u32), u32> = Default::default();
        for mode in &monitor.video_modes {
            let size = mode.physical_size;
            if size.x * 3 == size.y * 4 {
                let hz = best.entry((size.x, size.y)).or_default();
                *hz = (*hz).max(mode.refresh_rate_millihertz / 1000);
            }
        }
        let sizes: Vec<String> = best
            .iter()
            .map(|((w, h), hz)| format!("{w}x{h} ({hz} Hz)"))
            .collect();
        diag::info!(Ui, "monitor 4:3 modes: {}", sizes.join(", "));
    }
    // The monitor (and its modes) appears a frame or so after the window: wait for it rather
    // than settling for borderless.
    if settings.fullscreen && settings.exclusive_fullscreen && monitors.is_empty() {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    window
        .resolution
        .set_physical_resolution(settings.resolution.width, settings.resolution.height);
    // Exclusive fullscreen takes the monitor at the chosen resolution, at that size's highest
    // refresh rate; one the monitor doesn't list stays borderless at the monitor's own.
    let size = UVec2::new(settings.resolution.width, settings.resolution.height);
    let exclusive = settings
        .exclusive_fullscreen
        .then(|| {
            monitors
                .iter()
                .flat_map(|monitor| monitor.video_modes.iter())
                .filter(|mode| mode.physical_size == size)
                .max_by_key(|mode| (mode.refresh_rate_millihertz, mode.bit_depth))
                .copied()
        })
        .flatten();
    if settings.fullscreen && settings.exclusive_fullscreen && exclusive.is_none() {
        diag::warn!(
            Ui,
            "exclusive fullscreen: the monitor has no {}x{} mode; borderless instead",
            size.x,
            size.y
        );
    }
    window.mode = match (settings.fullscreen, exclusive) {
        (false, _) => bevy::window::WindowMode::Windowed,
        (true, Some(mode)) => bevy::window::WindowMode::Fullscreen(
            MonitorSelection::Current,
            VideoModeSelection::Specific(mode),
        ),
        (true, None) => bevy::window::WindowMode::BorderlessFullscreen(MonitorSelection::Current),
    };
    window.present_mode = present_mode;
    *applied = Some(display);
}
