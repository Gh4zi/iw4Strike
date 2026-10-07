//! Counter-Strike: Source HUD for CS rules: the health, armour, round timer and ammo panels and the
//! kill feed, placed as CS:S's `hudlayout.res` places them (640x480 units scaled by the screen
//! height, `r`/`c` positions from the right edge and the centre) and drawn in its fonts —
//! `cstrike.ttf` digits and HUD icons, `cs.ttf` grenade icons, `csd.ttf` kill icons — read from
//! the CS:S install at runtime, with the ammo icons cut from its `sprites/640hud1` sheet. While
//! it is up the MW2 weapon bar, score bar, splashes, player cards and kill feed stand down; the
//! MW2 minimap stays.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use assets::PreparedWeapons;
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::text::FontSize;
use bevy::ui::Display;
use frame::ViewSubject;
use net::{LocalPresentClient, PresentedSnapshot};
use playerstate_iw4::PlayerState;

use crate::scorebar::milliseconds;
use crate::ui_write::adopt_display;

/// Whether the CS HUD stands in for the MW2 one: always under CS rules.
pub(crate) fn replaces_mw2_hud() -> bool {
    movement_iw4::rules::CS_RULES
}

const ORANGE: Color = Color::srgb(1.0, 176.0 / 255.0, 0.0);
/// `LowHealthColor` / the timer's `FlashColor`.
const RED: Color = Color::srgb(0.86, 0.0, 0.0);
/// Panel backgrounds (`bgcolor_override 0 0 0 96`).
const PANEL_BG: Color = Color::srgba(0.0, 0.0, 0.0, 96.0 / 255.0);
const CT_BLUE: Color = Color::srgb(153.0 / 255.0, 204.0 / 255.0, 1.0);
const T_RED: Color = Color::srgb(1.0, 64.0 / 255.0, 64.0 / 255.0);
/// Health at or under which the digits turn red.
const LOW_HEALTH: i32 = 25;
/// Seconds left under which the round timer turns red.
const LOW_TIME_S: i32 = 10;

/// `HudNumbers` / `Icons` (`cstrike.ttf`, tall 28).
const NUMBER_TALL: f32 = 28.0;
/// The proportional `Default` font the kill feed names use (Verdana bold, tall 9).
const NAME_TALL: f32 = 9.0;
/// Kill icons, sized to sit in a feed line.
const DEATH_TALL: f32 = 24.0;
/// `csd.ttf` draws its icons high in the em box; this much of the font size brings them level
/// with the names.
const DEATH_DROP: f32 = 0.44;
/// Grenade icons in the ammo panel.
const GRENADE_TALL: f32 = 24.0;
/// `HudDeathNotice`: `MaxDeathNotices`, `LineHeight`, top `ypos`, right margin (640 - `wide` 628).
const FEED_LINES: usize = 4;
const FEED_LINE_HEIGHT: f32 = 22.0;
const FEED_TOP: f32 = 12.0;
const FEED_RIGHT: f32 = 12.0;
/// `hud_deathnotice_time`.
const FEED_SECONDS_MS: i32 = 6000;
/// Rounded panel corners (`PaintBackgroundType 2`).
const CORNER: f32 = 5.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Panel {
    Health,
    Armor,
    Timer,
    Ammo,
}

const PANELS: [Panel; 4] = [Panel::Health, Panel::Armor, Panel::Timer, Panel::Ammo];

impl Panel {
    /// `xpos`, `ypos`, `wide`, `tall` in 640x480 units; `x` already resolved against the screen
    /// width `w` in those units.
    fn rect(self, w: f32) -> [f32; 4] {
        match self {
            Self::Health => [8.0, 446.0, 80.0, 25.0],
            Self::Armor => [148.0, 446.0, 80.0, 25.0],
            Self::Timer => [w * 0.5 - 28.0, 446.0, 98.0, 25.0],
            Self::Ammo => [w - 157.0, 446.0, 142.0, 25.0],
        }
    }

    /// `icon_xpos`/`icon_ypos`.
    fn icon(self) -> [f32; 2] {
        [8.0, -4.0]
    }

    /// `digit_xpos`/`digit_ypos`.
    fn digits(self) -> [f32; 2] {
        match self {
            Self::Health => [35.0, -4.0],
            Self::Armor => [34.0, -4.0],
            Self::Timer => [42.0, -4.0],
            Self::Ammo => [8.0, -4.0],
        }
    }
}

/// HudAmmo: the bar between clip and reserve, the reserve digits and the ammo icon.
const AMMO_BAR: [f32; 4] = [53.0, 3.0, 2.0, 19.0];
const AMMO_RESERVE: [f32; 2] = [63.0, -4.0];
const AMMO_ICON: [f32; 2] = [110.0, 2.0];

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CsHudPart {
    Panel(Panel),
    Icon(Panel),
    Digits(Panel),
    AmmoBar,
    AmmoReserve,
    AmmoSprite,
    AmmoGlyph,
    FeedRow(u8),
    FeedAttacker(u8),
    FeedWeapon(u8),
    FeedHeadshot(u8),
    FeedVictim(u8),
}

#[derive(Component)]
pub(crate) struct CsHudRoot;

/// The CS:S fonts and ammo sheet, read once.
#[derive(Resource, Default)]
pub(crate) struct CsHudAssets {
    /// `cstrike.ttf`: digits and the health/armour/timer icons.
    numbers: Option<Handle<Font>>,
    /// `cs.ttf`: grenade icons.
    types: Option<Handle<Font>>,
    /// `csd.ttf`: kill icons.
    death: Option<Handle<Font>>,
    /// Kill feed names (Verdana bold, like CS:S's `Default`), and the fallback for the rest.
    names: Handle<Font>,
    /// `sprites/640hud1`: ammo icons.
    sprites: Option<Handle<Image>>,
}

/// Which font a kill icon is drawn in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum IconFont {
    Death,
    Types,
}

#[derive(Clone, Debug)]
struct FeedLine {
    start_ms: i32,
    attacker: Option<(String, Color)>,
    victim: (String, Color),
    icon: (IconFont, char),
    headshot: bool,
}

/// CS:S death notices: the newest last, at most [`FEED_LINES`].
#[derive(Resource, Default)]
pub(crate) struct CsKillFeed {
    lines: VecDeque<FeedLine>,
}

/// Fallback when Windows has no Verdana/Tahoma.
const EMBEDDED_FONT: &[u8] = include_bytes!("../../console/assets/FreeMono.otf");

fn read_font(fonts: &mut Assets<Font>, path: &Path) -> Option<Handle<Font>> {
    let bytes = std::fs::read(path).ok()?;
    Some(fonts.add(Font::from_bytes(bytes)))
}

fn system_font(fonts: &mut Assets<Font>) -> Handle<Font> {
    let dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:/Windows"))
        .join("Fonts");
    ["verdanab.ttf", "tahomabd.ttf"]
        .iter()
        .find_map(|name| read_font(fonts, &dir.join(name)))
        .unwrap_or_else(|| fonts.add(Font::from_bytes(EMBEDDED_FONT.to_vec())))
}

/// The ammo sheet as a tintable mask: CS:S draws these sprites additively, so whatever is lit
/// becomes coverage and the colour comes from the panel's orange.
fn sprite_sheet(pak: &Path, images: &mut Assets<Image>) -> Option<Handle<Image>> {
    let pack = mdl_source::Vpk::open(pak).ok()?;
    let bytes = pack.read("materials/sprites/640hud1.vtf")?;
    let image = mdl_source::vtf::decode(&bytes).ok()?;
    let mut rgba = image.rgba;
    for px in rgba.as_chunks_mut::<4>().0 {
        let lit = px[0].max(px[1]).max(px[2]);
        px[3] = px[3].min(lit);
        px[0] = 255;
        px[1] = 255;
        px[2] = 255;
    }
    Some(images.add(Image::new(
        Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )))
}

fn load_assets(fonts: &mut Assets<Font>, images: &mut Assets<Image>) -> CsHudAssets {
    let pak = asset_transport::find_css_pak();
    let resource = pak
        .as_deref()
        .and_then(Path::parent)
        .map(|cstrike| cstrike.join("resource"));
    let mut cs_font = |name: &str| {
        resource
            .as_ref()
            .and_then(|dir| read_font(fonts, &dir.join(name)))
    };
    let numbers = cs_font("cstrike.ttf");
    let types = cs_font("cs.ttf");
    let death = cs_font("csd.ttf");
    let sprites = pak.as_deref().and_then(|pak| sprite_sheet(pak, images));
    diag::info!(
        World,
        "cs hud: {} fonts, {} ammo icons",
        if numbers.is_some() {
            "CS:S"
        } else {
            "fallback"
        },
        if sprites.is_some() { "CS:S" } else { "no" }
    );
    CsHudAssets {
        numbers,
        types,
        death,
        names: system_font(fonts),
        sprites,
    }
}

fn text_bundle(part: CsHudPart, font: &Handle<Font>) -> impl Bundle {
    (
        part,
        Node {
            position_type: PositionType::Absolute,
            display: Display::None,
            ..default()
        },
        Text::new(""),
        TextFont {
            font: font.clone().into(),
            font_size: FontSize::Px(NUMBER_TALL),
            ..default()
        },
        TextColor(ORANGE),
        TextLayout::no_wrap(),
    )
}

fn feed_text(part: CsHudPart, font: &Handle<Font>) -> impl Bundle {
    (
        part,
        Node {
            display: Display::None,
            ..default()
        },
        Text::new(""),
        TextFont {
            font: font.clone().into(),
            font_size: FontSize::Px(NAME_TALL),
            ..default()
        },
        TextColor(ORANGE),
        TextLayout::no_wrap(),
        TextShadow {
            offset: Vec2::splat(1.0),
            color: Color::srgba(0.0, 0.0, 0.0, 0.6),
        },
    )
}

/// Builds the HUD under the MW2 HUD root once that exists, under CS rules.
pub(crate) fn spawn_cs_hud(
    mut commands: Commands,
    roots: Query<Entity, With<crate::plugin::HudRoot>>,
    existing: Query<(), With<CsHudRoot>>,
    mut fonts: ResMut<Assets<Font>>,
    mut images: ResMut<Assets<Image>>,
    mut assets: ResMut<CsHudAssets>,
) {
    if !replaces_mw2_hud() || !existing.is_empty() {
        return;
    }
    let Ok(root) = roots.single() else {
        return;
    };
    *assets = load_assets(&mut fonts, &mut images);
    let names = &assets.names;
    let numbers = assets.numbers.as_ref().unwrap_or(names);
    let death = assets.death.as_ref().unwrap_or(names);
    let sprites = assets.sprites.clone();
    commands.entity(root).with_children(|root| {
        root.spawn((
            CsHudRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
        ))
        .with_children(|hud| {
            for panel in PANELS {
                hud.spawn((
                    CsHudPart::Panel(panel),
                    Node {
                        position_type: PositionType::Absolute,
                        display: Display::None,
                        ..default()
                    },
                    BackgroundColor(PANEL_BG),
                ));
                hud.spawn(text_bundle(CsHudPart::Icon(panel), numbers));
                hud.spawn(text_bundle(CsHudPart::Digits(panel), numbers));
            }
            hud.spawn((
                CsHudPart::AmmoBar,
                Node {
                    position_type: PositionType::Absolute,
                    display: Display::None,
                    ..default()
                },
                BackgroundColor(ORANGE.with_alpha(0.5)),
            ));
            hud.spawn(text_bundle(CsHudPart::AmmoReserve, numbers));
            hud.spawn(text_bundle(
                CsHudPart::AmmoGlyph,
                assets.types.as_ref().unwrap_or(numbers),
            ));
            if let Some(sprites) = sprites {
                hud.spawn((
                    CsHudPart::AmmoSprite,
                    Node {
                        position_type: PositionType::Absolute,
                        display: Display::None,
                        ..default()
                    },
                    ImageNode {
                        image: sprites,
                        color: ORANGE,
                        ..default()
                    },
                ));
            }
            for row in 0..FEED_LINES as u8 {
                hud.spawn((
                    CsHudPart::FeedRow(row),
                    Node {
                        position_type: PositionType::Absolute,
                        display: Display::None,
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                ))
                .with_children(|line| {
                    line.spawn(feed_text(CsHudPart::FeedAttacker(row), names));
                    line.spawn(feed_text(CsHudPart::FeedWeapon(row), death));
                    line.spawn(feed_text(CsHudPart::FeedHeadshot(row), death));
                    line.spawn(feed_text(CsHudPart::FeedVictim(row), names));
                });
            }
        });
    });
}

/// The ammo icon's cell in `640hud1` (`hud_textures.txt`, 640 set), by CS weapon.
fn ammo_sprite(name: &str) -> Option<Rect> {
    // By ammo type: 7.62 (AK-47, Scout, G3/SG-1), 5.56 (M4A1, Galil, FAMAS, SG 552, AUG, SG 550,
    // M249), .338 (AWP), .50 AE, .45 ACP (USP, MAC-10, UMP45), 9mm (Glock, Elites, TMP, MP5),
    // .357 SIG (P228), 5.7 (Five-seveN, P90), 12 gauge (M3, XM1014).
    let [x, y, w, h] = match name {
        "ak47" | "scout" | "g3sg1" => [232.0, 48.0, 24.0, 26.0],
        "m4a1" | "galil" | "famas" | "sg552" | "aug" | "sg550" | "m249" => {
            [157.0, 74.0, 25.0, 24.0]
        }
        "awp" => [182.0, 74.0, 26.0, 24.0],
        "deagle" => [182.0, 48.0, 26.0, 26.0],
        "usp" | "mac10" | "ump45" => [182.0, 0.0, 26.0, 24.0],
        "glock" | "elite" | "tmp" | "mp5" => [208.0, 48.0, 24.0, 26.0],
        "p228" => [208.0, 0.0, 24.0, 24.0],
        "fiveseven" | "p90" => [208.0, 24.0, 24.0, 24.0],
        "m3" | "xm1014" => [157.0, 48.0, 25.0, 26.0],
        _ => return None,
    };
    Some(Rect::new(x, y, x + w, y + h))
}

/// The kill icon CS:S shows for `weapon` (a weapon script name).
fn kill_icon(weapon: &str) -> (IconFont, char) {
    use weapon_iw4::cs;
    let name = weapon.rsplit(['/', ':']).next().unwrap_or(weapon);
    if let Some(grenade) = cs::CS_GRENADES
        .iter()
        .find(|g| g.projectile.eq_ignore_ascii_case(name) || g.mw2_name.eq_ignore_ascii_case(name))
    {
        return match grenade.name {
            "flashbang" => (IconFont::Types, 'g'),
            _ => (IconFont::Death, 'h'),
        };
    }
    let Some(index) = cs::cs_weapon_index_for(name) else {
        return (IconFont::Death, 'C');
    };
    if cs::is_knife(index) {
        return (IconFont::Death, 'j');
    }
    let glyph = match cs::cs_weapon(index).map(|w| w.name) {
        Some("ak47") => 'b',
        Some("m4a1") => 'w',
        Some("awp") => 'r',
        Some("deagle") => 'f',
        Some("usp") => 'a',
        Some("glock") => 'c',
        Some("aug") => 'e',
        Some("elite") => 's',
        Some("famas") => 't',
        Some("fiveseven") => 'u',
        Some("g3sg1") => 'i',
        Some("galil") => 'v',
        Some("m249") => 'z',
        Some("m3") => 'k',
        Some("mac10") => 'l',
        Some("mp5") => 'x',
        Some("p228") => 'y',
        Some("p90") => 'm',
        Some("scout") => 'n',
        Some("sg552") => 'A',
        Some("sg550") => 'o',
        Some("tmp") => 'd',
        Some("ump45") => 'q',
        Some("xm1014") => 'B',
        _ => 'C',
    };
    (IconFont::Death, glyph)
}

/// The CS side an MW2 team plays. In the bomb mode it follows the role — the attackers (who plant)
/// are the Terrorists, so it flips when the sides switch; elsewhere axis are the Terrorists and
/// allies the Counter-Terrorists. `None` for no team.
pub(crate) fn is_terrorist(snap: Option<&sim::Snapshot>, team: i32) -> Option<bool> {
    if team != entity_iw4::TEAM_AXIS && team != entity_iw4::TEAM_ALLIES {
        return None;
    }
    let attackers = snap.and_then(|snap| {
        snap.meta
            .objectives
            .server_info
            .iter()
            .find(|(name, _)| name == "cs_attackers")
            .map(|(_, value)| value.as_str())
    });
    Some(match attackers {
        Some("axis") => team == entity_iw4::TEAM_AXIS,
        Some("allies") => team == entity_iw4::TEAM_ALLIES,
        _ => team == entity_iw4::TEAM_AXIS,
    })
}

fn team_color(snap: Option<&sim::Snapshot>, team: i32, is_local: bool) -> Color {
    match is_terrorist(snap, team) {
        Some(true) => T_RED,
        Some(false) => CT_BLUE,
        None if is_local => CT_BLUE,
        None => T_RED,
    }
}

/// Adds a death notice for every obituary.
pub(crate) fn obituary(
    obituary: On<net::EntityObituary>,
    mut feed: ResMut<CsKillFeed>,
    weapons: Option<Res<PreparedWeapons>>,
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
) {
    if obituary.in_killcam || !replaces_mw2_hud() {
        return;
    }
    let payload = obituary.event.payload;
    let player = |client: i32| {
        let name = crate::killfeed::snapshot_client_name(&presented, client);
        let team = crate::killfeed::snapshot_client_team(&presented, client);
        let color = team_color(
            presented.snapshot(),
            team,
            client >= 0 && client as u32 == local.0.0,
        );
        (name, color)
    };
    let has_attacker = (0..18).contains(&payload.attacker_entity_num)
        && payload.attacker_entity_num != payload.other_entity_num;
    let weapon = weapons.as_ref().map(|w| w.0.script_name_of(payload.weapon));
    let icon = match weapon.as_deref() {
        Some(weapon) if has_attacker => kill_icon(weapon),
        _ => (IconFont::Death, 'C'),
    };
    feed.lines.push_back(FeedLine {
        start_ms: milliseconds() as i32,
        attacker: has_attacker.then(|| player(payload.attacker_entity_num)),
        victim: player(payload.other_entity_num),
        icon,
        headshot: hud_iw4::obituary_mod(payload.event_parm) == Some(hud_iw4::MOD_HEAD_SHOT),
    });
    while feed.lines.len() > FEED_LINES {
        feed.lines.pop_front();
    }
}

/// What the panels show this frame.
#[derive(Default)]
struct PanelValues {
    health: Option<i32>,
    armor: Option<(i32, bool)>,
    timer_s: Option<i32>,
    ammo: Option<Ammo>,
}

struct Ammo {
    clip: i32,
    reserve: Option<i32>,
    sprite: Option<Rect>,
    grenade: Option<char>,
}

fn ammo_values(ps: &PlayerState, weapons: &PreparedWeapons, presented: &PresentedSnapshot, local: &LocalPresentClient) -> Option<Ammo> {
    use weapon_iw4::cs;
    let viewmodel = weapon_iw4::get_viewmodel_weapon_index(ps);
    if viewmodel == 0 {
        return None;
    }
    let index = cs::cs_weapon_index_for(&weapons.0.script_name_of(viewmodel));
    if index.is_some_and(cs::is_knife) {
        return None;
    }
    let meta = presented.snapshot().and_then(|s| s.meta.for_client(local.0));
    let ammo = crate::ammo::weaponbar_ammo(ps, weapons, meta)?;
    let clip = ammo.clip?;
    if let Some(grenade) = index.and_then(cs::cs_grenade) {
        let glyph = match grenade.name {
            "flashbang" => 'g',
            "hegrenade" => 'h',
            _ => 'h',
        };
        return Some(Ammo {
            clip,
            reserve: None,
            sprite: None,
            grenade: (grenade.name != "smokegrenade").then_some(glyph),
        });
    }
    Some(Ammo {
        clip,
        reserve: ammo.stock,
        sprite: index
            .and_then(cs::cs_weapon)
            .and_then(|w| ammo_sprite(w.name)),
        grenade: None,
    })
}

fn set_px(slot: &mut Val, px: f32) -> bool {
    let want = Val::Px(px.round());
    if *slot == want {
        return false;
    }
    *slot = want;
    true
}

/// Places `node` at `left`/`top` (and `size` when given), only touching what changed.
fn place(node: &mut Mut<Node>, left: f32, top: f32, size: Option<[f32; 2]>) {
    let mut next = Node::clone(node);
    let mut changed = set_px(&mut next.left, left) | set_px(&mut next.top, top);
    if let Some([w, h]) = size {
        changed |= set_px(&mut next.width, w) | set_px(&mut next.height, h);
    }
    if next.display != Display::Flex {
        next.display = Display::Flex;
        changed = true;
    }
    if changed {
        **node = next;
    }
}

fn set_text(text: &mut Mut<Text>, value: &str) {
    if text.0 != value {
        text.0 = value.to_owned();
    }
}

fn set_size(font: &mut Mut<TextFont>, px: f32) {
    let px = px.round().max(1.0);
    if !matches!(font.font_size, FontSize::Px(v) if v == px) {
        font.font_size = FontSize::Px(px);
    }
}

fn set_color(color: &mut Mut<TextColor>, want: Color) {
    if color.0 != want {
        color.0 = want;
    }
}

type PartQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static CsHudPart,
        &'static mut Node,
        Option<&'static mut Text>,
        Option<&'static mut TextFont>,
        Option<&'static mut TextColor>,
        Option<&'static mut ImageNode>,
    ),
>;

fn hide_all(parts: &mut PartQuery) {
    for (_, mut node, ..) in parts.iter_mut() {
        adopt_display(&mut node, Display::None);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_cs_hud(
    surface: Res<crate::surface::Hud2dSurface>,
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    weapons: Option<Res<PreparedWeapons>>,
    view: Res<ViewSubject>,
    assets: Res<CsHudAssets>,
    mut feed: ResMut<CsKillFeed>,
    mut parts: PartQuery,
) {
    if !replaces_mw2_hud() {
        return;
    }
    let now_ms = milliseconds() as i32;
    feed.lines
        .retain(|line| now_ms.saturating_sub(line.start_ms) < FEED_SECONDS_MS);
    if !surface.is_ready() || view.in_killcam() {
        hide_all(&mut parts);
        return;
    }
    let Some(ps) = presented.player(local.0) else {
        feed.lines.clear();
        hide_all(&mut parts);
        return;
    };

    let mut values = PanelValues::default();
    if ps.pm_type < playerstate_iw4::PM_TYPE_DEAD {
        values.health = Some(ps.health.max(0));
        values.armor = Some((ps.cs_armor as i32, ps.cs_helmet != 0));
        values.ammo = weapons
            .as_deref()
            .and_then(|w| ammo_values(ps, w, &presented, &local));
    }
    if let Some(snap) = presented.snapshot() {
        let now = snap.tick.0.saturating_mul(sim::MATCH_TICK_MS) as i32;
        let left = snap.meta.objectives.time_left_ms(now);
        if left > 0 {
            values.timer_s = Some((left + 999) / 1000);
        }
    }

    let unit = surface.height() / crate::presentation_scale::VIRTUAL_HEIGHT;
    let width_units = surface.width() / unit;
    let icons = assets.numbers.is_some();
    for (part, mut node, text, font, color, image) in parts.iter_mut() {
        match *part {
            CsHudPart::Panel(panel) | CsHudPart::Icon(panel) | CsHudPart::Digits(panel) => {
                let value: Option<(String, char, Color)> = match panel {
                    Panel::Health => values.health.map(|h| {
                        let tint = if h <= LOW_HEALTH { RED } else { ORANGE };
                        (h.to_string(), 'b', tint)
                    }),
                    Panel::Armor => values.armor.map(|(points, helmet)| {
                        (points.to_string(), if helmet { 'l' } else { 'a' }, ORANGE)
                    }),
                    Panel::Timer => values.timer_s.map(|s| {
                        let tint = if s < LOW_TIME_S { RED } else { ORANGE };
                        (format!("{}:{:02}", s / 60, s % 60), 'e', tint)
                    }),
                    Panel::Ammo => values.ammo.as_ref().map(|a| (a.clip.to_string(), ' ', ORANGE)),
                };
                let Some((digits, glyph, tint)) = value else {
                    adopt_display(&mut node, Display::None);
                    continue;
                };
                let [x, y, w, h] = panel.rect(width_units);
                match *part {
                    CsHudPart::Panel(_) => {
                        place(&mut node, x * unit, y * unit, Some([w * unit, h * unit]));
                        let radius = BorderRadius::all(Val::Px((CORNER * unit).round()));
                        if node.border_radius != radius {
                            node.border_radius = radius;
                        }
                    }
                    CsHudPart::Icon(_) => {
                        if glyph == ' ' || !icons {
                            adopt_display(&mut node, Display::None);
                            continue;
                        }
                        let [dx, dy] = panel.icon();
                        place(&mut node, (x + dx) * unit, (y + dy) * unit, None);
                        if let Some(mut text) = text {
                            set_text(&mut text, &glyph.to_string());
                        }
                        if let Some(mut color) = color {
                            set_color(&mut color, tint);
                        }
                        if let Some(mut font) = font {
                            set_size(&mut font, NUMBER_TALL * unit);
                        }
                    }
                    _ => {
                        let [dx, dy] = panel.digits();
                        place(&mut node, (x + dx) * unit, (y + dy) * unit, None);
                        if let Some(mut text) = text {
                            set_text(&mut text, &digits);
                        }
                        if let Some(mut color) = color {
                            set_color(&mut color, tint);
                        }
                        if let Some(mut font) = font {
                            set_size(&mut font, NUMBER_TALL * unit);
                        }
                    }
                }
            }
            CsHudPart::AmmoBar | CsHudPart::AmmoReserve => {
                let Some(reserve) = values.ammo.as_ref().and_then(|a| a.reserve) else {
                    adopt_display(&mut node, Display::None);
                    continue;
                };
                let [x, y, ..] = Panel::Ammo.rect(width_units);
                if *part == CsHudPart::AmmoBar {
                    let [dx, dy, w, h] = AMMO_BAR;
                    place(
                        &mut node,
                        (x + dx) * unit,
                        (y + dy) * unit,
                        Some([(w * unit).max(1.0), h * unit]),
                    );
                } else {
                    let [dx, dy] = AMMO_RESERVE;
                    place(&mut node, (x + dx) * unit, (y + dy) * unit, None);
                    if let Some(mut text) = text {
                        set_text(&mut text, &reserve.to_string());
                    }
                    if let Some(mut font) = font {
                        set_size(&mut font, NUMBER_TALL * unit);
                    }
                }
            }
            CsHudPart::AmmoSprite => {
                let rect = values.ammo.as_ref().and_then(|a| a.sprite);
                let (Some(rect), Some(mut image), true) = (rect, image, assets.sprites.is_some())
                else {
                    adopt_display(&mut node, Display::None);
                    continue;
                };
                let [x, y, ..] = Panel::Ammo.rect(width_units);
                let [dx, dy] = AMMO_ICON;
                place(
                    &mut node,
                    (x + dx) * unit,
                    (y + dy) * unit,
                    Some([rect.width() * unit, rect.height() * unit]),
                );
                if image.rect != Some(rect) {
                    image.rect = Some(rect);
                }
            }
            CsHudPart::AmmoGlyph => {
                let glyph = values.ammo.as_ref().and_then(|a| a.grenade);
                let (Some(glyph), true) = (glyph, assets.types.is_some()) else {
                    adopt_display(&mut node, Display::None);
                    continue;
                };
                let [x, y, ..] = Panel::Ammo.rect(width_units);
                let [dx, _] = AMMO_ICON;
                place(&mut node, (x + dx - 30.0) * unit, (y - 2.0) * unit, None);
                if let Some(mut text) = text {
                    set_text(&mut text, &glyph.to_string());
                }
                if let Some(mut font) = font {
                    set_size(&mut font, GRENADE_TALL * unit);
                }
            }
            CsHudPart::FeedRow(row) => {
                let Some(_) = feed_line(&feed, row) else {
                    adopt_display(&mut node, Display::None);
                    continue;
                };
                let top = (FEED_TOP + f32::from(row) * FEED_LINE_HEIGHT) * unit;
                let mut next = Node::clone(&node);
                let mut changed = set_px(&mut next.top, top)
                    | set_px(&mut next.right, FEED_RIGHT * unit)
                    | set_px(&mut next.height, FEED_LINE_HEIGHT * unit)
                    | set_px(&mut next.column_gap, 4.0 * unit);
                if next.display != Display::Flex {
                    next.display = Display::Flex;
                    changed = true;
                }
                if changed {
                    *node = next;
                }
            }
            CsHudPart::FeedAttacker(row)
            | CsHudPart::FeedWeapon(row)
            | CsHudPart::FeedHeadshot(row)
            | CsHudPart::FeedVictim(row) => {
                let Some(line) = feed_line(&feed, row) else {
                    adopt_display(&mut node, Display::None);
                    continue;
                };
                let content: Option<(String, Color, f32)> = match *part {
                    CsHudPart::FeedAttacker(_) => line
                        .attacker
                        .as_ref()
                        .map(|(name, tint)| (name.clone(), *tint, NAME_TALL)),
                    CsHudPart::FeedWeapon(_) => {
                        let (which, glyph) = line.icon;
                        let ready = match which {
                            IconFont::Death => assets.death.is_some(),
                            IconFont::Types => assets.types.is_some(),
                        };
                        ready.then(|| (glyph.to_string(), ORANGE, DEATH_TALL))
                    }
                    CsHudPart::FeedHeadshot(_) => (line.headshot && assets.death.is_some())
                        .then(|| ("D".to_owned(), ORANGE, DEATH_TALL)),
                    _ => Some((line.victim.0.clone(), line.victim.1, NAME_TALL)),
                };
                let Some((value, tint, tall)) = content else {
                    adopt_display(&mut node, Display::None);
                    continue;
                };
                let drop = if tall == DEATH_TALL {
                    (DEATH_DROP * tall * unit).round()
                } else {
                    0.0
                };
                if node.top != Val::Px(drop) {
                    node.top = Val::Px(drop);
                }
                adopt_display(&mut node, Display::Flex);
                if let Some(mut text) = text {
                    set_text(&mut text, &value);
                }
                if let Some(mut color) = color {
                    set_color(&mut color, tint);
                }
                if let Some(mut font) = font {
                    set_size(&mut font, tall * unit);
                    // The flashbang icon lives in cs.ttf, the rest in csd.ttf.
                    if let CsHudPart::FeedWeapon(_) = *part {
                        let want = match line.icon.0 {
                            IconFont::Death => assets.death.clone(),
                            IconFont::Types => assets.types.clone(),
                        };
                        if let Some(want) = want {
                            let source: bevy::text::FontSource = want.into();
                            if font.font != source {
                                font.font = source;
                            }
                        }
                    }
                }
            }
        };
    }
}

/// The feed line drawn in `row` (rows fill from the top, oldest first).
fn feed_line(feed: &CsKillFeed, row: u8) -> Option<&FeedLine> {
    feed.lines.get(usize::from(row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_icons_follow_the_weapon() {
        assert_eq!(kill_icon("ak47_mp"), (IconFont::Death, 'b'));
        assert_eq!(kill_icon("cheytac_mp"), (IconFont::Death, 'r'));
        assert_eq!(kill_icon("frag_grenade_mp"), (IconFont::Death, 'h'));
        assert_eq!(kill_icon("flash_grenade_mp"), (IconFont::Types, 'g'));
        assert_eq!(kill_icon("beretta_mp"), (IconFont::Death, 'j'));
        assert_eq!(kill_icon("rpg_mp"), (IconFont::Death, 'C'));
    }
}
