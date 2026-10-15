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

/// How smoke grenades look (`smoke_mode`): `weapon_iw4::cs::SMOKE_CSGO`, CS:GO's static cloud
/// (the default), `SMOKE_MW2`, MW2's particle smoke, or `SMOKE_CS2`, CS2's volumetric smoke,
/// still a work in progress. The server marks each pop with
/// its own (`CS_SMOKE_VOLUME_PARM`), so its clients draw the same smoke whatever theirs says.
static SMOKE_MODE: AtomicU32 = AtomicU32::new(weapon_iw4::cs::SMOKE_CSGO);

#[must_use]
pub fn smoke_mode() -> u32 {
    SMOKE_MODE.load(Ordering::Relaxed)
}

pub fn set_smoke_mode(mode: u32) {
    SMOKE_MODE.store(mode, Ordering::Relaxed);
}

/// Tells clients this server's movement (the server info `mv_mode`, and its cheats
/// `always_edgebug`, `always_pixelsurf` and `sv_autobunnyhop`, "1" or "0"): theirs follow it, so
/// their prediction moves as the server does and none turns a cheat on for themselves.
pub(crate) fn publish_movement_cheats(world: &mut crate::frame::FrameWorld) {
    let Some(mut runtime) = world.ecs().get_resource_mut::<crate::script::Runtime>() else {
        return;
    };
    let on = |on: bool| if on { "1" } else { "0" };
    for (name, value) in [
        ("mv_mode", movement_iw4::rules::mode().name()),
        ("always_edgebug", on(movement_iw4::rules::always_edgebug())),
        ("always_pixelsurf", on(movement_iw4::rules::always_pixelsurf())),
        ("sv_autobunnyhop", on(movement_iw4::rules::auto_bunnyhop())),
    ] {
        if runtime.dvars.get(name).map(String::as_str) != Some(value)
            || !runtime.server_info.contains(name)
        {
            runtime.server_info.insert(name.to_owned());
            runtime.dvars.insert(name.to_owned(), value.to_owned());
        }
    }
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

/// Whether `name` is a destructible's targetname: one of [`DESTRUCTIBLE_TARGETNAMES`], or a
/// map's own variant of one (the DLC maps' `destructible_vehicle_dlc2`).
pub(crate) fn is_destructible_targetname(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    DESTRUCTIBLE_TARGETNAMES.iter().any(|t| name == *t) || name.starts_with("destructible")
}

/// Map entities MW2's scripts break that have no destructible targetname, by model: the
/// vending machines `common_scripts/_dynamic_world` wrecks (sparks, a damaged model, a burst of
/// soda cans), on Terminal among others.
pub(crate) const DESTRUCTIBLE_MODEL_PREFIXES: [&str; 1] = ["com_vending"];
