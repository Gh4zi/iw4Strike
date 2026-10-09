//! Counter-Strike 1.6's HUD, for players who play with CS 1.6 instead of CS:S (no CS:S found,
//! or its Use box unticked): health, armour, round timer, money and ammo drawn with CS 1.6's own
//! sprites (`sprites/hud.txt` cells of the `640hud` sheets and each gun's `weapon_*.txt` ammo
//! icon), the C4 and defuse kit status icons, the plant/defuse progress bar and the kill feed
//! with its `d_` weapon sprites — all read from the player's CS 1.6 install at runtime.
//!
//! Placed, coloured and faded like the CS 1.6 client: numbers sit dim (`MIN_ALPHA`) and light
//! up when they change, health turns red at 25 and bright at 15, the money shows its last
//! change above it for 5 seconds, and the timer flashes red in the last 20 seconds. GoldSrc
//! draws its HUD in screen pixels; these are a 1024x768 screen's, scaled to the window's height.
//! GoldSrc adds its HUD sprites onto the picture; here their brightness becomes coverage of the
//! tint (see [`tint`]).

use std::collections::HashMap;
use std::path::Path;

use assets::PreparedWeapons;
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::text::FontSize;
use bevy::ui::Display;
use frame::ViewSubject;
use mdl_goldsrc::spr::{Sprite as SpriteFile, parse_hud_list};
use net::{LocalPresentClient, PresentedSnapshot};
use playerstate_iw4::PlayerState;

use crate::cs_hud::{CsKillFeed, FEED_LINES, place, set_px};
use crate::ui_write::adopt_display;

/// The screen height whose pixels the HUD is laid out in: CS 1.6 at 1024x768.
const VIRTUAL_HEIGHT: f32 = 768.0;
/// `RGB_YELLOWISH`, the HUD colour.
const YELLOWISH: [f32; 3] = [1.0, 160.0 / 255.0, 0.0];
/// `RGB_REDISH`: the timer's panic flash and a money loss.
const REDISH: [f32; 3] = [1.0, 16.0 / 255.0, 16.0 / 255.0];
/// `RGB_GREENISH`: a money gain and the status icons.
const GREENISH: [f32; 3] = [0.0, 160.0 / 255.0, 0.0];
/// Health at or under 25 is drawn in this red, and at or under 15 at full brightness.
const HEALTH_RED: [f32; 3] = [250.0 / 255.0, 0.0, 0.0];
/// The progress bar's orange (`255 140 0`).
const BAR_ORANGE: [f32; 3] = [1.0, 140.0 / 255.0, 0.0];
/// Numbers at rest, out of 255.
const MIN_ALPHA: f32 = 100.0;
/// A changed health or armour number starts this far above rest and fades at 20 a second.
const FADE_TIME: f32 = 100.0;
/// The ammo starts at 200 when it changes and fades to [`MIN_ALPHA`] at 20 a second.
const AMMO_FADE: f32 = 200.0;
const FADE_PER_SECOND: f32 = 20.0;
/// The money's change shows this long.
const MONEY_FADE_S: f32 = 5.0;
/// The timer starts flashing at this many seconds left.
const PANIC_S: i32 = 20;
/// `MAX_DEATHNOTICES` rows from `YRES(32) + 2`, 20 pixels apart; names in the console font.
const FEED_TOP: f32 = 32.0 * VIRTUAL_HEIGHT / 480.0 + 2.0;
const FEED_LINE_HEIGHT: f32 = 20.0;
const NAME_TALL: f32 = 14.0;
/// The kill feed's weapon sprites (`255 80 0`), and a team kill's "sickly green".
const KILL_ORANGE: [f32; 3] = [1.0, 80.0 / 255.0, 0.0];
const TEAM_KILL: [f32; 3] = [10.0 / 255.0, 240.0 / 255.0, 10.0 / 255.0];

/// The kill sprite for a world or unknown kill.
pub(crate) const WORLD_KILL: &str = "d_skull";

/// Sprite and fill slots: every number, icon and bar the HUD can show at once fits.
const SPRITES: u8 = 48;
const FILLS: u8 = 12;

/// The CS 1.6 file name of an iw4Strike CS weapon (`weapon_<name>.txt`, `d_<name>`).
fn goldsrc_weapon(name: &str) -> &str {
    match name {
        "glock" => "glock18",
        "mp5" => "mp5navy",
        other => other,
    }
}

/// The CS 1.6 kill sprite for `weapon` (a weapon script name).
pub(crate) fn kill_sprite(weapon: &str) -> &'static str {
    use weapon_iw4::cs;
    let name = weapon.rsplit(['/', ':']).next().unwrap_or(weapon);
    if let Some(grenade) = cs::CS_GRENADES
        .iter()
        .find(|g| g.projectile.eq_ignore_ascii_case(name) || g.mw2_name.eq_ignore_ascii_case(name))
    {
        return match grenade.name {
            "flashbang" => "d_flashbang",
            _ => "d_grenade",
        };
    }
    let Some(index) = cs::cs_weapon_index_for(name) else {
        return WORLD_KILL;
    };
    if cs::is_knife(index) {
        return "d_knife";
    }
    match cs::cs_weapon(index).map(|w| goldsrc_weapon(w.name)) {
        Some("ak47") => "d_ak47",
        Some("m4a1") => "d_m4a1",
        Some("awp") => "d_awp",
        Some("deagle") => "d_deagle",
        Some("usp") => "d_usp",
        Some("glock18") => "d_glock18",
        Some("aug") => "d_aug",
        Some("elite") => "d_elite",
        Some("famas") => "d_famas",
        Some("fiveseven") => "d_fiveseven",
        Some("g3sg1") => "d_g3sg1",
        Some("galil") => "d_galil",
        Some("m249") => "d_m249",
        Some("m3") => "d_m3",
        Some("mac10") => "d_mac10",
        Some("mp5navy") => "d_mp5navy",
        Some("p228") => "d_p228",
        Some("p90") => "d_p90",
        Some("scout") => "d_scout",
        Some("sg552") => "d_sg552",
        Some("sg550") => "d_sg550",
        Some("tmp") => "d_tmp",
        Some("ump45") => "d_ump45",
        Some("xm1014") => "d_xm1014",
        _ => WORLD_KILL,
    }
}

/// A named cell of a sprite sheet.
#[derive(Clone, Debug)]
struct Cell {
    sheet: Handle<Image>,
    rect: Rect,
}

impl Cell {
    fn width(&self) -> f32 {
        self.rect.width()
    }

    fn height(&self) -> f32 {
        self.rect.height()
    }
}

/// CS 1.6's HUD sprites, read once from the GoldSrc folders (Condition Zero has none of its own
/// and uses these too).
#[derive(Resource, Default)]
pub(crate) struct Cs16HudAssets {
    /// Playing with CS 1.6 and its HUD sprites were read: this HUD draws, the CS:S one doesn't.
    pub(crate) active: bool,
    /// `hud.txt`'s 640 cells by name.
    cells: HashMap<String, Cell>,
    /// Each weapon's `ammo` cell, by CS 1.6 weapon name.
    ammo: HashMap<String, Cell>,
    /// Kill feed names.
    font: Handle<Font>,
}

impl Cs16HudAssets {
    fn cell(&self, name: &str) -> Option<&Cell> {
        self.cells.get(name)
    }

    fn digit(&self, digit: i32) -> Option<&Cell> {
        self.cells
            .get(&format!("number_{}", digit.rem_euclid(10)))
    }

    /// `number_0`'s width: every digit advances this much.
    fn digit_width(&self) -> f32 {
        self.cell("number_0").map_or(20.0, Cell::width)
    }

    /// `m_iFontHeight`: `number_0`'s height.
    fn font_height(&self) -> f32 {
        self.cell("number_0").map_or(25.0, Cell::height)
    }
}

/// The cells the HUD draws (`hud.txt` names).
fn wanted(name: &str) -> bool {
    name.starts_with("number_")
        || name.starts_with("d_")
        || matches!(
            name,
            "cross"
                | "suit_full"
                | "suit_empty"
                | "suithelmet_full"
                | "suithelmet_empty"
                | "dollar"
                | "minus"
                | "plus"
                | "c4"
                | "defuser"
                | "stopwatch"
        )
}

/// A sheet as a tintable mask: what GoldSrc adds onto the screen becomes coverage, the colour
/// comes from the tint.
fn sheet_image(path: &Path, images: &mut Assets<Image>) -> Option<Handle<Image>> {
    let bytes = std::fs::read(path).ok()?;
    let sprite = match SpriteFile::parse(&bytes) {
        Ok(sprite) => sprite,
        Err(error) => {
            diag::warn!(World, "cs 1.6 hud: {}: {error}", path.display());
            return None;
        }
    };
    let frame = sprite.frames.into_iter().next()?;
    let mut rgba = frame.rgba;
    for px in rgba.as_chunks_mut::<4>().0 {
        let lit = px[0].max(px[1]).max(px[2]);
        *px = [255, 255, 255, px[3].min(lit)];
    }
    Some(images.add(Image::new(
        Extent3d {
            width: frame.width,
            height: frame.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )))
}

/// Reads the HUD sprites from the GoldSrc folders (each file from the first holding it); `None`
/// when `hud.txt` or the digits are missing.
pub(crate) fn load(
    dirs: &asset_transport::GoldSrcDirs,
    fonts: &mut Assets<Font>,
    images: &mut Assets<Image>,
) -> Option<Cs16HudAssets> {
    let list = std::fs::read_to_string(dirs.file("sprites/hud.txt")?).ok()?;
    let mut sheets: HashMap<String, Option<Handle<Image>>> = HashMap::new();
    let mut sheet = |name: &str, images: &mut Assets<Image>| {
        sheets
            .entry(name.to_ascii_lowercase())
            .or_insert_with(|| {
                let path = dirs.file(&format!("sprites/{name}.spr"))?;
                sheet_image(&path, images)
            })
            .clone()
    };
    let mut cell = |entry: &mdl_goldsrc::spr::HudSprite, images: &mut Assets<Image>| {
        let sheet = sheet(&entry.sheet, images)?;
        let [x, y] = [entry.x as f32, entry.y as f32];
        Some(Cell {
            sheet,
            rect: Rect::new(x, y, x + entry.width as f32, y + entry.height as f32),
        })
    };
    let mut cells = HashMap::new();
    for entry in parse_hud_list(&list) {
        if entry.resolution == 640
            && wanted(&entry.name)
            && let Some(found) = cell(&entry, images)
        {
            cells.insert(entry.name.clone(), found);
        }
    }
    if (0..10).any(|digit| !cells.contains_key(&format!("number_{digit}"))) {
        diag::warn!(World, "cs 1.6 hud: no digits in {:?}", dirs.0);
        return None;
    }
    let mut ammo = HashMap::new();
    let names = weapon_iw4::cs::CS_WEAPONS
        .iter()
        .map(|w| goldsrc_weapon(w.name))
        .chain(weapon_iw4::cs::CS_GRENADES.iter().map(|g| g.name))
        .chain([weapon_iw4::cs::CS_C4.name]);
    for name in names {
        let Some(text) = dirs
            .read(&format!("sprites/weapon_{name}.txt"))
            .and_then(|bytes| String::from_utf8(bytes).ok())
        else {
            continue;
        };
        if let Some(found) = parse_hud_list(&text)
            .iter()
            .find(|entry| entry.name == "ammo" && entry.resolution == 640)
            .and_then(|entry| cell(entry, images))
        {
            ammo.insert(name.to_owned(), found);
        }
    }
    diag::info!(
        World,
        "cs 1.6 hud: {} cells, {} ammo icons from {:?}",
        cells.len(),
        ammo.len(),
        dirs.0
    );
    Some(Cs16HudAssets {
        active: true,
        cells,
        ammo,
        font: crate::cs_hud::system_font(fonts, &["tahomabd.ttf", "verdanab.ttf"]),
    })
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Cs16Part {
    Sprite(u8),
    Fill(u8),
    FeedRow(u8),
    FeedAttacker(u8),
    FeedWeapon(u8),
    FeedHeadshot(u8),
    FeedVictim(u8),
}

fn hidden() -> Node {
    Node {
        position_type: PositionType::Absolute,
        display: Display::None,
        ..default()
    }
}

fn feed_name(part: Cs16Part, font: &Handle<Font>) -> impl Bundle {
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
        TextColor(Color::WHITE),
        TextLayout::no_wrap(),
    )
}

fn feed_sprite(part: Cs16Part) -> impl Bundle {
    (
        part,
        Node {
            display: Display::None,
            ..default()
        },
        ImageNode::default(),
    )
}

/// The HUD's slots, under the CS HUD root.
pub(crate) fn spawn_parts(hud: &mut ChildSpawnerCommands, assets: &Cs16HudAssets) {
    for slot in 0..SPRITES {
        hud.spawn((Cs16Part::Sprite(slot), hidden(), ImageNode::default()));
    }
    for slot in 0..FILLS {
        hud.spawn((Cs16Part::Fill(slot), hidden(), BackgroundColor(Color::NONE)));
    }
    for row in 0..FEED_LINES as u8 {
        hud.spawn((
            Cs16Part::FeedRow(row),
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::FlexStart,
                ..default()
            },
        ))
        .with_children(|line| {
            line.spawn(feed_name(Cs16Part::FeedAttacker(row), &assets.font));
            line.spawn(feed_sprite(Cs16Part::FeedWeapon(row)));
            line.spawn(feed_sprite(Cs16Part::FeedHeadshot(row)));
            line.spawn(feed_name(Cs16Part::FeedVictim(row), &assets.font));
        });
    }
}

/// One sprite this frame, in virtual pixels; `cut_top` pixels of the cell are left off its top.
struct SpriteDraw {
    cell: Cell,
    x: f32,
    y: f32,
    cut_top: f32,
    color: Color,
}

struct FillDraw {
    rect: [f32; 4],
    color: Color,
}

/// `SPR_Set` + `ScaleColors`: the colour at brightness `alpha` (0-255). GoldSrc adds the sprite
/// at that brightness; on MW2's sunlit maps the resting numbers (100) would all but vanish, so
/// coverage follows the brightness's square root — still dim at rest and bright on a change,
/// but readable on sand and sky.
fn tint(rgb: [f32; 3], alpha: f32) -> Color {
    Color::srgba(rgb[0], rgb[1], rgb[2], (alpha / 255.0).clamp(0.0, 1.0).sqrt())
}

#[derive(Default)]
struct Frame {
    sprites: Vec<SpriteDraw>,
    fills: Vec<FillDraw>,
}

impl Frame {
    fn sprite(&mut self, cell: Option<&Cell>, x: f32, y: f32, color: Color) {
        if let Some(cell) = cell {
            self.sprites.push(SpriteDraw {
                cell: cell.clone(),
                x,
                y,
                cut_top: 0.0,
                color,
            });
        }
    }

    fn fill(&mut self, rect: [f32; 4], color: Color) {
        self.fills.push(FillDraw { rect, color });
    }

    /// `DrawHudNumber` with `DHN_3DIGITS | DHN_DRAWZERO`: three digit places from `x`, blanks
    /// for missing leading digits, a lone 0 for zero. Returns the x after the last place.
    fn number3(&mut self, a: &Cs16HudAssets, mut x: f32, y: f32, n: i32, color: Color) -> f32 {
        let w = a.digit_width();
        let n = n.clamp(0, 999);
        if n >= 100 {
            self.sprite(a.digit(n / 100), x, y, color);
        }
        x += w;
        if n >= 10 {
            self.sprite(a.digit(n % 100 / 10), x, y, color);
        }
        x += w;
        self.sprite(a.digit(n % 10), x, y, color);
        x + w
    }

    /// `DrawHudNumber2`: `n` right-aligned in `places` digit places from `x` (padded with zeros
    /// when `zeros`). Returns the x after the last place.
    #[allow(clippy::too_many_arguments)]
    fn number2(
        &mut self,
        a: &Cs16HudAssets,
        x: f32,
        y: f32,
        zeros: bool,
        places: i32,
        mut n: i32,
        color: Color,
    ) -> f32 {
        let w = a.digit_width();
        let mut at = x + (places - 1) as f32 * w;
        let end = at + w;
        let mut left = places;
        n = n.max(0);
        loop {
            self.sprite(a.digit(n % 10), at, y, color);
            n /= 10;
            at -= w;
            left -= 1;
            if !(n > 0 || (left > 0 && zeros)) {
                break;
            }
        }
        end
    }
}

/// The weapon in hand, its clip and its count: the ammo flashes when any changes.
type AmmoKey = (u32, Option<i32>, i32);

/// What changed when, for the fades.
#[derive(Default)]
pub(crate) struct Fades {
    health: Option<(i32, f32)>,
    armor: Option<(i32, f32)>,
    ammo: Option<(AmmoKey, f32)>,
    money: Option<i32>,
    money_delta: i32,
    money_since: f32,
    panic_time: f32,
    panic_plain: bool,
    progress: Option<(f32, f32)>,
}

/// When `value` last changed (now, if it just did).
fn changed_at<T: PartialEq + Copy>(slot: &mut Option<(T, f32)>, value: T, now: f32) -> f32 {
    match slot {
        Some((last, since)) if *last == value => *since,
        _ => {
            *slot = Some((value, now));
            now
        }
    }
}

/// `MIN_ALPHA` plus what is left of a [`FADE_TIME`] flash `since` seconds ago.
fn faded(since: f32, now: f32) -> f32 {
    let fade = FADE_TIME - (now - since) * FADE_PER_SECOND;
    if fade > 0.0 {
        MIN_ALPHA + fade / FADE_TIME * 128.0
    } else {
        MIN_ALPHA
    }
}

/// The ammo line: the gun's clip and reserve, or a count alone (grenades, the C4).
struct Ammo {
    clip: Option<i32>,
    count: i32,
    icon: Option<Cell>,
    key: u32,
}

fn ammo_values(
    ps: &PlayerState,
    weapons: &PreparedWeapons,
    assets: &Cs16HudAssets,
    presented: &PresentedSnapshot,
    local: &LocalPresentClient,
) -> Option<Ammo> {
    use weapon_iw4::cs;
    let viewmodel = weapon_iw4::get_viewmodel_weapon_index(ps);
    if viewmodel == 0 {
        return None;
    }
    let index = cs::cs_weapon_index_for(&weapons.0.script_name_of(viewmodel))?;
    if cs::is_knife(index) {
        return None;
    }
    let icon = |name: &str| assets.ammo.get(name).cloned();
    if cs::is_c4(index) {
        return Some(Ammo {
            clip: None,
            count: 1,
            icon: icon(cs::CS_C4.name),
            key: viewmodel,
        });
    }
    let meta = presented.snapshot().and_then(|s| s.meta.for_client(local.0));
    let ammo = crate::ammo::weaponbar_ammo(ps, weapons, meta)?;
    if let Some(grenade) = cs::cs_grenade(index) {
        return Some(Ammo {
            clip: None,
            count: ammo.clip?,
            icon: icon(grenade.name),
            key: viewmodel,
        });
    }
    let name = goldsrc_weapon(cs::cs_weapon(index)?.name);
    Some(Ammo {
        clip: Some(ammo.clip?),
        count: ammo.stock.unwrap_or(0),
        icon: icon(name),
        key: viewmodel,
    })
}

type PartQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Cs16Part,
        &'static mut Node,
        Option<&'static mut ImageNode>,
        Option<&'static mut BackgroundColor>,
        Option<&'static mut Text>,
        Option<&'static mut TextFont>,
        Option<&'static mut TextColor>,
    ),
>;

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_cs16_hud(
    surface: Res<crate::surface::Hud2dSurface>,
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    weapons: Option<Res<PreparedWeapons>>,
    view: Res<ViewSubject>,
    assets: Res<Cs16HudAssets>,
    feed: Res<CsKillFeed>,
    time: Res<Time>,
    mut fades: Local<Fades>,
    mut parts: PartQuery,
) {
    if !assets.active || !crate::cs_hud::replaces_mw2_hud() {
        return;
    }
    let ps = presented.player(local.0);
    if !surface.is_ready() || view.in_killcam() || ps.is_none() {
        for (_, mut node, ..) in parts.iter_mut() {
            adopt_display(&mut node, Display::None);
        }
        return;
    }
    let scale = surface.height() / VIRTUAL_HEIGHT;
    let screen_w = surface.width() / scale;
    let frame = draw_frame(
        &assets,
        ps,
        &presented,
        &local,
        weapons.as_deref(),
        &time,
        &mut fades,
        screen_w,
    );

    for (part, mut node, image, background, text, font, color) in parts.iter_mut() {
        match *part {
            Cs16Part::Sprite(slot) => {
                let (Some(draw), Some(mut image)) = (frame.sprites.get(usize::from(slot)), image)
                else {
                    adopt_display(&mut node, Display::None);
                    continue;
                };
                let mut rect = draw.cell.rect;
                rect.min.y = (rect.min.y + draw.cut_top).min(rect.max.y);
                place(
                    &mut node,
                    draw.x * scale,
                    (draw.y + draw.cut_top) * scale,
                    Some([rect.width() * scale, rect.height() * scale]),
                );
                if image.image != draw.cell.sheet {
                    image.image = draw.cell.sheet.clone();
                }
                if image.rect != Some(rect) {
                    image.rect = Some(rect);
                }
                if image.color != draw.color {
                    image.color = draw.color;
                }
            }
            Cs16Part::Fill(slot) => {
                let (Some(draw), Some(mut background)) = (frame.fills.get(usize::from(slot)), background)
                else {
                    adopt_display(&mut node, Display::None);
                    continue;
                };
                let [x, y, w, h] = draw.rect;
                place(
                    &mut node,
                    x * scale,
                    y * scale,
                    Some([(w * scale).max(1.0), (h * scale).max(1.0)]),
                );
                if background.0 != draw.color {
                    background.0 = draw.color;
                }
            }
            Cs16Part::FeedRow(row) => {
                if feed.lines.get(usize::from(row)).is_none() {
                    adopt_display(&mut node, Display::None);
                    continue;
                }
                let mut next = Node::clone(&node);
                let mut changed = set_px(
                    &mut next.top,
                    (FEED_TOP + f32::from(row) * FEED_LINE_HEIGHT) * scale,
                ) | set_px(&mut next.right, 0.0);
                if next.display != Display::Flex {
                    next.display = Display::Flex;
                    changed = true;
                }
                if changed {
                    *node = next;
                }
            }
            Cs16Part::FeedAttacker(row) | Cs16Part::FeedVictim(row) => {
                let Some(line) = feed.lines.get(usize::from(row)) else {
                    adopt_display(&mut node, Display::None);
                    continue;
                };
                let name = if let Cs16Part::FeedAttacker(_) = *part {
                    line.attacker.as_ref()
                } else {
                    Some(&line.victim)
                };
                let Some((name, team)) = name else {
                    adopt_display(&mut node, Display::None);
                    continue;
                };
                adopt_display(&mut node, Display::Flex);
                // The killer's name stands 5 pixels off the weapon.
                if let Cs16Part::FeedAttacker(_) = *part {
                    let gap = Val::Px((5.0 * scale).round());
                    if node.margin.right != gap {
                        node.margin.right = gap;
                    }
                }
                if let Some(mut text) = text
                    && text.0 != *name
                {
                    text.0.clone_from(name);
                }
                if let Some(mut color) = color
                    && color.0 != *team
                {
                    color.0 = *team;
                }
                if let Some(mut font) = font {
                    let px = (NAME_TALL * scale).round().max(1.0);
                    if !matches!(font.font_size, FontSize::Px(v) if v == px) {
                        font.font_size = FontSize::Px(px);
                    }
                }
            }
            Cs16Part::FeedWeapon(row) | Cs16Part::FeedHeadshot(row) => {
                let line = feed.lines.get(usize::from(row));
                let sprite = match *part {
                    Cs16Part::FeedWeapon(_) => line.map(|line| line.goldsrc_icon),
                    _ => line.filter(|line| line.headshot).map(|_| "d_headshot"),
                };
                let (Some(line), Some(cell), Some(mut image)) =
                    (line, sprite.and_then(|name| assets.cell(name)), image)
                else {
                    adopt_display(&mut node, Display::None);
                    continue;
                };
                // A team kill shows its weapon green.
                let team_kill = line
                    .attacker
                    .as_ref()
                    .is_some_and(|(_, team)| *team == line.victim.1);
                let color = tint(if team_kill { TEAM_KILL } else { KILL_ORANGE }, 255.0);
                let mut next = Node::clone(&node);
                let mut changed = set_px(&mut next.width, cell.width() * scale)
                    | set_px(&mut next.height, cell.height() * scale);
                if next.display != Display::Flex {
                    next.display = Display::Flex;
                    changed = true;
                }
                if changed {
                    *node = next;
                }
                if image.image != cell.sheet {
                    image.image = cell.sheet.clone();
                }
                if image.rect != Some(cell.rect) {
                    image.rect = Some(cell.rect);
                }
                if image.color != color {
                    image.color = color;
                }
            }
        }
    }
}

/// Lays out this frame's sprites and fills, as the CS 1.6 client draws them.
#[allow(clippy::too_many_arguments)]
fn draw_frame(
    a: &Cs16HudAssets,
    ps: Option<&PlayerState>,
    presented: &PresentedSnapshot,
    local: &LocalPresentClient,
    weapons: Option<&PreparedWeapons>,
    time: &Time,
    fades: &mut Fades,
    screen_w: f32,
) -> Frame {
    let mut out = Frame::default();
    let now = time.elapsed_secs();
    let screen_h = VIRTUAL_HEIGHT;
    let font_h = a.font_height();
    let digit_w = a.digit_width();
    let snap = presented.snapshot();
    let alive = ps.filter(|ps| ps.pm_type < playerstate_iw4::PM_TYPE_DEAD);

    // Round timer: the stopwatch and m:ss, centred above the bottom edge.
    if let Some(left) = crate::cs_hud::round_seconds_left(snap) {
        let (minutes, seconds) = (left / 60, left % 60);
        let rgb = if left > PANIC_S {
            YELLOWISH
        } else {
            fades.panic_time += time.delta_secs();
            if fades.panic_time > seconds as f32 / 40.0 + 0.1 {
                fades.panic_time = 0.0;
                fades.panic_plain = !fades.panic_plain;
            }
            if fades.panic_plain { YELLOWISH } else { REDISH }
        };
        let color = tint(rgb, MIN_ALPHA);
        let watch = a.cell("stopwatch");
        let watch_w = watch.map_or(0.0, Cell::width);
        let colon_w = (digit_w / 2.0).floor();
        let total = watch_w + 2.0 * digit_w + colon_w + 2.0 * digit_w;
        let mut x = ((screen_w - total) / 2.0).floor();
        let y = (screen_h - 1.5 * font_h).floor();
        out.sprite(watch, x, y, color);
        x += watch_w;
        x = if minutes < 10 {
            out.number2(a, x + digit_w, y, false, 1, minutes, color)
        } else {
            out.number2(a, x, y, true, 2, minutes, color)
        };
        let dot_x = x + (colon_w / 2.0).floor();
        out.fill([dot_x, y + (font_h / 4.0).floor(), 2.0, 2.0], color);
        out.fill([dot_x, y + font_h - (font_h / 4.0).floor(), 2.0, 2.0], color);
        x += colon_w;
        out.number2(a, x, y, true, 2, seconds, color);
    }

    let Some(ps) = alive else {
        fades.health = None;
        fades.armor = None;
        fades.ammo = None;
        fades.progress = None;
        return out;
    };
    let y = (screen_h - font_h - font_h / 2.0).floor();

    // Health: the cross and three digits, bottom left.
    let health = ps.health.max(0);
    let since = changed_at(&mut fades.health, health, now);
    let alpha = if health <= 15 {
        255.0
    } else {
        faded(since, now)
    };
    let color = tint(if health > 25 { YELLOWISH } else { HEALTH_RED }, alpha);
    let cross = a.cell("cross");
    let cross_w = cross.map_or(24.0, Cell::width);
    out.sprite(cross, (cross_w / 2.0).floor(), y, color);
    out.number3(a, cross_w + (digit_w / 2.0).floor(), y, health, color);

    // Armour: the vest (or vest and helmet), lit from the bottom by how much is left.
    let armor = ps.cs_armor as i32;
    let since = changed_at(&mut fades.armor, armor, now);
    let color = tint(YELLOWISH, faded(since, now));
    let helmet = ps.cs_helmet != 0;
    let (full, empty) = if helmet {
        (a.cell("suithelmet_full"), a.cell("suithelmet_empty"))
    } else {
        (a.cell("suit_full"), a.cell("suit_empty"))
    };
    let mut x = (screen_w / 5.0).floor();
    out.sprite(full, x, y, color);
    let vest_h = a.cell("suit_full").map_or(24.0, Cell::height);
    if let Some(empty) = empty {
        let cut = (vest_h * (100 - armor.min(100)) as f32 * 0.01).floor();
        if cut < empty.height() {
            out.sprites.push(SpriteDraw {
                cell: empty.clone(),
                x,
                y,
                cut_top: cut,
                color,
            });
        }
        x += empty.width();
    }
    out.number3(a, x, y, armor, color);

    // Money (the bomb mode only): `$` and up to five digits, with the last change above it.
    if crate::cs_hud::bomb_mode(snap) {
        let money = ps.cs_money as i32;
        if let Some(last) = fades.money
            && last != money
        {
            fades.money_delta = money - last;
            fades.money_since = now;
        }
        fades.money = Some(money);
        let left = (MONEY_FADE_S - (now - fades.money_since)).max(0.0);
        let delta = if left > 0.0 { fades.money_delta } else { 0 };
        let interpolate = (MONEY_FADE_S - left) / MONEY_FADE_S;
        // A gain fades from green, a loss from red, into the HUD colour.
        let rgb = match delta {
            1.. => [interpolate * YELLOWISH[0], GREENISH[1], GREENISH[2]],
            ..0 => [
                REDISH[0],
                REDISH[1] + interpolate * (YELLOWISH[1] - REDISH[1]),
                REDISH[2] - interpolate * REDISH[2],
            ],
            0 => YELLOWISH,
        };
        let color = tint(rgb, 255.0 - interpolate * (255.0 - MIN_ALPHA));
        let dollar = a.cell("dollar");
        let dollar_w = dollar.map_or(18.0, Cell::width);
        let dollar_h = dollar.map_or(25.0, Cell::height);
        let x = screen_w - dollar_w * 7.0;
        let money_y = screen_h - 3.0 * font_h;
        if delta != 0 {
            let delta_rgb = if delta < 0 { REDISH } else { GREENISH };
            let delta_color = tint(delta_rgb, 255.0 - interpolate * 255.0);
            let delta_y = money_y - (dollar_h * 1.5).floor();
            let sign = a.cell(if delta < 0 { "minus" } else { "plus" });
            out.sprite(sign, x, delta_y, delta_color);
            out.number2(a, x + dollar_w, delta_y, false, 5, delta.abs(), delta_color);
        }
        out.sprite(dollar, x, money_y, color);
        out.number2(a, x + dollar_w, money_y, false, 5, money, color);
    }

    // Ammo, bottom right: clip | reserve and the ammo icon, or a count and icon.
    match weapons.and_then(|w| ammo_values(ps, w, a, presented, local)) {
        Some(ammo) => {
            let since = changed_at(&mut fades.ammo, (ammo.key, ammo.clip, ammo.count), now);
            let alpha = (AMMO_FADE - (now - since) * FADE_PER_SECOND).max(MIN_ALPHA);
            let color = tint(YELLOWISH, alpha);
            let icon_w = ammo.icon.as_ref().map_or(24.0, Cell::width);
            let ammo_y = y;
            let mut x;
            if let Some(clip) = ammo.clip {
                x = screen_w - 8.0 * digit_w - icon_w;
                x = out.number3(a, x, ammo_y, clip, color);
                let bar_w = (digit_w / 10.0).floor().max(1.0);
                x += (digit_w / 2.0).floor();
                out.fill([x, ammo_y, bar_w, font_h], color);
                x += bar_w + (digit_w / 2.0).floor();
                x = out.number3(a, x, ammo_y, ammo.count, color);
            } else {
                x = screen_w - 4.0 * digit_w - icon_w;
                x = out.number3(a, x, ammo_y, ammo.count, color);
            }
            if let Some(icon) = &ammo.icon {
                let offset = (icon.height() / 8.0).floor();
                out.sprite(Some(icon), x, ammo_y - offset, color);
            }
        }
        None => fades.ammo = None,
    }

    // Status icons, stacked up the left edge from the middle: the C4, the defuse kit.
    let mut icon_y = screen_h / 2.0;
    let carrying = weapons.is_some_and(|w| crate::cs_hud::carries_c4(ps, w));
    for (name, shown) in [("c4", carrying), ("defuser", ps.cs_defuser != 0)] {
        if let (true, Some(icon)) = (shown, a.cell(name)) {
            icon_y -= icon.height() + 5.0;
            out.sprite(Some(icon), 5.0, icon_y, tint(GREENISH, 255.0));
        }
    }

    // Planting or defusing: a framed bar a third up from the bottom.
    if ps.pm_type != playerstate_iw4::PM_TYPE_NORMAL_LINKED {
        fades.progress = None;
    } else if fades.progress.is_none() {
        fades.progress = Some((now, crate::cs_hud::progress_seconds(ps, weapons)));
    }
    if let Some((since, seconds)) = fades.progress {
        let fraction = ((now - since) / seconds).clamp(0.0, 1.0);
        let (x, y, w, h) = (
            (screen_w / 4.0).floor(),
            (screen_h * 2.0 / 3.0).floor(),
            (screen_w / 2.0).floor(),
            10.0,
        );
        let edge = tint(BAR_ORANGE, 255.0);
        out.fill([x, y, w, h], Color::srgba(0.0, 0.0, 0.0, 153.0 / 255.0));
        out.fill([x + 1.0, y, w - 1.0, 1.0], edge);
        out.fill([x, y, 1.0, h - 1.0], edge);
        out.fill([x + w - 1.0, y + 1.0, 1.0, h - 1.0], edge);
        out.fill([x, y + h - 1.0, w - 1.0, 1.0], edge);
        out.fill([x + 2.0, y + 2.0, fraction * (w - 4.0), 6.0], edge);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_sprites_use_the_cs16_names() {
        assert_eq!(kill_sprite("ak47_mp"), "d_ak47");
        assert_eq!(kill_sprite("glock_mp"), "d_glock18");
        assert_eq!(kill_sprite("mp5k_mp"), "d_mp5navy");
        assert_eq!(kill_sprite("beretta_mp"), "d_knife");
        assert_eq!(kill_sprite("frag_grenade_mp"), "d_grenade");
        assert_eq!(kill_sprite("flash_grenade_mp"), "d_flashbang");
        assert_eq!(kill_sprite("rpg_mp"), WORLD_KILL);
    }

    #[test]
    fn numbers_place_digits_like_the_cs16_client() {
        let mut assets = Cs16HudAssets::default();
        for digit in 0..10 {
            let x = digit as f32 * 24.0;
            assets.cells.insert(
                format!("number_{digit}"),
                Cell {
                    sheet: Handle::default(),
                    rect: Rect::new(x, 0.0, x + 20.0, 25.0),
                },
            );
        }
        let xs = |frame: &Frame| frame.sprites.iter().map(|s| s.x).collect::<Vec<_>>();
        let mut frame = Frame::default();
        // 7 health: two blank places, then the digit.
        assert_eq!(frame.number3(&assets, 0.0, 0.0, 7, Color::WHITE), 60.0);
        assert_eq!(xs(&frame), [40.0]);
        // $800 in five places, right-aligned, no padding zeros.
        let mut frame = Frame::default();
        assert_eq!(frame.number2(&assets, 0.0, 0.0, false, 5, 800, Color::WHITE), 100.0);
        assert_eq!(xs(&frame), [80.0, 60.0, 40.0]);
        // Seconds pad to two places.
        let mut frame = Frame::default();
        frame.number2(&assets, 0.0, 0.0, true, 2, 5, Color::WHITE);
        assert_eq!(xs(&frame), [20.0, 0.0]);
    }
}
