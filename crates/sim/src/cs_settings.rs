//! Counter-Strike fork server switches that are not movement rules: what of MW2's sandbox stays
//! on for competitive play. A listen server and its own client share these.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// How CS guns shoot (`shooting_mode`): `weapon_iw4::csgo::SHOOTING_CSGO` (the default) or
/// `SHOOTING_CS16`. The server copies its own onto every player (`PlayerState::cs_shooting_mode`),
/// so its clients shoot the same way whatever theirs says.
static SHOOTING_MODE: AtomicU32 = AtomicU32::new(weapon_iw4::csgo::SHOOTING_CSGO);

#[must_use]
pub fn shooting_mode() -> u32 {
    SHOOTING_MODE.load(Ordering::Relaxed)
}

pub fn set_shooting_mode(mode: u32) {
    SHOOTING_MODE.store(mode, Ordering::Relaxed);
}

/// Map destructibles (cars, explosive barrels, breakable walls) take damage and blow up. Off by
/// default under CS rules (`sv_destructibles 0`): a competitive map should not change shape.
static DESTRUCTIBLES: AtomicBool = AtomicBool::new(!movement_iw4::rules::CS_RULES);

#[must_use]
pub fn destructibles_enabled() -> bool {
    DESTRUCTIBLES.load(Ordering::Relaxed)
}

pub fn set_destructibles(on: bool) {
    DESTRUCTIBLES.store(on, Ordering::Relaxed);
}

/// MW2 local sounds Counter-Strike has no place for: the breathing `_healthoverlay` plays after
/// a hit (MW2 regenerates health; CS does not).
pub(crate) const MUTED_LOCAL_SOUNDS: [&str; 2] = ["breathing_hurt", "breathing_better"];

/// Map entities MW2's destructible scripts drive (`common_scripts/_destructible`,
/// `maps/mp/_destructables`), by targetname.
pub(crate) const DESTRUCTIBLE_TARGETNAMES: [&str; 5] = [
    "destructible_vehicle",
    "destructible_toy",
    "destructable",
    // MW2's red barrels and crates (`_explosive_barrels`), on Afghan among others.
    "explodable_barrel",
    "flammable_crate",
];
