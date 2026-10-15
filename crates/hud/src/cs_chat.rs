//! Chat, as CS's all chat (`messagemode`): T opens a line to type, Enter sends it to everyone on
//! the server (both teams), Escape drops it. The last lines show at the bottom left for 10 s each,
//! names in their team's colour (all the recent ones while typing).

use std::collections::{HashSet, VecDeque};

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use net::LocalPresentClient;

/// How long a line stays up.
const LINE_SECONDS: f64 = 10.0;
/// Lines shown at once.
const SHOWN_LINES: usize = 6;
/// Lines kept for while typing.
const KEPT_LINES: usize = 32;
/// Text size in 640x480 units.
const TEXT_SIZE: f32 = 8.5;

/// CS:GO's name colours: the Counter-Terrorists' blue, the Terrorists' gold, everyone else grey.
const CT_COLOR: [u8; 3] = [153, 204, 255];
const T_COLOR: [u8; 3] = [234, 190, 84];
const OTHER_COLOR: [u8; 3] = [204, 204, 204];

struct ChatLine {
    at: f64,
    name: String,
    text: String,
    team: i32,
}

/// The chat: its typing line and the lines heard.
#[derive(Resource, Default)]
pub struct CsChat {
    open: bool,
    input: String,
    /// Escape closed the typing line this frame (the pause menu must not take it too).
    ate_escape: bool,
    lines: VecDeque<ChatLine>,
    seen: HashSet<u32>,
    font: Option<Handle<Font>>,
    /// What is drawn: the lines (time heard), the typing line, the text size.
    shown: Option<(Vec<u64>, Option<String>, u32, bool)>,
}

impl CsChat {
    /// Typing: the keyboard is the chat's, not the game's.
    #[must_use]
    pub fn captures_input(&self) -> bool {
        self.open
    }

    #[must_use]
    pub fn eats_escape(&self) -> bool {
        self.open || self.ate_escape
    }
}

#[derive(Component)]
pub(crate) struct CsChatRoot;

/// Builds the chat's (empty) column under the HUD root once.
pub(crate) fn spawn_cs_chat(
    mut commands: Commands,
    roots: Query<Entity, With<crate::plugin::HudRoot>>,
    existing: Query<(), With<CsChatRoot>>,
    mut fonts: ResMut<Assets<Font>>,
    mut chat: ResMut<CsChat>,
) {
    if !existing.is_empty() {
        return;
    }
    let Ok(root) = roots.single() else {
        return;
    };
    chat.font = crate::system_fonts::read(&["verdana.ttf", "tahoma.ttf", "arial.ttf"])
        .map(|bytes| fonts.add(Font::from_bytes(bytes)));
    chat.shown = None;
    commands.entity(root).with_children(|root| {
        root.spawn((
            CsChatRoot,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(1.5),
                bottom: Val::Percent(17.0),
                width: Val::Percent(40.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexStart,
                row_gap: Val::Px(2.0),
                ..default()
            },
            ZIndex(40),
        ));
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_cs_chat(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut typed: MessageReader<KeyboardInput>,
    mut notifies: MessageReader<net::SvcGameNotify>,
    mut chat: ResMut<CsChat>,
    mut roots: Query<(Entity, &mut Node), With<CsChatRoot>>,
    (hud_input, buy, presented, local, surface): (
        Option<Res<frame::HudInputView>>,
        Option<Res<crate::cs_buymenu::CsBuyMenu>>,
        Res<net::PresentedSnapshot>,
        Res<LocalPresentClient>,
        Res<crate::surface::Hud2dSurface>,
    ),
    (mut inbox, mut ids, time): (
        Option<ResMut<net::ClientActionInbox>>,
        Option<ResMut<net::ActionRequestIds>>,
        Res<Time<Real>>,
    ),
) {
    let now = time.elapsed_secs_f64();
    chat.ate_escape = false;
    for cmd in notifies.read() {
        if cmd.tag != net::SVC_CHAT || !chat.seen.insert(cmd.id) {
            continue;
        }
        let mut args = cmd.name.split(sim::HUD_PRINT_ARG_SEPARATOR);
        let name = args.next().unwrap_or_default().to_owned();
        let text = args.next().unwrap_or_default().to_owned();
        let team = args.next().and_then(|t| t.parse().ok()).unwrap_or(-1);
        chat.lines.push_back(ChatLine { at: now, name, text, team });
        while chat.lines.len() > KEPT_LINES {
            chat.lines.pop_front();
        }
    }

    let in_game = presented.player(local.0).is_some();
    let console_open = hud_input.as_ref().is_some_and(|h| h.console_open);
    let menu_open = hud_input.as_ref().is_some_and(|h| h.script_menu_open)
        || buy.as_ref().is_some_and(|b| b.captures_input());
    if chat.open && (console_open || !in_game) {
        chat.open = false;
        chat.input.clear();
    }
    if !chat.open {
        // T, by its place on the keyboard or by the letter it types (keys sent by software
        // carry no place). The T that opens it is not typed.
        let t_typed = typed.read().any(|event| {
            event.state == ButtonState::Pressed
                && !event.repeat
                && matches!(&event.logical_key, Key::Character(c) if c.eq_ignore_ascii_case("t"))
        });
        if in_game
            && !console_open
            && !menu_open
            && (keys.just_pressed(KeyCode::KeyT) || t_typed)
        {
            chat.open = true;
            chat.input.clear();
        }
    } else {
        for event in typed.read() {
            if event.state != ButtonState::Pressed {
                continue;
            }
            match &event.logical_key {
                Key::Enter => {
                    let line = chat.input.trim().to_owned();
                    if !line.is_empty()
                        && let (Some(inbox), Some(ids)) = (inbox.as_deref_mut(), ids.as_deref_mut())
                    {
                        let action = sim::ClientAction::Say {
                            request_id: ids.allocate(),
                            text: sim::chat_field(&line),
                        };
                        if let Err(error) = inbox.push(local.0, action) {
                            diag::warn!(Console, "chat: not sent — {error:?}");
                        }
                    }
                    chat.open = false;
                    chat.input.clear();
                    break;
                }
                Key::Escape => {
                    chat.open = false;
                    chat.input.clear();
                    chat.ate_escape = true;
                    break;
                }
                Key::Backspace => {
                    chat.input.pop();
                }
                _ => {
                    for ch in event.text.as_deref().unwrap_or("").chars() {
                        if !ch.is_control() && chat.input.len() + ch.len_utf8() <= sim::CHAT_BYTES {
                            chat.input.push(ch);
                        }
                    }
                }
            }
        }
    }

    // Draw: rebuilt only when what shows changes.
    let Ok((root, mut node)) = roots.single_mut() else {
        return;
    };
    let hidden = menu_open || !in_game;
    let display = if hidden { Display::None } else { Display::Flex };
    if node.display != display {
        node.display = display;
    }
    if hidden || !surface.is_ready() {
        return;
    }
    // MW2's HUD shows chat in its own game messages.
    let lines_here = crate::cs_hud::replaces_mw2_hud();
    let start = chat.lines.len().saturating_sub(SHOWN_LINES);
    let shown_lines: Vec<usize> = (start..chat.lines.len())
        .filter(|&i| lines_here && (chat.open || now - chat.lines[i].at < LINE_SECONDS))
        .collect();
    let unit = surface.height() / crate::presentation_scale::VIRTUAL_HEIGHT;
    let size = (TEXT_SIZE * unit).round().max(10.0);
    let key = (
        shown_lines.iter().map(|&i| chat.lines[i].at.to_bits()).collect::<Vec<_>>(),
        chat.open.then(|| chat.input.clone()),
        size as u32,
        terrorist_allies(presented.snapshot()),
    );
    if chat.shown.as_ref() == Some(&key) {
        return;
    }
    let font = chat.font.clone().unwrap_or_default();
    let terrorist_allies = key.3;
    commands.entity(root).despawn_related::<Children>();
    commands.entity(root).with_children(|column| {
        for &i in &shown_lines {
            let line = &chat.lines[i];
            let name_color = match line.team {
                entity_iw4::TEAM_AXIS | entity_iw4::TEAM_ALLIES => {
                    if (line.team == entity_iw4::TEAM_ALLIES) == terrorist_allies {
                        T_COLOR
                    } else {
                        CT_COLOR
                    }
                }
                _ => OTHER_COLOR,
            };
            column
                .spawn(text_bundle(&font, size, String::new(), [255, 255, 255]))
                .with_children(|spans| {
                    if line.text.is_empty() {
                        // A line from the server itself (its admins): no name, white.
                        for (piece, color) in color_pieces(&line.name, [255, 255, 255]) {
                            spans.spawn(span(&font, size, piece, color));
                        }
                        return;
                    }
                    for (piece, color) in color_pieces(&line.name, name_color) {
                        spans.spawn(span(&font, size, piece, color));
                    }
                    spans.spawn(span(&font, size, " : ".to_owned(), [255, 255, 255]));
                    for (piece, color) in color_pieces(&line.text, [255, 255, 255]) {
                        spans.spawn(span(&font, size, piece, color));
                    }
                });
        }
        if let Some(input) = &key.1 {
            column
                .spawn((
                    Node {
                        padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
                        margin: UiRect::top(Val::Px(4.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
                ))
                .with_children(|bar| {
                    bar.spawn(text_bundle(&font, size, format!("Say : {input}_"), [255, 255, 255]));
                });
        }
    });
    chat.shown = Some(key);
}

/// MW2's `^0`-`^9` colour codes: the pieces of `text` with their colours. `^7` (white) and `^8`
/// go back to `base`, so a name keeps its team's colour after a coloured tag.
fn color_pieces(text: &str, base: [u8; 3]) -> Vec<(String, [u8; 3])> {
    let mut pieces = Vec::new();
    let mut color = base;
    let mut piece = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '^'
            && let Some(code) = chars.peek().and_then(|code| code.to_digit(10))
        {
            chars.next();
            if !piece.is_empty() {
                pieces.push((std::mem::take(&mut piece), color));
            }
            color = match code {
                0 => [0, 0, 0],
                1 => [255, 77, 77],
                2 => [110, 255, 110],
                3 => [255, 225, 80],
                4 => [90, 140, 255],
                5 => [90, 230, 255],
                6 => [255, 100, 220],
                9 => [150, 150, 150],
                _ => base,
            };
            continue;
        }
        piece.push(ch);
    }
    if !piece.is_empty() {
        pieces.push((piece, color));
    }
    pieces
}

/// Whether MW2's allies play the Terrorists (the bomb mode's attackers, as the server says).
fn terrorist_allies(snap: Option<&sim::Snapshot>) -> bool {
    let attackers = snap.and_then(|snap| {
        snap.meta
            .objectives
            .server_info
            .iter()
            .find(|(name, _)| name == "cs_attackers")
            .map(|(_, value)| value.as_str())
    });
    weapon_iw4::cs::cs_plays_terrorist(attackers, true)
}

fn text_bundle(font: &Handle<Font>, size: f32, value: String, color: [u8; 3]) -> impl Bundle {
    (
        Text::new(value),
        TextFont {
            font: font.clone().into(),
            font_size: FontSize::Px(size),
            ..default()
        },
        TextColor(Color::srgb_u8(color[0], color[1], color[2])),
        TextLayout::new(Justify::Left, LineBreak::WordBoundary),
        TextShadow {
            offset: Vec2::splat(1.0),
            color: Color::srgba(0.0, 0.0, 0.0, 0.85),
        },
    )
}

fn span(font: &Handle<Font>, size: f32, value: String, color: [u8; 3]) -> impl Bundle {
    (
        TextSpan::new(value),
        TextFont {
            font: font.clone().into(),
            font_size: FontSize::Px(size),
            ..default()
        },
        TextColor(Color::srgb_u8(color[0], color[1], color[2])),
    )
}

#[cfg(test)]
mod color_tests {
    use super::color_pieces;

    #[test]
    fn colour_codes_split_a_line() {
        let white = [255, 255, 255];
        assert_eq!(
            color_pieces("^3[Admin]^7 hi", white),
            [("[Admin]".to_owned(), [255, 225, 80]), (" hi".to_owned(), white)]
        );
        assert_eq!(color_pieces("plain", white), [("plain".to_owned(), white)]);
        assert_eq!(color_pieces("50^ off", white), [("50^ off".to_owned(), white)]);
    }
}
