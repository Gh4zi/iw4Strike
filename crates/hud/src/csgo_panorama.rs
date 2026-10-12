//! CS:GO's own HUD, built at runtime from the player's CS:GO install by the `panorama` crate:
//! `hud.xml` with its health and armour, money, weapon panel, team counter and kill feed
//! panels, driven as CS:GO's C++ HUD panels drive them (the same classes, dialog variables,
//! texts and progress). Nothing of CS:GO's is shipped: without the install the hand-built CS:GO
//! HUD (`csgo_hud`), CS:S's or CS 1.6's draws instead.

use assets::PreparedWeapons;
use bevy::prelude::*;
use frame::ViewSubject;
use net::{LocalPresentClient, PresentedSnapshot};
use playerstate_iw4::PM_TYPE_DEAD;

const HUD_LAYOUT: &str = "file://{resources}/layout/hud/hud.xml";
/// `g_LowHealthPercent`: at or under this share of health the panel turns critical.
const LOW_HEALTH: f32 = 0.2;
/// `g_HealthFlashSeconds`: how long a hit keeps the health panel flashing.
const DAMAGE_FLASH_SECONDS: f32 = 1.0;
/// The weapon panel's bullets turn red at or under this share of the magazine.
const LOW_AMMO: f32 = 0.2;
/// `DeathNoticeLifetime`, `DeathNoticeLocalPlayerLifetimeMod`, `DeathNoticeFadeOutTime`.
const NOTICE_SECONDS: f32 = 5.0;
const NOTICE_LOCAL_SCALE: f32 = 1.5;
const NOTICE_FADE_SECONDS: f32 = 1.0;

/// The layouts of the HUD's custom panels this port drives, by their type in `hud.xml`.
fn sublayout(tag: &str) -> Option<&'static str> {
    Some(match tag.to_ascii_lowercase().as_str() {
        "csgohudhealtharmor" => "file://{resources}/layout/hud/hudhealtharmor.xml",
        "csgohudmoney" => "file://{resources}/layout/hud/hudmoney.xml",
        "csgohudweaponpanel" => "file://{resources}/layout/hud/hudweaponpanel.xml",
        "csgohudteamcounter" => "file://{resources}/layout/hud/hudteamcounter.xml",
        "csgohuddeathnotice" => "file://{resources}/layout/hud/huddeathnotice.xml",
        "csgobuymenu" => "file://{resources}/layout/buymenu.xml",
        "csgohudradar" => "file://{resources}/layout/hud/hudradar.xml",
        "csgohudreticle" => "file://{resources}/layout/hud/hudreticle.xml",
        "csgoscoreboard" => "file://{resources}/layout/scoreboard.xml",
        "csgohudwinpanel" => "file://{resources}/layout/hud/hudwinpanel.xml",
        "csgoendofmatch" => "file://{resources}/layout/endofmatch.xml",
        "csgoendofmatchvotingpanel" => "file://{resources}/layout/endofmatch-voting.xml",
        _ => return None,
    })
}

#[derive(Component)]
pub(crate) struct CsgoPanoramaRoot;

/// The buy menu's wheels' categories, in CS:GO's order (its pages' names).
const BUY_CATEGORIES: [(&str, &str); 6] = [
    ("pistols", "PISTOLS"),
    ("heavy", "HEAVY"),
    ("smgs", "SMGS"),
    ("rifles", "RIFLES"),
    ("gear", "GEAR"),
    ("grenades", "GRENADES"),
];

/// One of the buy menu's radial wheels: its panel and its wedges (the buttons).
struct Wheel {
    panel: usize,
    wedges: Vec<usize>,
    /// Where its first wedge starts (`CCSGO_RadialSelector::m_angStart`): -150 degrees for six
    /// wedges, -135 for four.
    start: f32,
}

/// The buy menu's wheels: the categories and each category's items.
struct BuyWheels {
    categories: Wheel,
    items: Vec<Wheel>,
}

/// Builds CS:GO's buy wheels as `CCSGO_BuyMenu::CreateBuyWheels` does: a six-wedge wheel of
/// categories and, per category, a wheel of its items (four wedges when it holds four).
fn build_buy_wheels(doc: &mut panorama::Doc, files: &panorama::Files) -> Option<BuyWheels> {
    let container = doc.find("BuyWheelContainer")?;
    let wheel = |doc: &mut panorama::Doc, wedges: usize| -> Option<Wheel> {
        let (snippet, start) = if wedges > 4 {
            ("Radial_SixSlot", -150.0)
        } else {
            ("Radial_FourSlot", -135.0)
        };
        let panel = doc.add_snippet(files, container, snippet)?;
        doc.set_visible(panel, false);
        Some(Wheel {
            panel,
            wedges: doc.children(panel),
            start,
        })
    };
    let categories = wheel(doc, 6)?;
    for (i, (_, name)) in BUY_CATEGORIES.iter().enumerate() {
        let button = *categories.wedges.get(i)?;
        let contents = doc.find_in(button, "Contents")?;
        doc.load_snippet(files, contents, "CategoryWedge");
        doc.set_class(button, "no-hover", false);
        if let Some(label) = doc.find_in(contents, "CategoryName") {
            doc.set_text(label, name);
            doc.set_class(label, "buywheel-category__label--UPPERCASE", true);
        }
        if let Some(label) = doc.find_in(contents, "WedgeKeybinding") {
            doc.set_text(label, &(i + 1).to_string());
        }
    }
    let mut items = Vec::new();
    for (page, _) in BUY_CATEGORIES {
        let count =
            crate::cs_buymenu::csgo_items(page, true, 0).map_or(0, |(_, slots)| slots.len());
        let w = wheel(doc, count)?;
        for (i, &button) in w.wedges.iter().enumerate().take(count) {
            let contents = doc.find_in(button, "Contents")?;
            doc.load_snippet(files, contents, "ItemWedge");
            if let Some(label) = doc.find_in(contents, "WedgeKeybinding") {
                doc.set_text(label, &(i + 1).to_string());
            }
        }
        items.push(w);
    }
    Some(BuyWheels { categories, items })
}

/// The item details CS:GO's buy menu shows for a hovered gun (`CCSGO_BuyMenu` with
/// `Helper_GetWeaponAccuracy` and `GetEffectiveRangeRawValue`): its name, price, magazine and
/// reserve, kill award, and the damage, fire rate, handling, accurate range and armour
/// penetration bars.
fn show_item_details(doc: &mut panorama::Doc, info: usize, item: &str) {
    use weapon_iw4::cs;
    doc.set_var(info, "item_name", crate::cs_buymenu::csgo_item_name(item));
    doc.set_var(info, "cost", &cs::buy_price(item, sim::cs_settings::shooting_mode()).unwrap_or(0).to_string());
    let gun = cs::cs_weapon_by_name(item);
    let stats = doc.find_in(info, "WeaponStats");
    if let Some(stats) = stats {
        doc.set_visible(stats, gun.is_some());
    }
    let Some(gun) = gun else {
        return;
    };
    let index = cs::CS_WEAPONS
        .iter()
        .position(|w| w.name == gun.name)
        .map_or(0, |i| i as u8 + 1);
    let award = cs::kill_award(index);
    doc.set_var(info, "ammo_per_mag", &gun.clip.to_string());
    doc.set_var(info, "ammo_total", &gun.reserve.to_string());
    doc.set_var(info, "kill_reward", &award.to_string());
    doc.set_var(info, "kill_reward_pct", &(award * 100 / 300).to_string());
    let damage = gun.damage * gun.pellets.max(1) as f32;
    let fire_rate = if gun.cycle > 0.0 { 1.0 / gun.cycle } else { 0.0 };
    let sniper = gun.scope_overlay;
    let mode = usize::from(sniper);
    let (handling, range) = weapon_iw4::csgo::csgo_gun(gun.name).map_or((0.0, 0.0), |g| {
        let magnitude = g.recoil_magnitude[mode] * (1.0 + (g.recoil_magnitude_variance[mode] / 30.0).min(1.0));
        let angle = g.recoil_angle[mode] * (1.0 + (g.recoil_angle_variance[mode] / 180.0).min(1.0));
        let handling = ((1.0 - (magnitude + angle - 5.0) / 80.0) * 100.0).max(1.0).floor();
        let spread = (g.inaccuracy_stand[mode] + g.spread[mode]) * 0.001;
        // A 12-inch plate's radius over the spread, in metres, as a share of 90 m.
        let metres = if spread > 1e-5 { 6.0 / spread * 0.0254 } else { 1e6 };
        (handling, ((metres / 90.0).min(1.0) * 100.0).floor())
    });
    let armor = (100.0 * cs::armor_penetration(index)).floor();
    for (var, value) in [
        ("damage", damage),
        ("fire_rate", fire_rate),
        ("accuracy", handling),
        ("range", range),
        ("armor_pen", armor),
    ] {
        doc.set_var(info, var, &(value as i32).to_string());
    }
    for (id, value) in [
        ("DmgProgress", damage),
        ("FireRateProgress", fire_rate),
        ("AccuracyProgress", handling),
        ("RangeProgress", range),
        ("ArmorPenProgress", armor),
    ] {
        if let Some(bar) = doc.find_in(info, id) {
            let min = doc.attr(bar, "min").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
            let max = doc.attr(bar, "max").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
            doc.set_value(bar, (value - min) / (max - min).max(1e-3));
        }
    }
}

/// CS:GO's round radar (`#Radar__Round--Inner`): 250 Panorama pixels across, showing as much of
/// the map as MW2's minimap does.
const RADAR_SIZE: f32 = 250.0;
/// CS:GO's `#Radar` panel the radar sits centred in (`Radar_PanelSize`), where the bomb site
/// icons are placed.
const RADAR_PANEL_SIZE: f32 = 300.0;
/// How many players the radar has icons for.
const RADAR_ICONS: usize = 32;
/// The icons a player package can show (one at a time, plus the local player's view cone).
const RADAR_ICON_IDS: [&str; 15] = [
    "ViewFrustrum",
    "PlayerPing",
    "PlayerPingOffMap",
    "CTOnMap",
    "CTOffMap",
    "CTDeath",
    "CTGhost",
    "TOnMap",
    "TOffMap",
    "TDeath",
    "TGhost",
    "EnemyOnMap",
    "EnemyOffMap",
    "EnemyDeath",
    "EnemyGhost",
];
/// How long a cross shows where a player died (`DEAD_FADE_TIME`), and a lost enemy's ghost
/// where they were last seen (`GHOST_FADE_TIME`), fading out, in seconds.
const RADAR_DEAD_FADE_S: f32 = 4.0;
const RADAR_GHOST_FADE_S: f32 = 6.0;

/// A player's icon on the radar this frame: where (Panorama pixels in the radar), which of the
/// package's images, with the view cone or not, turned by `turn` radians, at `fade` opacity.
struct RadarIcon {
    at: (f32, f32),
    icon: String,
    cone: bool,
    turn: f32,
    fade: f32,
    /// A teammate's colour (`cl_teammate_colors_show`), over the side's.
    color: Option<&'static str>,
}

/// How long the round-end panel stays up: the round restarts 7 s after it is decided.
const WIN_PANEL_SECONDS: f32 = 6.5;

/// How many teammates' pings show on screen at once.
const PING_MARKERS: usize = 10;
/// CS:GO's ping marker (`PlayerPing`, laid out 1024 pixels across) at the size it shows on screen.
const PING_MARKER_SCALE: f32 = 0.4;

/// A teammate's ping still showing: where, whether it warns of danger, and how faded.
struct ShownPing {
    origin: [f32; 3],
    danger: bool,
    fade: f32,
}

/// The pings of the local player's team (theirs too) still showing
/// ([`weapon_iw4::cs::CS_PING_SECONDS`], fading over the last second); none with pings off
/// (`cl_player_ping_mute 2`).
fn team_pings(snap: Option<&sim::Snapshot>, local: sim::ClientId, mute: u8) -> Vec<ShownPing> {
    let Some(snap) = snap.filter(|_| mute < 2) else {
        return Vec::new();
    };
    let team = snap.meta.for_client(local).map_or(0, |m| m.client_state_team);
    let now = i64::from(snap.tick.0) * i64::from(sim::MATCH_TICK_MS);
    let life = weapon_iw4::cs::CS_PING_SECONDS * 1000.0;
    snap.meta
        .clients
        .iter()
        .filter(|(id, meta)| *id == local || (team != 0 && meta.client_state_team == team))
        .filter_map(|(_, meta)| {
            let ping = meta.cs_ping?;
            let age = (now - i64::from(ping.placed_ms)) as f32;
            (0.0..life).contains(&age).then(|| ShownPing {
                origin: ping.origin,
                danger: ping.danger,
                fade: ((life - age) / 1000.0).clamp(0.0, 1.0),
            })
        })
        .collect()
}

/// Whether the end-of-match map vote is open (the number keys vote).
pub(crate) fn map_vote_open(snap: Option<&sim::Snapshot>) -> bool {
    crate::cs_hud::server_info(snap, "cs_mapvote")
            .and_then(parse_map_vote)
            .is_some_and(|(_, _, _, winner)| winner.is_none())
}

/// The server's `cs_mapvote`: the maps, each one's votes, the votes a map needs, the winner.
fn parse_map_vote(value: &str) -> Option<(Vec<String>, Vec<u32>, u32, Option<usize>)> {
    let mut parts = value.split('|');
    let maps = parts.next()?.split_whitespace().map(str::to_owned).collect();
    let counts = parts.next()?.split_whitespace().filter_map(|c| c.parse().ok()).collect();
    let needed = parts.next()?.parse().ok()?;
    let winner = parts.next()?.parse().ok();
    Some((maps, counts, needed, winner))
}

/// A map's MW2 loading screen, read from its zone's game folder, as a texture.
fn map_picture(map: &str, images: &mut Assets<Image>) -> Option<Handle<Image>> {
    let root = asset_transport::games_root_from_env().ok()?;
    let zone = asset_transport::find_zone_file(&root, map).ok()?;
    let name = map.rsplit(':').next().unwrap_or(map);
    let (width, height, rgba) = asset_material::decode_map_preview(&zone.path, name).ok()??;
    Some(images.add(Image::new(
        bevy::render::render_resource::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        rgba,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    )))
}

/// How many avatars a side shows (CS:GO's large avatars, a five-a-side match).
const AVATARS_PER_SIDE: usize = 5;
/// CS:GO's teammate colours (`cl_teammate_color_1..5`) and their letters.
const TEAMMATE_COLORS: [(&str, &str); 5] = [
    ("rgb(248, 246, 45)", "Y"),
    ("rgb(161, 25, 240)", "P"),
    ("rgb(0, 181, 98)", "G"),
    ("rgb(92, 168, 255)", "B"),
    ("rgb(255, 155, 37)", "O"),
];

/// A team counter avatar (`AvatarLargeSnippet`), and the parts the game sets. The skull, colour
/// and letter CS:GO moves out of the snippet's hidden part are copies under its shown part here.
pub(crate) struct Avatar {
    panel: usize,
    background: usize,
    health: Option<usize>,
    skull: usize,
    color: usize,
    letter: usize,
}

/// Each side's players in its avatars (`UpdateAvatarSlot`), by client number: the team's
/// portrait frame (the local player outlined white), the health of a teammate under it, a skull
/// once dead (red for a teammate, the team's colour for an enemy), and a living teammate's
/// colour and letter.
fn update_avatars(
    doc: &mut panorama::Doc,
    files: &panorama::Files,
    avatars: &mut [Vec<Avatar>; 2],
    counter: usize,
    snap: Option<&sim::Snapshot>,
    local: sim::ClientId,
) {
    let local_team = snap.and_then(|s| s.meta.for_client(local)).map_or(0, |m| m.client_state_team);
    let mut sides: [Vec<(sim::ClientId, &playerstate_iw4::PlayerState, bool)>; 2] = [Vec::new(), Vec::new()];
    if let Some(snap) = snap {
        for (id, ps) in &snap.players {
            let team = snap.meta.for_client(*id).map_or(0, |m| m.client_state_team);
            if let Some(terrorist) = crate::cs_hud::is_terrorist(Some(snap), team) {
                sides[usize::from(terrorist)].push((*id, ps, team == local_team));
            }
        }
    }
    for (k, side) in ["CT", "T"].into_iter().enumerate() {
        let Some(parent) = doc.find_in(counter, &format!("TeamLarge{side}")) else {
            continue;
        };
        if avatars[k].is_empty() {
            for idx in 0..AVATARS_PER_SIDE {
                let panel = doc.add_element(
                    files,
                    parent,
                    "Panel",
                    &[("id", &format!("Avatar{}", k * AVATARS_PER_SIDE + idx))],
                );
                doc.load_snippet(files, panel, "AvatarLargeSnippet");
                doc.set_class(panel, "AvatarLargeSnippet--ScaleSmaller", true);
                let Some(background) = doc.find_in(panel, "AvatarL__ImageBG") else {
                    continue;
                };
                for class in [
                    format!("AvatarImage{side}__Color"),
                    format!("AvatarL__Default--{side}"),
                ] {
                    doc.set_class(background, &class, true);
                }
                let shown = doc.find_in(panel, "Visible").unwrap_or(panel);
                let skull = doc.add_element(
                    files,
                    shown,
                    "Image",
                    &[("class", "AvatarL__Skull"), ("src", "file://{images}/icons/ui/elimination.svg")],
                );
                let color = doc.add_element(
                    files,
                    shown,
                    "Image",
                    &[
                        ("class", "AvatarL__PlayerColor"),
                        ("src", "file://{images}/hud/teamcounter/teamcounter_playercolor.png"),
                    ],
                );
                let letter = doc.add_element(files, shown, "Label", &[("class", "AvatarL__PlayerLetter")]);
                for id in ["ArsenalProgress", "AvatarImage"] {
                    if let Some(part) = doc.find_in(panel, id) {
                        doc.set_visible(part, false);
                    }
                }
                let health = doc.find_in(panel, "HealthBar");
                avatars[k].push(Avatar { panel, background, health, skull, color, letter });
            }
        }
        let players = &mut sides[k];
        players.sort_by_key(|(id, _, _)| id.0);
        for (slot, avatar) in avatars[k].iter().enumerate() {
            let Some(&(id, ps, teammate)) = players.get(slot) else {
                doc.set_visible(avatar.panel, false);
                continue;
            };
            doc.set_visible(avatar.panel, true);
            let dead = ps.pm_type >= PM_TYPE_DEAD;
            let me = id == local;
            doc.set_visible(avatar.background, !dead);
            doc.set_class(avatar.background, "Avatar__PlayerOutline--White", me && !dead);
            doc.set_class(avatar.background, &format!("Avatar__PlayerBorder--{side}"), !(me && !dead));
            if let Some(bar) = avatar.health {
                let health = ps.health.clamp(0, 100);
                doc.set_visible(bar, teammate && !dead);
                doc.set_style(bar, "width", Some(&format!("{health}%")));
                doc.set_class(bar, "Avatar__HealthBar--Red", health <= 20);
                doc.set_class(bar, "Avatar__HealthBar--Normal", health > 20);
            }
            doc.set_visible(avatar.skull, dead);
            doc.set_class(avatar.skull, "Avatar__Skull--Red", dead && teammate);
            doc.set_class(avatar.skull, &format!("Avatar__Skull--{side}"), dead && !teammate);
            // A teammate's colour, by their place on the team.
            let color = TEAMMATE_COLORS.get(slot).filter(|_| teammate && !dead);
            doc.set_visible(avatar.color, color.is_some());
            doc.set_visible(avatar.letter, color.is_some());
            if let Some((rgb, letter)) = color {
                doc.set_style(avatar.color, "wash-color", Some(rgb));
                doc.set_text(avatar.letter, letter);
            }
        }
    }
}

/// A map's name as a title: `mp_rust` is Rust.
fn map_title(zone: &str) -> String {
    let name = zone.strip_prefix("mp_").unwrap_or(zone);
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or(String::new(), |first| first.to_uppercase().chain(chars).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the scoreboard shows: each side's players (the scoreboard's kills, assists, deaths,
/// score and ping; money and the C4 or kit for teammates only, as CS:GO shows them), the sides'
/// rounds, the server's round history, the match's length by its rules, the map and the clock.
fn scoreboard_data(
    snap: Option<&sim::Snapshot>,
    local: sim::ClientId,
    scores: Option<&net::Scoreboard>,
    weapons: Option<&PreparedWeapons>,
    map: String,
) -> crate::csgo_scoreboard::Data {
    use crate::csgo_scoreboard::{Data, Line, Side};
    let rules = weapon_iw4::cs::cs_round_rules(sim::cs_settings::shooting_mode());
    let mut data = Data {
        lines: Vec::new(),
        scores: crate::csgo_hud::scores(snap).unwrap_or([0, 0]),
        history: crate::cs_hud::server_info(snap, "cs_round_history").unwrap_or_default().to_owned(),
        max_rounds: rules.round_limit,
        half: rules.round_switch,
        map,
        clock: clock(crate::cs_hud::round_seconds_left(snap).unwrap_or(0)),
    };
    let Some(snap) = snap else {
        return data;
    };
    let fallback = net::parse_scoreboard_cmd(&net::format_scoreboard_from_snapshot(snap));
    let parsed = scores.filter(|s| s.cmd.is_some()).map_or(&fallback, |s| &s.parsed);
    let local_team = snap.meta.for_client(local).map_or(0, |m| m.client_state_team);
    for row in crate::scoreboard::rows_from_parsed(snap, parsed) {
        let team = row.score.team;
        let Some(terrorist) = crate::cs_hud::is_terrorist(Some(snap), team) else {
            continue;
        };
        let client = row.score.client as u32;
        let ps = snap.players.iter().find(|(id, _)| id.0 == client).map(|(_, ps)| ps);
        let teammate = local_team != 0 && team == local_team;
        let status = if row.dead {
            1
        } else if teammate && ps.zip(weapons).is_some_and(|(ps, w)| crate::cs_hud::carries_c4(ps, w)) {
            2
        } else if teammate && ps.is_some_and(|ps| ps.cs_defuser != 0) {
            7
        } else {
            0
        };
        data.lines.push(Line {
            client,
            side: if terrorist { Side::T } else { Side::Ct },
            name: row.name.clone(),
            dead: row.dead,
            ping: row.score.ping,
            kills: row.score.kills,
            assists: row.score.assists,
            deaths: row.score.deaths,
            score: row.score.score,
            mvps: snap.meta.for_client(sim::ClientId(client)).map_or(0, |m| m.cs_mvps),
            money: ps.filter(|_| teammate).map(|ps| ps.cs_money as i32),
            status,
            local: client == local.0,
        });
    }
    data
}

/// What the radar remembers of a player: where they were last seen, when, and when they died.
struct RadarTrack {
    at: [f32; 2],
    seen_at: Option<f32>,
    dead_at: Option<f32>,
}

/// Draws MW2's minimap `(width, height, rgba)` into `out` as CS:GO's round radar sees it:
/// `size` pixels across, `range` world units across, centred on `at` and turned so `yaw`
/// points up; black off the map, faded out at the rim.
fn draw_radar(
    out: &mut [u8],
    size: u32,
    map: (u32, u32, &[u8]),
    bounds: &hud_iw4::CompassMapBounds,
    at: [f32; 2],
    yaw: f32,
    range: f32,
) {
    let (w, h, rgba) = map;
    let (sin, cos) = yaw.to_radians().sin_cos();
    let forward = [cos, sin];
    let right = [sin, -cos];
    let east = [bounds.north[1], -bounds.north[0]];
    let south = [-bounds.north[0], -bounds.north[1]];
    let units = range / size as f32;
    let radius = size as f32 * 0.5;
    let texel = |x: i64, y: i64| -> [f32; 4] {
        if x < 0 || y < 0 || x >= i64::from(w) || y >= i64::from(h) {
            return [0.0, 0.0, 0.0, 255.0];
        }
        let i = ((y as usize) * w as usize + x as usize) * 4;
        rgba.get(i..i + 4).map_or([0.0, 0.0, 0.0, 255.0], |p| {
            [f32::from(p[0]), f32::from(p[1]), f32::from(p[2]), 255.0]
        })
    };
    for j in 0..size {
        for i in 0..size {
            let dx = i as f32 + 0.5 - radius;
            let dy = j as f32 + 0.5 - radius;
            let o = ((j * size + i) * 4) as usize;
            let edge = (radius - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            if edge <= 0.0 {
                out[o..o + 4].copy_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let world = [
                at[0] + (dx * right[0] - dy * forward[0]) * units,
                at[1] + (dx * right[1] - dy * forward[1]) * units,
            ];
            let delta = [world[0] - bounds.upper_left[0], world[1] - bounds.upper_left[1]];
            let u = (east[0] * delta[0] + east[1] * delta[1]) / bounds.world_size[0];
            let v = (south[0] * delta[0] + south[1] * delta[1]) / bounds.world_size[1];
            let (fx, fy) = (u * w as f32 - 0.5, v * h as f32 - 0.5);
            let (x0, y0) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - x0, fy - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            let (a, b, c, d) = (texel(x0, y0), texel(x0 + 1, y0), texel(x0, y0 + 1), texel(x0 + 1, y0 + 1));
            for k in 0..3 {
                let top = a[k] + (b[k] - a[k]) * tx;
                let bottom = c[k] + (d[k] - c[k]) * tx;
                out[o + k] = (top + (bottom - top) * ty) as u8;
            }
            out[o + 3] = (edge * 255.0) as u8;
        }
    }
}

/// The equipment icon of what a buy name sells (`icons/equipment/<name>.svg`).
fn buy_icon(item: &str) -> &str {
    match item {
        "vest" => "kevlar",
        "vesthelm" => "armor_helmet",
        "defuser" => "defuser",
        "flashbang" | "hegrenade" | "smokegrenade" => item,
        "m4a1" => "m4a1_silencer",
        "m4a4" => "m4a1",
        "usp" => "usp_silencer",
        "p2000" => "hkp2000",
        "p228" => "p250",
        "m3" => "nova",
        "tmp" => "mp9",
        "mp5" => "mp5sd",
        "galil" => "galilar",
        "sg552" => "sg556",
        "scout" => "ssg08",
        "sg550" => "scar20",
        other => other,
    }
}

/// A death notice on screen: its panel, when it came and how long it stays.
struct Notice {
    panel: usize,
    since: f32,
    lifetime: f32,
}

/// The Panorama HUD: CS:GO's files, its document and what the panels remember between frames.
#[derive(Resource, Default)]
pub(crate) struct CsgoPanorama {
    pub(crate) active: bool,
    files: Option<panorama::Files>,
    fonts: panorama::Fonts,
    doc: Option<panorama::Doc>,
    root: Option<Entity>,
    health: Option<i32>,
    damaged_at: Option<f32>,
    notices: Vec<Notice>,
    /// The newest kill feed line already shown (its time).
    feed_seen: i32,
    buy: Option<BuyWheels>,
    /// The radar's map texture and its size in pixels, and its players' icon packages.
    radar_image: Option<(Handle<Image>, u32)>,
    radar_icons: Vec<usize>,
    /// The bomb sites A and B (where their radar objectives are), for the map whose minimap
    /// this names.
    radar_sites: (String, [Option<[f32; 2]>; 2]),
    /// The scoreboard (TAB).
    scoreboard: crate::csgo_scoreboard::Scoreboard,
    /// The last `cs_round_end` seen (`None` before the first snapshot), and the round whose win
    /// panel is up and since when.
    win_seen: Option<Option<String>>,
    win_shown: Option<(String, f32)>,
    /// The end-of-match vote's map tiles (the maps they show), and maps whose picture is still
    /// to be read.
    vote_tiles: Vec<(usize, String)>,
    vote_pictures: Vec<(usize, String)>,
    /// The team counter's avatars: the Counter-Terrorists' and the Terrorists'.
    avatars: [Vec<Avatar>; 2],
    /// The on-screen ping markers (`PlayerPing` snippets).
    ping_markers: Vec<usize>,
    /// The radar's memory of each player, by client number.
    radar_tracks: std::collections::HashMap<u32, RadarTrack>,
}

/// Reads CS:GO's HUD; `None` without CS:GO (or its packed UI and fonts).
pub(crate) fn load(fonts: &mut Assets<Font>) -> Option<CsgoPanorama> {
    let files = panorama::Files::open()?;
    let font_set = files.fonts(fonts)?;
    let mut doc = panorama::Doc::load(&files, HUD_LAYOUT, sublayout)?;
    // `cl_hud_color 0`, the default HUD colour.
    let root = doc.root();
    doc.set_class(root, "csgo-hud--cl-hud-color-0", true);
    if let Some(fill) = doc.find("HealthBar_Left") {
        doc.set_class(fill, "cl-hud-background-color", true);
    }
    // The team counter as `CCSGO_HudTeamCounter` sets it up for a classic match: no bomb
    // status, join banner or small player strips; the clock, the scores and each side's players'
    // avatars (`cl_hud_playercount_showcount 0`), not the alive counts, at the top.
    if let Some(counter) = doc.find("HudTeamCounter") {
        for id in [
            "BombStatus",
            "BombPlanted",
            "BombPlantedLines",
            "BombDefused",
            "TeamAll",
            "TeamSmallContainerCT",
            "TeamSmallContainerT",
            "JoinPanelParent",
            "AliveBackgroundCT",
            "AliveBackgroundT",
        ] {
            if let Some(panel) = doc.find_in(counter, id) {
                doc.set_visible(panel, false);
            }
        }
        doc.set_class(counter, "TeamCounter__Align--Top", true);
        doc.set_class(counter, "TeamCounter__Flow--Down", true);
    }
    let buy = build_buy_wheels(&mut doc, &files);
    if let Some(menu) = doc.find("BuyMenu") {
        // Its bar's keys (`{v:csgo_bind:…}`) are this game's binds; no buying for teammates.
        for (bind, key) in [("bind_cancelselect", "ESC"), ("bind_autobuy", "F1"), ("bind_rebuy", "F2")]
        {
            doc.set_var(menu, bind, key);
        }
        for id in ["BuyForTeammateLabel", "BuyForTeammateHintLabel"] {
            if let Some(panel) = doc.find_in(menu, id) {
                doc.set_visible(panel, false);
            }
        }
        if let Some(info) = doc.find_in(menu, "PreviewPanelContainer") {
            doc.set_class(info, "preview-container", true);
            doc.set_class(info, "preview-container--hidden", true);
        }
    }
    Some(CsgoPanorama {
        active: true,
        buy,
        files: Some(files),
        fonts: font_set,
        doc: Some(doc),
        // Nothing of the feed before the HUD is seen.
        feed_seen: i32::MAX,
        ..CsgoPanorama::default()
    })
}

/// The node the HUD's panels hang from, a child of the CS HUD root.
pub(crate) fn spawn_root(hud: &mut ChildSpawnerCommands, panorama: &mut CsgoPanorama) {
    let root = hud
        .spawn((
            CsgoPanoramaRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                display: Display::Grid,
                ..default()
            },
        ))
        .id();
    panorama.root = Some(root);
}

/// `m:ss`.
fn clock(seconds: i32) -> String {
    let s = seconds.max(0);
    format!("{}:{:02}", s / 60, s % 60)
}

/// The equipment icon of a kill feed weapon (`icons/equipment/<name>.svg`); `None` for a
/// suicide.
fn equipment_icon(csgo_icon: &str) -> Option<&str> {
    match csgo_icon {
        "suicide" => None,
        "knife_default_ct" => Some("knife"),
        "knife_default_t" => Some("knife_t"),
        other => Some(other),
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn update_csgo_panorama(
    mut commands: Commands,
    mut hud: ResMut<CsgoPanorama>,
    surface: Res<crate::surface::Hud2dSurface>,
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    weapons: Option<Res<PreparedWeapons>>,
    view: Res<ViewSubject>,
    feed: Res<crate::cs_hud::CsKillFeed>,
    (time, mut images): (Res<Time>, ResMut<Assets<Image>>),
    (compass, mut hud_images, mut gaps, cameras): (
        Option<Res<assets::SessionCompass>>,
        ResMut<crate::images::HudImages>,
        ResMut<crate::gaps::HudPresentationGaps>,
        Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    ),
    (menu, mut wheel_input, windows, nodes, settings): (
        Res<crate::cs_buymenu::CsBuyMenu>,
        ResMut<crate::cs_buymenu::CsgoBuyWheel>,
        Query<&Window, With<bevy::window::PrimaryWindow>>,
        Query<(&ComputedNode, &bevy::ui::UiGlobalTransform)>,
        Res<frame::GameSettings>,
    ),
    (actions, scores, identity): (
        Option<Res<net::ClientActionInput>>,
        Option<Res<net::Scoreboard>>,
        Option<Res<frame::LaunchIdentity>>,
    ),
) {
    if !hud.active || !crate::cs_hud::replaces_mw2_hud() || !surface.is_ready() {
        return;
    }
    let now = time.elapsed_secs();
    let state = &mut *hud;
    let (Some(files), Some(doc), Some(root)) = (state.files.as_mut(), state.doc.as_mut(), state.root)
    else {
        return;
    };
    doc.set_scale(surface.height() / 1080.0);
    let snap = presented.snapshot();
    let ps = presented.player(local.0).filter(|_| !view.in_killcam());
    let alive = ps.is_some_and(|ps| ps.pm_type < PM_TYPE_DEAD);
    let living = ps.filter(|_| alive);

    // Health and armour (`CCSGO_HudHealthArmor`).
    if let Some(panel) = doc.find("HudHealthArmor") {
        doc.set_class(panel, "hud-HA--active", alive);
        if let Some(ps) = living {
            let health = ps.health.max(0);
            if state.health.is_some_and(|before| health < before) {
                state.damaged_at = Some(now);
            }
            state.health = Some(health);
            let share = health as f32 / ps.max_health.max(1) as f32;
            doc.set_var(panel, "health", &health.to_string());
            if let Some(bar) = doc.find_in(panel, "HealthBar") {
                doc.set_value(bar, share);
            }
            let flashing = state.damaged_at.is_some_and(|at| now - at < DAMAGE_FLASH_SECONDS);
            doc.set_class(panel, "hud-HA--on-damage", flashing);
            doc.set_class(panel, "hud-HA--critical", share <= LOW_HEALTH);
            let armor = ps.cs_armor as i32;
            doc.set_var(panel, "armor", &armor.to_string());
            if let Some(bar) = doc.find_in(panel, "ArmorBar") {
                doc.set_value(bar, armor as f32 / 100.0);
            }
            doc.set_class(panel, "hud-HA--helmet", ps.cs_helmet != 0);
        } else {
            state.health = None;
        }
    }

    // Money (`CCSGO_HudMoney`): only where rounds earn it.
    if let Some(panel) = doc.find("HudMoney") {
        let shown = living.is_some() && crate::cs_hud::bomb_mode(snap);
        doc.set_visible(panel, shown);
        if let Some(ps) = living {
            doc.set_var(panel, "hud-money-amount", &ps.cs_money.to_string());
            doc.set_class(
                panel,
                "money__in-buy-zone",
                ps.cs_buy & playerstate_iw4::cs_buy::ZONE != 0,
            );
        }
    }

    // The weapon panel (`CCSGO_HudWeaponPanel`): magazine and reserve, the bullets, the kit.
    if let Some(panel) = doc.find("HudWeaponPanel") {
        doc.set_class(panel, "WeaponPanel--Hidden", !alive);
        let ammo = living.zip(weapons.as_deref()).and_then(|(ps, w)| {
            crate::cs_hud::ammo_counts(ps, w, &presented, &local)
        });
        let gun_clip = living.zip(weapons.as_deref()).and_then(|(ps, w)| {
            let weapon = weapon_iw4::get_viewmodel_weapon_index(ps);
            let index = weapon_iw4::cs::cs_weapon_index_for(&w.0.script_name_of(weapon))?;
            weapon_iw4::cs::cs_weapon(index).map(|gun| gun.clip.max(1))
        });
        if let Some(content) = doc.find_in(panel, "AmmoContent") {
            doc.set_class(content, "Ammo--Hidden", ammo.is_none());
        }
        if let Some(bg) = doc.find_in(panel, "WeaponPanelBottomBG") {
            doc.set_visible(bg, ammo.is_some());
        }
        if let Some((clip, reserve)) = ammo {
            if let Some(label) = doc.find_in(panel, "AmmoTextClip") {
                doc.set_text(label, &clip.to_string());
            }
            if let Some(label) = doc.find_in(panel, "AmmoTextTotal") {
                doc.set_text(label, &reserve.map_or_else(String::new, |r| format!("/ {r}")));
            }
        }
        if let Some(burst) = doc.find_in(panel, "AmmoBurstIcons") {
            doc.set_class(burst, "Ammo__Burst--Burst", false);
            doc.set_class(burst, "Ammo__Burst--Single", false);
        }
        if let Some(bullets) = doc.find_in(panel, "AmmoAnimBullets") {
            doc.set_class(bullets, "AmmoAnim__Bullets--Hidden", gun_clip.is_none());
            if let (Some(max), Some((clip, _))) = (gun_clip, ammo) {
                let low = clip as f32 / max as f32 <= LOW_AMMO;
                for (i, icon) in doc.children(bullets).into_iter().enumerate() {
                    doc.set_class(icon, "AmmoAnim__BulletIcon--Red", low);
                    doc.set_class(icon, "AmmoAnim__BulletIcon--Hidden", clip <= i as i32);
                }
            }
        }
        if let Some(grenades) = doc.find_in(panel, "AmmoAnimGrenades") {
            doc.set_class(grenades, "AmmoAnim__Grenades--Hidden", true);
        }
        if let Some(icons) = doc.find_in(panel, "KillCountIcons") {
            doc.set_class(icons, "Kills__Count--Hidden", true);
        }
        if let Some(text) = doc.find_in(panel, "KillCountText") {
            doc.set_class(text, "Kills__Count--Hidden", true);
        }
        if let Some(eater) = doc.find_in(panel, "KillEaterContent") {
            doc.set_class(eater, "Kills__KillEater--Hidden", true);
        }
        if let Some(kit) = doc.find_in(panel, "DefuseIcon") {
            doc.set_class(
                kit,
                "WeaponPanelTop__Icon--Show",
                living.is_some_and(|ps| ps.cs_defuser != 0),
            );
        }
    }

    // The team counter (`CCSGO_HudTeamCounter`): the round clock, the scores, who is alive.
    if let Some(panel) = doc.find("HudTeamCounter") {
        if let Some(label) = doc.find_in(panel, "TimerText") {
            let text = crate::cs_hud::round_seconds_left(snap).map_or_else(String::new, clock);
            doc.set_text(label, &text);
        }
        let [ct, t] = crate::csgo_hud::scores(snap).unwrap_or([0, 0]);
        if let Some(label) = doc.find_in(panel, "ScoreCT") {
            doc.set_text(label, &ct.to_string());
        }
        if let Some(label) = doc.find_in(panel, "ScoreT") {
            doc.set_text(label, &t.to_string());
        }
        let mut alive_counts = [0, 0];
        if let Some(snap) = snap {
            for (id, ps) in &snap.players {
                let team = snap.meta.for_client(*id).map_or(0, |m| m.client_state_team);
                if ps.pm_type < PM_TYPE_DEAD
                    && let Some(terrorist) = crate::cs_hud::is_terrorist(Some(snap), team)
                {
                    alive_counts[usize::from(terrorist)] += 1;
                }
            }
        }
        // The avatars (`UpdateAvatarSlot`).
        update_avatars(doc, files, &mut state.avatars, panel, snap, local.0);
        // `UpdateNumberCount`: each side's alive count, or a skull when none is left.
        for (side, alive) in [("CT", alive_counts[0]), ("T", alive_counts[1])] {
            if let Some(label) = doc.find_in(panel, &format!("AliveCount{side}")) {
                doc.set_text(label, &alive.to_string());
                doc.set_visible(label, alive > 0);
            }
            if let Some(label) = doc.find_in(panel, &format!("AliveText{side}")) {
                doc.set_visible(label, alive > 0);
            }
            if let Some(skull) = doc.find_in(panel, &format!("AliveSkull{side}")) {
                doc.set_visible(skull, alive == 0);
            }
        }
    }

    // The kill feed (`CCSGO_HudDeathNotice`): one notice per kill, gone after its lifetime.
    if let Some(list) = doc.find("VisibleNotices") {
        let newest = feed.lines.back().map_or(state.feed_seen, |l| l.start_ms);
        if state.feed_seen == i32::MAX {
            state.feed_seen = newest;
        }
        for line in feed.lines.iter().filter(|l| l.start_ms > state.feed_seen) {
            let Some(notice) = doc.add_snippet(files, list, "DeathNotice") else {
                continue;
            };
            let side = |color: Color| {
                if color == crate::cs_hud::CT_BLUE {
                    "DeathNoticeCTColor"
                } else {
                    "DeathNoticeTColor"
                }
            };
            let suicide = line.attacker.is_none();
            let (attacker_name, attacker_color) =
                line.attacker.clone().unwrap_or_else(|| line.victim.clone());
            if let Some(label) = doc.find_in(notice, "Attacker") {
                doc.set_var(label, "name", &attacker_name);
                doc.set_class(label, side(attacker_color), true);
            }
            if let Some(label) = doc.find_in(notice, "Victim") {
                doc.set_var(label, "name", &line.victim.0);
                doc.set_class(label, side(line.victim.1), true);
            }
            if let Some(image) = doc.find_in(notice, "Weapon") {
                let src = equipment_icon(line.csgo_icon)
                    .map(|name| format!("file://{{images}}/icons/equipment/{name}.svg"));
                doc.set_image(image, src.as_deref());
            }
            doc.set_class(notice, "DeathNotice_Killer", line.killer);
            doc.set_class(notice, "DeathNotice_Victim", line.local && !line.killer);
            doc.set_class(notice, "DeathNoticeHeadShot", line.headshot);
            doc.set_class(notice, "DeathNoticeSuicide", suicide);
            state.notices.push(Notice {
                panel: notice,
                since: now,
                lifetime: if line.local {
                    NOTICE_SECONDS * NOTICE_LOCAL_SCALE
                } else {
                    NOTICE_SECONDS
                },
            });
        }
        state.feed_seen = state.feed_seen.max(newest);
        state.notices.retain(|n| {
            let age = now - n.since;
            if age > n.lifetime + NOTICE_FADE_SECONDS {
                doc.remove(n.panel);
                return false;
            }
            if age > n.lifetime {
                doc.set_class(n.panel, "FadeOut", true);
            }
            true
        });
    }

    // The radar (`cl_radar csgo`): CS:GO's round radar over MW2's minimap, with CS:GO's player
    // icons; money sits under whichever radar shows.
    let csgo_radar = settings.radar == "csgo";
    if let Some(radar) = doc.find("HudRadar") {
        doc.set_visible(radar, csgo_radar && ps.is_some());
        doc.set_class(radar, "hud-radar__active", csgo_radar && ps.is_some());
        // Only the round radar: no square one, and no bomb or hostage markers (the game shows
        // those as they happen, which this radar does not yet).
        for id in [
            "Radar__Square",
            "RI_BombDefuserPackage",
            "HZone0",
            "HZone1",
            "HZone2",
            "HZone3",
            "DirectionArrow",
        ] {
            if let Some(panel) = doc.find_in(radar, id) {
                doc.set_visible(panel, false);
            }
        }
        if state.radar_icons.is_empty()
            && let Some(inner) = doc.find_in(radar, "Radar__Round--Inner")
        {
            for _ in 0..RADAR_ICONS {
                if let Some(icons) = doc.add_snippet(files, inner, "PlayerIconPackage") {
                    for child in doc.find_class_in(icons, "PI_Text") {
                        doc.set_visible(child, false);
                    }
                    if let Some(rotated) = doc.find_in(icons, "PI_FirstRotated") {
                        for child in doc.children(rotated) {
                            doc.set_visible(child, false);
                        }
                    }
                    doc.set_visible(icons, false);
                    state.radar_icons.push(icons);
                }
            }
        }
        let drawable = csgo_radar
            .then(|| crate::compass::resolve(compass.as_deref(), &mut hud_images, &mut gaps))
            .flatten();
        if let (Some(ps), Some(drawable), Some(inner)) =
            (ps, drawable, doc.find_in(radar, "Radar__Round--Inner"))
        {
            let size = (RADAR_SIZE * doc.scale).round().max(16.0) as u32;
            if state.radar_image.as_ref().is_none_or(|(_, s)| *s != size) {
                let image = Image::new_fill(
                    bevy::render::render_resource::Extent3d {
                        width: size,
                        height: size,
                        depth_or_array_layers: 1,
                    },
                    bevy::render::render_resource::TextureDimension::D2,
                    &[0, 0, 0, 0],
                    bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
                    bevy::asset::RenderAssetUsages::default(),
                );
                state.radar_image = Some((images.add(image), size));
            }
            if let Some((handle, size)) = state.radar_image.clone() {
                let map_ns = hud_images.map_namespace();
                if let Some((w, h, rgba)) = hud_images.rgba(map_ns, &drawable.image_name)
                    && let Some(mut image) = images.get_mut(&handle)
                    && let Some(data) = image.data.as_mut()
                {
                    draw_radar(
                        data,
                        size,
                        (*w, *h, rgba.as_slice()),
                        &drawable.bounds,
                        [ps.origin[0], ps.origin[1]],
                        ps.viewangles[1],
                        drawable.max_range,
                    );
                }
                doc.set_image_handle(inner, Some(handle));
            }
            let local_team = snap
                .and_then(|s| s.meta.for_client(local.0))
                .map_or(0, |m| m.client_state_team);
            let (sin, cos) = ps.viewangles[1].to_radians().sin_cos();
            let units = drawable.max_range / RADAR_SIZE;
            let half = RADAR_SIZE * 0.5;
            // Where a world point lands from the radar's middle, pulled in to `rim` when off it
            // (and whether it was on it).
            let project = |to: [f32; 2], rim: f32| {
                let d = [to[0] - ps.origin[0], to[1] - ps.origin[1]];
                let x = (d[0] * sin - d[1] * cos) / units;
                let y = -(d[0] * cos + d[1] * sin) / units;
                let len = (x * x + y * y).sqrt();
                if len > rim {
                    (x * rim / len, y * rim / len, false)
                } else {
                    (x, y, true)
                }
            };
            // The bomb sites (`PlaceGoalIcons`): CS:GO's A and B over the sites' radar
            // objectives, remembered for the map once seen; smaller and dimmer on the radar,
            // full size at its rim when off it.
            if state.radar_sites.0 != drawable.image_name {
                state.radar_sites = (drawable.image_name.clone(), [None; 2]);
            }
            for objective in snap.iter().flat_map(|s| &s.meta.objectives.compass) {
                let icon = objective.icon.as_str();
                let site = if !icon.contains("waypoint_") {
                    None
                } else if icon.ends_with("_a") {
                    Some(0)
                } else if icon.ends_with("_b") {
                    Some(1)
                } else {
                    None
                };
                if let Some(site) = site {
                    state.radar_sites.1[site] = Some([objective.origin[0], objective.origin[1]]);
                }
            }
            for (site, id) in ["BombZoneA", "BombZoneB"].into_iter().enumerate() {
                let Some(zone) = doc.find_in(radar, id) else {
                    continue;
                };
                let Some(at) = state.radar_sites.1[site] else {
                    doc.set_visible(zone, false);
                    continue;
                };
                let (x, y, on_map) = project(at, half - 8.0);
                doc.set_visible(zone, true);
                doc.set_class(zone, "BombZone_OnMap", on_map);
                doc.set_live_transform(
                    zone,
                    (RADAR_PANEL_SIZE * 0.5 + x, RADAR_PANEL_SIZE * 0.5 + y),
                    0.0,
                );
            }
            // The players (`SetupIconsFromStates`): teammates always, enemies while the server
            // has them spotted (`cs_spotted`), then for 6 s as a fading ghost where last seen;
            // the dead (if seen dying) as a cross fading over 4 s; anyone off the radar as an
            // arrow at its rim pointing their way.
            let mut shown: Vec<RadarIcon> = Vec::new();
            // Teammates' pings: CS:GO's ring where each is, an arrow at the rim pointing its way
            // when off the radar; under the players.
            for ping in team_pings(snap, local.0, settings.ping_mute) {
                let (x, y, on_map) = project([ping.origin[0], ping.origin[1]], half - 6.0);
                shown.push(RadarIcon {
                    at: (half + x, half + y),
                    icon: if on_map { "PlayerPing" } else { "PlayerPingOffMap" }.to_owned(),
                    cone: false,
                    turn: if on_map { 0.0 } else { y.atan2(x) + std::f32::consts::FRAC_PI_2 },
                    fade: ping.fade,
                    color: None,
                });
            }
            if let Some(snap) = snap {
                let mut present = Vec::new();
                // Teammates in their CS:GO colours, by their place on the team (as their avatars).
                let mut mates: Vec<u32> = snap
                    .players
                    .iter()
                    .filter(|(id, _)| {
                        local_team != 0
                            && snap.meta.for_client(*id).is_some_and(|m| m.client_state_team == local_team)
                    })
                    .map(|(id, _)| id.0)
                    .collect();
                mates.sort_unstable();
                for (id, other) in &snap.players {
                    let meta = snap.meta.for_client(*id);
                    let team = meta.map_or(0, |m| m.client_state_team);
                    let me = *id == local.0;
                    let friend = me || (local_team != 0 && team == local_team);
                    if team == 0 && !me {
                        continue;
                    }
                    present.push(id.0);
                    let dead = other.pm_type >= PM_TYPE_DEAD;
                    let spotted = friend || meta.is_some_and(|m| m.cs_spotted);
                    let here = [other.origin[0], other.origin[1]];
                    let track = state.radar_tracks.entry(id.0).or_insert(RadarTrack {
                        at: here,
                        seen_at: None,
                        dead_at: None,
                    });
                    if dead {
                        track.dead_at.get_or_insert(now);
                    } else {
                        track.dead_at = None;
                    }
                    if spotted {
                        track.at = here;
                        track.seen_at = Some(now);
                    }
                    let (state_name, fade) = match (dead, spotted, track.dead_at, track.seen_at) {
                        (true, true, Some(died), _) if now - died < RADAR_DEAD_FADE_S => {
                            ("Death", 1.0 - (now - died) / RADAR_DEAD_FADE_S)
                        }
                        (false, true, _, _) => ("OnMap", 1.0),
                        (false, false, _, Some(seen)) if now - seen < RADAR_GHOST_FADE_S => {
                            ("Ghost", 1.0 - (now - seen) / RADAR_GHOST_FADE_S)
                        }
                        _ => continue,
                    };
                    let (x, y, on_map) = project(track.at, half - 6.0);
                    let state_name = if on_map || state_name == "Death" {
                        state_name
                    } else {
                        "OffMap"
                    };
                    let side = if !friend {
                        "Enemy"
                    } else if crate::cs_hud::is_terrorist(Some(snap), team).unwrap_or(false) {
                        "T"
                    } else {
                        "CT"
                    };
                    // Off the radar the arrow points out; a living teammate on it faces where
                    // they look; everything else stays upright.
                    let turn = if state_name == "OffMap" {
                        y.atan2(x) + std::f32::consts::FRAC_PI_2
                    } else if friend && !me && state_name == "OnMap" {
                        (ps.viewangles[1] - other.viewangles[1]).to_radians()
                    } else {
                        0.0
                    };
                    shown.push(RadarIcon {
                        at: (half + x, half + y),
                        icon: format!("{side}{state_name}"),
                        cone: me && state_name == "OnMap",
                        turn,
                        fade,
                        color: mates
                            .iter()
                            .position(|mate| *mate == id.0)
                            .filter(|_| friend)
                            .and_then(|slot| TEAMMATE_COLORS.get(slot))
                            .map(|(rgb, _)| *rgb),
                    });
                }
                state.radar_tracks.retain(|id, _| present.contains(id));
            }
            // The local player draws last, over the others.
            shown.sort_by_key(|icon| icon.cone);
            for (k, &icons) in state.radar_icons.iter().enumerate() {
                let Some(shown) = shown.get(k) else {
                    doc.set_visible(icons, false);
                    continue;
                };
                doc.set_visible(icons, true);
                doc.set_live_transform(icons, shown.at, 0.0);
                // Fades restyle, so only in tenths.
                let fade = (shown.fade * 10.0).ceil() / 10.0;
                let opacity = (fade < 1.0).then(|| format!("{fade:.1}"));
                doc.set_style(icons, "opacity", opacity.as_deref());
                if let Some(rotated) = doc.find_in(icons, "PI_FirstRotated") {
                    doc.set_live_transform(rotated, (0.0, 0.0), shown.turn);
                }
                for id in RADAR_ICON_IDS {
                    if let Some(image) = doc.find_in(icons, id) {
                        doc.set_visible(image, id == shown.icon || (shown.cone && id == "ViewFrustrum"));
                        if id == shown.icon {
                            doc.set_style(image, "wash-color-fast", shown.color);
                        }
                    }
                }
            }
        } else {
            for &icons in &state.radar_icons {
                doc.set_visible(icons, false);
            }
            for id in ["BombZoneA", "BombZoneB"] {
                if let Some(zone) = doc.find_in(radar, id) {
                    doc.set_visible(zone, false);
                }
            }
        }
    }
    // Pings on screen (`#VisiblePlayerPings`): CS:GO's marker over each teammate's pinged spot,
    // seen through walls; a danger ping (a double tap) shows the alert icon in red. The rest of
    // CS:GO's reticle layer (its crosshair, target IDs) stays off: this HUD draws those.
    if let Some(reticle) = doc.find("HudReticle") {
        for id in [
            "FriendCrosshair",
            "Crosshair",
            "CrosshairObserver",
            "TargetID",
            "VisiblePlayerIDs",
            "UnusedPlayerIDs",
            "UnusedPlayerPings",
        ] {
            if let Some(panel) = doc.find_in(reticle, id) {
                doc.set_visible(panel, false);
            }
        }
        if state.ping_markers.is_empty()
            && let Some(parent) = doc.find_in(reticle, "VisiblePlayerPings")
        {
            for _ in 0..PING_MARKERS {
                if let Some(marker) = doc.add_snippet(files, parent, "PlayerPing") {
                    for id in ["WeaponIcon", "CrateIcon", "ItemIcon", "LocationLabel"] {
                        if let Some(panel) = doc.find_in(marker, id) {
                            doc.set_visible(panel, false);
                        }
                    }
                    if let Some(icon) = doc.find_in(marker, "DangerIcon") {
                        doc.set_style(icon, "wash-color-fast", Some("#ff2b2b"));
                    }
                    doc.set_visible(marker, false);
                    state.ping_markers.push(marker);
                }
            }
        }
        let pings = team_pings(snap.filter(|_| ps.is_some()), local.0, settings.ping_mute);
        let camera = cameras.iter().find(|(camera, _)| camera.is_active);
        for (k, &marker) in state.ping_markers.iter().enumerate() {
            let shown = pings.get(k).zip(camera).and_then(|(ping, (camera, transform))| {
                let pixel = camera
                    .world_to_viewport(transform, Vec3::from_array(ping.origin))
                    .ok()
                    .filter(|pixel| pixel.is_finite())?;
                Some((pixel / doc.scale, ping))
            });
            let Some((pixel, ping)) = shown else {
                doc.set_visible(marker, false);
                continue;
            };
            doc.set_visible(marker, true);
            doc.set_live_transform_scaled(marker, (pixel.x, pixel.y), 0.0, PING_MARKER_SCALE);
            let fade = (ping.fade * 10.0).ceil() / 10.0;
            let opacity = (fade < 1.0).then(|| format!("{fade:.1}"));
            doc.set_style(marker, "opacity", opacity.as_deref());
            for (id, on) in [("DefaultIcon", !ping.danger), ("DangerIcon", ping.danger)] {
                if let Some(icon) = doc.find_in(marker, id) {
                    doc.set_visible(icon, on);
                }
            }
        }
    }

    // The round-end panel (`CCSGO_HudWinPanel`, `cl_roundbanner csgo`): when a round is decided
    // (a new `cs_round_end`), the winners' banner with CS:GO's title and the round's MVP
    // (`cs_round_mvp`) and why; up until the next round starts.
    if let Some(panel) = doc.find("HudWinPanel") {
        let round_end = crate::cs_hud::server_info(snap, "cs_round_end").map(str::to_owned);
        if snap.is_some() && state.win_seen.as_ref() != Some(&round_end) {
            // A value already there when we join is an old round: only a change shows.
            if state.win_seen.is_some()
                && let Some(end) = &round_end
            {
                state.win_shown = Some((end.clone(), now));
            }
            state.win_seen = Some(round_end);
        }
        let up = state
            .win_shown
            .as_ref()
            .filter(|(_, since)| now - *since < WIN_PANEL_SECONDS)
            .filter(|_| settings.round_banner == "csgo" && !view.in_killcam())
            .map(|(end, _)| end.clone());
        doc.set_class(panel, "WinPanelRoot--ShowAnim", up.is_some());
        doc.set_class(panel, "WinPanelRoot--ForceHide", up.is_none());
        for id in [
            "RoundImpactScoreReport",
            "id-survival-winpanel",
            "Funfact",
            "Progress",
            "Medals",
            "MedalStats",
            "RankUp",
            "GunGameExtra",
        ] {
            if let Some(row) = doc.find_in(panel, id) {
                doc.set_class(row, "WinPanelRow--Hidden", true);
                doc.set_visible(row, false);
            }
        }
        if let Some(end) = up {
            let winner = end.split_whitespace().nth(1).unwrap_or("");
            doc.set_class(panel, "WinPanelRoot--TWin", winner == "t");
            doc.set_class(panel, "WinPanelRoot--CTWin", winner == "ct");
            doc.set_class(panel, "WinPanelRoot--Draw", winner != "t" && winner != "ct");
            if let Some(title) = doc.find_in(panel, "Title") {
                doc.set_text(
                    title,
                    match winner {
                        "t" => "#SFUI_WinPanel_T_Win",
                        "ct" => "#SFUI_WinPanel_CT_Win",
                        _ => "#SFUI_WinPanel_Round_Draw",
                    },
                );
            }
            // The MVP (`ShowMVP`), for this round only.
            let mvp = crate::cs_hud::server_info(snap, "cs_round_mvp")
                .and_then(|value| value.strip_prefix(&format!("{end}|")))
                .and_then(|rest| rest.split_once('|'))
                .and_then(|(client, reason)| {
                    let client = client.parse::<u32>().ok()?;
                    let meta = snap?.meta.for_client(sim::ClientId(client))?;
                    let name = entity_iw4::client_state_name(&meta.name)?.to_owned();
                    Some((name, reason.to_owned()))
                });
            if let Some(mvp_panel) = doc.find_in(panel, "MVP") {
                doc.set_class(mvp_panel, "MVP--Hidden", mvp.is_none());
                doc.set_visible(mvp_panel, mvp.is_some());
            }
            for id in ["MVPMusicKit", "MVPGunGameAvatars", "MVPAvatar"] {
                if let Some(part) = doc.find_in(panel, id) {
                    doc.set_visible(part, false);
                }
            }
            if let Some((name, reason)) = mvp {
                doc.set_var(panel, "mvp", &name);
                if let Some(label) = doc.find_in(panel, "MVPWinnerName") {
                    doc.set_text(
                        label,
                        match reason.as_str() {
                            "bombplant" => "#Panorama_winpanel_mvp_award_bombplant",
                            "bombdefuse" => "#Panorama_winpanel_mvp_award_bombdefuse",
                            _ => "#Panorama_winpanel_mvp_award_kills",
                        },
                    );
                }
            }
        }
    }

    // The end-of-match map vote (`CSGOEndOfMatchVotingPanel`): once the match is over, a tile
    // per map the server offers (its MW2 loading screen, its name and its number key, the votes
    // for it out of those it needs), the one voted for selected and the winner ticked.
    if let Some(eom) = doc.find("EndOfMatch") {
        let vote = crate::cs_hud::server_info(snap, "cs_mapvote").and_then(parse_map_vote);
        // The server offers the vote (`cs_mapvote`) only once the match is over.
        let show = vote.is_some();
        doc.set_class(eom, "eom--reveal", show);
        doc.set_visible(eom, show);
        if let Some((maps, counts, needed, winner)) = vote.filter(|_| show) {
            if state.vote_tiles.is_empty()
                && let Some(layout) = doc.find_in(eom, "id-eom-layout")
            {
                doc.load_snippet(files, layout, "snippet-eom-layout--default");
                for id in ["eom-win", "eom-rank", "eom-skillgroup", "eom-drops", "id-content-navbar__tabs", "id-display-timer-progress-bar-container"] {
                    if let Some(panel) = doc.find_in(eom, id) {
                        doc.set_visible(panel, false);
                    }
                }
                if let Some(voting) = doc.find_in(eom, "eom-voting") {
                    doc.set_class(voting, "eom-panel--active", true);
                }
                if let Some(list) = doc.find_in(eom, "id-map-selection-list") {
                    for map in &maps {
                        let tile = doc.add_element(files, list, "RadioButton", &[]);
                        doc.load_snippet(files, tile, "MapGroupSelection");
                        if let Some(carousel) = doc.find_in(tile, "MapGroupImagesCarousel") {
                            let picture = doc.add_element(
                                files,
                                carousel,
                                "Panel",
                                &[("class", "map-selection-btn__screenshot")],
                            );
                            state.vote_pictures.push((picture, map.clone()));
                        }
                        state.vote_tiles.push((tile, map.clone()));
                    }
                }
            }
            // A map's picture a frame, read from its zone's loading screen.
            if let Some((picture, map)) = state.vote_pictures.pop()
                && let Some(handle) = map_picture(&map, &mut images)
            {
                doc.set_image_handle(picture, Some(handle));
            }
            for (k, (tile, map)) in state.vote_tiles.iter().enumerate() {
                if let Some(name) = doc.find_in(*tile, "MapGroupName") {
                    doc.set_text(name, &format!("{}  {}", k + 1, map_title(map)));
                }
                if let Some(count) = doc.find_in(*tile, "id-map-selection-btn__count") {
                    doc.set_text(count, &format!("{}/{needed}", counts.get(k).copied().unwrap_or(0)));
                }
                doc.set_selected(*tile, menu.last_map_vote == u8::try_from(k).ok());
                if let Some(check) = doc.find_in(*tile, "id-map-selection-btn__winner") {
                    doc.set_class(check, "appear", winner == Some(k));
                    doc.set_visible(check, winner == Some(k));
                }
            }
        }
    }

    // The scoreboard (TAB, and at the match's end).
    {
        let down = actions.as_ref().is_some_and(|a| a.client.kb.scores.active);
        let show = snap.is_some_and(|snap| crate::scoreboard::displayed(down, snap, local.0));
        let map = identity.as_ref().map_or(String::new(), |i| map_title(&i.zone));
        let data = scoreboard_data(snap, local.0, scores.as_deref(), weapons.as_deref(), map);
        state.scoreboard.update(doc, files, show, &data);
    }

    if let Some(money) = doc.find("HudMoney") {
        // Under MW2's minimap (its square, 240 pixels from the top) when that is the radar.
        doc.set_style(money, "margin-top", (!csgo_radar).then_some("250px"));
    }

    // The buy menu (`CCSGO_BuyMenu`): the category wheel, or the open category's items; the
    // wedge under the mouse found by its angle (`CCSGO_RadialSelector`).
    wheel_input.drawn = false;
    wheel_input.hover = None;
    if let (Some(buy), Some(menu_panel)) = (state.buy.as_ref(), doc.find("BuyMenu")) {
        let page = menu.csgo_page().map(str::to_owned);
        doc.set_visible(menu_panel, page.is_some());
        wheel_input.drawn = true;
        let mut parts = page.as_deref().unwrap_or("").split(':').skip(1);
        let (side, page_name) = (parts.next().unwrap_or("ct"), parts.next().unwrap_or(""));
        let terrorist = side == "t";
        if let Some(logo) = doc.find_in(menu_panel, "TeamLogo") {
            let name = if terrorist { "t_logo" } else { "ct_logo" };
            doc.set_image(logo, Some(&format!("file://{{images}}/icons/{name}.svg")));
        }
        let money = living.map_or(0, |ps| ps.cs_money as i32);
        doc.set_visible(buy.categories.panel, page.is_some() && page_name == "main");
        let mut open = (page_name == "main").then_some((&buy.categories, None));
        for (w, (name, _)) in buy.items.iter().zip(BUY_CATEGORIES) {
            let shown = page.is_some() && page_name == name;
            doc.set_visible(w.panel, shown);
            let slots =
                crate::cs_buymenu::csgo_items(name, terrorist, weapon_iw4::cs::cs_loadout_flags(&settings.loadout_pistol, &settings.loadout_m4)).map(|(_, s)| s);
            for (i, &button) in w.wedges.iter().enumerate() {
                let item = slots.and_then(|s| s.get(i).copied().flatten());
                doc.set_class(button, "no-hover", item.is_none());
                let Some(contents) = doc.find_in(button, "Contents") else {
                    continue;
                };
                let price = item.and_then(|item| weapon_iw4::cs::buy_price(item, sim::cs_settings::shooting_mode()));
                if let Some(label) = doc.find_in(contents, "ItemName") {
                    doc.set_text(label, item.map_or("", crate::cs_buymenu::csgo_item_name));
                }
                if let Some(label) = doc.find_in(contents, "ItemPrice") {
                    doc.set_text(label, &price.map_or_else(String::new, |p| format!("${p}")));
                }
                if let Some(icon) = doc.find_in(contents, "ItemIcon") {
                    let src = item.map(|i| format!("file://{{images}}/icons/equipment/{}.svg", buy_icon(i)));
                    doc.set_image(icon, src.as_deref());
                }
                doc.set_class(button, "buywheel-cant-afford", price.is_some_and(|p| money < p));
            }
            if shown {
                open = Some((w, slots));
            }
        }
        // Hover: inside the wheel's circle, the wedge the mouse's angle falls in.
        let cursor = windows.single().ok().and_then(Window::cursor_position);
        let mut hovered = None;
        if let Some((w, slots)) = open
            && let Some(cursor) = cursor
            && let Some((node, transform)) = doc.entity(w.panel).and_then(|e| nodes.get(e).ok())
        {
            let inverse = node.inverse_scale_factor();
            let center = transform.translation * inverse;
            let size = node.size() * inverse;
            let radius = size.x.min(size.y) * 0.5;
            let d = cursor - center;
            if d.length_squared() < radius * radius {
                let mut angle = d.y.atan2(d.x).to_degrees() - w.start;
                if angle < 0.0 {
                    angle += 360.0;
                }
                let index = (angle % 360.0 / (360.0 / w.wedges.len() as f32)) as usize;
                let filled = slots.is_none_or(|s| s.get(index).copied().flatten().is_some());
                if index < w.wedges.len() && filled {
                    hovered = Some(index);
                }
            }
        }
        for w in std::iter::once(&buy.categories).chain(buy.items.iter()) {
            let current = open.is_some_and(|(o, _)| o.panel == w.panel);
            for (i, &button) in w.wedges.iter().enumerate() {
                doc.set_class(button, "radial-select-hover", current && hovered == Some(i));
            }
        }
        wheel_input.hover = hovered.map(|i| i as u8 + 1);
        // The item details for the hovered item (the last one stays up, as in CS:GO); hidden
        // over the categories.
        if let Some(info) = doc.find_in(menu_panel, "PreviewPanelContainer") {
            let item = open.and_then(|(_, slots)| slots).zip(hovered).and_then(|(slots, i)| {
                slots.get(i).copied().flatten()
            });
            if let Some(item) = item {
                doc.set_class(info, "preview-container--hidden", false);
                doc.set_class(info, "preview-container--details", true);
                show_item_details(doc, info, item);
            } else if page_name == "main" || page.is_none() {
                doc.set_class(info, "preview-container--details", false);
                doc.set_class(info, "preview-container--hidden", true);
            }
        }
        // Buy time left (`mp_buytime`, from the server's `cs_buy_end`): during the freeze what
        // is left of it and all the buy time, then the time to when buying ends.
        if let Some(countdown) = doc.find_in(menu_panel, "Countdown") {
            let info = crate::cs_hud::server_info(snap, "cs_buy_end").unwrap_or_default();
            let left_ms = if let Some(buy) = info.strip_prefix("freeze ") {
                let freeze = crate::cs_hud::round_seconds_left(snap).unwrap_or(0);
                i64::from(freeze) * 1000 + buy.parse::<i64>().unwrap_or(0)
            } else {
                let now = snap.map_or(0, |s| i64::from(s.tick.0) * i64::from(sim::MATCH_TICK_MS));
                info.parse::<i64>().map_or(0, |end| end - now)
            };
            let left = i32::try_from((left_ms.max(0) + 999) / 1000).unwrap_or(0);
            doc.set_var(countdown, "duration", &clock(left));
        }
    }

    doc.sync(root, &mut commands, files, &mut images, &state.fonts);
}
