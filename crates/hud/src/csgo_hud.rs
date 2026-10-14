//! CS:GO's HUD for CS rules, when CS:GO is installed: health and armour with their bars at the
//! bottom left, the magazine and reserve at the bottom right, the round timer between the two
//! sides' scores at the top, the money (with the buy cart) under the minimap and the kill feed at
//! the top right, laid out as CS:GO's Panorama HUD lays them out (1080 pixel units scaled by the
//! screen height). Drawn in CS:GO's Stratum2 font with its own HUD icons and kill icons, read from
//! the install at runtime. It stands in for the CS:S and CS 1.6 HUDs, sharing their kill feed.

use std::collections::HashMap;
use std::path::Path;

use assets::PreparedWeapons;
use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use bevy::text::FontSize;
use bevy::ui::Display;
use frame::ViewSubject;
use net::{LocalPresentClient, PresentedSnapshot};

use crate::cs_hud::CsKillFeed;
use crate::scorebar::milliseconds;
use crate::ui_write::adopt_display;

/// The HUD's layout height: positions and sizes below are 1080p pixels.
const VIRTUAL_HEIGHT: f32 = 1080.0;

const WHITE: Color = Color::srgb(0.93, 0.93, 0.91);
const LOW_RED: Color = Color::srgb(0.9, 0.2, 0.15);
const PANEL: Color = Color::srgba(0.0, 0.0, 0.0, 0.45);
const BAR_BACK: Color = Color::srgba(1.0, 1.0, 1.0, 0.15);
const CT_BLUE: Color = Color::srgb(0.45, 0.68, 0.95);
const T_GOLD: Color = Color::srgb(0.92, 0.76, 0.33);
const MONEY_GREEN: Color = Color::srgb(0.5, 0.85, 0.35);
const MONEY_LOSS: Color = Color::srgb(0.85, 0.25, 0.2);
/// A kill feed line the local player is in: CS:GO outlines those in red.
const FEED_OWN: Color = Color::srgba(0.45, 0.05, 0.05, 0.65);
const FEED_BG: Color = Color::srgba(0.0, 0.0, 0.0, 0.55);

/// Health at or under which the number turns red; seconds under which the timer does.
const LOW_HEALTH: i32 = 20;
const LOW_TIME_S: i32 = 10;
const MONEY_FLASH_MS: i32 = 3000;

/// Bottom-left health and armour panel, bottom-right ammo panel.
const BOTTOM_PANEL: [f32; 2] = [430.0, 70.0];
const AMMO_PANEL: [f32; 2] = [330.0, 70.0];
const ICON: f32 = 34.0;
const NUMBER: f32 = 44.0;
const SMALL: f32 = 26.0;
const BAR: [f32; 2] = [92.0, 4.0];
/// Top centre: timer box and the score boxes beside it.
const TIMER_BOX: [f32; 2] = [120.0, 46.0];
const SCORE_BOX: [f32; 2] = [64.0, 46.0];
/// Money under the minimap.
const MONEY_TOP: f32 = 268.0;
const MONEY_LEFT: f32 = 22.0;
/// Kill feed lines.
const FEED_TOP: f32 = 64.0;
const FEED_RIGHT: f32 = 22.0;
const FEED_LINE: f32 = 40.0;
const FEED_NAME: f32 = 21.0;
const FEED_ICON: f32 = 26.0;
/// Plant or defuse bar.
const PROGRESS: [f32; 2] = [320.0, 10.0];

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CsgoPart {
    HealthPanel,
    HealthIcon,
    Health,
    HealthBarBack,
    HealthBar,
    ArmorIcon,
    Armor,
    ArmorBarBack,
    ArmorBar,
    AmmoPanel,
    Clip,
    Reserve,
    TimerBox,
    Timer,
    ScoreBox(Side),
    Score(Side),
    MoneyIcon,
    Money,
    MoneyDelta,
    BombIcon,
    ProgressBack,
    ProgressFill,
    FeedRow(u8),
    FeedAttacker(u8),
    FeedWeapon(u8),
    FeedHeadshot(u8),
    FeedVictim(u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Side {
    Ct,
    T,
}

/// The fonts and icons, read once from the CS:GO install.
#[derive(Resource, Default)]
pub(crate) struct CsgoHudAssets {
    pub(crate) active: bool,
    regular: Handle<Font>,
    bold: Handle<Font>,
    /// HUD icons by name (`health`, `armor`, `helmet`, `cart`, `c4`, `defuser`).
    icons: HashMap<&'static str, Handle<Image>>,
    /// Kill icons by CS:GO weapon (`ak47`, `knife_default_ct`, `headshot`, `suicide` …).
    kills: HashMap<&'static str, Handle<Image>>,
}

/// The money flash and the plant/defuse bar's start, between frames.
#[derive(Default)]
pub(crate) struct CsgoHudState {
    money: Option<i32>,
    delta: i32,
    since_ms: i32,
    progress: Option<(f32, f32)>,
}

/// The CS:GO kill icons the feed can show (`deathnotice/icon-<name>.png`).
const KILL_ICONS: [&str; 41] = [
    "ak47",
    "m4a1",
    "m4a1_silencer",
    "m4a1_silencer_off",
    "awp",
    "deagle",
    "usp_silencer",
    "usp_silencer_off",
    "glock",
    "p250",
    "fiveseven",
    "elite",
    "nova",
    "xm1014",
    "mac10",
    "mp9",
    "mp5sd",
    "ump45",
    "p90",
    "galilar",
    "famas",
    "sg556",
    "aug",
    "ssg08",
    "g3sg1",
    "scar20",
    "m249",
    "mp7",
    "bizon",
    "mag7",
    "sawedoff",
    "negev",
    "hkp2000",
    "tec9",
    "cz75a",
    "revolver",
    "knife_default_ct",
    "hegrenade",
    "headshot",
    "suicide",
    "knife",
];

/// The CS:GO kill icon of the weapon a kill was made with (an MW2 script name a CS weapon
/// wears); `suicide` when nothing killed.
pub(crate) fn kill_icon(weapon: Option<&str>) -> &'static str {
    use weapon_iw4::cs;
    let Some(index) = weapon.and_then(cs::cs_weapon_index_for) else {
        return "suicide";
    };
    if cs::is_knife(index) {
        return "knife_default_ct";
    }
    if cs::is_grenade(index) {
        return "hegrenade";
    }
    match cs::cs_weapon(index).map(|w| w.name) {
        Some("m4a1") => "m4a1_silencer",
        Some("usp") => "usp_silencer",
        Some("p228") => "p250",
        Some("m3") => "nova",
        Some("tmp") => "mp9",
        Some("mp5") => "mp5sd",
        Some("galil") => "galilar",
        Some("sg552") => "sg556",
        Some("scout") => "ssg08",
        Some("sg550") => "scar20",
        Some("m4a4") => "m4a1",
        Some("p2000") => "hkp2000",
        Some(name) => KILL_ICONS
            .iter()
            .find(|icon| **icon == name)
            .copied()
            .unwrap_or("suicide"),
        None => "suicide",
    }
}

/// CS:GO's `.vfont` (`VFONT1`): an sfnt font XORed with a key rolled from a salt stored at its
/// end. Decoded in memory only. Follows CS-Craft (github.com/FrosttysBots/CS-Craft, Apache-2.0).
fn decode_vfont(mut bytes: Vec<u8>) -> Option<Vec<u8>> {
    let trailer = bytes.len().checked_sub(7)?;
    if &bytes[trailer + 1..] != b"VFONT1" {
        return None;
    }
    let salt_len = usize::from(bytes[trailer]);
    if salt_len == 0 {
        return None;
    }
    let end = (trailer + 1).checked_sub(salt_len)?;
    let mut key = 0xa7u8;
    for &salt in &bytes[end..trailer] {
        key ^= salt.wrapping_add(0xa7);
    }
    for b in &mut bytes[..end] {
        let encoded = *b;
        *b ^= key;
        key = encoded.wrapping_add(0xa7);
    }
    bytes.truncate(end);
    (bytes.starts_with(&[0, 1, 0, 0]) || bytes.starts_with(b"OTTO")).then_some(bytes)
}

fn png(vpk: &mdl_source::Vpk, path: &str, images: &mut Assets<Image>) -> Option<Handle<Image>> {
    let bytes = vpk.read(path)?;
    let image = Image::from_buffer(
        &bytes,
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::linear(),
        RenderAssetUsages::default(),
    )
    .ok()?;
    Some(images.add(image))
}

/// Reads CS:GO's HUD fonts and icons; `None` without CS:GO (or without its fonts), and the CS:S
/// or CS 1.6 HUD draws instead.
pub(crate) fn load(fonts: &mut Assets<Font>, images: &mut Assets<Image>) -> Option<CsgoHudAssets> {
    let pak = asset_transport::find_csgo_pak()?;
    let dir = pak.parent()?;
    let font = |name: &str| {
        std::fs::read(dir.join("panorama").join("fonts").join(format!("{name}.vfont")))
            .ok()
            .and_then(decode_vfont)
    };
    let (regular, bold) = (font("stratum2regular")?, font("stratum2bold")?);
    let vpk = mdl_source::Vpk::open(&pak).ok()?;
    let hud = |path: &str, images: &mut Assets<Image>| {
        png(
            &vpk,
            &format!("materials/panorama/images/hud/{path}.png"),
            images,
        )
    };
    let mut icons = HashMap::new();
    for (name, path) in [
        ("health", "healtharmor/icon-cross1"),
        ("armor", "healtharmor/icon-shield"),
        ("helmet", "healtharmor/icon-armor-helmet"),
        ("cart", "money/icon-cart"),
        ("c4", "weaponpanel/icon-c4"),
        ("defuser", "weaponpanel/icon-defuser"),
    ] {
        if let Some(image) = hud(path, images) {
            icons.insert(name, image);
        }
    }
    let mut kills = HashMap::new();
    for name in KILL_ICONS {
        if let Some(image) = hud(&format!("deathnotice/icon-{name}"), images) {
            kills.insert(name, image);
        }
    }
    diag::info!(
        World,
        "csgo hud: Stratum2, {} icons, {} kill icons from {}",
        icons.len(),
        kills.len(),
        Path::new(&pak).display()
    );
    Some(CsgoHudAssets {
        active: true,
        regular: fonts.add(Font::from_bytes(regular)),
        bold: fonts.add(Font::from_bytes(bold)),
        icons,
        kills,
    })
}

fn hidden() -> Node {
    Node {
        position_type: PositionType::Absolute,
        display: Display::None,
        ..default()
    }
}

fn text(part: CsgoPart, font: &Handle<Font>, justify: Justify) -> impl Bundle {
    (
        part,
        hidden(),
        Text::new(""),
        TextFont {
            font: font.clone().into(),
            font_size: FontSize::Px(NUMBER),
            ..default()
        },
        TextColor(WHITE),
        TextLayout::new(justify, LineBreak::NoWrap),
        TextShadow {
            offset: Vec2::splat(1.5),
            color: Color::srgba(0.0, 0.0, 0.0, 0.7),
        },
    )
}

fn fill(part: CsgoPart, color: Color) -> impl Bundle {
    (part, hidden(), BackgroundColor(color))
}

fn icon(part: CsgoPart) -> impl Bundle {
    (part, hidden(), ImageNode::default())
}

/// Spawns the HUD's parts under the CS HUD root.
pub(crate) fn spawn_parts(hud: &mut ChildSpawnerCommands, assets: &CsgoHudAssets) {
    let (regular, bold) = (&assets.regular, &assets.bold);
    hud.spawn(fill(CsgoPart::HealthPanel, PANEL));
    hud.spawn(fill(CsgoPart::AmmoPanel, PANEL));
    // The timer and the scores sit centred in their boxes.
    let boxed = |part: CsgoPart, color: Color| {
        (
            part,
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(color),
        )
    };
    hud.spawn(boxed(CsgoPart::TimerBox, PANEL.with_alpha(0.65)))
        .with_children(|timer| {
        timer
            .spawn(text(CsgoPart::Timer, bold, Justify::Center))
            .insert(Node::default());
    });
    for side in [Side::Ct, Side::T] {
        let color = match side {
            Side::Ct => CT_BLUE.with_alpha(0.8),
            Side::T => T_GOLD.with_alpha(0.8),
        };
        hud.spawn(boxed(CsgoPart::ScoreBox(side), color))
            .with_children(|score| {
                score
                    .spawn(text(CsgoPart::Score(side), bold, Justify::Center))
                    .insert(Node::default());
            });
    }
    for part in [CsgoPart::HealthBarBack, CsgoPart::ArmorBarBack, CsgoPart::ProgressBack] {
        hud.spawn(fill(part, BAR_BACK));
    }
    for part in [CsgoPart::HealthBar, CsgoPart::ArmorBar, CsgoPart::ProgressFill] {
        hud.spawn(fill(part, WHITE));
    }
    for part in [
        CsgoPart::HealthIcon,
        CsgoPart::ArmorIcon,
        CsgoPart::MoneyIcon,
        CsgoPart::BombIcon,
    ] {
        hud.spawn(icon(part));
    }
    for part in [CsgoPart::Health, CsgoPart::Armor, CsgoPart::Clip, CsgoPart::Money] {
        hud.spawn(text(part, bold, Justify::Left));
    }
    hud.spawn(text(CsgoPart::Reserve, regular, Justify::Left));
    hud.spawn(text(CsgoPart::MoneyDelta, regular, Justify::Left));
    for row in 0..crate::cs_hud::FEED_LINES as u8 {
        hud.spawn((
            CsgoPart::FeedRow(row),
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(8.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(FEED_BG),
        ))
        .with_children(|line| {
            line.spawn(text(CsgoPart::FeedAttacker(row), bold, Justify::Left))
                .insert(Node::default());
            line.spawn((CsgoPart::FeedWeapon(row), Node::default(), ImageNode::default()));
            line.spawn((CsgoPart::FeedHeadshot(row), Node::default(), ImageNode::default()));
            line.spawn(text(CsgoPart::FeedVictim(row), bold, Justify::Left))
                .insert(Node::default());
        });
    }
}

type PartQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static CsgoPart,
        &'static mut Node,
        Option<&'static mut Text>,
        Option<&'static mut TextFont>,
        Option<&'static mut TextColor>,
        Option<&'static mut ImageNode>,
        Option<&'static mut BackgroundColor>,
    ),
>;

/// An absolutely placed rectangle in screen pixels: `left`/`right`, `top`/`bottom` (one of each
/// pair), and the size when given.
#[derive(Clone, Copy)]
struct At {
    left: Option<f32>,
    right: Option<f32>,
    top: Option<f32>,
    bottom: Option<f32>,
    size: Option<[f32; 2]>,
}

impl At {
    fn left_bottom(left: f32, bottom: f32) -> Self {
        Self {
            left: Some(left),
            right: None,
            top: None,
            bottom: Some(bottom),
            size: None,
        }
    }
    fn right_bottom(right: f32, bottom: f32) -> Self {
        Self {
            right: Some(right),
            left: None,
            ..Self::left_bottom(0.0, bottom)
        }
    }
    fn left_top(left: f32, top: f32) -> Self {
        Self {
            left: Some(left),
            right: None,
            top: Some(top),
            bottom: None,
            size: None,
        }
    }
    fn right_top(right: f32, top: f32) -> Self {
        Self {
            right: Some(right),
            left: None,
            ..Self::left_top(0.0, top)
        }
    }
    fn sized(self, w: f32, h: f32) -> Self {
        Self {
            size: Some([w, h]),
            ..self
        }
    }
}

/// Shows `node` at `at` (scaled by `scale`), touching it only when that changes it.
fn show(node: &mut Mut<Node>, at: At, scale: f32) {
    let px = |v: Option<f32>| v.map_or(Val::Auto, |v| Val::Px((v * scale).round()));
    let mut next = Node::clone(node);
    next.position_type = PositionType::Absolute;
    next.display = Display::Flex;
    next.left = px(at.left);
    next.right = px(at.right);
    next.top = px(at.top);
    next.bottom = px(at.bottom);
    if let Some([w, h]) = at.size {
        next.width = Val::Px((w * scale).round().max(1.0));
        next.height = Val::Px((h * scale).round().max(1.0));
    }
    if **node != next {
        **node = next;
    }
}

fn set_text(text: Option<Mut<Text>>, value: &str) {
    if let Some(mut text) = text
        && text.0 != value
    {
        value.clone_into(&mut text.0);
    }
}

fn set_font(font: Option<Mut<TextFont>>, size: f32, scale: f32) {
    if let Some(mut font) = font {
        let px = (size * scale).round().max(1.0);
        if !matches!(font.font_size, FontSize::Px(v) if v == px) {
            font.font_size = FontSize::Px(px);
        }
    }
}

fn set_color(color: Option<Mut<TextColor>>, want: Color) {
    if let Some(mut color) = color
        && color.0 != want
    {
        color.0 = want;
    }
}

fn set_image(image: Option<Mut<ImageNode>>, handle: Option<&Handle<Image>>) -> bool {
    match (image, handle) {
        (Some(mut image), Some(handle)) => {
            if image.image != *handle {
                image.image = handle.clone();
            }
            true
        }
        _ => false,
    }
}

/// A kill feed name's colour in CS:GO's team colours (the feed keeps CS:S's).
fn feed_color(color: Color) -> Color {
    if color == crate::cs_hud::T_RED {
        T_GOLD
    } else if color == crate::cs_hud::CT_BLUE {
        CT_BLUE
    } else {
        color
    }
}

/// The two sides' rounds won: the Counter-Terrorists' and the Terrorists'.
pub(crate) fn scores(snap: Option<&sim::Snapshot>) -> Option<[i32; 2]> {
    let snap = snap?;
    let scores = snap.meta.objectives.scores;
    let terrorists = match snap
        .meta
        .objectives
        .server_info
        .iter()
        .find(|(name, _)| name == "cs_attackers")
        .map(|(_, value)| value.as_str())
    {
        Some("allies") => entity_iw4::TEAM_ALLIES,
        _ => entity_iw4::TEAM_AXIS,
    };
    let cts = if terrorists == entity_iw4::TEAM_AXIS {
        entity_iw4::TEAM_ALLIES
    } else {
        entity_iw4::TEAM_AXIS
    };
    Some([scores[cts as usize], scores[terrorists as usize]])
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_csgo_hud(
    surface: Res<crate::surface::Hud2dSurface>,
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    weapons: Option<Res<PreparedWeapons>>,
    view: Res<ViewSubject>,
    assets: Res<CsgoHudAssets>,
    feed: Res<CsKillFeed>,
    time: Res<Time>,
    hud_input: Option<Res<frame::HudInputView>>,
    mut state: Local<CsgoHudState>,
    mut parts: PartQuery,
) {
    if !assets.active || !crate::cs_hud::replaces_mw2_hud() {
        return;
    }
    // Put away under the pause menu, as CS:GO's.
    let menu = hud_input.is_some_and(|input| input.script_menu_open);
    let ps = presented.player(local.0);
    let Some(ps) = ps.filter(|_| surface.is_ready() && !view.in_killcam() && !menu) else {
        for (_, mut node, ..) in &mut parts {
            adopt_display(&mut node, Display::None);
        }
        return;
    };
    let scale = surface.height() / VIRTUAL_HEIGHT;
    let snap = presented.snapshot();
    let now_ms = milliseconds() as i32;
    let alive = ps.pm_type < playerstate_iw4::PM_TYPE_DEAD;
    let health = alive.then(|| ps.health.max(0));
    let armor = alive.then(|| (ps.cs_armor as i32, ps.cs_helmet != 0));
    let money = (alive && crate::cs_hud::bomb_mode(snap)).then_some(ps.cs_money as i32);
    let ammo = weapons
        .as_deref()
        .filter(|_| alive)
        .and_then(|w| crate::cs_hud::ammo_counts(ps, w, &presented, &local));
    let bomb = if weapons
        .as_deref()
        .is_some_and(|w| alive && crate::cs_hud::carries_c4(ps, w))
    {
        Some("c4")
    } else {
        (alive && ps.cs_defuser != 0).then_some("defuser")
    };
    let timer = crate::cs_hud::round_seconds_left(snap);
    let scores = scores(snap);
    if let Some(money) = money {
        if let Some(last) = state.money
            && last != money
        {
            state.delta = money - last;
            state.since_ms = now_ms;
        }
        state.money = Some(money);
    }
    let delta_left = 1.0 - now_ms.saturating_sub(state.since_ms) as f32 / MONEY_FLASH_MS as f32;
    let now_s = time.elapsed_secs();
    if ps.pm_type != playerstate_iw4::PM_TYPE_NORMAL_LINKED {
        state.progress = None;
    } else if state.progress.is_none() {
        state.progress = Some((now_s, crate::cs_hud::progress_seconds(ps, weapons.as_deref())));
    }
    let progress = state
        .progress
        .map(|(since, seconds)| ((now_s - since) / seconds).clamp(0.0, 1.0));
    let screen_w = surface.width() / scale;

    for (part, mut node, text, font, color, image, background) in &mut parts {
        let mut hide = false;
        match *part {
            CsgoPart::HealthPanel => match health {
                Some(_) => show(
                    &mut node,
                    At::left_bottom(0.0, 0.0).sized(BOTTOM_PANEL[0], BOTTOM_PANEL[1]),
                    scale,
                ),
                None => hide = true,
            },
            CsgoPart::HealthIcon | CsgoPart::ArmorIcon => {
                let health_icon = *part == CsgoPart::HealthIcon;
                let name = if health_icon {
                    health.map(|_| "health")
                } else {
                    armor.map(|(_, helmet)| if helmet { "helmet" } else { "armor" })
                };
                if set_image(image, name.and_then(|n| assets.icons.get(n))) {
                    let left = if health_icon { 18.0 } else { 222.0 };
                    show(
                        &mut node,
                        At::left_bottom(left, (BOTTOM_PANEL[1] - ICON) * 0.5).sized(ICON, ICON),
                        scale,
                    );
                } else {
                    hide = true;
                }
            }
            CsgoPart::Health | CsgoPart::Armor => {
                let (value, left, low) = if *part == CsgoPart::Health {
                    (health, 62.0, health.is_some_and(|h| h <= LOW_HEALTH))
                } else {
                    (armor.map(|(points, _)| points), 266.0, false)
                };
                match value {
                    Some(value) => {
                        set_text(text, &value.to_string());
                        set_font(font, NUMBER, scale);
                        set_color(color, if low { LOW_RED } else { WHITE });
                        show(&mut node, At::left_bottom(left, 14.0), scale);
                    }
                    None => hide = true,
                }
            }
            CsgoPart::HealthBarBack | CsgoPart::ArmorBarBack => {
                let (shown, left) = if *part == CsgoPart::HealthBarBack {
                    (health.is_some(), 62.0)
                } else {
                    (armor.is_some(), 266.0)
                };
                if shown {
                    show(
                        &mut node,
                        At::left_bottom(left, 10.0).sized(BAR[0], BAR[1]),
                        scale,
                    );
                } else {
                    hide = true;
                }
            }
            CsgoPart::HealthBar | CsgoPart::ArmorBar => {
                let (value, left, low) = if *part == CsgoPart::HealthBar {
                    (health, 62.0, health.is_some_and(|h| h <= LOW_HEALTH))
                } else {
                    (armor.map(|(points, _)| points), 266.0, false)
                };
                match value.filter(|v| *v > 0) {
                    Some(value) => {
                        let width = BAR[0] * (value.min(100) as f32 / 100.0);
                        show(
                            &mut node,
                            At::left_bottom(left, 10.0).sized(width, BAR[1]),
                            scale,
                        );
                        if let Some(mut background) = background {
                            let want = if low { LOW_RED } else { WHITE };
                            if background.0 != want {
                                background.0 = want;
                            }
                        }
                    }
                    None => hide = true,
                }
            }
            CsgoPart::AmmoPanel => match ammo {
                Some(_) => show(
                    &mut node,
                    At::right_bottom(0.0, 0.0).sized(AMMO_PANEL[0], AMMO_PANEL[1]),
                    scale,
                ),
                None => hide = true,
            },
            CsgoPart::Clip => match ammo {
                Some((clip, _)) => {
                    set_text(text, &clip.to_string());
                    set_font(font, NUMBER, scale);
                    let low = clip == 0;
                    set_color(color, if low { LOW_RED } else { WHITE });
                    show(&mut node, At::right_bottom(150.0, 14.0), scale);
                }
                None => hide = true,
            },
            CsgoPart::Reserve => match ammo.and_then(|(_, reserve)| reserve) {
                Some(reserve) => {
                    set_text(text, &format!("/ {reserve}"));
                    set_font(font, SMALL, scale);
                    set_color(color, WHITE.with_alpha(0.8));
                    show(&mut node, At::right_bottom(40.0, 18.0), scale);
                }
                None => hide = true,
            },
            CsgoPart::TimerBox | CsgoPart::Timer => match timer {
                Some(seconds) => {
                    let left = screen_w * 0.5 - TIMER_BOX[0] * 0.5;
                    if *part == CsgoPart::TimerBox {
                        show(
                            &mut node,
                            At::left_top(left, 0.0).sized(TIMER_BOX[0], TIMER_BOX[1]),
                            scale,
                        );
                    } else {
                        set_text(text, &format!("{}:{:02}", seconds / 60, seconds % 60));
                        set_font(font, 34.0, scale);
                        set_color(color, if seconds < LOW_TIME_S { LOW_RED } else { WHITE });
                        adopt_display(&mut node, Display::Flex);
                    }
                }
                None => hide = true,
            },
            CsgoPart::ScoreBox(side) | CsgoPart::Score(side) => match scores {
                Some([ct, t]) => {
                    let left = match side {
                        Side::Ct => screen_w * 0.5 - TIMER_BOX[0] * 0.5 - SCORE_BOX[0],
                        Side::T => screen_w * 0.5 + TIMER_BOX[0] * 0.5,
                    };
                    if matches!(*part, CsgoPart::ScoreBox(_)) {
                        show(
                            &mut node,
                            At::left_top(left, 0.0).sized(SCORE_BOX[0], SCORE_BOX[1]),
                            scale,
                        );
                    } else {
                        let value = if side == Side::Ct { ct } else { t };
                        set_text(text, &value.to_string());
                        set_font(font, 34.0, scale);
                        adopt_display(&mut node, Display::Flex);
                    }
                }
                None => hide = true,
            },
            CsgoPart::MoneyIcon => {
                if money.is_some() && set_image(image, assets.icons.get("cart")) {
                    show(
                        &mut node,
                        At::left_top(MONEY_LEFT, MONEY_TOP + 6.0).sized(30.0, 30.0),
                        scale,
                    );
                } else {
                    hide = true;
                }
            }
            CsgoPart::Money => match money {
                Some(money) => {
                    set_text(text, &format!("${money}"));
                    set_font(font, 36.0, scale);
                    set_color(color, MONEY_GREEN);
                    show(&mut node, At::left_top(MONEY_LEFT + 40.0, MONEY_TOP), scale);
                }
                None => hide = true,
            },
            CsgoPart::MoneyDelta => match money {
                Some(_) if delta_left > 0.0 && state.delta != 0 => {
                    let (sign, tint) = if state.delta > 0 {
                        ("+", MONEY_GREEN)
                    } else {
                        ("-", MONEY_LOSS)
                    };
                    set_text(text, &format!("{sign}${}", state.delta.abs()));
                    set_font(font, 24.0, scale);
                    set_color(color, tint.with_alpha(delta_left));
                    show(
                        &mut node,
                        At::left_top(MONEY_LEFT + 40.0, MONEY_TOP + 42.0),
                        scale,
                    );
                }
                _ => hide = true,
            },
            CsgoPart::BombIcon => {
                if set_image(image, bomb.and_then(|b| assets.icons.get(b))) {
                    let top = MONEY_TOP + if money.is_some() { 76.0 } else { 0.0 };
                    show(
                        &mut node,
                        At::left_top(MONEY_LEFT, top).sized(36.0, 36.0),
                        scale,
                    );
                } else {
                    hide = true;
                }
            }
            CsgoPart::ProgressBack | CsgoPart::ProgressFill => match progress {
                Some(done) => {
                    let width = if *part == CsgoPart::ProgressBack {
                        PROGRESS[0]
                    } else {
                        PROGRESS[0] * done
                    };
                    show(
                        &mut node,
                        At::left_top(screen_w * 0.5 - PROGRESS[0] * 0.5, VIRTUAL_HEIGHT * 0.62)
                            .sized(width.max(1.0), PROGRESS[1]),
                        scale,
                    );
                }
                None => hide = true,
            },
            CsgoPart::FeedRow(row) => match feed.lines.get(usize::from(row)) {
                Some(line) => {
                    show(
                        &mut node,
                        At::right_top(FEED_RIGHT, FEED_TOP + f32::from(row) * FEED_LINE),
                        scale,
                    );
                    if let Some(mut background) = background {
                        let own = line.local;
                        let want = if own { FEED_OWN } else { FEED_BG };
                        if background.0 != want {
                            background.0 = want;
                        }
                    }
                }
                None => hide = true,
            },
            CsgoPart::FeedAttacker(row) | CsgoPart::FeedVictim(row) => {
                let line = feed.lines.get(usize::from(row));
                let name = match *part {
                    CsgoPart::FeedAttacker(_) => line.and_then(|l| l.attacker.as_ref()),
                    _ => line.map(|l| &l.victim),
                };
                match name {
                    Some((name, team)) => {
                        set_text(text, name);
                        set_font(font, FEED_NAME, scale);
                        set_color(color, feed_color(*team));
                        adopt_display(&mut node, Display::Flex);
                    }
                    None => hide = true,
                }
            }
            CsgoPart::FeedWeapon(row) | CsgoPart::FeedHeadshot(row) => {
                let line = feed.lines.get(usize::from(row));
                let name = match *part {
                    CsgoPart::FeedWeapon(_) => line.map(|l| l.csgo_icon),
                    _ => line.filter(|l| l.headshot).map(|_| "headshot"),
                };
                if set_image(image, name.and_then(|n| assets.kills.get(n))) {
                    let height = Val::Px((FEED_ICON * scale).round());
                    let mut next = Node::clone(&node);
                    next.display = Display::Flex;
                    next.height = height;
                    next.width = Val::Auto;
                    if *node != next {
                        *node = next;
                    }
                } else {
                    hide = true;
                }
            }
        }
        if hide {
            adopt_display(&mut node, Display::None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_salted_font_and_rejects_bad_trailers() {
        let data = b"\0\x01\0\0testfont";
        let salts = [5u8, 27];
        let mut key = salts.iter().fold(0xa7u8, |key, s| key ^ s.wrapping_add(0xa7));
        let mut encoded = Vec::new();
        for &b in data {
            let value = b ^ key;
            encoded.push(value);
            key = value.wrapping_add(0xa7);
        }
        encoded.extend(salts);
        // The salt length counts its own byte.
        encoded.push(salts.len() as u8 + 1);
        encoded.extend(b"VFONT1");
        assert_eq!(decode_vfont(encoded.clone()).as_deref(), Some(data.as_slice()));
        for n in 0..encoded.len() {
            assert!(decode_vfont(encoded[..n].to_vec()).is_none());
        }
    }
}
