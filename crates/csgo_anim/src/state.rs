//! CS:GO's player animation state (`CCSGOPlayerAnimState`, the subset that poses a player's
//! body: aim matrix, weapon actions, locomotion, jumping and landing, the idle loop and the
//! lean), driven by what a player state says each frame.

use glam::Vec3;
use mdl_source::{BoneFrame, StudioModel};
use playerstate_iw4::{ENTITYNUM_NONE, PlayerState, cs_grenade};

// CS:GO's movement numbers the animations follow (`cs_shareddefs.cpp`).
const SPEED_RUN: f32 = 260.0;
const SPEED_WALK_MODIFIER: f32 = 0.52;
const SPEED_DUCK_MODIFIER: f32 = 0.34;

// CS:GO's player animation state (`csgo_playeranimstate.cpp`).
const MAX_VEL_LIMIT: f32 = 1.2;
const DUCK_APPROACH_SPEED: f32 = 6.0;
const AIM_YAW_MAX: f32 = 58.0;
const AIM_YAW_MIN: f32 = -58.0;
const SPEED_TO_CHANGE_AIM_MATRIX: f32 = 0.8;
const SPEED_TO_CHANGE_AIM_MATRIX_SCOPED: f32 = 4.2;
const LOWER_CATCHUP_IDLE: f32 = 100.0;
const AIM_NARROW_WALK: f32 = 0.8;
const AIM_NARROW_RUN: f32 = 0.5;
const AIM_NARROW_CROUCHMOVING: f32 = 0.5;
const LOWER_REALIGN_DELAY: f32 = 1.1;
const WALK_TO_RUN_TRANSITION_SPEED: f32 = 2.0;
const ONGROUND_FUZZY_APPROACH: f32 = 8.0;
const ONGROUND_FUZZY_APPROACH_CROUCH: f32 = 16.0;
const RUN_ANIM_PLAYBACK_MULTIPLIER: f32 = 0.85;

/// The animation layers, in the order they play over each other (`animstate_layer_t`).
const AIMMATRIX: usize = 0;
const WEAPON_ACTION: usize = 1;
const WEAPON_ACTION_RECROUCH: usize = 2;
const JUMP_OR_FALL: usize = 4;
const LAND_OR_CLIMB: usize = 5;
const MOVE: usize = 6;
const FLASHED: usize = 9;
const ALIVELOOP: usize = 11;
const LEAN: usize = 12;
const LAYER_COUNT: usize = 13;

/// The CS:GO agent a faction is drawn as, by its radio voice (`cs::csgo_radio_voice`).
pub fn agent_model(voice: &str) -> &'static str {
    match voice {
        "sas" => "ctm_sas",
        "seal" => "ctm_st6",
        "balkan" => "tm_balkan_varianta",
        "leet" => "tm_leet_varianta",
        "separatist" => "tm_separatist",
        _ => "tm_phoenix",
    }
}

fn approach(target: f32, value: f32, speed: f32) -> f32 {
    let delta = target - value;
    if delta > speed {
        value + speed
    } else if delta < -speed {
        value - speed
    } else {
        target
    }
}

fn approach_vec(target: Vec3, value: Vec3, speed: f32) -> Vec3 {
    let delta = target - value;
    let len = delta.length();
    if len > speed { value + delta / len * speed } else { target }
}

/// An angle in (-180, 180].
fn angle_normalize(angle: f32) -> f32 {
    let a = angle.rem_euclid(360.0);
    if a > 180.0 { a - 360.0 } else { a }
}

/// How far `dest` is from `src`, the short way round.
fn angle_diff(dest: f32, src: f32) -> f32 {
    angle_normalize(dest - src)
}

fn approach_angle(target: f32, value: f32, speed: f32) -> f32 {
    let delta = angle_diff(target, value);
    let speed = speed.abs();
    if delta > speed {
        value + speed
    } else if delta < -speed {
        value - speed
    } else {
        target
    }
}

/// Schlick's bias: `x` bent toward 0 or 1 (`amount` 0.5 leaves it).
fn bias(x: f32, amount: f32) -> f32 {
    x.max(0.0).powf(amount.ln() / 0.5f32.ln())
}

fn smoothstep_bounds(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + (b - a) * t
}

fn remap_clamped(value: f32, a: f32, b: f32, c: f32, d: f32) -> f32 {
    let t = ((value - a) / (b - a)).clamp(0.0, 1.0);
    c + (d - c) * t
}

/// A cycle wrapped into 0..1.
fn clamp_cycle(cycle: f32) -> f32 {
    cycle - cycle.floor()
}

/// A pose parameter of the animation set, by the role CS:GO gives it.
#[derive(Clone, Copy, Default)]
struct Params {
    move_yaw: Option<usize>,
    body_yaw: Option<usize>,
    body_pitch: Option<usize>,
    speed: Option<usize>,
    stand: Option<usize>,
    jump_fall: Option<usize>,
    lean_yaw: Option<usize>,
    aim_stand_idle: Option<usize>,
    aim_stand_walk: Option<usize>,
    aim_stand_run: Option<usize>,
    aim_crouch_idle: Option<usize>,
    aim_crouch_walk: Option<usize>,
    move_blend_walk: Option<usize>,
    move_blend_run: Option<usize>,
    move_blend_crouch: Option<usize>,
}

/// A CS:GO animation set and the sequences the animation state looks up.
pub struct Animset {
    pub model: StudioModel,
    params: Params,
    lean: Option<usize>,
    recrouch: Option<usize>,
}

impl Animset {
    pub fn new(model: StudioModel) -> Self {
        let p = |name: &str| model.pose_param(name);
        let params = Params {
            move_yaw: p("move_yaw"),
            body_yaw: p("body_yaw"),
            body_pitch: p("body_pitch"),
            speed: p("speed"),
            stand: p("stand"),
            jump_fall: p("jump_fall"),
            lean_yaw: p("lean_yaw"),
            aim_stand_idle: p("aim_blend_stand_idle"),
            aim_stand_walk: p("aim_blend_stand_walk"),
            aim_stand_run: p("aim_blend_stand_run"),
            aim_crouch_idle: p("aim_blend_crouch_idle"),
            aim_crouch_walk: p("aim_blend_crouch_walk"),
            move_blend_walk: p("move_blend_walk"),
            move_blend_run: p("move_blend_run"),
            move_blend_crouch: p("move_blend_crouch"),
        };
        Self {
            lean: model.sequence_by_label("lean"),
            recrouch: model.sequence_by_label("recrouch_generic"),
            params,
            model,
        }
    }

    /// Playback rate of `sequence` in cycles per second (`Studio_CPS`).
    fn cycle_rate(&self, sequence: usize) -> f32 {
        let duration = self
            .model
            .sequences
            .get(sequence)
            .map_or(0.0, mdl_source::studio::Sequence::duration);
        if duration > 0.0 { 1.0 / duration } else { 10.0 }
    }

    fn activity(&self, sequence: Option<usize>) -> &str {
        sequence
            .and_then(|s| self.model.sequences.get(s))
            .map_or("", |s| s.activity.as_str())
    }

    fn has_act_mod(&self, sequence: Option<usize>, name: &str) -> bool {
        sequence
            .and_then(|s| self.model.sequences.get(s))
            .is_some_and(|s| s.act_mods.iter().any(|m| m == name))
    }

    /// The sequence playing `activity` that best fits `mods` (`SelectWeightedSequenceFromModifiers`):
    /// each modifier it has that is wanted counts two, each one not wanted costs one; a tie is
    /// drawn at random.
    fn select(&self, activity: &str, mods: &[&str], roll: u32) -> Option<usize> {
        let mut top = i32::MIN;
        let mut best = Vec::new();
        for (index, seq) in self.model.sequences.iter().enumerate() {
            if !seq.activity.eq_ignore_ascii_case(activity) {
                continue;
            }
            let score: i32 = seq
                .act_mods
                .iter()
                .map(|m| if mods.contains(&m.as_str()) { 2 } else { -1 })
                .sum();
            if score > top {
                top = score;
                best.clear();
            }
            if score == top {
                best.push(index);
            }
        }
        (!best.is_empty()).then(|| best[roll as usize % best.len()])
    }
}

/// One animation layer (`CAnimationLayer`).
#[derive(Clone, Copy, Default)]
pub struct Layer {
    pub sequence: Option<usize>,
    pub cycle: f32,
    pub rate: f32,
    pub weight: f32,
    pub weight_rate: f32,
}

/// How long an aim matrix's state has held, and its blend (`aimmatrix_transition_t`).
#[derive(Clone, Copy, Default)]
struct Transition {
    valid_for: f32,
    invalid_for: f32,
    blend_in_wait: f32,
    blend_out_wait: f32,
    blend: f32,
}

impl Transition {
    fn new(blend_in_wait: f32, blend_out_wait: f32) -> Self {
        Self {
            blend_in_wait,
            blend_out_wait,
            ..Self::default()
        }
    }

    fn update(&mut self, valid: bool, dt: f32, speed: f32) {
        if valid {
            self.invalid_for = 0.0;
            self.valid_for += dt;
            if self.valid_for >= self.blend_in_wait {
                self.blend = approach(1.0, self.blend, speed);
            }
        } else {
            self.valid_for = 0.0;
            self.invalid_for += dt;
            if self.invalid_for >= self.blend_out_wait {
                self.blend = approach(0.0, self.blend, speed);
            }
        }
    }
}

/// What the game says about a player this frame.
pub struct Input {
    pub position: Vec3,
    pub eye_yaw: f32,
    pub eye_pitch: f32,
    pub velocity: Vec3,
    pub duck: f32,
    pub on_ground: bool,
    pub walking: bool,
    pub scoped: bool,
    pub max_speed: f32,
    pub class: &'static str,
    /// Now, and when a flash's raised arm starts down and is down (server ms).
    pub time_ms: i32,
    pub flash_arm_ms: (i32, i32),
}

/// Something the player did that starts an animation (`PlayerAnimEvent_t`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Fire,
    Operate,
    SilencerAttach,
    SilencerDetach,
    Reload,
    ReloadStart,
    ReloadLoop,
    ReloadEnd,
    Deploy,
    Jump,
}

/// One player's CS:GO animation state (`CCSGOPlayerAnimState`).
#[derive(Clone)]
pub struct AnimState {
    pub ct: bool,
    pub first_run: bool,
    pub time: f32,
    pub dt: f32,
    pub seed: u32,
    pub params: Vec<f32>,
    pub layers: [Layer; LAYER_COUNT],
    pub class: &'static str,
    pub eye_yaw: f32,
    pub eye_pitch: f32,
    pub foot_yaw: f32,
    pub move_yaw: f32,
    pub move_yaw_ideal: f32,
    pub lower_body_target: f32,
    pub lower_body_realign_at: f32,
    pub duck: f32,
    pub duck_additional: f32,
    pub position: Vec3,
    pub velocity: Vec3,
    pub velocity_dir: Vec3,
    pub velocity_z: f32,
    pub speed: f32,
    pub portion_run: f32,
    pub portion_walk: f32,
    pub portion_crouch: f32,
    pub duration_moving: f32,
    pub walk_to_run: f32,
    pub to_run: bool,
    stand_walk: Transition,
    stand_run: Transition,
    crouch_walk: Transition,
    pub primary_cycle: f32,
    pub move_weight: f32,
    pub on_ground: bool,
    pub landing: bool,
    pub jumping: bool,
    pub in_air_smooth: f32,
    pub duration_in_air: f32,
    pub left_ground_height: f32,
    pub land_multiplier: f32,
    pub velocity_last: Vec3,
    pub velocity_test_at: f32,
    pub accel: Vec3,
    pub target_accel: Vec3,
    // What the game showed last frame, to see events in.
    pub seen_weapon: u32,
    pub seen_fire_ms: i32,
    pub seen_weaponstate: i32,
    pub seen_grenade: u32,
    pub seen_silencers: u32,
    /// The weapon class the idle loop was chosen for.
    pub alive_class: &'static str,
    /// The flashed reaction is up (`m_bFlashed`).
    pub flashed: bool,
}

impl AnimState {
    pub fn new(animset: &Animset, ct: bool, seed: u32) -> Self {
        Self {
            ct,
            first_run: true,
            time: 0.0,
            dt: 0.0,
            seed: seed.wrapping_mul(2_654_435_761) | 1,
            params: animset.model.default_pose_params(),
            layers: [Layer::default(); LAYER_COUNT],
            class: "rifle",
            eye_yaw: 0.0,
            eye_pitch: 0.0,
            foot_yaw: 0.0,
            move_yaw: 0.0,
            move_yaw_ideal: 0.0,
            lower_body_target: 0.0,
            lower_body_realign_at: 0.0,
            duck: 0.0,
            duck_additional: 0.0,
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            velocity_dir: Vec3::X,
            velocity_z: 0.0,
            speed: 0.0,
            portion_run: 0.0,
            portion_walk: 0.0,
            portion_crouch: 0.0,
            duration_moving: 0.0,
            walk_to_run: 0.0,
            to_run: true,
            stand_walk: Transition::new(0.4, 0.2),
            stand_run: Transition::new(0.2, 0.4),
            crouch_walk: Transition::new(0.3, 0.3),
            primary_cycle: 0.0,
            move_weight: 0.0,
            on_ground: true,
            landing: false,
            jumping: false,
            in_air_smooth: 0.0,
            duration_in_air: 0.0,
            left_ground_height: 0.0,
            land_multiplier: 1.0,
            velocity_last: Vec3::ZERO,
            velocity_test_at: 0.0,
            accel: Vec3::ZERO,
            target_accel: Vec3::ZERO,
            seen_weapon: 0,
            seen_fire_ms: 0,
            seen_weaponstate: 0,
            seen_grenade: 0,
            seen_silencers: 0,
            alive_class: "",
            flashed: false,
        }
    }

    fn roll(&mut self) -> u32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed
    }

    fn random(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * (self.roll() % 10_000) as f32 / 10_000.0
    }

    fn set_param(&mut self, animset: &Animset, param: Option<usize>, value: f32) {
        let Some(index) = param else {
            return;
        };
        let Some(desc) = animset.model.pose_params.get(index) else {
            return;
        };
        let (low, high) = (desc.start.min(desc.end), desc.start.max(desc.end));
        if let Some(slot) = self.params.get_mut(index) {
            *slot = value.clamp(low, high);
        }
    }

    fn set_sequence(&mut self, animset: &Animset, layer: usize, sequence: Option<usize>) {
        if let Some(sequence) = sequence {
            self.layers[layer] = Layer {
                sequence: Some(sequence),
                cycle: 0.0,
                rate: animset.cycle_rate(sequence),
                weight: 0.0,
                weight_rate: self.layers[layer].weight_rate,
            };
        }
    }

    fn increment_cycle(&mut self, layer: usize, allow_loop: bool) {
        let l = &mut self.layers[layer];
        if l.rate.abs() <= 0.0 {
            return;
        }
        let mut cycle = l.cycle + self.dt * l.rate;
        if !allow_loop && cycle >= 1.0 {
            cycle = 0.999;
        }
        l.cycle = clamp_cycle(cycle);
    }

    fn increment_weight(&mut self, layer: usize) {
        let l = &mut self.layers[layer];
        if l.weight_rate.abs() <= 0.0 {
            return;
        }
        l.weight = (l.weight + self.dt * l.weight_rate).clamp(0.0, 1.0);
    }

    fn set_weight(&mut self, layer: usize, weight: f32) {
        self.layers[layer].weight = weight.clamp(0.0, 1.0);
    }

    fn set_weight_rate(&mut self, layer: usize, previous: f32) {
        if self.dt > 0.0 {
            self.layers[layer].weight_rate = (self.layers[layer].weight - previous) / self.dt;
        }
    }

    fn completed(&self, layer: usize) -> bool {
        let l = &self.layers[layer];
        l.cycle + self.dt * l.rate >= 1.0
    }

    /// The weight a layer's sequence wants at its cycle, easing in and out (`fadein`/`fadeout`).
    fn ideal_weight(&self, animset: &Animset, layer: usize) -> f32 {
        let l = &self.layers[layer];
        let Some(seq) = l.sequence.and_then(|s| animset.model.sequences.get(s)) else {
            return 0.0;
        };
        let cycle = if l.cycle >= 0.999 { 1.0 } else { l.cycle };
        let mut weight = 1.0;
        if seq.fade_in > 0.0 && cycle < seq.fade_in {
            weight = smoothstep_bounds(0.0, seq.fade_in, cycle);
        } else if seq.fade_out < 1.0 && cycle > seq.fade_out {
            weight = smoothstep_bounds(1.0, seq.fade_out, cycle);
        }
        if weight < 0.0015 { 0.0 } else { weight.clamp(0.0, 1.0) }
    }

    fn update_layer(&mut self, layer: usize, sequence: Option<usize>, rate: f32, weight: f32, cycle: f32) {
        if let Some(sequence) = sequence {
            self.layers[layer] = Layer {
                sequence: Some(sequence),
                rate,
                cycle: cycle.clamp(0.0, 1.0),
                weight: weight.clamp(0.0, 1.0),
                weight_rate: self.layers[layer].weight_rate,
            };
        }
    }

    /// The activity modifiers events choose sequences by (`UpdateActivityModifiers`).
    fn mods(&self) -> Vec<&'static str> {
        let mut mods = vec![self.class];
        if self.portion_walk > 0.25 {
            mods.push("moving");
        }
        if self.duck > 0.55 {
            mods.push("crouch");
        }
        mods
    }

    fn select(&mut self, animset: &Animset, activity: &str) -> Option<usize> {
        let mods = self.mods();
        let roll = self.roll();
        animset.select(activity, &mods, roll)
    }

    /// `DoAnimationEvent`.
    pub fn event(&mut self, animset: &Animset, event: Event) {
        let (layer, activity) = match event {
            Event::Fire => (WEAPON_ACTION, "ACT_CSGO_FIRE_PRIMARY"),
            Event::Operate => (WEAPON_ACTION, "ACT_CSGO_OPERATE"),
            Event::SilencerAttach => (WEAPON_ACTION, "ACT_CSGO_SILENCER_ATTACH"),
            Event::SilencerDetach => (WEAPON_ACTION, "ACT_CSGO_SILENCER_DETACH"),
            // Shotguns load shell by shell instead.
            Event::Reload if self.class == "shotgun" => return,
            Event::Reload => (WEAPON_ACTION, "ACT_CSGO_RELOAD"),
            Event::ReloadStart => (WEAPON_ACTION, "ACT_CSGO_RELOAD_START"),
            Event::ReloadLoop => (WEAPON_ACTION, "ACT_CSGO_RELOAD_LOOP"),
            Event::ReloadEnd => (WEAPON_ACTION, "ACT_CSGO_RELOAD_END"),
            Event::Deploy => (WEAPON_ACTION, "ACT_CSGO_DEPLOY"),
            Event::Jump => {
                self.jumping = true;
                (JUMP_OR_FALL, "ACT_CSGO_JUMP")
            }
        };
        let sequence = self.select(animset, activity);
        self.set_sequence(animset, layer, sequence);
    }

    /// One frame (`Update`): every layer's sequence, cycle and weight, and the pose parameters.
    pub fn update(&mut self, animset: &Animset, input: &Input, dt: f32) {
        self.dt = dt.max(0.0);
        self.time += self.dt;
        self.eye_yaw = angle_normalize(input.eye_yaw);
        self.eye_pitch = angle_normalize(input.eye_pitch);
        self.position = input.position;
        if self.class != input.class {
            self.class = input.class;
        }
        self.duck = approach(
            (input.duck + self.duck_additional).clamp(0.0, 1.0),
            self.duck,
            self.dt * DUCK_APPROACH_SPEED,
        )
        .clamp(0.0, 1.0);
        self.velocity_phase(animset, input);
        self.aim_matrix(animset, input);
        self.weapon_action(animset);
        self.movement(animset, input);
        self.alive_loop(animset);
        self.flashed_reaction(animset, input);
        self.lean(animset, input);
        self.first_run = false;
    }

    /// `SetUpVelocity`: speed, the feet's yaw, move yaw and the aim matrix's yaw and pitch.
    fn velocity_phase(&mut self, animset: &Animset, input: &Input) {
        let p = animset.params;
        let dt = self.dt;
        let mut velocity = input.velocity;
        let limit = SPEED_RUN * MAX_VEL_LIMIT;
        if velocity.length_squared() > limit * limit {
            velocity = velocity.normalize() * limit;
        }
        self.velocity_z = velocity.z;
        velocity.z = 0.0;
        self.velocity = approach_vec(velocity, self.velocity, dt * 2000.0);
        self.speed = self.velocity.length().min(SPEED_RUN);
        if self.speed > 0.0 {
            self.velocity_dir = self.velocity.normalize_or(self.velocity_dir);
        }
        let max_run = input.max_speed.max(0.001);
        self.portion_run = (self.speed / max_run).clamp(0.0, 1.0);
        self.portion_walk = self.speed / (max_run * SPEED_WALK_MODIFIER);
        self.portion_crouch = self.speed / (max_run * SPEED_DUCK_MODIFIER);
        let started_moving = self.speed > 0.0 && self.duration_moving <= 0.0;
        if self.speed > 0.0 {
            self.duration_moving += dt;
        } else {
            self.duration_moving = 0.0;
        }

        // The feet stay put while the eyes look around within the aim matrix's width (narrower
        // on the move), and turn to them once moving or after a while.
        self.foot_yaw = self.foot_yaw.clamp(-360.0, 360.0);
        let eye_foot = angle_diff(self.eye_yaw, self.foot_yaw);
        let mut width = lerp(
            self.portion_walk.clamp(0.0, 1.0),
            1.0,
            lerp(self.walk_to_run, AIM_NARROW_WALK, AIM_NARROW_RUN),
        );
        if self.duck > 0.0 {
            width = lerp(
                self.duck * self.portion_crouch.clamp(0.0, 1.0),
                width,
                AIM_NARROW_CROUCHMOVING,
            );
        }
        let (yaw_max, yaw_min) = (AIM_YAW_MAX * width, AIM_YAW_MIN * width);
        if eye_foot > yaw_max {
            self.foot_yaw = self.eye_yaw - yaw_max.abs();
        } else if eye_foot < yaw_min {
            self.foot_yaw = self.eye_yaw + yaw_min.abs();
        }
        self.foot_yaw = angle_normalize(self.foot_yaw);
        if self.speed > 0.1 || self.velocity_z.abs() > 100.0 {
            self.foot_yaw = approach_angle(
                self.eye_yaw,
                self.foot_yaw,
                dt * (30.0 + 20.0 * self.walk_to_run),
            );
            self.lower_body_realign_at = self.time + LOWER_REALIGN_DELAY * 0.2;
            self.lower_body_target = self.eye_yaw;
        } else {
            self.foot_yaw = approach_angle(self.lower_body_target, self.foot_yaw, dt * LOWER_CATCHUP_IDLE);
            if self.time > self.lower_body_realign_at
                && angle_diff(self.foot_yaw, self.eye_yaw).abs() > 35.0
            {
                self.lower_body_realign_at = self.time + LOWER_REALIGN_DELAY;
                self.lower_body_target = self.eye_yaw;
            }
        }

        // Which way the legs move, relative to the feet (180 forward).
        if self.speed > 0.0 {
            let mut raw = (-self.velocity.y).atan2(-self.velocity.x).to_degrees();
            if raw < 0.0 {
                raw += 360.0;
            }
            self.move_yaw_ideal = angle_normalize(angle_diff(raw, self.foot_yaw));
        }
        let to_ideal = angle_normalize(angle_diff(self.move_yaw_ideal, self.move_yaw));
        if started_moving && self.move_weight <= 0.0 {
            self.move_yaw = self.move_yaw_ideal;
        } else {
            let weight = lerp(
                self.duck,
                self.portion_walk.clamp(0.0, 1.0),
                self.portion_crouch.clamp(0.0, 1.0),
            );
            let ratio = bias(weight, 0.18) + 0.1;
            self.move_yaw = angle_normalize(self.move_yaw + to_ideal * ratio);
        }
        self.set_param(animset, p.move_yaw, self.move_yaw);

        // The aim matrix's yaw and pitch (its limits are CS:GO's defaults, ±58° and ±90°, so
        // no rescaling).
        self.set_param(animset, p.body_yaw, angle_diff(self.eye_yaw, self.foot_yaw));
        self.set_param(animset, p.body_pitch, angle_diff(self.eye_pitch, 0.0));
        self.set_param(animset, p.speed, self.portion_walk);
        self.set_param(animset, p.stand, 1.0 - self.duck * self.in_air_smooth);
    }

    /// `SetUpAimMatrix`: the full-body aim poses, cross-faded between standing, walking,
    /// running and crouched as the player's speed holds for a moment.
    fn aim_matrix(&mut self, animset: &Animset, input: &Input) {
        let p = animset.params;
        if self.duck <= 0.0 || self.duck >= 1.0 {
            let speed = self.dt
                * if input.scoped {
                    SPEED_TO_CHANGE_AIM_MATRIX_SCOPED
                } else {
                    SPEED_TO_CHANGE_AIM_MATRIX
                };
            if input.scoped {
                for t in [&mut self.stand_walk, &mut self.stand_run, &mut self.crouch_walk] {
                    t.invalid_for = t.blend_out_wait;
                }
            }
            let dt = self.dt;
            self.stand_walk.update(
                input.walking && !input.scoped && self.portion_walk > 0.7 && self.portion_run < 0.7,
                dt,
                speed,
            );
            self.stand_run
                .update(!input.scoped && self.portion_run >= 0.7, dt, speed);
            self.crouch_walk
                .update(!input.scoped && self.portion_crouch >= 0.5, dt, speed);
        }
        let mut stand_idle = 1.0;
        let mut stand_walk = self.stand_walk.blend;
        let mut stand_run = self.stand_run.blend;
        let mut crouch_idle = 1.0;
        let mut crouch_walk = self.crouch_walk.blend;
        if stand_walk >= 1.0 {
            stand_idle = 0.0;
        }
        if stand_run >= 1.0 {
            stand_idle = 0.0;
            stand_walk = 0.0;
        }
        if crouch_walk >= 1.0 {
            crouch_idle = 0.0;
        }
        if self.duck >= 1.0 {
            stand_idle = 0.0;
            stand_walk = 0.0;
            stand_run = 0.0;
        } else if self.duck <= 0.0 {
            crouch_idle = 0.0;
            crouch_walk = 0.0;
        }
        crouch_idle *= self.duck;
        crouch_walk *= self.duck;
        stand_walk *= 1.0 - self.duck;
        stand_run *= 1.0 - self.duck;
        if crouch_idle < 1.0 && crouch_walk < 1.0 && stand_walk < 1.0 && stand_run < 1.0 {
            stand_idle = 1.0;
        }
        self.set_param(animset, p.aim_stand_idle, stand_idle);
        self.set_param(animset, p.aim_stand_walk, stand_walk);
        self.set_param(animset, p.aim_stand_run, stand_run);
        self.set_param(animset, p.aim_crouch_idle, crouch_idle);
        self.set_param(animset, p.aim_crouch_walk, crouch_walk);
        let aim = animset.model.sequence_by_label(&format!("{}_aim", self.class));
        self.update_layer(AIMMATRIX, aim, 0.0, 1.0, 0.0);
    }

    /// `SetUpWeaponAction`: firing, reloading, deploying; re-crouching a standing action.
    fn weapon_action(&mut self, animset: &Animset) {
        let action = self.layers[WEAPON_ACTION];
        let mut recrouch = 0.0;
        if action.weight > 0.0 {
            if self.layers[WEAPON_ACTION_RECROUCH].sequence.is_none() {
                self.layers[WEAPON_ACTION_RECROUCH].sequence = animset.recrouch;
            }
            if animset.has_act_mod(action.sequence, "crouch") {
                if self.duck < 1.0 {
                    recrouch = action.weight * (1.0 - self.duck);
                }
            } else if self.duck > 0.0 {
                recrouch = action.weight * self.duck;
            }
        } else if self.layers[WEAPON_ACTION_RECROUCH].weight > 0.0 {
            recrouch = approach(0.0, self.layers[WEAPON_ACTION_RECROUCH].weight, self.dt * 4.0);
        }
        self.set_weight(WEAPON_ACTION_RECROUCH, recrouch);
        // CS:GO steps the action's cycle twice a frame.
        self.increment_cycle(WEAPON_ACTION, false);
        let previous = self.layers[WEAPON_ACTION].weight;
        let ideal = self.ideal_weight(animset, WEAPON_ACTION);
        self.set_weight(WEAPON_ACTION, ideal);
        self.set_weight_rate(WEAPON_ACTION, previous);
        if self.layers[WEAPON_ACTION].weight > 0.0 {
            self.increment_cycle(WEAPON_ACTION, false);
            self.increment_weight(WEAPON_ACTION);
        }
    }

    /// `SetUpMovement`: the locomotion cycle and its weight, jumping, falling, landing.
    fn movement(&mut self, animset: &Animset, input: &Input) {
        let p = animset.params;
        let dt = self.dt;
        if self.walk_to_run > 0.0 && self.walk_to_run < 1.0 {
            let step = dt * WALK_TO_RUN_TRANSITION_SPEED;
            self.walk_to_run = (self.walk_to_run + if self.to_run { step } else { -step }).clamp(0.0, 1.0);
        }
        let walk_threshold = SPEED_RUN * SPEED_WALK_MODIFIER;
        if self.speed > walk_threshold && !self.to_run {
            self.to_run = true;
            self.walk_to_run = self.walk_to_run.max(0.01);
        } else if self.speed < walk_threshold && self.to_run {
            self.to_run = false;
            self.walk_to_run = self.walk_to_run.min(0.99);
        }
        self.set_param(animset, p.move_blend_walk, (1.0 - self.walk_to_run) * (1.0 - self.duck));
        self.set_param(animset, p.move_blend_run, self.walk_to_run * (1.0 - self.duck));
        self.set_param(animset, p.move_blend_crouch, self.duck);

        let move_seq = animset
            .model
            .sequence_by_label(&format!("move_{}", self.class))
            .or_else(|| animset.model.sequence_by_label("move"));
        let target = lerp(
            self.duck,
            self.portion_walk.clamp(0.0, 1.0),
            self.portion_crouch.clamp(0.0, 1.0),
        );
        if self.move_weight <= target {
            self.move_weight = target;
        } else {
            self.move_weight = approach(target, self.move_weight, dt * 2.0);
        }
        let yaw = (self.foot_yaw + self.move_yaw + 180.0).to_radians();
        let move_dir = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
        self.move_weight *= bias(self.velocity_dir.dot(move_dir).abs(), 0.2);
        let mut weight = self.move_weight * self.in_air_smooth;
        weight *= (1.0 - self.layers[LAND_OR_CLIMB].weight).max(0.55);
        let mut rate = 0.0;
        if let Some(seq) = move_seq.filter(|_| self.speed > 0.0) {
            // The cycle covers as much ground as the player does.
            let stride = Vec3::from(mdl_source::blend::sequence_movement(&animset.model, seq, &self.params))
                .length();
            rate = if stride > 0.001 {
                self.speed / stride
            } else {
                animset.cycle_rate(seq)
            };
            rate *= lerp(self.walk_to_run, 1.0, RUN_ANIM_PLAYBACK_MULTIPLIER);
        }
        let increment = rate * dt;
        self.primary_cycle = clamp_cycle(self.primary_cycle + increment);
        self.update_layer(MOVE, move_seq, increment, weight.clamp(0.0, 1.0), self.primary_cycle);

        let was_on_ground = self.on_ground;
        self.on_ground = input.on_ground;
        let landed = !self.first_run && !was_on_ground && self.on_ground;
        let left_ground = was_on_ground && !self.on_ground;
        if left_ground {
            self.left_ground_height = self.position.z;
        }
        if landed {
            let fell = (self.left_ground_height - self.position.z).abs();
            let fall = bias(remap_clamped(fell, 12.0, 72.0, 0.0, 1.0), 0.4);
            self.land_multiplier = bias(self.duration_in_air, 0.3).clamp(0.1, 1.0);
            self.duck_additional = self.land_multiplier.max(fall);
        } else {
            self.duck_additional = approach(0.0, self.duck_additional, dt * 2.0);
        }
        self.in_air_smooth = approach(
            if self.on_ground { 1.0 } else { 0.0 },
            self.in_air_smooth,
            lerp(self.duck, ONGROUND_FUZZY_APPROACH, ONGROUND_FUZZY_APPROACH_CROUCH) * dt,
        )
        .clamp(0.0, 1.0);

        if self.on_ground {
            if !self.landing && landed {
                let activity = if self.duration_in_air > 1.0 {
                    "ACT_CSGO_LAND_HEAVY"
                } else {
                    "ACT_CSGO_LAND_LIGHT"
                };
                let seq = self.select(animset, activity);
                self.set_sequence(animset, LAND_OR_CLIMB, seq);
                self.layers[LAND_OR_CLIMB].cycle = 0.0;
                self.landing = true;
            }
            self.duration_in_air = 0.0;
            if self.landing {
                self.jumping = false;
                self.increment_cycle(LAND_OR_CLIMB, false);
                self.increment_cycle(JUMP_OR_FALL, false);
                self.set_param(animset, p.jump_fall, 0.0);
                if self.completed(LAND_OR_CLIMB) {
                    self.landing = false;
                    self.set_weight(LAND_OR_CLIMB, 0.0);
                    self.set_weight(JUMP_OR_FALL, 0.0);
                    self.land_multiplier = 1.0;
                } else {
                    let land = self.ideal_weight(animset, LAND_OR_CLIMB)
                        * self.land_multiplier
                        * (1.0 - self.duck).clamp(0.2, 1.0);
                    self.set_weight(LAND_OR_CLIMB, land);
                    let jump = self.layers[JUMP_OR_FALL].weight;
                    if jump > 0.0 {
                        self.set_weight(JUMP_OR_FALL, approach(0.0, jump, dt * 10.0));
                    }
                }
            }
            if !self.landing && !self.jumping {
                self.set_weight(LAND_OR_CLIMB, 0.0);
            }
        } else {
            self.landing = false;
            if left_ground {
                if !self.jumping {
                    let seq = self.select(animset, "ACT_CSGO_FALL");
                    self.set_sequence(animset, JUMP_OR_FALL, seq);
                }
                self.duration_in_air = 0.0;
            }
            self.duration_in_air += dt;
            self.increment_cycle(JUMP_OR_FALL, false);
            let next = self.ideal_weight(animset, JUMP_OR_FALL);
            if next > self.layers[JUMP_OR_FALL].weight {
                self.set_weight(JUMP_OR_FALL, next);
            }
            let lingering = self.layers[LAND_OR_CLIMB].weight;
            if lingering > 0.0 {
                self.set_weight(
                    LAND_OR_CLIMB,
                    lingering * smoothstep_bounds(0.2, 0.0, self.duration_in_air),
                );
            }
            self.set_param(
                animset,
                p.jump_fall,
                smoothstep_bounds(0.72, 1.52, self.duration_in_air).clamp(0.0, 1.0),
            );
        }
    }

    /// `SetUpAliveloop`: breathing and fidgeting, faded out when running.
    fn alive_loop(&mut self, animset: &Animset) {
        if animset.activity(self.layers[ALIVELOOP].sequence) != "ACT_CSGO_ALIVE_LOOP" {
            let seq = self.select(animset, "ACT_CSGO_ALIVE_LOOP");
            self.set_sequence(animset, ALIVELOOP, seq);
            self.layers[ALIVELOOP].cycle = self.random(0.0, 1.0);
            if let Some(seq) = self.layers[ALIVELOOP].sequence {
                self.layers[ALIVELOOP].rate = animset.cycle_rate(seq) * self.random(0.8, 1.1);
            }
            self.alive_class = self.class;
        } else if self.alive_class != self.class {
            // A new weapon's own loop, carrying on where the old one was.
            let cycle = self.layers[ALIVELOOP].cycle;
            let seq = self.select(animset, "ACT_CSGO_ALIVE_LOOP");
            self.set_sequence(animset, ALIVELOOP, seq);
            self.layers[ALIVELOOP].cycle = cycle;
            self.alive_class = self.class;
        } else if self.completed(ALIVELOOP) {
            if let Some(seq) = self.layers[ALIVELOOP].sequence {
                self.layers[ALIVELOOP].rate = animset.cycle_rate(seq) * self.random(0.8, 1.1);
            }
        } else {
            let weight = remap_clamped(self.portion_run, 0.55, 0.9, 1.0, 0.0);
            self.set_weight(ALIVELOOP, weight);
        }
        self.increment_cycle(ALIVELOOP, true);
    }

    /// `SetUpLean`: the body leans into acceleration.
    fn lean(&mut self, animset: &Animset, input: &Input) {
        let interval = self.time - self.velocity_test_at;
        if interval > 0.025 {
            let interval = interval.min(0.1);
            self.velocity_test_at = self.time;
            self.target_accel = (input.velocity - self.velocity_last) / interval;
            self.target_accel.z = 0.0;
            self.velocity_last = input.velocity;
        }
        self.accel = approach_vec(self.target_accel, self.accel, self.dt * 800.0);
        let accel_yaw = self.accel.y.atan2(self.accel.x).to_degrees();
        let weight = (self.accel.length() / SPEED_RUN * self.portion_run).clamp(0.0, 1.0);
        self.set_param(animset, animset.params.lean_yaw, angle_normalize(self.foot_yaw - accel_yaw));
        if self.layers[LEAN].sequence.is_none() {
            self.set_sequence(animset, LEAN, animset.lean);
        }
        self.set_weight(LEAN, weight);
    }

    /// The animation set's local transforms for this frame: the rest pose with every layer
    /// played over it in order.
    /// `SetUpFlashedReaction`: a flashed player shields their eyes, the arm held up until the
    /// flash's ease-out starts, then lowered (a spline) until it ends.
    fn flashed_reaction(&mut self, animset: &Animset, input: &Input) {
        let (from, until) = input.flash_arm_ms;
        if until < input.time_ms {
            self.set_weight(FLASHED, 0.0);
            self.flashed = false;
            return;
        }
        if !self.flashed {
            let seq = self.select(animset, "ACT_CSGO_FLASHBANG_REACTION");
            self.set_sequence(animset, FLASHED, seq);
            self.flashed = true;
        }
        let amount = remap_clamped(input.time_ms as f32, from as f32, until as f32, 1.0, 0.0);
        self.layers[FLASHED].cycle = 0.0;
        self.layers[FLASHED].rate = 0.0;
        let previous = self.layers[FLASHED].weight;
        let weight = amount * amount * (3.0 - 2.0 * amount);
        self.set_weight(FLASHED, weight);
        self.layers[FLASHED].weight_rate = if weight >= previous { 0.0 } else { previous };
    }

    pub fn pose(&self, animset: &Animset, out: &mut Vec<BoneFrame>) {
        out.clear();
        out.extend_from_slice(&animset.model.rest);
        for layer in &self.layers {
            let Some(sequence) = layer.sequence.filter(|_| layer.weight > 0.0) else {
                continue;
            };
            mdl_source::blend::accumulate(
                &animset.model,
                out,
                sequence,
                layer.cycle,
                layer.weight,
                &self.params,
            );
        }
    }
}


/// What a player shows this frame, for [`AnimState::observe`].
pub struct Observation<'a> {
    pub ps: &'a PlayerState,
    /// Where the body stands and looks: a drawn player's interpolated place, the server's own.
    pub position: Vec3,
    pub eye_yaw: f32,
    pub eye_pitch: f32,
    /// The script name of the weapon in hand (`ak47_mp`).
    pub weapon_name: &'a str,
}

impl AnimState {
    /// One frame of a player: the animation events their state shows since the frame before
    /// (`DoAnimationEvent`), seen with this frame's speed and crouch as CS:GO's are, then
    /// [`update`](Self::update).
    pub fn observe(&mut self, animset: &Animset, seen: &Observation<'_>, dt: f32) {
        let ps = seen.ps;
        let weapon = ps.weapon;
        let cs_index = weapon_iw4::cs::cs_weapon_index_for(seen.weapon_name);
        let class = cs_index.map_or("knife", weapon_iw4::cs::csgo_anim_class);
        let max_speed = cs_index
            .and_then(weapon_iw4::cs::cs_weapon)
            .map_or(250.0, |gun| gun.max_speed);
        let input = Input {
            position: seen.position,
            eye_yaw: seen.eye_yaw,
            eye_pitch: seen.eye_pitch,
            velocity: Vec3::from(ps.velocity),
            duck: ps.cs_duck_amount.clamp(0.0, 1.0),
            on_ground: ps.ground_entity_num != ENTITYNUM_NONE,
            walking: ps.cs_move_buttons & 0x2 != 0,
            scoped: ps.cs_zoom != 0,
            max_speed,
            class,
            // The player's own clock: the time of the last command they ran.
            time_ms: ps.command_time,
            flash_arm_ms: (ps.cs_flash_arm_from_ms, ps.cs_flash_arm_until_ms),
        };
        let mut events = Vec::new();
        if !self.first_run {
            if weapon != self.seen_weapon {
                events.push(Event::Deploy);
            } else if ps.cs_last_fire_ms != self.seen_fire_ms && ps.cs_last_fire_ms != 0 {
                events.push(Event::Fire);
            }
            let reloading = |s: i32| s == weapon_iw4::WeaponState::Reloading as i32;
            let state_now = ps.weaponstate_primary;
            if state_now != self.seen_weaponstate {
                if state_now == weapon_iw4::WeaponState::ReloadStart as i32 {
                    events.push(Event::ReloadStart);
                } else if reloading(state_now) {
                    events.push(if class == "shotgun" { Event::ReloadLoop } else { Event::Reload });
                } else if state_now == weapon_iw4::WeaponState::ReloadEnd as i32 {
                    events.push(Event::ReloadEnd);
                }
            }
            if ps.cs_grenade == cs_grenade::PULLED && self.seen_grenade != cs_grenade::PULLED {
                events.push(Event::Operate);
            }
            // The silencer screwed on or off the gun in hand.
            if let Some(index) = cs_index.filter(|_| weapon == self.seen_weapon) {
                let bit = weapon_iw4::cs::silencer_bit(index);
                if (ps.cs_silencers ^ self.seen_silencers) & bit != 0 {
                    let on = weapon_iw4::cs::silencer_on(ps.cs_silencers, bit, ps.cs_shooting_mode);
                    events.push(if on { Event::SilencerAttach } else { Event::SilencerDetach });
                }
            }
            // A take-off: rising fast just after leaving the ground (the first airborne state
            // can come before the jump's speed does).
            if !input.on_ground
                && !self.jumping
                && input.velocity.z > 100.0
                && (self.on_ground || self.duration_in_air < 0.2)
            {
                events.push(Event::Jump);
            }
        }
        self.seen_weapon = weapon;
        self.seen_fire_ms = ps.cs_last_fire_ms;
        self.seen_weaponstate = ps.weaponstate_primary;
        self.seen_grenade = ps.cs_grenade;
        self.seen_silencers = ps.cs_silencers;
        self.class = class;
        for event in events {
            self.event(animset, event);
        }
        self.update(animset, &input, dt);
    }
}
