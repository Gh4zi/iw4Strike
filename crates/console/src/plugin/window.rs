//! The console window: a CS2-style frame around the scrollback and the prompt. It is dragged by
//! its title strip, resized from the grip in its bottom-right corner and closed from its X; the
//! mouse wheel and the scrollbar move through the scrollback, which follows new lines while it
//! sits at the bottom.

use bevy::{
    asset::RenderAssetUsages,
    input::mouse::{MouseScrollUnit, MouseWheel},
    input_focus::InputFocus,
    picking::{
        events::{Click, Drag, Pointer},
        pointer::PointerButton,
    },
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    text::EditableText,
    window::PrimaryWindow,
};

use super::{ConsoleInputText, ConsolePanel, ConsoleState, FONT_SIZE};

pub(super) const TITLE_HEIGHT: f32 = 20.0;
pub(super) const INPUT_HEIGHT: f32 = 30.0;
pub(super) const SCROLLBAR_WIDTH: f32 = 18.0;
pub(super) const LOG_PADDING_LEFT: f32 = 8.0;
pub(super) const INPUT_PADDING: f32 = 7.0;
const GRIP_SIZE: u32 = 16;
const MIN_SIZE: Vec2 = Vec2::new(420.0, 240.0);
const THUMB_MIN: f32 = 20.0;
/// Lines one wheel detent moves the scrollback by.
const WHEEL_LINES: f32 = 3.0;

pub(super) const COLOR_WINDOW: Color = Color::srgb(0.14, 0.14, 0.14);
pub(super) const COLOR_TITLE: Color = Color::srgb(0.204, 0.204, 0.204);
pub(super) const COLOR_INPUT_BG: Color = Color::srgb(0.08, 0.08, 0.08);
pub(super) const COLOR_SCROLL_GRAB: Color = Color::srgb(0.34, 0.41, 0.41);
const COLOR_GRIP: Color = Color::srgb(0.408, 0.408, 0.408);
const COLOR_GRIP_HOT: Color = Color::srgb(0.608, 0.608, 0.608);

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

/// A lower-right triangle, the shape of the resize grip (tinted by `ImageNode::color`).
pub(super) fn grip_image() -> Image {
    let n = GRIP_SIZE as usize;
    let mut data = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            if x + y >= n - 1 {
                data[(y * n + x) * 4..][..4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    Image::new(
        Extent3d {
            width: GRIP_SIZE,
            height: GRIP_SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

pub(super) fn grip_bundle(image: Handle<Image>) -> impl Bundle {
    (
        ConsoleResizeGrip,
        Interaction::default(),
        Node {
            position_type: PositionType::Absolute,
            right: px(0),
            bottom: px(0),
            width: px(GRIP_SIZE as f32),
            height: px(GRIP_SIZE as f32),
            ..default()
        },
        ImageNode {
            image,
            color: COLOR_GRIP,
            ..default()
        },
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
    mut panels: Query<&mut Node, With<ConsolePanel>>,
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

/// The grip lightens under the pointer, as ImGui's does.
pub(super) fn highlight_console_grip(
    mut grips: Query<
        (&Interaction, &mut ImageNode),
        (With<ConsoleResizeGrip>, Changed<Interaction>),
    >,
) {
    for (interaction, mut image) in &mut grips {
        image.color = match interaction {
            Interaction::None => COLOR_GRIP,
            Interaction::Hovered | Interaction::Pressed => COLOR_GRIP_HOT,
        };
    }
}
