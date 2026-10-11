//! The CS2 console's right-click menu (`console_style 1`). Selecting text there only marks it,
//! as in CS2; a right click over the scrollback or the prompt, with something selected, opens a
//! small menu whose Copy puts it on the clipboard. Any other click, Escape, or closing the
//! console dismisses the menu.

use bevy::{
    picking::{
        events::{Click, Pointer, Press},
        pointer::PointerButton,
    },
    prelude::*,
    text::EditableText,
};
use ui::UiLayer;

use super::{
    ConsoleFont, ConsoleInputText, ConsoleLog, ConsolePrompt, ConsoleState,
    style::{self, ConsoleStyle},
};

/// CS2's context menu (`context_menus/context_menu_base.vcss`): a dark grey body with a 2 px
/// black border.
const COLOR_MENU: Color = Color::srgba(0.192, 0.192, 0.192, 0.99);
const COLOR_ITEM_HOT: Color = Color::srgb(0.30, 0.30, 0.30);
const COLOR_ITEM_TEXT: Color = Color::srgb(0.9, 0.9, 0.9);

#[derive(Component)]
pub(super) struct ConsoleContextMenu;

#[derive(Component)]
pub(super) struct ConsoleCopyItem;

/// The open menu, if any.
#[derive(Resource, Default)]
pub(super) struct ContextMenuState {
    menu: Option<Entity>,
}

/// The text a Copy would take: the scrollback's selection, else the prompt's.
fn selected_text(state: &ConsoleState, prompt: Option<&EditableText>) -> Option<String> {
    if let Some(anchor) = state.scroll_anchor {
        let joined = state.log.join("\n");
        let slice = crate::input::slice_char_range(&joined, anchor, state.scroll_focus);
        if !slice.is_empty() {
            return Some(slice.to_owned());
        }
    }
    prompt
        .and_then(|editable| editable.editor().selected_text())
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn console_context_menu(
    mut commands: Commands,
    mut presses: MessageReader<Pointer<Press>>,
    mut clicks: MessageReader<Pointer<Click>>,
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<ConsoleState>,
    look: Res<style::ConsoleLook>,
    log: Option<Res<ConsoleLog>>,
    prompt: Option<Res<ConsolePrompt>>,
    font: Res<ConsoleFont>,
    parents: Query<&ChildOf>,
    prompts: Query<&EditableText, With<ConsoleInputText>>,
    items: Query<(), With<ConsoleCopyItem>>,
    ui_scale: Res<UiScale>,
    mut menu_state: ResMut<ContextMenuState>,
    mut clipboard: ResMut<Clipboard>,
) {
    let close = |commands: &mut Commands, menu_state: &mut ContextMenuState| {
        if let Some(menu) = menu_state.menu.take() {
            commands.entity(menu).despawn();
        }
    };
    let cs2 = look.style == ConsoleStyle::Cs2;
    if !state.open || !cs2 || keys.just_pressed(KeyCode::Escape) {
        presses.clear();
        clicks.clear();
        close(&mut commands, &mut menu_state);
        return;
    }
    let prompt_text = prompt
        .as_ref()
        .and_then(|prompt| prompts.get(prompt.0).ok());

    for click in clicks.read() {
        if click.button == PointerButton::Primary && items.contains(click.entity) {
            if let Some(text) = selected_text(&state, prompt_text) {
                // A failed copy has nowhere to say so but the log, which CS2 keeps quiet.
                if let Err(error) = clipboard.set_text(text) {
                    diag::warn!(Console, "console copy failed: {error}");
                }
            }
            close(&mut commands, &mut menu_state);
        }
    }

    let log_entity = log.map(|log| log.0);
    let prompt_entity = prompt.map(|prompt| prompt.0);
    let over_text = |entity: Entity| {
        Some(entity) == prompt_entity
            || Some(entity) == log_entity
            || parents.get(entity).is_ok_and(|parent| Some(parent.0) == log_entity)
            || log_entity.is_some_and(|log| parents.get(log).is_ok_and(|p| p.0 == entity))
    };
    for press in presses.read() {
        match press.button {
            PointerButton::Secondary if over_text(press.entity) => {
                close(&mut commands, &mut menu_state);
                if selected_text(&state, prompt_text).is_none() {
                    continue;
                }
                let at = press.pointer_location.position / ui_scale.0.max(f32::EPSILON);
                menu_state.menu = Some(spawn_menu(&mut commands, at, font.0.clone()));
            }
            PointerButton::Primary if !items.contains(press.entity) => {
                close(&mut commands, &mut menu_state);
            }
            _ => {}
        }
    }
}

fn spawn_menu(commands: &mut Commands, at: Vec2, font: Handle<Font>) -> Entity {
    commands
        .spawn((
            ConsoleContextMenu,
            UiLayer::Overlay,
            Node {
                position_type: PositionType::Absolute,
                left: px(at.x),
                top: px(at.y),
                padding: UiRect {
                    left: px(4),
                    right: px(4),
                    top: px(4),
                    bottom: px(4),
                },
                border: UiRect::all(px(2)),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            BackgroundColor(COLOR_MENU),
            BorderColor::all(Color::BLACK),
            GlobalZIndex(20_001),
        ))
        .with_children(|menu| {
            menu.spawn((
                ConsoleCopyItem,
                Interaction::default(),
                Node {
                    padding: UiRect::axes(px(10), px(4)),
                    min_width: px(90),
                    ..default()
                },
                BackgroundColor(Color::NONE),
            ))
            .with_children(|item| {
                item.spawn((
                    Text::new("Copy"),
                    TextFont {
                        font: font.into(),
                        font_size: FontSize::Px(style::CS2.font_size),
                        ..default()
                    },
                    TextColor(COLOR_ITEM_TEXT),
                    Pickable::IGNORE,
                ));
            });
        })
        .id()
}

/// The menu's item lights up under the pointer.
pub(super) fn highlight_context_menu(
    mut items: Query<
        (&Interaction, &mut BackgroundColor),
        (With<ConsoleCopyItem>, Changed<Interaction>),
    >,
) {
    for (interaction, mut background) in &mut items {
        background.0 = match interaction {
            Interaction::None => Color::NONE,
            Interaction::Hovered | Interaction::Pressed => COLOR_ITEM_HOT,
        };
    }
}
