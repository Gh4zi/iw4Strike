//! The CS2-style console window (`console_style 1`), after CS2's own console: its layout
//! (`panorama/layout/console.vxml`) and its sizes and colours (`panorama/styles/console.vcss`,
//! `gameconsole.vcss`, and Panorama's scrollbar in `panorama_base.vcss`). It is dragged by its
//! header, resized from the corner mark at its bottom right and closed from its X; the mouse wheel
//! and the scrollbar move through the scrollback, which follows new lines while it sits at the
//! bottom.

use bevy::{
    input::mouse::{MouseScrollUnit, MouseWheel},
    input_focus::InputFocus,
    picking::{
        events::{Click, Drag, Pointer},
        pointer::PointerButton,
    },
    prelude::*,
    text::EditableText,
    window::PrimaryWindow,
};

use super::{ConsoleInputText, ConsoleState, style::Cs2Frame};

const FONT_SIZE: f32 = super::style::CS2.font_size;

/// The header: the title's line and 3-4 px above and below it.
pub(super) const TITLE_HEIGHT: f32 = 24.0;
/// The input: a 14 px line, 4 px padding and its 1 px top border.
pub(super) const INPUT_HEIGHT: f32 = 26.0;
pub(super) const SCROLLBAR_WIDTH: f32 = 8.0;
pub(super) const INPUT_PADDING: f32 = 4.0;
pub(super) const DEFAULT_POS: Vec2 = Vec2::new(20.0, 20.0);
const RESIZE_SIZE: f32 = 10.0;
const MIN_SIZE: Vec2 = Vec2::new(300.0, 300.0);
const THUMB_MIN: f32 = 32.0;
/// Lines one wheel detent moves the scrollback by.
const WHEEL_LINES: f32 = 3.0;

pub(super) const COLOR_WINDOW: Color = Color::srgb(0.133, 0.133, 0.133);
pub(super) const COLOR_BORDER: Color = Color::srgb(0.2, 0.2, 0.2);
pub(super) const COLOR_TITLE: Color = Color::srgb(0.2, 0.2, 0.2);
pub(super) const COLOR_INPUT_BG: Color = Color::BLACK;
pub(super) const COLOR_SCROLL_GRAB: Color = Color::srgb(0.337, 0.404, 0.404);
pub(super) const COLOR_CLOSE: Color = Color::srgb(0.8, 0.8, 0.8);
const COLOR_CLOSE_HOT: Color = Color::WHITE;
const COLOR_CLOSE_DOWN: Color = Color::srgb(0.6, 0.6, 0.6);
const COLOR_RESIZE: Color = Color::srgb(0.4, 0.4, 0.4);
const COLOR_RESIZE_HOT: Color = Color::srgb(0.6, 0.6, 0.6);

/// Where the console window sits and how big it is, in UI pixels.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub(super) struct ConsoleWindow {
    pub pos: Vec2,
    pub size: Vec2,
}

/// Whether the scrollback follows new lines (it sits at the bottom) or was scrolled back.
#[derive(Resource, Clone, Copy, Debug)]
pub(super) struct ConsoleScroll {
    pub follow: bool,
}

impl Default for ConsoleScroll {
    fn default() -> Self {
        Self { follow: true }
    }
}

#[derive(Component)]
pub(super) struct ConsoleTitleBar;

#[derive(Component)]
pub(super) struct ConsoleCloseButton;

#[derive(Component)]
pub(super) struct ConsoleResizeGrip;

#[derive(Component)]
pub(super) struct ConsoleLogViewport;

#[derive(Component)]
pub(super) struct ConsoleScrollTrack;

#[derive(Component)]
pub(super) struct ConsoleScrollThumb;

/// The resize mark: the bottom-right corner of a 10 px square drawn by its right and bottom
/// borders (CS2's `#ResizeDragTarget`).
pub(super) fn resize_bundle() -> impl Bundle {
    (
        ConsoleResizeGrip,
        Interaction::default(),
        Node {
            position_type: PositionType::Absolute,
            right: px(0),
            bottom: px(0),
            width: px(RESIZE_SIZE),
            height: px(RESIZE_SIZE),
            border: UiRect {
                right: px(2),
                bottom: px(2),
                ..default()
            },
            ..default()
        },
        BorderColor::all(COLOR_RESIZE),
    )
}

/// The size of the area the console window has to stay inside, in UI pixels. None while the
/// game window is minimized (Windows reports it as 0x0) or smaller than the console can be:
/// fitting the console to that would shrink it for good.
fn screen_size(windows: &Query<&Window, With<PrimaryWindow>>, ui_scale: f32) -> Option<Vec2> {
    let window = windows.single().ok()?;
    let screen = Vec2::new(window.width(), window.height()) / ui_scale.max(f32::EPSILON);
    (screen.x >= MIN_SIZE.x && screen.y >= MIN_SIZE.y).then_some(screen)
}

/// Where the console shows: the player's placement, fitted to the screen.
fn shown_window(console_window: ConsoleWindow, screen: Option<Vec2>) -> ConsoleWindow {
    screen.map_or(console_window, |screen| {
        clamp_window(console_window, screen)
    })
}

fn clamp_window(mut window: ConsoleWindow, screen: Vec2) -> ConsoleWindow {
    let min = MIN_SIZE.min(screen);
    window.size = window.size.clamp(min, screen.max(min));
    window.pos = window
        .pos
        .clamp(Vec2::ZERO, (screen - window.size).max(Vec2::ZERO));
    window
}

/// Moves the window by its title strip and resizes it by its grip.
pub(super) fn drag_console_window(
    mut drags: MessageReader<Pointer<Drag>>,
    titles: Query<(), With<ConsoleTitleBar>>,
    grips: Query<(), With<ConsoleResizeGrip>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    ui_scale: Res<UiScale>,
    mut console_window: ResMut<ConsoleWindow>,
) {
    let scale = ui_scale.0.max(f32::EPSILON);
    let screen = screen_size(&windows, ui_scale.0);
    let mut next = shown_window(*console_window, screen);
    let mut moved = false;
    for drag in drags.read() {
        if drag.button != PointerButton::Primary {
            continue;
        }
        let delta = drag.delta / scale;
        if titles.contains(drag.entity) {
            next.pos += delta;
            moved = true;
        } else if grips.contains(drag.entity) {
            next.size += delta;
            moved = true;
        }
    }
    if !moved {
        return;
    }
    next = shown_window(next, screen);
    if next != *console_window {
        *console_window = next;
    }
}

/// The X closes the console the way the console key does.
pub(super) fn close_console_button(
    mut clicks: MessageReader<Pointer<Click>>,
    closes: Query<(), With<ConsoleCloseButton>>,
    mut state: ResMut<ConsoleState>,
    mut focus: ResMut<InputFocus>,
    mut editables: Query<&mut EditableText, With<ConsoleInputText>>,
) {
    let mut close = false;
    for click in clicks.read() {
        close |= click.button == PointerButton::Primary && closes.contains(click.entity);
    }
    if !close || !state.open {
        return;
    }
    state.open = false;
    for mut editable in &mut editables {
        editable.pending_edits.clear();
    }
    focus.clear();
}

/// Places the panel where `ConsoleWindow` says, fitted to the screen. The fit is only shown,
/// never stored: a smaller window (or a minimized one) must not shrink the player's console.
pub(super) fn apply_console_window(
    console_window: Res<ConsoleWindow>,
    windows: Query<&Window, With<PrimaryWindow>>,
    ui_scale: Res<UiScale>,
    mut panels: Query<&mut Node, With<Cs2Frame>>,
) {
    let ConsoleWindow { pos, size } =
        shown_window(*console_window, screen_size(&windows, ui_scale.0));
    for mut node in &mut panels {
        let want = (px(pos.x), px(pos.y), px(size.x), px(size.y));
        if (node.left, node.top, node.width, node.height) != want {
            node.left = want.0;
            node.top = want.1;
            node.width = want.2;
            node.height = want.3;
        }
    }
}

/// How far the scrollback can scroll, in UI pixels (0 when it all fits).
fn max_scroll(viewport: &ComputedNode) -> f32 {
    ((viewport.content_size().y - viewport.size().y) * viewport.inverse_scale_factor()).max(0.0)
}

/// The mouse wheel scrolls the scrollback while the console is open.
pub(super) fn scroll_console_log(
    state: Res<ConsoleState>,
    mut wheels: MessageReader<MouseWheel>,
    mut viewports: Query<(&ComputedNode, &mut ScrollPosition), With<ConsoleLogViewport>>,
    mut scroll: ResMut<ConsoleScroll>,
) {
    let lines: f32 = wheels
        .read()
        .map(|wheel| match wheel.unit {
            MouseScrollUnit::Line => wheel.y * WHEEL_LINES,
            MouseScrollUnit::Pixel => wheel.y / (FONT_SIZE * 1.2),
        })
        .sum();
    if !state.open || lines == 0.0 {
        return;
    }
    let Ok((viewport, mut position)) = viewports.single_mut() else {
        return;
    };
    let max = max_scroll(viewport);
    let next = (position.y - lines * FONT_SIZE * 1.2).clamp(0.0, max);
    if next != position.y {
        position.y = next;
    }
    scroll.follow = next >= max - 1.0;
}

/// Keeps the scrollback at its newest line while it follows.
pub(super) fn follow_console_log(
    state: Res<ConsoleState>,
    scroll: Res<ConsoleScroll>,
    mut viewports: Query<(&ComputedNode, &mut ScrollPosition), With<ConsoleLogViewport>>,
) {
    if !state.open || !scroll.follow {
        return;
    }
    let Ok((viewport, mut position)) = viewports.single_mut() else {
        return;
    };
    let max = max_scroll(viewport);
    if position.y != max {
        position.y = max;
    }
}

/// Dragging the scrollbar thumb scrolls the scrollback.
pub(super) fn drag_console_scrollbar(
    mut drags: MessageReader<Pointer<Drag>>,
    thumbs: Query<&ComputedNode, With<ConsoleScrollThumb>>,
    tracks: Query<&ComputedNode, With<ConsoleScrollTrack>>,
    mut viewports: Query<(&ComputedNode, &mut ScrollPosition), With<ConsoleLogViewport>>,
    ui_scale: Res<UiScale>,
    mut scroll: ResMut<ConsoleScroll>,
) {
    let scale = ui_scale.0.max(f32::EPSILON);
    let dy: f32 = drags
        .read()
        .filter(|drag| drag.button == PointerButton::Primary && thumbs.contains(drag.entity))
        .map(|drag| drag.delta.y / scale)
        .sum();
    if dy == 0.0 {
        return;
    }
    let (Ok(thumb), Ok(track), Ok((viewport, mut position))) =
        (thumbs.single(), tracks.single(), viewports.single_mut())
    else {
        return;
    };
    let max = max_scroll(viewport);
    let travel = (track.size().y - thumb.size().y) * track.inverse_scale_factor();
    if travel <= 0.0 || max <= 0.0 {
        return;
    }
    let next = (position.y + dy * max / travel).clamp(0.0, max);
    position.y = next;
    scroll.follow = next >= max - 1.0;
}

/// Sizes and places the scrollbar thumb from the scrollback's extent and position.
pub(super) fn update_console_scrollbar(
    viewports: Query<(&ComputedNode, &ScrollPosition), With<ConsoleLogViewport>>,
    tracks: Query<&ComputedNode, With<ConsoleScrollTrack>>,
    mut thumbs: Query<(&mut Node, &mut Visibility), With<ConsoleScrollThumb>>,
) {
    let (Ok((viewport, position)), Ok(track)) = (viewports.single(), tracks.single()) else {
        return;
    };
    let Ok((mut node, mut visibility)) = thumbs.single_mut() else {
        return;
    };
    let max = max_scroll(viewport);
    let want_visibility = if max > 0.0 {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    if *visibility != want_visibility {
        *visibility = want_visibility;
    }
    if max <= 0.0 {
        return;
    }
    let inverse = viewport.inverse_scale_factor();
    let view = viewport.size().y * inverse;
    let content = viewport.content_size().y * inverse;
    let track_h = track.size().y * track.inverse_scale_factor();
    let thumb_h = (track_h * view / content).clamp(THUMB_MIN.min(track_h), track_h);
    let top = (track_h - thumb_h) * (position.y / max).clamp(0.0, 1.0);
    let (want_top, want_height) = (px(top), px(thumb_h));
    if node.top != want_top || node.height != want_height {
        node.top = want_top;
        node.height = want_height;
    }
}

/// The resize mark and the X light up under the pointer, as CS2's do.
pub(super) fn highlight_console_controls(
    mut grips: Query<
        (&Interaction, &mut BorderColor),
        (With<ConsoleResizeGrip>, Changed<Interaction>),
    >,
    closes: Query<(&Interaction, &Children), (With<ConsoleCloseButton>, Changed<Interaction>)>,
    mut labels: Query<&mut TextColor>,
) {
    for (interaction, mut border) in &mut grips {
        *border = BorderColor::all(match interaction {
            Interaction::None => COLOR_RESIZE,
            Interaction::Hovered | Interaction::Pressed => COLOR_RESIZE_HOT,
        });
    }
    for (interaction, children) in &closes {
        let color = match interaction {
            Interaction::None => COLOR_CLOSE,
            Interaction::Hovered => COLOR_CLOSE_HOT,
            Interaction::Pressed => COLOR_CLOSE_DOWN,
        };
        for child in children {
            if let Ok(mut text) = labels.get_mut(*child) {
                text.0 = color;
            }
        }
    }
}
