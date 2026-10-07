//! Counter-Strike scoreboard (TAB) under CS rules, in place of MW2's: the Counter-Terrorists
//! (MW2 allies) and Terrorists (axis) each with their players' score, deaths and latency, team
//! scores and the spectators; free-for-all lists everyone together. Drawn in the look of the
//! installed game: Counter-Strike: Source's rounded dark panel, or Counter-Strike 1.6's flat box
//! with orange headers. Our own layout, no Valve files beyond the system fonts.

use std::path::PathBuf;

use bevy::prelude::*;
use entity_iw4::{TEAM_ALLIES, TEAM_AXIS};
use frame::LaunchIdentity;
use net::{
    ClientActionInput, LocalPresentClient, MasterBridge, MasterBridgeState, PresentedSnapshot,
    Scoreboard,
};

use crate::cs_hud::replaces_mw2_hud;
use crate::scoreboard::{ScoreboardRow, displayed, rows_from_parsed};

/// Which game's scoreboard look to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Style {
    #[default]
    Source,
    GoldSrc,
}

#[derive(Component)]
pub(crate) struct CsBoardRoot;

#[derive(Resource, Default)]
pub(crate) struct CsBoardState {
    style: Style,
    regular: Option<Handle<Font>>,
    bold: Option<Handle<Font>>,
    /// What the board shows now; it is rebuilt only when this changes.
    shown: Option<Board>,
}

#[derive(Clone, Debug, PartialEq)]
struct Board {
    title: String,
    map: String,
    sections: Vec<Section>,
    spectators: Vec<String>,
    size: [u32; 2],
}

#[derive(Clone, Debug, PartialEq)]
struct Section {
    name: &'static str,
    color: [u8; 3],
    score: Option<i32>,
    rows: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq)]
struct Row {
    name: String,
    dead: bool,
    score: i32,
    deaths: i32,
    ping: i32,
    mine: bool,
}

/// Counter-Strike's team colours (GoldSrc `iTeamColors`, CS:S's CT/T blue and red).
const CT_COLOR: [u8; 3] = [153, 204, 255];
const T_COLOR: [u8; 3] = [255, 64, 64];
const FREE_COLOR_SOURCE: [u8; 3] = [255, 255, 255];
/// GoldSrc's default HUD orange, its scoreboard headers and team-less players.
const GOLDSRC_ORANGE: [u8; 3] = [255, 170, 0];
const SPECTATOR_COLOR: [u8; 3] = [204, 204, 204];

/// Panel width and the columns' right edges, in 640x480 units.
const WIDTH: f32 = 560.0;
const COLUMNS: [(&str, f32); 3] = [("Score", 120.0), ("Deaths", 70.0), ("Latency", 70.0)];

fn color([r, g, b]: [u8; 3], alpha: f32) -> Color {
    Color::srgba_u8(r, g, b, (alpha * 255.0) as u8)
}

fn system_font(fonts: &mut Assets<Font>, names: &[&str]) -> Option<Handle<Font>> {
    let dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:/Windows"))
        .join("Fonts");
    names
        .iter()
        .find_map(|name| std::fs::read(dir.join(name)).ok())
        .map(|bytes| fonts.add(Font::from_bytes(bytes)))
}

/// Builds the (hidden) board root under the HUD root once, under CS rules.
pub(crate) fn spawn_cs_scoreboard(
    mut commands: Commands,
    roots: Query<Entity, With<crate::plugin::HudRoot>>,
    existing: Query<(), With<CsBoardRoot>>,
    mut fonts: ResMut<Assets<Font>>,
    mut state: ResMut<CsBoardState>,
) {
    if !replaces_mw2_hud() || !existing.is_empty() {
        return;
    }
    let Ok(root) = roots.single() else {
        return;
    };
    state.style = if asset_transport::find_css_pak().is_none()
        && asset_transport::find_cstrike().is_some()
    {
        Style::GoldSrc
    } else {
        Style::Source
    };
    // CS:S draws its scoreboard in Verdana; GoldSrc's VGUI in Arial-like Tahoma.
    let (regular, bold): (&[&str], &[&str]) = match state.style {
        Style::Source => (&["verdana.ttf", "tahoma.ttf"], &["verdanab.ttf", "tahomabd.ttf"]),
        Style::GoldSrc => (&["tahoma.ttf", "arial.ttf"], &["tahomabd.ttf", "arialbd.ttf"]),
    };
    state.regular = system_font(&mut fonts, regular);
    state.bold = system_font(&mut fonts, bold).or_else(|| state.regular.clone());
    state.shown = None;
    commands.entity(root).with_children(|root| {
        root.spawn((
            CsBoardRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                display: Display::None,
                justify_content: JustifyContent::Center,
                ..default()
            },
            ZIndex(50),
        ));
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_cs_scoreboard(
    mut commands: Commands,
    surface: Res<crate::surface::Hud2dSurface>,
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    actions: Option<Res<ClientActionInput>>,
    scores: Option<Res<Scoreboard>>,
    identity: Option<Res<LaunchIdentity>>,
    bridge: Option<Res<MasterBridge>>,
    mut state: ResMut<CsBoardState>,
    mut roots: Query<(Entity, &mut Node), With<CsBoardRoot>>,
) {
    if !replaces_mw2_hud() {
        return;
    }
    let Ok((root, mut node)) = roots.single_mut() else {
        return;
    };
    let down = actions.as_ref().is_some_and(|a| a.client.kb.scores.active);
    let board = presented
        .snapshot()
        .filter(|snap| surface.is_ready() && displayed(down, snap, local.0))
        .and_then(|snap| {
            let fallback = net::parse_scoreboard_cmd(&net::format_scoreboard_from_snapshot(snap));
            let parsed = scores
                .as_ref()
                .filter(|s| s.cmd.is_some())
                .map_or(&fallback, |s| &s.parsed);
            let rows = rows_from_parsed(snap, parsed);
            (!rows.is_empty()).then(|| {
                let title = bridge
                    .as_ref()
                    .and_then(|b| match b.state() {
                        MasterBridgeState::Hosting { name, .. }
                        | MasterBridgeState::Joined { name, .. } => Some(name),
                        _ => None,
                    })
                    .unwrap_or_else(|| "iw4Strike".to_owned());
                let map = identity.as_ref().map(|i| i.zone.clone()).unwrap_or_default();
                build_board(
                    &rows,
                    snap.meta.kind.is_team(),
                    snap.meta.objectives.scores,
                    local.0.0 as i32,
                    state.style,
                    title,
                    map,
                    [surface.width() as u32, surface.height() as u32],
                )
            })
        });
    let Some(board) = board else {
        if node.display != Display::None {
            node.display = Display::None;
        }
        return;
    };
    if node.display != Display::Flex {
        node.display = Display::Flex;
    }
    if state.shown.as_ref() == Some(&board) {
        return;
    }
    commands.entity(root).despawn_related::<Children>();
    let unit = surface.height() / crate::presentation_scale::VIRTUAL_HEIGHT;
    let fonts = Fonts {
        regular: state.regular.clone().unwrap_or_default(),
        bold: state.bold.clone().unwrap_or_default(),
    };
    let style = state.style;
    commands
        .entity(root)
        .with_children(|root| draw_board(root, &board, style, unit, &fonts));
    state.shown = Some(board);
}

#[allow(clippy::too_many_arguments)]
fn build_board(
    rows: &[ScoreboardRow],
    team_based: bool,
    team_scores: [i32; 3],
    local: i32,
    style: Style,
    title: String,
    map: String,
    size: [u32; 2],
) -> Board {
    let row = |r: &ScoreboardRow| Row {
        name: r.name.clone(),
        dead: r.dead,
        // CS's score is frags.
        score: r.score.kills,
        deaths: r.score.deaths,
        ping: r.score.ping,
        mine: r.score.client == local,
    };
    let players = |team: i32| {
        let mut list: Vec<Row> = rows.iter().filter(|r| r.score.team == team).map(row).collect();
        list.sort_by(|a, b| b.score.cmp(&a.score).then(a.deaths.cmp(&b.deaths)));
        list
    };
    let mut sections = Vec::new();
    if team_based {
        sections.push(Section {
            name: "Counter-Terrorists",
            color: CT_COLOR,
            score: Some(team_scores[TEAM_ALLIES as usize]),
            rows: players(TEAM_ALLIES),
        });
        sections.push(Section {
            name: "Terrorists",
            color: T_COLOR,
            score: Some(team_scores[TEAM_AXIS as usize]),
            rows: players(TEAM_AXIS),
        });
    } else {
        sections.push(Section {
            name: "Players",
            color: match style {
                Style::Source => FREE_COLOR_SOURCE,
                Style::GoldSrc => GOLDSRC_ORANGE,
            },
            score: None,
            rows: rows
                .iter()
                .filter(|r| r.score.team != entity_iw4::TEAM_SPECTATOR)
                .map(row)
                .collect(),
        });
        sections[0]
            .rows
            .sort_by(|a, b| b.score.cmp(&a.score).then(a.deaths.cmp(&b.deaths)));
    }
    let spectators = rows
        .iter()
        .filter(|r| r.score.team == entity_iw4::TEAM_SPECTATOR)
        .map(|r| r.name.clone())
        .collect();
    Board {
        title,
        map,
        sections,
        spectators,
        size,
    }
}

struct Fonts {
    regular: Handle<Font>,
    bold: Handle<Font>,
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
        TextLayout::no_wrap(),
    )
}

/// One line of columns: the name (and status) on the left, the numbers right-aligned in their
/// columns.
fn line(
    parent: &mut ChildSpawnerCommands,
    unit: f32,
    height: f32,
    background: Option<Color>,
    cells: [(String, Color, &Handle<Font>, f32); 5],
) {
    let mut node = parent.spawn(Node {
        width: Val::Percent(100.0),
        height: Val::Px(height * unit),
        align_items: AlignItems::Center,
        padding: UiRect::horizontal(Val::Px(8.0 * unit)),
        ..default()
    });
    if let Some(background) = background {
        node.insert(BackgroundColor(background));
    }
    node.with_children(|row| {
        let [name, status, score, deaths, ping] = cells;
        row.spawn((
            Node {
                flex_grow: 1.0,
                overflow: Overflow::clip(),
                ..default()
            },
            text(name.2, name.3 * unit, name.1, name.0),
        ));
        row.spawn((
            Node {
                width: Val::Px(60.0 * unit),
                ..default()
            },
            text(status.2, status.3 * unit, status.1, status.0),
        ));
        for ((value, color, font, size), (_, width)) in [score, deaths, ping].into_iter().zip(COLUMNS)
        {
            row.spawn((
                Node {
                    width: Val::Px(width * unit),
                    justify_content: JustifyContent::FlexEnd,
                    ..default()
                },
                BackgroundColor(Color::NONE),
            ))
            .with_children(|cell| {
                cell.spawn(text(font, size * unit, color, value));
            });
        }
    });
}

fn draw_board(
    root: &mut ChildSpawnerCommands,
    board: &Board,
    style: Style,
    unit: f32,
    fonts: &Fonts,
) {
    let (panel, radius, border, header, dim) = match style {
        Style::Source => (
            Color::srgba(0.0, 0.0, 0.0, 0.78),
            6.0,
            None,
            Color::WHITE,
            Color::srgb(0.7, 0.7, 0.7),
        ),
        Style::GoldSrc => (
            Color::srgba(0.0, 0.0, 0.0, 0.6),
            0.0,
            Some(color(GOLDSRC_ORANGE, 0.6)),
            color(GOLDSRC_ORANGE, 1.0),
            color(GOLDSRC_ORANGE, 0.8),
        ),
    };
    let mut panel_node = root.spawn((
        Node {
            width: Val::Px(WIDTH * unit),
            max_width: Val::Percent(96.0),
            margin: UiRect::top(Val::Px(40.0 * unit)),
            align_self: AlignSelf::FlexStart,
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(10.0 * unit)),
            row_gap: Val::Px(2.0 * unit),
            border: UiRect::all(Val::Px(if border.is_some() { 1.0 } else { 0.0 })),
            border_radius: BorderRadius::all(Val::Px(radius * unit)),
            ..default()
        },
        BackgroundColor(panel),
    ));
    if let Some(border) = border {
        panel_node.insert(BorderColor::all(border));
    }
    panel_node.with_children(|panel| {
        // Title: the server (or iw4Strike) and the map.
        panel
            .spawn(Node {
                width: Val::Percent(100.0),
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::FlexEnd,
                padding: UiRect::horizontal(Val::Px(8.0 * unit)),
                margin: UiRect::bottom(Val::Px(6.0 * unit)),
                ..default()
            })
            .with_children(|title| {
                title.spawn(text(&fonts.bold, 14.0 * unit, header, board.title.clone()));
                title.spawn(text(&fonts.regular, 10.0 * unit, dim, board.map.clone()));
            });
        // Column titles once, over the first section.
        let titles = |label: &str| (label.to_owned(), dim, &fonts.regular, 9.0);
        line(
            panel,
            unit,
            14.0,
            None,
            [
                titles("Name"),
                titles(""),
                titles(COLUMNS[0].0),
                titles(COLUMNS[1].0),
                titles(COLUMNS[2].0),
            ],
        );
        for section in &board.sections {
            let team = color(section.color, 1.0);
            // Team header: name and player count, team score on the right, a rule under it.
            let count = section.rows.len();
            let players = format!("{count} player{}", if count == 1 { "" } else { "s" });
            let label = if section.score.is_some() {
                format!("{}   {players}", section.name)
            } else {
                players
            };
            line(
                panel,
                unit,
                20.0,
                None,
                [
                    (label, team, &fonts.bold, 11.0),
                    (String::new(), team, &fonts.regular, 9.0),
                    (
                        section.score.map(|s| s.to_string()).unwrap_or_default(),
                        team,
                        &fonts.bold,
                        14.0,
                    ),
                    (String::new(), team, &fonts.regular, 9.0),
                    (String::new(), team, &fonts.regular, 9.0),
                ],
            );
            panel.spawn((
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(1.0_f32.max(unit)),
                    margin: UiRect::bottom(Val::Px(2.0 * unit)),
                    ..default()
                },
                BackgroundColor(color(section.color, 0.6)),
            ));
            for row in &section.rows {
                let tint = if row.dead {
                    color(section.color, 0.55)
                } else {
                    team
                };
                let highlight = row.mine.then(|| match style {
                    Style::Source => Color::srgba(1.0, 1.0, 1.0, 0.12),
                    Style::GoldSrc => color(GOLDSRC_ORANGE, 0.18),
                });
                let cell = |value: String| (value, tint, &fonts.regular, 10.0);
                line(
                    panel,
                    unit,
                    16.0,
                    highlight,
                    [
                        cell(row.name.clone()),
                        (
                            if row.dead { "DEAD".to_owned() } else { String::new() },
                            tint,
                            &fonts.regular,
                            8.0,
                        ),
                        cell(row.score.to_string()),
                        cell(row.deaths.to_string()),
                        cell(if row.ping >= 0 {
                            row.ping.to_string()
                        } else {
                            String::new()
                        }),
                    ],
                );
            }
            panel.spawn(Node {
                height: Val::Px(8.0 * unit),
                ..default()
            });
        }
        if !board.spectators.is_empty() {
            panel
                .spawn(Node {
                    padding: UiRect::horizontal(Val::Px(8.0 * unit)),
                    ..default()
                })
                .with_children(|line| {
                    line.spawn(text(
                        &fonts.regular,
                        9.0 * unit,
                        color(SPECTATOR_COLOR, 1.0),
                        format!("Spectators: {}", board.spectators.join(", ")),
                    ));
                });
        }
    });
}
