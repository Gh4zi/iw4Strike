//! Counter-Strike fork server switches that are not movement rules: what of MW2's sandbox stays
//! on for competitive play. A listen server and its own client share these.

use std::sync::atomic::{AtomicBool, Ordering};

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
pub(crate) const DESTRUCTIBLE_TARGETNAMES: [&str; 3] =
    ["destructible_vehicle", "destructible_toy", "destructable"];
