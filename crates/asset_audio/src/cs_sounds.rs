//! Counter-Strike 1.6 weapon sounds, read at runtime from the local CS install's
//! `sound/weapons/*.wav` and added to the match sound bank as aliases `cs/weapons/<name>`
//! (positional, for other players) and `cs/weapons/<name>/plr` (the local player's own gun).
//! Each borrows its mixing settings (volume, distance falloff, channel) from MW2's own gunshot
//! aliases, so CS sounds sit in the mix like MW2's did. Nothing is copied out of the install.

use std::path::Path;

use crate::{LooseClip, SoundCatalog};

/// Alias prefix of Counter-Strike: Source sound script entries (`css/weapon_ak47.single`).
pub const CSS_SOUND_PREFIX: &str = "css/";

/// Alias prefix of CS:GO sound script entries (`csgo/weapon_ak47.single`).
pub const CSGO_SOUND_PREFIX: &str = "csgo/";

/// The CS:GO sound script groups our guns, knife, grenades and bomb play from (`weapon_ak47.`
/// …): their shots and the sounds their animations call. CS:GO's other weapon sounds stay unread.
const CSGO_SOUND_GROUPS: [&str; 55] = [
    // A head hit: on a helmet, on a bare head (each with the shooter's feedback version).
    "player.damagehelmet",
    "player.damageheadshot",
    "weapon_ak47.",
    "weapon_m4a1.",
    "weapon_awp.",
    "weapon_deagle.",
    "weapon_usp.",
    "weapon_glock.",
    "weapon_p250.",
    "weapon_fiveseven.",
    "weapon_elite.",
    "weapon_nova.",
    "weapon_xm1014.",
    "weapon_mac10.",
    "weapon_mp9.",
    "weapon_mp5.",
    "weapon_ump45.",
    "weapon_p90.",
    "weapon_galilar.",
    "weapon_famas.",
    "weapon_sg556.",
    "weapon_aug.",
    "weapon_ssg08.",
    "weapon_g3sg1.",
    "weapon_scar20.",
    "weapon_m249.",
    "weapon_mp7.",
    "weapon_bizon.",
    "weapon_mag7.",
    "weapon_sawedoff.",
    "weapon_negev.",
    "weapon_hkp2000.",
    "weapon_tec9.",
    "weapon_cz75a.",
    "weapon_revolver.",
    "weapon_knife.",
    // The other knives' draws, flips and inspects (their viewmodels call these, not
    // `Weapon_Knife.*`), and the cloth rustle some inspects play.
    "butterflyknife.",
    "knife.gypsy.",
    "knife.stilleto.",
    "knife.ursus.",
    "knife.widow.",
    "knifebowie.",
    "knifefalchion.",
    "knifepush.",
    "weapon.weaponmove",
    "weapon_hegrenade.",
    // The grenades: drawing one and pulling its pin, bouncing, and going off (the HE's blast is
    // `BaseGrenade.Explode`, the smoke's pop `BaseSmokeEffect.Sound`).
    "hegrenade.",
    "flashbang.",
    "smokegrenade.",
    "basegrenade.explode",
    "basesmokeeffect.sound",
    // A teammate's ping, where it was placed.
    "ui.playerping",
    "weapon_flashbang.",
    "weapon_smokegrenade.",
    "weapon_c4.",
];

/// Alias prefix every CS sound is registered under.
pub const CS_SOUND_PREFIX: &str = "cs/weapons/";
/// Alias prefix of the CS 1.6 player sounds (`sound/player/pl_fallpain1.wav` → `cs/player/...`).
pub const CS_PLAYER_SOUND_PREFIX: &str = "cs/player/";
/// Suffix of the non-positional alias for the local player's own sounds.
pub const CS_SOUND_PLAYER_SUFFIX: &str = "/plr";
/// Alias prefix of the CS 1.6 radio voice (`sound/radio/ctwin.wav` → `cs/radio/ctwin`).
pub const CS_RADIO_SOUND_PREFIX: &str = "cs/radio/";
/// Round and bomb announcements under names that don't depend on the install
/// (`cs_event_ctwin`, `cs_event_terwin`, `cs_event_rounddraw`, `cs_event_bombplanted`,
/// `cs_event_bombdefused`): the bomb mode's script plays these, and whichever game is
/// installed fills them with its own voice (CS:S `Event.*`, CS 1.6 `radio/*.wav`).
pub const CS_EVENT_PREFIX: &str = "cs_event_";
/// Ladder steps under install-free names, for your own (`cs_ladder_plr_step`, flat) and other
/// players' (`cs_ladder_step`, positional), modelled on MW2's ladder steps: CS:S's
/// `player/footsteps/ladder1-4`, else CS 1.6's `player/pl_ladder1-4`.
pub const CS_LADDER_STEP: &str = "cs_ladder_step";
pub const CS_LADDER_STEP_PLR: &str = "cs_ladder_plr_step";

/// Register the ladder step waves (`rate, channels, pcm16`) under both ladder step names.
fn add_ladder_steps(catalog: &mut SoundCatalog, waves: &[(u32, i32, Vec<u8>)]) -> bool {
    if waves.is_empty() {
        return false;
    }
    let mut added = true;
    for (name, templates) in [
        (CS_LADDER_STEP, ["step_run_ladder", "step_run_default"]),
        (CS_LADDER_STEP_PLR, ["step_run_plr_ladder", "step_run_plr_default"]),
    ] {
        let Some(template) = templates
            .into_iter()
            .find(|name| catalog.has_alias(crate::AssetNamespace::Iw4, name))
        else {
            return false;
        };
        let clips = waves
            .iter()
            .enumerate()
            .map(|(i, (rate, channels, pcm))| LooseClip {
                name: format!("{name}#{i}"),
                rate: *rate,
                channels: *channels,
                pcm16: pcm.clone().into(),
            })
            .collect();
        added &= catalog.add_loose_alias(name, &template, clips);
    }
    added
}

/// The bomb's sounds under install-free names (`cs_c4_plant`, `cs_c4_disarm`, `cs_c4_disarmed`,
/// `cs_c4_explode`, `cs_c4_click`, `cs_c4_beep1..5`), each mixed like the MW2 bomb sound it
/// stands in for. CS:S has one beep that speeds up and carries farther step by step; CS 1.6
/// steps through five sounds.
pub const CS_C4_PREFIX: &str = "cs_c4_";

/// The CS sounds the scripts play by name (the bomb mode's, and every death's cry). The server
/// checks every script sound against the zones' aliases; these come from the local CS install on
/// each client instead, so they are added to that list.
pub const CS_SCRIPT_SOUNDS: [&str; 16] = [
    CS_DEATH_CRY,
    "cs_c4_plant",
    "cs_c4_disarm",
    "cs_c4_disarmed",
    "cs_c4_explode",
    "cs_c4_click",
    "cs_c4_beep1",
    "cs_c4_beep2",
    "cs_c4_beep3",
    "cs_c4_beep4",
    "cs_c4_beep5",
    "cs_event_terwin",
    "cs_event_ctwin",
    "cs_event_rounddraw",
    "cs_event_bombplanted",
    "cs_event_bombdefused",
];

/// CS 1.6 `sound/weapons` waves behind each bomb sound.
const CS16_C4: [(&str, &str); 10] = [
    ("plant", "c4_plant"),
    ("disarm", "c4_disarm"),
    ("disarmed", "c4_disarmed"),
    ("explode", "c4_explode1"),
    ("click", "c4_click"),
    ("beep1", "c4_beep1"),
    ("beep2", "c4_beep2"),
    ("beep3", "c4_beep3"),
    ("beep4", "c4_beep4"),
    ("beep5", "c4_beep5"),
];

/// CS:S sound script entries behind each bomb sound. CS:S has one beep; its five steps are the
/// same sound carrying farther as the timer runs down (see [`beep_reach`]).
const CSS_C4: [(&str, &str); 10] = [
    ("plant", "c4.plant"),
    ("disarm", "c4.disarmstart"),
    ("disarmed", "c4.disarmfinish"),
    ("explode", "c4.explode"),
    ("click", "c4.click"),
    ("beep1", "c4.plantsound"),
    ("beep2", "c4.plantsound"),
    ("beep3", "c4.plantsound"),
    ("beep4", "c4.plantsound"),
    ("beep5", "c4.plantsound"),
];

/// Voices and bomb sounds play at their own pitch: the gunshot aliases CS sounds borrow their
/// mixing from vary it shot to shot, which made the radio voice sound off now and then.
fn steady_pitch(row: &mut crate::CapturedAlias) {
    row.pitch_min = 1.0;
    row.pitch_max = 1.0;
}

/// The CS 1.6 player waves read (`sound/player`): the body hits a hurting landing makes, and the
/// death cries.
const CS16_PLAYER_WAVES: [&str; 3] = ["bhit_flesh", "die", "death6"];

/// A hurting CS 1.6 landing, as CS 1.6 sounds it: its movement code (`PM_CheckFalling`) plays
/// only the landing step — no cry — and the fall damage (`TakeDamage` → `Pain`, hit group
/// generic, never armoured: armour does not take fall damage) one of `bhit_flesh-1-3` at random.
pub const CS_FALL_BODY_HIT: &str = "cs/player/body_hit";
/// A deadly landing's splat (`PostThink`, `common/bodysplat.wav`: Half-Life's, CS 1.6 ships no
/// copy of its own), heard with the death cry.
pub const CS_FALL_SPLAT: &str = "cs/player/bodysplat";
/// Every death's cry, under the install-free name the death script plays: CS 1.6's
/// `DeathSound` (one of `die1-3` and `death6`), else CS:S's `Player.Death`.
pub const CS_DEATH_CRY: &str = "cs_death";

/// Which random-variant alias a CS 1.6 player wave belongs to.
fn player_wave_group(stem: &str) -> Option<&'static str> {
    if stem.starts_with("bhit_flesh") {
        Some(CS_FALL_BODY_HIT)
    } else if stem.starts_with("die") || stem == "death6" {
        Some(CS_DEATH_CRY)
    } else {
        None
    }
}

/// CS:S's own fall cry's gain over the gun template it borrows: that template's 0.40-0.44 suits
/// gun waves recorded near full scale, and left the quieter cry under the landing thud; doubled
/// it sits at about the level other players' falls get from the world template (0.77-0.81).
const OWN_FALL_CRY_GAIN: f32 = 2.0;

/// Your own fall cry: louder than the gun template makes it, and at CS's fixed pitch.
fn own_fall_cry(row: &mut crate::CapturedAlias) {
    steady_pitch(row);
    row.vol_min *= OWN_FALL_CRY_GAIN;
    row.vol_max *= OWN_FALL_CRY_GAIN;
}

/// How far a bomb beep carries (units). CS 1.6 (`C4Think`) beeps quietly at first and louder
/// as the timer runs out (attenuation 1.5, 1.0, 0.8, 0.5, 0.2: a GoldSrc sound fades out at
/// 1000 / attenuation units). CS:S (`C_PlantedC4::ClientThink`) gives each beep the attenuation
/// `0.3 + 0.6 × time left`, from 0.9 to 0.3; its five steps take the middle of each fifth of
/// the timer.
fn beep_reach(event: &str, css: bool) -> f32 {
    match (event, css) {
        ("beep1", true) => 1000.0 / 0.84,
        ("beep2", true) => 1000.0 / 0.72,
        ("beep3", true) => 1000.0 / 0.6,
        ("beep4", true) => 1000.0 / 0.48,
        (_, true) => 1000.0 / 0.36,
        ("beep1", _) => 667.0,
        ("beep2", _) => 1000.0,
        ("beep3", _) => 1250.0,
        ("beep4", _) => 2000.0,
        _ => 5000.0,
    }
}

/// Register one bomb sound (`event` of [`CS_C4_PREFIX`]) with its waves, mixed like MW2's
/// bomb sound for the same moment; the beeps are positional, from the bomb, and fade out at
/// CS's distance.
fn add_c4_sound(
    catalog: &mut SoundCatalog,
    event: &str,
    waves: &[(u32, i32, Vec<u8>)],
    css: bool,
) -> bool {
    let preferred: &[&str] = match event {
        "plant" => &["mp_bomb_plant"],
        "disarm" | "disarmed" => &["mp_bomb_defuse"],
        "explode" => &["exp_suitcase_bomb_main"],
        // A world sound (MW2's bomb timer is a flat UI sound heard everywhere).
        _ => &[],
    };
    let Some(template) = preferred
        .iter()
        .chain(WORLD_TEMPLATES)
        .find(|name| catalog.has_alias(crate::AssetNamespace::Iw4, name))
    else {
        return false;
    };
    let name = format!("{CS_C4_PREFIX}{event}");
    let clips = waves
        .iter()
        .enumerate()
        .map(|(i, (rate, channels, pcm))| LooseClip {
            name: format!("{name}#{i}"),
            rate: *rate,
            channels: *channels,
            pcm16: pcm.clone().into(),
        })
        .collect();
    let beep = event.starts_with("beep").then(|| beep_reach(event, css));
    catalog.add_loose_alias_tuned(&name, template, clips, |row| {
        steady_pitch(row);
        if let Some(reach) = beep {
            row.dist_min = 60.0;
            row.dist_max = reach;
        }
    })
}

/// The CS 1.6 radio wave behind each announcement.
const CS16_EVENTS: [(&str, &str); 5] = [
    ("ctwin", "ctwin"),
    ("terwin", "terwin"),
    ("rounddraw", "rounddraw"),
    ("bombpl", "bombplanted"),
    ("bombdef", "bombdefused"),
];

/// Decode a RIFF/WAVE PCM file (8-bit unsigned or 16-bit signed) to 16-bit little-endian.
#[must_use]
pub fn decode_wav(bytes: &[u8]) -> Option<(u32, i32, Vec<u8>)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut at = 12;
    let mut format: Option<(u16, u16, u32, u16)> = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let body = bytes.get(at + 8..(at + 8 + len).min(bytes.len()))?;
        match id {
            b"fmt " if body.len() >= 16 => {
                let tag = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                let bits = u16::from_le_bytes([body[14], body[15]]);
                format = Some((tag, channels, rate, bits));
            }
            b"data" => {
                let (tag, channels, rate, bits) = format?;
                if tag != 1 || channels == 0 || rate == 0 {
                    return None;
                }
                let pcm = match bits {
                    8 => body
                        .iter()
                        .flat_map(|&s| ((i16::from(s) - 128) << 8).to_le_bytes())
                        .collect(),
                    16 => body[..body.len() / 2 * 2].to_vec(),
                    _ => return None,
                };
                return Some((rate, i32::from(channels), pcm));
            }
            _ => {}
        }
        at += 8 + len + (len & 1);
    }
    None
}

/// MW2 aliases to model CS sounds on: the AK-47's gunshot as heard by its shooter and by others.
const PLAYER_TEMPLATES: &[&str] = &["weap_ak47_fire_plr"];
const WORLD_TEMPLATES: &[&str] = &["weap_ak47_fire_npc"];

/// MW2's own death cries, whose mixing (a voice's limits, channel and falloff) the CS death cry
/// takes: modelled on a gunshot, it was refused for the gun's concurrency limit.
const DEATH_TEMPLATES: &[&str] = &["generic_death_american_1", "generic_death_russian_1"];

/// The template the CS death cry is modelled on: MW2's death cry, else `fallback`.
fn death_template(catalog: &SoundCatalog, fallback: &str) -> String {
    DEATH_TEMPLATES
        .iter()
        .find(|name| catalog.has_alias(crate::AssetNamespace::Iw4, name))
        .map_or_else(|| fallback.to_owned(), |name| (*name).to_owned())
}

fn template(catalog: &SoundCatalog, preferred: &[&str], suffix: &str) -> Option<String> {
    preferred
        .iter()
        .find(|name| catalog.has_alias(crate::AssetNamespace::Iw4, name))
        .map(|name| (*name).to_owned())
        .or_else(|| catalog.first_alias_ending_with(crate::AssetNamespace::Iw4, suffix))
}

/// Add every `.wav` in the GoldSrc install's `sound/weapons` folder to `catalog`: Condition
/// Zero's wave where it has one, else CS 1.6's. Returns how many sounds were added; 0 when there
/// is no install.
pub fn append_cs_weapon_sounds(catalog: &mut SoundCatalog) -> usize {
    let Some(goldsrc) = asset_transport::find_goldsrc() else {
        return 0;
    };
    let (Some(player), Some(world)) = (
        template(catalog, PLAYER_TEMPLATES, "fire_plr"),
        template(catalog, WORLD_TEMPLATES, "fire_npc"),
    ) else {
        diag::warn!(Audio, "cs sounds: no MW2 gunshot alias to model them on");
        return 0;
    };
    // Every gun and grenade wave, the fall pain sounds of the player folder, the radio voice,
    // and Half-Life's own explosions that CS 1.6's HE grenade uses (`valve/sound/weapons`); a
    // wave the better game has hides the others' of the same name.
    let half_life = goldsrc
        .0
        .last()
        .and_then(|dir| dir.parent())
        .map(|dir| dir.join("valve"));
    let mut folders: Vec<(_, _, &[&str])> = Vec::new();
    for game in &goldsrc.0 {
        let sound = game.join("sound");
        folders.push((sound.join("weapons"), CS_SOUND_PREFIX, &[""]));
        folders.push((
            sound.join("player"),
            CS_PLAYER_SOUND_PREFIX,
            &CS16_PLAYER_WAVES,
        ));
        folders.push((sound.join("radio"), CS_RADIO_SOUND_PREFIX, &[""]));
    }
    if let Some(valve) = &half_life {
        folders.push((
            valve.join("sound").join("weapons"),
            CS_SOUND_PREFIX,
            &["explode"],
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut added = 0;
    // The player sounds a landing picks from at random, by group.
    let mut player_groups: std::collections::BTreeMap<&'static str, Vec<(String, LooseClip)>> =
        std::collections::BTreeMap::new();
    for (folder, prefix, wanted) in folders {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
            {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let stem = stem.to_ascii_lowercase();
            if !wanted.iter().any(|wanted| stem.starts_with(wanted))
                || !seen.insert(format!("{prefix}{stem}"))
            {
                continue;
            }
            let Some((rate, channels, pcm)) =
                std::fs::read(&path).ok().and_then(|b| decode_wav(&b))
            else {
                diag::warn!(Audio, "cs sounds: {} is not PCM wave", path.display());
                continue;
            };
            let alias = format!("{prefix}{stem}");
            let clip = |name: String| LooseClip {
                name,
                rate,
                channels,
                pcm16: pcm.clone().into(),
            };
            // The radio voice and the player's own sounds keep their pitch; guns vary theirs as
            // MW2's do.
            let voice = prefix == CS_RADIO_SOUND_PREFIX || prefix == CS_PLAYER_SOUND_PREFIX;
            let tune = |row: &mut crate::CapturedAlias| {
                if voice {
                    steady_pitch(row);
                }
            };
            if prefix == CS_PLAYER_SOUND_PREFIX
                && let Some(group) = player_wave_group(&stem)
            {
                player_groups
                    .entry(group)
                    .or_default()
                    .push((stem.clone(), clip(String::new())));
            }
            let both = catalog.add_loose_alias_tuned(
                &alias,
                &world,
                vec![clip(alias.clone())],
                tune,
            ) && catalog.add_loose_alias_tuned(
                &format!("{alias}{CS_SOUND_PLAYER_SUFFIX}"),
                &player,
                vec![clip(format!("{alias}{CS_SOUND_PLAYER_SUFFIX}"))],
                tune,
            );
            if both {
                added += 1;
            }
            if prefix == CS_RADIO_SOUND_PREFIX
                && let Some((_, event)) = CS16_EVENTS.iter().find(|(wave, _)| *wave == stem)
            {
                let event = format!("{CS_EVENT_PREFIX}{event}");
                catalog.add_loose_alias_tuned(&event, &player, vec![clip(event.clone())], steady_pitch);
            }
        }
    }
    // A deadly landing's splat: the GoldSrc game's own, else Half-Life's.
    if let Some((stem, splat)) = goldsrc
        .0
        .iter()
        .chain(&half_life)
        .map(|game| game.join("sound").join("common").join("bodysplat.wav"))
        .find_map(|path| std::fs::read(path).ok())
        .and_then(|bytes| decode_wav(&bytes))
        .map(|(rate, channels, pcm16)| {
            (
                "bodysplat".to_owned(),
                LooseClip {
                    name: String::new(),
                    rate,
                    channels,
                    pcm16: pcm16.into(),
                },
            )
        })
    {
        player_groups.entry(CS_FALL_SPLAT).or_default().push((stem, splat));
    }
    // The player's sounds by alias, each wave of one a random variant of it, for others
    // (positional) and for yourself (`/plr`).
    for (alias, mut waves) in player_groups {
        waves.sort_by(|a, b| a.0.cmp(&b.0));
        diag::info!(
            Audio,
            "cs sounds: {alias} = {}",
            waves
                .iter()
                .map(|(stem, _)| format!("{stem}.wav"))
                .collect::<Vec<_>>()
                .join(" | ")
        );
        let own = format!("{alias}{CS_SOUND_PLAYER_SUFFIX}");
        let named = |name: &str| -> Vec<LooseClip> {
            waves
                .iter()
                .enumerate()
                .map(|(i, (_, clip))| LooseClip {
                    name: format!("{name}#{i}"),
                    ..clip.clone()
                })
                .collect()
        };
        let others = if alias == CS_DEATH_CRY {
            death_template(catalog, &world)
        } else {
            world.clone()
        };
        catalog.add_loose_alias_tuned(alias, &others, named(alias), steady_pitch);
        catalog.add_loose_alias_tuned(&own, &player, named(&own), steady_pitch);
    }
    // The bomb's sounds.
    for (event, wave) in CS16_C4 {
        let decoded = goldsrc
            .read(&format!("sound/weapons/{wave}.wav"))
            .and_then(|b| decode_wav(&b));
        if let Some(decoded) = decoded {
            add_c4_sound(catalog, event, &[decoded], false);
        }
    }
    // Ladder steps: the GoldSrc game's own, else Half-Life's (the same waves).
    let ladder: Vec<_> = (1..=4)
        .filter_map(|i| {
            goldsrc
                .0
                .iter()
                .chain(&half_life)
                .map(|game| {
                    game.join("sound")
                        .join("player")
                        .join(format!("pl_ladder{i}.wav"))
                })
                .find_map(|path| std::fs::read(path).ok())
                .and_then(|bytes| decode_wav(&bytes))
        })
        .collect();
    add_ladder_steps(catalog, &ladder);
    diag::info!(
        Audio,
        "cs sounds: {added} weapon sounds from {:?} (modelled on `{player}` vol {} / `{world}` vol {})",
        goldsrc.0,
        alias_volume_range(catalog, &player),
        alias_volume_range(catalog, &world)
    );
    added
}

/// An alias's volume range for the log, `?` when the bank has no such alias.
fn alias_volume_range(catalog: &SoundCatalog, alias: &str) -> String {
    catalog
        .sound_in(crate::AssetNamespace::Iw4, alias)
        .and_then(|sound| sound.aliases.first())
        .map_or_else(
            || "?".to_owned(),
            |row| format!("{:.2}-{:.2}", row.vol_min, row.vol_max),
        )
}

/// Add Counter-Strike: Source's weapon sound script entries (`weapon_*`, `default.*`) from its
/// pack as aliases `css/<entry>` and `css/<entry>/plr`, each with the entry's random waves as
/// variants. Returns how many entries were added; 0 when CS:S is not installed.
/// The CS:S sound script entries (lowercase name prefixes) loaded into the bank: guns and their
/// zoom/dry-fire, radio calls, grenades, the C4's keypad, and the player's fall and armour hits.
const CSS_SOUND_GROUPS: [&str; 13] = [
    "weapon_",
    "c4.",
    "default.",
    "radio.",
    "flashbang.",
    "hegrenade.",
    "smokegrenade.",
    "basegrenade.",
    "basesmokeeffect.",
    "event.",
    "player.fall",
    "player.damage",
    "player.death",
];

/// Add CS:GO's sound script entries for its guns, knife, grenades and bomb ([`CSGO_SOUND_GROUPS`])
/// to `catalog` as `csgo/<entry>` (positional) and `csgo/<entry>/plr` (your own), mixed like
/// CS:S's; a gun's far-away shot is left out. Returns how many entries were added; 0 without
/// CS:GO.
pub fn append_csgo_weapon_sounds(catalog: &mut SoundCatalog) -> usize {
    let Some(pak) = asset_transport::find_csgo_pak() else {
        return 0;
    };
    let vpk = match mdl_source::Vpk::open(&pak) {
        Ok(vpk) => vpk,
        Err(error) => {
            diag::warn!(Audio, "csgo sounds: {error}");
            return 0;
        }
    };
    let (Some(player), Some(world)) = (
        template(catalog, PLAYER_TEMPLATES, "fire_plr"),
        template(catalog, WORLD_TEMPLATES, "fire_npc"),
    ) else {
        return 0;
    };
    let scripts = mdl_source::SoundScripts::load(&vpk);
    let mut names: Vec<String> = scripts
        .names()
        .filter(|n| CSGO_SOUND_GROUPS.iter().any(|group| n.starts_with(group)))
        .filter(|n| !n.ends_with(".singledistant") && !n.ends_with(".singlefocused"))
        .map(str::to_owned)
        .collect();
    names.sort();
    let (mut added, mut bytes, mut skipped) = (0usize, 0usize, 0usize);
    for name in names {
        let mut clips = Vec::new();
        for (i, wave) in scripts.waves(&name).iter().enumerate() {
            match vpk
                .read(&format!("sound/{wave}"))
                .as_deref()
                .and_then(decode_wav)
            {
                Some((rate, channels, pcm16)) => {
                    bytes += pcm16.len();
                    clips.push(LooseClip {
                        name: format!("{CSGO_SOUND_PREFIX}{name}#{i}"),
                        rate,
                        channels,
                        pcm16: pcm16.into(),
                    });
                }
                None => skipped += 1,
            }
        }
        if clips.is_empty() {
            continue;
        }
        let alias = format!("{CSGO_SOUND_PREFIX}{name}");
        let player_clips = clips
            .iter()
            .map(|c| LooseClip {
                name: format!("{}{CS_SOUND_PLAYER_SUFFIX}", c.name),
                ..c.clone()
            })
            .collect();
        // The bomb and the ping keep their pitch; guns vary theirs as MW2's do. The ping plays
        // at a fifth of a shot's volume (`UI.PlayerPing`'s 0.2).
        let tune = |row: &mut crate::CapturedAlias| {
            if name.starts_with("weapon_c4.") || name == "ui.playerping" {
                steady_pitch(row);
            }
            if name == "ui.playerping" {
                row.vol_min *= 0.2;
                row.vol_max *= 0.2;
            }
        };
        if catalog.add_loose_alias_tuned(&alias, &world, clips, tune)
            && catalog.add_loose_alias_tuned(
                &format!("{alias}{CS_SOUND_PLAYER_SUFFIX}"),
                &player,
                player_clips,
                tune,
            )
        {
            added += 1;
        }
    }
    diag::info!(
        Audio,
        "csgo sounds: {added} entries ({:.1} MB PCM, {skipped} waves not PCM) from {}",
        bytes as f64 / 1_048_576.0,
        pak.display()
    );
    added
}

/// Add CS:GO's grenade radio calls: each of [`weapon_iw4::cs::CSGO_RADIO_VOICES`] making the call
/// of each of [`weapon_iw4::cs::CSGO_RADIO_GRENADES`], as a 2D alias
/// (`csgo/radio/<voice>/<call>`) whose variants are the lines its talker response picks
/// from (`scripts/talker/<voice>.txt`: `scene "scenes/<voice>/<line>.vcd"`, spoken by
/// `sound/player/vo/<voice>/<line>.wav`). Returns how many calls were added; 0 without CS:GO.
pub fn append_csgo_radio_calls(catalog: &mut SoundCatalog) -> usize {
    use weapon_iw4::cs::{
        CSGO_RADIO_ALIAS_PREFIX, CSGO_RADIO_GRENADES, CSGO_RADIO_VOICES, csgo_grenade_concept,
        csgo_grenade_radio,
    };
    let Some(pak) = asset_transport::find_csgo_pak() else {
        return 0;
    };
    let vpk = match mdl_source::Vpk::open(&pak) {
        Ok(vpk) => vpk,
        Err(error) => {
            diag::warn!(Audio, "csgo radio: {error}");
            return 0;
        }
    };
    let Some(player) = template(catalog, PLAYER_TEMPLATES, "fire_plr") else {
        return 0;
    };
    let (mut added, mut bytes, mut skipped) = (0usize, 0usize, 0usize);
    for voice in CSGO_RADIO_VOICES {
        let Some(script) = vpk.read(&format!("scripts/talker/{voice}.txt")) else {
            continue;
        };
        let script = String::from_utf8_lossy(&script);
        for grenade in CSGO_RADIO_GRENADES {
            let alias = format!("{CSGO_RADIO_ALIAS_PREFIX}{voice}/{}", csgo_grenade_radio(grenade));
            let response = format!("{}{voice}", csgo_grenade_concept(grenade));
            let mut clips = Vec::new();
            for (i, scene) in talker_scenes(&script, &response).iter().enumerate() {
                let Some(line) = scene.strip_prefix("scenes/") else {
                    continue;
                };
                let wave = format!("sound/player/vo/{}.wav", line.trim_end_matches(".vcd"));
                match vpk.read(&wave).as_deref().and_then(decode_wav) {
                    Some((rate, channels, pcm16)) => {
                        bytes += pcm16.len();
                        clips.push(LooseClip {
                            name: format!("{alias}#{i}"),
                            rate,
                            channels,
                            pcm16: pcm16.into(),
                        });
                    }
                    None => skipped += 1,
                }
            }
            if !clips.is_empty() && catalog.add_loose_alias_tuned(&alias, &player, clips, steady_pitch) {
                added += 1;
            }
        }
    }
    diag::info!(
        Audio,
        "csgo radio: {added} grenade calls ({:.1} MB PCM, {skipped} lines missing)",
        bytes as f64 / 1_048_576.0
    );
    added
}

/// The scenes talker response `name` picks from: the `scene "..."` lines of its block.
fn talker_scenes(script: &str, name: &str) -> Vec<String> {
    let mut lines = script.lines();
    while let Some(line) = lines.next() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix("Response")
            .or_else(|| line.strip_prefix("response"))
        else {
            continue;
        };
        if !rest.trim().eq_ignore_ascii_case(name) {
            continue;
        }
        let mut scenes = Vec::new();
        for line in lines.by_ref() {
            let line = line.trim();
            if line.starts_with('}') {
                break;
            }
            if let Some(scene) = line.strip_prefix("scene").and_then(|rest| rest.split('"').nth(1)) {
                scenes.push(scene.to_ascii_lowercase());
            }
        }
        return scenes;
    }
    Vec::new()
}

pub fn append_css_weapon_sounds(catalog: &mut SoundCatalog) -> usize {
    let Some(pak) = asset_transport::find_css_pak() else {
        return 0;
    };
    let vpk = match mdl_source::Vpk::open(&pak) {
        Ok(vpk) => vpk,
        Err(error) => {
            diag::warn!(Audio, "css sounds: {error}");
            return 0;
        }
    };
    let (Some(player), Some(world)) = (
        template(catalog, PLAYER_TEMPLATES, "fire_plr"),
        template(catalog, WORLD_TEMPLATES, "fire_npc"),
    ) else {
        return 0;
    };
    let loose = pak.parent().map(|dir| dir.join("sound"));
    let scripts = mdl_source::SoundScripts::load(&vpk);
    let mut names: Vec<String> = scripts
        .names()
        .filter(|n| CSS_SOUND_GROUPS.iter().any(|group| n.starts_with(group)))
        .map(str::to_owned)
        .collect();
    names.sort();
    let (mut added, mut bytes, mut skipped) = (0usize, 0usize, 0usize);
    for name in names {
        let mut clips = Vec::new();
        for (i, wave) in scripts.waves(&name).iter().enumerate() {
            let data = vpk.read(&format!("sound/{wave}")).or_else(|| {
                loose
                    .as_ref()
                    .and_then(|dir| std::fs::read(dir.join(wave)).ok())
            });
            match data.as_deref().and_then(decode_wav) {
                Some((rate, channels, pcm16)) => {
                    bytes += pcm16.len();
                    clips.push(LooseClip {
                        name: format!("{CSS_SOUND_PREFIX}{name}#{i}"),
                        rate,
                        channels,
                        pcm16: pcm16.into(),
                    });
                }
                None => skipped += 1,
            }
        }
        if clips.is_empty() {
            continue;
        }
        let alias = format!("{CSS_SOUND_PREFIX}{name}");
        // Round and bomb announcements also go under their install-free names.
        let event = name.strip_prefix("event.").map(|event| {
            let event = format!("{CS_EVENT_PREFIX}{event}");
            let clips: Vec<LooseClip> = clips
                .iter()
                .enumerate()
                .map(|(i, c)| LooseClip {
                    name: format!("{event}#{i}"),
                    ..c.clone()
                })
                .collect();
            (event, clips)
        });
        // Every death's cry also goes under the name the death script plays.
        let death_cry = (name == "player.death").then(|| {
            clips
                .iter()
                .enumerate()
                .map(|(i, c)| LooseClip {
                    name: format!("{CS_DEATH_CRY}#{i}"),
                    ..c.clone()
                })
                .collect::<Vec<_>>()
        });
        let player_clips = clips
            .iter()
            .map(|c| LooseClip {
                name: format!("{}{CS_SOUND_PLAYER_SUFFIX}", c.name),
                ..c.clone()
            })
            .collect();
        // Voices and the bomb keep their pitch; guns vary theirs as MW2's do.
        let voice = ["radio.", "event.", "c4.", "bot."]
            .iter()
            .any(|group| name.starts_with(group));
        let fall_cry = name.starts_with("player.fall");
        let tune = |row: &mut crate::CapturedAlias| {
            if voice || fall_cry {
                steady_pitch(row);
            }
        };
        let tune_own = |row: &mut crate::CapturedAlias| {
            if fall_cry {
                own_fall_cry(row);
            } else {
                tune(row);
            }
        };
        if catalog.add_loose_alias_tuned(&alias, &world, clips, tune)
            && catalog.add_loose_alias_tuned(
                &format!("{alias}{CS_SOUND_PLAYER_SUFFIX}"),
                &player,
                player_clips,
                tune_own,
            )
        {
            added += 1;
        }
        if let Some((event, clips)) = event {
            catalog.add_loose_alias_tuned(&event, &player, clips, steady_pitch);
        }
        if let Some(clips) = death_cry {
            let template = death_template(catalog, &world);
            catalog.add_loose_alias_tuned(CS_DEATH_CRY, &template, clips, steady_pitch);
        }
    }
    // The bomb's sounds.
    for (event, entry) in CSS_C4 {
        let waves: Vec<_> = scripts
            .waves(entry)
            .iter()
            .filter_map(|wave| vpk.read(&format!("sound/{wave}")))
            .filter_map(|bytes| decode_wav(&bytes))
            .collect();
        add_c4_sound(catalog, event, &waves, true);
    }
    // Ladder steps: Half-Life 2's, in CS:S's own pack or the `hl2` sound pack beside it.
    let hl2 = pak
        .parent()
        .and_then(Path::parent)
        .map(|game| game.join("hl2").join("hl2_sound_misc_dir.vpk"))
        .and_then(|dir| mdl_source::Vpk::open(&dir).ok());
    let ladder: Vec<_> = (1..=4)
        .filter_map(|i| {
            let wave = format!("sound/player/footsteps/ladder{i}.wav");
            vpk.read(&wave)
                .or_else(|| hl2.as_ref().and_then(|hl2| hl2.read(&wave)))
                .and_then(|bytes| decode_wav(&bytes))
        })
        .collect();
    if !add_ladder_steps(catalog, &ladder) {
        diag::info!(Audio, "css sounds: no ladder steps found, CS 1.6's or MW2's are used");
    }
    diag::info!(
        Audio,
        "css sounds: {added} entries ({:.1} MB PCM, {skipped} waves not PCM) from {}; own fall cry vol {}",
        bytes as f64 / 1_048_576.0,
        pak.display(),
        alias_volume_range(
            catalog,
            &format!("{CSS_SOUND_PREFIX}player.falldamage{CS_SOUND_PLAYER_SUFFIX}")
        )
    );
    added
}

#[cfg(test)]
mod tests {
    use super::decode_wav;

    fn wav(bits: u16, data: &[u8]) -> Vec<u8> {
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&22050u32.to_le_bytes());
        out.extend_from_slice(&(22050u32 * u32::from(bits / 8)).to_le_bytes());
        out.extend_from_slice(&(bits / 8).to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn eight_bit_unsigned_becomes_sixteen_bit_signed() {
        let (rate, channels, pcm) = decode_wav(&wav(8, &[128, 255, 0])).expect("decodes");
        assert_eq!((rate, channels), (22050, 1));
        let samples: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(samples, [0, 127 << 8, -128 << 8]);
    }

    #[test]
    fn sixteen_bit_passes_through() {
        let data = [0x34, 0x12, 0xff, 0xff];
        let (_, _, pcm) = decode_wav(&wav(16, &data)).expect("decodes");
        assert_eq!(pcm, data);
    }
}
