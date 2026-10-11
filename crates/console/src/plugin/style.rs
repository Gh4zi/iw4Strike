//! The console's two looks, chosen by `console_style`: 0 the original panel in the bottom-left
//! corner, 1 the CS2-style window (see `window`). Switching rebuilds the panel; the scrollback,
//! the history and the line being typed carry over.

use bevy::{
    input_focus::{FocusCause, InputFocus},
    prelude::*,
    text::{EditableText, LetterSpacing, TextCursorStyle},
    ui::{BoxShadow, ShadowStyle},
};
use ui::UiLayer;

use super::{
    ConsoleEditor, ConsoleFont, ConsoleInputText, ConsoleLog, ConsoleLogText, ConsolePanel,
    ConsolePrompt, ConsoleSettings, ConsoleState, ConsoleSuggestionText, PROMPT, window,
    write_widget_from_editor,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConsoleStyle {
    Classic,
    Cs2,
}

impl ConsoleStyle {
    pub(super) fn from_setting(value: u8) -> Self {
        if value == 1 { Self::Cs2 } else { Self::Classic }
    }

    pub(super) const fn palette(self) -> &'static Palette {
        match self {
            Self::Classic => &CLASSIC,
            Self::Cs2 => &CS2,
        }
    }
}

/// Font size and colours of one look.
pub(super) struct Palette {
    pub font_size: f32,
    /// Typed text (and, in the scrollback, the echo of a submitted line).
    pub body: Color,
    /// Everything else the console prints.
    pub output: Color,
    pub matched: Color,
    pub rest_selected: Color,
    pub rest_muted: Color,
    pub panel: Color,
    pub suggest_bg: Color,
    /// The prompt shows `PROMPT` before the line (the suggestions indent past it).
    pub prompt_glyph: bool,
    /// The scrollback's line height, when the look sets one.
    pub line_height: Option<f32>,
}

const CLASSIC_BODY: Color = Color::srgb(0.82, 0.92, 0.82);

pub(super) const CLASSIC: Palette = Palette {
    font_size: 15.0,
    body: CLASSIC_BODY,
    output: CLASSIC_BODY,
    matched: Color::srgb(0.40, 0.48, 0.42),
    rest_selected: Color::srgb(0.95, 0.78, 0.35),
    rest_muted: Color::srgb(0.48, 0.58, 0.50),
    panel: Color::srgba(0.02, 0.03, 0.04, 0.96),
    suggest_bg: Color::srgb(0.02, 0.03, 0.04),
    prompt_glyph: true,
    line_height: None,
};

/// CS2's console (`gameconsole.vcss`): 14 px text on 19.5 px lines, white, unwrapped. Its
/// suggestions are a list of their own (`fill_cs2_suggestions`), so the suggestion colours
/// here only serve the classic span list.
pub(super) const CS2: Palette = Palette {
    font_size: 14.0,
    body: Color::WHITE,
    output: Color::srgb(0.93, 0.93, 0.93),
    matched: Color::srgba(1.0, 1.0, 1.0, 0.6),
    rest_selected: Color::WHITE,
    rest_muted: Color::srgba(1.0, 1.0, 1.0, 0.6),
    panel: window::COLOR_WINDOW,
    suggest_bg: CS2_SUGGEST_BG,
    prompt_glyph: false,
    line_height: Some(19.5),
};

/// The original panel's size (the CS2 window takes `ConsoleSettings`' size instead).
const CLASSIC_SIZE: Vec2 = Vec2::new(780.0, 360.0);

/// The look on screen, the panel that draws it, and what each look is drawn with.
#[derive(Resource)]
pub(super) struct ConsoleLook {
    pub style: ConsoleStyle,
    pub panel: Entity,
    pub classic_font: Handle<Font>,
    pub cs2_font: Handle<Font>,
    pub cs2_title_font: Handle<Font>,
    /// A rebuilt panel's prompt takes the focus once it exists (the console was open).
    refocus: bool,
}

impl ConsoleLook {
    pub(super) fn font(&self, style: ConsoleStyle) -> &Handle<Font> {
        match style {
            ConsoleStyle::Classic => &self.classic_font,
            ConsoleStyle::Cs2 => &self.cs2_font,
        }
    }
}

/// Marks the CS2 window's root, which `window` places and sizes.
#[derive(Component)]
pub(super) struct Cs2Frame;

const SELECTION: Color = Color::srgba(0.35, 0.55, 0.85, 0.45);
/// CS2's `selectedTextBackgroundColor` (#66669966).
const CS2_SELECTION: Color = Color::srgba(0.4, 0.4, 0.6, 0.4);

fn log_cursor(selection: Color) -> TextCursorStyle {
    TextCursorStyle {
        color: Color::NONE,
        selection_color: selection,
        unfocused_selection_color: selection,
        selected_text_color: None,
    }
}

fn prompt_cursor(palette: &Palette) -> TextCursorStyle {
    TextCursorStyle {
        color: palette.body,
        selection_color: SELECTION,
        unfocused_selection_color: Color::srgba(0.35, 0.55, 0.85, 0.2),
        selected_text_color: None,
    }
}

fn prompt_text() -> EditableText {
    EditableText {
        allow_newlines: false,
        visible_lines: Some(1.0),
        ..default()
    }
}

/// The entities a spawned look hands back: its root, its prompt and its scrollback.
pub(super) struct SpawnedPanel {
    pub panel: Entity,
    pub prompt: Entity,
    pub log: Entity,
}

pub(super) fn spawn_panel(
    commands: &mut Commands,
    style: ConsoleStyle,
    look: &ConsoleLook,
    console_window: window::ConsoleWindow,
) -> SpawnedPanel {
    match style {
        ConsoleStyle::Classic => spawn_classic(commands, look.classic_font.clone()),
        ConsoleStyle::Cs2 => spawn_cs2(
            commands,
            look.cs2_font.clone(),
            look.cs2_title_font.clone(),
            console_window,
        ),
    }
}

fn spawn_classic(commands: &mut Commands, font: Handle<Font>) -> SpawnedPanel {
    let palette = &CLASSIC;
    let text_font = || TextFont {
        font: font.clone().into(),
        font_size: FontSize::Px(palette.font_size),
        ..default()
    };
    let mut prompt = None;
    let mut log = None;
    let panel = commands
        .spawn((
            ConsolePanel,
            UiLayer::Overlay,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                left: px(12),
                bottom: px(12),
                width: px(CLASSIC_SIZE.x),
                height: px(CLASSIC_SIZE.y),
                padding: UiRect::all(px(12)),
                ..default()
            },
            BackgroundColor(palette.panel),
            GlobalZIndex(20_000),
            ZIndex(100),
        ))
        .with_children(|panel| {
            panel
                .spawn(Node {
                    position_type: PositionType::Absolute,
                    left: px(12),
                    right: px(12),
                    top: px(12),
                    bottom: px(36),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::End,
                    overflow: Overflow::clip(),
                    ..default()
                })
                .with_children(|clip| {
                    log = Some(
                        clip.spawn((
                            ConsoleLogText,
                            Text::new(""),
                            text_font(),
                            TextColor(palette.output),
                            log_cursor(SELECTION),
                        ))
                        .id(),
                    );
                });
            panel.spawn((
                ConsoleSuggestionText,
                Node {
                    position_type: PositionType::Absolute,
                    left: px(12),
                    bottom: px(36),
                    padding: UiRect::axes(px(0), px(2)),
                    ..default()
                },
                BackgroundColor(palette.suggest_bg),
                Visibility::Hidden,
                Text::new(""),
                text_font(),
                TextColor(palette.rest_muted),
            ));
            panel
                .spawn(Node {
                    position_type: PositionType::Absolute,
                    left: px(12),
                    right: px(12),
                    bottom: px(12),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(4),
                    overflow: Overflow::clip(),
                    ..default()
                })
                .with_children(|row| {
                    row.spawn((Text::new(PROMPT), text_font(), TextColor(palette.body)));
                    prompt = Some(
                        row.spawn((
                            ConsoleInputText,
                            prompt_text(),
                            TextLayout::no_wrap(),
                            text_font(),
                            TextColor(palette.body),
                            prompt_cursor(palette),
                            Node {
                                flex_grow: 1.0,
                                min_width: px(8),
                                height: px(20),
                                overflow: Overflow::clip(),
                                ..default()
                            },
                        ))
                        .id(),
                    );
                });
        })
        .id();
    SpawnedPanel {
        panel,
        prompt: prompt.expect("console prompt entity"),
        log: log.expect("console log entity"),
    }
}

fn spawn_cs2(
    commands: &mut Commands,
    font: Handle<Font>,
    title_font: Handle<Font>,
    console_window: window::ConsoleWindow,
) -> SpawnedPanel {
    let palette = &CS2;
    let text_font = |font: &Handle<Font>, size: f32| TextFont {
        font: font.clone().into(),
        font_size: FontSize::Px(size),
        ..default()
    };
    let mut prompt = None;
    let mut log = None;
    let panel = commands
        .spawn((
            ConsolePanel,
            Cs2Frame,
            UiLayer::Overlay,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                left: px(console_window.pos.x),
                top: px(console_window.pos.y),
                width: px(console_window.size.x),
                height: px(console_window.size.y),
                flex_direction: FlexDirection::Column,
                border: UiRect::all(px(1)),
                ..default()
            },
            BackgroundColor(palette.panel),
            BorderColor::all(window::COLOR_BORDER),
            BoxShadow(vec![ShadowStyle {
                color: Color::srgba(0.0, 0.0, 0.0, 0.6),
                x_offset: px(2),
                y_offset: px(2),
                spread_radius: px(0),
                blur_radius: px(4),
            }]),
            GlobalZIndex(20_000),
            ZIndex(100),
        ))
        .with_children(|panel| {
            panel
                .spawn((
                    window::ConsoleTitleBar,
                    Node {
                        width: percent(100),
                        height: px(window::TITLE_HEIGHT),
                        flex_shrink: 0.0,
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    BackgroundColor(window::COLOR_TITLE),
                ))
                .with_children(|header| {
                    // CS2's title: Stratum2 Mono bold, uppercase, 2 px letter spacing. The
                    // monospace face matches its visible size at CS2_TITLE_SIZE, drawn twice a
                    // pixel apart for the bold weight it does not have.
                    header
                        .spawn((
                            Node {
                                flex_grow: 1.0,
                                height: percent(100),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ))
                        .with_children(|title| {
                            for nudge in [0.0, 1.0] {
                                title.spawn((
                                    Text::new("CONSOLE"),
                                    text_font(&title_font, CS2_TITLE_SIZE),
                                    LetterSpacing::Px(2.0),
                                    TextColor(Color::WHITE),
                                    Node {
                                        position_type: PositionType::Absolute,
                                        left: px(4.0 + nudge),
                                        top: px(
                                            ((window::TITLE_HEIGHT - CS2_TITLE_SIZE * 1.2) / 2.0)
                                                .round(),
                                        ),
                                        ..default()
                                    },
                                    Pickable::IGNORE,
                                ));
                            }
                        });
                    header
                        .spawn((
                            window::ConsoleCloseButton,
                            Interaction::default(),
                            Node {
                                padding: UiRect::axes(px(4), px(3)),
                                ..default()
                            },
                        ))
                        .with_children(|close| {
                            close.spawn((
                                Text::new("X"),
                                text_font(&title_font, CS2_TITLE_SIZE),
                                TextColor(window::COLOR_CLOSE),
                                Pickable::IGNORE,
                            ));
                        });
                });
            panel
                .spawn((
                    window::ConsoleLogViewport,
                    Node {
                        width: percent(100),
                        flex_grow: 1.0,
                        min_height: px(0),
                        padding: UiRect {
                            left: px(4),
                            right: px(4.0 + window::SCROLLBAR_WIDTH),
                            top: px(4),
                            bottom: px(12),
                        },
                        flex_direction: FlexDirection::Column,
                        // Long lines run on past the right edge and are cut there, as in CS2.
                        overflow: Overflow {
                            x: OverflowAxis::Clip,
                            y: OverflowAxis::Scroll,
                        },
                        ..default()
                    },
                    ScrollPosition::default(),
                ))
                .with_children(|clip| {
                    log = Some(
                        clip.spawn((
                            ConsoleLogText,
                            Text::new(""),
                            text_font(&font, palette.font_size),
                            TextColor(palette.output),
                            TextLayout::no_wrap(),
                            log_cursor(CS2_SELECTION),
                            Node {
                                margin: UiRect::all(px(2)),
                                ..default()
                            },
                        ))
                        .id(),
                    );
                });
            panel
                .spawn((
                    Node {
                        width: percent(100),
                        height: px(window::INPUT_HEIGHT),
                        flex_shrink: 0.0,
                        border: UiRect::top(px(1)),
                        padding: UiRect::all(px(window::INPUT_PADDING)),
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    BackgroundColor(window::COLOR_INPUT_BG),
                    BorderColor::all(window::COLOR_BORDER),
                ))
                .with_children(|row| {
                    prompt = Some(
                        row.spawn((
                            ConsoleInputText,
                            prompt_text(),
                            TextLayout::no_wrap(),
                            text_font(&font, palette.font_size),
                            TextColor(palette.body),
                            prompt_cursor(palette),
                            Node {
                                flex_grow: 1.0,
                                min_width: px(8),
                                height: px(17),
                                overflow: Overflow::clip(),
                                ..default()
                            },
                        ))
                        .id(),
                    );
                });
            panel
                .spawn((
                    window::ConsoleScrollTrack,
                    Node {
                        position_type: PositionType::Absolute,
                        right: px(0),
                        top: px(window::TITLE_HEIGHT),
                        bottom: px(window::INPUT_HEIGHT),
                        width: px(window::SCROLLBAR_WIDTH),
                        ..default()
                    },
                ))
                .with_children(|track| {
                    track.spawn((
                        window::ConsoleScrollThumb,
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(0),
                            right: px(0),
                            top: px(0),
                            height: px(0),
                            border: UiRect {
                                top: px(1),
                                right: px(1),
                                ..default()
                            },
                            ..default()
                        },
                        BackgroundColor(window::COLOR_SCROLL_GRAB),
                        BorderColor {
                            top: Color::srgba(0.667, 0.667, 0.667, 0.133),
                            right: Color::srgba(0.0, 0.0, 0.0, 0.667),
                            bottom: Color::NONE,
                            left: Color::NONE,
                        },
                        Visibility::Hidden,
                    ));
                });
            panel.spawn((
                Cs2Suggestions,
                Node {
                    position_type: PositionType::Absolute,
                    left: px(-1),
                    top: percent(100),
                    width: px(600),
                    max_height: px(200),
                    flex_direction: FlexDirection::Column,
                    overflow: Overflow::clip(),
                    ..default()
                },
                BackgroundColor(CS2_SUGGEST_BG),
                BoxShadow(vec![ShadowStyle {
                    color: Color::srgba(0.0, 0.0, 0.0, 0.4),
                    x_offset: px(0),
                    y_offset: px(0),
                    spread_radius: px(0),
                    blur_radius: px(6),
                }]),
                Visibility::Hidden,
            ));
            panel.spawn(window::resize_bundle());
        })
        .id();
    SpawnedPanel {
        panel,
        prompt: prompt.expect("console prompt entity"),
        log: log.expect("console log entity"),
    }
}

/// The title's size in the monospace face, matching what CS2's 18 px Stratum2 Mono shows
/// (measured against CS2 screenshots: about 70-75 px wide on a 1000 px console).
const CS2_TITLE_SIZE: f32 = 14.0;

/// The CS2 console's autocomplete: a list that drops below the window (CS2's
/// `TextEntryAutocomplete`, `autocompleteposition bottom`).
#[derive(Component)]
pub(super) struct Cs2Suggestions;

pub(super) const CS2_SUGGEST_BG: Color = Color::srgb(0.239, 0.267, 0.282);
const CS2_SUGGEST_SELECTED: Color = Color::srgb(0.345, 0.369, 0.384);
const CS2_SUGGEST_TEXT: Color = Color::srgba(1.0, 1.0, 1.0, 0.6);
const CS2_SUGGEST_RULE: Color = Color::srgba(0.0, 0.0, 0.0, 0.4);
/// Rows the list shows at once: its 200 px height over a 14 px line with 2 px padding and rules.
const CS2_SUGGEST_ROWS: usize = 9;

/// Fills the CS2 autocomplete with the suggestions around the selected one.
pub(super) fn fill_cs2_suggestions(
    commands: &mut Commands,
    list: Entity,
    suggestions: &[String],
    selected: usize,
    font: &Handle<Font>,
) {
    commands.entity(list).despawn_related::<Children>();
    if suggestions.is_empty() {
        return;
    }
    let selected = selected % suggestions.len();
    let start = selected.saturating_sub(CS2_SUGGEST_ROWS - 1);
    let end = (start + CS2_SUGGEST_ROWS).min(suggestions.len());
    commands.entity(list).with_children(|list| {
        for (index, value) in suggestions.iter().enumerate().take(end).skip(start) {
            list.spawn((
                Node {
                    width: percent(100),
                    flex_shrink: 0.0,
                    padding: UiRect::axes(px(8), px(2)),
                    border: UiRect::vertical(px(1)),
                    ..default()
                },
                BackgroundColor(if index == selected {
                    CS2_SUGGEST_SELECTED
                } else {
                    CS2_SUGGEST_BG
                }),
                BorderColor::all(CS2_SUGGEST_RULE),
            ))
            .with_children(|row| {
                row.spawn((
                    Text::new(value.clone()),
                    TextFont {
                        font: font.clone().into(),
                        font_size: FontSize::Px(CS2.font_size),
                        ..default()
                    },
                    TextColor(CS2_SUGGEST_TEXT),
                    TextLayout::no_wrap(),
                ));
            });
        }
    });
}

/// The CS2 window's starting place and size.
pub(super) fn initial_window(settings: &ConsoleSettings) -> window::ConsoleWindow {
    window::ConsoleWindow {
        pos: window::DEFAULT_POS,
        size: Vec2::new(settings.width, settings.height),
    }
}

/// Rebuilds the panel when `console_style` changes, and hands the focus to a rebuilt prompt.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_console_style(
    mut commands: Commands,
    game: Res<frame::GameSettings>,
    mut look: ResMut<ConsoleLook>,
    console_window: Res<window::ConsoleWindow>,
    mut state: ResMut<ConsoleState>,
    mut focus: ResMut<InputFocus>,
    prompt: Res<ConsolePrompt>,
    mut editables: Query<&mut EditableText, With<ConsoleInputText>>,
    mut scroll: ResMut<window::ConsoleScroll>,
) {
    if look.refocus && state.open && editables.contains(prompt.0) {
        look.refocus = false;
        focus.set(prompt.0, FocusCause::Navigated);
        if let Ok(mut editable) = editables.get_mut(prompt.0) {
            let editor: &mut ConsoleEditor = &mut state.editor;
            write_widget_from_editor(&mut editable, editor);
        }
    }
    let wanted = ConsoleStyle::from_setting(game.console_style);
    if wanted == look.style {
        return;
    }
    if focus.get() == Some(prompt.0) {
        focus.clear();
    }
    commands.entity(look.panel).despawn();
    let font = look.font(wanted).clone();
    let spawned = spawn_panel(&mut commands, wanted, &look, *console_window);
    commands.insert_resource(ConsolePrompt(spawned.prompt));
    commands.insert_resource(ConsoleLog(spawned.log));
    commands.insert_resource(ConsoleFont(font));
    look.style = wanted;
    look.panel = spawned.panel;
    look.refocus = state.open;
    scroll.follow = true;
}

/// The look before any panel exists (`panel` is set once the first one is spawned).
pub(super) fn new_look(
    classic_font: Handle<Font>,
    cs2_font: Handle<Font>,
    cs2_title_font: Handle<Font>,
) -> ConsoleLook {
    ConsoleLook {
        style: ConsoleStyle::Classic,
        panel: Entity::PLACEHOLDER,
        classic_font,
        cs2_font,
        cs2_title_font,
        refocus: false,
    }
}
