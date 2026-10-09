//! CS:GO's shooting (`shooting_mode csgo`, the default; `shooting_mode cs16` keeps CS 1.6's): each
//! gun's spray is a fixed table grown from its recoil seed, every shot kicks the aim (and a little
//! of the view) on a velocity that dies away, and where a bullet goes is the aim plus twice that
//! kick, thrown off by an inaccuracy that builds with firing, moving and jumping and recovers on
//! the gun's own clock, plus the gun's fixed spread. The numbers are CS:GO's `items_game.txt`;
//! each CS 1.6 gun takes the CS:GO gun closest to it.

/// `PlayerState::cs_shooting_mode`.
pub const SHOOTING_CSGO: u32 = 0;
pub const SHOOTING_CS16: u32 = 1;

/// The shooting mode `shooting_mode` names: csgo or cs16.
#[must_use]
pub fn shooting_mode_from_name(name: &str) -> Option<u32> {
    let name = name.trim();
    if name.eq_ignore_ascii_case("csgo") {
        Some(SHOOTING_CSGO)
    } else if name.eq_ignore_ascii_case("cs16") {
        Some(SHOOTING_CS16)
    } else {
        None
    }
}

/// A gun's CS:GO shooting numbers, primary and alternate mode (`[0]`, `[1]`: alternate is scoped,
/// silenced or in burst). Inaccuracy and spread are the schema's thousandths of a unit offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsgoGun {
    /// The CS 1.6 gun (`CsWeapon::name`) and the CS:GO item whose numbers it takes.
    pub cs_name: &'static str,
    pub item: &'static str,
    pub full_auto: bool,
    pub recoil_seed: i32,
    pub recoil_angle: [f32; 2],
    pub recoil_angle_variance: [f32; 2],
    pub recoil_magnitude: [f32; 2],
    pub recoil_magnitude_variance: [f32; 2],
    pub spread: [f32; 2],
    pub inaccuracy_stand: [f32; 2],
    pub inaccuracy_crouch: [f32; 2],
    pub inaccuracy_move: [f32; 2],
    pub inaccuracy_jump: [f32; 2],
    pub inaccuracy_jump_initial: f32,
    pub inaccuracy_land: [f32; 2],
    pub inaccuracy_ladder: [f32; 2],
    pub inaccuracy_fire: [f32; 2],
    pub inaccuracy_reload: f32,
    /// Seconds for the inaccuracy to fall to a tenth; the `final` ones take over across the
    /// spray's bullets `recovery_transition` (a negative final means none).
    pub recovery_time_stand: f32,
    pub recovery_time_crouch: f32,
    pub recovery_time_stand_final: f32,
    pub recovery_time_crouch_final: f32,
    pub recovery_transition: (i32, i32),
    pub max_speed: [f32; 2],
    pub cycletime: f32,
}

/// The CS:GO numbers of the CS 1.6 gun named `name` (`CsWeapon::name`).
#[must_use]
pub fn csgo_gun(name: &str) -> Option<&'static CsgoGun> {
    CSGO_GUNS.iter().find(|gun| gun.cs_name == name)
}

const THOUSANDTH: f32 = 0.001;
/// `weapon_recoil_suppression_shots`, `_factor` and `weapon_recoil_variance`.
const RECOIL_SUPPRESSION_SHOTS: usize = 4;
const RECOIL_SUPPRESSION_FACTOR: f32 = 0.75;
const RECOIL_VARIANCE: f32 = 0.55;
/// The spray table repeats after this many bullets.
const RECOIL_TABLE_LEN: usize = 64;
/// `weapon_recoil_decay2_exp`, `_lin`, `weapon_recoil_vel_decay`, `view_punch_decay`.
const AIM_PUNCH_DECAY_EXP: f32 = 8.0;
const AIM_PUNCH_DECAY_LIN: f32 = 18.0;
const AIM_PUNCH_VEL_DECAY: f32 = 4.5;
const VIEW_PUNCH_DECAY: f32 = 18.0;
/// `weapon_recoil_view_punch_extra`: the share of a kick that also shakes the view.
const VIEW_PUNCH_EXTRA: f32 = 0.055;
/// `weapon_recoil_scale`: a bullet leaves along the aim plus this many times the aim punch.
pub const AIM_PUNCH_SCALE: f32 = 2.0;
/// `view_recoil_tracking`: how much of that the view follows.
pub const VIEW_RECOIL_TRACKING: f32 = 0.45;
/// `weapon_recoil_decay_coefficient`: the spray position falls to a tenth in half a second once
/// the trigger has rested a little longer (`WEAPON_RECOIL_DECAY_THRESHOLD`) than the cycle.
const RECOIL_INDEX_DECAY: f32 = 2.0;
const RECOIL_DECAY_THRESHOLD: f32 = 1.10;
/// `CS_PLAYER_SPEED_DUCK_MODIFIER`, `MOVEMENT_CURVE01_EXPONENT`, `sv_jump_impulse`.
const DUCK_SPEED: f32 = 0.34;
const MOVEMENT_CURVE_EXPONENT: f32 = 0.25;
const JUMP_IMPULSE: f32 = 301.993_38;
/// In the air the inaccuracy recovers four times slower (from the crouched time).
const AIR_RECOVERY_SCALE: f32 = 4.0;

fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + (b - a) * t
}

/// `RemapValClamped`.
fn remap_clamped(value: f32, a: f32, b: f32, c: f32, d: f32) -> f32 {
    if a == b {
        return if value >= b { d } else { c };
    }
    let t = ((value - a) / (b - a)).clamp(0.0, 1.0);
    c + (d - c) * t
}

/// Valve's uniform random stream (`CUniformRandomStream`): Park and Miller's minimal standard
/// generator behind a 32-slot Bays-Durham shuffle, the spray tables' source.
struct UniformStream {
    idum: i32,
    iy: i32,
    iv: [i32; 32],
}

impl UniformStream {
    fn new(seed: i32) -> Self {
        Self {
            idum: if seed < 0 { seed } else { -seed },
            iy: 0,
            iv: [0; 32],
        }
    }

    fn step(&mut self) {
        const IA: i32 = 16807;
        const IM: i32 = 2_147_483_647;
        const IQ: i32 = 127_773;
        const IR: i32 = 2836;
        let k = self.idum / IQ;
        self.idum = IA * (self.idum - k * IQ) - IR * k;
        if self.idum < 0 {
            self.idum += IM;
        }
    }

    fn next(&mut self) -> i32 {
        const NDIV: i32 = 1 + (2_147_483_647 - 1) / 32;
        if self.idum <= 0 || self.iy == 0 {
            self.idum = if -self.idum < 1 { 1 } else { -self.idum };
            for j in (0..40).rev() {
                self.step();
                if j < 32 {
                    self.iv[j] = self.idum;
                }
            }
            self.iy = self.iv[0];
        }
        self.step();
        let j = (self.iy / NDIV) as usize & 31;
        self.iy = self.iv[j];
        self.iv[j] = self.idum;
        self.iy
    }

    fn float(&mut self, low: f32, high: f32) -> f32 {
        const AM: f64 = 1.0 / 2_147_483_647.0;
        const RNMX: f64 = 1.0 - 1.2e-7;
        let mut unit = (AM * f64::from(self.next())) as f32;
        if f64::from(unit) > RNMX {
            unit = RNMX as f32;
        }
        unit * (high - low) + low
    }
}

/// The kick of spray bullet `index` (from 0) in `mode`: (angle, magnitude) (`GenerateRecoilTable`
/// then `GetRecoilOffsets`). Each kick is the gun's angle and magnitude give or take their
/// variance, drawn from its seed; a full-auto gun's moves only 55% of the way to each new draw,
/// and its first four kicks are softened from 75%.
#[must_use]
pub fn recoil_offset(gun: &CsgoGun, mode: usize, index: usize) -> (f32, f32) {
    let index = index % RECOIL_TABLE_LEN;
    let mut stream = UniformStream::new(gun.recoil_seed);
    let (mut angle, mut magnitude) = (0.0, 0.0);
    for j in 0..=index {
        let spread = gun.recoil_angle_variance[mode];
        let angle_new = gun.recoil_angle[mode] + stream.float(-spread, spread);
        let spread = gun.recoil_magnitude_variance[mode];
        let magnitude_new = gun.recoil_magnitude[mode] + stream.float(-spread, spread);
        if gun.full_auto && j > 0 {
            angle = lerp(RECOIL_VARIANCE, angle, angle_new);
            magnitude = lerp(RECOIL_VARIANCE, magnitude, magnitude_new);
        } else {
            angle = angle_new;
            magnitude = magnitude_new;
        }
        if gun.full_auto && j < RECOIL_SUPPRESSION_SHOTS {
            magnitude *= lerp(
                j as f32 / RECOIL_SUPPRESSION_SHOTS as f32,
                RECOIL_SUPPRESSION_FACTOR,
                1.0,
            );
        }
    }
    (angle, magnitude)
}

/// `KickBack`: a kick of `magnitude` toward `angle` (0 straight up, positive to the left) speeds
/// the aim punch up and shakes the view a little. Angles are pitch, yaw, roll; pitch up is
/// negative.
pub fn kick(aim_punch_vel: &mut [f32; 3], view_punch: &mut [f32; 3], angle: f32, magnitude: f32) {
    let (sin, cos) = libm::sincosf(angle.to_radians());
    aim_punch_vel[1] -= sin * magnitude;
    aim_punch_vel[0] -= cos * magnitude;
    let extra = magnitude * VIEW_PUNCH_EXTRA;
    view_punch[1] -= sin * extra;
    view_punch[0] -= cos * extra;
}

/// `HybridDecay` / `DecayAngles`: shrink exponentially, then by a fixed amount, to zero.
fn decay(v: &mut [f32; 3], exp: f32, lin: f32, dt: f32) {
    let shrink = libm::expf(-exp * dt);
    for axis in v.iter_mut() {
        *axis *= shrink;
    }
    let length = libm::sqrtf(v.iter().map(|a| a * a).sum());
    let lin = lin * dt;
    if length > lin {
        let scale = 1.0 - lin / length;
        for axis in v.iter_mut() {
            *axis *= scale;
        }
    } else {
        *v = [0.0; 3];
    }
}

/// `DecayAimPunchAngle` and `DecayViewPunchAngle` over `dt` seconds: the aim punch eases back
/// while its velocity carries it on and dies away; the view shake fades.
pub fn decay_punch(
    aim_punch: &mut [f32; 3],
    aim_punch_vel: &mut [f32; 3],
    view_punch: &mut [f32; 3],
    dt: f32,
) {
    decay(aim_punch, AIM_PUNCH_DECAY_EXP, AIM_PUNCH_DECAY_LIN, dt);
    let fade = libm::expf(-AIM_PUNCH_VEL_DECAY * dt);
    for axis in 0..3 {
        aim_punch[axis] += aim_punch_vel[axis] * dt * 0.5;
        aim_punch_vel[axis] *= fade;
        aim_punch[axis] += aim_punch_vel[axis] * dt * 0.5;
    }
    decay(view_punch, VIEW_PUNCH_DECAY, 0.0, dt);
}

/// What the shooter is doing, for inaccuracy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsgoShooter {
    pub on_ground: bool,
    pub ducked: bool,
    pub on_ladder: bool,
    /// Holding the walk key.
    pub walking: bool,
    pub reloading: bool,
    /// Horizontal and vertical speed.
    pub speed: f32,
    pub vertical_speed: f32,
}

/// `GetRecoveryTime`: how long the inaccuracy takes to fall to a tenth now.
fn recovery_time(gun: &CsgoGun, shooter: CsgoShooter, recoil_index: f32) -> f32 {
    if shooter.on_ladder {
        return gun.recovery_time_stand;
    }
    if !shooter.on_ground {
        return gun.recovery_time_crouch * AIR_RECOVERY_SCALE;
    }
    let (base, last) = if shooter.ducked {
        (gun.recovery_time_crouch, gun.recovery_time_crouch_final)
    } else {
        (gun.recovery_time_stand, gun.recovery_time_stand_final)
    };
    if last < 0.0 {
        return base;
    }
    let (start, end) = gun.recovery_transition;
    remap_clamped(
        (recoil_index as i32) as f32,
        start as f32,
        end as f32,
        base,
        last,
    )
}

/// `UpdateAccuracyPenalty`, once per command of `dt` seconds: the inaccuracy floor for the
/// stance (standing, crouching, in the air, on a ladder, reloading) takes over at once when
/// higher, else the inaccuracy recovers toward it; the spray position falls back once the
/// trigger has rested past the gun's cycle (`since_shot` seconds since the last shot).
pub fn update_accuracy(
    gun: &CsgoGun,
    mode: usize,
    shooter: CsgoShooter,
    penalty: &mut f32,
    recoil_index: &mut f32,
    dt: f32,
    since_shot: f32,
) {
    let mut floor = if shooter.on_ladder {
        gun.inaccuracy_ladder[mode] + gun.inaccuracy_ladder[0]
    } else if !shooter.on_ground {
        gun.inaccuracy_stand[mode] + gun.inaccuracy_jump[mode]
    } else if shooter.ducked {
        gun.inaccuracy_crouch[mode]
    } else {
        gun.inaccuracy_stand[mode]
    };
    if shooter.reloading {
        floor += gun.inaccuracy_reload;
    }
    let floor = floor * THOUSANDTH;
    if floor > *penalty {
        *penalty = floor;
    } else {
        let rate = core::f32::consts::LN_10 / recovery_time(gun, shooter, *recoil_index);
        *penalty = lerp(libm::expf(-dt * rate), floor, *penalty);
    }
    if since_shot > gun.cycletime * RECOIL_DECAY_THRESHOLD {
        let rate = core::f32::consts::LN_10 * RECOIL_INDEX_DECAY;
        *recoil_index = lerp(libm::expf(-dt * rate), 0.0, *recoil_index);
    }
}

/// `GetInaccuracy` for a shot: the accumulated penalty, plus moving (from a crouch walk's speed
/// up to 95% of the run, steeply unless walking), plus in the air by the vertical speed (worst
/// leaving the ground, none at the top of the jump, never past twice the take-off penalty).
#[must_use]
pub fn inaccuracy(gun: &CsgoGun, mode: usize, penalty: f32, shooter: CsgoShooter) -> f32 {
    let max_speed = gun.max_speed[mode];
    let mut total = penalty;
    let moving = remap_clamped(
        shooter.speed,
        max_speed * DUCK_SPEED,
        max_speed * 0.95,
        0.0,
        1.0,
    );
    if moving > 0.0 {
        let moving = if shooter.walking {
            moving
        } else {
            libm::powf(moving, MOVEMENT_CURVE_EXPONENT)
        };
        total += moving * gun.inaccuracy_move[mode] * THOUSANDTH;
    }
    if !shooter.on_ground {
        let initial = gun.inaccuracy_jump_initial * THOUSANDTH;
        let top = libm::sqrtf(JUMP_IMPULSE);
        let air = remap(
            libm::sqrtf(shooter.vertical_speed.abs()),
            top * 0.25,
            top,
            0.0,
            initial,
        );
        total += air.clamp(0.0, 2.0 * initial);
    }
    total.min(1.0)
}

/// `RemapVal` (unclamped).
fn remap(value: f32, a: f32, b: f32, c: f32, d: f32) -> f32 {
    if a == b {
        return if value >= b { d } else { c };
    }
    c + (d - c) * (value - a) / (b - a)
}

/// The gun's fixed spread in `mode`.
#[must_use]
pub fn spread(gun: &CsgoGun, mode: usize) -> f32 {
    gun.spread[mode] * THOUSANDTH
}

/// A shot leaves (`CSBaseGunFire`): the inaccuracy grows by the gun's fire penalty and the aim is
/// kicked by spray bullet `recoil_index`, which moves on.
pub fn fire(
    gun: &CsgoGun,
    mode: usize,
    penalty: &mut f32,
    recoil_index: &mut f32,
    aim_punch_vel: &mut [f32; 3],
    view_punch: &mut [f32; 3],
) {
    *penalty += gun.inaccuracy_fire[mode] * THOUSANDTH;
    let (angle, magnitude) = recoil_offset(gun, mode, recoil_index.max(0.0) as usize);
    kick(aim_punch_vel, view_punch, angle, magnitude);
    *recoil_index += 1.0;
}

/// One bullet's offset from the aim, along its right and up (`FX_FireBullets`): the inaccuracy
/// throws a shot anywhere within a disc of that radius (one draw for all its pellets) and the
/// spread each pellet within another. `rolls` are four uniform draws in `0..1`: the inaccuracy's
/// radius and angle, then the pellet's.
#[must_use]
pub fn bullet_offset(inaccuracy: f32, spread: f32, rolls: [f32; 4]) -> (f32, f32) {
    let (sin0, cos0) = libm::sincosf(rolls[1] * core::f32::consts::TAU);
    let (sin1, cos1) = libm::sincosf(rolls[3] * core::f32::consts::TAU);
    let r0 = rolls[0] * inaccuracy;
    let r1 = rolls[2] * spread;
    (r0 * cos0 + r1 * cos1, r0 * sin0 + r1 * sin1)
}

/// How a CS shot's bullets scatter: CS 1.6's spread, or CS:GO's inaccuracy and spread
/// ([`bullet_offset`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShotSpread {
    Cs16(f32),
    Csgo { inaccuracy: f32, spread: f32 },
}

/// The CS:GO mode a CS gun shoots in: alternate while scoped, silenced or in burst.
#[must_use]
pub fn gun_mode(zoomed: bool, silenced: bool, burst: bool) -> usize {
    usize::from(zoomed || silenced || burst)
}

/// Where the aim punch puts bullets (`GetFinalAimAngle`): `AIM_PUNCH_SCALE` times it in CS:GO,
/// the punch itself in CS 1.6.
#[must_use]
pub fn aim_offset(mode: u32, punch: [f32; 3]) -> [f32; 3] {
    if mode == SHOOTING_CS16 {
        punch
    } else {
        punch.map(|a| a * AIM_PUNCH_SCALE)
    }
}

/// How far the camera is turned by the recoil: in CS:GO the view shake plus `VIEW_RECOIL_TRACKING`
/// of where bullets go; in CS 1.6 the punch.
#[must_use]
pub fn view_offset(mode: u32, punch: [f32; 3], view_punch: [f32; 3]) -> [f32; 3] {
    if mode == SHOOTING_CS16 {
        punch
    } else {
        core::array::from_fn(|i| view_punch[i] + punch[i] * AIM_PUNCH_SCALE * VIEW_RECOIL_TRACKING)
    }
}

pub const CSGO_GUNS: [CsgoGun; 24] = [
    CsgoGun {
        cs_name: "ak47",
        item: "weapon_ak47",
        full_auto: true,
        recoil_seed: 223,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [70.0, 70.0],
        recoil_magnitude: [30.0, 30.0],
        recoil_magnitude_variance: [0.0, 0.0],
        spread: [0.6, 0.6],
        inaccuracy_stand: [6.41, 6.41],
        inaccuracy_crouch: [4.81, 4.81],
        inaccuracy_move: [175.059998, 175.059998],
        inaccuracy_jump: [140.759995, 140.759995],
        inaccuracy_jump_initial: 100.940002,
        inaccuracy_land: [0.242, 0.242],
        inaccuracy_ladder: [140.0, 140.0],
        inaccuracy_fire: [7.8, 7.8],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.368,
        recovery_time_crouch: 0.305257,
        recovery_time_stand_final: 0.506,
        recovery_time_crouch_final: 0.419728,
        recovery_transition: (2, 5),
        max_speed: [215.0, 215.0],
        cycletime: 0.1,
    },
    CsgoGun {
        cs_name: "m4a1",
        item: "weapon_m4a1_silencer",
        full_auto: true,
        recoil_seed: 38965,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [65.0, 65.0],
        recoil_magnitude: [25.0, 21.0],
        recoil_magnitude_variance: [3.0, 0.0],
        spread: [0.6, 0.5],
        inaccuracy_stand: [4.9, 4.9],
        inaccuracy_crouch: [4.1, 4.1],
        inaccuracy_move: [92.879997, 122.0],
        inaccuracy_jump: [99.699997, 99.699997],
        inaccuracy_jump_initial: 96.769997,
        inaccuracy_land: [0.197, 0.197],
        inaccuracy_ladder: [110.994003, 113.671997],
        inaccuracy_fire: [12.0, 7.0],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.338941,
        recovery_time_crouch: 0.2421,
        recovery_time_stand_final: 0.466044,
        recovery_time_crouch_final: 0.332888,
        recovery_transition: (2, 5),
        max_speed: [225.0, 225.0],
        cycletime: 0.1,
    },
    CsgoGun {
        cs_name: "awp",
        item: "weapon_awp",
        full_auto: false,
        recoil_seed: 4100,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [20.0, 20.0],
        recoil_magnitude: [78.0, 25.0],
        recoil_magnitude_variance: [15.0, 2.0],
        spread: [0.2, 0.2],
        inaccuracy_stand: [80.800003, 2.0],
        inaccuracy_crouch: [60.599998, 1.5],
        inaccuracy_move: [176.479996, 176.479996],
        inaccuracy_jump: [133.830002, 133.830002],
        inaccuracy_jump_initial: 172.860001,
        inaccuracy_land: [0.307, 0.1],
        inaccuracy_ladder: [136.5, 136.5],
        inaccuracy_fire: [53.849998, 53.849998],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.34539,
        recovery_time_crouch: 0.24671,
        recovery_time_stand_final: 0.34539,
        recovery_time_crouch_final: 0.24671,
        recovery_transition: (2, 5),
        max_speed: [200.0, 100.0],
        cycletime: 1.455,
    },
    CsgoGun {
        cs_name: "deagle",
        item: "weapon_deagle",
        full_auto: false,
        recoil_seed: 1454,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [60.0, 60.0],
        recoil_magnitude: [48.200001, 48.200001],
        recoil_magnitude_variance: [18.0, 18.0],
        spread: [2.0, 2.0],
        inaccuracy_stand: [4.2, 4.2],
        inaccuracy_crouch: [2.18, 2.18],
        inaccuracy_move: [48.099998, 48.099998],
        inaccuracy_jump: [40.549999, 371.549988],
        inaccuracy_jump_initial: 548.820007,
        inaccuracy_land: [0.043, 0.73],
        inaccuracy_ladder: [152.0, 152.0],
        inaccuracy_fire: [72.230003, 72.230003],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.8112,
        recovery_time_crouch: 0.449927,
        recovery_time_stand_final: 0.8112,
        recovery_time_crouch_final: 0.449927,
        recovery_transition: (3, 10),
        max_speed: [230.0, 230.0],
        cycletime: 0.225,
    },
    CsgoGun {
        cs_name: "usp",
        item: "weapon_usp_silencer",
        full_auto: false,
        recoil_seed: 5426,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [0.0, 0.0],
        recoil_magnitude: [29.0, 23.0],
        recoil_magnitude_variance: [0.0, 0.0],
        spread: [2.5, 1.5],
        inaccuracy_stand: [4.9, 4.9],
        inaccuracy_crouch: [3.68, 3.68],
        inaccuracy_move: [13.87, 13.87],
        inaccuracy_jump: [94.480003, 94.480003],
        inaccuracy_jump_initial: 96.599998,
        inaccuracy_land: [0.191, 0.198],
        inaccuracy_ladder: [138.320007, 119.900002],
        inaccuracy_fire: [71.0, 52.0],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.349532,
        recovery_time_crouch: 0.291277,
        recovery_time_stand_final: 0.349532,
        recovery_time_crouch_final: 0.291277,
        recovery_transition: (3, 10),
        max_speed: [240.0, 240.0],
        cycletime: 0.17,
    },
    CsgoGun {
        cs_name: "glock",
        item: "weapon_glock",
        full_auto: false,
        recoil_seed: 4484,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [20.0, 20.0],
        recoil_magnitude: [18.0, 30.0],
        recoil_magnitude_variance: [0.0, 5.0],
        spread: [2.0, 15.0],
        inaccuracy_stand: [5.6, 5.6],
        inaccuracy_crouch: [4.2, 3.0],
        inaccuracy_move: [10.0, 12.95],
        inaccuracy_jump: [87.870003, 87.870003],
        inaccuracy_jump_initial: 96.620003,
        inaccuracy_land: [0.185, 0.185],
        inaccuracy_ladder: [137.0, 119.25],
        inaccuracy_fire: [56.0, 45.0],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.2,
        recovery_time_crouch: 0.2,
        recovery_time_stand_final: 0.33,
        recovery_time_crouch_final: 0.33,
        recovery_transition: (0, 5),
        max_speed: [240.0, 240.0],
        cycletime: 0.15,
    },
    CsgoGun {
        cs_name: "p228",
        item: "weapon_p250",
        full_auto: false,
        recoil_seed: 9788,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [10.0, 10.0],
        recoil_magnitude: [26.0, 26.0],
        recoil_magnitude_variance: [3.0, 3.0],
        spread: [2.0, 2.0],
        inaccuracy_stand: [9.1, 9.1],
        inaccuracy_crouch: [6.83, 6.83],
        inaccuracy_move: [20.0, 13.41],
        inaccuracy_jump: [92.959999, 92.959999],
        inaccuracy_jump_initial: 96.620003,
        inaccuracy_land: [0.19, 0.19],
        inaccuracy_ladder: [138.0, 138.0],
        inaccuracy_fire: [52.450001, 52.450001],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.345388,
        recovery_time_crouch: 0.287823,
        recovery_time_stand_final: 0.345388,
        recovery_time_crouch_final: 0.287823,
        recovery_transition: (3, 10),
        max_speed: [240.0, 240.0],
        cycletime: 0.15,
    },
    CsgoGun {
        cs_name: "fiveseven",
        item: "weapon_fiveseven",
        full_auto: false,
        recoil_seed: 33244,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [5.0, 5.0],
        recoil_magnitude: [25.0, 25.0],
        recoil_magnitude_variance: [4.0, 4.0],
        spread: [2.0, 2.0],
        inaccuracy_stand: [9.1, 9.1],
        inaccuracy_crouch: [6.83, 6.83],
        inaccuracy_move: [40.0, 13.41],
        inaccuracy_jump: [89.699997, 89.699997],
        inaccuracy_jump_initial: 99.879997,
        inaccuracy_land: [0.19, 0.19],
        inaccuracy_ladder: [138.0, 138.0],
        inaccuracy_fire: [25.0, 32.450001],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.2,
        recovery_time_crouch: 0.2,
        recovery_time_stand_final: 0.5,
        recovery_time_crouch_final: 0.5,
        recovery_transition: (0, 5),
        max_speed: [240.0, 240.0],
        cycletime: 0.15,
    },
    CsgoGun {
        cs_name: "elite",
        item: "weapon_elite",
        full_auto: false,
        recoil_seed: 24563,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [20.0, 20.0],
        recoil_magnitude: [27.0, 27.0],
        recoil_magnitude_variance: [4.0, 4.0],
        spread: [2.0, 2.0],
        inaccuracy_stand: [7.0, 10.0],
        inaccuracy_crouch: [5.25, 7.5],
        inaccuracy_move: [17.85, 17.85],
        inaccuracy_jump: [158.419998, 158.419998],
        inaccuracy_jump_initial: 95.860001,
        inaccuracy_land: [0.255, 0.255],
        inaccuracy_ladder: [102.0, 102.0],
        inaccuracy_fire: [11.16, 11.96],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.524989,
        recovery_time_crouch: 0.437491,
        recovery_time_stand_final: 0.524989,
        recovery_time_crouch_final: 0.437491,
        recovery_transition: (3, 10),
        max_speed: [240.0, 240.0],
        cycletime: 0.12,
    },
    CsgoGun {
        cs_name: "m3",
        item: "weapon_nova",
        full_auto: false,
        recoil_seed: 7763,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [20.0, 20.0],
        recoil_magnitude: [143.0, 143.0],
        recoil_magnitude_variance: [22.0, 22.0],
        spread: [40.0, 40.0],
        inaccuracy_stand: [7.0, 7.0],
        inaccuracy_crouch: [5.25, 5.25],
        inaccuracy_move: [36.75, 36.75],
        inaccuracy_jump: [126.309998, 126.309998],
        inaccuracy_jump_initial: 109.699997,
        inaccuracy_land: [0.236, 0.236],
        inaccuracy_ladder: [78.75, 78.75],
        inaccuracy_fire: [9.72, 9.72],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.460517,
        recovery_time_crouch: 0.328941,
        recovery_time_stand_final: 0.460517,
        recovery_time_crouch_final: 0.328941,
        recovery_transition: (2, 5),
        max_speed: [220.0, 220.0],
        cycletime: 0.88,
    },
    CsgoGun {
        cs_name: "xm1014",
        item: "weapon_xm1014",
        full_auto: true,
        recoil_seed: 24862,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [20.0, 20.0],
        recoil_magnitude: [80.0, 80.0],
        recoil_magnitude_variance: [20.0, 20.0],
        spread: [38.0, 38.0],
        inaccuracy_stand: [7.0, 7.0],
        inaccuracy_crouch: [5.25, 5.25],
        inaccuracy_move: [36.029999, 36.029999],
        inaccuracy_jump: [130.830002, 130.830002],
        inaccuracy_jump_initial: 100.379997,
        inaccuracy_land: [0.232, 0.232],
        inaccuracy_ladder: [77.209999, 77.209999],
        inaccuracy_fire: [8.83, 8.83],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.506569,
        recovery_time_crouch: 0.361835,
        recovery_time_stand_final: 0.506569,
        recovery_time_crouch_final: 0.361835,
        recovery_transition: (2, 5),
        max_speed: [215.0, 215.0],
        cycletime: 0.35,
    },
    CsgoGun {
        cs_name: "mac10",
        item: "weapon_mac10",
        full_auto: true,
        recoil_seed: 34079,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [70.0, 70.0],
        recoil_magnitude: [18.0, 18.0],
        recoil_magnitude_variance: [1.0, 1.0],
        spread: [0.6, 0.6],
        inaccuracy_stand: [13.3, 13.3],
        inaccuracy_crouch: [9.98, 9.98],
        inaccuracy_move: [13.99, 13.99],
        inaccuracy_jump: [33.299999, 33.299999],
        inaccuracy_jump_initial: 34.990002,
        inaccuracy_land: [0.069, 0.069],
        inaccuracy_ladder: [34.259998, 34.259998],
        inaccuracy_fire: [4.76, 4.76],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.399729,
        recovery_time_crouch: 0.285521,
        recovery_time_stand_final: 0.399729,
        recovery_time_crouch_final: 0.285521,
        recovery_transition: (2, 5),
        max_speed: [240.0, 240.0],
        cycletime: 0.075,
    },
    CsgoGun {
        cs_name: "tmp",
        item: "weapon_mp9",
        full_auto: true,
        recoil_seed: 50729,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [70.0, 70.0],
        recoil_magnitude: [19.0, 19.0],
        recoil_magnitude_variance: [1.0, 1.0],
        spread: [0.6, 0.6],
        inaccuracy_stand: [9.0, 9.0],
        inaccuracy_crouch: [5.5, 5.5],
        inaccuracy_move: [29.040001, 29.040001],
        inaccuracy_jump: [18.43, 18.43],
        inaccuracy_jump_initial: 37.279999,
        inaccuracy_land: [0.056, 0.056],
        inaccuracy_ladder: [148.912506, 148.912506],
        inaccuracy_fire: [3.7, 3.7],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.25789,
        recovery_time_crouch: 0.184207,
        recovery_time_stand_final: 0.25789,
        recovery_time_crouch_final: 0.184207,
        recovery_transition: (2, 5),
        max_speed: [240.0, 240.0],
        cycletime: 0.07,
    },
    CsgoGun {
        cs_name: "mp5",
        item: "weapon_mp5sd",
        full_auto: true,
        recoil_seed: 61649,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [70.0, 70.0],
        recoil_magnitude: [16.0, 16.0],
        recoil_magnitude_variance: [1.0, 1.0],
        spread: [0.6, 0.6],
        inaccuracy_stand: [10.0, 10.0],
        inaccuracy_crouch: [5.92, 5.92],
        inaccuracy_move: [30.0, 19.860001],
        inaccuracy_jump: [59.599998, 59.599998],
        inaccuracy_jump_initial: 55.41,
        inaccuracy_land: [0.115, 0.115],
        inaccuracy_ladder: [57.560001, 57.560001],
        inaccuracy_fire: [2.18, 2.18],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.437491,
        recovery_time_crouch: 0.312494,
        recovery_time_stand_final: 0.437491,
        recovery_time_crouch_final: 0.312494,
        recovery_transition: (2, 5),
        max_speed: [235.0, 220.0],
        cycletime: 0.08,
    },
    CsgoGun {
        cs_name: "ump45",
        item: "weapon_ump45",
        full_auto: true,
        recoil_seed: 59299,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [40.0, 40.0],
        recoil_magnitude: [23.0, 23.0],
        recoil_magnitude_variance: [1.0, 1.0],
        spread: [1.0, 1.0],
        inaccuracy_stand: [13.43, 13.43],
        inaccuracy_crouch: [10.07, 10.07],
        inaccuracy_move: [28.76, 28.76],
        inaccuracy_jump: [37.25, 37.25],
        inaccuracy_jump_initial: 47.209999,
        inaccuracy_land: [0.085, 0.085],
        inaccuracy_ladder: [42.349998, 42.349998],
        inaccuracy_fire: [3.42, 3.42],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.349993,
        recovery_time_crouch: 0.249995,
        recovery_time_stand_final: 0.349993,
        recovery_time_crouch_final: 0.249995,
        recovery_transition: (2, 5),
        max_speed: [230.0, 230.0],
        cycletime: 0.09,
    },
    CsgoGun {
        cs_name: "p90",
        item: "weapon_p90",
        full_auto: true,
        recoil_seed: 6213,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [70.0, 70.0],
        recoil_magnitude: [16.0, 16.0],
        recoil_magnitude_variance: [1.0, 1.0],
        spread: [1.0, 1.0],
        inaccuracy_stand: [13.65, 13.65],
        inaccuracy_crouch: [10.24, 10.24],
        inaccuracy_move: [31.0, 31.0],
        inaccuracy_jump: [90.080002, 90.080002],
        inaccuracy_jump_initial: 104.599998,
        inaccuracy_land: [0.082, 0.082],
        inaccuracy_ladder: [132.169998, 132.169998],
        inaccuracy_fire: [2.85, 2.85],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.372098,
        recovery_time_crouch: 0.265784,
        recovery_time_stand_final: 0.372098,
        recovery_time_crouch_final: 0.265784,
        recovery_transition: (2, 5),
        max_speed: [230.0, 230.0],
        cycletime: 0.07,
    },
    CsgoGun {
        cs_name: "galil",
        item: "weapon_galilar",
        full_auto: true,
        recoil_seed: 51191,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [70.0, 70.0],
        recoil_magnitude: [21.0, 21.0],
        recoil_magnitude_variance: [1.0, 1.0],
        spread: [0.6, 0.6],
        inaccuracy_stand: [8.77, 7.78],
        inaccuracy_crouch: [6.58, 4.84],
        inaccuracy_move: [123.559998, 106.519997],
        inaccuracy_jump: [149.779999, 149.779999],
        inaccuracy_jump_initial: 105.389999,
        inaccuracy_land: [0.256, 0.256],
        inaccuracy_ladder: [113.580002, 113.580002],
        inaccuracy_fire: [7.0, 5.85],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.3,
        recovery_time_crouch: 0.15,
        recovery_time_stand_final: 0.5,
        recovery_time_crouch_final: 0.47,
        recovery_transition: (2, 5),
        max_speed: [215.0, 215.0],
        cycletime: 0.09,
    },
    CsgoGun {
        cs_name: "famas",
        item: "weapon_famas",
        full_auto: true,
        recoil_seed: 39623,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [60.0, 50.0],
        recoil_magnitude: [20.0, 20.0],
        recoil_magnitude_variance: [1.0, 1.0],
        spread: [0.6, 0.6],
        inaccuracy_stand: [9.85, 3.69],
        inaccuracy_crouch: [7.39, 3.25],
        inaccuracy_move: [99.339996, 99.339996],
        inaccuracy_jump: [110.389999, 110.389999],
        inaccuracy_jump_initial: 94.769997,
        inaccuracy_land: [0.205, 0.205],
        inaccuracy_ladder: [118.716003, 118.716003],
        inaccuracy_fire: [6.05, 3.35],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.25,
        recovery_time_crouch: 0.12,
        recovery_time_stand_final: 0.5,
        recovery_time_crouch_final: 0.48,
        recovery_transition: (2, 5),
        max_speed: [220.0, 220.0],
        cycletime: 0.09,
    },
    CsgoGun {
        cs_name: "sg552",
        item: "weapon_sg556",
        full_auto: true,
        recoil_seed: 43500,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [60.0, 60.0],
        recoil_magnitude: [28.0, 19.0],
        recoil_magnitude_variance: [2.0, 2.0],
        spread: [0.6, 0.3],
        inaccuracy_stand: [5.81, 3.81],
        inaccuracy_crouch: [3.81, 3.05],
        inaccuracy_move: [136.009995, 136.009995],
        inaccuracy_jump: [109.0, 109.0],
        inaccuracy_jump_initial: 78.790001,
        inaccuracy_land: [0.188, 0.188],
        inaccuracy_ladder: [83.660004, 138.757996],
        inaccuracy_fire: [7.95, 9.2],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.452886,
        recovery_time_crouch: 0.379204,
        recovery_time_stand_final: 0.452886,
        recovery_time_crouch_final: 0.379204,
        recovery_transition: (2, 5),
        max_speed: [210.0, 150.0],
        cycletime: 0.11,
    },
    CsgoGun {
        cs_name: "aug",
        item: "weapon_aug",
        full_auto: true,
        recoil_seed: 24204,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [60.0, 60.0],
        recoil_magnitude: [24.0, 16.0],
        recoil_magnitude_variance: [0.0, 0.0],
        spread: [0.5, 0.3],
        inaccuracy_stand: [4.9, 3.68],
        inaccuracy_crouch: [3.68, 3.11],
        inaccuracy_move: [135.449997, 105.449997],
        inaccuracy_jump: [105.989998, 105.989998],
        inaccuracy_jump_initial: 101.559998,
        inaccuracy_land: [0.208, 0.208],
        inaccuracy_ladder: [110.040001, 100.040001],
        inaccuracy_fire: [7.29, 7.29],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.429727,
        recovery_time_crouch: 0.30552,
        recovery_time_stand_final: 0.429727,
        recovery_time_crouch_final: 0.30552,
        recovery_transition: (2, 5),
        max_speed: [220.0, 150.0],
        cycletime: 0.1,
    },
    CsgoGun {
        cs_name: "scout",
        item: "weapon_ssg08",
        full_auto: false,
        recoil_seed: 1278,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [20.0, 20.0],
        recoil_magnitude: [33.0, 25.0],
        recoil_magnitude_variance: [15.0, 2.0],
        spread: [0.28, 0.23],
        inaccuracy_stand: [31.700001, 3.0],
        inaccuracy_crouch: [23.780001, 2.8],
        inaccuracy_move: [123.449997, 123.449997],
        inaccuracy_jump: [5.72, 5.72],
        inaccuracy_jump_initial: 208.720001,
        inaccuracy_land: [0.215, 0.215],
        inaccuracy_ladder: [95.489998, 95.489998],
        inaccuracy_fire: [22.92, 22.92],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.142096,
        recovery_time_crouch: 0.055783,
        recovery_time_stand_final: 0.142096,
        recovery_time_crouch_final: 0.055783,
        recovery_transition: (2, 5),
        max_speed: [230.0, 230.0],
        cycletime: 1.25,
    },
    CsgoGun {
        cs_name: "g3sg1",
        item: "weapon_g3sg1",
        full_auto: true,
        recoil_seed: 29908,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [30.0, 30.0],
        recoil_magnitude: [30.0, 30.0],
        recoil_magnitude_variance: [4.0, 4.0],
        spread: [0.3, 0.3],
        inaccuracy_stand: [25.799999, 2.0],
        inaccuracy_crouch: [19.35, 1.5],
        inaccuracy_move: [150.479996, 150.479996],
        inaccuracy_jump: [153.770004, 153.770004],
        inaccuracy_jump_initial: 107.690002,
        inaccuracy_land: [0.262, 0.262],
        inaccuracy_ladder: [116.389999, 116.389999],
        inaccuracy_fire: [18.610001, 18.610001],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.544331,
        recovery_time_crouch: 0.388808,
        recovery_time_stand_final: 0.544331,
        recovery_time_crouch_final: 0.388808,
        recovery_transition: (2, 5),
        max_speed: [215.0, 120.0],
        cycletime: 0.25,
    },
    CsgoGun {
        cs_name: "sg550",
        item: "weapon_scar20",
        full_auto: true,
        recoil_seed: 19364,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [30.0, 30.0],
        recoil_magnitude: [31.0, 31.0],
        recoil_magnitude_variance: [4.0, 4.0],
        spread: [0.3, 0.3],
        inaccuracy_stand: [25.799999, 2.0],
        inaccuracy_crouch: [19.35, 1.5],
        inaccuracy_move: [150.479996, 150.479996],
        inaccuracy_jump: [153.770004, 153.770004],
        inaccuracy_jump_initial: 107.690002,
        inaccuracy_land: [0.262, 0.262],
        inaccuracy_ladder: [116.389999, 116.389999],
        inaccuracy_fire: [18.610001, 18.610001],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.544331,
        recovery_time_crouch: 0.388808,
        recovery_time_stand_final: 0.544331,
        recovery_time_crouch_final: 0.388808,
        recovery_transition: (2, 5),
        max_speed: [215.0, 120.0],
        cycletime: 0.25,
    },
    CsgoGun {
        cs_name: "m249",
        item: "weapon_m249",
        full_auto: true,
        recoil_seed: 50310,
        recoil_angle: [0.0, 0.0],
        recoil_angle_variance: [50.0, 50.0],
        recoil_magnitude: [25.0, 25.0],
        recoil_magnitude_variance: [2.0, 2.0],
        spread: [2.0, 2.0],
        inaccuracy_stand: [7.7, 7.7],
        inaccuracy_crouch: [5.34, 5.34],
        inaccuracy_move: [156.25, 156.25],
        inaccuracy_jump: [279.470001, 279.470001],
        inaccuracy_jump_initial: 118.269997,
        inaccuracy_land: [0.398, 0.398],
        inaccuracy_ladder: [132.809998, 132.809998],
        inaccuracy_fire: [3.56, 3.56],
        inaccuracy_reload: 0.0,
        recovery_time_stand: 0.828931,
        recovery_time_crouch: 0.592093,
        recovery_time_stand_final: 0.828931,
        recovery_time_crouch_final: 0.592093,
        recovery_transition: (2, 5),
        max_speed: [195.0, 195.0],
        cycletime: 0.08,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn every_cs_gun_has_csgo_numbers() {
        for weapon in &crate::cs::CS_WEAPONS {
            let gun = csgo_gun(weapon.name).expect(weapon.name);
            assert!(
                gun.recoil_magnitude[0] > 0.0 && gun.cycletime > 0.0,
                "{}",
                gun.item
            );
        }
    }

    #[test]
    fn the_random_stream_is_numerical_recipes_ran1() {
        // `ran1` seeded with -1 first returns 0.41599935.
        let mut stream = UniformStream::new(1);
        assert!((stream.float(0.0, 1.0) - 0.415_999_35).abs() < 1e-6);
    }

    #[test]
    fn the_ak_spray_starts_softened_and_climbs() {
        let ak = csgo_gun("ak47").expect("ak");
        // No magnitude variance: 30, softened to 75% for the first bullet, then 55% of the way
        // back to 30 and softened 81.25% for the second.
        let (angle, magnitude) = recoil_offset(ak, 0, 0);
        assert!(near(magnitude, 22.5) && angle.abs() <= 70.0);
        assert!(near(
            recoil_offset(ak, 0, 1).1,
            (22.5 + 7.5 * 0.55) * 0.8125
        ));
        // Past the softened bullets it closes on 30, and the table repeats after 64.
        assert!((recoil_offset(ak, 0, 10).1 - 30.0).abs() < 0.1);
        assert_eq!(recoil_offset(ak, 0, 3), recoil_offset(ak, 0, 67));
    }

    #[test]
    fn a_kick_speeds_the_aim_up_and_dies_away() {
        let (mut punch, mut vel, mut view) = ([0.0; 3], [0.0; 3], [0.0; 3]);
        kick(&mut vel, &mut view, 0.0, 30.0);
        assert!(near(vel[0], -30.0) && near(view[0], -1.65));
        decay_punch(&mut punch, &mut vel, &mut view, 1.0 / 64.0);
        assert!(punch[0] < 0.0);
        for _ in 0..256 {
            decay_punch(&mut punch, &mut vel, &mut view, 1.0 / 64.0);
        }
        assert!(punch.iter().chain(&view).all(|a| a.abs() < 1e-3));
    }

    #[test]
    fn inaccuracy_builds_with_moving_and_recovers() {
        let ak = csgo_gun("ak47").expect("ak");
        let still = CsgoShooter {
            on_ground: true,
            ducked: false,
            on_ladder: false,
            walking: false,
            reloading: false,
            speed: 0.0,
            vertical_speed: 0.0,
        };
        let (mut penalty, mut index) = (0.0, 0.0);
        update_accuracy(ak, 0, still, &mut penalty, &mut index, 1.0 / 64.0, 1.0);
        assert!(near(penalty, 0.00641));
        assert!(near(inaccuracy(ak, 0, penalty, still), 0.00641));
        // Running flat out adds the whole move penalty.
        let running = CsgoShooter {
            speed: 215.0,
            ..still
        };
        assert!(near(
            inaccuracy(ak, 0, penalty, running),
            0.00641 + 0.175_06
        ));
        // A shot adds 7.8 and kicks; a second later it is back near standing.
        let (mut vel, mut view) = ([0.0; 3], [0.0; 3]);
        fire(ak, 0, &mut penalty, &mut index, &mut vel, &mut view);
        assert!(near(penalty, 0.00641 + 0.0078) && index == 1.0);
        for _ in 0..64 {
            update_accuracy(ak, 0, still, &mut penalty, &mut index, 1.0 / 64.0, 1.0);
        }
        assert!(penalty < 0.0066 && index < 0.02);
    }

    #[test]
    fn bullets_follow_twice_the_punch_and_the_view_less() {
        let punch = [-2.0, 1.0, 0.0];
        assert_eq!(aim_offset(SHOOTING_CSGO, punch), [-4.0, 2.0, 0.0]);
        assert_eq!(aim_offset(SHOOTING_CS16, punch), punch);
        let view = view_offset(SHOOTING_CSGO, punch, [-0.5, 0.0, 0.0]);
        assert!(near(view[0], -0.5 - 1.8) && near(view[1], 0.9));
    }
}
