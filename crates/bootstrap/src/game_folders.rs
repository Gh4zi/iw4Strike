//! The game folders window: a small window of its own, shown before the game, where the player
//! selects the MW2, Counter-Strike: Source and Counter-Strike 1.6 folders. They are saved in
//! `settings.cfg`; Play starts the game again as a new process, since one process gets one
//! window event loop.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use asset_transport::{GameFolder, GamesRoot, game_paths};
use bevy::prelude::*;
use bevy::window::{EnabledButtons, WindowResolution};
use bevy::winit::{UpdateMode, WinitSettings};

/// Why the window opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opened {
    /// The game folders were never confirmed (no `game_path_*` lines in settings.cfg).
    FirstLaunch,
    /// MW2 multiplayer data is nowhere to be found.
    Mw2Missing,
    /// `iw4l paths`, or Options > Game Folders in the game.
    Asked,
}

/// Whether the menu launch should first show the window.
pub(crate) fn wanted(games: &GamesRoot) -> Option<Opened> {
    if asset_game::ui_games_root(games).is_err() {
        return Some(Opened::Mw2Missing);
    }
    (!game_paths::confirmed()).then_some(Opened::FirstLaunch)
}

/// Shows the window until Play or Quit; Play saves the folders and starts the game menu again.
pub(crate) fn run(games: &GamesRoot, artifacts: &Path, opened: Opened, cheats: sim::HostCheats) {
    let settings = game_paths::settings_file(artifacts);
    let folders = Folders::load(games, settings, opened);
    let outcome = Arc::new(Mutex::new(Outcome::Quit));

    let mut app = App::new();
    app.add_plugins(crate::plugins::default_plugins_with_quiet_log(
        WindowPlugin {
            primary_window: Some(Window {
                title: "iw4Strike".into(),
                // Laid out in pixels, the same size on every display scale.
                resolution: WindowResolution::new(WIDTH, HEIGHT).with_scale_factor_override(1.0),
                resizable: false,
                enabled_buttons: EnabledButtons {
                    maximize: false,
                    ..default()
                },
                ..default()
            }),
            ..default()
        },
    ));
    // The picker runs on its own thread; poll for its answer while this window is unfocused.
    let poll = UpdateMode::reactive(Duration::from_millis(50));
    app.insert_resource(WinitSettings {
        focused_mode: poll,
        unfocused_mode: poll,
    });
    app.insert_resource(ClearColor(BACKGROUND));
    app.insert_resource(folders);
    app.insert_resource(Shared(outcome.clone()));
    app.init_resource::<Picker>();
    app.add_systems(Startup, spawn_window);
    app.add_systems(
        Update,
        (press_buttons, poll_picker, refresh.run_if(resource_changed::<Folders>)).chain(),
    );
    app.run();

    let outcome = *outcome.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if outcome == Outcome::Play {
        relaunch(cheats);
    }
}

fn relaunch(cheats: sim::HostCheats) {
    let mut args = vec!["menu"];
    if !cheats.0 {
        args.push("--no-cheats");
    }
    match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).args(&args).spawn())
    {
        Ok(_) => diag::info!(Launch, "game folders saved; starting the game"),
        Err(error) => diag::exit_launch_error(&format!("cannot start the game again: {error}")),
    }
}

const WIDTH: u32 = 860;
const HEIGHT: u32 = 600;

// Valve's VGUI "Steam" scheme.
const BACKGROUND: Color = Color::srgb_u8(62, 70, 55);
const PANEL: Color = Color::srgb_u8(76, 88, 68);
const FIELD: Color = Color::srgb_u8(46, 52, 40);
const BORDER_LIGHT: Color = Color::srgb_u8(136, 145, 128);
const BORDER_DARK: Color = Color::srgb_u8(40, 46, 34);
const TEXT: Color = Color::srgb_u8(216, 222, 211);
const DIM: Color = Color::srgb_u8(160, 170, 152);
const ACCENT: Color = Color::srgb_u8(196, 181, 80);
const GOOD: Color = Color::srgb_u8(150, 205, 110);
const WARN: Color = Color::srgb_u8(232, 176, 72);
const BAD: Color = Color::srgb_u8(236, 100, 80);
const BUTTON: Color = Color::srgb_u8(76, 88, 68);
const BUTTON_HOVER: Color = Color::srgb_u8(94, 108, 84);
const BUTTON_DOWN: Color = Color::srgb_u8(56, 64, 50);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Play,
    Quit,
}

#[derive(Resource)]
struct Shared(Arc<Mutex<Outcome>>);

#[derive(Resource)]
struct Folders {
    settings: Option<PathBuf>,
    opened: Opened,
    /// Selected in this window or saved before (resolved to the folder the game reads).
    chosen: [Option<PathBuf>; 3],
    /// A pick (or saved folder) without the game's data, for the status line.
    rejected: [Option<PathBuf>; 3],
    auto_mw2: Option<PathBuf>,
    steam_css: Option<PathBuf>,
    css_env: Option<String>,
    cs16_env: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Good,
    Warn,
    Bad,
    Dim,
}

impl Tone {
    const fn color(self) -> Color {
        match self {
            Self::Good => GOOD,
            Self::Warn => WARN,
            Self::Bad => BAD,
            Self::Dim => DIM,
        }
    }
}

impl Folders {
    fn load(games: &GamesRoot, settings: Option<PathBuf>, opened: Opened) -> Self {
        let saved = settings
            .as_deref()
            .and_then(game_paths::read_saved)
            .unwrap_or_default();
        let mut chosen: [Option<PathBuf>; 3] = Default::default();
        let mut rejected: [Option<PathBuf>; 3] = Default::default();
        for folder in GameFolder::ALL {
            let raw = saved[folder.index()].trim();
            if raw.is_empty() {
                continue;
            }
            match folder.resolve(Path::new(raw)) {
                Some(found) => chosen[folder.index()] = Some(found),
                None => rejected[folder.index()] = Some(PathBuf::from(raw)),
            }
        }
        Self {
            settings,
            opened,
            chosen,
            rejected,
            auto_mw2: asset_transport::discover::auto_mw2_folder(&games.0),
            steam_css: asset_transport::steam_css_pak()
                .and_then(|pak| pak.parent().map(Path::to_path_buf)),
            css_env: asset_transport::css_env_override(),
            cs16_env: asset_transport::cstrike_env_override(),
            error: None,
        }
    }

    /// The folder the game will read for `folder`, and how this window describes it.
    fn status(&self, folder: GameFolder) -> (Option<PathBuf>, Tone, String) {
        let index = folder.index();
        let rejected = self.rejected[index].as_ref().map(|path| {
            format!(
                "{} does not hold {}. ",
                path.display(),
                folder.expected()
            )
        });
        let rejected = rejected.unwrap_or_default();
        match folder {
            GameFolder::Mw2 => {
                if let Some(path) = &self.chosen[index] {
                    (Some(path.clone()), Tone::Good, format!("{rejected}Selected."))
                } else if let Some(path) = &self.auto_mw2 {
                    (
                        Some(path.clone()),
                        Tone::Good,
                        format!("{rejected}Found automatically."),
                    )
                } else {
                    (
                        None,
                        Tone::Bad,
                        format!(
                            "{rejected}Not found. Press Browse and select the Call of Duty \
                             Modern Warfare 2 folder (the one with the zone folder)."
                        ),
                    )
                }
            }
            GameFolder::Css => {
                if let Some(env) = &self.css_env {
                    let dir = PathBuf::from(env);
                    return if dir.join(game_paths::CSS_PAK).is_file() {
                        (Some(dir), Tone::Good, "Set by IW4L_CSS in .env (it overrides this window).".into())
                    } else {
                        (
                            None,
                            Tone::Bad,
                            format!(
                                "IW4L_CSS in .env names {env}, which has no {}. Fix or remove that line.",
                                game_paths::CSS_PAK
                            ),
                        )
                    };
                }
                if let Some(path) = &self.chosen[index] {
                    (Some(path.clone()), Tone::Good, format!("{rejected}Selected."))
                } else if let Some(path) = &self.steam_css {
                    (
                        Some(path.clone()),
                        Tone::Good,
                        format!("{rejected}Found in your Steam library."),
                    )
                } else {
                    (
                        None,
                        Tone::Warn,
                        format!(
                            "{rejected}Not found. Recommended: its models, sounds and HUD. \
                             Without it Counter-Strike 1.6 (below) is used, else the MW2 guns."
                        ),
                    )
                }
            }
            GameFolder::Cs16 => {
                let found = if let Some(env) = &self.cs16_env {
                    let dir = PathBuf::from(env);
                    if !dir.join("models").is_dir() {
                        return (
                            None,
                            Tone::Bad,
                            format!(
                                "IW4L_CSTRIKE in .env names {env}, which has no models folder. \
                                 Fix or remove that line."
                            ),
                        );
                    }
                    Some((dir, "Set by IW4L_CSTRIKE in .env (it overrides this window)."))
                } else {
                    self.chosen[index].clone().map(|dir| (dir, "Selected."))
                };
                let css = self.status(GameFolder::Css).0.is_some();
                match found {
                    Some((dir, _)) if css => (
                        Some(dir),
                        Tone::Dim,
                        "Not used while Counter-Strike: Source is found.".into(),
                    ),
                    Some((dir, how)) => (Some(dir), Tone::Good, format!("{rejected}{how}")),
                    None if css => (
                        None,
                        Tone::Dim,
                        format!("{rejected}Optional. Only used without Counter-Strike: Source."),
                    ),
                    None => (
                        None,
                        Tone::Dim,
                        format!(
                            "{rejected}Optional. Select your Half-Life folder (or its cstrike \
                             folder) to use the Counter-Strike 1.6 models and sounds."
                        ),
                    ),
                }
            }
        }
    }

    fn can_play(&self) -> bool {
        self.status(GameFolder::Mw2).0.is_some()
    }

    fn save(&self) -> Result<(), String> {
        let file = self
            .settings
            .as_deref()
            .ok_or("there is no settings file location")?;
        let paths = GameFolder::ALL.map(|folder| {
            self.chosen[folder.index()]
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        });
        game_paths::write_saved(file, &paths)
    }
}

/// The picker thread's answer: `Some(None)` = cancelled.
#[derive(Resource, Default)]
struct Picker(Option<(GameFolder, Arc<Mutex<Option<Option<PathBuf>>>>)>);

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Action {
    Browse(GameFolder),
    Clear(GameFolder),
    Play,
    Quit,
}

#[derive(Component)]
struct PathText(GameFolder);

#[derive(Component)]
struct StatusText(GameFolder);

#[derive(Component)]
struct BannerText;

fn font_bytes(name: &str) -> Option<Vec<u8>> {
    let dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:/Windows"))
        .join("Fonts");
    std::fs::read(dir.join(name)).ok()
}

const EMBEDDED_FONT: &[u8] = include_bytes!("../../console/assets/FreeMono.otf");

fn load_font(fonts: &mut Assets<Font>, names: &[&str]) -> Handle<Font> {
    let bytes = names
        .iter()
        .find_map(|name| font_bytes(name))
        .unwrap_or_else(|| EMBEDDED_FONT.to_vec());
    fonts.add(Font::from_bytes(bytes))
}

fn text(font: &Handle<Font>, size: f32, color: Color, value: impl Into<String>) -> impl Bundle {
    (
        Text::new(value),
        TextFont {
            font: font.clone().into(),
            font_size: FontSize::Px(size),
            ..default()
        },
        TextColor(color),
    )
}

fn bevel(width: f32) -> (UiRect, BorderColor) {
    (
        UiRect::all(Val::Px(width)),
        BorderColor {
            top: BORDER_LIGHT,
            left: BORDER_LIGHT,
            bottom: BORDER_DARK,
            right: BORDER_DARK,
        },
    )
}

fn button(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    label: &str,
    action: Action,
    width: f32,
) {
    let (border, border_color) = bevel(1.0);
    parent
        .spawn((
            Button,
            action,
            Node {
                width: Val::Px(width),
                height: Val::Px(30.0),
                border,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                flex_shrink: 0.0,
                ..default()
            },
            border_color,
            BackgroundColor(BUTTON),
        ))
        .with_children(|button| {
            button.spawn(text(font, 14.0, TEXT, label));
        });
}

fn spawn_window(mut commands: Commands, mut fonts: ResMut<Assets<Font>>, folders: Res<Folders>) {
    let regular = load_font(&mut fonts, &["verdana.ttf", "tahoma.ttf"]);
    let bold = load_font(&mut fonts, &["verdanab.ttf", "tahomabd.ttf", "verdana.ttf"]);
    commands.spawn(Camera2d);
    let version = option_env!("IW4STRIKE_VERSION")
        .filter(|version| !version.is_empty())
        .map_or_else(|| "development build".to_owned(), |version| format!("version {version}"));
    let settings = folders
        .settings
        .as_ref()
        .map_or_else(String::new, |file| format!("Saved in {}. ", file.display()));

    commands
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(22.0)),
            row_gap: Val::Px(12.0),
            ..default()
        })
        .with_children(|root| {
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::FlexEnd,
                ..default()
            })
            .with_children(|header| {
                header.spawn(text(&bold, 28.0, ACCENT, "iw4Strike"));
                header.spawn(text(&regular, 13.0, DIM, version));
            });
            root.spawn((text(&regular, 14.0, TEXT, ""), BannerText));

            for folder in GameFolder::ALL {
                let (border, border_color) = bevel(1.0);
                root.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::axes(Val::Px(14.0), Val::Px(10.0)),
                        row_gap: Val::Px(6.0),
                        border,
                        ..default()
                    },
                    border_color,
                    BackgroundColor(PANEL),
                ))
                .with_children(|panel| {
                    let tag = match folder {
                        GameFolder::Mw2 => "required",
                        GameFolder::Css => "recommended",
                        GameFolder::Cs16 => "optional",
                    };
                    panel
                        .spawn(Node {
                            column_gap: Val::Px(8.0),
                            align_items: AlignItems::FlexEnd,
                            ..default()
                        })
                        .with_children(|title| {
                            title.spawn(text(&bold, 15.0, TEXT, folder.title()));
                            title.spawn(text(&regular, 12.0, DIM, tag));
                        });
                    panel
                        .spawn(Node {
                            column_gap: Val::Px(8.0),
                            align_items: AlignItems::Center,
                            ..default()
                        })
                        .with_children(|row| {
                            let (border, _) = bevel(1.0);
                            row.spawn((
                                Node {
                                    flex_grow: 1.0,
                                    height: Val::Px(30.0),
                                    padding: UiRect::horizontal(Val::Px(8.0)),
                                    align_items: AlignItems::Center,
                                    overflow: Overflow::clip(),
                                    border,
                                    ..default()
                                },
                                BorderColor {
                                    top: BORDER_DARK,
                                    left: BORDER_DARK,
                                    bottom: BORDER_LIGHT,
                                    right: BORDER_LIGHT,
                                },
                                BackgroundColor(FIELD),
                            ))
                            .with_children(|field| {
                                field.spawn((
                                    text(&regular, 13.0, TEXT, ""),
                                    TextLayout::no_wrap(),
                                    PathText(folder),
                                ));
                            });
                            button(row, &regular, "Browse...", Action::Browse(folder), 96.0);
                            button(row, &regular, "Clear", Action::Clear(folder), 70.0);
                        });
                    panel.spawn((text(&regular, 12.0, DIM, ""), StatusText(folder)));
                });
            }

            root.spawn(Node {
                flex_grow: 1.0,
                ..default()
            });
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                column_gap: Val::Px(12.0),
                ..default()
            })
            .with_children(|footer| {
                footer.spawn((
                    text(
                        &regular,
                        11.0,
                        DIM,
                        format!(
                            "{settings}Reopen this window from Options > Game Folders, or start \
                             iw4l.exe paths. Your games are read in place, never copied."
                        ),
                    ),
                    Node {
                        max_width: Val::Px(560.0),
                        ..default()
                    },
                ));
                footer
                    .spawn(Node {
                        column_gap: Val::Px(8.0),
                        ..default()
                    })
                    .with_children(|buttons| {
                        button(buttons, &bold, "Play", Action::Play, 100.0);
                        button(buttons, &regular, "Quit", Action::Quit, 80.0);
                    });
            });
        });
}

fn press_buttons(
    mut buttons: Query<(&Interaction, &Action, &mut BackgroundColor), Changed<Interaction>>,
    mut folders: ResMut<Folders>,
    mut picker: ResMut<Picker>,
    shared: Res<Shared>,
    mut exit: MessageWriter<AppExit>,
) {
    for (interaction, action, mut background) in &mut buttons {
        let enabled = *action != Action::Play || folders.can_play();
        background.0 = match interaction {
            Interaction::Pressed if enabled => BUTTON_DOWN,
            Interaction::Hovered if enabled => BUTTON_HOVER,
            _ => BUTTON,
        };
        // While the folder picker is open only Quit answers.
        if *interaction != Interaction::Pressed
            || !enabled
            || (picker.0.is_some() && *action != Action::Quit)
        {
            continue;
        }
        match *action {
            Action::Browse(folder) => {
                let start = folders
                    .status(folder)
                    .0
                    .and_then(|path| path.parent().map(Path::to_path_buf));
                let answer = Arc::new(Mutex::new(None));
                let slot = answer.clone();
                let title = match folder {
                    GameFolder::Mw2 => "Select the Call of Duty Modern Warfare 2 folder",
                    GameFolder::Css => "Select the Counter-Strike Source folder",
                    GameFolder::Cs16 => "Select the Half-Life folder (Counter-Strike 1.6)",
                };
                std::thread::spawn(move || {
                    let picked = asset_transport::pick_folder(title, start.as_deref());
                    *slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(picked);
                });
                picker.0 = Some((folder, answer));
            }
            Action::Clear(folder) => {
                folders.chosen[folder.index()] = None;
                folders.rejected[folder.index()] = None;
            }
            Action::Play => match folders.save() {
                Ok(()) => {
                    *shared.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Outcome::Play;
                    exit.write(AppExit::Success);
                }
                Err(error) => {
                    diag::warn!(Launch, "game folders: {error}");
                    folders.error = Some(error);
                }
            },
            Action::Quit => {
                exit.write(AppExit::Success);
            }
        }
    }
}

fn poll_picker(mut picker: ResMut<Picker>, mut folders: ResMut<Folders>) {
    let Some((folder, answer)) = &picker.0 else {
        return;
    };
    let folder = *folder;
    let Some(picked) = answer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    else {
        return;
    };
    picker.0 = None;
    let Some(picked) = picked else {
        return;
    };
    match folder.resolve(&picked) {
        Some(found) => {
            diag::info!(Launch, "game folders: {} = {}", folder.title(), found.display());
            folders.chosen[folder.index()] = Some(found);
            folders.rejected[folder.index()] = None;
        }
        None => folders.rejected[folder.index()] = Some(picked),
    }
}

fn refresh(
    folders: Res<Folders>,
    mut paths: Query<(&PathText, &mut Text, &mut TextColor), (Without<StatusText>, Without<BannerText>)>,
    mut statuses: Query<(&StatusText, &mut Text, &mut TextColor), (Without<PathText>, Without<BannerText>)>,
    mut banner: Query<(&mut Text, &mut TextColor), (With<BannerText>, Without<PathText>, Without<StatusText>)>,
    buttons: Query<(&Action, &Children)>,
    mut labels: Query<&mut TextColor, (Without<PathText>, Without<StatusText>, Without<BannerText>)>,
) {
    for (PathText(folder), mut text, mut color) in &mut paths {
        let (path, _, _) = folders.status(*folder);
        match path {
            Some(path) => {
                text.0 = path.display().to_string();
                color.0 = TEXT;
            }
            None => {
                text.0 = "(none)".into();
                color.0 = DIM;
            }
        }
    }
    for (StatusText(folder), mut text, mut color) in &mut statuses {
        let (_, tone, line) = folders.status(*folder);
        text.0 = line;
        color.0 = tone.color();
    }
    let can_play = folders.can_play();
    if let Ok((mut text, mut color)) = banner.single_mut() {
        let (line, tone) = if let Some(error) = &folders.error {
            (format!("Could not save the game folders: {error}"), BAD)
        } else if !can_play {
            (
                "Call of Duty: Modern Warfare 2 multiplayer was not found. Select its folder \
                 to play."
                    .to_owned(),
                BAD,
            )
        } else {
            match folders.opened {
                Opened::FirstLaunch => (
                    "Welcome. Check the game folders below, then press Play.".to_owned(),
                    TEXT,
                ),
                Opened::Mw2Missing => (
                    "Call of Duty: Modern Warfare 2 found. Press Play to start.".to_owned(),
                    TEXT,
                ),
                Opened::Asked => ("Game folders".to_owned(), TEXT),
            }
        };
        text.0 = line;
        color.0 = tone;
    }
    for (action, children) in &buttons {
        if *action != Action::Play {
            continue;
        }
        for child in children {
            if let Ok(mut color) = labels.get_mut(*child) {
                color.0 = if can_play { TEXT } else { DIM };
            }
        }
    }
}
