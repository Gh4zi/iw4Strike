//! The round-end banner, in the look picked with `cl_roundbanner` (Options > Multiplayer):
//! `css` CS:S's win panel (the winning side in its colour, the reason underneath, low in the
//! middle of the screen), `cs16` CS 1.6's centre message ("Terrorists Win!", "Target
//! Succesfully Bombed!"), or `mw2` MW2's own round outcome (its script HUD, left alone).
//!
//! The bomb mode's script publishes each decided round as the server-info value
//! `cs_round_end` = "<time> <t|ct|draw> <reason>"; a new value shows the banner. MW2's outcome
//! elements are tagged by the script with `CS_OUTCOME_SORT` so the HUD can drop them when a CS
//! banner is picked.

use bevy::prelude::*;
use bevy::text::FontSize;
use bevy::ui::Display;

use crate::ui_write::adopt_display;

/// The sort MW2's round outcome elements get from the CS script patch.
pub(crate) const CS_OUTCOME_SORT: f32 = 4242.0;

/// How long a banner stays up (the round ends 7 s after it is decided).
const BANNER_SECONDS: f32 = 6.5;

const CT_BLUE: Color = Color::srgb(153.0 / 255.0, 204.0 / 255.0, 1.0);
const T_RED: Color = Color::srgb(1.0, 64.0 / 255.0, 64.0 / 255.0);
const DRAW_GREY: Color = Color::srgb(0.85, 0.85, 0.85);

/// Banner looks, as `cl_roundbanner` names them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoundBanner {
    Css,
    Cs16,
    Mw2,
}

impl RoundBanner {
    pub const NAMES: [&'static str; 3] = ["css", "cs16", "mw2"];

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "css" | "source" => Some(Self::Css),
            "cs16" | "1.6" | "16" => Some(Self::Cs16),
            "mw2" => Some(Self::Mw2),
            _ => None,
        }
    }

    fn of(settings: &frame::GameSettings) -> Self {
        Self::parse(&settings.round_banner).unwrap_or(Self::Css)
    }
}

/// Whether MW2's own round outcome shows: only with the `mw2` banner (or outside CS rules).
pub(crate) fn mw2_outcome_shows(settings: &frame::GameSettings) -> bool {
    !crate::cs_hud::replaces_mw2_hud() || RoundBanner::of(settings) == RoundBanner::Mw2
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BannerPart {
    Panel,
    Title,
    Reason,
}

/// The last `cs_round_end` value seen (`None` before the first snapshot) and when its banner
/// went up.
#[derive(Resource, Default)]
pub(crate) struct CsRoundBanner {
    seen: Option<Option<String>>,
    shown: Option<(Side, Reason, f32)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    T,
    Ct,
    Draw,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reason {
    TargetBombed,
    BombDefused,
    CtsEliminated,
    TsEliminated,
    TargetSaved,
    None,
}

fn parse_round_end(value: &str) -> Option<(Side, Reason)> {
    let mut words = value.split_whitespace().skip(1);
    let side = match words.next()? {
        "t" => Side::T,
        "ct" => Side::Ct,
        _ => Side::Draw,
    };
    let reason = match words.next().unwrap_or("") {
        "target_bombed" => Reason::TargetBombed,
        "bomb_defused" => Reason::BombDefused,
        "cts_eliminated" => Reason::CtsEliminated,
        "ts_eliminated" => Reason::TsEliminated,
        "target_saved" => Reason::TargetSaved,
        _ => Reason::None,
    };
    Some((side, reason))
}

/// CS:S's win panel title and reason (`winpanel_*`).
fn css_text(side: Side, reason: Reason) -> (&'static str, &'static str, Color) {
    let (title, color) = match side {
        Side::T => ("Terrorists Win", T_RED),
        Side::Ct => ("Counter-Terrorists Win", CT_BLUE),
        Side::Draw => ("Round Draw", DRAW_GREY),
    };
    let reason = match reason {
        Reason::TargetBombed => "Bomb detonated",
        Reason::BombDefused => "Bomb defused",
        Reason::CtsEliminated => "CTs eliminated",
        Reason::TsEliminated => "Terrorists eliminated",
        Reason::TargetSaved => "Bombing failed",
        Reason::None => "",
    };
    (title, reason, color)
}

/// CS 1.6's centre message (`titles.txt`, retail spelling).
fn cs16_text(side: Side, reason: Reason) -> &'static str {
    match (side, reason) {
        (_, Reason::TargetBombed) => "Target Succesfully Bombed!",
        (_, Reason::BombDefused) => "Bomb Defused!",
        (_, Reason::TargetSaved) => "Target has been saved!",
        (Side::T, _) => "Terrorists Win!",
        (Side::Ct, _) => "Counter-Terrorists Win!",
        (Side::Draw, _) => "Round Draw!",
    }
}

fn banner_text(part: BannerPart, font: &Handle<Font>) -> impl Bundle {
    (
        part,
        Node {
            display: Display::None,
            ..default()
        },
        Text::new(""),
        TextFont {
            font: font.clone().into(),
            font_size: FontSize::Px(16.0),
            ..default()
        },
        TextColor(Color::WHITE),
        TextLayout::new(Justify::Center, LineBreak::NoWrap),
        TextShadow {
            offset: Vec2::splat(1.0),
            color: Color::srgba(0.0, 0.0, 0.0, 0.7),
        },
    )
}

#[derive(Component)]
pub(crate) struct BannerRoot;

/// Builds the banner under the HUD root once it exists, under CS rules: a box (the panel) that
/// centres its title and reason lines.
pub(crate) fn spawn_cs_round_banner(
    mut commands: Commands,
    roots: Query<Entity, With<crate::plugin::HudRoot>>,
    existing: Query<(), With<BannerRoot>>,
    huds: Query<(), With<crate::cs_hud::CsHudRoot>>,
    assets: Res<crate::cs_hud::CsHudAssets>,
) {
    // After the CS HUD, whose spawn reads the fonts this uses.
    if !crate::cs_hud::replaces_mw2_hud() || !existing.is_empty() || huds.is_empty() {
        return;
    }
    let Ok(root) = roots.single() else {
        return;
    };
    let font = assets.name_font();
    commands.entity(root).with_children(|root| {
        root.spawn((
            BannerRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
        ))
        .with_children(|banner| {
            banner
                .spawn((
                    BannerPart::Panel,
                    Node {
                        position_type: PositionType::Absolute,
                        display: Display::None,
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::Center,
                        ..default()
                    },
                    BackgroundColor(Color::NONE),
                ))
                .with_children(|panel| {
                    panel.spawn(banner_text(BannerPart::Title, &font));
                    panel.spawn(banner_text(BannerPart::Reason, &font));
                });
        });
    });
}

/// Shows the banner for a newly decided round, in the picked look.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_cs_round_banner(
    surface: Res<crate::surface::Hud2dSurface>,
    presented: Res<net::PresentedSnapshot>,
    view: Res<frame::ViewSubject>,
    settings: Res<frame::GameSettings>,
    time: Res<Time>,
    mut banner: ResMut<CsRoundBanner>,
    mut lines: Query<(
        &BannerPart,
        &mut Node,
        &mut Text,
        &mut TextFont,
        &mut TextColor,
    )>,
    mut panels: Query<(&mut Node, &mut BackgroundColor), (With<BannerPart>, Without<Text>)>,
) {
    if !crate::cs_hud::replaces_mw2_hud() {
        return;
    }
    let now = time.elapsed_secs();
    if let Some(snap) = presented.snapshot() {
        let value = snap
            .meta
            .objectives
            .server_info
            .iter()
            .find(|(name, _)| name == "cs_round_end")
            .map(|(_, value)| value.clone());
        if banner.seen.as_ref() != Some(&value) {
            // A value already there when we join is an old round: only a change shows.
            if banner.seen.is_some()
                && let Some((side, reason)) = value.as_deref().and_then(parse_round_end)
            {
                banner.shown = Some((side, reason, now));
            }
            banner.seen = Some(value);
        }
    }
    let style = RoundBanner::of(&settings);
    let up = banner
        .shown
        .filter(|&(_, _, since)| now - since < BANNER_SECONDS)
        .filter(|_| style != RoundBanner::Mw2 && surface.is_ready() && !view.in_killcam());
    let Some((side, reason, _)) = up else {
        for (mut node, _) in &mut panels {
            adopt_display(&mut node, Display::None);
        }
        return;
    };

    let unit = surface.height() / crate::presentation_scale::VIRTUAL_HEIGHT;
    let width = surface.width();
    // CS:S's win panel: a dark box low in the middle, the side in its colour and the reason
    // under it. CS 1.6's centre print: one plain line a third of the way down, no box.
    let (title, why, color, title_px, box_rect, background) = match style {
        RoundBanner::Css => {
            let (title, why, color) = css_text(side, reason);
            let (w, h) = (300.0 * unit, if why.is_empty() { 36.0 } else { 54.0 } * unit);
            let rect = ((width - w) * 0.5, 300.0 * unit, w, h);
            (title, why, color, 18.0, rect, Color::srgba(0.0, 0.0, 0.0, 0.78))
        }
        RoundBanner::Cs16 | RoundBanner::Mw2 => {
            let rect = (0.0, 480.0 * 0.35 * unit, width, 24.0 * unit);
            (cs16_text(side, reason), "", Color::WHITE, 14.0, rect, Color::NONE)
        }
    };
    for (mut node, mut fill) in &mut panels {
        let (left, top, w, h) = box_rect;
        place(&mut node, left, top, Some((w, h)));
        let radius = BorderRadius::all(Val::Px((5.0 * unit).round()));
        if node.border_radius != radius {
            node.border_radius = radius;
        }
        if fill.0 != background {
            fill.0 = background;
        }
    }
    for (part, mut node, mut text, mut font, mut tint) in &mut lines {
        let (value, px, want) = match part {
            BannerPart::Title => (title, title_px, color),
            BannerPart::Reason => (why, 11.0, Color::srgb(0.9, 0.9, 0.9)),
            BannerPart::Panel => continue,
        };
        if value.is_empty() {
            adopt_display(&mut node, Display::None);
            continue;
        }
        adopt_display(&mut node, Display::Flex);
        set_text(&mut text, value, &mut font, px * unit, &mut tint, want);
    }
}

fn set_text(
    text: &mut Mut<Text>,
    value: &str,
    font: &mut Mut<TextFont>,
    px: f32,
    tint: &mut Mut<TextColor>,
    want: Color,
) {
    if text.0 != value {
        text.0 = value.to_owned();
    }
    let px = px.round().max(1.0);
    if !matches!(font.font_size, FontSize::Px(v) if v == px) {
        font.font_size = FontSize::Px(px);
    }
    if tint.0 != want {
        tint.0 = want;
    }
}

fn place(node: &mut Mut<Node>, left: f32, top: f32, size: Option<(f32, f32)>) {
    let px = |v: f32| Val::Px(v.round());
    let mut next = Node::clone(node);
    next.left = px(left);
    next.top = px(top);
    if let Some((w, h)) = size {
        next.width = px(w);
        next.height = px(h);
    }
    next.display = Display::Flex;
    if next != **node {
        **node = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_end_values_read_as_the_script_writes_them() {
        assert_eq!(
            parse_round_end("51250 ct bomb_defused"),
            Some((Side::Ct, Reason::BombDefused))
        );
        assert_eq!(parse_round_end("9000 t"), Some((Side::T, Reason::None)));
        assert_eq!(cs16_text(Side::T, Reason::TargetBombed), "Target Succesfully Bombed!");
        assert_eq!(cs16_text(Side::Ct, Reason::TsEliminated), "Counter-Terrorists Win!");
        assert_eq!(RoundBanner::parse("CS16"), Some(RoundBanner::Cs16));
    }
}
