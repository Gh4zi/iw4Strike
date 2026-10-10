pub const PLAYER_ANIM_TYPE_NAMES: &[&str] = &[
    "none",
    "other",
    "pistol",
    "smg",
    "autorifle",
    "mg",
    "sniper",
    "rocketlauncher",
    "explosive",
    "grenade",
    "turret",
    "c4",
    "m203",
    "hold",
    "briefcase",
    "riotshield",
    "laptop",
    "throwingknife",
];

pub const PLAYER_ANIM_TYPE_COUNT: usize = PLAYER_ANIM_TYPE_NAMES.len();

/// `grenade`: a grenade held in one hand (`pb_stand_grenade_pullpin`), thrown on fire.
pub const PLAYER_ANIM_TYPE_GRENADE: i32 = 9;
/// `pistol`: an upright body with a one-handed weapon (`pb_stand_alert_pistol`).
pub const PLAYER_ANIM_TYPE_PISTOL: i32 = 2;
/// `throwingknife`: a knife held in one hand (`pb_stand_pullout_knife`).
pub const PLAYER_ANIM_TYPE_THROWINGKNIFE: i32 = 17;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_types_match_their_names() {
        assert_eq!(PLAYER_ANIM_TYPE_NAMES[PLAYER_ANIM_TYPE_GRENADE as usize], "grenade");
        assert_eq!(PLAYER_ANIM_TYPE_NAMES[PLAYER_ANIM_TYPE_PISTOL as usize], "pistol");
        assert_eq!(
            PLAYER_ANIM_TYPE_NAMES[PLAYER_ANIM_TYPE_THROWINGKNIFE as usize],
            "throwingknife"
        );
    }
}
