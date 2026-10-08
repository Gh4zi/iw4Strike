//! The bomb mode's planted C4 as the local install hears it: CS:S's one beep coming faster as
//! the timer runs down, or CS 1.6's beeps every 1.4 s stepping through its five sounds.
//!
//! The script publishes the bomb as the server info `cs_bomb`: "planted <time ms> <timer s> <x>
//! <y> <z>" while it ticks, else "none", "defused" or "exploded".

use bevy::prelude::*;

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
}

/// CS:S beeps once, every `max(0.1 + 0.9 × left / timer, 0.15)` s; CS 1.6 (`C4Think`) beeps every
/// 1.4 s from half a second in, its sound stepping `c4_beep1..5` after a quarter of the timer,
/// then 0.9 of the step before.
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
        beeps.bomb = None;
        return;
    };
    let now = server_ms(snap);
    if beeps.bomb != Some(bomb) {
        *beeps = Beeps {
            bomb: Some(bomb),
            next_beep: bomb.planted_ms + 500,
            next_wave: bomb.planted_ms,
            wave_ms: bomb.timer_ms as f64 / 4.0,
            wave: 0,
        };
    }
    let blow = bomb.planted_ms + bomb.timer_ms;
    if now >= blow {
        return;
    }
    let css = super::cs_world_model::css_installed();
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
        ("cs_c4_beep1".to_owned(), (0.1 + 0.9 * left).max(0.15))
    } else {
        (format!("cs_c4_beep{}", beeps.wave.max(1)), 1.4)
    };
    beeps.next_beep = now + (next * 1000.0) as i64;
    sounds.write(audio::AliasCommand::Play(audio::PlayAlias {
        event: None,
        namespace: asset_core::AssetNamespace::Iw4,
        alias,
        fallback: None,
        origin_inches: Some(bomb.origin),
        snd_ent: None,
    }));
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
}
