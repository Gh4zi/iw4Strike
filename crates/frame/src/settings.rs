use bevy::prelude::Resource;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayResolution {
    pub width: u32,
    pub height: u32,
}

impl DisplayResolution {
    pub const HD: Self = Self {
        width: 1280,
        height: 720,
    };

    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

impl core::fmt::Display for DisplayResolution {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

#[derive(Resource, Clone, Debug, PartialEq)]
pub struct GameSettings {
    pub resolution: DisplayResolution,
    pub fullscreen: bool,
    /// Fullscreen takes the monitor at `resolution` (exclusive) instead of covering it borderless
    /// at its own: how a 4:3 resolution plays stretched, or with black bars, as the GPU's scaling
    /// says.
    pub exclusive_fullscreen: bool,
    pub vsync: bool,
    pub fov: f32,
    /// Counter-Strike viewmodel field of view (horizontal degrees at 4:3, widened Hor+).
    pub viewmodel_fov: f32,
    /// `cl_righthand`: the CS gun is held in the right hand (false: the left).
    pub right_hand: bool,
    /// `cl_camera_anim`: with a CS gun in hand, the MW2 weapon animations underneath still move
    /// the view (their camera bone shakes the head on draws and reloads). Off by default.
    pub camera_anim: bool,
    /// Movement preset (`mv_mode`): csgo, surf, mmod or cs16.
    pub mv_mode: String,
    /// How CS guns shoot when this game is the server (`shooting_mode`): csgo or cs16.
    pub shooting_mode: String,
    /// How smoke grenades look when this game is the server (`smoke_mode`): cs2 (volumetric)
    /// or csgo (particle smoke).
    pub smoke_mode: String,
    /// How finely this game draws CS2 smoke (`smoke_quality`, see [`SmokeQuality`]).
    pub smoke_quality: String,
    /// Map destructibles (cars, barrels) take damage (`sv_destructibles`); off for CS play.
    pub destructibles: bool,
    pub third_person: bool,
    pub master_volume: f32,
    /// `snd_ambient_volume`: the map's own ambience (background bed and looping emitters), 0-1.
    pub ambient_volume: f32,
    /// `snd_timer_warning_volume`: the round timer's last-seconds countdown ticks, 0-1.
    pub timer_warning_volume: f32,
    /// `_vgui_menus`: the buy menu as a window you click (CS:S, or CS 1.6 VGUI), or 0 for
    /// CS 1.6's old numbered text menu.
    pub vgui_menus: bool,
    /// `cl_roundbanner`: the round-end banner, `css` (CS:S win panel), `cs16` (CS 1.6 centre
    /// message) or `mw2` (MW2's round outcome).
    pub round_banner: String,
    pub brightness: f32,
    /// `shadows`: 0 off, 1 the sun's shadow only, 2 sun and spot-light shadows.
    pub shadows: u8,
    pub depth_of_field: bool,
    pub bloom: bool,
    /// `max_frames_ahead`: frames the CPU may queue ahead of the GPU (1 or 2). 2 lets the render
    /// thread record while the GPU finishes the last frame (more fps); 1 adds no queue when the GPU
    /// is the bottleneck. Read when the window is made, so a change applies after a restart.
    pub max_frames_ahead: u8,
    /// The CS crosshair (`cl_crosshair*`).
    pub crosshair: crate::Crosshair,
    /// CS's `sensitivity`: degrees a mouse count turns, over `m_yaw` (0.022), at fov 90.
    pub sensitivity: f32,
    pub invert_mouse: bool,
    /// `sensitivity_fov_match`: at another fov the sensitivity turns as CS's at 90 looks on
    /// screen (monitor distance 0%, by the fovs' half-angle tangents); off, a count turns CS's
    /// degrees at any fov (the same distance a full turn).
    pub sensitivity_fov_match: bool,
    /// `zoom_sensitivity_ratio`: a CS scope slows the mouse by its fov over 90 times this (1 in
    /// CS:GO and CS2, 1.2 in CS 1.6).
    pub zoom_sensitivity_ratio: f32,
    /// `m_rawinput`: the mouse's own counts (Windows raw input); off, the pointer's motion, with
    /// Windows' pointer speed and acceleration.
    pub raw_input: bool,
    pub player_name: String,

    pub pad_layout: u8,
    pub pad_stick_layout: u8,
    pub pad_sensitivity_preset: u8,
    pub pad_custom_sensitivity: f32,
    pub pad_ads_sensitivity: f32,
    pub pad_invert: bool,
    pub pad_curve: u8,
    pub pad_acceleration: bool,
    pub pad_aim_assist: u8,
    pub pad_prompts: u8,
    pub pad_vibration: bool,
    pub pad_deadzone_left: f32,
    pub pad_deadzone_right: f32,

    /// Game folders from the game folders window (`game_path_mw2/css/cz/cs16`, in
    /// `asset_transport::GameFolder::index` order), kept as read so a
    /// save writes them back; `None` until that window first saved them.
    pub game_paths: Option<[String; 5]>,
    /// Whether each game is used (`use_css` / `use_cz` / `use_cs16`, the window's "Use" boxes),
    /// same order, kept as read for the same reason; `None` when the file has no such line.
    pub game_used: Option<[bool; 5]>,

    pub revision: u64,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            resolution: DisplayResolution::HD,
            fullscreen: false,
            exclusive_fullscreen: false,
            vsync: true,
            fov: Self::FOV_DEFAULT,
            viewmodel_fov: Self::VIEWMODEL_FOV_DEFAULT,
            right_hand: true,
            camera_anim: false,
            mv_mode: "csgo".to_owned(),
            shooting_mode: "csgo".to_owned(),
            smoke_mode: "cs2".to_owned(),
            smoke_quality: "high".to_owned(),
            destructibles: false,
            third_person: false,
            master_volume: 1.0,
            ambient_volume: Self::AMBIENT_VOLUME_DEFAULT,
            timer_warning_volume: 1.0,
            vgui_menus: true,
            round_banner: "css".to_owned(),
            brightness: 0.0,
            shadows: Self::SHADOWS_ALL,
            depth_of_field: true,
            bloom: true,
            max_frames_ahead: Self::MAX_FRAMES_AHEAD_DEFAULT,
            crosshair: crate::Crosshair::default(),
            sensitivity: 2.5,
            invert_mouse: false,
            sensitivity_fov_match: true,
            zoom_sensitivity_ratio: 1.0,
            raw_input: true,
            player_name: "Player".to_owned(),
            pad_layout: 0,
            pad_stick_layout: 0,
            pad_sensitivity_preset: 0,
            pad_custom_sensitivity: 1.0,
            pad_ads_sensitivity: 1.0,
            pad_invert: false,
            pad_curve: 0,
            pad_acceleration: true,
            pad_aim_assist: 0,
            pad_prompts: 0,
            pad_vibration: true,
            pad_deadzone_left: 0.12,
            pad_deadzone_right: 0.12,
            game_paths: None,
            game_used: None,
            revision: 0,
        }
    }
}

impl GameSettings {
    pub const FOV_DEFAULT: f32 = 65.0;
    pub const MAX_FRAMES_AHEAD_DEFAULT: u8 = 2;
    pub const SHADOWS_OFF: u8 = 0;
    pub const SHADOWS_SUN: u8 = 1;
    pub const SHADOWS_ALL: u8 = 2;
    pub const FOV_MIN: f32 = 65.0;
    pub const FOV_MAX: f32 = 120.0;
    pub const VIEWMODEL_FOV_DEFAULT: f32 = 68.0;
    pub const AMBIENT_VOLUME_DEFAULT: f32 = 0.35;
    pub const VIEWMODEL_FOV_MIN: f32 = 54.0;
    pub const VIEWMODEL_FOV_MAX: f32 = 90.0;
    pub const PAD_SENSITIVITY_PRESETS: [f32; 10] =
        [0.6, 1.0, 1.4, 1.8, 2.0, 2.2, 2.6, 3.0, 3.5, 4.0];

    pub fn pad_look_sensitivity(&self) -> f32 {
        self.pad_sensitivity_preset
            .checked_sub(1)
            .and_then(|index| Self::PAD_SENSITIVITY_PRESETS.get(usize::from(index)))
            .copied()
            .unwrap_or(self.pad_custom_sensitivity)
    }

    pub const PAD_LAYOUT_CUSTOM: u8 = 255;

    pub fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn sanitize(&mut self) {
        self.resolution.width = self.resolution.width.clamp(640, 7680);
        self.resolution.height = self.resolution.height.clamp(480, 4320);
        self.fov = if self.fov.is_finite() {
            self.fov.clamp(Self::FOV_MIN, Self::FOV_MAX)
        } else {
            Self::FOV_DEFAULT
        };
        self.viewmodel_fov = if self.viewmodel_fov.is_finite() {
            self.viewmodel_fov
                .clamp(Self::VIEWMODEL_FOV_MIN, Self::VIEWMODEL_FOV_MAX)
        } else {
            Self::VIEWMODEL_FOV_DEFAULT
        };
        self.brightness = if self.brightness.is_finite() {
            self.brightness.clamp(-0.2, 0.2)
        } else {
            0.0
        };
        self.master_volume = self.master_volume.clamp(0.0, 1.0);
        self.sensitivity = self.sensitivity.clamp(0.01, 30.0);
        self.zoom_sensitivity_ratio = if self.zoom_sensitivity_ratio.is_finite() {
            self.zoom_sensitivity_ratio.clamp(0.1, 5.0)
        } else {
            1.0
        };
        self.max_frames_ahead = self.max_frames_ahead.clamp(1, 2);
        self.shadows = self.shadows.min(Self::SHADOWS_ALL);
        self.crosshair.sanitize();
        if self.pad_layout != Self::PAD_LAYOUT_CUSTOM {
            self.pad_layout = self.pad_layout.min(4);
        }
        self.pad_stick_layout = self.pad_stick_layout.min(3);
        self.pad_curve = self.pad_curve.min(2);
        self.pad_aim_assist = 0;
        self.pad_prompts = self.pad_prompts.min(3);
        let finite = |v: f32, lo: f32, hi: f32, default: f32| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                default
            }
        };
        self.pad_sensitivity_preset = self.pad_sensitivity_preset.min(10);
        self.pad_custom_sensitivity = finite(self.pad_custom_sensitivity, 0.1, 5.0, 1.0);
        self.pad_ads_sensitivity = finite(self.pad_ads_sensitivity, 0.5, 1.5, 1.0);
        self.pad_deadzone_left = finite(self.pad_deadzone_left, 0.0, 0.4, 0.12);
        self.pad_deadzone_right = finite(self.pad_deadzone_right, 0.0, 0.4, 0.12);
        self.player_name = self.player_name.trim().chars().take(16).collect();
        if self.player_name.is_empty() {
            self.player_name = "Player".to_owned();
        }
    }
}

/// `smoke_quality`: how finely this game draws CS2 smoke, against what it costs. Each player's
/// own; the smoke covers the same space at every level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SmokeQuality {
    /// Every pixel: the sharpest billows and edges.
    #[default]
    High,
    /// Half resolution: about a quarter of the cost.
    Medium,
    /// Half resolution in coarser steps: the cheapest.
    Low,
}

impl SmokeQuality {
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        [("high", Self::High), ("medium", Self::Medium), ("low", Self::Low)]
            .into_iter()
            .find(|(n, _)| name.eq_ignore_ascii_case(n))
            .map(|(_, quality)| quality)
    }
}
