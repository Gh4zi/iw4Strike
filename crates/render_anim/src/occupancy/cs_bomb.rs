//! The bomb mode's planted C4 as the local install hears it: CS:GO's and CS:S's one beep coming
//! faster as the timer runs down (CS:GO's then giving way to its last-second warning), or CS 1.6's
//! beeps every 1.4 s stepping through its five sounds.
//!
//! The script publishes the bomb as the server info `cs_bomb`: "planted <time ms> <timer s> <x>
//! <y> <z>" while it ticks, else "none", "defused" or "exploded".

use std::sync::OnceLock;

use bevy::prelude::*;

/// CS:GO's bomb sounds its last second with `C4.ExplodeTriggerTrip` (a rising whine) and
/// `C4.ExplodeWarning` instead of beeps.
const CSGO_WARNING_MS: i64 = 1000;
const CSGO_WARNINGS: [&str; 2] = ["cs_c4_trip", "cs_c4_warning"];

/// Whether CS:GO is on: its bomb sounds are then the ones in the bank
/// (`asset_audio::append_csgo_round_sounds` fills the bomb's names last).
fn csgo_installed() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| asset_transport::find_csgo_pak().is_some())
}

/// The planted bomb: when (server ms), for how long, and where.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlantedBomb {
    pub planted_ms: i64,
    pub timer_ms: i64,
    pub origin: [f32; 3],
}

/// The bomb's state word from the server info (`none`, `planted`, `defused`, `exploded`).
#[must_use]
pub fn bomb_state(snap: &sim::Snapshot) -> Option<&str> {
    snap.meta
        .objectives
        .server_info
        .iter()
        .find(|(name, _)| name == "cs_bomb")
        .and_then(|(_, value)| value.split_whitespace().next())
}

/// The ticking bomb, if one is planted.
#[must_use]
pub fn planted_bomb(snap: &sim::Snapshot) -> Option<PlantedBomb> {
    let value = &snap
        .meta
        .objectives
        .server_info
        .iter()
        .find(|(name, _)| name == "cs_bomb")?
        .1;
    parse_planted(value)
}

fn parse_planted(value: &str) -> Option<PlantedBomb> {
    let mut words = value.split_whitespace();
    if words.next()? != "planted" {
        return None;
    }
    let mut number = || words.next()?.parse::<f64>().ok();
    let planted_ms = number()? as i64;
    let timer_ms = (number()? * 1000.0) as i64;
    let origin = [number()? as f32, number()? as f32, number()? as f32];
    Some(PlantedBomb {
        planted_ms,
        timer_ms,
        origin,
    })
}

/// Server time of a snapshot, the clock the script's `getTime()` runs on.
fn server_ms(snap: &sim::Snapshot) -> i64 {
    i64::from(snap.tick.0) * i64::from(sim::MATCH_TICK_MS)
}

/// The beeping of one planted bomb.
#[derive(Default)]
pub(crate) struct Beeps {
    bomb: Option<PlantedBomb>,
    next_beep: i64,
    /// CS 1.6: the next step to a faster beep sound, the time until the one after, and the
    /// sound (1-5) now playing.
    next_wave: i64,
    wave_ms: f64,
    wave: u32,
    /// The beep sound last started, cut off by the next one or when the bomb stops ticking.
    playing: Option<String>,
    /// CS:GO's last-second warning has started (cut off when the bomb is defused or blows).
    warned: bool,
}

/// The CS:S beep step (1-5) for the fraction of the timer `left`: the same sound, carrying
/// farther each fifth of the way down (`cs_c4_beep1..5`).
fn css_step(left: f32) -> u32 {
    5 - ((left.clamp(0.0, 1.0) * 5.0) as u32).min(4)
}

fn stop_beep(beeps: &mut Beeps, sounds: &mut MessageWriter<audio::AliasCommand>) {
    if let Some(alias) = beeps.playing.take() {
        sounds.write(audio::AliasCommand::Stop {
            namespace: asset_core::AssetNamespace::Iw4,
            alias,
            snd_ent: None,
        });
    }
}

/// Stop the beep and CS:GO's last-second warning: the bomb was defused or blew.
fn stop_all(beeps: &mut Beeps, sounds: &mut MessageWriter<audio::AliasCommand>) {
    stop_beep(beeps, sounds);
    if std::mem::take(&mut beeps.warned) {
        for alias in CSGO_WARNINGS {
            sounds.write(audio::AliasCommand::Stop {
                namespace: asset_core::AssetNamespace::Iw4,
                alias: alias.to_owned(),
                snd_ent: None,
            });
        }
    }
}

fn play_at(alias: String, origin: [f32; 3], sounds: &mut MessageWriter<audio::AliasCommand>) {
    sounds.write(audio::AliasCommand::Play(audio::PlayAlias {
        event: None,
        namespace: asset_core::AssetNamespace::Iw4,
        alias,
        fallback: None,
        origin_inches: Some(origin),
        snd_ent: None,
    }));
}

/// CS:S and CS:GO beep once, every `max(0.1 + 0.9 × left / timer, 0.15)` s, carrying farther as
/// the timer runs down; CS:GO's last second is its warning instead ([`CSGO_WARNING_MS`]). CS 1.6
/// (`C4Think`) beeps every 1.4 s from half a second in, its sound stepping `c4_beep1..5` after a
/// quarter of the timer, then 0.9 of the step before. All play the beep on the bomb's voice
/// channel: a beep cuts the one before (CS 1.6's sounds run 1.5 s, longer than the 1.4 s between
/// them), and defusing or the blast cuts the last.
pub(crate) fn beep_cs_bomb(
    presented: Res<net::PresentedSnapshot>,
    mut beeps: Local<Beeps>,
    mut sounds: MessageWriter<audio::AliasCommand>,
) {
    if !movement_iw4::rules::CS_RULES {
        return;
    }
    let Some(snap) = presented.snapshot() else {
        return;
    };
    let Some(bomb) = planted_bomb(snap) else {
        stop_all(&mut beeps, &mut sounds);
        beeps.bomb = None;
        return;
    };
    let now = server_ms(snap);
    if beeps.bomb != Some(bomb) {
        stop_all(&mut beeps, &mut sounds);
        *beeps = Beeps {
            bomb: Some(bomb),
            next_beep: bomb.planted_ms + 500,
            next_wave: bomb.planted_ms,
            wave_ms: bomb.timer_ms as f64 / 4.0,
            wave: 0,
            playing: None,
            warned: false,
        };
    }
    let blow = bomb.planted_ms + bomb.timer_ms;
    if now >= blow {
        return;
    }
    let csgo = csgo_installed();
    if csgo && blow - now <= CSGO_WARNING_MS {
        if !beeps.warned {
            beeps.warned = true;
            stop_beep(&mut beeps, &mut sounds);
            for alias in CSGO_WARNINGS {
                play_at(alias.to_owned(), bomb.origin, &mut sounds);
            }
        }
        return;
    }
    let css = csgo || super::cs_world_model::css_installed();
    if !css && now >= beeps.next_wave && beeps.wave < 5 {
        beeps.wave += 1;
        beeps.next_wave = now + beeps.wave_ms as i64;
        beeps.wave_ms *= 0.9;
    }
    if now < beeps.next_beep {
        return;
    }
    let (alias, next) = if css {
        let left = (blow - now) as f32 / bomb.timer_ms.max(1) as f32;
        (
            format!("cs_c4_beep{}", css_step(left)),
            (0.1 + 0.9 * left).max(0.15),
        )
    } else {
        (format!("cs_c4_beep{}", beeps.wave.max(1)), 1.4)
    };
    beeps.next_beep = now + (next * 1000.0) as i64;
    stop_beep(&mut beeps, &mut sounds);
    beeps.playing = Some(alias.clone());
    play_at(alias, bomb.origin, &mut sounds);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planted_bomb_reads_as_the_script_writes_it() {
        assert_eq!(
            parse_planted("planted 51250 40 120.5 -30 -239.875"),
            Some(PlantedBomb {
                planted_ms: 51250,
                timer_ms: 40_000,
                origin: [120.5, -30.0, -239.875],
            })
        );
        assert_eq!(parse_planted("defused"), None);
        assert_eq!(parse_planted("none"), None);
    }

    #[test]
    fn css_beep_carries_farther_each_fifth_of_the_timer() {
        assert_eq!(css_step(1.0), 1);
        assert_eq!(css_step(0.85), 1);
        assert_eq!(css_step(0.79), 2);
        assert_eq!(css_step(0.5), 3);
        assert_eq!(css_step(0.3), 4);
        assert_eq!(css_step(0.1), 5);
        assert_eq!(css_step(0.0), 5);
    }
}
