//! Counter-Strike 1.6 weapon sounds, read at runtime from the local CS install's
//! `sound/weapons/*.wav` and added to the match sound bank as aliases `cs/weapons/<name>`
//! (positional, for other players) and `cs/weapons/<name>/plr` (the local player's own gun).
//! Each borrows its mixing settings (volume, distance falloff, channel) from MW2's own gunshot
//! aliases, so CS sounds sit in the mix like MW2's did. Nothing is copied out of the install.

use std::path::Path;

use crate::{LooseClip, SoundCatalog};

/// Alias prefix of Counter-Strike: Source sound script entries (`css/weapon_ak47.single`).
pub const CSS_SOUND_PREFIX: &str = "css/";

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
                pcm16: pcm.clone(),
            })
            .collect();
        added &= catalog.add_loose_alias(name, &template, clips);
    }
    added
}

/// The bomb's sounds under install-free names (`cs_c4_plant`, `cs_c4_disarm`, `cs_c4_disarmed`,
/// `cs_c4_explode`, `cs_c4_click`, `cs_c4_beep1`; CS 1.6 also `cs_c4_beep2..5`), each mixed like
/// the MW2 bomb sound it stands in for. CS:S has one beep that speeds up; CS 1.6 steps through
/// five.
pub const CS_C4_PREFIX: &str = "cs_c4_";

/// The CS sounds the bomb mode's script plays by name. The server checks every script sound
/// against the zones' aliases; these come from the local CS install on each client instead, so
/// they are added to that list.
pub const CS_SCRIPT_SOUNDS: [&str; 15] = [
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

/// CS:S sound script entries behind each bomb sound.
const CSS_C4: [(&str, &str); 6] = [
    ("plant", "c4.plant"),
    ("disarm", "c4.disarmstart"),
    ("disarmed", "c4.disarmfinish"),
    ("explode", "c4.explode"),
    ("click", "c4.click"),
    ("beep1", "c4.plantsound"),
];

/// Register one bomb sound (`event` of [`CS_C4_PREFIX`]) with its waves, mixed like MW2's
/// bomb sound for the same moment.
fn add_c4_sound(catalog: &mut SoundCatalog, event: &str, waves: &[(u32, i32, Vec<u8>)]) -> bool {
    let preferred: &[&str] = match event {
        "plant" => &["mp_bomb_plant"],
        "disarm" | "disarmed" => &["mp_bomb_defuse"],
        "explode" => &["exp_suitcase_bomb_main"],
        _ => &["ui_mp_suitcasebomb_timer"],
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
            pcm16: pcm.clone(),
        })
        .collect();
    catalog.add_loose_alias(&name, template, clips)
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

fn template(catalog: &SoundCatalog, preferred: &[&str], suffix: &str) -> Option<String> {
    preferred
        .iter()
        .find(|name| catalog.has_alias(crate::AssetNamespace::Iw4, name))
        .map(|name| (*name).to_owned())
        .or_else(|| catalog.first_alias_ending_with(crate::AssetNamespace::Iw4, suffix))
}

/// Add every `.wav` in the CS install's `sound/weapons` folder to `catalog`. Returns how many
/// sounds were added; 0 when there is no install.
pub fn append_cs_weapon_sounds(catalog: &mut SoundCatalog) -> usize {
    let Some(cstrike) = asset_transport::find_cstrike() else {
        return 0;
    };
    let (Some(player), Some(world)) = (
        template(catalog, PLAYER_TEMPLATES, "fire_plr"),
        template(catalog, WORLD_TEMPLATES, "fire_npc"),
    ) else {
        diag::warn!(Audio, "cs sounds: no MW2 gunshot alias to model them on");
        return 0;
    };
    let dir = cstrike.join("sound").join("weapons");
    // Every gun and grenade wave, the fall pain sounds of the player folder, the radio voice,
    // and Half-Life's own explosions that CS 1.6's HE grenade uses (`valve/sound/weapons`).
    let half_life = cstrike.parent().map(|dir| dir.join("valve")).unwrap_or_default();
    let folders = [
        (dir.clone(), CS_SOUND_PREFIX, ""),
        (
            cstrike.join("sound").join("player"),
            CS_PLAYER_SOUND_PREFIX,
            "pl_fallpain",
        ),
        (cstrike.join("sound").join("radio"), CS_RADIO_SOUND_PREFIX, ""),
        (half_life.join("sound").join("weapons"), CS_SOUND_PREFIX, "explode"),
    ];
    let mut added = 0;
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
            if !stem.starts_with(wanted) {
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
                pcm16: pcm.clone(),
            };
            let both = catalog.add_loose_alias(&alias, &world, vec![clip(alias.clone())])
                && catalog.add_loose_alias(
                    &format!("{alias}{CS_SOUND_PLAYER_SUFFIX}"),
                    &player,
                    vec![clip(format!("{alias}{CS_SOUND_PLAYER_SUFFIX}"))],
                );
            if both {
                added += 1;
            }
            if prefix == CS_RADIO_SOUND_PREFIX
                && let Some((_, event)) = CS16_EVENTS.iter().find(|(wave, _)| *wave == stem)
            {
                let event = format!("{CS_EVENT_PREFIX}{event}");
                catalog.add_loose_alias(&event, &player, vec![clip(event.clone())]);
            }
        }
    }
    // The bomb's sounds.
    for (event, wave) in CS16_C4 {
        let path = dir.join(format!("{wave}.wav"));
        if let Some(decoded) = std::fs::read(&path).ok().and_then(|b| decode_wav(&b)) {
            add_c4_sound(catalog, event, &[decoded]);
        }
    }
    // Ladder steps: CS 1.6's own, else Half-Life's (the same waves).
    let ladder: Vec<_> = (1..=4)
        .filter_map(|i| {
            [&cstrike, &half_life]
                .into_iter()
                .map(|game| game.join("sound").join("player").join(format!("pl_ladder{i}.wav")))
                .find_map(|path| std::fs::read(path).ok())
                .and_then(|bytes| decode_wav(&bytes))
        })
        .collect();
    add_ladder_steps(catalog, &ladder);
    diag::info!(
        Audio,
        "cs sounds: {added} weapon sounds from {} (modelled on `{player}` / `{world}`)",
        dir.display()
    );
    added
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
                        pcm16,
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
        let player_clips = clips
            .iter()
            .map(|c| LooseClip {
                name: format!("{}{CS_SOUND_PLAYER_SUFFIX}", c.name),
                ..c.clone()
            })
            .collect();
        if catalog.add_loose_alias(&alias, &world, clips)
            && catalog.add_loose_alias(
                &format!("{alias}{CS_SOUND_PLAYER_SUFFIX}"),
                &player,
                player_clips,
            )
        {
            added += 1;
        }
        if let Some((event, clips)) = event {
            catalog.add_loose_alias(&event, &player, clips);
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
        add_c4_sound(catalog, event, &waves);
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
        "css sounds: {added} entries ({:.1} MB PCM, {skipped} waves not PCM) from {}",
        bytes as f64 / 1_048_576.0,
        pak.display()
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
