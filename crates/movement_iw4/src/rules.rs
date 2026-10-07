//! Which movement ruleset normal players use (`mv_mode`). Client prediction and the authority
//! of a listen server read the same setting, so both sides always simulate the same rules.

use core::sync::atomic::{AtomicU8, Ordering};

use playerstate_iw4::PlayerState;

use crate::{cs, source};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ruleset {
    /// GoldSrc `pm_shared` (CS 1.6).
    GoldSrc(cs::MovementProfile),
    /// Source / Momentum Mod `gamemovement` (CS:S, CS:GO).
    Source(source::SourceProfile),
}

/// Whether the CS fork's rules (movement, weapons, HUD) replace MW2's. `false` keeps retail IW4.
pub const CS_RULES: bool = true;

/// The movement presets `mv_mode` picks between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovementMode {
    /// CS:GO competitive: CS:GO's stamina, crouch fatigue, ground acceleration and speed clamp,
    /// 1.1x anti-bhop cap, airaccelerate 12, no autohop.
    Csgo,
    /// CS:GO movement stepped like a 64 tick server.
    Csgo64,
    /// CS:GO movement stepped like a 128 tick server.
    Csgo128,
    /// CS:S-style (Momentum's CS mode): CS:S stamina, airaccelerate 12, no autohop.
    Css,
    /// Surf servers: airaccelerate 150, autohop.
    Surf,
    /// Momentum Mod bhop: airaccelerate 1000, autohop, no stamina, 260 run speed.
    Mmod,
    /// Counter-Strike 1.6 (GoldSrc `pm_shared`): its smaller hull and lower jump.
    Cs16,
}

impl MovementMode {
    pub const ALL: [Self; 7] = [
        Self::Csgo,
        Self::Csgo64,
        Self::Csgo128,
        Self::Css,
        Self::Surf,
        Self::Mmod,
        Self::Cs16,
    ];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Csgo => "csgo",
            Self::Csgo64 => "csgo64",
            Self::Csgo128 => "csgo128",
            Self::Css => "css",
            Self::Surf => "surf",
            Self::Mmod => "mmod",
            Self::Cs16 => "cs16",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        let is = |names: &[&str]| names.iter().any(|n| n.eq_ignore_ascii_case(name));
        if is(&["csgo", "cs:go", "vanilla", "vnl"]) {
            Some(Self::Csgo)
        } else if is(&["csgo64", "64", "64tick"]) {
            Some(Self::Csgo64)
        } else if is(&["csgo128", "128", "128tick"]) {
            Some(Self::Csgo128)
        } else if is(&["css", "cs:s", "source"]) {
            Some(Self::Css)
        } else if is(&["surf"]) {
            Some(Self::Surf)
        } else if is(&["mmod", "momentum", "bhop"]) {
            Some(Self::Mmod)
        } else if is(&["cs16", "cs1.6", "1.6", "goldsrc"]) {
            Some(Self::Cs16)
        } else {
            None
        }
    }

    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Csgo => "CS:GO competitive (CS:GO stamina and crouch fatigue, 1.1x bhop cap)",
            Self::Csgo64 => "CS:GO movement on a 64 tick server's 15.6 ms steps",
            Self::Csgo128 => "CS:GO movement on a 128 tick server's 7.8 ms steps",
            Self::Css => "CS:S style (airaccelerate 12, CS:S stamina, no autohop)",
            Self::Surf => "surf (airaccelerate 150, autohop)",
            Self::Mmod => "Momentum bhop (airaccelerate 1000, autohop, no stamina, 260 speed)",
            Self::Cs16 => "CS 1.6 (GoldSrc movement, 1.6 hull and jump)",
        }
    }

    #[must_use]
    pub fn ruleset(self) -> Ruleset {
        match self {
            Self::Csgo => Ruleset::Source(source::CSGO),
            Self::Csgo64 => Ruleset::Source(source::CSGO_64),
            Self::Csgo128 => Ruleset::Source(source::CSGO_128),
            Self::Css => Ruleset::Source(source::CSS),
            Self::Surf => Ruleset::Source(source::MOMENTUM_SURF),
            Self::Mmod => Ruleset::Source(source::MOMENTUM_BHOP),
            Self::Cs16 => Ruleset::GoldSrc(cs::CS16),
        }
    }
}

static MODE: AtomicU8 = AtomicU8::new(0);

/// The movement preset in use.
#[must_use]
pub fn mode() -> MovementMode {
    MovementMode::ALL
        .get(usize::from(MODE.load(Ordering::Relaxed)))
        .copied()
        .unwrap_or(MovementMode::Csgo)
}

pub fn set_mode(mode: MovementMode) {
    let index = MovementMode::ALL.iter().position(|m| *m == mode).unwrap_or(0);
    MODE.store(index as u8, Ordering::Relaxed);
}

/// The ruleset normal players move with. `None` keeps retail IW4 movement.
#[must_use]
pub fn active() -> Option<Ruleset> {
    CS_RULES.then(|| mode().ruleset())
}

const PM_TYPE_NORMAL: i32 = 0;

/// The ruleset `ps` moves with this command, if any replaces IW4 movement.
#[must_use]
pub fn active_for(ps: &PlayerState) -> Option<Ruleset> {
    active().filter(|_| ps.pm_type == PM_TYPE_NORMAL)
}

/// Damage for landing at `speed` under the active ruleset.
#[must_use]
pub fn fall_damage(speed: f32) -> i32 {
    match active() {
        Some(Ruleset::GoldSrc(_)) => cs::fall_damage(speed),
        Some(Ruleset::Source(profile)) => source::fall_damage(&profile, speed),
        None => 0,
    }
}

/// CS run speed follows the held weapon: its move scale is `weapon max speed / 250`, the zoomed
/// scale while scoped. No weapon runs at full speed.
#[must_use]
pub fn weapon_speed_scale(ps: &PlayerState, scales: &crate::CmdScaleWalkContext) -> f32 {
    if ps.weapon == 0 {
        return 1.0;
    }
    let zoomed = ps.f_weapon_pos_frac >= 1.0 || ps.cs_zoom != 0;
    let scale = if zoomed && scales.weapon_ads_move_speed_scale > 0.0 {
        scales.weapon_ads_move_speed_scale
    } else {
        scales.weapon_move_speed_scale
    };
    if scale > 0.0 { scale } else { 1.0 }
}

/// Height of the solid box a player's body is to other players under CS rules: the stand or duck
/// hull, flat on top so players can stand on heads (boosting). `None` under IW4 rules, whose
/// bodies are capsules.
#[must_use]
pub fn body_height(ps: &PlayerState) -> Option<f32> {
    let ducked = ps.cs_duck_state & playerstate_iw4::cs_duck::DUCKED != 0;
    let (stand, duck) = match active_for(ps)? {
        Ruleset::GoldSrc(profile) => (profile.stand_height, profile.duck_height),
        Ruleset::Source(profile) => (profile.stand_height, profile.duck_height),
    };
    Some(if ducked { duck } else { stand })
}
