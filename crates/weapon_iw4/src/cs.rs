//! Counter-Strike 1.6 gunplay on MW2 weapon assets.
//!
//! Each [`CsWeapon`] borrows an MW2 weapon's model, animations and sounds (its `mw2_name`) and
//! replaces how it plays with the CS 1.6 rules as ReGameDLL ships them: damage with range
//! falloff, hitgroup multipliers, cycle time, magazine and reserve, reload time, run speed, the
//! accuracy that degrades with sustained fire, spread that depends on how the shooter moves, and
//! the punch-angle recoil (`KickBack`) that bullets fire along.

use crate::{HITLOC_COUNT, WeaponCombatFacts};

/// How a weapon's spread (`flSpread` in `FireBullets3`) depends on the shooter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CsSpread {
    /// Rifles and SMGs: `base + scale * accuracy`, by airborne / running (> 140 u/s) / still.
    Rifle {
        air: (f32, f32),
        run: (f32, f32),
        still: (f32, f32),
    },
    /// Pistols: `factor * (1 - accuracy)`, by airborne / moving / ducked / standing.
    Pistol {
        air: f32,
        moving: f32,
        ducked: f32,
        standing: f32,
    },
    /// Bolt snipers: fixed spread by state, plus `unscoped` when not zoomed.
    Sniper {
        air: f32,
        run: f32,
        walk: f32,
        ducked: f32,
        standing: f32,
        unscoped: f32,
    },
    /// Auto snipers (G3SG1, SG550): `(1 - accuracy) * (state + unscoped)` by airborne / moving /
    /// ducked / standing; `moving_scaled` false keeps the moving value whole (SG550).
    AutoSniper {
        air: f32,
        moving: f32,
        ducked: f32,
        standing: f32,
        unscoped: f32,
        moving_scaled: bool,
    },
    /// Shotguns: every pellet strays by this cone (`vecCone`).
    Cone(f32),
}

/// `KickBack(up_base, lateral_base, up_modifier, lateral_modifier, up_max, lateral_max,
/// direction_change)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsKick {
    pub up_base: f32,
    pub lateral_base: f32,
    pub up_modifier: f32,
    pub lateral_modifier: f32,
    pub up_max: f32,
    pub lateral_max: f32,
    pub direction_change: u32,
}

const fn kick(
    up_base: f32,
    lateral_base: f32,
    up_modifier: f32,
    lateral_modifier: f32,
    up_max: f32,
    lateral_max: f32,
    direction_change: u32,
) -> CsKick {
    CsKick {
        up_base,
        lateral_base,
        up_modifier,
        lateral_modifier,
        up_max,
        lateral_max,
        direction_change,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CsRecoil {
    /// Automatic weapons: a `KickBack` per state (moving, airborne, ducked, standing).
    KickBack {
        moving: CsKick,
        air: CsKick,
        ducked: CsKick,
        standing: CsKick,
    },
    /// Pistols and bolt snipers: a flat upward punch.
    Punch(f32),
    /// Auto snipers: pitch kicks up by a random `up` plus a quarter of the punch already there,
    /// and yaw by a random amount within `side`.
    AutoSniper { up: (f32, f32), side: f32 },
    /// Shotguns: a random whole-degree pitch kick, by on the ground / in the air.
    Shotgun {
        ground: (u32, u32),
        air: (u32, u32),
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CsAccuracy {
    /// `shots^3 / divisor + base`, capped at `max`. Retail 1.6 divides as integers, so the
    /// first few bullets of a spray keep `base` and the next jumps to `max`.
    Sustained {
        initial: f32,
        divisor: f32,
        /// Retail divides as integers for most guns (the MP5 divides by 220.1 as a float).
        integer: bool,
        base: f32,
        max: f32,
        /// Shots fired are raised to this power (3 for rifles, 2 for most SMGs).
        power: u32,
    },
    /// Auto snipers: `(seconds since the last shot) * scale + base`, at most `cap`; `first`
    /// when there was no last shot.
    SniperTime {
        initial: f32,
        scale: f32,
        base: f32,
        cap: f32,
        first: f32,
    },
    /// Pistols: accuracy drops by `(recover - since_last_shot) * factor`, clamped.
    Pistol {
        initial: f32,
        recover: f32,
        factor: f32,
        min: f32,
        max: f32,
    },
    /// Snipers ignore accuracy.
    None,
}

/// One CS 1.6 weapon and the MW2 weapon that wears it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsWeapon {
    /// The CS buy name (`ak47`, `deagle`...).
    pub name: &'static str,
    /// MW2 script name whose assets it borrows.
    pub mw2_name: &'static str,
    /// CS 1.6 first-person model in the install's `models/` folder, without `.mdl`.
    pub view_model: &'static str,
    /// Gunshot sounds in the install's `sound/weapons/`, without `.wav`; one is picked per shot.
    pub fire_sounds: &'static [&'static str],
    /// Counter-Strike: Source first-person model (`models/weapons/<name>.mdl` in its pack).
    pub css_view_model: &'static str,
    /// Counter-Strike: Source sound script entry of its gunshot.
    pub css_fire_sound: &'static str,
    pub price: i32,
    pub damage: f32,
    pub range_modifier: f32,
    /// Seconds between shots.
    pub cycle: f32,
    pub clip: i32,
    pub reserve: i32,
    pub reload: f32,
    pub max_speed: f32,
    /// Run speed while zoomed (snipers), else `max_speed`.
    pub max_speed_zoomed: f32,
    /// Scope fields of view (4:3 horizontal degrees) right click steps through before it
    /// unzooms (`SecondaryAttack`); empty without a scope.
    pub zoom: &'static [u32],
    pub semi_auto: bool,
    /// Bullets beyond this many units do nothing.
    pub distance: f32,
    pub spread: CsSpread,
    pub recoil: CsRecoil,
    pub accuracy: CsAccuracy,
    /// The pistol rule that resets shots fired when the trigger is released.
    pub pistol: bool,
    /// CS 1.6 dynamic crosshair: resting gap and growth per shot, in 640x480 pixels.
    pub crosshair: CsCrosshair,
    /// The silencer right click screws on and off (M4A1, USP), if any.
    pub silencer: Option<CsSilencer>,
    /// Ground speed above which a rifle or SMG uses its running spread (`RIFLE_RUN_SPEED`).
    pub run_speed: f32,
    /// Bullets per shot (shotgun pellets); 1 otherwise.
    pub pellets: u32,
    /// The burst mode right click switches to (Glock-18, FAMAS), if any.
    pub burst: Option<CsBurst>,
    /// Seconds between shots while zoomed, when that differs from `cycle` (AUG, SG 552); 0 = same.
    pub cycle_zoomed: f32,
    /// A shot drops the scope back to the unzoomed view until the gun is ready again (AWP, Scout).
    pub unzoom_on_fire: bool,
    /// Zooming draws the sniper scope overlay and hides the gun (snipers); the AUG and SG 552 only
    /// narrow the field of view.
    pub scope_overlay: bool,
    /// Shotguns reload one shell at a time: (start, per shell, finish) in seconds.
    pub shell_reload: Option<(f32, f32, f32)>,
    /// Two guns that fire in turn (Dual Elites); `cs_burst_modes` holds which hand is next.
    pub dual: bool,
}

/// A burst mode (`WPNSTATE_GLOCK18_BURST_MODE`, `WPNSTATE_FAMAS_BURST_MODE`): one press fires
/// `shots` bullets, `first_gap` then `gap` seconds apart, and the gun waits `cycle` before the
/// next burst.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsBurst {
    pub shots: u32,
    pub first_gap: f32,
    pub gap: f32,
    pub cycle: f32,
    pub damage: f32,
    pub range_modifier: f32,
    /// Spread of the first bullet of a burst.
    pub spread: CsSpread,
    /// The bullets after the first keep this spread (`FireRemaining`); `None` keeps the first
    /// bullet's own.
    pub follow_spread: Option<f32>,
    /// Range modifier of the bullets after the first, when it differs.
    pub follow_range_modifier: Option<f32>,
    pub fire_sounds: &'static [&'static str],
    pub css_fire_sound: &'static str,
}

/// What every [`CsWeapon`] starts from: an entry names only what it changes.
const BASE: CsWeapon = CsWeapon {
    name: "",
    mw2_name: "",
    view_model: "",
    fire_sounds: &[],
    css_view_model: "",
    css_fire_sound: "",
    price: 0,
    damage: 0.0,
    range_modifier: 1.0,
    cycle: 0.1,
    clip: 30,
    reserve: 90,
    reload: 3.0,
    max_speed: 250.0,
    max_speed_zoomed: 250.0,
    zoom: &[],
    semi_auto: false,
    distance: 8192.0,
    spread: CsSpread::Cone(0.0),
    recoil: CsRecoil::Punch(0.0),
    accuracy: CsAccuracy::None,
    pistol: false,
    crosshair: CsCrosshair {
        gap: 4.0,
        delta: 4.0,
    },
    silencer: None,
    run_speed: RIFLE_RUN_SPEED,
    pellets: 1,
    burst: None,
    cycle_zoomed: 0.0,
    unzoom_on_fire: true,
    scope_overlay: false,
    shell_reload: None,
    dual: false,
};

/// A silencer (`SecondaryAttack` on the M4A1 and USP): what changes while it is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsSilencer {
    pub damage: f32,
    pub range_modifier: f32,
    pub spread: CsSpread,
    /// Seconds attaching or detaching takes; the gun cannot fire meanwhile.
    pub adjust: f32,
    pub fire_sounds: &'static [&'static str],
    pub css_fire_sound: &'static str,
}

impl CsWeapon {
    /// The weapon as it fires with its silencer `on` (itself without one).
    #[must_use]
    pub fn with_silencer(&self, on: bool) -> CsWeapon {
        match self.silencer {
            Some(silencer) if on => CsWeapon {
                damage: silencer.damage,
                range_modifier: silencer.range_modifier,
                spread: silencer.spread,
                fire_sounds: silencer.fire_sounds,
                css_fire_sound: silencer.css_fire_sound,
                ..*self
            },
            _ => *self,
        }
    }
}

impl CsWeapon {
    /// The weapon as a burst bullet fires: `burst` 1 is the first bullet of a burst, 2 a later
    /// one, anything else (or a gun without a burst mode) is itself.
    #[must_use]
    pub fn with_burst(&self, burst: u8) -> CsWeapon {
        match self.burst {
            Some(mode) if burst == 1 || burst == 2 => CsWeapon {
                damage: mode.damage,
                range_modifier: if burst == 2 {
                    mode.follow_range_modifier.unwrap_or(mode.range_modifier)
                } else {
                    mode.range_modifier
                },
                spread: mode.spread,
                fire_sounds: mode.fire_sounds,
                css_fire_sound: mode.css_fire_sound,
                ..*self
            },
            _ => *self,
        }
    }

    /// Whether the CS layer times this gun's shots itself (burst, or a cycle that depends on the
    /// scope) rather than leaving them to the MW2 weapon machine.
    #[must_use]
    pub fn gated(&self) -> bool {
        self.burst.is_some() || self.cycle_zoomed > 0.0
    }
}

/// Bit of a weapon-fire event's `simulation_flags` marking a burst bullet (see
/// [`SILENCED_SHOT_FLAG`]).
pub const BURST_SHOT_FLAG: u8 = 4;

/// `PlayerState::cs_silencers` bit of CS weapon `index`.
#[must_use]
pub fn silencer_bit(index: u8) -> u32 {
    1u32.checked_shl(u32::from(index)).unwrap_or(0)
}

/// Bit of a weapon-fire event's `simulation_flags` marking a shot from a silenced gun, so every
/// client picks the silenced gunshot (bit 0 is the impact events' "penetrated").
pub const SILENCED_SHOT_FLAG: u8 = 2;

/// Crosshair gap at rest and how much each shot widens it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsCrosshair {
    pub gap: f32,
    pub delta: f32,
}

/// The knife's crosshair, also shown with empty hands.
pub const CS_KNIFE_CROSSHAIR: CsCrosshair = CsCrosshair {
    gap: 7.0,
    delta: 3.0,
};

/// CS 1.6 hitgroup multipliers (`CBasePlayer::TraceAttack`) on MW2 hit locations.
pub const CS_LOCATION_DAMAGE: [f32; HITLOC_COUNT] = [
    1.0,  // none
    4.0,  // helmet
    4.0,  // head
    1.0,  // neck: CS chest
    1.0,  // torso_upper: chest
    1.25, // torso_lower: stomach
    1.0,  // right_arm_upper
    1.0,  // left_arm_upper
    1.0,  // right_arm_lower
    1.0,  // left_arm_lower
    1.0,  // right_hand
    1.0,  // left_hand
    0.75, // right_leg_upper
    0.75, // left_leg_upper
    0.75, // right_leg_lower
    0.75, // left_leg_lower
    0.75, // right_foot
    0.75, // left_foot
    1.0,  // gun
    1.0,  // shield
];

const RIFLE_RUN_SPEED: f32 = 140.0;

pub const CS_WEAPONS: [CsWeapon; 24] = [
    CsWeapon {
        name: "ak47",
        mw2_name: "ak47_mp",
        view_model: "v_ak47",
        fire_sounds: &["ak47-1", "ak47-2"],
        css_view_model: "v_rif_ak47",
        css_fire_sound: "Weapon_AK47.Single",
        price: 2500,
        damage: 36.0,
        range_modifier: 0.98,
        cycle: 0.0955,
        clip: 30,
        reserve: 90,
        reload: 2.45,
        max_speed: 221.0,
        max_speed_zoomed: 221.0,
        zoom: &[],
        semi_auto: false,
        distance: 8192.0,
        spread: CsSpread::Rifle {
            air: (0.04, 0.4),
            run: (0.04, 0.07),
            still: (0.0, 0.0275),
        },
        recoil: CsRecoil::KickBack {
            moving: kick(1.5, 0.45, 0.225, 0.05, 6.5, 2.5, 7),
            air: kick(2.0, 1.0, 0.5, 0.35, 9.0, 6.0, 5),
            ducked: kick(0.9, 0.35, 0.15, 0.025, 5.5, 1.5, 9),
            standing: kick(1.0, 0.375, 0.175, 0.0375, 5.75, 1.75, 8),
        },
        accuracy: CsAccuracy::Sustained {
            initial: 0.2,
            divisor: 200.0,
            integer: true,
            base: 0.35,
            max: 1.25,
            power: 3,
        },
        pistol: false,
        crosshair: CsCrosshair {
            gap: 4.0,
            delta: 4.0,
        },
        silencer: None,
        ..BASE
    },
    CsWeapon {
        name: "m4a1",
        mw2_name: "m4_mp",
        view_model: "v_m4a1",
        fire_sounds: &["m4a1_unsil-1", "m4a1_unsil-2"],
        css_view_model: "v_rif_m4a1",
        css_fire_sound: "Weapon_M4A1.Single",
        price: 3100,
        damage: 32.0,
        range_modifier: 0.97,
        cycle: 0.0875,
        clip: 30,
        reserve: 90,
        reload: 3.05,
        max_speed: 230.0,
        max_speed_zoomed: 230.0,
        zoom: &[],
        semi_auto: false,
        distance: 8192.0,
        spread: CsSpread::Rifle {
            air: (0.035, 0.4),
            run: (0.035, 0.07),
            still: (0.0, 0.02),
        },
        recoil: CsRecoil::KickBack {
            moving: kick(1.0, 0.45, 0.28, 0.045, 3.75, 3.0, 7),
            air: kick(1.2, 0.5, 0.23, 0.15, 5.5, 3.5, 6),
            ducked: kick(0.6, 0.3, 0.2, 0.0125, 3.25, 2.0, 7),
            standing: kick(0.65, 0.35, 0.25, 0.015, 3.5, 2.25, 7),
        },
        accuracy: CsAccuracy::Sustained {
            initial: 0.2,
            divisor: 220.0,
            integer: true,
            base: 0.3,
            max: 1.0,
            power: 3,
        },
        pistol: false,
        crosshair: CsCrosshair {
            gap: 4.0,
            delta: 3.0,
        },
        // `M4A1_DAMAGE_SIL`, `M4A1_RANGE_MODIFER_SIL`; standing spread 0.025 (not 0.02).
        silencer: Some(CsSilencer {
            damage: 33.0,
            range_modifier: 0.95,
            spread: CsSpread::Rifle {
                air: (0.035, 0.4),
                run: (0.035, 0.07),
                still: (0.0, 0.025),
            },
            adjust: 2.0,
            fire_sounds: &["m4a1-1"],
            css_fire_sound: "Weapon_M4A1.Silenced",
        }),
        ..BASE
    },
    CsWeapon {
        name: "awp",
        mw2_name: "cheytac_mp",
        view_model: "v_awp",
        fire_sounds: &["awp1"],
        css_view_model: "v_snip_awp",
        css_fire_sound: "Weapon_AWP.Single",
        price: 4750,
        damage: 115.0,
        range_modifier: 0.99,
        cycle: 1.45,
        clip: 10,
        reserve: 30,
        reload: 2.5,
        max_speed: 210.0,
        max_speed_zoomed: 150.0,
        zoom: &[40, 10],
        semi_auto: true,
        distance: 8192.0,
        spread: CsSpread::Sniper {
            air: 0.85,
            run: 0.25,
            walk: 0.1,
            ducked: 0.0,
            standing: 0.001,
            unscoped: 0.08,
        },
        recoil: CsRecoil::Punch(2.0),
        accuracy: CsAccuracy::None,
        pistol: false,
        crosshair: CsCrosshair {
            gap: 8.0,
            delta: 3.0,
        },
        silencer: None,
        scope_overlay: true,
        ..BASE
    },
    CsWeapon {
        name: "deagle",
        mw2_name: "deserteagle_mp",
        view_model: "v_deagle",
        fire_sounds: &["deagle-1", "deagle-2"],
        css_view_model: "v_pist_deagle",
        css_fire_sound: "Weapon_DEagle.Single",
        price: 650,
        damage: 54.0,
        range_modifier: 0.81,
        cycle: 0.225,
        clip: 7,
        reserve: 35,
        reload: 2.2,
        max_speed: 250.0,
        max_speed_zoomed: 250.0,
        zoom: &[],
        semi_auto: true,
        distance: 4096.0,
        spread: CsSpread::Pistol {
            air: 1.5,
            moving: 0.25,
            ducked: 0.115,
            standing: 0.13,
        },
        recoil: CsRecoil::Punch(2.0),
        accuracy: CsAccuracy::Pistol {
            initial: 0.9,
            recover: 0.4,
            factor: 0.35,
            min: 0.55,
            max: 0.9,
        },
        pistol: true,
        crosshair: CsCrosshair {
            gap: 8.0,
            delta: 3.0,
        },
        silencer: None,
        ..BASE
    },
    CsWeapon {
        name: "usp",
        mw2_name: "usp_mp",
        view_model: "v_usp",
        fire_sounds: &["usp_unsil-1"],
        css_view_model: "v_pist_usp",
        css_fire_sound: "Weapon_USP.Single",
        price: 500,
        damage: 34.0,
        range_modifier: 0.79,
        cycle: 0.15,
        clip: 12,
        reserve: 100,
        reload: 2.7,
        max_speed: 250.0,
        max_speed_zoomed: 250.0,
        zoom: &[],
        semi_auto: true,
        distance: 4096.0,
        spread: CsSpread::Pistol {
            air: 1.2,
            moving: 0.225,
            ducked: 0.08,
            standing: 0.1,
        },
        recoil: CsRecoil::Punch(2.0),
        accuracy: CsAccuracy::Pistol {
            initial: 0.92,
            recover: 0.3,
            factor: 0.275,
            min: 0.6,
            max: 0.92,
        },
        pistol: true,
        crosshair: CsCrosshair {
            gap: 8.0,
            delta: 3.0,
        },
        // `USP_DAMAGE_SIL`, `USP_ADJUST_SIL_TIME` (retail 3.0).
        silencer: Some(CsSilencer {
            damage: 30.0,
            range_modifier: 0.79,
            spread: CsSpread::Pistol {
                air: 1.3,
                moving: 0.25,
                ducked: 0.125,
                standing: 0.15,
            },
            adjust: 3.0,
            fire_sounds: &["usp1", "usp2"],
            css_fire_sound: "Weapon_USP.SilencedShot",
        }),
        ..BASE
    },
    CsWeapon {
        name: "glock",
        mw2_name: "glock_mp",
        view_model: "v_glock18",
        // CS plays glock18-2 for a single shot and glock18-1 for a burst (`EV_FireGlock18`).
        fire_sounds: &["glock18-2"],
        css_view_model: "v_pist_glock18",
        css_fire_sound: "Weapon_Glock.Single",
        price: 400,
        damage: 25.0,
        range_modifier: 0.75,
        cycle: 0.15,
        clip: 20,
        reserve: 120,
        reload: 2.2,
        max_speed: 250.0,
        max_speed_zoomed: 250.0,
        zoom: &[],
        semi_auto: true,
        distance: 8192.0,
        spread: CsSpread::Pistol {
            air: 1.0,
            moving: 0.165,
            ducked: 0.075,
            standing: 0.1,
        },
        recoil: CsRecoil::Punch(0.0),
        accuracy: CsAccuracy::Pistol {
            initial: 0.9,
            recover: 0.325,
            factor: 0.275,
            min: 0.6,
            max: 0.9,
        },
        pistol: true,
        crosshair: CsCrosshair {
            gap: 8.0,
            delta: 3.0,
        },
        // `GLOCK18Fire` in burst mode: 3 bullets 0.1 s apart, cycle 0.5 s; the bullets after the
        // first fire at spread 0.05 with range modifier 0.9 (`FireRemaining`).
        burst: Some(CsBurst {
            shots: 3,
            first_gap: 0.1,
            gap: 0.1,
            cycle: 0.5,
            damage: 25.0,
            range_modifier: 0.75,
            spread: CsSpread::Pistol {
                air: 1.2,
                moving: 0.185,
                ducked: 0.095,
                standing: 0.3,
            },
            follow_spread: Some(0.05),
            follow_range_modifier: Some(0.9),
            fire_sounds: &["glock18-1"],
            css_fire_sound: "Weapon_Glock.Single",
        }),
        ..BASE
    },
    // ---- Pistols ----
    CsWeapon {
        name: "p228",
        mw2_name: "deserteaglegold_mp",
        view_model: "v_p228",
        fire_sounds: &["p228-1"],
        css_view_model: "v_pist_p228",
        css_fire_sound: "Weapon_P228.Single",
        price: 600,
        damage: 32.0,
        range_modifier: 0.8,
        cycle: 0.15,
        clip: 13,
        reserve: 52,
        reload: 2.7,
        semi_auto: true,
        distance: 4096.0,
        spread: CsSpread::Pistol {
            air: 1.5,
            moving: 0.255,
            ducked: 0.075,
            standing: 0.15,
        },
        recoil: CsRecoil::Punch(2.0),
        accuracy: CsAccuracy::Pistol {
            initial: 0.9,
            recover: 0.325,
            factor: 0.3,
            min: 0.6,
            max: 0.9,
        },
        pistol: true,
        crosshair: CsCrosshair {
            gap: 8.0,
            delta: 3.0,
        },
        ..BASE
    },
    CsWeapon {
        name: "fiveseven",
        mw2_name: "kriss_mp",
        view_model: "v_fiveseven",
        fire_sounds: &["fiveseven-1"],
        css_view_model: "v_pist_fiveseven",
        css_fire_sound: "Weapon_FiveSeven.Single",
        price: 750,
        damage: 20.0,
        range_modifier: 0.885,
        cycle: 0.15,
        clip: 20,
        reserve: 100,
        reload: 2.7,
        semi_auto: true,
        distance: 4096.0,
        spread: CsSpread::Pistol {
            air: 1.5,
            moving: 0.255,
            ducked: 0.075,
            standing: 0.15,
        },
        recoil: CsRecoil::Punch(2.0),
        accuracy: CsAccuracy::Pistol {
            initial: 0.92,
            recover: 0.275,
            factor: 0.25,
            min: 0.725,
            max: 0.92,
        },
        pistol: true,
        crosshair: CsCrosshair {
            gap: 8.0,
            delta: 3.0,
        },
        ..BASE
    },
    CsWeapon {
        name: "elite",
        mw2_name: "sa80_mp",
        view_model: "v_elite",
        fire_sounds: &["elite_fire"],
        css_view_model: "v_pist_elite",
        css_fire_sound: "Weapon_Elite.Single",
        price: 800,
        damage: 36.0,
        range_modifier: 0.75,
        // `ELITEFire`: 0.2 s less the retail 0.125 s.
        cycle: 0.075,
        clip: 30,
        reserve: 120,
        reload: 4.5,
        semi_auto: true,
        distance: 4096.0,
        spread: CsSpread::Pistol {
            air: 1.3,
            moving: 0.175,
            ducked: 0.08,
            standing: 0.1,
        },
        recoil: CsRecoil::Punch(2.0),
        accuracy: CsAccuracy::Pistol {
            initial: 0.88,
            recover: 0.325,
            factor: 0.275,
            min: 0.55,
            max: 0.88,
        },
        pistol: true,
        crosshair: CsCrosshair {
            gap: 8.0,
            delta: 3.0,
        },
        dual: true,
        ..BASE
    },
    // ---- Shotguns ----
    CsWeapon {
        name: "m3",
        mw2_name: "spas12_mp",
        view_model: "v_m3",
        fire_sounds: &["m3-1"],
        css_view_model: "v_shot_m3super90",
        css_fire_sound: "Weapon_M3.Single",
        price: 1700,
        damage: 20.0,
        cycle: 0.875,
        clip: 8,
        reserve: 32,
        max_speed: 230.0,
        max_speed_zoomed: 230.0,
        semi_auto: true,
        distance: 3000.0,
        spread: CsSpread::Cone(0.0675),
        recoil: CsRecoil::Shotgun {
            ground: (4, 6),
            air: (8, 11),
        },
        crosshair: CsCrosshair {
            gap: 8.0,
            delta: 6.0,
        },
        pellets: 9,
        shell_reload: Some((0.55, 0.45, 0.45)),
        ..BASE
    },
    CsWeapon {
        name: "xm1014",
        mw2_name: "m1014_mp",
        view_model: "v_xm1014",
        fire_sounds: &["xm1014-1"],
        css_view_model: "v_shot_xm1014",
        css_fire_sound: "Weapon_XM1014.Single",
        price: 3000,
        damage: 20.0,
        cycle: 0.25,
        clip: 7,
        reserve: 32,
        max_speed: 240.0,
        max_speed_zoomed: 240.0,
        semi_auto: true,
        distance: 3048.0,
        spread: CsSpread::Cone(0.0725),
        recoil: CsRecoil::Shotgun {
            ground: (3, 5),
            air: (7, 10),
        },
        crosshair: CsCrosshair {
            gap: 9.0,
            delta: 4.0,
        },
        pellets: 6,
        shell_reload: Some((0.55, 0.3, 0.4)),
        ..BASE
    },
    // ---- Submachine guns ----
    CsWeapon {
        name: "mac10",
        mw2_name: "uzi_mp",
        view_model: "v_mac10",
        fire_sounds: &["mac10-1"],
        css_view_model: "v_smg_mac10",
        css_fire_sound: "Weapon_MAC10.Single",
        price: 1400,
        damage: 29.0,
        range_modifier: 0.82,
        cycle: 0.07,
        clip: 30,
        reserve: 100,
        reload: 3.15,
        spread: CsSpread::Rifle {
            air: (0.0, 0.375),
            run: (0.0, 0.03),
            still: (0.0, 0.03),
        },
        recoil: CsRecoil::KickBack {
            moving: kick(0.9, 0.45, 0.25, 0.035, 3.5, 2.75, 7),
            air: kick(1.3, 0.55, 0.4, 0.05, 4.75, 3.75, 5),
            ducked: kick(0.75, 0.4, 0.175, 0.03, 2.75, 2.5, 10),
            standing: kick(0.775, 0.425, 0.2, 0.03, 3.0, 2.75, 9),
        },
        accuracy: CsAccuracy::Sustained {
            initial: 0.15,
            divisor: 200.0,
            integer: true,
            base: 0.6,
            max: 1.65,
            power: 3,
        },
        crosshair: CsCrosshair {
            gap: 9.0,
            delta: 3.0,
        },
        ..BASE
    },
    CsWeapon {
        name: "tmp",
        mw2_name: "tmp_mp",
        view_model: "v_tmp",
        fire_sounds: &["tmp-1", "tmp-2"],
        css_view_model: "v_smg_tmp",
        css_fire_sound: "Weapon_TMP.Single",
        price: 1250,
        damage: 20.0,
        range_modifier: 0.85,
        cycle: 0.07,
        clip: 30,
        reserve: 120,
        reload: 2.12,
        spread: CsSpread::Rifle {
            air: (0.0, 0.25),
            run: (0.0, 0.03),
            still: (0.0, 0.03),
        },
        recoil: CsRecoil::KickBack {
            moving: kick(0.8, 0.4, 0.2, 0.03, 3.0, 2.5, 7),
            air: kick(1.1, 0.5, 0.35, 0.045, 4.5, 3.5, 6),
            ducked: kick(0.7, 0.35, 0.125, 0.025, 2.5, 2.0, 10),
            standing: kick(0.725, 0.375, 0.15, 0.025, 2.75, 2.25, 9),
        },
        accuracy: CsAccuracy::Sustained {
            initial: 0.2,
            divisor: 200.0,
            integer: true,
            base: 0.55,
            max: 1.4,
            power: 3,
        },
        crosshair: CsCrosshair {
            gap: 7.0,
            delta: 3.0,
        },
        ..BASE
    },
    CsWeapon {
        name: "mp5",
        mw2_name: "mp5k_mp",
        view_model: "v_mp5",
        fire_sounds: &["mp5-1", "mp5-2"],
        css_view_model: "v_smg_mp5",
        css_fire_sound: "Weapon_MP5Navy.Single",
        price: 1500,
        damage: 26.0,
        range_modifier: 0.84,
        cycle: 0.075,
        clip: 30,
        reserve: 120,
        reload: 2.63,
        spread: CsSpread::Rifle {
            air: (0.0, 0.2),
            run: (0.0, 0.04),
            still: (0.0, 0.04),
        },
        recoil: CsRecoil::KickBack {
            moving: kick(0.5, 0.275, 0.2, 0.03, 3.0, 2.0, 10),
            air: kick(0.9, 0.475, 0.35, 0.0425, 5.0, 3.0, 6),
            ducked: kick(0.225, 0.15, 0.1, 0.015, 2.0, 1.0, 10),
            standing: kick(0.25, 0.175, 0.125, 0.02, 2.25, 1.25, 10),
        },
        // The MP5 is the one gun that divides by a float (220.1).
        accuracy: CsAccuracy::Sustained {
            initial: 0.0,
            divisor: 220.1,
            integer: false,
            base: 0.45,
            max: 0.75,
            power: 2,
        },
        crosshair: CsCrosshair {
            gap: 6.0,
            delta: 3.0,
        },
        ..BASE
    },
    CsWeapon {
        name: "ump45",
        mw2_name: "ump45_mp",
        view_model: "v_ump45",
        fire_sounds: &["ump45-1"],
        css_view_model: "v_smg_ump45",
        css_fire_sound: "Weapon_UMP45.Single",
        price: 1700,
        damage: 30.0,
        range_modifier: 0.82,
        cycle: 0.1,
        clip: 25,
        reserve: 100,
        reload: 3.5,
        spread: CsSpread::Rifle {
            air: (0.0, 0.24),
            run: (0.0, 0.04),
            still: (0.0, 0.04),
        },
        recoil: CsRecoil::KickBack {
            moving: kick(0.55, 0.3, 0.225, 0.03, 3.5, 2.5, 10),
            air: kick(0.125, 0.65, 0.55, 0.0475, 5.5, 4.0, 10),
            ducked: kick(0.25, 0.175, 0.125, 0.02, 2.25, 1.25, 10),
            standing: kick(0.275, 0.2, 0.15, 0.0225, 2.5, 1.5, 10),
        },
        accuracy: CsAccuracy::Sustained {
            initial: 0.0,
            divisor: 210.0,
            integer: true,
            base: 0.5,
            max: 1.0,
            power: 2,
        },
        crosshair: CsCrosshair {
            gap: 6.0,
            delta: 3.0,
        },
        ..BASE
    },
    CsWeapon {
        name: "p90",
        mw2_name: "p90_mp",
        view_model: "v_p90",
        fire_sounds: &["p90-1"],
        css_view_model: "v_smg_p90",
        css_fire_sound: "Weapon_P90.Single",
        price: 2350,
        damage: 21.0,
        range_modifier: 0.885,
        cycle: 0.066,
        clip: 50,
        reserve: 100,
        reload: 3.4,
        max_speed: 245.0,
        max_speed_zoomed: 245.0,
        spread: CsSpread::Rifle {
            air: (0.0, 0.3),
            run: (0.0, 0.115),
            still: (0.0, 0.045),
        },
        recoil: CsRecoil::KickBack {
            moving: kick(0.45, 0.3, 0.2, 0.0275, 4.0, 2.25, 7),
            air: kick(0.9, 0.45, 0.35, 0.04, 5.25, 3.5, 4),
            ducked: kick(0.275, 0.2, 0.125, 0.02, 3.0, 1.0, 9),
            standing: kick(0.3, 0.225, 0.125, 0.02, 3.25, 1.25, 8),
        },
        accuracy: CsAccuracy::Sustained {
            initial: 0.2,
            divisor: 175.0,
            integer: true,
            base: 0.45,
            max: 1.0,
            power: 2,
        },
        crosshair: CsCrosshair {
            gap: 7.0,
            delta: 4.0,
        },
        run_speed: 170.0,
        ..BASE
    },
    // ---- Rifles ----
    CsWeapon {
        name: "galil",
        mw2_name: "fal_mp",
        view_model: "v_galil",
        fire_sounds: &["galil-1", "galil-2"],
        css_view_model: "v_rif_galil",
        css_fire_sound: "Weapon_Galil.Single",
        price: 2000,
        damage: 30.0,
        range_modifier: 0.98,
        cycle: 0.0875,
        clip: 35,
        reserve: 90,
        reload: 2.45,
        max_speed: 240.0,
        max_speed_zoomed: 240.0,
        spread: CsSpread::Rifle {
            air: (0.04, 0.3),
            run: (0.04, 0.07),
            still: (0.0, 0.0375),
        },
        recoil: CsRecoil::KickBack {
            moving: kick(1.0, 0.45, 0.28, 0.045, 3.75, 3.0, 7),
            air: kick(1.2, 0.5, 0.23, 0.15, 5.5, 3.5, 6),
            ducked: kick(0.6, 0.3, 0.2, 0.0125, 3.25, 2.0, 7),
            standing: kick(0.65, 0.35, 0.25, 0.015, 3.5, 2.25, 7),
        },
        accuracy: CsAccuracy::Sustained {
            initial: 0.2,
            divisor: 200.0,
            integer: true,
            base: 0.35,
            max: 1.25,
            power: 3,
        },
        crosshair: CsCrosshair {
            gap: 4.0,
            delta: 4.0,
        },
        ..BASE
    },
    CsWeapon {
        name: "famas",
        mw2_name: "famas_mp",
        view_model: "v_famas",
        fire_sounds: &["famas-1", "famas-2"],
        css_view_model: "v_rif_famas",
        css_fire_sound: "Weapon_FAMAS.Single",
        price: 2250,
        damage: 30.0,
        range_modifier: 0.96,
        cycle: 0.0825,
        clip: 25,
        reserve: 90,
        reload: 3.3,
        max_speed: 240.0,
        max_speed_zoomed: 240.0,
        // Full auto adds 0.01 to the spread (`FamasFire`); burst does not.
        spread: CsSpread::Rifle {
            air: (0.04, 0.3),
            run: (0.04, 0.07),
            still: (0.01, 0.02),
        },
        recoil: CsRecoil::KickBack {
            moving: kick(1.0, 0.45, 0.275, 0.05, 4.0, 2.5, 7),
            air: kick(1.25, 0.45, 0.22, 0.18, 5.5, 4.0, 5),
            ducked: kick(0.575, 0.325, 0.2, 0.011, 3.25, 2.0, 8),
            standing: kick(0.625, 0.375, 0.25, 0.0125, 3.5, 2.25, 8),
        },
        accuracy: CsAccuracy::Sustained {
            initial: 0.2,
            divisor: 215.0,
            integer: true,
            base: 0.3,
            max: 1.0,
            power: 3,
        },
        crosshair: CsCrosshair {
            gap: 3.0,
            delta: 3.0,
        },
        // `FamasFire` in burst mode: 3 bullets (0.05 s then 0.1 s apart), 34 damage, 0.55 s
        // before the next burst; the later bullets keep the first one's spread.
        burst: Some(CsBurst {
            shots: 3,
            first_gap: 0.05,
            gap: 0.1,
            cycle: 0.55,
            damage: 34.0,
            range_modifier: 0.96,
            spread: CsSpread::Rifle {
                air: (0.03, 0.3),
                run: (0.03, 0.07),
                still: (0.0, 0.02),
            },
            follow_spread: None,
            follow_range_modifier: None,
            fire_sounds: &["famas-burst"],
            css_fire_sound: "Weapon_FAMAS.Single",
        }),
        ..BASE
    },
    CsWeapon {
        name: "sg552",
        mw2_name: "fn2000_mp",
        view_model: "v_sg552",
        fire_sounds: &["sg552-1", "sg552-2"],
        css_view_model: "v_rif_sg552",
        css_fire_sound: "Weapon_SG552.Single",
        price: 3500,
        damage: 33.0,
        range_modifier: 0.955,
        cycle: 0.0825,
        clip: 30,
        reserve: 90,
        reload: 3.0,
        max_speed: 235.0,
        max_speed_zoomed: 200.0,
        zoom: &[55],
        spread: CsSpread::Rifle {
            air: (0.035, 0.45),
            run: (0.035, 0.075),
            still: (0.0, 0.02),
        },
        recoil: CsRecoil::KickBack {
            moving: kick(1.0, 0.45, 0.28, 0.04, 4.25, 2.5, 7),
            air: kick(1.25, 0.45, 0.22, 0.18, 6.0, 4.0, 5),
            ducked: kick(0.6, 0.35, 0.2, 0.0125, 3.7, 2.0, 10),
            standing: kick(0.625, 0.375, 0.25, 0.0125, 4.0, 2.25, 9),
        },
        accuracy: CsAccuracy::Sustained {
            initial: 0.2,
            divisor: 220.0,
            integer: true,
            base: 0.3,
            max: 1.0,
            power: 3,
        },
        crosshair: CsCrosshair {
            gap: 3.0,
            delta: 4.0,
        },
        cycle_zoomed: 0.135,
        unzoom_on_fire: false,
        ..BASE
    },
    CsWeapon {
        name: "aug",
        mw2_name: "aug_mp",
        view_model: "v_aug",
        fire_sounds: &["aug-1"],
        css_view_model: "v_rif_aug",
        css_fire_sound: "Weapon_AUG.Single",
        price: 3500,
        damage: 32.0,
        range_modifier: 0.96,
        cycle: 0.0825,
        clip: 30,
        reserve: 90,
        reload: 3.3,
        max_speed: 240.0,
        max_speed_zoomed: 240.0,
        zoom: &[55],
        spread: CsSpread::Rifle {
            air: (0.035, 0.4),
            run: (0.035, 0.07),
            still: (0.0, 0.02),
        },
        recoil: CsRecoil::KickBack {
            moving: kick(1.0, 0.45, 0.275, 0.05, 4.0, 2.5, 7),
            air: kick(1.25, 0.45, 0.22, 0.18, 5.5, 4.0, 5),
            ducked: kick(0.575, 0.325, 0.2, 0.011, 3.25, 2.0, 8),
            standing: kick(0.625, 0.375, 0.25, 0.0125, 3.5, 2.25, 8),
        },
        accuracy: CsAccuracy::Sustained {
            initial: 0.2,
            divisor: 215.0,
            integer: true,
            base: 0.3,
            max: 1.0,
            power: 3,
        },
        crosshair: CsCrosshair {
            gap: 3.0,
            delta: 3.0,
        },
        cycle_zoomed: 0.135,
        unzoom_on_fire: false,
        ..BASE
    },
    // ---- Snipers ----
    CsWeapon {
        name: "scout",
        mw2_name: "m21_mp",
        view_model: "v_scout",
        fire_sounds: &["scout_fire-1"],
        css_view_model: "v_snip_scout",
        css_fire_sound: "Weapon_Scout.Single",
        price: 2750,
        damage: 75.0,
        range_modifier: 0.98,
        cycle: 1.25,
        clip: 10,
        reserve: 90,
        reload: 2.0,
        max_speed: 260.0,
        max_speed_zoomed: 220.0,
        zoom: &[40, 15],
        semi_auto: true,
        spread: CsSpread::Sniper {
            air: 0.2,
            run: 0.075,
            walk: 0.007,
            ducked: 0.0,
            standing: 0.007,
            unscoped: 0.025,
        },
        recoil: CsRecoil::Punch(2.0),
        crosshair: CsCrosshair {
            gap: 7.0,
            delta: 3.0,
        },
        run_speed: 170.0,
        scope_overlay: true,
        ..BASE
    },
    CsWeapon {
        name: "g3sg1",
        mw2_name: "wa2000_mp",
        view_model: "v_g3sg1",
        fire_sounds: &["g3sg1-1"],
        css_view_model: "v_snip_g3sg1",
        css_fire_sound: "Weapon_G3SG1.Single",
        price: 5000,
        damage: 80.0,
        range_modifier: 0.98,
        cycle: 0.25,
        clip: 20,
        reserve: 90,
        reload: 3.5,
        max_speed: 210.0,
        max_speed_zoomed: 150.0,
        zoom: &[40, 15],
        semi_auto: true,
        spread: CsSpread::AutoSniper {
            air: 0.45,
            moving: 0.15,
            ducked: 0.035,
            standing: 0.055,
            unscoped: 0.025,
            moving_scaled: true,
        },
        recoil: CsRecoil::AutoSniper {
            up: (0.75, 1.75),
            side: 0.75,
        },
        accuracy: CsAccuracy::SniperTime {
            initial: 0.2,
            scale: 0.3,
            base: 0.55,
            cap: 0.98,
            first: 0.98,
        },
        crosshair: CsCrosshair {
            gap: 6.0,
            delta: 4.0,
        },
        unzoom_on_fire: false,
        scope_overlay: true,
        ..BASE
    },
    CsWeapon {
        name: "sg550",
        mw2_name: "barrett_mp",
        view_model: "v_sg550",
        fire_sounds: &["sg550-1"],
        css_view_model: "v_snip_sg550",
        css_fire_sound: "Weapon_SG550.Single",
        price: 4200,
        damage: 70.0,
        range_modifier: 0.98,
        cycle: 0.25,
        clip: 30,
        reserve: 90,
        reload: 3.35,
        max_speed: 210.0,
        max_speed_zoomed: 150.0,
        zoom: &[40, 15],
        semi_auto: true,
        spread: CsSpread::AutoSniper {
            air: 0.45,
            moving: 0.15,
            ducked: 0.04,
            standing: 0.05,
            unscoped: 0.025,
            moving_scaled: false,
        },
        recoil: CsRecoil::AutoSniper {
            up: (0.75, 1.25),
            side: 0.75,
        },
        accuracy: CsAccuracy::SniperTime {
            initial: 0.9,
            scale: 0.35,
            base: 0.65,
            cap: 0.98,
            first: 0.9,
        },
        crosshair: CsCrosshair {
            gap: 5.0,
            delta: 2.0,
        },
        unzoom_on_fire: false,
        scope_overlay: true,
        ..BASE
    },
    // ---- Machine gun ----
    CsWeapon {
        name: "m249",
        mw2_name: "m240_mp",
        view_model: "v_m249",
        fire_sounds: &["m249-1", "m249-2"],
        css_view_model: "v_mach_m249para",
        css_fire_sound: "Weapon_M249.Single",
        price: 5750,
        damage: 32.0,
        range_modifier: 0.97,
        cycle: 0.1,
        clip: 100,
        reserve: 200,
        reload: 4.7,
        max_speed: 220.0,
        max_speed_zoomed: 220.0,
        spread: CsSpread::Rifle {
            air: (0.045, 0.5),
            run: (0.045, 0.095),
            still: (0.0, 0.03),
        },
        recoil: CsRecoil::KickBack {
            moving: kick(1.1, 0.5, 0.3, 0.06, 4.0, 3.0, 8),
            air: kick(1.8, 0.65, 0.45, 0.125, 5.0, 3.5, 8),
            ducked: kick(0.75, 0.325, 0.25, 0.025, 3.5, 2.5, 9),
            standing: kick(0.8, 0.35, 0.3, 0.03, 3.75, 3.0, 9),
        },
        accuracy: CsAccuracy::Sustained {
            initial: 0.2,
            divisor: 175.0,
            integer: true,
            base: 0.4,
            max: 0.9,
            power: 3,
        },
        crosshair: CsCrosshair {
            gap: 6.0,
            delta: 6.0,
        },
        ..BASE
    },
];

/// The CS 1.6 knife (`CKnife`) on slot 3, worn by an MW2 pistol that never fires. It is not in
/// [`CS_WEAPONS`]: no bullets, magazine, spread or recoil.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsKnife {
    pub name: &'static str,
    pub mw2_name: &'static str,
    pub view_model: &'static str,
    pub css_view_model: &'static str,
    pub max_speed: f32,
    pub slash_damage: f32,
    pub stab_damage: f32,
    /// Reach of each attack from the eye, units.
    pub slash_range: f32,
    pub stab_range: f32,
    /// A stab in the back (`flDot > 0.8`) multiplies damage by this.
    pub backstab_multiplier: f32,
    /// Seconds until the next slash after a slash that hit / missed, and until the next stab
    /// after any slash.
    pub slash_hit_delay: f32,
    pub slash_miss_delay: f32,
    pub slash_stab_delay: f32,
    /// Seconds until either attack after a stab that hit / missed.
    pub stab_hit_delay: f32,
    pub stab_miss_delay: f32,
    /// Half extents of the hull a missed line sweeps instead (`head_hull`): sideways, up.
    pub hull: (f32, f32),
}

pub const CS_KNIFE: CsKnife = CsKnife {
    name: "knife",
    mw2_name: "beretta_mp",
    view_model: "v_knife",
    css_view_model: "v_knife_t",
    max_speed: 250.0,
    slash_damage: 15.0,
    stab_damage: 65.0,
    slash_range: 48.0,
    stab_range: 32.0,
    backstab_multiplier: 3.0,
    slash_hit_delay: 0.4,
    slash_miss_delay: 0.35,
    slash_stab_delay: 0.5,
    stab_hit_delay: 1.1,
    stab_miss_delay: 1.0,
    hull: (16.0, 18.0),
};

/// `WeaponCombatFacts::cs_weapon` of the knife: past the guns, so [`cs_weapon`] stays guns only.
pub const CS_KNIFE_INDEX: u8 = CS_WEAPONS.len() as u8 + 1;

#[must_use]
pub fn is_knife(index: u8) -> bool {
    index == CS_KNIFE_INDEX
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnifeAttack {
    Slash,
    Stab,
}

/// The attack the knife makes this command (`ItemPostFrame`: the stab button wins), if its timer
/// has run out.
#[must_use]
pub fn knife_attack(
    now_ms: i32,
    next_slash_ms: i32,
    next_stab_ms: i32,
    slash_held: bool,
    stab_held: bool,
) -> Option<KnifeAttack> {
    if stab_held && now_ms >= next_stab_ms {
        Some(KnifeAttack::Stab)
    } else if slash_held && now_ms >= next_slash_ms {
        Some(KnifeAttack::Slash)
    } else {
        None
    }
}

/// Seconds until the next slash and the next stab after `attack`.
#[must_use]
pub fn knife_delays(knife: &CsKnife, attack: KnifeAttack, hit: bool) -> (f32, f32) {
    match (attack, hit) {
        (KnifeAttack::Slash, true) => (knife.slash_hit_delay, knife.slash_stab_delay),
        (KnifeAttack::Slash, false) => (knife.slash_miss_delay, knife.slash_stab_delay),
        (KnifeAttack::Stab, true) => (knife.stab_hit_delay, knife.stab_hit_delay),
        (KnifeAttack::Stab, false) => (knife.stab_miss_delay, knife.stab_miss_delay),
    }
}

#[must_use]
pub fn knife_range(knife: &CsKnife, attack: KnifeAttack) -> f32 {
    match attack {
        KnifeAttack::Slash => knife.slash_range,
        KnifeAttack::Stab => knife.stab_range,
    }
}

/// A stab lands in the back (`CKnife::Stab`): the flat line from the attacker to the victim
/// runs along the victim's facing (`flDot > 0.8`), i.e. the attacker stands behind them,
/// wherever the attacker looks.
#[must_use]
pub fn is_backstab(attacker_origin: [f32; 3], victim_origin: [f32; 3], victim_yaw: f32) -> bool {
    let (dx, dy) = (
        victim_origin[0] - attacker_origin[0],
        victim_origin[1] - attacker_origin[1],
    );
    let len = libm::sqrtf(dx * dx + dy * dy);
    if len < 1e-3 {
        return false;
    }
    let v = victim_yaw.to_radians();
    (dx * libm::cosf(v) + dy * libm::sinf(v)) / len > 0.8
}

/// Damage before hitgroup: a slash, or a stab (tripled in the back). Retail 1.6 sets the next
/// attack time before it checks for the faster-swing bonus, so a slash always deals the base.
#[must_use]
pub fn knife_damage(knife: &CsKnife, attack: KnifeAttack, backstab: bool) -> f32 {
    match attack {
        KnifeAttack::Slash => knife.slash_damage,
        KnifeAttack::Stab if backstab => knife.stab_damage * knife.backstab_multiplier,
        KnifeAttack::Stab => knife.stab_damage,
    }
}

/// A CS 1.6 grenade held on slot 4 (`CHEGrenade`, `CFlashbang`, `CSmokeGrenade`), worn by an
/// MW2 pistol that never fires; its magazine counts the grenades. Throwing launches the MW2
/// grenade `projectile` along CS's throw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsGrenade {
    /// The CS buy name.
    pub name: &'static str,
    pub mw2_name: &'static str,
    /// MW2 grenade weapon whose projectile, explosion and effects a throw uses.
    pub projectile: &'static str,
    pub view_model: &'static str,
    pub css_view_model: &'static str,
    pub price: i32,
    /// How many a player may carry.
    pub carry: i32,
    /// CS:S sound script entry played when it goes off, instead of the MW2 projectile's.
    pub css_explode_sound: Option<&'static str>,
    /// CS 1.6 `sound/weapons` waves for the same, picked by the explosion.
    pub explode_sounds: &'static [&'static str],
    /// CS:S sound script entry played when it bounces (CS:GO's `BounceSound` plays the same).
    pub css_bounce_sound: &'static str,
    /// CS 1.6 `sound/weapons` waves for the same, picked by the bounce.
    pub bounce_sounds: &'static [&'static str],
}

pub const CS_GRENADES: [CsGrenade; 3] = [
    CsGrenade {
        name: "hegrenade",
        mw2_name: "coltanaconda_mp",
        projectile: "frag_grenade_mp",
        view_model: "v_hegrenade",
        css_view_model: "v_eq_fraggrenade",
        price: 300,
        carry: 1,
        css_explode_sound: Some("BaseGrenade.Explode"),
        // Half-Life's `weapons/explode3-5.wav`, CS 1.6's HE blast.
        explode_sounds: &["explode3", "explode4", "explode5"],
        css_bounce_sound: "HEGrenade.Bounce",
        bounce_sounds: &["he_bounce-1"],
    },
    CsGrenade {
        name: "flashbang",
        mw2_name: "beretta393_mp",
        projectile: "flash_grenade_mp",
        view_model: "v_flashbang",
        css_view_model: "v_eq_flashbang",
        price: 200,
        carry: 2,
        css_explode_sound: Some("Flashbang.Explode"),
        explode_sounds: &["flashbang-1", "flashbang-2"],
        css_bounce_sound: "Flashbang.Bounce",
        bounce_sounds: &["grenade_hit1", "grenade_hit2", "grenade_hit3"],
    },
    CsGrenade {
        name: "smokegrenade",
        mw2_name: "pp2000_mp",
        projectile: "smoke_grenade_mp",
        view_model: "v_smokegrenade",
        css_view_model: "v_eq_smokegrenade",
        price: 300,
        carry: 1,
        css_explode_sound: Some("BaseSmokeEffect.Sound"),
        explode_sounds: &["sg_explode"],
        css_bounce_sound: "SmokeGrenade.Bounce",
        bounce_sounds: &["grenade_hit1", "grenade_hit2", "grenade_hit3"],
    },
];

/// The CS grenade whose throws fly as MW2's `projectile` weapon.
#[must_use]
pub fn cs_grenade_for_projectile(projectile: &str) -> Option<&'static CsGrenade> {
    CS_GRENADES.iter().find(|grenade| grenade.projectile == projectile)
}

/// Counter-Strike 2's model of a CS weapon and the viewmodel animation graph its clips come from,
/// in CS2's own files (`game/csgo/pak01_dir.vpk`): each CS 1.6 gun as the CS2 gun whose CS:GO
/// numbers it plays with (the TMP as the MP9, the M3 as the Nova...), and the knife, grenades and
/// C4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cs2View {
    /// The CS weapon.
    pub name: &'static str,
    /// The weapon model.
    pub model: &'static str,
    /// The viewmodel graph whose clips the weapon plays (CS2's gun, knife or grenade graph's
    /// variation for it); `graph#part` keeps only that graph's own clips whose path holds `part`.
    pub graph: &'static str,
}

const CS2_VIEWS: [Cs2View; 29] = [
    Cs2View {
        name: "ak47",
        model: "weapons/models/ak47/weapon_rif_ak47.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+ak47.vnmgraph",
    },
    Cs2View {
        name: "m4a1",
        model: "weapons/models/m4a1_silencer/weapon_rif_m4a1_silencer.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+m4a1s.vnmgraph",
    },
    Cs2View {
        name: "awp",
        model: "weapons/models/awp/weapon_snip_awp.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+awp.vnmgraph",
    },
    Cs2View {
        name: "deagle",
        model: "weapons/models/deagle/weapon_pist_deagle.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+deagle.vnmgraph",
    },
    Cs2View {
        name: "usp",
        model: "weapons/models/usp_silencer/weapon_pist_usp_silencer.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+usp.vnmgraph",
    },
    Cs2View {
        name: "glock",
        model: "weapons/models/glock18/weapon_pist_glock18.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+glock.vnmgraph",
    },
    Cs2View {
        name: "p228",
        model: "weapons/models/p250/weapon_pist_p250.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+p250.vnmgraph",
    },
    Cs2View {
        name: "fiveseven",
        model: "weapons/models/fiveseven/weapon_pist_fiveseven.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+five_seven.vnmgraph",
    },
    Cs2View {
        name: "elite",
        model: "weapons/models/elite/weapon_pist_elite.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun_elites.vnmgraph",
    },
    Cs2View {
        name: "m3",
        model: "weapons/models/nova/weapon_shot_nova.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+nova.vnmgraph",
    },
    Cs2View {
        name: "xm1014",
        model: "weapons/models/xm1014/weapon_shot_xm1014.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+xm1014.vnmgraph",
    },
    Cs2View {
        name: "mac10",
        model: "weapons/models/mac10/weapon_smg_mac10.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+mac10.vnmgraph",
    },
    Cs2View {
        name: "tmp",
        model: "weapons/models/mp9/weapon_smg_mp9.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+mp9.vnmgraph",
    },
    Cs2View {
        name: "mp5",
        model: "weapons/models/mp5sd/weapon_smg_mp5sd.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+mp5sd.vnmgraph",
    },
    Cs2View {
        name: "ump45",
        model: "weapons/models/ump45/weapon_smg_ump45.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+ump45.vnmgraph",
    },
    Cs2View {
        name: "p90",
        model: "weapons/models/p90/weapon_smg_p90.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+p90.vnmgraph",
    },
    Cs2View {
        name: "galil",
        model: "weapons/models/galilar/weapon_rif_galilar.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+galil.vnmgraph",
    },
    Cs2View {
        name: "famas",
        model: "weapons/models/famas/weapon_rif_famas.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+famas.vnmgraph",
    },
    Cs2View {
        name: "sg552",
        model: "weapons/models/sg556/weapon_rif_sg556.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+sg556.vnmgraph",
    },
    Cs2View {
        name: "aug",
        model: "weapons/models/aug/weapon_rif_aug.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+aug.vnmgraph",
    },
    Cs2View {
        name: "scout",
        model: "weapons/models/ssg08/weapon_snip_ssg08.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+ssg08.vnmgraph",
    },
    Cs2View {
        name: "g3sg1",
        model: "weapons/models/g3sg1/weapon_snip_g3sg1.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+g3sg1.vnmgraph",
    },
    Cs2View {
        name: "sg550",
        model: "weapons/models/scar20/weapon_snip_scar20.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+scar20.vnmgraph",
    },
    Cs2View {
        name: "m249",
        model: "weapons/models/m249/weapon_mach_m249.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_gun.vnmgraph+m249.vnmgraph",
    },
    Cs2View {
        name: "knife",
        model: "weapons/models/knife/knife_default_ct/weapon_knife_default_ct.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_knife.vnmgraph+default_ct.vnmgraph",
    },
    Cs2View {
        name: "hegrenade",
        model: "weapons/models/grenade/hegrenade/weapon_hegrenade.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_grenade.vnmgraph+he.vnmgraph",
    },
    Cs2View {
        name: "flashbang",
        model: "weapons/models/grenade/flashbang/weapon_flashbang.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_grenade.vnmgraph+flash.vnmgraph",
    },
    Cs2View {
        name: "smokegrenade",
        model: "weapons/models/grenade/smokegrenade/weapon_smokegrenade.vmdl",
        graph: "animation/graphs/viewmodel/viewmodel_grenade.vnmgraph+smoke.vnmgraph",
    },
    Cs2View {
        name: "c4",
        model: "weapons/models/c4/weapon_c4.vmdl",
        // CS2 lists the C4's clips in its main viewmodel graph, beside every weapon's own graph.
        graph: "animation/graphs/viewmodel/viewmodel.vnmgraph#/equipment/c4/",
    },
];

/// [`Cs2View`] of the CS weapon named `name`.
#[must_use]
pub fn cs2_view(name: &str) -> Option<Cs2View> {
    CS2_VIEWS.iter().find(|view| view.name == name).copied()
}

/// `WeaponCombatFacts::cs_weapon` of the first grenade; the others follow.
pub const CS_GRENADE_INDEX: u8 = CS_KNIFE_INDEX + 1;

#[must_use]
pub fn cs_grenade(index: u8) -> Option<&'static CsGrenade> {
    index
        .checked_sub(CS_GRENADE_INDEX)
        .and_then(|i| CS_GRENADES.get(usize::from(i)))
}

#[must_use]
pub fn is_grenade(index: u8) -> bool {
    cs_grenade(index).is_some()
}

/// The CS 1.6 C4 (`CC4`) on slot 5, worn by MW2's own bomb weapon. Only the bomb carrier has
/// it; held, attack in a bomb site plants it ([`CS_C4_ARMING_SECONDS`], frozen in place). The
/// bomb mode's script does the rest (the site, the timer, the explosion).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsC4 {
    pub name: &'static str,
    pub mw2_name: &'static str,
    pub view_model: &'static str,
    pub css_view_model: &'static str,
    pub max_speed: f32,
}

pub const CS_C4: CsC4 = CsC4 {
    name: "c4",
    mw2_name: "briefcase_bomb_mp",
    view_model: "v_c4",
    css_view_model: "v_c4",
    max_speed: 250.0,
};

/// `C4_ARMING_ON_TIME`: holding attack this long in a site plants the bomb.
pub const CS_C4_ARMING_SECONDS: f32 = 3.0;
/// Defusing takes this long, or [`CS_DEFUSE_KIT_SECONDS`] with a defuse kit.
pub const CS_DEFUSE_SECONDS: f32 = 10.0;
pub const CS_DEFUSE_KIT_SECONDS: f32 = 5.0;
/// `DEFUSEKIT_PRICE`; Counter-Terrorists only.
pub const CS_DEFUSE_KIT_PRICE: i32 = 200;
/// The planted bomb's blast (`m_flBombRadius` 500 by default): 500 damage falling off linearly
/// to nothing at 3.5 times that.
pub const CS_C4_DAMAGE: f32 = 500.0;
pub const CS_C4_RADIUS: f32 = 1750.0;

/// `WeaponCombatFacts::cs_weapon` of the C4: past the grenades.
pub const CS_C4_INDEX: u8 = CS_GRENADE_INDEX + CS_GRENADES.len() as u8;

#[must_use]
pub fn is_c4(index: u8) -> bool {
    index == CS_C4_INDEX
}

/// HE grenade blast (`pev->dmg` 100, radius `dmg * 3.5`), falling off linearly to the edge.
pub const CS_HE_DAMAGE: i32 = 100;
pub const CS_HE_RADIUS: i32 = 350;
/// Every CS grenade's fuse once thrown.
pub const CS_GRENADE_FUSE_MS: i32 = 1500;
/// CS:GO's smoke grenade (`CSmokeGrenadeProjectile`): still moving when its fuse is up, it looks
/// again every this long until it has stopped, then pops.
pub const CS_SMOKE_RECHECK_MS: i32 = 200;
/// A popped smoke grenade lies in its smoke this long before it is removed (`Think_Fade` 12.5 s
/// on, then 255 alpha steps a tick at 64 ticks a second).
pub const CS_SMOKE_GRENADE_LINGER_MS: i32 = 16_500;
/// `event_parm` of the second firing of a CS smoke's cloud, which keeps it going as long as
/// CS's; it makes no sound.
pub const CS_SMOKE_REFIRE_PARM: i32 = 1;
/// `event_parm` of a CS smoke's pop when the server draws smokes as CS2's volumetric cloud
/// (`smoke_mode cs2`): each client fills the cloud through the map itself and fires no
/// particle smoke, and the pop is not fired again.
pub const CS_SMOKE_VOLUME_PARM: i32 = 2;

/// How a server's smoke grenades look (`smoke_mode`): CS2's volumetric cloud that fills the
/// space it pops in (the default), or the CS:GO-timed particle smoke.
pub const SMOKE_CS2: u32 = 0;
pub const SMOKE_CSGO: u32 = 1;

#[must_use]
pub fn smoke_mode_from_name(name: &str) -> Option<u32> {
    let name = name.trim();
    if name.eq_ignore_ascii_case("cs2") {
        Some(SMOKE_CS2)
    } else if name.eq_ignore_ascii_case("csgo") {
        Some(SMOKE_CSGO)
    } else {
        None
    }
}
/// A pulled pin throws no sooner than this after the pull.
pub const CS_GRENADE_PULL_MS: i32 = 500;
/// After a throw: the next grenade comes up, or the last one's hand retires.
pub const CS_GRENADE_REDEPLOY_MS: i32 = 750;
pub const CS_GRENADE_RETIRE_MS: i32 = 500;

/// Flashbang (`RadiusFlash`, CS:GO): strength 3 at the flash (`sv_flashbang_strength` 3.55,
/// read as a whole number), falling to 0 at 3000 units.
pub const CS_FLASH_STRENGTH: f32 = 3.0;
pub const CS_FLASH_RADIUS: f32 = 3000.0;
/// How much of a flash reaches eyes it has no straight line to (`PercentageOfFlashForPlayer`):
/// this much for each of three bent lines that gets there — via a point 50 units above the
/// flash, and via points 75 units to either side of it (and 10 up).
pub const CS_FLASH_PARTIAL: f32 = 0.167;
pub const CS_FLASH_BEND_UP: f32 = 50.0;
pub const CS_FLASH_BEND_SIDE: f32 = 75.0;
pub const CS_FLASH_BEND_SIDE_UP: f32 = 10.0;

/// A CS flash on a player's screen (CS:GO `m_flFlashDuration`, `m_flFlashBangTime`,
/// `m_flFlashMaxAlpha`), timed from when the white-out began: it ends `end_ms` in, the latest
/// flash lasts `duration_ms` (what the frozen frame fades over), and `alpha` is how white it
/// gets (255 full).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsFlash {
    pub duration_ms: i32,
    pub end_ms: i32,
    pub alpha: u32,
}

/// What a flash does to a player at `distance` from it (`RadiusFlash`), `facing` = cosine
/// between their view and the line to the flash, `seen` = how much of it reaches their eyes (1
/// for a straight line, else [`CS_FLASH_PARTIAL`] per bent line): (hold, fade) in seconds.
/// CS:GO numbers per point of strength: looking at it (facing ≥ 0.6) 1.25 and 2.5; to the side
/// (≥ 0.3) 0.8 and 1.75; further to the side (≥ -0.2) 0.5 and 1; facing away 0.25 and 0.5.
#[must_use]
pub fn flash_for(distance: f32, facing: f32, seen: f32) -> Option<(f32, f32)> {
    let strength = CS_FLASH_STRENGTH - distance * CS_FLASH_STRENGTH / CS_FLASH_RADIUS;
    if strength <= 0.0 || seen <= 0.0 {
        return None;
    }
    let (hold, fade) = if facing >= 0.6 {
        (1.25, 2.5)
    } else if facing >= 0.3 {
        (0.8, 1.75)
    } else if facing >= -0.2 {
        (0.5, 1.0)
    } else {
        (0.25, 0.5)
    };
    Some((strength * hold * seen, strength * fade * seen))
}

/// The screen lasts the flash's fade over this (`Blind`).
pub const CS_FLASH_DURATION_DIVISOR: f32 = 1.4;
/// The white comes up over this long at the start of a fresh flash (255 / 45 a frame at 60).
pub const CS_FLASH_BUILD_UP_MS: f32 = 255.0 / 45.0 / 60.0 * 1000.0;
/// The screen stays fully white until this much of the flash is left, then clears.
pub const CS_FLASH_CERTAIN_BLINDNESS_MS: f32 = 3000.0;

/// A flash with `fade_s` blinding a player (`Blind`): the screen lasts `fade_s / 1.4`. On top of
/// a flash still showing (`old` and the time since it began) it only lasts as long as whichever
/// is longer, carrying on from the old one (no new white-up, same frozen frame); otherwise it
/// starts afresh. Returns the flash and whether it starts afresh now.
#[must_use]
pub fn blind(fade_s: f32, old: Option<(CsFlash, i32)>) -> (CsFlash, bool) {
    let duration_ms = libm::roundf(fade_s / CS_FLASH_DURATION_DIVISOR * 1000.0) as i32;
    match old {
        Some((old, elapsed_ms)) if elapsed_ms >= 0 && elapsed_ms < old.end_ms => {
            let duration_ms = duration_ms.max(old.end_ms - elapsed_ms);
            let flash = CsFlash {
                duration_ms,
                end_ms: elapsed_ms + duration_ms,
                alpha: old.alpha.max(255),
            };
            (flash, false)
        }
        _ => {
            let flash = CsFlash {
                duration_ms,
                end_ms: duration_ms,
                alpha: 255,
            };
            (flash, true)
        }
    }
}

/// How white the screen is `elapsed_ms` into `flash` (0..1), and how strongly the frozen frame
/// of the flash moment still shows over the world (`UpdateFlashBangEffect`): both come up over
/// [`CS_FLASH_BUILD_UP_MS`]; then the frozen frame fades evenly to the end of the latest flash,
/// while the white stays full until [`CS_FLASH_CERTAIN_BLINDNESS_MS`] are left and clears on
/// the square of what is left of those.
#[must_use]
pub fn flash_screen(flash: CsFlash, elapsed_ms: i32) -> Option<(f32, f32)> {
    if elapsed_ms < 0 || elapsed_ms >= flash.end_ms {
        return None;
    }
    let peak = flash.alpha as f32 / 255.0;
    let elapsed = elapsed_ms as f32;
    if elapsed < CS_FLASH_BUILD_UP_MS {
        let up = peak * elapsed / CS_FLASH_BUILD_UP_MS;
        return Some((up, up));
    }
    let left = (flash.end_ms - elapsed_ms) as f32;
    let frame = peak * (left / flash.duration_ms.max(1) as f32).clamp(0.0, 1.0);
    let white = if left > CS_FLASH_CERTAIN_BLINDNESS_MS {
        peak
    } else {
        let part = left / CS_FLASH_CERTAIN_BLINDNESS_MS;
        peak * part * part
    };
    Some((white, frame))
}

/// CS:GO's grenade throw velocity (every grenade's weapon data); a throw flies at 0.9 of it.
pub const CS_GRENADE_THROW_VELOCITY: f32 = 750.0;
/// A throw leaves the hand this long after the buttons are let go (`StartGrenadeThrow`).
pub const CS_GRENADE_RELEASE_MS: i32 = 100;
/// The thrower's own velocity rides along this much: a running or jumping throw goes further.
pub const CS_GRENADE_OWNER_VELOCITY: f32 = 1.25;
/// The weakest throw (right click alone) flies at this much of the speed, from this much below
/// the eye.
pub const CS_GRENADE_LOB_SPEED: f32 = 0.3;
pub const CS_GRENADE_LOB_LOWER: f32 = 12.0;
/// How fast (per second) the throw strength moves toward what the held buttons ask for.
pub const CS_GRENADE_STRENGTH_RATE: f32 = 1.3;
/// The throw starts this far ahead of the eye, pulled back this much from anything in the way
/// (a 4-unit box traced 22 units out, then 6 back).
pub const CS_GRENADE_THROW_REACH: f32 = 22.0;
pub const CS_GRENADE_THROW_PULLBACK: f32 = 6.0;
pub const CS_GRENADE_THROW_HALF_SIZE: f32 = 2.0;

/// The throw strength a pin is pulled with: a right click starts a lob (0), a left click a full
/// throw (1). Both at once start from the lob.
#[must_use]
pub fn grenade_strength_at_pull(secondary: bool) -> f32 {
    if secondary { 0.0 } else { 1.0 }
}

/// The throw strength `dt_ms` later with the pin out and these buttons held: it moves toward 1
/// for the left button, 0 for the right, 0.5 for both.
#[must_use]
pub fn grenade_strength(strength: f32, primary: bool, secondary: bool, dt_ms: i32) -> f32 {
    let ideal = 0.5 + if primary { 0.5 } else { 0.0 } - if secondary { 0.5 } else { 0.0 };
    let step = CS_GRENADE_STRENGTH_RATE * dt_ms.max(0) as f32 / 1000.0;
    if strength < ideal {
        (strength + step).min(ideal)
    } else {
        (strength - step).max(ideal)
    }
}

/// CS:GO's throw (`ThrowGrenade`) at `strength` (0 lob .. 1 full): the view pitch (positive
/// down) gets up to 10 degrees of lift (all of it looking level, none looking straight up or
/// down), and the speed is 675, a lob's 30% of it. Returns (pitch, speed).
#[must_use]
pub fn grenade_throw(view_pitch: f32, strength: f32) -> (f32, f32) {
    let mut pitch = view_pitch;
    if pitch > 90.0 {
        pitch -= 360.0;
    } else if pitch < -90.0 {
        pitch += 360.0;
    }
    let pitch = pitch.clamp(-90.0, 90.0);
    let pitch = pitch - 10.0 * (90.0 - pitch.abs()) / 90.0;
    let strength = strength.clamp(0.0, 1.0);
    let speed = (CS_GRENADE_THROW_VELOCITY * 0.9).clamp(15.0, 750.0)
        * (CS_GRENADE_LOB_SPEED + (1.0 - CS_GRENADE_LOB_SPEED) * strength);
    (pitch, speed)
}

/// How far below the eye a throw at `strength` starts.
#[must_use]
pub fn grenade_throw_lower(strength: f32) -> f32 {
    CS_GRENADE_LOB_LOWER * (1.0 - strength.clamp(0.0, 1.0))
}

/// How much speed a CS grenade keeps off a bounce (`GetGrenadeElasticity`), and off a player
/// a further 0.3 of that.
pub const CS_GRENADE_ELASTICITY: f32 = 0.45;
pub const CS_GRENADE_PLAYER_ELASTICITY: f32 = 0.3;
/// A grenade that lands slower than this comes to rest.
pub const CS_GRENADE_SLEEP_SPEED: f32 = 20.0;

/// A CS grenade bouncing off a surface with `normal` (`ResolveFlyCollisionCustom`): reflected
/// and slowed to [`CS_GRENADE_ELASTICITY`]. Landing on a floor (normal up past 0.7, or a slope
/// slower than [`CS_GRENADE_SLEEP_SPEED`]) it comes to rest once that slow; a fast steep landing
/// loses more (so the first toss doesn't spring off the ground). Players are not floors. Returns
/// the new velocity and whether it now rests.
#[must_use]
pub fn grenade_bounce(incoming: [f32; 3], normal: [f32; 3], off_player: bool) -> ([f32; 3], bool) {
    let elasticity = (CS_GRENADE_ELASTICITY
        * if off_player {
            CS_GRENADE_PLAYER_ELASTICITY
        } else {
            1.0
        })
    .clamp(0.0, 0.9);
    let into: f32 = (0..3).map(|i| incoming[i] * normal[i]).sum();
    let mut out: [f32; 3] =
        core::array::from_fn(|i| (incoming[i] - 2.0 * into * normal[i]) * elasticity);
    let speed_sq: f32 = out.iter().map(|v| v * v).sum();
    let sleep_sq = CS_GRENADE_SLEEP_SPEED * CS_GRENADE_SLEEP_SPEED;
    let floor = normal[2] > 0.7 || (normal[2] > 0.1 && speed_sq < sleep_sq);
    if off_player || !floor {
        return (out, false);
    }
    if speed_sq < sleep_sq {
        return ([0.0; 3], true);
    }
    if speed_sq > 96_000.0 {
        let along = (0..3).map(|i| out[i] * normal[i]).sum::<f32>() / libm::sqrtf(speed_sq);
        if along > 0.5 {
            let padding = (1.0 - along) + 0.5;
            out = out.map(|v| v * padding);
        }
    }
    (out, false)
}

impl CsWeapon {
    /// Counter-Strike buy/selection slot: 1 primary, 2 pistol.
    #[must_use]
    pub fn slot(&self) -> u8 {
        if self.pistol { 2 } else { 1 }
    }
}

/// `weapon`'s gunshot sound for shot `shot` (a name in the install's `sound/weapons/`).
#[must_use]
pub fn fire_sound(weapon: &CsWeapon, shot: u32) -> Option<&'static str> {
    let count = weapon.fire_sounds.len();
    weapon
        .fire_sounds
        .get((shot as usize).checked_rem(count)?)
        .copied()
}

/// The CS selection slot a held weapon answers to: its CS slot, else 2 for an MW2 pistol and 1
/// for any other primary-inventory weapon; 0 for equipment.
#[must_use]
pub fn slot_of(facts: &WeaponCombatFacts) -> u8 {
    if is_knife(facts.cs_weapon) {
        return 3;
    }
    if is_grenade(facts.cs_weapon) {
        return 4;
    }
    if is_c4(facts.cs_weapon) {
        return 5;
    }
    if let Some(weapon) = cs_weapon(facts.cs_weapon) {
        return weapon.slot();
    }
    match (facts.inventory_type, facts.weap_class) {
        (0, crate::WEAPCLASS_PISTOL) => 2,
        (0, _) => 1,
        _ => 0,
    }
}

/// The run speed every CS weapon's `max_speed` is relative to (the knife).
pub const CS_BASE_SPEED: f32 = 250.0;

/// `WeaponCombatFacts::cs_weapon` holds this table's index plus one; 0 is not a CS weapon.
#[must_use]
pub fn cs_weapon(index: u8) -> Option<&'static CsWeapon> {
    index
        .checked_sub(1)
        .and_then(|i| CS_WEAPONS.get(usize::from(i)))
}

/// The CS weapon an MW2 weapon wears, by its script name (`ak47_mp`, or a path ending in it).
#[must_use]
pub fn cs_weapon_index_for(script_name: &str) -> Option<u8> {
    let name = script_name.rsplit(['/', ':']).next().unwrap_or(script_name);
    if CS_KNIFE.mw2_name.eq_ignore_ascii_case(name) {
        return Some(CS_KNIFE_INDEX);
    }
    if CS_C4.mw2_name.eq_ignore_ascii_case(name) {
        return Some(CS_C4_INDEX);
    }
    if let Some(i) = CS_GRENADES
        .iter()
        .position(|g| g.mw2_name.eq_ignore_ascii_case(name))
    {
        return u8::try_from(i).ok().map(|i| CS_GRENADE_INDEX + i);
    }
    CS_WEAPONS
        .iter()
        .position(|w| w.mw2_name.eq_ignore_ascii_case(name))
        .and_then(|i| u8::try_from(i + 1).ok())
}

/// Kevlar (`ARMOR_RATIO` 0.5, `ARMOR_BONUS` 0.5): body hits keep `ratio` of the damage and the
/// vest pays `bonus` per point it stopped; a blast doubles the bonus.
pub const CS_ARMOR_RATIO: f32 = 0.5;
pub const CS_ARMOR_BONUS: f32 = 0.5;
pub const CS_KEVLAR_PRICE: i32 = 650;
pub const CS_KEVLAR_HELMET_PRICE: i32 = 1000;

/// How much more of its damage a CS weapon puts through kevlar (`TakeDamage`'s `flRatio *=`).
#[must_use]
pub fn armor_penetration(index: u8) -> f32 {
    if is_knife(index) {
        return 1.7;
    }
    // `TakeDamage`'s per-weapon `flRatio *=`.
    match cs_weapon(index).map(|w| w.name) {
        Some("ak47" | "galil") => 1.55,
        Some("m4a1" | "aug" | "famas" | "sg552") => 1.4,
        Some("awp") => 1.95,
        Some("g3sg1") => 1.65,
        Some("sg550") => 1.45,
        Some("m249") => 1.5,
        Some("deagle" | "fiveseven" | "p90") => 1.5,
        Some("glock" | "elite") => 1.05,
        Some("mac10") => 0.95,
        Some("p228") => 1.25,
        Some("scout") => 1.7,
        _ => 1.0,
    }
}

/// Damage after kevlar, and the armour left: `damage` on a covered hit with `armor` points,
/// from a weapon with `penetration` ([`armor_penetration`]), `blast` for explosions.
#[must_use]
pub fn armor_absorb(damage: f32, armor: f32, penetration: f32, blast: bool) -> (f32, f32) {
    if armor <= 0.0 {
        return (damage, 0.0);
    }
    let bonus = if blast { CS_ARMOR_BONUS * 2.0 } else { CS_ARMOR_BONUS };
    let kept = CS_ARMOR_RATIO * penetration * damage;
    let cost = (damage - kept) * bonus;
    if cost > armor {
        // The vest gives out: it stops what its points pay for, the rest goes through.
        (damage - armor / bonus, 0.0)
    } else {
        (kept, armor - cost.max(1.0))
    }
}

/// The name the HUD shows for CS weapon `index` (CS:S's names).
#[must_use]
pub fn display_name(index: u8) -> Option<&'static str> {
    if is_knife(index) {
        return Some("Knife");
    }
    if let Some(grenade) = cs_grenade(index) {
        return Some(match grenade.name {
            "hegrenade" => "HE Grenade",
            "flashbang" => "Flashbang",
            _ => "Smoke Grenade",
        });
    }
    Some(match cs_weapon(index)?.name {
        "ak47" => "AK-47",
        "m4a1" => "M4A1",
        "awp" => "AWP",
        "deagle" => "Desert Eagle",
        "usp" => "USP",
        "glock" => "Glock-18",
        "p228" => "P228",
        "fiveseven" => "Five-seveN",
        "elite" => "Dual Elites",
        "m3" => "M3",
        "xm1014" => "XM1014",
        "mac10" => "MAC-10",
        "tmp" => "TMP",
        "mp5" => "MP5",
        "ump45" => "UMP45",
        "p90" => "P90",
        "galil" => "Galil",
        "famas" => "FAMAS",
        "sg552" => "SG 552",
        "aug" => "AUG",
        "scout" => "Scout",
        "g3sg1" => "G3/SG-1",
        "sg550" => "SG 550",
        "m249" => "M249",
        other => other,
    })
}

/// Anything CS sells by its buy name (guns and grenades): (name, MW2 twin, price).
#[must_use]
pub fn cs_buyable(name: &str) -> Option<(&'static str, &'static str, i32)> {
    cs_buy_list().find(|(buy, _, _)| buy.eq_ignore_ascii_case(name))
}

/// Every buy name with its MW2 twin and price, guns first.
pub fn cs_buy_list() -> impl Iterator<Item = (&'static str, &'static str, i32)> {
    CS_WEAPONS
        .iter()
        .map(|w| (w.name, w.mw2_name, w.price))
        .chain(CS_GRENADES.iter().map(|g| (g.name, g.mw2_name, g.price)))
}

/// Which side may buy an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuyTeam {
    Both,
    Terrorists,
    CounterTerrorists,
}

/// Who sells `name` (a buy name): the AK-47, Galil, SG 552, MAC-10, G3/SG-1 and Dual Elites are
/// the Terrorists' alone, the M4A1, FAMAS, AUG, TMP, SG 550 and Five-seveN the
/// Counter-Terrorists' (CS 1.6 and CS:S).
#[must_use]
pub fn buy_team(name: &str) -> BuyTeam {
    match name {
        "ak47" | "galil" | "sg552" | "mac10" | "g3sg1" | "elite" => BuyTeam::Terrorists,
        "m4a1" | "famas" | "aug" | "tmp" | "sg550" | "fiveseven" | "defuser" => {
            BuyTeam::CounterTerrorists
        }
        _ => BuyTeam::Both,
    }
}

/// The buy name behind a CS buy command or alias: CS:S's `buy <name>` names, CS 1.6's aliases
/// (`fn57`, `elites`, `mp5`, `hegren`, `sgren`, `flash`, ...). `None` for what isn't sold here
/// (night vision, the shield, ammo).
#[must_use]
pub fn buy_alias(command: &str) -> Option<&'static str> {
    const ALIASES: [(&str, &str); 14] = [
        ("elites", "elite"),
        ("fn57", "fiveseven"),
        ("mp5navy", "mp5"),
        ("smg", "mp5"),
        ("nighthawk", "deagle"),
        ("km45", "usp"),
        ("9x19mm", "glock"),
        ("228compact", "p228"),
        ("flash", "flashbang"),
        ("hegren", "hegrenade"),
        ("sgren", "smokegrenade"),
        ("vest", "vest"),
        ("vesthelm", "vesthelm"),
        ("defuser", "defuser"),
    ];
    let command = command.trim();
    let name = command
        .get(..4)
        .filter(|head| head.eq_ignore_ascii_case("buy "))
        .map_or(command, |_| &command[4..])
        .trim();
    if let Some((_, buy)) = ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
    {
        return Some(buy);
    }
    cs_buyable(name).map(|(buy, _, _)| buy)
}

/// The price of kevlar (`vest`) or kevlar and helmet (`vesthelm`), or of any weapon by buy name.
#[must_use]
pub fn buy_price(name: &str) -> Option<i32> {
    match name {
        "vest" => Some(CS_KEVLAR_PRICE),
        "vesthelm" => Some(CS_KEVLAR_HELMET_PRICE),
        "defuser" => Some(CS_DEFUSE_KIT_PRICE),
        other => cs_buyable(other).map(|(_, _, price)| price),
    }
}

/// The CS weapon by buy name (`ak47`).
#[must_use]
pub fn cs_weapon_by_name(name: &str) -> Option<&'static CsWeapon> {
    CS_WEAPONS
        .iter()
        .find(|w| w.name.eq_ignore_ascii_case(name))
}

/// Rewrite an MW2 weapon's combat facts to play as `index`'s CS weapon. Returns the run-speed
/// scales (hip, zoomed) relative to [`CS_BASE_SPEED`].
pub fn apply_overrides(facts: &mut WeaponCombatFacts, index: u8) -> Option<(f32, f32)> {
    if is_knife(index) || is_c4(index) || cs_grenade(index).is_some() || cs_weapon(index).is_some()
    {
        apply_deploy(facts, deploy_seconds(index));
    }
    if is_c4(index) {
        // Carried at the knife's run speed; it never fires (attack plants it).
        facts.cs_weapon = index;
        facts.can_hold_breath = false;
        facts.aim_down_sight = false;
        let scale = CS_C4.max_speed / CS_BASE_SPEED;
        return Some((scale, scale));
    }
    if is_knife(index) {
        facts.cs_weapon = index;
        facts.can_hold_breath = false;
        facts.aim_down_sight = false;
        // Bodies hold it in one hand as MW2 holds a throwing knife, not as its pistol twin.
        facts.player_anim_type = crate::PLAYER_ANIM_TYPE_THROWINGKNIFE;
        facts.location_damage = CS_LOCATION_DAMAGE;
        let scale = CS_KNIFE.max_speed / CS_BASE_SPEED;
        return Some((scale, scale));
    }
    if let Some(grenade) = cs_grenade(index) {
        // Grenades in hand don't slow the run (250); the magazine counts them.
        facts.cs_weapon = index;
        facts.can_hold_breath = false;
        facts.aim_down_sight = false;
        // Bodies carry it upright as MW2 holds a throwing knife, not as its pistol or SMG twin
        // (the sim's anim conditions switch to MW2's grenade animations for the throw itself).
        facts.player_anim_type = crate::PLAYER_ANIM_TYPE_THROWINGKNIFE;
        facts.clip_size = grenade.carry;
        facts.start_ammo = 1;
        facts.max_ammo = 0;
        return Some((1.0, 1.0));
    }
    let weapon = cs_weapon(index)?;
    facts.cs_weapon = index;
    let ms = |seconds: f32| (seconds * 1000.0 + 0.5) as i32;
    // A bolt gun's cycle is the shot plus the MW2 rechamber animation that follows it.
    facts.fire_time_ms = if weapon.gated() {
        // The CS layer times these guns' shots itself (`cs_fire_gate_ms`); the MW2 machine
        // only has to be ready by the next tick.
        40
    } else if facts.bolt_action {
        (ms(weapon.cycle) - facts.rechamber_time_ms.max(0)).max(50)
    } else {
        ms(weapon.cycle)
    };
    if weapon.pellets > 1 {
        facts.weap_class = crate::WEAPCLASS_SPREAD;
        facts.shots_per_fire = weapon.pellets as i32;
    }
    if let Some((start, per_shell, finish)) = weapon.shell_reload {
        facts.reload_start_time_ms = ms(start);
        facts.reload_start_add_time_ms = ms(start);
        facts.reload_add_time_ms = ms(per_shell);
        facts.reload_empty_add_time_ms = ms(per_shell);
        facts.reload_end_time_ms = ms(finish);
    }
    facts.clip_size = weapon.clip;
    facts.start_ammo = weapon.clip + weapon.reserve;
    facts.max_ammo = weapon.reserve;
    // Each pass of a shotgun's reload loop is one shell and lasts the per-shell time, not the
    // whole reload (which would make every shell take seconds).
    let loop_seconds = weapon
        .shell_reload
        .map_or(weapon.reload, |(_, per_shell, _)| per_shell);
    facts.reload_time_ms = ms(loop_seconds);
    facts.reload_empty_time_ms = ms(loop_seconds);
    facts.damage = weapon.damage as i32;
    facts.min_damage = weapon.damage as i32;
    facts.max_damage_range = weapon.distance;
    facts.min_damage_range = weapon.distance;
    facts.fire_type = if weapon.semi_auto { 1 } else { 0 };
    facts.location_damage = CS_LOCATION_DAMAGE;
    // No hold-breath steadying: CS scopes do not sway, and SHIFT walks.
    facts.can_hold_breath = false;
    // No MW2 aim-down-sights: CS rifles and pistols have none, and snipers zoom with
    // [`next_zoom`] instead.
    facts.aim_down_sight = false;
    Some((
        weapon.max_speed / CS_BASE_SPEED,
        weapon.max_speed_zoomed / CS_BASE_SPEED,
    ))
}

/// CS 1.6 `DefaultDeploy`: the drawn weapon can attack 0.75 s later (`m_flNextAttack`); the AWP
/// (1.45 s) and Scout (1.25 s) take longer. The knife and grenades deploy like any gun.
#[must_use]
pub fn deploy_seconds(index: u8) -> f32 {
    match cs_weapon(index).map(|weapon| weapon.name) {
        Some("awp") => 1.45,
        Some("scout") => 1.25,
        _ => 0.75,
    }
}

/// CS weapon switching: the held weapon goes away at once (CS has no put-away time) and the new
/// one raises for its deploy time, quick (pistol) switches included.
fn apply_deploy(facts: &mut WeaponCombatFacts, seconds: f32) {
    let ms = (seconds * 1000.0 + 0.5) as i32;
    facts.drop_time_ms = 1;
    facts.quick_drop_time_ms = 1;
    facts.raise_time_ms = ms;
    facts.quick_raise_time_ms = ms;
}

/// Milliseconds between zoom steps, and before the first after the gun is drawn.
pub const CS_ZOOM_DELAY_MS: i32 = 300;
pub const CS_DEPLOY_ZOOM_DELAY_MS: i32 = 1000;

/// The zoom right click steps to from `current` (0 = unzoomed): the next level, or back to 0.
#[must_use]
pub fn next_zoom(weapon: &CsWeapon, current: u32) -> u32 {
    match weapon.zoom.iter().position(|&z| z == current) {
        Some(i) => weapon.zoom.get(i + 1).copied().unwrap_or(0),
        None if current == 0 => weapon.zoom.first().copied().unwrap_or(0),
        None => 0,
    }
}

/// CS damage `distance` units out: `damage * range_modifier ^ (distance / 500)`.
#[must_use]
pub fn damage_at_distance(weapon: &CsWeapon, distance: f32) -> f32 {
    if distance > weapon.distance {
        return 0.0;
    }
    weapon.damage * libm::powf(weapon.range_modifier, distance / 500.0)
}

/// The shooter's movement when a shot leaves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsShooter {
    pub on_ground: bool,
    pub ducked: bool,
    /// Horizontal speed.
    pub speed: f32,
    /// Fully zoomed (snipers).
    pub zoomed: bool,
}

/// Per-player CS weapon state, kept in `PlayerState` so prediction and the authority agree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsGunState {
    pub shots_fired: i32,
    pub accuracy: f32,
    /// Game time of the previous shot, ms; 0 before the first.
    pub last_fire_ms: i32,
    /// Lateral kick direction (`m_iDirection`).
    pub direction: i32,
}

/// Fire one shot: return the spread `FireBullets3` uses (from the accuracy the previous shot
/// left), then advance accuracy for the next one.
pub fn fire(weapon: &CsWeapon, state: &mut CsGunState, shooter: CsShooter, now_ms: i32) -> f32 {
    // Auto snipers work their accuracy out before the shot, from the time since the last one.
    if let CsAccuracy::SniperTime {
        scale,
        base,
        cap,
        first,
        ..
    } = weapon.accuracy
    {
        state.accuracy = if state.last_fire_ms != 0 {
            let since = (now_ms - state.last_fire_ms) as f32 / 1000.0;
            (since * scale + base).min(cap)
        } else {
            first
        };
    }
    let spread = spread(weapon, state.accuracy, shooter);
    state.shots_fired += 1;
    match weapon.accuracy {
        CsAccuracy::Sustained {
            divisor,
            integer,
            base,
            max,
            power,
            ..
        } => {
            // Retail divides as integers for most guns, so the first bullets keep `base`.
            let shots = state.shots_fired.min(1000);
            let powered = if power == 2 { shots * shots } else { shots * shots * shots };
            let grown = if integer {
                (powered / divisor as i32) as f32
            } else {
                powered as f32 / divisor
            };
            state.accuracy = (grown + base).min(max);
        }
        CsAccuracy::SniperTime { .. } => {}
        CsAccuracy::Pistol {
            recover,
            factor,
            min,
            max,
            ..
        } => {
            if state.last_fire_ms != 0 {
                let since = (now_ms - state.last_fire_ms) as f32 / 1000.0;
                state.accuracy = (state.accuracy - (recover - since) * factor).clamp(min, max);
            }
        }
        CsAccuracy::None => {}
    }
    state.last_fire_ms = now_ms;
    spread
}

/// Spread for the shot as the weapon's `PrimaryAttack` picks it.
#[must_use]
pub fn spread(weapon: &CsWeapon, accuracy: f32, shooter: CsShooter) -> f32 {
    match weapon.spread {
        CsSpread::Rifle { air, run, still } => {
            let (base, scale) = if !shooter.on_ground {
                air
            } else if shooter.speed > weapon.run_speed {
                run
            } else {
                still
            };
            base + scale * accuracy
        }
        CsSpread::AutoSniper {
            air,
            moving,
            ducked,
            standing,
            unscoped,
            moving_scaled,
        } => {
            let base = if !shooter.on_ground {
                air
            } else if shooter.speed > 0.0 {
                moving
            } else if shooter.ducked {
                ducked
            } else {
                standing
            };
            let moving_whole = shooter.on_ground && shooter.speed > 0.0 && !moving_scaled;
            let base = if shooter.zoomed { base } else { base + unscoped };
            if moving_whole {
                base
            } else {
                (1.0 - accuracy) * base
            }
        }
        CsSpread::Cone(cone) => cone,
        CsSpread::Pistol {
            air,
            moving,
            ducked,
            standing,
        } => {
            let factor = if !shooter.on_ground {
                air
            } else if shooter.speed > 0.0 {
                moving
            } else if shooter.ducked {
                ducked
            } else {
                standing
            };
            factor * (1.0 - accuracy)
        }
        CsSpread::Sniper {
            air,
            run,
            walk,
            ducked,
            standing,
            unscoped,
        } => {
            let base = if !shooter.on_ground {
                air
            } else if shooter.speed > weapon.run_speed {
                run
            } else if shooter.speed > 10.0 {
                walk
            } else if shooter.ducked {
                ducked
            } else {
                standing
            };
            if shooter.zoomed {
                base
            } else {
                base + unscoped
            }
        }
    }
}

/// `KickBack` / the pistol punch after a shot: add recoil to `punch` (pitch, yaw, roll).
/// `flip_roll` is a uniform roll in `0..=direction_change` deciding whether the lateral kick
/// changes side, as `RANDOM_LONG(0, direction_change) == 0` does.
pub fn recoil(
    weapon: &CsWeapon,
    state: &mut CsGunState,
    shooter: CsShooter,
    punch: &mut [f32; 3],
    flip_roll: u32,
) {
    // Two uniform values in 0..1 from the roll, for the random kicks.
    let unit = |shift: u32| ((flip_roll >> shift) & 0xffff) as f32 / 65535.0;
    let kick = match weapon.recoil {
        CsRecoil::Punch(up) => {
            punch[0] -= up;
            return;
        }
        CsRecoil::AutoSniper { up, side } => {
            let random_up = up.0 + (up.1 - up.0) * unit(0);
            punch[0] -= random_up + punch[0] * 0.25;
            punch[1] += (unit(16) * 2.0 - 1.0) * side;
            return;
        }
        CsRecoil::Shotgun { ground, air } => {
            let (low, high) = if shooter.on_ground { ground } else { air };
            let span = high.saturating_sub(low) + 1;
            punch[0] -= (low + (flip_roll >> 3) % span) as f32;
            return;
        }
        CsRecoil::KickBack {
            moving,
            air,
            ducked,
            standing,
        } => {
            if shooter.speed > 0.0 {
                moving
            } else if !shooter.on_ground {
                air
            } else if shooter.ducked {
                ducked
            } else {
                standing
            }
        }
    };
    let shots = state.shots_fired as f32;
    let (up, lateral) = if state.shots_fired <= 1 {
        (kick.up_base, kick.lateral_base)
    } else {
        (
            shots * kick.up_modifier + kick.up_base,
            shots * kick.lateral_modifier + kick.lateral_base,
        )
    };
    punch[0] = (punch[0] - up).max(-kick.up_max);
    if state.direction == 1 {
        punch[1] = (punch[1] + lateral).min(kick.lateral_max);
    } else {
        punch[1] = (punch[1] - lateral).max(-kick.lateral_max);
    }
    if flip_roll % (kick.direction_change + 1) == 0 {
        state.direction = 1 - state.direction;
    }
}

/// `PM_DropPunchAngle`: the punch eases back to centre.
pub fn drop_punch(punch: &mut [f32; 3], frametime: f32) {
    let len = libm::sqrtf(punch[0] * punch[0] + punch[1] * punch[1] + punch[2] * punch[2]);
    if len <= 0.0 {
        return;
    }
    let next = (len - (10.0 + len * 0.5) * frametime).max(0.0);
    let scale = next / len;
    for axis in punch.iter_mut() {
        *axis *= scale;
    }
}

/// `FireBullets3` direction: forward plus a triangular random offset of `spread` along right and
/// up. `rolls` are four uniform values in `0..1`.
#[must_use]
pub fn bullet_direction(
    forward: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    spread: f32,
    rolls: [f32; 4],
) -> [f32; 3] {
    let x = (rolls[0] - 0.5) + (rolls[1] - 0.5);
    let y = (rolls[2] - 0.5) + (rolls[3] - 0.5);
    let mut dir = [0.0; 3];
    for axis in 0..3 {
        dir[axis] = forward[axis] + x * spread * right[axis] + y * spread * up[axis];
    }
    let len = libm::sqrtf(dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]);
    if len > 0.0 {
        for axis in &mut dir {
            *axis /= len;
        }
    }
    dir
}

/// What the weapon does between shots, per command (`ItemPostFrame` with no fire button):
/// pistols forget shots fired when the trigger is released; automatics shed one shot every
/// 22.5 ms after a 0.4 s pause, capped at 15 first. `delay_fire` is set by every shot.
pub fn post_frame(
    weapon: &CsWeapon,
    state: &mut CsGunState,
    attack_held: bool,
    now_ms: i32,
    decrease_at_ms: &mut i32,
    delay_fire: &mut bool,
) {
    if attack_held {
        return;
    }
    if *delay_fire {
        *delay_fire = false;
        state.shots_fired = state.shots_fired.min(15);
        *decrease_at_ms = now_ms + 400;
    }
    if weapon.pistol {
        state.shots_fired = 0;
    } else if state.shots_fired > 0 && *decrease_at_ms < now_ms {
        *decrease_at_ms = now_ms + 22;
        state.shots_fired -= 1;
    }
}

/// Accuracy a weapon has when drawn or reloaded.
#[must_use]
pub fn initial_accuracy(weapon: &CsWeapon) -> f32 {
    match weapon.accuracy {
        CsAccuracy::Sustained { initial, .. } => initial,
        CsAccuracy::SniperTime { initial, .. } => initial,
        CsAccuracy::Pistol { initial, .. } => initial,
        CsAccuracy::None => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STANDING: CsShooter = CsShooter {
        on_ground: true,
        ducked: false,
        speed: 0.0,
        zoomed: false,
    };

    fn weapon(name: &str) -> &'static CsWeapon {
        cs_weapon_by_name(name).expect("weapon")
    }

    fn fresh(weapon: &CsWeapon) -> CsGunState {
        CsGunState {
            shots_fired: 0,
            accuracy: initial_accuracy(weapon),
            last_fire_ms: 0,
            direction: 0,
        }
    }

    #[test]
    fn mw2_twins_map_both_ways() {
        for (i, w) in CS_WEAPONS.iter().enumerate() {
            let index = cs_weapon_index_for(w.mw2_name).expect("mapped");
            assert_eq!(usize::from(index), i + 1);
            assert_eq!(cs_weapon(index), Some(w));
        }
        assert_eq!(cs_weapon_index_for("iw4:weapon/ak47_mp"), Some(1));
        assert_eq!(cs_weapon_index_for("ak47_acog_mp"), None);
        assert_eq!(cs_weapon(0), None);
        assert_eq!(cs_weapon_index_for(CS_KNIFE.mw2_name), Some(CS_KNIFE_INDEX));
        assert_eq!(cs_weapon(CS_KNIFE_INDEX), None);
    }

    #[test]
    fn knife_stab_wins_and_waits_its_own_timer() {
        assert_eq!(
            knife_attack(1000, 0, 0, true, true),
            Some(KnifeAttack::Stab)
        );
        assert_eq!(
            knife_attack(1000, 0, 1500, true, true),
            Some(KnifeAttack::Slash)
        );
        assert_eq!(knife_attack(1000, 1200, 1500, true, true), None);
        assert_eq!(knife_attack(1000, 0, 0, false, false), None);
        assert_eq!(
            knife_delays(&CS_KNIFE, KnifeAttack::Slash, false),
            (0.35, 0.5)
        );
        assert_eq!(knife_delays(&CS_KNIFE, KnifeAttack::Stab, true), (1.1, 1.1));
    }

    #[test]
    fn grenades_take_slot_four_and_throw_like_cs() {
        for (i, g) in CS_GRENADES.iter().enumerate() {
            let index = cs_weapon_index_for(g.mw2_name).expect("grenade twin");
            assert_eq!(usize::from(index - CS_GRENADE_INDEX), i);
            assert_eq!(cs_grenade(index), Some(g));
            assert_eq!(cs_weapon(index), None);
            let mut facts = WeaponCombatFacts::none();
            assert_eq!(apply_overrides(&mut facts, index), Some((1.0, 1.0)));
            assert_eq!(slot_of(&facts), 4);
            assert_eq!(facts.clip_size, g.carry);
        }
        assert!(!is_grenade(CS_KNIFE_INDEX));
        // CS:GO: a level throw is lifted 10 degrees, half way down 5, straight down none; the
        // speed is 675 whatever the pitch (a wrapped 350 is 10 up).
        assert_eq!(grenade_throw(0.0, 1.0), (-10.0, 675.0));
        assert_eq!(grenade_throw(45.0, 1.0), (40.0, 675.0));
        assert_eq!(grenade_throw(90.0, 1.0).0, 90.0);
        let (pitch, _) = grenade_throw(350.0, 1.0);
        assert!((pitch - (-10.0 - 10.0 * 80.0 / 90.0)).abs() < 1e-4);
        // Right click lobs at 30% from 12 lower; both buttons half way between.
        assert!((grenade_throw(0.0, 0.0).1 - 202.5).abs() < 1e-3);
        assert!((grenade_throw(0.0, 0.5).1 - 438.75).abs() < 1e-3);
        assert_eq!(
            (grenade_throw_lower(0.0), grenade_throw_lower(1.0)),
            (12.0, 0.0)
        );
    }

    #[test]
    fn grenade_strength_follows_the_held_buttons() {
        assert_eq!(grenade_strength_at_pull(false), 1.0);
        assert_eq!(grenade_strength_at_pull(true), 0.0);
        // Both held from a lob: 1.3 a second toward the middle, and no further.
        assert!((grenade_strength(0.0, true, true, 100) - 0.13).abs() < 1e-6);
        assert_eq!(grenade_strength(0.0, true, true, 1000), 0.5);
        assert_eq!(grenade_strength(1.0, true, true, 1000), 0.5);
        assert_eq!(grenade_strength(0.5, false, true, 1000), 0.0);
        assert_eq!(grenade_strength(0.5, true, false, 1000), 1.0);
    }

    #[test]
    fn grenades_bounce_like_csgo() {
        let up = [0.0, 0.0, 1.0];
        // A wall hit keeps 45%, reflected.
        let (v, rests) = grenade_bounce([400.0, 0.0, 0.0], [-1.0, 0.0, 0.0], false);
        assert!(!rests && (v[0] + 180.0).abs() < 1e-3);
        // Off a player only 13.5%, and never rests on one.
        let (v, rests) = grenade_bounce([0.0, 0.0, -100.0], up, true);
        assert!(!rests && (v[2] - 13.5).abs() < 1e-3);
        // A slow landing rests; a slow touch on a wall doesn't.
        assert_eq!(
            grenade_bounce([30.0, 0.0, -20.0], up, false),
            ([0.0; 3], true)
        );
        assert!(!grenade_bounce([0.0, 30.0, 0.0], [1.0, 0.0, 0.0], false).1);
        // A fast steep first toss loses more than the 45%: 900 down comes back at 405 * 0.5.
        let (v, _) = grenade_bounce([0.0, 0.0, -900.0], up, false);
        assert!((v[2] - 202.5).abs() < 1e-3);
        // A fast glancing landing keeps its 45%.
        let (v, _) = grenade_bounce([900.0, 0.0, -100.0], up, false);
        assert!((v[0] - 405.0).abs() < 1e-3 && (v[2] - 45.0).abs() < 1e-3);
    }

    #[test]
    fn kevlar_soaks_like_cs() {
        // AK body shot into full kevlar: 36 * 0.775 = 27.9 through, vest pays 4.05.
        let (through, left) = armor_absorb(36.0, 100.0, armor_penetration(1), false);
        assert!((through - 27.9).abs() < 1e-3 && (left - 95.95).abs() < 1e-3);
        // USP into kevlar: half goes through.
        let usp = cs_weapon_index_for("usp_mp").expect("usp");
        assert_eq!(armor_absorb(34.0, 100.0, armor_penetration(usp), false).0, 17.0);
        // A worn-out vest stops only what its points pay for.
        let (through, left) = armor_absorb(100.0, 10.0, 1.0, false);
        assert_eq!((through, left), (80.0, 0.0));
        // HE at 100 into full kevlar: 50 through, blast bonus makes the vest pay 50.
        assert_eq!(armor_absorb(100.0, 100.0, 1.0, true), (50.0, 50.0));
        assert_eq!(armor_absorb(50.0, 0.0, 1.55, false), (50.0, 0.0));
    }

    #[test]
    fn flashbangs_follow_cs_facing_and_distance() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        // Point blank, looking at it: hold 3.75 s, fade 7.5 s; half way out, half of that.
        let (hold, fade) = flash_for(0.0, 1.0, 1.0).expect("facing");
        assert!(near(hold, 3.75) && near(fade, 7.5));
        let (hold, fade) = flash_for(1500.0, 1.0, 1.0).expect("half");
        assert!(near(hold, 1.875) && near(fade, 3.75));
        // To the side, further to the side, facing away; out of range or unseen: nothing.
        let fade = |facing| flash_for(0.0, facing, 1.0).expect("in range").1;
        assert!(near(fade(0.4), 5.25) && near(fade(0.0), 3.0) && near(fade(-0.8), 1.5));
        assert!(flash_for(3000.0, 1.0, 1.0).is_none());
        assert!(flash_for(0.0, 1.0, 0.0).is_none());
        // Round a corner, one bent line through: a sixth of it.
        let (_, fade) = flash_for(0.0, 1.0, CS_FLASH_PARTIAL).expect("corner");
        assert!(near(fade, 7.5 * 0.167));
        // The screen lasts fade / 1.4: 7.5 s of fade is 5.357 s.
        let (full, fresh) = blind(7.5, None);
        assert!(fresh && full.duration_ms == 5357 && full.end_ms == 5357 && full.alpha == 255);
        // It whites up over 94 ms, holds full white until 3 s are left, then clears on the
        // square: 1.5 s left is a quarter white, the frozen frame still 1.5 / 5.357.
        let (white, frame) = flash_screen(full, 47).expect("building up");
        assert!(near(white, 0.4977) && white == frame);
        assert_eq!(flash_screen(full, 2000).map(|s| s.0), Some(1.0));
        let (white, frame) = flash_screen(full, 5357 - 1500).expect("clearing");
        assert!(near(white, 0.25) && near(frame, 1500.0 / 5357.0));
        assert!(flash_screen(full, 5357).is_none());
        // A weaker flash 1 s in only carries the old one on; a longer one stretches it to its
        // own length from now. Neither starts afresh. Once the old one is over, a new one does.
        let (weak, fresh) = blind(1.4, Some((full, 1000)));
        assert!(!fresh && weak.end_ms == 5357 && weak.duration_ms == 4357);
        let (strong, _) = blind(14.0, Some((full, 1000)));
        assert!(strong.end_ms == 11_000 && strong.duration_ms == 10_000);
        assert!(blind(1.4, Some((full, 6000))).1);
    }

    #[test]
    fn knife_damage_matches_retail() {
        assert_eq!(knife_damage(&CS_KNIFE, KnifeAttack::Slash, true), 15.0);
        assert_eq!(knife_damage(&CS_KNIFE, KnifeAttack::Stab, false), 65.0);
        assert_eq!(knife_damage(&CS_KNIFE, KnifeAttack::Stab, true), 195.0);
        // Victim at the origin facing +x; the attacker behind (at -x) stabs a back.
        assert!(is_backstab([-40.0, 0.0, 0.0], [0.0; 3], 0.0));
        assert!(is_backstab([-40.0, 10.0, 0.0], [0.0; 3], 0.0));
        assert!(!is_backstab([40.0, 0.0, 0.0], [0.0; 3], 0.0));
        assert!(!is_backstab([0.0, -40.0, 0.0], [0.0; 3], 0.0));
        // A stab to the head is 260: lethal even through armour.
        assert!(knife_damage(&CS_KNIFE, KnifeAttack::Stab, false) * CS_LOCATION_DAMAGE[2] > 200.0);
        let mut facts = WeaponCombatFacts::none();
        assert_eq!(
            apply_overrides(&mut facts, CS_KNIFE_INDEX),
            Some((1.0, 1.0))
        );
        assert_eq!(slot_of(&facts), 3);
    }

    #[test]
    fn weapons_deploy_like_cs() {
        let deploy = |index: u8| {
            let mut facts = WeaponCombatFacts::none();
            apply_overrides(&mut facts, index).expect("cs weapon");
            (facts.drop_time_ms, facts.raise_time_ms, facts.quick_raise_time_ms)
        };
        let index = |name: &str| {
            CS_WEAPONS.iter().position(|w| w.name == name).expect("weapon") as u8 + 1
        };
        assert_eq!(deploy(index("ak47")), (1, 750, 750));
        assert_eq!(deploy(index("deagle")), (1, 750, 750));
        assert_eq!(deploy(index("awp")), (1, 1450, 1450));
        assert_eq!(deploy(index("scout")), (1, 1250, 1250));
        assert_eq!(deploy(CS_KNIFE_INDEX), (1, 750, 750));
        assert_eq!(deploy(CS_GRENADE_INDEX), (1, 750, 750));
    }

    #[test]
    fn damage_falls_off_per_500_units() {
        let ak = weapon("ak47");
        assert!((damage_at_distance(ak, 0.0) - 36.0).abs() < 1e-4);
        assert!((damage_at_distance(ak, 500.0) - 36.0 * 0.98).abs() < 1e-3);
        let deagle = weapon("deagle");
        assert!((damage_at_distance(deagle, 1000.0) - 54.0 * 0.81 * 0.81).abs() < 1e-3);
        assert_eq!(damage_at_distance(deagle, 5000.0), 0.0);
        // An AK head hit at close range is lethal without armour.
        assert!(damage_at_distance(ak, 100.0) * CS_LOCATION_DAMAGE[2] > 100.0);
    }

    #[test]
    fn silencers_change_damage_range_and_gunshot() {
        // Only the M4A1 and USP carry one; every other gun is unchanged by it.
        for name in ["ak47", "awp", "deagle", "glock"] {
            let gun = weapon(name);
            assert!(gun.silencer.is_none(), "{name}");
            assert_eq!(gun.with_silencer(true), *gun, "{name}");
        }
        let (m4, usp) = (weapon("m4a1"), weapon("usp"));
        let (m4_sil, usp_sil) = (m4.with_silencer(true), usp.with_silencer(true));
        assert_eq!(m4.with_silencer(false), *m4);
        // M4A1 hits harder but loses range; the USP loses damage and range.
        assert!((damage_at_distance(m4, 0.0) - 32.0).abs() < 1e-4);
        assert!((damage_at_distance(&m4_sil, 0.0) - 33.0).abs() < 1e-4);
        assert!(damage_at_distance(&m4_sil, 1000.0) < damage_at_distance(m4, 1000.0));
        assert!((damage_at_distance(usp, 0.0) - 34.0).abs() < 1e-4);
        assert!((damage_at_distance(&usp_sil, 0.0) - 30.0).abs() < 1e-4);
        // The silenced gunshot differs, and the silencer bit is one per weapon index.
        assert_ne!(m4_sil.css_fire_sound, m4.css_fire_sound);
        assert_ne!(usp_sil.css_fire_sound, usp.css_fire_sound);
        assert_ne!(silencer_bit(1), silencer_bit(2));
        assert_eq!(SILENCED_SHOT_FLAG & 1, 0, "bit 0 is the impact events' penetrated flag");
    }

    #[test]
    fn the_table_is_consistent() {
        // One bit per gun in the per-weapon flags (`silencer_bit`), unique names and twins.
        assert!(CS_WEAPONS.len() < 31);
        for (i, a) in CS_WEAPONS.iter().enumerate() {
            for b in &CS_WEAPONS[i + 1..] {
                assert_ne!(a.name, b.name);
                assert_ne!(a.mw2_name, b.mw2_name, "{} and {} share a twin", a.name, b.name);
            }
            assert_ne!(a.mw2_name, CS_KNIFE.mw2_name, "{}", a.name);
            assert!(CS_GRENADES.iter().all(|g| g.mw2_name != a.mw2_name), "{}", a.name);
            assert!(a.clip > 0 && a.damage > 0.0 && a.cycle > 0.0, "{}", a.name);
            assert!(!a.fire_sounds.is_empty() && !a.css_view_model.is_empty(), "{}", a.name);
            // Scoped guns have levels; a gun that drops its zoom per shot must have one.
            assert!(a.zoom.is_empty() || a.zoom.iter().all(|z| *z < 90), "{}", a.name);
            assert_eq!(a.gated(), a.burst.is_some() || a.cycle_zoomed > 0.0);
        }
        assert!(CS_KNIFE_INDEX as usize <= 31);
    }

    #[test]
    fn smgs_square_the_shots_and_the_mp5_divides_as_a_float() {
        let (mac, mp5) = (weapon("mac10"), weapon("mp5"));
        let mut state = fresh(mac);
        assert!((state.accuracy - 0.15).abs() < 1e-6);
        // `shots^3 / 200` is 0 for the first 5 bullets (integer division), so the base stays.
        for _ in 0..5 {
            fire(mac, &mut state, STANDING, 1000);
        }
        assert!((state.accuracy - 0.6).abs() < 1e-5);
        let mut state = fresh(mp5);
        assert_eq!(state.accuracy, 0.0);
        fire(mp5, &mut state, STANDING, 1000);
        // 1 shot: 1 / 220.1 + 0.45 (float division), not the integer 0 + 0.45.
        assert!((state.accuracy - (1.0 / 220.1 + 0.45)).abs() < 1e-5);
        for _ in 0..40 {
            fire(mp5, &mut state, STANDING, 1000);
        }
        assert!((state.accuracy - 0.75).abs() < 1e-6, "capped at 0.75");
    }

    #[test]
    fn burst_changes_damage_range_and_keeps_later_bullets_steady() {
        let famas = weapon("famas");
        let burst = famas.with_burst(1);
        assert_eq!(famas.with_burst(0), *famas);
        assert!((damage_at_distance(famas, 0.0) - 30.0).abs() < 1e-4);
        assert!((damage_at_distance(&burst, 0.0) - 34.0).abs() < 1e-4);
        // Full auto adds 0.01 to a still FAMAS's spread; burst does not.
        let auto = spread(famas, 0.5, STANDING);
        let first = spread(&burst, 0.5, STANDING);
        assert!((auto - first - 0.01).abs() < 1e-6);
        let glock = weapon("glock");
        let mode = glock.burst.expect("burst");
        assert_eq!(mode.follow_spread, Some(0.05));
        assert!((damage_at_distance(&glock.with_burst(2), 500.0) - 25.0 * 0.9).abs() < 1e-4);
        assert!((damage_at_distance(&glock.with_burst(1), 500.0) - 25.0 * 0.75).abs() < 1e-4);
        // Only these two have a burst mode, and the CS layer times them.
        assert_eq!(CS_WEAPONS.iter().filter(|w| w.burst.is_some()).count(), 2);
        assert!(famas.gated() && glock.gated() && !weapon("ak47").gated());
    }

    #[test]
    fn auto_snipers_recover_with_time_and_scoped_rifles_zoom_to_55() {
        let g3 = weapon("g3sg1");
        let mut state = fresh(g3);
        // No last shot: 0.98 straight away, so the first scoped shot is nearly perfect.
        let scoped = CsShooter { zoomed: true, ..STANDING };
        let first = fire(g3, &mut state, scoped, 5000);
        assert!((state.accuracy - 0.98).abs() < 1e-6);
        assert!((first - 0.02 * 0.055).abs() < 1e-6);
        // Firing again 100 ms later: 0.1 * 0.3 + 0.55 = 0.58, so the spread opens up.
        let second = fire(g3, &mut state, scoped, 5100);
        assert!((state.accuracy - 0.58).abs() < 1e-5);
        assert!((second - 0.42 * 0.055).abs() < 1e-5);
        // Unscoped adds 0.025 before the accuracy takes its share.
        assert!(spread(g3, 0.98, STANDING) > spread(g3, 0.98, scoped));
        for name in ["aug", "sg552"] {
            let rifle = weapon(name);
            assert_eq!(rifle.zoom, &[55]);
            assert!(!rifle.unzoom_on_fire && !rifle.scope_overlay);
            assert!(rifle.cycle_zoomed > rifle.cycle);
            assert_eq!(next_zoom(rifle, 0), 55);
            assert_eq!(next_zoom(rifle, 55), 0);
        }
        for name in ["awp", "scout", "g3sg1", "sg550"] {
            assert!(weapon(name).scope_overlay, "{name}");
        }
        assert!(weapon("awp").unzoom_on_fire && weapon("scout").unzoom_on_fire);
        assert!(!g3.unzoom_on_fire && !weapon("sg550").unzoom_on_fire);
    }

    #[test]
    fn shotguns_fire_pellets_and_kick_by_ground_or_air() {
        for (name, pellets, damage) in [("m3", 9, 20.0), ("xm1014", 6, 20.0)] {
            let gun = weapon(name);
            assert_eq!(gun.pellets, pellets);
            assert!(gun.shell_reload.is_some());
            assert!((damage_at_distance(gun, 1000.0) - damage).abs() < 1e-4, "no falloff");
        }
        let m3 = weapon("m3");
        let mut state = fresh(m3);
        let mut punch = [0.0; 3];
        recoil(m3, &mut state, STANDING, &mut punch, 12345);
        assert!((-6.0..=-4.0).contains(&punch[0]), "{punch:?}");
        let mut punch = [0.0; 3];
        let air = CsShooter { on_ground: false, ..STANDING };
        recoil(m3, &mut state, air, &mut punch, 12345);
        assert!((-11.0..=-8.0).contains(&punch[0]), "{punch:?}");
        assert!((spread(m3, 1.0, STANDING) - 0.0675).abs() < 1e-6);
    }

    #[test]
    fn new_guns_use_the_right_armor_ratio() {
        let index = |name: &str| {
            u8::try_from(CS_WEAPONS.iter().position(|w| w.name == name).unwrap() + 1).unwrap()
        };
        assert_eq!(armor_penetration(index("scout")), 1.7);
        assert_eq!(armor_penetration(index("g3sg1")), 1.65);
        assert_eq!(armor_penetration(index("mac10")), 0.95);
        assert_eq!(armor_penetration(index("galil")), 1.55);
        assert_eq!(armor_penetration(index("mp5")), 1.0);
    }

    #[test]
    fn ak_spray_is_tight_for_five_bullets_then_opens() {
        let ak = weapon("ak47");
        let mut state = fresh(ak);
        let spreads: [f32; 7] =
            core::array::from_fn(|i| fire(ak, &mut state, STANDING, 100 * i as i32 + 100));
        assert!(
            (spreads[0] - 0.0275 * 0.2).abs() < 1e-6,
            "first shot {}",
            spreads[0]
        );
        for spread in &spreads[1..6] {
            assert!((spread - 0.0275 * 0.35).abs() < 1e-6, "{spread}");
        }
        assert!((spreads[6] - 0.0275 * 1.25).abs() < 1e-6, "{}", spreads[6]);
    }

    #[test]
    fn rifle_spread_follows_movement() {
        let ak = weapon("ak47");
        let air = CsShooter {
            on_ground: false,
            ..STANDING
        };
        let running = CsShooter {
            speed: 250.0,
            ..STANDING
        };
        let walking = CsShooter {
            speed: 130.0,
            ..STANDING
        };
        assert!((spread(ak, 0.35, air) - (0.04 + 0.4 * 0.35)).abs() < 1e-6);
        assert!((spread(ak, 0.35, running) - (0.04 + 0.07 * 0.35)).abs() < 1e-6);
        assert!((spread(ak, 0.35, walking) - 0.0275 * 0.35).abs() < 1e-6);
    }

    #[test]
    fn pistol_accuracy_punishes_spam_and_recovers() {
        let deagle = weapon("deagle");
        let mut state = fresh(deagle);
        let first = fire(deagle, &mut state, STANDING, 1000);
        assert!((first - 0.13 * (1.0 - 0.9)).abs() < 1e-6);
        fire(deagle, &mut state, STANDING, 1225);
        assert!(state.accuracy < 0.9);
        let spammed = state.accuracy;
        fire(deagle, &mut state, STANDING, 3000);
        assert!(state.accuracy > spammed);
        assert!((state.accuracy - 0.9).abs() < 1e-6);
    }

    #[test]
    fn awp_needs_the_scope() {
        let awp = weapon("awp");
        let scoped = CsShooter {
            zoomed: true,
            ..STANDING
        };
        assert!((spread(awp, 0.0, scoped) - 0.001).abs() < 1e-6);
        assert!((spread(awp, 0.0, STANDING) - 0.081).abs() < 1e-6);
        let ducked = CsShooter {
            ducked: true,
            ..scoped
        };
        assert_eq!(spread(awp, 0.0, ducked), 0.0);
    }

    #[test]
    fn kickback_climbs_to_the_cap_and_swaps_sides() {
        let ak = weapon("ak47");
        let mut state = fresh(ak);
        let mut punch = [0.0; 3];
        let mut flips = 0;
        for shot in 0..30 {
            fire(ak, &mut state, STANDING, shot * 96);
            let before = state.direction;
            recoil(ak, &mut state, STANDING, &mut punch, shot as u32);
            if state.direction != before {
                flips += 1;
            }
            assert!(punch[0] >= -5.75 - 1e-4, "pitch {}", punch[0]);
            assert!(punch[1].abs() <= 1.75 + 1e-4, "yaw {}", punch[1]);
        }
        assert!((punch[0] + 5.75).abs() < 1e-4);
        assert!(flips > 0);
    }

    #[test]
    fn punch_eases_back_to_centre() {
        let mut punch = [-5.75, 1.5, 0.0];
        let mut ticks = 0;
        while punch.iter().any(|axis| *axis != 0.0) && ticks < 200 {
            drop_punch(&mut punch, 0.01);
            ticks += 1;
        }
        // (10 + len/2) deg/s: about half a second from a full spray.
        assert!((30..=60).contains(&ticks), "{ticks}");
    }

    #[test]
    fn released_trigger_lets_the_spray_settle() {
        let ak = weapon("ak47");
        let mut state = fresh(ak);
        for shot in 0..20 {
            fire(ak, &mut state, STANDING, shot * 96);
        }
        let (mut decrease_at, mut delay) = (0, true);
        let mut now = 2000;
        post_frame(ak, &mut state, false, now, &mut decrease_at, &mut delay);
        assert_eq!(state.shots_fired, 15);
        assert!(!delay);
        while state.shots_fired > 0 && now < 3000 {
            now += 10;
            post_frame(ak, &mut state, false, now, &mut decrease_at, &mut delay);
        }
        assert_eq!(state.shots_fired, 0);
        // 0.4 s pause, then one shot per 22.5 ms, which a 100 Hz tick sees every 30 ms.
        assert!((2800..=2850).contains(&now), "settled at {now}");

        let usp = weapon("usp");
        let mut state = fresh(usp);
        fire(usp, &mut state, STANDING, 100);
        post_frame(usp, &mut state, false, 110, &mut decrease_at, &mut delay);
        assert_eq!(state.shots_fired, 0);
    }

    #[test]
    fn overrides_carry_cs_numbers() {
        let mut facts = WeaponCombatFacts::none();
        facts.aim_down_sight = true;
        let index = cs_weapon_index_for("ak47_mp").expect("ak");
        let (hip, zoomed) = apply_overrides(&mut facts, index).expect("applied");
        assert_eq!(facts.cs_weapon, index);
        assert_eq!(facts.fire_time_ms, 96);
        assert_eq!(facts.clip_size, 30);
        assert_eq!(facts.start_ammo, 120);
        assert_eq!(facts.fire_type, 0);
        assert!(!facts.aim_down_sight);
        assert!((hip - 221.0 / 250.0).abs() < 1e-6 && hip == zoomed);

        let mut awp = WeaponCombatFacts::none();
        awp.bolt_action = true;
        awp.rechamber_time_ms = 1000;
        awp.aim_down_sight = true;
        let awp_index = cs_weapon_index_for("cheytac_mp").expect("awp");
        let (_, zoomed) = apply_overrides(&mut awp, awp_index).expect("applied");
        assert_eq!(awp.fire_time_ms, 450);
        assert!(!awp.aim_down_sight);
        assert!((zoomed - 0.6).abs() < 1e-6);
    }

    #[test]
    fn shotgun_reloads_a_shell_per_pass_and_gated_guns_get_a_short_fire_time() {
        // Every pass of the reload loop is one shell, so it lasts the per-shell time (it once
        // took the whole 3 s reload per shell).
        for (twin, start, per_shell, finish) in [("spas12_mp", 550, 450, 450), ("m1014_mp", 550, 300, 400)] {
            let mut facts = WeaponCombatFacts::none();
            let index = cs_weapon_index_for(twin).expect("shotgun");
            apply_overrides(&mut facts, index).expect("applied");
            assert_eq!(facts.reload_time_ms, per_shell, "{twin}");
            assert_eq!(facts.reload_empty_time_ms, per_shell, "{twin}");
            assert_eq!(facts.reload_add_time_ms, per_shell, "{twin}");
            assert_eq!(facts.reload_start_time_ms, start, "{twin}");
            assert_eq!(facts.reload_end_time_ms, finish, "{twin}");
            assert!(facts.shots_per_fire >= 6);
        }
        // Burst guns are timed by the CS layer; the MW2 machine only has to be ready each tick.
        for twin in ["glock_mp", "famas_mp", "aug_mp", "fn2000_mp"] {
            let mut facts = WeaponCombatFacts::none();
            apply_overrides(&mut facts, cs_weapon_index_for(twin).expect("gun")).expect("applied");
            assert_eq!(facts.fire_time_ms, 40, "{twin}");
        }
        // A normal gun keeps its real cycle, and the burst gunshots are CS:S entries that exist.
        assert!(CS_WEAPONS.iter().flat_map(|w| w.burst).all(|b| b.css_fire_sound.ends_with(".Single")));
    }

    #[test]
    fn awp_zoom_steps_40_then_10_then_off() {
        let awp = weapon("awp");
        assert_eq!(next_zoom(awp, 0), 40);
        assert_eq!(next_zoom(awp, 40), 10);
        assert_eq!(next_zoom(awp, 10), 0);
        let ak = weapon("ak47");
        assert_eq!(next_zoom(ak, 0), 0);
    }
}
