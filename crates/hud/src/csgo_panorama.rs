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
        _ => return None,
    })
}

#[derive(Component)]
pub(crate) struct CsgoPanoramaRoot;

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
}

/// Reads CS:GO's HUD; `None` without CS:GO (or its packed UI and fonts).
pub(crate) fn load(fonts: &mut Assets<Font>) -> Option<CsgoPanorama> {
    let files = panorama::Files::open()?;
    let font_set = files.fonts(fonts)?;
    let mut doc = panorama::Doc::load(&files, HUD_LAYOUT, &sublayout)?;
    // `cl_hud_color 0`, the default HUD colour.
    let root = doc.root();
    doc.set_class(root, "csgo-hud--cl-hud-color-0", true);
    if let Some(fill) = doc.find("HealthBar_Left") {
        doc.set_class(fill, "cl-hud-background-color", true);
    }
    // The team counter as `CCSGO_HudTeamCounter` sets it up for a classic match: no bomb
    // status, join banner or player strips (yet); the clock and score shown, at the top.
    if let Some(counter) = doc.find("HudTeamCounter") {
        for id in [
            "BombStatus",
            "BombPlanted",
            "BombPlantedLines",
            "BombDefused",
            "TeamAll",
            "TeamLargeCT",
            "TeamLargeT",
            "TeamSmallContainerCT",
            "TeamSmallContainerT",
            "JoinPanelParent",
        ] {
            if let Some(panel) = doc.find_in(counter, id) {
                doc.set_visible(panel, false);
            }
        }
        doc.set_class(counter, "TeamCounter__Align--Top", true);
        doc.set_class(counter, "TeamCounter__Flow--Down", true);
    }
    Some(CsgoPanorama {
        active: true,
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

    doc.sync(root, &mut commands, files, &mut images, &state.fonts);
}
