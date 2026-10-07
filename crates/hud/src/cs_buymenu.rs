//! Counter-Strike buy menu (`buymenu`, B). Drawn from the installed game at runtime, nothing
//! copied:
//!
//! - **VGUI** (`_vgui_menus 1`): the game's own buy window — its `resource/ui` layout pages
//!   (CS:S `buymenu_ct/ter.res`, CS 1.6 `MainBuyMenu.res`, then `Buy<category>[_CT|_TER].res`),
//!   the weapon panels of `classes/<weapon>.res`, the text of `resource/cstrike_english.txt` and
//!   the `gfx/vgui` pictures (CS:S's pack, CS 1.6's `.tga` files). The mouse picks (the panel
//!   under it shows the weapon), the number keys too; like CS:S you stand still while it is open.
//! - **Classic** (`_vgui_menus 0`): CS 1.6's old numbered text menu at the left of the screen,
//!   its menus read from 1.6's `titles.txt` (our own wording when 1.6 isn't installed). Only the
//!   number keys pick; you keep moving and looking around.
//!
//! The menu opens in a buy zone during the buy time (the bomb mode), anywhere in the other modes.
//! A purchase goes through the same server rules as the `buy` command.

use std::collections::HashMap;
use std::path::PathBuf;

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::text::{FontSize, Justify, LineBreak};
use frame::ViewSubject;
use net::{ClientActionInput, LocalPresentClient, PresentedSnapshot};
use playerstate_iw4::cs_buy::{TIME, ZONE};
use weapon_iw4::cs::{self, BuyTeam};

use crate::cs_hud::replaces_mw2_hud;

/// The buy menu's state, shared with the input layer (the VGUI window owns the keyboard and
/// mouse while open) and the console (which sends the purchases).
#[derive(Resource, Default)]
pub struct CsBuyMenu {
    screen: Option<Screen>,
    /// Escape closed the menu this frame; the pause menu must not take it as well.
    ate_escape: bool,
    /// Buy names picked, for the console to send.
    purchases: Vec<&'static str>,
    /// `menuselect` picks waiting for the next frame.
    selected: Vec<u8>,
    /// A refusal shown in the middle of the screen, until `until_s`.
    message: Option<(String, f32)>,
    /// Free-for-all has no sides: which side's guns the menu shows there (9 switches).
    ffa_counter_terrorist: bool,
    data: Option<Data>,
    pages: HashMap<String, Option<Vec<Control>>>,
    images: HashMap<String, Option<Handle<Image>>>,
    fonts: Option<Fonts>,
    shown: Option<View>,
}

impl CsBuyMenu {
    /// The VGUI window is open: gameplay input stops and the cursor is free.
    #[must_use]
    pub fn captures_input(&self) -> bool {
        matches!(self.screen, Some(Screen::Vgui(_)))
    }

    /// Escape belongs to the buy menu this frame (it closes the menu, not opens the pause menu).
    #[must_use]
    pub fn eats_escape(&self) -> bool {
        self.screen.is_some() || self.ate_escape
    }

    /// Picks item `key` (1-9, 10 for 0) of the open menu, as CS's `menuselect`.
    pub fn select(&mut self, key: u8) {
        self.selected.push(key);
    }

    /// The buy names picked since the last call.
    pub fn take_purchases(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut self.purchases)
    }

    fn close(&mut self) {
        self.screen = None;
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Screen {
    /// A VGUI page, by its path in the install.
    Vgui(String),
    /// A classic text menu, by its `titles.txt` name.
    Classic(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Look {
    Source,
    GoldSrc,
}

/// Where the menu's files come from.
enum Install {
    /// Counter-Strike: Source: its pack, and the `cstrike` folder beside it.
    Source {
        pack: mdl_source::Vpk,
        dir: PathBuf,
    },
    /// Counter-Strike 1.6: the `cstrike` folder.
    GoldSrc { dir: PathBuf },
}

impl Install {
    fn find() -> Option<Self> {
        if let Some(pak) = asset_transport::find_css_pak() {
            let dir = pak.parent()?.to_path_buf();
            let pack = mdl_source::Vpk::open(&pak).ok()?;
            return Some(Self::Source { pack, dir });
        }
        asset_transport::find_cstrike().map(|dir| Self::GoldSrc { dir })
    }

    fn look(&self) -> Look {
        match self {
            Self::Source { .. } => Look::Source,
            Self::GoldSrc { .. } => Look::GoldSrc,
        }
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let path = path.replace('\\', "/").to_ascii_lowercase();
        match self {
            Self::Source { pack, dir } => pack
                .read(&path)
                .or_else(|| std::fs::read(dir.join(&path)).ok()),
            Self::GoldSrc { dir } => std::fs::read(dir.join(&path)).ok(),
        }
    }

    fn exists(&self, path: &str) -> bool {
        let path = path.replace('\\', "/").to_ascii_lowercase();
        match self {
            Self::Source { pack, dir } => pack.contains(&path) || dir.join(&path).is_file(),
            Self::GoldSrc { dir } => dir.join(&path).is_file(),
        }
    }

    /// A VGUI picture (`gfx/vgui/M4A1`) as RGBA.
    fn image(&self, name: &str) -> Option<(u32, u32, Vec<u8>)> {
        match self {
            Self::Source { .. } => {
                let bytes = self.read(&format!("materials/vgui/{name}.vtf"))?;
                let image = mdl_source::vtf::decode(&bytes).ok()?;
                Some((image.width, image.height, image.rgba))
            }
            Self::GoldSrc { .. } => decode_tga(&self.read(&format!("{name}.tga"))?),
        }
    }
}

/// What the menu reads once, the first time it opens.
struct Data {
    install: Option<Install>,
    strings: HashMap<String, String>,
    /// CS 1.6's `titles.txt` menus, by lowercase name (our own when 1.6 isn't there).
    classic: HashMap<String, Vec<String>>,
}

impl Data {
    fn load() -> Self {
        let install = Install::find();
        let strings = install
            .as_ref()
            .and_then(|install| install.read("resource/cstrike_english.txt"))
            .map(|bytes| parse_localization(&bytes))
            .unwrap_or_default();
        let classic = cs16_dir()
            .and_then(|dir| std::fs::read(dir.join("titles.txt")).ok())
            .map(|bytes| parse_titles(&String::from_utf8_lossy(&bytes)))
            .filter(|menus| menus.contains_key("buy"))
            .unwrap_or_else(own_classic_menus);
        diag::info!(
            World,
            "cs buy menu: VGUI from {}, {} strings, classic menus {}",
            match install.as_ref().map(Install::look) {
                Some(Look::Source) => "CS:S",
                Some(Look::GoldSrc) => "CS 1.6",
                None => "nowhere (classic only)",
            },
            strings.len(),
            if cs16_dir().is_some() {
                "from CS 1.6"
            } else {
                "built in"
            }
        );
        Self {
            install,
            strings,
            classic,
        }
    }

    /// `#Token` from the game's text, else the text itself.
    fn localize(&self, text: &str) -> String {
        text.strip_prefix('#')
            .and_then(|key| self.strings.get(&key.to_ascii_lowercase()))
            .cloned()
            .unwrap_or_else(|| text.to_owned())
    }

    fn say(&self, key: &str, fallback: &str) -> String {
        self.strings
            .get(&key.to_ascii_lowercase())
            .cloned()
            .unwrap_or_else(|| fallback.to_owned())
    }
}

/// CS 1.6's `cstrike` folder, for `titles.txt`, even when CS:S draws everything else.
fn cs16_dir() -> Option<PathBuf> {
    asset_transport::steam::cstrike_env_override()
        .map(PathBuf::from)
        .or_else(|| asset_transport::game_paths::saved(asset_transport::GameFolder::Cs16))
        .filter(|dir| dir.join("titles.txt").is_file())
}

// ---------------------------------------------------------------------------------------------
// Reading the game's files.

/// Valve localization (`"lang" { "Tokens" { "key" "value" ... } }`), UTF-16 or UTF-8, with
/// `\"` and `\n` escapes and `[$PLATFORM]` conditions.
fn parse_localization(bytes: &[u8]) -> HashMap<String, String> {
    let text = if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    let tokens = localization_tokens(&text);
    let mut out = HashMap::new();
    let Some(start) = tokens
        .windows(2)
        .position(|pair| pair[0].eq_ignore_ascii_case("tokens") && pair[1] == "{")
    else {
        return out;
    };
    let mut rest = tokens[start + 2..]
        .iter()
        .filter(|token| !token.starts_with("[$") && !token.starts_with("[!$"));
    while let Some(key) = rest.next() {
        if key == "}" {
            break;
        }
        let Some(value) = rest.next() else {
            break;
        };
        out.insert(key.to_ascii_lowercase(), value.clone());
    }
    out
}

fn localization_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(other) => s.push(other),
                            None => {}
                        },
                        '\r' => {}
                        other => s.push(other),
                    }
                }
                out.push(s);
            }
            '{' | '}' => out.push(c.to_string()),
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            c if c.is_whitespace() => {}
            c => {
                let mut s = c.to_string();
                while let Some(&next) = chars.peek() {
                    if next.is_whitespace() || matches!(next, '"' | '{' | '}') {
                        break;
                    }
                    s.push(next);
                    chars.next();
                }
                out.push(s);
            }
        }
    }
    out
}

/// GoldSrc `titles.txt`: `Name` then a `{ ... }` block of lines.
fn parse_titles(text: &str) -> HashMap<String, Vec<String>> {
    let mut out = HashMap::new();
    let mut name: Option<String> = None;
    let mut body: Option<Vec<String>> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(lines) = body.as_mut() {
            if line.trim() == "}" {
                if let (Some(name), Some(lines)) = (name.take(), body.take()) {
                    out.insert(name.to_ascii_lowercase(), lines);
                }
            } else {
                lines.push(line.to_owned());
            }
        } else if line.trim() == "{" {
            body = Some(Vec::new());
        } else if !line.trim().is_empty() && !line.trim_start().starts_with("//") {
            name = Some(line.trim().to_owned());
        }
    }
    out
}

/// An uncompressed or RLE true-colour TGA (CS 1.6's `gfx/vgui` pictures) as RGBA, top row first.
fn decode_tga(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let header = bytes.get(..18)?;
    let (id_len, colour_map, kind) = (usize::from(header[0]), header[1], header[2]);
    if colour_map != 0 || !matches!(kind, 2 | 10) {
        return None;
    }
    let width = usize::from(u16::from_le_bytes([header[12], header[13]]));
    let height = usize::from(u16::from_le_bytes([header[14], header[15]]));
    let depth = usize::from(header[16]) / 8;
    if !matches!(depth, 3 | 4) || width == 0 || height == 0 {
        return None;
    }
    let top_first = header[17] & 0x20 != 0;
    let mut data = bytes.get(18 + id_len..)?;
    let pixel = |p: &[u8]| [p[2], p[1], p[0], if depth == 4 { p[3] } else { 255 }];
    let total = width * height;
    let mut rgba = Vec::with_capacity(total * 4);
    if kind == 2 {
        for p in data.chunks_exact(depth).take(total) {
            rgba.extend(pixel(p));
        }
    } else {
        while rgba.len() < total * 4 {
            let (&packet, rest) = data.split_first()?;
            data = rest;
            let count = usize::from(packet & 0x7F) + 1;
            if packet & 0x80 != 0 {
                let p = pixel(data.get(..depth)?);
                data = &data[depth..];
                for _ in 0..count {
                    rgba.extend(p);
                }
            } else {
                for _ in 0..count {
                    rgba.extend(pixel(data.get(..depth)?));
                    data = &data[depth..];
                }
            }
        }
    }
    if rgba.len() < total * 4 {
        return None;
    }
    rgba.truncate(total * 4);
    if !top_first {
        let row = width * 4;
        let mut flipped = Vec::with_capacity(rgba.len());
        for y in (0..height).rev() {
            flipped.extend_from_slice(&rgba[y * row..(y + 1) * row]);
        }
        rgba = flipped;
    }
    Some((width as u32, height as u32, rgba))
}

// ---------------------------------------------------------------------------------------------
// VGUI pages.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Label,
    Button,
    Image,
    Divider,
    /// Where the hovered button's `classes/<name>.res` panel goes.
    ItemInfo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Align {
    West,
    Center,
    East,
}

/// One control of a `.res` page, in its 640x480 layout units.
#[derive(Clone, Debug, PartialEq)]
struct Control {
    kind: Kind,
    name: String,
    rect: [f32; 4],
    text: String,
    /// The number key that presses it (`&1` in its text; 10 is 0).
    hotkey: Option<u8>,
    align: Align,
    title: bool,
    /// `dulltext`: the dimmer label orange.
    dull: bool,
    command: String,
    image: Option<String>,
    fill: Option<[u8; 4]>,
}

/// A layout position: plain, `r<n>` from the right/bottom, `c<n>` from the centre.
fn coord(value: Option<&str>, size: f32) -> f32 {
    let value = value.unwrap_or("0").trim();
    let number = |text: &str| text.trim().parse::<f32>().unwrap_or(0.0);
    if let Some(rest) = value.strip_prefix(['r', 'R']) {
        size - number(rest)
    } else if let Some(rest) = value.strip_prefix(['c', 'C']) {
        size / 2.0 + number(rest)
    } else {
        number(value)
    }
}

/// A scheme colour by name, or `r g b a`.
fn scheme_color(value: &str) -> Option<[u8; 4]> {
    match value.to_ascii_lowercase().as_str() {
        "windowbg" => Some([0, 0, 0, 200]),
        "black" => Some([0, 0, 0, 255]),
        "transparentblack" => Some([0, 0, 0, 196]),
        "blank" => None,
        other => {
            let parts: Vec<u8> = other
                .split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect();
            (parts.len() == 4).then(|| [parts[0], parts[1], parts[2], parts[3]])
        }
    }
}

/// A `.res` file's controls as lowercase key/value lists, with the `#base` files it names merged
/// under them (CS:S's weapon panels only override `base_weapon_small.res`).
fn res_blocks(install: &Install, path: &str, depth: usize) -> Vec<(String, Vec<(String, String)>)> {
    use mdl_source::kv::Value;
    let Some(bytes) = install.read(path) else {
        return Vec::new();
    };
    let root = mdl_source::kv::parse(&String::from_utf8_lossy(&bytes));
    let mut blocks = Vec::new();
    for (key, value) in root.entries() {
        if let Value::Text(base) = value
            && key.eq_ignore_ascii_case("#base")
            && depth < 4
        {
            let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
            let base = if dir.is_empty() {
                base.clone()
            } else {
                format!("{dir}/{base}")
            };
            merge_blocks(&mut blocks, res_blocks(install, &base, depth + 1));
        }
    }
    let Some((_, page)) = root.entries().find(|(_, v)| matches!(v, Value::Block(_))) else {
        return blocks;
    };
    let own = page
        .entries()
        .filter_map(|(name, control)| match control {
            Value::Block(items) => Some((
                name.to_ascii_lowercase(),
                items
                    .iter()
                    .filter_map(|(k, v)| match v {
                        Value::Text(text) => Some((k.to_ascii_lowercase(), text.clone())),
                        Value::Block(_) => None,
                    })
                    .collect(),
            )),
            Value::Text(_) => None,
        })
        .collect();
    merge_blocks(&mut blocks, own);
    blocks
}

fn merge_blocks(
    into: &mut Vec<(String, Vec<(String, String)>)>,
    from: Vec<(String, Vec<(String, String)>)>,
) {
    for (name, keys) in from {
        let Some((_, existing)) = into.iter_mut().find(|(have, _)| *have == name) else {
            into.push((name, keys));
            continue;
        };
        for (key, value) in keys {
            match existing.iter_mut().find(|(have, _)| *have == key) {
                Some(slot) => slot.1 = value,
                None => existing.push((key, value)),
            }
        }
    }
}

fn load_controls(data: &Data, path: &str) -> Option<Vec<Control>> {
    let install = data.install.as_ref()?;
    let blocks = res_blocks(install, path, 0);
    if blocks.is_empty() {
        return None;
    }
    let mut controls = Vec::new();
    for (block, keys) in &blocks {
        let get = |key: &str| {
            keys.iter()
                .find(|(have, _)| have == key)
                .map(|(_, value)| value.as_str())
        };
        if get("visible") == Some("0") {
            continue;
        }
        let name = get("fieldname").unwrap_or(block).to_ascii_lowercase();
        let kind = match get("controlname").unwrap_or_default().to_ascii_lowercase().as_str() {
            "label" => Kind::Label,
            "mouseoverpanelbutton" | "button" => Kind::Button,
            "imagepanel" => Kind::Image,
            "divider" => Kind::Divider,
            "panel" if name == "iteminfo" => Kind::ItemInfo,
            _ => continue,
        };
        let rect = [
            coord(get("xpos"), 640.0),
            coord(get("ypos"), 480.0),
            coord(get("wide"), 640.0),
            coord(get("tall"), 480.0),
        ];
        // Off-screen panels (the main page's item info) are never seen.
        if rect[0] >= 640.0 || rect[1] >= 480.0 {
            continue;
        }
        let image = get("image").map(str::to_owned);
        if image.as_deref().is_some_and(|i| i.contains("market_sticker")) {
            continue;
        }
        let raw = data.localize(get("labeltext").unwrap_or_default());
        let hotkey = raw
            .find('&')
            .and_then(|at| raw[at + 1..].chars().next())
            .and_then(|c| c.to_digit(10))
            .map(|d| if d == 0 { 10 } else { d as u8 });
        controls.push(Control {
            kind,
            name,
            rect,
            text: raw.replace('&', ""),
            hotkey,
            align: match get("textalignment").map(str::to_ascii_lowercase).as_deref() {
                Some("center") => Align::Center,
                Some("east") => Align::East,
                _ => Align::West,
            },
            title: get("font").is_some_and(|f| {
                f.eq_ignore_ascii_case("MenuTitle") || f.eq_ignore_ascii_case("Title")
            }),
            dull: get("dulltext") == Some("1"),
            command: get("command").unwrap_or_default().to_owned(),
            image,
            fill: get("fillcolor").and_then(scheme_color),
        });
    }
    Some(controls)
}

/// The page a category button's `Resource/UI/BuyRifles[_CT].res` means for this side.
fn page_path(install: &Install, command: &str, terrorist: bool) -> Option<String> {
    let lower = command.replace('\\', "/").to_ascii_lowercase();
    let stem = lower.strip_suffix(".res")?;
    let base = stem
        .strip_suffix("_ct")
        .or_else(|| stem.strip_suffix("_ter"))
        .unwrap_or(stem);
    let side = if terrorist { "_ter" } else { "_ct" };
    [format!("{base}{side}.res"), format!("{base}.res")]
        .into_iter()
        .find(|path| install.exists(path))
}

/// The first page: CS:S's `buymenu_<side>.res`, CS 1.6's `MainBuyMenu.res`.
fn main_page(install: &Install, terrorist: bool) -> Option<String> {
    let side = if terrorist { "ter" } else { "ct" };
    [
        format!("resource/ui/buymenu_{side}.res"),
        "resource/ui/mainbuymenu.res".to_owned(),
    ]
    .into_iter()
    .find(|path| install.exists(path))
}

// ---------------------------------------------------------------------------------------------
// The classic menus (CS 1.6 `MenuSelect` for the buy menus).

#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    Open(String),
    Buy(&'static str),
    Close,
    /// Not sold here (ammo, night vision, defuse kit, shield).
    Unavailable,
    /// Free-for-all: the other side's guns.
    SwitchSide,
}

const CT_PISTOLS: [&str; 5] = ["glock", "usp", "p228", "deagle", "fiveseven"];
const T_PISTOLS: [&str; 5] = ["glock", "usp", "p228", "deagle", "elite"];
const SHOTGUNS: [&str; 2] = ["m3", "xm1014"];
const CT_SMGS: [&str; 4] = ["tmp", "mp5", "ump45", "p90"];
const T_SMGS: [&str; 4] = ["mac10", "mp5", "ump45", "p90"];
const T_RIFLES: [&str; 6] = ["galil", "ak47", "scout", "sg552", "awp", "g3sg1"];
const CT_RIFLES: [&str; 6] = ["famas", "scout", "m4a1", "aug", "sg550", "awp"];
const MACHINE_GUNS: [&str; 1] = ["m249"];
const EQUIPMENT: [&str; 5] = ["vest", "vesthelm", "flashbang", "hegrenade", "smokegrenade"];

fn classic_items(menu: &str, terrorist: bool) -> Option<&'static [&'static str]> {
    Some(match menu.to_ascii_lowercase().as_str() {
        "ct_buypistol" | "t_buypistol" if terrorist => &T_PISTOLS,
        "ct_buypistol" | "t_buypistol" => &CT_PISTOLS,
        "buyshotgun" => &SHOTGUNS,
        "ct_buysubmachinegun" | "t_buysubmachinegun" if terrorist => &T_SMGS,
        "ct_buysubmachinegun" | "t_buysubmachinegun" => &CT_SMGS,
        "ct_buyrifle" | "t_buyrifle" if terrorist => &T_RIFLES,
        "ct_buyrifle" | "t_buyrifle" => &CT_RIFLES,
        "buymachinegun" => &MACHINE_GUNS,
        "ct_buyitem" | "t_buyitem" => &EQUIPMENT,
        _ => return None,
    })
}

fn classic_action(menu: &str, key: u8, terrorist: bool, ffa: bool) -> Option<Action> {
    let side = |t: &'static str, ct: &'static str| if terrorist { t } else { ct };
    if key == 10 {
        return Some(Action::Close);
    }
    if menu.eq_ignore_ascii_case("buy") {
        return Some(match key {
            1 => Action::Open(side("T_BuyPistol", "CT_BuyPistol").to_owned()),
            2 => Action::Open("BuyShotgun".to_owned()),
            3 => Action::Open(side("T_BuySubMachineGun", "CT_BuySubMachineGun").to_owned()),
            4 => Action::Open(side("T_BuyRifle", "CT_BuyRifle").to_owned()),
            5 => Action::Open("BuyMachineGun".to_owned()),
            6 | 7 => Action::Unavailable,
            8 => Action::Open(side("T_BuyItem", "CT_BuyItem").to_owned()),
            9 if ffa => Action::SwitchSide,
            _ => return None,
        });
    }
    let items = classic_items(menu, terrorist)?;
    match items.get(usize::from(key) - 1) {
        Some(name) => Some(Action::Buy(name)),
        // The equipment menu's night vision, defuse kit and shield.
        None if items.len() == EQUIPMENT.len() && key <= 8 => Some(Action::Unavailable),
        None => None,
    }
}

/// Our own wording of the classic menus, for when CS 1.6 isn't installed.
fn own_classic_menus() -> HashMap<String, Vec<String>> {
    let mut out = HashMap::new();
    out.insert(
        "buy".to_owned(),
        [
            "\\yBuy Item\\w",
            "",
            "1. Pistols",
            "2. Shotguns",
            "3. Sub-Machine Guns",
            "4. Rifles",
            "5. Machine Guns",
            "",
            "6. Primary Ammo",
            "7. Secondary Ammo",
            "",
            "8. Equipment",
            "",
            "0. Exit",
        ]
        .map(str::to_owned)
        .to_vec(),
    );
    let menus: [(&str, &str, &[&str]); 10] = [
        ("CT_BuyPistol", "Buy Pistol", &CT_PISTOLS),
        ("T_BuyPistol", "Buy Pistol", &T_PISTOLS),
        ("BuyShotgun", "Buy Shotgun", &SHOTGUNS),
        ("CT_BuySubMachineGun", "Buy Sub-Machine Gun", &CT_SMGS),
        ("T_BuySubMachineGun", "Buy Sub-Machine Gun", &T_SMGS),
        ("CT_BuyRifle", "Buy Rifle", &CT_RIFLES),
        ("T_BuyRifle", "Buy Rifle", &T_RIFLES),
        ("BuyMachineGun", "Buy Machine Gun", &MACHINE_GUNS),
        ("CT_BuyItem", "Buy Equipment", &EQUIPMENT),
        ("T_BuyItem", "Buy Equipment", &EQUIPMENT),
    ];
    for (name, title, items) in menus {
        let mut lines = vec![format!("\\y{title}\\R$   Cost"), String::new()];
        for (n, item) in items.iter().enumerate() {
            lines.push(format!(
                "\\w{}. {}\\y\\R{}",
                n + 1,
                item_name(item),
                cs::buy_price(item).unwrap_or(0)
            ));
        }
        lines.push(String::new());
        lines.push("\\w0. Exit".to_owned());
        out.insert(name.to_ascii_lowercase(), lines);
    }
    out
}

fn item_name(name: &str) -> &'static str {
    match name {
        "vest" => "Kevlar Vest",
        "vesthelm" => "Kevlar Vest & Helmet",
        other => cs::cs_weapon_by_name(other)
            .and_then(|_| {
                cs::CS_WEAPONS
                    .iter()
                    .position(|w| w.name == other)
                    .and_then(|i| cs::display_name(i as u8 + 1))
            })
            .or_else(|| {
                cs::CS_GRENADES
                    .iter()
                    .position(|g| g.name == other)
                    .and_then(|i| cs::display_name(cs::CS_GRENADE_INDEX + i as u8))
            })
            .unwrap_or("?"),
    }
}

// ---------------------------------------------------------------------------------------------
// Drawing.

const ORANGE: [u8; 4] = [255, 176, 0, 255];
/// Disabled text and button borders (`LightOrange`).
const LIGHT_ORANGE: [u8; 4] = [188, 112, 0, 128];
/// `dulltext` labels (`LabelDimText`).
const DIM_ORANGE: [u8; 4] = [255, 176, 0, 164];
const FRAME_BG: [u8; 4] = [0, 0, 0, 196];
const BUTTON_BG: [u8; 4] = [0, 0, 0, 64];
/// CS:S `Button.ArmedBgColor` ("Red"); CS 1.6 lights the button orange (`SelectionBG`).
const SOURCE_ARMED_BG: [u8; 4] = [192, 28, 0, 140];
const GOLDSRC_ARMED_BG: [u8; 4] = [255, 176, 0, 100];
/// GoldSrc menu text colours: `\w`, `\y`, `\d`, `\r`.
const MENU_WHITE: [u8; 4] = [255, 255, 255, 255];
const MENU_YELLOW: [u8; 4] = [255, 210, 64, 255];
const MENU_GRAY: [u8; 4] = [100, 100, 100, 255];
const MENU_RED: [u8; 4] = [210, 24, 0, 255];

const MESSAGE_SECONDS: f32 = 2.5;
/// The bomb mode's buy time (`scr_cs_buytime`), for the "seconds have passed" message.
const BUY_TIME_SECONDS: i32 = 20;

fn rgba([r, g, b, a]: [u8; 4]) -> Color {
    Color::srgba_u8(r, g, b, a)
}

#[derive(Clone)]
struct Fonts {
    regular: Handle<Font>,
    bold: Handle<Font>,
    menu: Handle<Font>,
}

fn system_font(fonts: &mut Assets<Font>, names: &[&str]) -> Handle<Font> {
    let dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:/Windows"))
        .join("Fonts");
    names
        .iter()
        .find_map(|name| std::fs::read(dir.join(name)).ok())
        .map(|bytes| fonts.add(Font::from_bytes(bytes)))
        .unwrap_or_default()
}

/// What is on screen; the nodes are rebuilt only when it changes.
#[derive(Clone, Debug, PartialEq)]
struct View {
    size: [u32; 2],
    screen: Option<Screen>,
    hover: Option<String>,
    money: i32,
    terrorist: bool,
    ffa: bool,
    economy: bool,
    message: Option<String>,
}

#[derive(Component)]
pub(crate) struct CsBuyRoot;

pub(crate) fn spawn_cs_buymenu(
    mut commands: Commands,
    roots: Query<Entity, With<crate::plugin::HudRoot>>,
    existing: Query<(), With<CsBuyRoot>>,
) {
    if !replaces_mw2_hud() || !existing.is_empty() {
        return;
    }
    let Ok(root) = roots.single() else {
        return;
    };
    commands.entity(root).with_children(|root| {
        root.spawn((
            CsBuyRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                display: Display::None,
                ..default()
            },
            ZIndex(60),
        ));
    });
}

/// Who the local player buys as.
struct Buyer {
    alive: bool,
    bits: u32,
    money: i32,
    terrorist: bool,
    ffa: bool,
    economy: bool,
    armor: u32,
    helmet: bool,
}

impl Buyer {
    fn may_buy(&self, name: &str) -> bool {
        let side = match cs::buy_team(name) {
            BuyTeam::Both => true,
            BuyTeam::Terrorists => self.terrorist || !self.economy,
            BuyTeam::CounterTerrorists => !self.terrorist || !self.economy,
        };
        side && cs::buy_price(name).is_some_and(|price| price <= self.money)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_cs_buymenu(
    mut commands: Commands,
    surface: Res<crate::surface::Hud2dSurface>,
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    view_subject: Res<ViewSubject>,
    settings: Res<frame::GameSettings>,
    (keys, mouse, windows, time): (
        Res<ButtonInput<KeyCode>>,
        Res<ButtonInput<MouseButton>>,
        Query<&Window, With<bevy::window::PrimaryWindow>>,
        Res<Time>,
    ),
    mut actions: Option<ResMut<ClientActionInput>>,
    mut menu: ResMut<CsBuyMenu>,
    mut fonts: ResMut<Assets<Font>>,
    mut images: ResMut<Assets<Image>>,
    mut roots: Query<(Entity, &mut Node), With<CsBuyRoot>>,
) {
    if !replaces_mw2_hud() {
        return;
    }
    let Ok((root, mut node)) = roots.single_mut() else {
        return;
    };
    let menu = &mut *menu;
    menu.ate_escape = false;
    let toggle = actions
        .as_mut()
        .is_some_and(|a| std::mem::take(&mut a.client.buy_menu));
    let mut picks: Vec<u8> = actions
        .as_mut()
        .and_then(|a| a.menu_keys.as_mut().map(std::mem::take))
        .unwrap_or_default();
    picks.append(&mut menu.selected);
    let now = time.elapsed_secs();

    let snap = presented.snapshot();
    let ps = presented.player(local.0);
    let meta = snap.and_then(|s| s.meta.for_client(local.0));
    let team = meta.map_or(0, |m| m.client_state_team);
    let economy = snap.is_some_and(|s| {
        s.meta
            .objectives
            .server_info
            .iter()
            .any(|(name, _)| name == "cs_attackers")
    });
    let ffa = snap.is_some_and(|s| !s.meta.kind.is_team());
    let terrorist = if ffa {
        !menu.ffa_counter_terrorist
    } else {
        crate::cs_hud::is_terrorist(snap, team) == Some(true)
    };
    let buyer = Buyer {
        alive: ps.is_some_and(|ps| ps.pm_type < playerstate_iw4::PM_TYPE_DEAD)
            && meta.is_some_and(|m| m.lifecycle == sim::ClientLifecycle::Alive)
            && !view_subject.in_killcam()
            && (ffa || team == entity_iw4::TEAM_AXIS || team == entity_iw4::TEAM_ALLIES),
        bits: ps.map_or(0, |ps| ps.cs_buy),
        money: ps.map_or(0, |ps| ps.cs_money as i32),
        terrorist,
        ffa,
        economy,
        armor: ps.map_or(0, |ps| ps.cs_armor),
        helmet: ps.is_some_and(|ps| ps.cs_helmet != 0),
    };

    if menu.data.is_none() && (toggle || menu.screen.is_some()) {
        menu.data = Some(Data::load());
    }
    if menu.fonts.is_none() {
        menu.fonts = Some(Fonts {
            regular: system_font(&mut fonts, &["verdana.ttf", "tahoma.ttf", "arial.ttf"]),
            bold: system_font(&mut fonts, &["verdanab.ttf", "tahomabd.ttf", "arialbd.ttf"]),
            menu: system_font(&mut fonts, &["trebuc.ttf", "tahoma.ttf", "arial.ttf"]),
        });
    }

    // Open and close.
    if toggle {
        if menu.screen.is_some() {
            menu.close();
        } else if buyer.alive {
            let data = menu.data.as_ref().expect("loaded above");
            if buyer.bits & TIME == 0 {
                let text = data
                    .say(
                        "Cstrike_TitlesTXT_Cant_buy",
                        "%s1 seconds have passed.\nYou can't buy anything now!",
                    )
                    .replace("%s1", &BUY_TIME_SECONDS.to_string());
                menu.message = Some((text, now + MESSAGE_SECONDS));
            } else if buyer.bits & ZONE == 0 {
                menu.message = Some(("You are not in a buy zone.".to_owned(), now + MESSAGE_SECONDS));
            } else {
                let vgui = settings.vgui_menus
                    && data
                        .install
                        .as_ref()
                        .and_then(|install| main_page(install, terrorist))
                        .is_some();
                menu.screen = Some(if vgui {
                    Screen::Vgui(
                        data.install
                            .as_ref()
                            .and_then(|install| main_page(install, terrorist))
                            .unwrap_or_default(),
                    )
                } else {
                    Screen::Classic("Buy".to_owned())
                });
            }
        }
    }
    if menu.screen.is_some() && (!buyer.alive || buyer.bits != ZONE | TIME) {
        menu.close();
    }
    if menu.screen.is_some() && keys.just_pressed(KeyCode::Escape) {
        menu.close();
        menu.ate_escape = true;
    }
    if let Some(actions) = actions.as_mut() {
        let want = menu.screen.is_some();
        if want != actions.menu_keys.is_some() {
            actions.menu_keys = want.then(Vec::new);
        }
    }

    // The VGUI window: the mouse, and the number keys straight from the keyboard (gameplay
    // input, the slot binds included, is held while it is open).
    let unit = (surface.height() / 480.0).max(0.01);
    let mut hover = None;
    let mut clicked = None;
    if let Some(Screen::Vgui(page)) = menu.screen.clone() {
        for (key, digit) in DIGIT_KEYS {
            if keys.just_pressed(key) {
                picks.push(digit);
            }
        }
        let controls = page_controls(menu, &page);
        if let Some(cursor) = windows.single().ok().and_then(Window::cursor_position) {
            let (x, y) = (cursor.x / unit, cursor.y / unit);
            hover = controls
                .iter()
                .chain(ffa_button(menu, &page, buyer.ffa).iter())
                .find(|c| {
                    c.kind == Kind::Button
                        && x >= c.rect[0]
                        && x < c.rect[0] + c.rect[2]
                        && y >= c.rect[1]
                        && y < c.rect[1] + c.rect[3]
                })
                .cloned();
        }
        if mouse.just_pressed(MouseButton::Left) {
            clicked = hover.clone();
        }
    }

    // Picks.
    for key in picks {
        let Some(screen) = menu.screen.clone() else {
            break;
        };
        let action = match &screen {
            Screen::Classic(name) => classic_action(name, key, buyer.terrorist, buyer.ffa),
            Screen::Vgui(page) => {
                let controls = page_controls(menu, page);
                controls
                    .iter()
                    .chain(ffa_button(menu, page, buyer.ffa).iter())
                    .find(|c| c.kind == Kind::Button && c.hotkey == Some(key))
                    .map(|c| vgui_action(menu, c, buyer.terrorist))
            }
        };
        if let Some(action) = action {
            act(menu, action, &buyer, now);
        }
    }
    if let Some(control) = clicked
        && let Some(Screen::Vgui(_)) = &menu.screen
    {
        let action = vgui_action(menu, &control, buyer.terrorist);
        act(menu, action, &buyer, now);
    }
    if menu.message.as_ref().is_some_and(|(_, until)| now >= *until) {
        menu.message = None;
    }

    // Draw.
    let view = View {
        size: [surface.width() as u32, surface.height() as u32],
        screen: menu.screen.clone(),
        hover: hover.map(|c| c.name),
        money: buyer.money,
        terrorist: buyer.terrorist,
        ffa: buyer.ffa,
        economy: buyer.economy,
        message: menu.message.as_ref().map(|(text, _)| text.clone()),
    };
    let visible = surface.is_ready() && (view.screen.is_some() || view.message.is_some());
    let display = if visible { Display::Flex } else { Display::None };
    if node.display != display {
        node.display = display;
    }
    if !visible || menu.shown.as_ref() == Some(&view) {
        return;
    }
    commands.entity(root).despawn_related::<Children>();
    let fonts = menu.fonts.clone().expect("loaded above");
    match view.screen.clone() {
        Some(Screen::Vgui(page)) => {
            let look = menu
                .data
                .as_ref()
                .and_then(|d| d.install.as_ref())
                .map_or(Look::Source, Install::look);
            let mut controls = page_controls(menu, &page);
            controls.extend(ffa_button(menu, &page, buyer.ffa));
            let info = view
                .hover
                .as_ref()
                .and_then(|name| {
                    controls
                        .iter()
                        .find(|c| c.kind == Kind::ItemInfo)
                        .map(|panel| (name.clone(), [panel.rect[0], panel.rect[1]]))
                })
                .and_then(|(name, at)| {
                    let panel = page_controls(menu, &format!("classes/{name}.res"));
                    (!panel.is_empty()).then_some((panel, at))
                });
            let mut pictures = HashMap::new();
            for control in controls
                .iter()
                .chain(info.iter().flat_map(|(panel, _)| panel.iter()))
            {
                if let Some(name) = &control.image {
                    pictures.insert(name.clone(), picture(menu, &mut images, name));
                }
            }
            let enabled: Vec<bool> = controls
                .iter()
                .map(|c| vgui_enabled(menu, c, &buyer))
                .collect();
            commands.entity(root).with_children(|root| {
                draw_vgui(
                    root,
                    &controls,
                    &enabled,
                    view.hover.as_deref(),
                    info.as_ref(),
                    &pictures,
                    look,
                    unit,
                    &fonts,
                );
            });
        }
        Some(Screen::Classic(name)) => {
            let lines = menu
                .data
                .as_ref()
                .and_then(|d| d.classic.get(&name.to_ascii_lowercase()))
                .cloned()
                .unwrap_or_default();
            let lines = classic_lines(&name, lines, &buyer);
            commands.entity(root).with_children(|root| {
                draw_classic(root, &lines, unit, &fonts.menu);
            });
        }
        None => {}
    }
    if let Some(message) = &view.message {
        commands.entity(root).with_children(|root| {
            draw_message(root, message, unit, &fonts.bold);
        });
    }
    menu.shown = Some(view);
}

const DIGIT_KEYS: [(KeyCode, u8); 20] = [
    (KeyCode::Digit1, 1),
    (KeyCode::Digit2, 2),
    (KeyCode::Digit3, 3),
    (KeyCode::Digit4, 4),
    (KeyCode::Digit5, 5),
    (KeyCode::Digit6, 6),
    (KeyCode::Digit7, 7),
    (KeyCode::Digit8, 8),
    (KeyCode::Digit9, 9),
    (KeyCode::Digit0, 10),
    (KeyCode::Numpad1, 1),
    (KeyCode::Numpad2, 2),
    (KeyCode::Numpad3, 3),
    (KeyCode::Numpad4, 4),
    (KeyCode::Numpad5, 5),
    (KeyCode::Numpad6, 6),
    (KeyCode::Numpad7, 7),
    (KeyCode::Numpad8, 8),
    (KeyCode::Numpad9, 9),
    (KeyCode::Numpad0, 10),
];

fn page_controls(menu: &mut CsBuyMenu, path: &str) -> Vec<Control> {
    if !menu.pages.contains_key(path) {
        let controls = menu.data.as_ref().and_then(|data| load_controls(data, path));
        menu.pages.insert(path.to_owned(), controls);
    }
    menu.pages.get(path).cloned().flatten().unwrap_or_default()
}

fn picture(menu: &mut CsBuyMenu, images: &mut Assets<Image>, name: &str) -> Option<Handle<Image>> {
    if let Some(handle) = menu.images.get(name) {
        return handle.clone();
    }
    let handle = menu
        .data
        .as_ref()
        .and_then(|d| d.install.as_ref())
        .and_then(|install| install.image(name))
        .map(|(width, height, rgba)| {
            images.add(Image::new(
                Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                rgba,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::default(),
            ))
        });
    menu.images.insert(name.to_owned(), handle.clone());
    handle
}

/// Free-for-all's extra button on the VGUI first page: the other side's guns.
fn ffa_button(menu: &CsBuyMenu, page: &str, ffa: bool) -> Option<Control> {
    let first = menu
        .data
        .as_ref()
        .and_then(|d| d.install.as_ref())
        .and_then(|install| main_page(install, !menu.ffa_counter_terrorist));
    (ffa && first.as_deref() == Some(page)).then(|| Control {
        kind: Kind::Button,
        name: "ffa_side".to_owned(),
        rect: [52.0, 340.0, 170.0, 20.0],
        text: if menu.ffa_counter_terrorist {
            "9 TERRORIST GUNS".to_owned()
        } else {
            "9 COUNTER-TERRORIST GUNS".to_owned()
        },
        hotkey: Some(9),
        align: Align::West,
        title: false,
        dull: false,
        command: "ffa_side".to_owned(),
        image: None,
        fill: None,
    })
}

fn vgui_action(menu: &CsBuyMenu, control: &Control, terrorist: bool) -> Action {
    let command = control.command.trim();
    if command.eq_ignore_ascii_case("vguicancel") {
        return Action::Close;
    }
    if command == "ffa_side" {
        return Action::SwitchSide;
    }
    if command.to_ascii_lowercase().ends_with(".res") {
        return menu
            .data
            .as_ref()
            .and_then(|d| d.install.as_ref())
            .and_then(|install| page_path(install, command, terrorist))
            .map_or(Action::Unavailable, Action::Open);
    }
    cs::buy_alias(command).map_or(Action::Unavailable, Action::Buy)
}

fn vgui_enabled(menu: &CsBuyMenu, control: &Control, buyer: &Buyer) -> bool {
    if control.kind != Kind::Button {
        return true;
    }
    match vgui_action(menu, control, buyer.terrorist) {
        Action::Buy(name) => buyer.may_buy(name),
        Action::Unavailable => false,
        _ => true,
    }
}

fn act(menu: &mut CsBuyMenu, action: Action, buyer: &Buyer, now: f32) {
    match action {
        Action::Open(page) => {
            menu.screen = Some(match menu.screen {
                Some(Screen::Vgui(_)) => Screen::Vgui(page.to_owned()),
                _ => Screen::Classic(page.to_owned()),
            });
        }
        Action::Close => menu.close(),
        Action::Unavailable => {}
        Action::SwitchSide => {
            menu.ffa_counter_terrorist = !menu.ffa_counter_terrorist;
            if let Some(Screen::Vgui(_)) = menu.screen {
                let page = menu
                    .data
                    .as_ref()
                    .and_then(|d| d.install.as_ref())
                    .and_then(|install| main_page(install, !menu.ffa_counter_terrorist));
                if let Some(page) = page {
                    menu.screen = Some(Screen::Vgui(page));
                }
            }
        }
        Action::Buy(name) => {
            // CS closes the menu on any pick and says why a purchase can't happen.
            menu.close();
            let data = menu.data.as_ref();
            let say = |key: &str, fallback: &str| {
                data.map_or_else(|| fallback.to_owned(), |d| d.say(key, fallback))
            };
            let refusal = match cs::buy_team(name) {
                BuyTeam::Terrorists if buyer.economy && !buyer.terrorist => Some(name),
                BuyTeam::CounterTerrorists if buyer.economy && buyer.terrorist => Some(name),
                _ => None,
            }
            .map(|name| {
                say(
                    "Cstrike_TitlesTXT_Alias_Not_Avail",
                    "The \"%s1\" is not available for your team to buy.",
                )
                .replace("%s1", item_name(name))
            })
            .or_else(|| match name {
                "vest" if buyer.armor >= 100 => Some(say(
                    "Cstrike_TitlesTXT_Already_Have_Kevlar",
                    "You already have kevlar!",
                )),
                "vesthelm" if buyer.armor >= 100 && buyer.helmet => Some(say(
                    "Cstrike_TitlesTXT_Already_Have_Kevlar_Helmet",
                    "You already have kevlar and a helmet!",
                )),
                _ => None,
            })
            .or_else(|| {
                // Over full kevlar, the helmet alone costs $350.
                let price = match name {
                    "vesthelm" if buyer.armor >= 100 => 350,
                    other => cs::buy_price(other).unwrap_or(0),
                };
                (price > buyer.money).then(|| {
                    say(
                        "Cstrike_TitlesTXT_Not_Enough_Money",
                        "You have insufficient funds!",
                    )
                })
            });
            match refusal {
                Some(text) => menu.message = Some((text, now + MESSAGE_SECONDS)),
                None => menu.purchases.push(name),
            }
        }
    }
}

/// A classic menu's lines as shown: items we don't sell greyed (`\d`), free-for-all's side
/// switch on the first menu.
fn classic_lines(name: &str, mut lines: Vec<String>, buyer: &Buyer) -> Vec<String> {
    for line in &mut lines {
        let body = line.trim_start_matches("\\w").trim_start_matches("\\d");
        let Some(key) = body
            .split('.')
            .next()
            .and_then(|n| n.trim().parse::<u8>().ok())
            .filter(|n| (1..=9).contains(n))
        else {
            continue;
        };
        if classic_action(name, key, buyer.terrorist, buyer.ffa) == Some(Action::Unavailable) {
            // Menu colours carry on to the next line: hand white back after.
            *line = format!("\\d{body}\\w");
        }
    }
    if buyer.ffa && name.eq_ignore_ascii_case("buy") {
        let side = if buyer.terrorist {
            "9. Counter-Terrorist guns"
        } else {
            "9. Terrorist guns"
        };
        let at = lines
            .iter()
            .position(|l| l.trim_start_matches("\\w").starts_with("0."))
            .unwrap_or(lines.len());
        lines.insert(at, String::new());
        lines.insert(at, side.to_owned());
    }
    lines
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

fn place(rect: [f32; 4], unit: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(rect[0] * unit),
        top: Val::Px(rect[1] * unit),
        width: Val::Px(rect[2] * unit),
        height: Val::Px(rect[3] * unit),
        align_items: AlignItems::Center,
        ..default()
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_vgui(
    root: &mut ChildSpawnerCommands,
    controls: &[Control],
    enabled: &[bool],
    hover: Option<&str>,
    info: Option<&(Vec<Control>, [f32; 2])>,
    pictures: &HashMap<String, Option<Handle<Image>>>,
    look: Look,
    unit: f32,
    fonts: &Fonts,
) {
    // The window covers the screen (CS:S `Frame.BgColor`, TransparentBlack).
    root.spawn((
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(rgba(FRAME_BG)),
    ));
    let line = 1.0_f32.max(unit * 0.5).round();
    let draw = |root: &mut ChildSpawnerCommands, control: &Control, offset: [f32; 2], enabled: bool| {
        let rect = [
            control.rect[0] + offset[0],
            control.rect[1] + offset[1],
            control.rect[2],
            control.rect[3],
        ];
        let mut node = place(rect, unit);
        match control.kind {
            Kind::Label | Kind::Button => {
                node.justify_content = match control.align {
                    Align::West => JustifyContent::FlexStart,
                    Align::Center => JustifyContent::Center,
                    Align::East => JustifyContent::FlexEnd,
                };
                let button = control.kind == Kind::Button;
                if button {
                    node.padding = UiRect::horizontal(Val::Px(6.0 * unit));
                    node.border = UiRect::all(Val::Px(line));
                }
                let armed = button && enabled && hover == Some(control.name.as_str());
                let background = match (button, armed, look) {
                    (true, true, Look::Source) => SOURCE_ARMED_BG,
                    (true, true, Look::GoldSrc) => GOLDSRC_ARMED_BG,
                    (true, false, _) => BUTTON_BG,
                    (false, ..) => [0; 4],
                };
                let color = match (enabled, control.dull) {
                    (false, _) => LIGHT_ORANGE,
                    (true, true) => DIM_ORANGE,
                    (true, false) => ORANGE,
                };
                let (font, size) = if control.title {
                    (&fonts.bold, 18.0)
                } else if button {
                    (&fonts.bold, 8.5)
                } else {
                    (&fonts.regular, 8.0)
                };
                let mut entity = root.spawn((node, BackgroundColor(rgba(background))));
                if button {
                    entity.insert(BorderColor::all(rgba(LIGHT_ORANGE)));
                }
                entity.with_children(|cell| {
                    cell.spawn(text(font, size * unit, rgba(color), control.text.clone()));
                });
            }
            Kind::Image => {
                let mut entity = root.spawn(node);
                if let Some(fill) = control.fill {
                    entity.insert(BackgroundColor(rgba(fill)));
                }
                if let Some(Some(image)) = control.image.as_ref().and_then(|i| pictures.get(i)) {
                    entity.insert(ImageNode::new(image.clone()));
                }
            }
            Kind::Divider => {
                node.border = UiRect::all(Val::Px(line));
                root.spawn((node, BorderColor::all(rgba(LIGHT_ORANGE))));
            }
            Kind::ItemInfo => {}
        }
    };
    for (control, &enabled) in controls.iter().zip(enabled) {
        draw(root, control, [0.0; 2], enabled);
    }
    if let Some((panel, at)) = info {
        for control in panel {
            draw(root, control, *at, true);
        }
    }
}

/// GoldSrc's menu text: `\w` white, `\y` yellow, `\d` grey, `\r` red, `\R` the rest at the right.
fn draw_classic(root: &mut ChildSpawnerCommands, lines: &[String], unit: f32, font: &Handle<Font>) {
    const LINE: f32 = 12.0;
    const WIDTH: f32 = 230.0;
    let newlines = lines.len().saturating_sub(1) as f32;
    let top = 240.0 - (newlines / 2.0).floor() * LINE - 40.0;
    let mut color = MENU_WHITE;
    root.spawn(Node {
        position_type: PositionType::Absolute,
        left: Val::Px(20.0 * unit),
        top: Val::Px(top.max(4.0) * unit),
        width: Val::Px(WIDTH * unit),
        flex_direction: FlexDirection::Column,
        ..default()
    })
    .with_children(|menu| {
        for line in lines {
            menu.spawn(Node {
                width: Val::Percent(100.0),
                height: Val::Px(LINE * unit),
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|row| {
                let mut right = false;
                let mut chars = line.chars().peekable();
                let mut run = String::new();
                let flush = |row: &mut ChildSpawnerCommands, run: &mut String, color: [u8; 4], right: bool| {
                    if run.is_empty() {
                        return;
                    }
                    let mut entity = row.spawn(text(font, 9.0 * unit, rgba(color), std::mem::take(run)));
                    if right {
                        entity.insert(Node {
                            margin: UiRect::left(Val::Auto),
                            ..default()
                        });
                    }
                };
                while let Some(c) = chars.next() {
                    if c == '\\'
                        && let Some(&code) = chars.peek()
                        && matches!(code, 'w' | 'y' | 'd' | 'r' | 'R')
                    {
                        chars.next();
                        flush(row, &mut run, color, right);
                        match code {
                            'w' => color = MENU_WHITE,
                            'y' => color = MENU_YELLOW,
                            'd' => color = MENU_GRAY,
                            'r' => color = MENU_RED,
                            _ => right = true,
                        }
                        continue;
                    }
                    if c == '\t' {
                        continue;
                    }
                    run.push(c);
                }
                flush(row, &mut run, color, right);
            });
        }
    });
}

/// A refusal in the middle of the screen, as CS's centre print.
fn draw_message(root: &mut ChildSpawnerCommands, message: &str, unit: f32, font: &Handle<Font>) {
    root.spawn(Node {
        position_type: PositionType::Absolute,
        width: Val::Percent(100.0),
        top: Val::Percent(30.0),
        justify_content: JustifyContent::Center,
        ..default()
    })
    .with_children(|line| {
        line.spawn((
            Text::new(message),
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(10.0 * unit),
                ..default()
            },
            TextColor(Color::WHITE),
            TextLayout::new(Justify::Center, LineBreak::NoWrap),
            TextShadow::default(),
        ));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localization_reads_escapes_and_conditions() {
        let file = "\"lang\"\n{\n\"Language\" \"English\"\n\"Tokens\"\n{\n\"A\" \"The \\\"%s1\\\"\nnext\" [$WIN32]\n// note\n\"Cstrike_Pistols\" \"&1 PISTOLS\"\n}\n}\n";
        let strings = parse_localization(file.as_bytes());
        assert_eq!(strings["a"], "The \"%s1\"\nnext");
        assert_eq!(strings["cstrike_pistols"], "&1 PISTOLS");
    }

    #[test]
    fn titles_menus_parse() {
        let menus = parse_titles("Buy\r\n{\r\n\\yBuy Item\\w\r\n\r\n1. Handgun\r\n}\r\n");
        assert_eq!(menus["buy"], vec!["\\yBuy Item\\w", "", "1. Handgun"]);
    }

    #[test]
    fn classic_menus_follow_cs16() {
        assert_eq!(classic_action("T_BuyRifle", 2, true, false), Some(Action::Buy("ak47")));
        assert_eq!(classic_action("CT_BuyRifle", 3, false, false), Some(Action::Buy("m4a1")));
        assert_eq!(classic_action("T_BuyPistol", 5, true, false), Some(Action::Buy("elite")));
        assert_eq!(classic_action("CT_BuyItem", 7, false, false), Some(Action::Unavailable));
        assert_eq!(classic_action("Buy", 4, true, false), Some(Action::Open("T_BuyRifle".to_owned())));
        assert_eq!(classic_action("Buy", 9, true, false), None);
        assert_eq!(classic_action("Buy", 9, true, true), Some(Action::SwitchSide));
        assert_eq!(classic_action("BuyShotgun", 10, true, false), Some(Action::Close));
    }

    #[test]
    fn own_menus_cover_every_page() {
        let menus = own_classic_menus();
        for key in 1..=8 {
            for terrorist in [true, false] {
                if let Some(Action::Open(page)) = classic_action("Buy", key, terrorist, false) {
                    assert!(menus.contains_key(&page.to_ascii_lowercase()), "{page}");
                }
            }
        }
        assert!(menus["t_buyrifle"][3].contains("AK-47"));
    }

    #[test]
    fn tga_reads_bottom_up_bgra() {
        // 1x2, 32-bit, bottom row first: blue then red.
        let mut tga = vec![0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 2, 0, 32, 8];
        tga.extend([255, 0, 0, 255, 0, 0, 255, 255]);
        let (w, h, rgba) = decode_tga(&tga).unwrap();
        assert_eq!((w, h), (1, 2));
        assert_eq!(rgba, vec![255, 0, 0, 255, 0, 0, 255, 255]);
    }
}
