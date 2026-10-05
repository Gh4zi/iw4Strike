//! Source engine player movement, as Momentum Mod runs it for CS-style modes, on IW4 collision.
//!
//! The order of a command follows Momentum's `FullWalkMove`: half gravity, jump, ground friction,
//! walk (with step-up and stay-on-ground) or air move, ground categorisation, the other half of
//! gravity. Ground categorisation carries Momentum's slope fix (landing on a downhill keeps the
//! speed the slope would give) and its quadrant probe for ledges; the slide move clips with no
//! overbounce, as surfing needs. Numbers live in [`SourceProfile`]: [`CSGO`] is the competitive
//! default, [`MOMENTUM_BHOP`] and [`MOMENTUM_SURF`] are Momentum's mode settings.
//!
//! Source keeps the player origin at the feet, like IW4, so hull heights map one to one.

use playerstate_iw4::{ENTITYNUM_NONE, PlayerState, UserCmd, buttons, cs_duck, eflags, pm_flags};
use trace_iw4::Trace;

use crate::{
    CollisionBackend, GroundTraceInput, LadderAttachBackend, LadderTraceHit,
    MoveBounds, Pml, PmoveResult, PmoveSingleContext, check_ladder_move, drop_timers,
    footstep_event, footsteps_bob_cycle, jump, ladder_footsteps, should_make_footsteps,
    update_ads_frac, update_ads_intent, update_view_angles,
};

/// CS:S stamina as Momentum's KZ mode keeps it: a jump costs stamina that recovers over time,
/// lowering the next jump and bleeding ground speed while it lasts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stamina {
    pub max: f32,
    pub jump_cost: f32,
    pub recover_rate: f32,
    /// Frame time the ground slowdown was tuned at (GoldSrc ran it per frame).
    pub reference_frametime: f32,
}

/// Tunables of one Source ruleset. Units are inches and seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceProfile {
    /// Speed cap without a weapon override (knife speed).
    pub max_speed: f32,
    /// Client key speeds (`cl_forwardspeed`, `cl_backspeed`, `cl_sidespeed`) for a full stick.
    pub forward_speed: f32,
    pub back_speed: f32,
    pub side_speed: f32,
    /// Walk key multiplier on the clamped command.
    pub walk_scale: f32,
    /// Command multiplier while ducking (`DUCK_SPEED_MULTIPLIER`).
    pub duck_scale: f32,
    pub accelerate: f32,
    pub air_accelerate: f32,
    /// Air wish speed cap (`GetAirSpeedCap`), the source of strafing and surfing.
    pub air_speed_cap: f32,
    pub friction: f32,
    pub stop_speed: f32,
    /// Upward speed a jump adds (`sqrt(2 * 800 * 57)` for CS:GO's 57 units).
    pub jump_impulse: f32,
    /// After a jump the player is lifted this far off the ground so the ground cannot re-catch
    /// them (Momentum's `sv_jump_z_offset`).
    pub jump_z_offset: f32,
    /// Holding jump hops again on landing.
    pub autohop: bool,
    /// Jumping above `factor * max_speed` scales velocity down to it (`PreventBunnyHopping`).
    pub bhop_cap_factor: Option<f32>,
    pub stamina: Option<Stamina>,
    pub stand_height: f32,
    pub duck_height: f32,
    pub stand_eye: f32,
    pub duck_eye: f32,
    /// Share of the hull difference the feet move when ducking or standing in the air.
    pub duck_air_shift: f32,
    pub duck_seconds: f32,
    pub unduck_seconds: f32,
    /// How far below the feet still counts as ground (`sv_considered_on_ground`).
    pub ground_probe: f32,
    /// Rising faster than this, the player is never on ground (`NON_JUMP_VELOCITY`).
    pub non_jump_velocity: f32,
    pub step_size: f32,
    /// `sv_maxvelocity`, per axis.
    pub max_velocity: f32,
    /// Momentum's slope fix: a landing that the ground would speed up keeps that speed.
    pub slope_fix: bool,
    /// Fall damage: none up to `fall_safe_speed`, 100 points per `fatal - safe`, scaled.
    pub fall_safe_speed: f32,
    pub fall_fatal_speed: f32,
    pub fall_damage_scale: f32,
}

/// CS:GO competitive movement on Momentum's Source code: CS:GO speeds, hull and eye heights, the
/// 57 unit jump, CS:S stamina, no autohop.
pub const CSGO: SourceProfile = SourceProfile {
    max_speed: 250.0,
    forward_speed: 450.0,
    back_speed: 450.0,
    side_speed: 450.0,
    walk_scale: 0.52,
    duck_scale: 0.34,
    accelerate: 5.5,
    air_accelerate: 12.0,
    air_speed_cap: 30.0,
    friction: 5.2,
    stop_speed: 80.0,
    jump_impulse: 301.993_38,
    jump_z_offset: 1.5,
    autohop: false,
    bhop_cap_factor: None,
    stamina: Some(Stamina {
        max: 100.0,
        jump_cost: 25.0,
        recover_rate: 19.0,
        reference_frametime: 1.0 / 70.0,
    }),
    stand_height: 72.0,
    duck_height: 54.0,
    stand_eye: 64.06,
    duck_eye: 46.04,
    duck_air_shift: 0.5,
    duck_seconds: 0.4,
    unduck_seconds: 0.2,
    ground_probe: 2.0,
    non_jump_velocity: 140.0,
    step_size: 18.0,
    max_velocity: 3500.0,
    slope_fix: true,
    fall_safe_speed: 580.0,
    fall_fatal_speed: 1024.0,
    fall_damage_scale: 1.25,
};

/// Momentum Mod's bhop mode on CS:GO's hull: autohop, airaccelerate 1000, no stamina.
pub const MOMENTUM_BHOP: SourceProfile = SourceProfile {
    max_speed: 260.0,
    accelerate: 5.0,
    air_accelerate: 1000.0,
    friction: 4.0,
    stop_speed: 75.0,
    autohop: true,
    stamina: None,
    ground_probe: 1.0,
    max_velocity: 100_000.0,
    ..CSGO
};

/// Momentum Mod's surf mode on CS:GO's hull: autohop, airaccelerate 150.
pub const MOMENTUM_SURF: SourceProfile = SourceProfile {
    air_accelerate: 150.0,
    max_velocity: 3500.0,
    ..MOMENTUM_BHOP
};

const VIEW_HEIGHT_STAND: i32 = 0x3c;

const VIEW_HEIGHT_CROUCH: i32 = 0x28;

const ANIM_EVENT_STAND_TO_CROUCH: u8 = 13;

const ANIM_EVENT_CROUCH_TO_STAND: u8 = 14;

/// Source's `GAMEMOVEMENT_DUCK_TIME`: the duck timer counts down from this, in ms.
const DUCK_TIMER_MS: f32 = 1000.0;

const DIST_EPSILON: f32 = 0.031_25;

/// How far past a steep step landing to look for the walkable top of a lip.
const LIP_PROBES: [f32; 3] = [2.0, 4.0, 6.0];

const STOP_EPSILON: f32 = 0.1;

const MAX_CLIP_PLANES: usize = 5;

const MAX_BUMPS: usize = 8;

/// `MAX_CLIMB_SPEED`: ladder climbing speed.
const LADDER_CLIMB_SPEED: f32 = 200.0;

/// Speed a jump pushes the player straight off a ladder.
const LADDER_JUMP_OFF_SPEED: f32 = 270.0;

/// Momentum's `sv_ramp_initial_retrace_length`: how far a bad trace on a ramp nudges off it.
const RAMP_RETRACE_LENGTH: f32 = 0.2;

/// Other players' bodies (`CONTENTS_BODY`): overlapping one is not being stuck in the world.
const CONTENTS_BODY: u32 = 0x0200_0000;

/// `CheckStuck` nudges, smallest first: up, sideways, then down.
const STUCK_NUDGES: [[f32; 3]; 18] = [
    [0.0, 0.0, 0.125],
    [0.0, 0.0, 1.0],
    [0.0, 0.0, 2.0],
    [1.0, 0.0, 0.0],
    [-1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, -1.0, 0.0],
    [2.0, 0.0, 0.0],
    [-2.0, 0.0, 0.0],
    [0.0, 2.0, 0.0],
    [0.0, -2.0, 0.0],
    [2.0, 2.0, 0.0],
    [-2.0, 2.0, 0.0],
    [2.0, -2.0, 0.0],
    [-2.0, -2.0, 0.0],
    [0.0, 0.0, 4.0],
    [0.0, 0.0, -1.0],
    [0.0, 0.0, -2.0],
];

/// Live multiplier on the stamina a jump costs (`mv_stamina`): 1 is the profile, 0 turns stamina
/// off. Prediction and a listen authority share the process, so both read the same value; a
/// remote client and server must be set alike.
static STAMINA_SCALE_BITS: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0x3f80_0000);

/// The current `mv_stamina` scale.
#[must_use]
pub fn stamina_scale() -> f32 {
    f32::from_bits(STAMINA_SCALE_BITS.load(core::sync::atomic::Ordering::Relaxed))
}

/// Set the `mv_stamina` scale, clamped to `0..=4`.
pub fn set_stamina_scale(scale: f32) {
    let scale = if scale.is_finite() {
        scale.clamp(0.0, 4.0)
    } else {
        1.0
    };
    STAMINA_SCALE_BITS.store(scale.to_bits(), core::sync::atomic::Ordering::Relaxed);
}

/// Damage for landing at `speed` (`FlPlayerFallDamage`).
#[must_use]
pub fn fall_damage(profile: &SourceProfile, speed: f32) -> i32 {
    if speed <= profile.fall_safe_speed {
        return 0;
    }
    let per_unit = 100.0 / (profile.fall_fatal_speed - profile.fall_safe_speed);
    let damage = (speed - profile.fall_safe_speed) * per_unit * profile.fall_damage_scale;
    if damage < 1.0 { 0 } else { damage as i32 }
}

/// Hull for the player's current duck state.
#[must_use]
pub fn hull(ps: &PlayerState, profile: &SourceProfile, base: MoveBounds) -> MoveBounds {
    let mut bounds = base;
    bounds.mins[2] = 0.0;
    bounds.maxs[2] = if ps.cs_duck_state & cs_duck::DUCKED != 0 {
        profile.duck_height
    } else {
        profile.stand_height
    };
    bounds
}

/// One command's movement state that is not in `PlayerState`.
struct Move<'a, C: CollisionBackend> {
    collision: &'a C,
    profile: SourceProfile,
    bounds: MoveBounds,
    gravity: f32,
    frametime: f32,
    /// Source's `m_surfaceFriction`: 0.25 while sliding up off a surface that is not ground.
    surface_friction: f32,
    landing_speed: f32,
}

#[derive(Clone, Copy, Debug)]
struct Wish {
    forward: f32,
    side: f32,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn pmove<C: CollisionBackend>(
    ps: &mut PlayerState,
    cmd: &mut UserCmd,
    context: PmoveSingleContext,
    collision: &C,
    profile: SourceProfile,
    mut pml: Pml,
) -> PmoveResult {
    let profile = SourceProfile {
        max_speed: profile.max_speed * crate::rules::weapon_speed_scale(ps, &context.walk.cmd_scale),
        ..profile
    };
    let walk_key = cmd.buttons & buttons::SPRINT != 0;
    cmd.buttons &= !(buttons::SPRINT | buttons::PRONE);
    ps.pm_flags &= !(pm_flags::SPRINTING | pm_flags::PRONE | pm_flags::MANTLE);
    ps.e_flags &= !eflags::PRONE;

    update_view_angles(ps, cmd, context.view_angles);
    let (forward, right, up) = math_iw4::angle_vectors(ps.viewangles);
    pml.forward = forward;
    pml.right = right;
    pml.up = up;

    let _ads = update_ads_intent(ps, cmd, context.old_buttons, context.ads_intent);
    update_ads_frac(ps, pml.msec, context.ads_frac);
    crate::breath::update_hold_breath(ps, cmd.buttons, pml.msec, context.can_hold_breath);
    reduce_timers(ps, pml.msec);

    let was_crouched = ps.pm_flags & pm_flags::CROUCH != 0;
    let mut mv = Move {
        collision,
        profile,
        bounds: hull(ps, &profile, context.bounds),
        gravity: ps.gravity as f32,
        frametime: pml.frametime,
        surface_friction: 1.0,
        landing_speed: 0.0,
    };

    let unstuck = mv.unstick(ps);
    mv.categorize(ps, &mut pml);
    if pml.walking == 0 {
        ps.cs_fall_velocity = -ps.velocity[2];
    }
    drop_timers(ps, &pml);

    let mut wish = command_wish(cmd, &profile, walk_key);
    mv.duck(ps, &mut pml, cmd, context.old_buttons, &mut wish);

    {
        let mut ladder_backend = LadderBackend {
            collision,
            bounds: mv.bounds,
        };
        if check_ladder_move(
            ps,
            crate::ladder::cs_ladder_context(cmd, pml.walking != 0, pml.forward, pml.right),
            &mut ladder_backend,
        ) {
            pml.record_jump_animation(crate::JumpAnimation::Forward, true);
        }
    }

    if ps.pm_flags & pm_flags::LADDER != 0 {
        mv.ladder_move(ps, &mut pml, cmd);
        mv.categorize(ps, &mut pml);
    } else {
        mv.full_walk_move(ps, &mut pml, cmd, context.old_buttons, wish);
    }

    if ps.pm_flags & pm_flags::LADDER != 0 {
        ladder_footsteps(ps, pml.msec, cmd.server_time);
    } else {
        let old_bob = ps.bob_cycle as u8;
        footsteps_bob_cycle(
            ps,
            pml.msec,
            cmd.forwardmove,
            cmd.rightmove,
            pml.almost_ground_plane != 0,
            cmd.server_time,
            context.walk.cmd_scale,
        );
        footstep_event(
            ps,
            old_bob,
            ps.bob_cycle as u8,
            pml.ground_trace[4],
            should_make_footsteps(ps),
        );
    }

    let crouched = ps.pm_flags & pm_flags::CROUCH != 0;
    let stance_event = match (was_crouched, crouched) {
        (false, true) => Some(ANIM_EVENT_STAND_TO_CROUCH),
        (true, false) => Some(ANIM_EVENT_CROUCH_TO_STAND),
        _ => None,
    };
    PmoveResult {
        pml,
        bounds: mv.bounds,
        stance_event,
        reset_torso: stance_event.is_some(),
        landing_speed: mv.landing_speed,
        unstuck,
    }
}

fn reduce_timers(ps: &mut PlayerState, msec: i32) {
    let msec = msec as f32;
    ps.cs_duck_time = (ps.cs_duck_time - msec).max(0.0);
    ps.cs_stamina = (ps.cs_stamina - msec).max(0.0);
}

/// Key movement in Source units: a full stick is `cl_forwardspeed`, the total is clamped to the
/// speed cap, and the walk key scales what is left.
fn command_wish(cmd: &UserCmd, profile: &SourceProfile, walk_key: bool) -> Wish {
    let forward_speed = if cmd.forwardmove < 0 {
        profile.back_speed
    } else {
        profile.forward_speed
    };
    let mut wish = Wish {
        forward: f32::from(cmd.forwardmove) / 127.0 * forward_speed,
        side: f32::from(cmd.rightmove) / 127.0 * profile.side_speed,
    };
    let speed = libm::sqrtf(wish.forward * wish.forward + wish.side * wish.side);
    if speed > profile.max_speed {
        let ratio = profile.max_speed / speed;
        wish.forward *= ratio;
        wish.side *= ratio;
    }
    if walk_key {
        wish.forward *= profile.walk_scale;
        wish.side *= profile.walk_scale;
    }
    wish
}

impl<C: CollisionBackend> Move<'_, C> {
    fn trace(&self, start: [f32; 3], end: [f32; 3], bounds: MoveBounds) -> Trace {
        self.collision.trace(GroundTraceInput {
            start,
            end,
            mins: bounds.mins,
            maxs: bounds.maxs,
            tracemask: bounds.tracemask,
        })
    }

    fn player_trace(&self, start: [f32; 3], end: [f32; 3]) -> Trace {
        self.trace(start, end, self.bounds)
    }

    /// `FullWalkMove` for a player out of water.
    fn full_walk_move(
        &mut self,
        ps: &mut PlayerState,
        pml: &mut Pml,
        cmd: &UserCmd,
        old_buttons: u32,
        wish: Wish,
    ) {
        self.start_gravity(ps);
        self.check_jump(ps, pml, cmd, old_buttons);
        if pml.walking != 0 {
            ps.velocity[2] = 0.0;
            self.friction(ps);
        }
        self.check_velocity(ps);
        if pml.walking != 0 {
            self.walk_move(ps, pml, wish);
            self.categorize(ps, pml);
        } else {
            self.air_move(ps, pml, wish);
        }
        self.categorize(ps, pml);
        self.check_velocity(ps);
        self.finish_gravity(ps);
        if pml.walking != 0 {
            ps.velocity[2] = 0.0;
            self.landing_speed = ps.cs_fall_velocity.max(0.0);
            ps.cs_fall_velocity = 0.0;
        }
    }

    fn start_gravity(&self, ps: &mut PlayerState) {
        ps.velocity[2] -= self.gravity * 0.5 * self.frametime;
        self.check_velocity(ps);
    }

    fn finish_gravity(&self, ps: &mut PlayerState) {
        ps.velocity[2] -= self.gravity * 0.5 * self.frametime;
        self.check_velocity(ps);
    }

    fn check_velocity(&self, ps: &mut PlayerState) {
        for axis in &mut ps.velocity {
            if axis.is_nan() {
                *axis = 0.0;
            }
            *axis = axis.clamp(-self.profile.max_velocity, self.profile.max_velocity);
        }
    }

    /// `CategorizePosition`: is the player standing on walkable ground within reach?
    fn categorize(&mut self, ps: &mut PlayerState, pml: &mut Pml) {
        self.surface_friction = 1.0;
        let origin = ps.origin;
        if ps.velocity[2] > self.profile.non_jump_velocity {
            self.leave_ground(ps, pml);
            return;
        }
        let below = [origin[0], origin[1], origin[2] - self.profile.ground_probe];
        let mut trace = self.player_trace(origin, below);
        if !standable(&trace) {
            let quadrants = self.touch_ground_in_quadrants(origin, below);
            if standable(&quadrants) {
                trace = quadrants;
            } else if trace.fraction < 1.0
                && ps.ground_entity_num != ENTITYNUM_NONE
                && self.lip_supported(&trace, &ps.velocity)
            {
                // Walking over a lip: its bevelled edge is under the hull, its top just ahead.
                trace.normal = [0.0, 0.0, 1.0];
            }
        }
        if !standable(&trace) {
            self.leave_ground(ps, pml);
            if ps.velocity[2] > 0.0 {
                self.surface_friction = 0.25;
            }
            return;
        }
        if ps.ground_entity_num == ENTITYNUM_NONE {
            // Landing: only if the velocity the ground leaves us is not still a launch.
            let mut next = ps.velocity;
            next[2] -= self.gravity * 0.5 * self.frametime;
            next = clip_velocity(next, trace.normal, 1.0);
            if next[2] > self.profile.non_jump_velocity {
                return;
            }
            if self.profile.slope_fix && flat_length_sq(&next) > flat_length_sq(&ps.velocity) {
                ps.velocity = next;
            }
        }
        self.set_ground(ps, pml, &trace);
    }

    /// Source's `TryTouchGroundInQuadrants`: a player straddling an edge stands on whichever
    /// quarter of the hull still has floor under it.
    fn touch_ground_in_quadrants(&self, origin: [f32; 3], below: [f32; 3]) -> Trace {
        let mins = self.bounds.mins;
        let maxs = self.bounds.maxs;
        let quadrants = [
            (
                [mins[0], mins[1]],
                [0.0_f32.min(maxs[0]), 0.0_f32.min(maxs[1])],
            ),
            (
                [0.0_f32.max(mins[0]), 0.0_f32.max(mins[1])],
                [maxs[0], maxs[1]],
            ),
            (
                [mins[0], 0.0_f32.max(mins[1])],
                [0.0_f32.min(maxs[0]), maxs[1]],
            ),
            (
                [0.0_f32.max(mins[0]), mins[1]],
                [maxs[0], 0.0_f32.min(maxs[1])],
            ),
        ];
        let mut last = Trace::default();
        for (low, high) in quadrants {
            let bounds = MoveBounds {
                mins: [low[0], low[1], mins[2]],
                maxs: [high[0], high[1], maxs[2]],
                tracemask: self.bounds.tracemask,
            };
            last = self.trace(origin, below, bounds);
            if standable(&last) {
                return last;
            }
        }
        last
    }

    fn set_ground(&mut self, ps: &mut PlayerState, pml: &mut Pml, trace: &Trace) {
        let landing = ps.ground_entity_num == ENTITYNUM_NONE;
        crate::ground::copy_ground_trace(pml, trace);
        pml.ground_plane = 1;
        pml.almost_ground_plane = 1;
        pml.walking = 1;
        if landing {
            ps.velocity[2] = 0.0;
            crate::crash_land(ps, pml);
        }
        let entity = crate::ground::trace_entity_id(trace);
        ps.ground_entity_num = entity;
        self.collision.touch_entity(entity);
    }

    fn leave_ground(&self, ps: &mut PlayerState, pml: &mut Pml) {
        ps.ground_entity_num = ENTITYNUM_NONE;
        pml.ground_plane = 0;
        pml.almost_ground_plane = 0;
        pml.walking = 0;
    }

    /// `CheckJumpButton` for CS-style modes.
    fn check_jump(&mut self, ps: &mut PlayerState, pml: &mut Pml, cmd: &UserCmd, old_buttons: u32) {
        let profile = self.profile;
        if cmd.buttons & buttons::JUMP == 0 || pml.walking == 0 {
            return;
        }
        if old_buttons & buttons::JUMP != 0 && !profile.autohop {
            return;
        }
        if let Some(factor) = profile.bhop_cap_factor {
            let cap = factor * profile.max_speed;
            let speed = length(&ps.velocity);
            if cap > 0.0 && speed > cap {
                let fraction = cap / speed;
                for axis in &mut ps.velocity {
                    *axis *= fraction;
                }
            }
        }
        self.leave_ground(ps, pml);
        ps.jump_origin_z = ps.origin[2];
        ps.jump_time = cmd.server_time;

        ps.velocity[2] += profile.jump_impulse;
        if let Some(stamina) = profile.stamina {
            if ps.cs_stamina > 0.0 {
                ps.velocity[2] *= stamina_ratio(&stamina, ps.cs_stamina);
            }
            // Scaling the cost scales both how hard and how long the slowdown bites.
            ps.cs_stamina = stamina.jump_cost * stamina_scale() / stamina.recover_rate * 1000.0;
        }
        self.finish_gravity(ps);

        // Lift the feet off the ground so the next categorisation cannot re-catch them.
        let origin = ps.origin;
        let probe = [
            origin[0],
            origin[1],
            origin[2] - (profile.ground_probe + 0.1),
        ];
        let down = self.player_trace(origin, probe);
        if down.fraction < 1.0 && down.startsolid == 0 && down.allsolid == 0 {
            let raised = [
                down.endpos[0],
                down.endpos[1],
                down.endpos[2] + profile.jump_z_offset,
            ];
            let up = self.player_trace(down.endpos, raised);
            if up.fraction >= 1.0 && up.startsolid == 0 && up.allsolid == 0 {
                ps.origin = raised;
            }
        }

        jump::event(ps, pml.ground_trace[4]);
        let animation = if cmd.forwardmove >= 0 {
            crate::JumpAnimation::Forward
        } else {
            crate::JumpAnimation::Backward
        };
        pml.record_jump_animation(animation, true);
    }

    /// Source `Friction`: stop-speed friction on the ground plane.
    fn friction(&self, ps: &mut PlayerState) {
        let speed = length(&ps.velocity);
        if speed < 0.1 {
            return;
        }
        let friction = self.profile.friction * self.surface_friction;
        let control = speed.max(self.profile.stop_speed);
        let drop = control * friction * self.frametime;
        let scale = (speed - drop).max(0.0) / speed;
        for axis in &mut ps.velocity {
            *axis *= scale;
        }
    }

    fn accelerate(&self, ps: &mut PlayerState, dir: [f32; 3], wish_speed: f32, accel: f32) {
        let current = dot(&ps.velocity, &dir);
        let add = wish_speed - current;
        if add <= 0.0 {
            return;
        }
        let step = (accel * self.frametime * wish_speed * self.surface_friction).min(add);
        for (axis, component) in ps.velocity.iter_mut().zip(dir) {
            *axis += step * component;
        }
    }

    fn air_accelerate(&self, ps: &mut PlayerState, dir: [f32; 3], wish_speed: f32) {
        let capped = wish_speed.min(self.profile.air_speed_cap);
        let current = dot(&ps.velocity, &dir);
        let add = capped - current;
        if add <= 0.0 {
            return;
        }
        let step =
            (self.profile.air_accelerate * wish_speed * self.frametime * self.surface_friction)
                .min(add);
        for (axis, component) in ps.velocity.iter_mut().zip(dir) {
            *axis += step * component;
        }
    }

    /// `WalkMove`: accelerate, move straight or step, then stay glued to the ground.
    fn walk_move(&mut self, ps: &mut PlayerState, pml: &mut Pml, wish: Wish) {
        if let Some(stamina) = self.profile.stamina
            && ps.cs_stamina > 0.0
        {
            let ratio = libm::powf(
                stamina_ratio(&stamina, ps.cs_stamina).max(0.0),
                self.frametime / stamina.reference_frametime,
            );
            ps.velocity[0] *= ratio;
            ps.velocity[1] *= ratio;
        }

        let (dir, wish_speed) = wish_direction(pml, wish);
        let wish_speed = wish_speed.min(self.profile.max_speed);
        self.accelerate(ps, dir, wish_speed, self.profile.accelerate);

        if length(&ps.velocity) < 0.0001 {
            ps.velocity = [0.0; 3];
            return;
        }

        let dest = [
            ps.origin[0] + ps.velocity[0] * self.frametime,
            ps.origin[1] + ps.velocity[1] * self.frametime,
            ps.origin[2],
        ];
        let direct = self.player_trace(ps.origin, dest);
        if direct.fraction >= 1.0 && direct.startsolid == 0 && direct.allsolid == 0 {
            ps.origin = dest;
            self.stay_on_ground(ps);
            return;
        }
        self.step_move(ps, pml);
        self.stay_on_ground(ps);
    }

    /// Source `StepMove`: the better of a plain slide and a slide one step up.
    fn step_move(&mut self, ps: &mut PlayerState, pml: &mut Pml) {
        let start = ps.origin;
        let start_velocity = ps.velocity;

        self.try_player_move(ps, pml);
        let down_pos = ps.origin;
        let down_velocity = ps.velocity;

        ps.origin = start;
        ps.velocity = start_velocity;
        let raised = [
            start[0],
            start[1],
            start[2] + self.profile.step_size + DIST_EPSILON,
        ];
        let up = self.player_trace(start, raised);
        if up.startsolid == 0 && up.allsolid == 0 {
            ps.origin = up.endpos;
        }
        self.try_player_move(ps, pml);

        let lowered = [
            ps.origin[0],
            ps.origin[1],
            ps.origin[2] - self.profile.step_size - DIST_EPSILON,
        ];
        let down = self.player_trace(ps.origin, lowered);
        let landed = down.fraction < 1.0
            && (down.normal[2] >= 0.7 || self.lip_supported(&down, &start_velocity));
        if !landed {
            ps.origin = down_pos;
            ps.velocity = down_velocity;
            return;
        }
        if down.startsolid == 0 && down.allsolid == 0 {
            ps.origin = down.endpos;
        }
        if flat_distance_sq(&down_pos, &start) > flat_distance_sq(&ps.origin, &start) {
            ps.origin = down_pos;
            ps.velocity = down_velocity;
        } else {
            // Source keeps the slide's climb rate; a step never launches, though: sliding into a
            // lip on a slope can turn most of the run into climb, past the leave-ground speed.
            ps.velocity[2] = down_velocity[2].min(self.profile.non_jump_velocity - 1.0);
        }
    }

    /// A step that came down on a too-steep face (the bevelled top edge of a small lip, common
    /// on MW2's meshes, like the flanges on mp_rust's pipe) still counts when walkable ground lies
    /// just past it at about the same height: the hull is cresting the lip, not climbing a wall.
    fn lip_supported(&self, landing: &Trace, velocity: &[f32; 3]) -> bool {
        let speed = libm::sqrtf(flat_length_sq(velocity));
        if landing.startsolid != 0 || speed < 1.0 {
            return false;
        }
        let dir = [velocity[0] / speed, velocity[1] / speed];
        let step = self.profile.step_size;
        LIP_PROBES.iter().any(|&ahead| {
            let x = landing.endpos[0] + dir[0] * ahead;
            let y = landing.endpos[1] + dir[1] * ahead;
            let from = [x, y, landing.endpos[2] + step];
            let to = [x, y, landing.endpos[2] - step];
            let probe = self.player_trace(from, to);
            probe.startsolid == 0 && standable(&probe)
        })
    }

    /// Source `StayOnGround`: keep a walking player on slopes and stairs going down.
    fn stay_on_ground(&self, ps: &mut PlayerState) {
        let origin = ps.origin;
        let above = [origin[0], origin[1], origin[2] + 2.0];
        let end = [origin[0], origin[1], origin[2] - self.profile.step_size];
        let up = self.player_trace(origin, above);
        let start = if up.fraction >= 1.0 { above } else { up.endpos };
        let down = self.player_trace(start, end);
        if down.fraction > 0.0
            && down.fraction < 1.0
            && down.startsolid == 0
            && down.normal[2] >= 0.7
            && (origin[2] - down.endpos[2]).abs() > 0.5 * DIST_EPSILON
        {
            ps.origin = down.endpos;
        }
    }

    /// Source / GoldSrc `LadderMove` on the ladder IW4 attached the player to (`v_ladder_vec` is
    /// its outward normal). Forward climbs where the view points: into the ladder goes up, and
    /// looking down while pressing forward goes down. Jump pushes straight off. No gravity.
    fn ladder_move(&mut self, ps: &mut PlayerState, pml: &mut Pml, cmd: &UserCmd) {
        let normal = ps.v_ladder_vec;
        let mut climb = LADDER_CLIMB_SPEED;
        if cmd.buttons & buttons::CROUCH != 0 {
            climb *= self.profile.duck_scale;
        }
        let forward_speed = climb * f32::from(cmd.forwardmove) / 127.0;
        let right_speed = climb * f32::from(cmd.rightmove) / 127.0;

        if cmd.buttons & buttons::JUMP != 0 {
            ps.velocity = normal.map(|axis| axis * LADDER_JUMP_OFF_SPEED);
            crate::clear_ladder_flag(ps);
            // IW4's attach waits out `LADDER_JUMP_BLOCK_MS` after a jump, so the push-off sticks.
            ps.jump_time = cmd.server_time;
        } else if forward_speed != 0.0 || right_speed != 0.0 {
            let velocity = [0, 1, 2]
                .map(|axis| pml.forward[axis] * forward_speed + pml.right[axis] * right_speed);
            let mut perp = cross(&[0.0, 0.0, 1.0], &normal);
            normalize(&mut perp);
            let into = dot(&velocity, &normal);
            let lateral = [0, 1, 2].map(|axis| velocity[axis] - normal[axis] * into);
            // Up the ladder face.
            let along = cross(&normal, &perp);
            ps.velocity = [0, 1, 2].map(|axis| lateral[axis] - into * along[axis]);
            if pml.walking != 0 && into > 0.0 {
                // Walking away from a ladder on the floor steps off it.
                for (axis, component) in ps.velocity.iter_mut().zip(normal) {
                    *axis += LADDER_CLIMB_SPEED * component;
                }
            }
        } else {
            ps.velocity = [0.0; 3];
        }
        self.try_player_move(ps, pml);
    }

    /// `AirMove`: capped air acceleration, then a slide with no stepping.
    fn air_move(&mut self, ps: &mut PlayerState, pml: &mut Pml, wish: Wish) {
        let (dir, wish_speed) = wish_direction(pml, wish);
        let wish_speed = wish_speed.min(self.profile.max_speed);
        self.air_accelerate(ps, dir, wish_speed);
        self.try_player_move(ps, pml);
    }

    /// `TryPlayerMove`: move through the frame, clipping along up to five planes with no
    /// overbounce. A bad trace in the air on a ramp nudges off the last good plane and retries,
    /// as Momentum's ramp fix does, instead of stopping the player dead.
    fn try_player_move(&mut self, ps: &mut PlayerState, pml: &Pml) {
        let airborne = pml.walking == 0;
        let primal_velocity = ps.velocity;
        let mut original_velocity = ps.velocity;
        let mut planes = [[0.0_f32; 3]; MAX_CLIP_PLANES];
        let mut plane_count = 0usize;
        let mut last_good_plane: Option<[f32; 3]> = None;
        let mut time_left = self.frametime;
        let mut all_fraction = 0.0_f32;

        for bump in 0..MAX_BUMPS {
            if length(&ps.velocity) == 0.0 {
                break;
            }
            let end = [
                ps.origin[0] + ps.velocity[0] * time_left,
                ps.origin[1] + ps.velocity[1] * time_left,
                ps.origin[2] + ps.velocity[2] * time_left,
            ];
            let trace = self.player_trace(ps.origin, end);

            let bad = trace.allsolid != 0 || (trace.fraction < 1.0 && length(&trace.normal) < 0.5);
            if bad {
                if airborne
                    && bump > 0
                    && let Some(plane) = last_good_plane
                {
                    // Ramp fix: step off the surface along its normal and try again.
                    let nudge = RAMP_RETRACE_LENGTH * bump as f32;
                    for (axis, component) in ps.origin.iter_mut().zip(plane) {
                        *axis += component * nudge;
                    }
                    continue;
                }
                if trace.allsolid != 0 {
                    ps.velocity = [0.0; 3];
                    return;
                }
            }

            if trace.fraction > 0.0 {
                ps.origin = if trace.fraction >= 1.0 {
                    end
                } else {
                    trace.endpos
                };
                original_velocity = ps.velocity;
                plane_count = 0;
                all_fraction += trace.fraction;
            }
            if trace.fraction >= 1.0 {
                break;
            }
            last_good_plane = Some(trace.normal);
            time_left -= time_left * trace.fraction;

            // The same surface again (IW4 meshes report a zero-fraction hit for a glancing sweep
            // that starts inside their clip epsilon): not a crease, so push the velocity out
            // along it instead of stopping dead, as IW4's own `PM_SlideMove` does.
            if planes[..plane_count]
                .iter()
                .any(|plane| dot(plane, &trace.normal) > 0.99)
            {
                for (axis, push) in ps.velocity.iter_mut().zip(trace.normal) {
                    *axis += push;
                }
                continue;
            }

            if plane_count >= MAX_CLIP_PLANES {
                ps.velocity = [0.0; 3];
                break;
            }
            planes[plane_count] = trace.normal;
            plane_count += 1;

            if plane_count == 1 && airborne {
                // A floor or, while surfing, the ramp itself: slide along it with no bounce.
                let clipped = clip_velocity(original_velocity, planes[0], 1.0);
                ps.velocity = clipped;
                original_velocity = clipped;
                continue;
            }

            let mut fit = None;
            for i in 0..plane_count {
                let clipped = clip_velocity(original_velocity, planes[i], 1.0);
                let ok = (0..plane_count)
                    .filter(|&j| j != i)
                    .all(|j| dot(&clipped, &planes[j]) >= 0.0);
                if ok {
                    fit = Some(clipped);
                    break;
                }
            }
            match fit {
                Some(clipped) => ps.velocity = clipped,
                None => {
                    if plane_count != 2 {
                        ps.velocity = [0.0; 3];
                        break;
                    }
                    let mut crease = cross(&planes[0], &planes[1]);
                    normalize(&mut crease);
                    let along = dot(&crease, &ps.velocity);
                    ps.velocity = [crease[0] * along, crease[1] * along, crease[2] * along];
                }
            }
            if dot(&ps.velocity, &primal_velocity) <= 0.0 {
                ps.velocity = [0.0; 3];
                break;
            }
        }
        if all_fraction == 0.0 {
            ps.velocity = [0.0; 3];
        }
    }

    /// Source `Duck` with `DoDuck` / `DoUnduck` / `FinishDuck` / `FinishUnDuck`.
    #[allow(clippy::too_many_lines)]
    fn duck(
        &mut self,
        ps: &mut PlayerState,
        pml: &mut Pml,
        cmd: &UserCmd,
        old_buttons: u32,
        wish: &mut Wish,
    ) {
        let profile = self.profile;
        let held = cmd.buttons & buttons::CROUCH != 0;
        let pressed = held && old_buttons & buttons::CROUCH == 0;
        let released = !held && old_buttons & buttons::CROUCH != 0;
        let ducked = ps.cs_duck_state & cs_duck::DUCKED != 0;
        let ducking = ps.cs_duck_state & cs_duck::IN_DUCK != 0;
        let in_air = pml.walking == 0;
        let shift = profile.duck_air_shift * (profile.stand_height - profile.duck_height);

        if held || ducking || ducked {
            wish.forward *= profile.duck_scale;
            wish.side *= profile.duck_scale;
        }

        if held {
            if pressed {
                if !ducked {
                    ps.cs_duck_time = DUCK_TIMER_MS;
                    ps.cs_duck_state |= cs_duck::IN_DUCK;
                } else if ducking {
                    // Reverse an unduck in progress.
                    let remaining = (DUCK_TIMER_MS - ps.cs_duck_time)
                        * (profile.duck_seconds / profile.unduck_seconds);
                    ps.cs_duck_time = DUCK_TIMER_MS - profile.duck_seconds * 1000.0 + remaining;
                }
            }
            if ps.cs_duck_state & cs_duck::IN_DUCK != 0 {
                let seconds = (DUCK_TIMER_MS - ps.cs_duck_time).max(0.0) / 1000.0;
                if seconds > profile.duck_seconds || in_air {
                    self.finish_duck(ps, pml, in_air, shift);
                } else {
                    let fraction = simple_spline(seconds / profile.duck_seconds);
                    set_eye(ps, &profile, fraction);
                }
            }
        } else if ducking || ducked {
            if released {
                if ducked && !ducking {
                    ps.cs_duck_time = DUCK_TIMER_MS;
                    ps.cs_duck_state |= cs_duck::IN_DUCK;
                } else if ducking {
                    // Reverse a duck in progress.
                    let remaining = (DUCK_TIMER_MS - ps.cs_duck_time)
                        * (profile.unduck_seconds / profile.duck_seconds);
                    ps.cs_duck_time = DUCK_TIMER_MS - profile.unduck_seconds * 1000.0 + remaining;
                }
            }
            if self.can_unduck(ps, in_air, shift) {
                let seconds = (DUCK_TIMER_MS - ps.cs_duck_time).max(0.0) / 1000.0;
                if seconds > profile.unduck_seconds || in_air || !ducked {
                    if ducked {
                        self.finish_unduck(ps, pml, in_air, shift);
                    } else {
                        // Released before the hull ever shrank: just stand the view back up.
                        ps.cs_duck_state = 0;
                        ps.cs_duck_time = 0.0;
                        set_eye(ps, &profile, 0.0);
                    }
                } else {
                    let fraction = simple_spline(1.0 - seconds / profile.unduck_seconds);
                    set_eye(ps, &profile, fraction);
                    ps.cs_duck_state |= cs_duck::IN_DUCK;
                }
            } else {
                // No room to stand: stay ducked until there is.
                ps.cs_duck_time = DUCK_TIMER_MS;
                ps.cs_duck_state = cs_duck::DUCKED;
                set_eye(ps, &profile, 1.0);
            }
        } else {
            set_eye(ps, &profile, 0.0);
        }

        let crouched = ps.cs_duck_state != 0;
        if crouched {
            ps.pm_flags |= pm_flags::CROUCH;
            ps.e_flags |= eflags::DUCK;
            ps.view_height_target = VIEW_HEIGHT_CROUCH;
        } else {
            ps.pm_flags &= !pm_flags::CROUCH;
            ps.e_flags &= !eflags::DUCK;
            ps.view_height_target = VIEW_HEIGHT_STAND;
        }
        ps.view_height_lerp_time = 0;
    }

    fn finish_duck(&mut self, ps: &mut PlayerState, pml: &mut Pml, in_air: bool, shift: f32) {
        let was_ducked = ps.cs_duck_state & cs_duck::DUCKED != 0;
        ps.cs_duck_state = cs_duck::DUCKED;
        set_eye(ps, &self.profile, 1.0);
        if !was_ducked && in_air {
            // In the air the smaller hull keeps the view where it was: the feet come up.
            ps.origin[2] += shift;
        }
        self.bounds = hull(ps, &self.profile, self.bounds);
        self.categorize(ps, pml);
    }

    fn finish_unduck(&mut self, ps: &mut PlayerState, pml: &mut Pml, in_air: bool, shift: f32) {
        if in_air {
            ps.origin[2] -= shift;
        }
        ps.cs_duck_state = 0;
        ps.cs_duck_time = 0.0;
        set_eye(ps, &self.profile, 0.0);
        self.bounds = hull(ps, &self.profile, self.bounds);
        self.categorize(ps, pml);
    }

    /// `CanUnduck`: the standing hull fits where standing would put it.
    fn can_unduck(&self, ps: &PlayerState, in_air: bool, shift: f32) -> bool {
        if ps.cs_duck_state & cs_duck::DUCKED == 0 {
            return true;
        }
        let mut stand = self.bounds;
        stand.maxs[2] = self.profile.stand_height;
        let mut target = ps.origin;
        if in_air {
            target[2] -= shift;
        }
        let trace = self.trace(ps.origin, target, stand);
        if trace.startsolid != 0 || trace.allsolid != 0 || trace.fraction < 1.0 {
            return false;
        }
        // A sweep only meets surfaces it moves toward: the in-air drop never sees a wall or an
        // overhang the taller hull would poke into. Test the spot itself.
        let fits = self.trace(target, target, stand);
        fits.startsolid == 0 && fits.allsolid == 0
    }

    fn fits(&self, origin: [f32; 3], bounds: MoveBounds) -> bool {
        let bounds = MoveBounds {
            tracemask: bounds.tracemask & !CONTENTS_BODY,
            ..bounds
        };
        let trace = self.trace(origin, origin, bounds);
        trace.startsolid == 0 && trace.allsolid == 0
    }

    /// Source `CheckStuck` / `FixPlayerCrouchStuck`: a hull that starts the command inside the
    /// world crouches when only the standing hull is stuck (it stands back up once there is
    /// room), else takes the first small nudge that frees it. Returns whether it had to.
    fn unstick(&mut self, ps: &mut PlayerState) -> bool {
        if self.fits(ps.origin, self.bounds) {
            return false;
        }
        if ps.cs_duck_state & cs_duck::DUCKED == 0 {
            let mut duck = self.bounds;
            duck.maxs[2] = self.profile.duck_height;
            if self.fits(ps.origin, duck) {
                ps.cs_duck_state = cs_duck::DUCKED;
                ps.cs_duck_time = DUCK_TIMER_MS;
                set_eye(ps, &self.profile, 1.0);
                self.bounds = duck;
                return true;
            }
        }
        for nudge in STUCK_NUDGES {
            let origin = core::array::from_fn(|axis| ps.origin[axis] + nudge[axis]);
            if self.fits(origin, self.bounds) {
                ps.origin = origin;
                return true;
            }
        }
        false
    }
}

fn set_eye(ps: &mut PlayerState, profile: &SourceProfile, duck_fraction: f32) {
    ps.view_height_current =
        profile.duck_eye * duck_fraction + profile.stand_eye * (1.0 - duck_fraction);
}

fn simple_spline(value: f32) -> f32 {
    let squared = value * value;
    3.0 * squared - 2.0 * squared * value
}

fn stamina_ratio(stamina: &Stamina, remaining_ms: f32) -> f32 {
    (stamina.max - remaining_ms / 1000.0 * stamina.recover_rate) / stamina.max
}

fn standable(trace: &Trace) -> bool {
    trace.fraction < 1.0 && trace.allsolid == 0 && trace.normal[2] >= 0.7
}

/// Source `ClipVelocity`: slide along `normal`, dropping tiny components, and make sure the
/// result does not still point into the plane.
fn clip_velocity(input: [f32; 3], normal: [f32; 3], overbounce: f32) -> [f32; 3] {
    let backoff = dot(&input, &normal) * overbounce;
    let mut out = [0.0; 3];
    for axis in 0..3 {
        out[axis] = input[axis] - normal[axis] * backoff;
        if out[axis] > -STOP_EPSILON && out[axis] < STOP_EPSILON {
            out[axis] = 0.0;
        }
    }
    let adjust = dot(&out, &normal);
    if adjust < 0.0 {
        for axis in 0..3 {
            out[axis] -= normal[axis] * adjust;
        }
    }
    out
}

fn wish_direction(pml: &Pml, wish: Wish) -> ([f32; 3], f32) {
    let mut forward = [pml.forward[0], pml.forward[1], 0.0];
    let mut right = [pml.right[0], pml.right[1], 0.0];
    normalize(&mut forward);
    normalize(&mut right);
    let mut dir = [
        forward[0] * wish.forward + right[0] * wish.side,
        forward[1] * wish.forward + right[1] * wish.side,
        0.0,
    ];
    let speed = normalize(&mut dir);
    (dir, speed)
}

struct LadderBackend<'a, C: CollisionBackend> {
    collision: &'a C,
    bounds: MoveBounds,
}

impl<C: CollisionBackend> LadderAttachBackend for LadderBackend<'_, C> {
    fn ladder_trace(
        &mut self,
        origin: [f32; 3],
        dir: [f32; 3],
        dist: f32,
    ) -> Option<LadderTraceHit> {
        let end = [
            origin[0] + dir[0] * dist,
            origin[1] + dir[1] * dist,
            origin[2] + dir[2] * dist,
        ];
        let mut mins = self.bounds.mins;
        let mut maxs = self.bounds.maxs;
        mins[0] += 6.0;
        mins[1] += 6.0;
        mins[2] = 8.0;
        maxs[0] -= 6.0;
        maxs[1] -= 6.0;
        if maxs[2] < 8.0 {
            maxs[2] = mins[2];
        }
        let hit = self.collision.trace(GroundTraceInput {
            start: origin,
            end,
            mins,
            maxs,
            tracemask: self.bounds.tracemask,
        });
        if hit.fraction >= 1.0 {
            return None;
        }
        Some(LadderTraceHit {
            fraction: hit.fraction,
            normal: hit.normal,
            surface_flags: hit.surface_flags,
        })
    }
}

fn length(v: &[f32; 3]) -> f32 {
    libm::sqrtf(v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
}

fn dot(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: &[f32; 3], b: &[f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: &mut [f32; 3]) -> f32 {
    let len = length(v);
    if len > 0.0 {
        let inv = 1.0 / len;
        v[0] *= inv;
        v[1] *= inv;
        v[2] *= inv;
    }
    len
}

fn flat_length_sq(v: &[f32; 3]) -> f32 {
    v[0] * v[0] + v[1] * v[1]
}

fn flat_distance_sq(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cs::tests::{Floor, Ramp, context};

    const TICK_MS: i32 = 10;

    const RULES: Option<crate::rules::Ruleset> = Some(crate::rules::Ruleset::Source(CSGO));

    struct Sim<C: CollisionBackend> {
        ps: PlayerState,
        time: i32,
        old_buttons: u32,
        world: C,
    }

    impl<C: CollisionBackend> Sim<C> {
        fn new(world: C) -> Self {
            let mut ps = PlayerState::ZERO;
            ps.gravity = 800;
            ps.speed = 190;
            ps.view_height_target = VIEW_HEIGHT_STAND;
            ps.view_height_current = 60.0;
            ps.ground_entity_num = ENTITYNUM_NONE;
            Self {
                ps,
                time: 0,
                old_buttons: 0,
                world,
            }
        }

        fn tick(&mut self, forward: i8, right: i8, buttons: u32, yaw: f32) {
            self.time += TICK_MS;
            let mut cmd = UserCmd {
                server_time: self.time,
                buttons,
                forwardmove: forward,
                rightmove: right,
                ..UserCmd::default()
            };
            cmd.angles[1] = (yaw * crate::ANGLE2SHORT) as i32;
            crate::pmove_with_rules(
                &mut self.ps,
                &mut cmd,
                context(self.old_buttons),
                &self.world,
                &crate::FlatMantleAnimLength::default(),
                &crate::ZeroMantleRootDelta,
                RULES,
            );
            self.old_buttons = buttons;
        }

        fn flat_speed(&self) -> f32 {
            libm::sqrtf(flat_length_sq(&self.ps.velocity))
        }

        fn settle(&mut self) {
            for _ in 0..20 {
                self.tick(0, 0, 0, 0.0);
            }
        }

        fn grounded(&self) -> bool {
            self.ps.ground_entity_num != ENTITYNUM_NONE
        }
    }

    fn steady_speed(buttons: u32) -> f32 {
        let mut sim = Sim::new(Floor);
        sim.settle();
        for _ in 0..300 {
            sim.tick(127, 0, buttons, 0.0);
        }
        sim.flat_speed()
    }

    #[test]
    fn csgo_run_walk_and_duck_speeds() {
        assert!(
            (steady_speed(0) - 250.0).abs() < 1.0,
            "run {}",
            steady_speed(0)
        );
        let walk = steady_speed(buttons::SPRINT);
        assert!((walk - 130.0).abs() < 1.0, "walk {walk}");
        let duck = steady_speed(buttons::CROUCH);
        assert!((duck - 85.0).abs() < 1.0, "duck {duck}");
    }

    #[test]
    fn friction_stops_a_runner() {
        let mut sim = Sim::new(Floor);
        sim.settle();
        for _ in 0..300 {
            sim.tick(127, 0, 0, 0.0);
        }
        let mut ticks = 0;
        while sim.flat_speed() > 0.0 && ticks < 200 {
            sim.tick(0, 0, 0, 0.0);
            ticks += 1;
        }
        assert!((30..=60).contains(&ticks), "stopped after {ticks} ticks");
    }

    #[test]
    fn jump_reaches_csgo_height_and_does_not_pogo() {
        let mut sim = Sim::new(Floor);
        sim.settle();
        let mut apex = 0.0_f32;
        let mut jumps = 0;
        let mut airborne = false;
        for _ in 0..200 {
            sim.tick(0, 0, buttons::JUMP, 0.0);
            apex = apex.max(sim.ps.origin[2]);
            let now_airborne = !sim.grounded();
            if now_airborne && !airborne {
                jumps += 1;
            }
            airborne = now_airborne;
        }
        assert!((apex - 57.0).abs() < 2.0, "apex {apex}");
        assert_eq!(jumps, 1, "holding jump must not pogo without autohop");
    }

    #[test]
    fn back_to_back_jumps_lose_height_to_stamina() {
        let mut sim = Sim::new(Floor);
        sim.settle();
        let mut apexes = [0.0_f32; 2];
        let mut landings = 0;
        let mut was_grounded = true;
        let mut apex = 0.0_f32;
        let mut pressed = buttons::JUMP;
        for _ in 0..400 {
            sim.tick(0, 0, pressed, 0.0);
            apex = apex.max(sim.ps.origin[2]);
            let grounded = sim.grounded();
            if grounded && !was_grounded {
                if landings < 2 {
                    apexes[landings] = apex;
                }
                landings += 1;
                apex = 0.0;
            }
            was_grounded = grounded;
            pressed = if grounded { buttons::JUMP } else { 0 };
        }
        assert!(landings >= 2);
        assert!(apexes[1] < apexes[0] - 2.0, "{apexes:?}");
    }

    #[test]
    fn air_strafing_gains_speed() {
        let mut sim = Sim::new(Floor);
        sim.ps.origin = [0.0, 0.0, 5000.0];
        sim.ps.velocity = [250.0, 0.0, 0.0];
        for _ in 0..100 {
            let yaw = libm::atan2f(sim.ps.velocity[1], sim.ps.velocity[0]).to_degrees();
            sim.tick(0, -127, 0, yaw);
        }
        assert!(sim.flat_speed() > 300.0, "{}", sim.flat_speed());
    }

    #[test]
    fn ducking_in_the_air_lifts_the_feet_by_half_the_hull_gap() {
        let mut sim = Sim::new(Floor);
        sim.ps.origin = [0.0, 0.0, 500.0];
        sim.tick(0, 0, 0, 0.0);
        let before = sim.ps.origin[2];
        sim.tick(0, 0, buttons::CROUCH, 0.0);
        let rise = sim.ps.origin[2] - before;
        assert!(rise > 8.5 && rise < 9.1, "rise {rise}");
        assert_eq!(sim.ps.cs_duck_state, cs_duck::DUCKED);
    }

    #[test]
    fn ground_duck_keeps_feet_down_and_lowers_the_eye() {
        let mut sim = Sim::new(Floor);
        sim.settle();
        for _ in 0..60 {
            sim.tick(0, 0, buttons::CROUCH, 0.0);
        }
        assert_eq!(sim.ps.cs_duck_state, cs_duck::DUCKED);
        assert!(sim.ps.origin[2].abs() < 0.5);
        assert!((sim.ps.view_height_current - CSGO.duck_eye).abs() < 0.01);
        for _ in 0..30 {
            sim.tick(0, 0, 0, 0.0);
        }
        assert_eq!(sim.ps.cs_duck_state, 0);
        assert!(sim.ps.origin[2].abs() < 0.5);
        assert!((sim.ps.view_height_current - CSGO.stand_eye).abs() < 0.01);
    }

    #[test]
    fn surfing_a_steep_ramp_keeps_its_speed() {
        let ramp = Ramp {
            normal: [0.866_025_4, 0.0, 0.5],
        };
        let mut sim = Sim::new(ramp);
        sim.ps.origin = [15.0 + 0.1, 0.0, 0.1];
        sim.ps.velocity = [0.0, 600.0, 0.0];
        for _ in 0..100 {
            sim.tick(0, 127, 0, 180.0);
            let bounds = hull(&sim.ps, &CSGO, context(0).bounds);
            assert!(
                sim.world.clearance(sim.ps.origin, bounds.mins, bounds.maxs) > -0.01,
                "fell into the ramp at {:?}",
                sim.ps.origin
            );
        }
        assert!(!sim.grounded(), "a 60 degree ramp is not ground");
        assert!(
            sim.ps.velocity[1] > 595.0,
            "lost surf speed: {:?}",
            sim.ps.velocity
        );
    }

    #[test]
    fn landing_on_a_downhill_keeps_slope_speed() {
        // 30 degree slope falling away along +x: walkable, so the player lands on it.
        let ramp = Ramp {
            normal: [0.5, 0.0, 0.866_025_4],
        };
        let mut sim = Sim::new(ramp);
        // At this height the hull's downhill corner clears the slope by about 0.7 units.
        sim.ps.origin = [0.0, 0.0, 9.5];
        sim.ps.velocity = [200.0, 0.0, -400.0];
        sim.tick(0, 0, 0, 0.0);
        sim.tick(0, 0, 0, 0.0);
        assert!(sim.grounded());
        assert!(sim.flat_speed() > 190.0, "{}", sim.flat_speed());
    }

    #[test]
    fn fall_damage_follows_source_rules() {
        assert_eq!(fall_damage(&CSGO, 580.0), 0);
        assert_eq!(fall_damage(&CSGO, 1024.0), 125);
    }

    /// Infinite planes, each solid on its negative side: `dot(p, normal) < offset`.
    struct Planes([([f32; 3], f32, u32); 2]);

    impl Planes {
        fn clearance(
            normal: [f32; 3],
            offset: f32,
            origin: [f32; 3],
            input: &GroundTraceInput,
        ) -> f32 {
            let mut lowest = -offset;
            for axis in 0..3 {
                let corner = if normal[axis] > 0.0 {
                    input.mins[axis]
                } else {
                    input.maxs[axis]
                };
                lowest += (origin[axis] + corner) * normal[axis];
            }
            lowest
        }
    }

    impl CollisionBackend for Planes {
        fn trace(&self, input: GroundTraceInput) -> Trace {
            let mut best = Trace {
                fraction: 1.0,
                endpos: input.end,
                hit_type: trace_iw4::HITTYPE_ENTITY,
                hit_id: trace_iw4::ENTITYNUM_WORLD,
                ..Trace::default()
            };
            for &(normal, offset, surface_flags) in &self.0 {
                let start = Self::clearance(normal, offset, input.start, &input);
                let end = Self::clearance(normal, offset, input.end, &input);
                if start < -0.001 {
                    if end < -0.001 {
                        best.allsolid = 1;
                    }
                    best.startsolid = 1;
                    best.fraction = 0.0;
                    best.endpos = input.start;
                    best.normal = normal;
                    best.surface_flags = surface_flags;
                    continue;
                }
                if end >= 0.0 {
                    continue;
                }
                let fraction = ((start - 0.031_25) / (start - end)).clamp(0.0, 1.0);
                if fraction < best.fraction {
                    best.fraction = fraction;
                    best.normal = normal;
                    best.surface_flags = surface_flags;
                    best.walkable = u8::from(normal[2] >= 0.7);
                    for axis in 0..3 {
                        best.endpos[axis] =
                            input.start[axis] + (input.end[axis] - input.start[axis]) * fraction;
                    }
                }
            }
            best
        }
    }

    fn ladder_world() -> Planes {
        // Floor at z = 0 and a ladder wall at x = 0 facing +x.
        Planes([
            ([0.0, 0.0, 1.0], 0.0, 0),
            ([1.0, 0.0, 0.0], 0.0, crate::SURF_LADDER),
        ])
    }

    #[test]
    fn ladders_climb_where_the_view_points_and_jump_pushes_off() {
        let mut sim = Sim::new(ladder_world());
        sim.ps.origin = [16.0, 0.0, 0.0];
        sim.settle();
        // Face the ladder (yaw 180 looks along -x) and press forward.
        for _ in 0..50 {
            sim.tick(127, 0, 0, 180.0);
        }
        assert!(sim.ps.pm_flags & pm_flags::LADDER != 0, "not on the ladder");
        assert!(
            (sim.ps.velocity[2] - LADDER_CLIMB_SPEED).abs() < 1.0,
            "climb speed {:?}",
            sim.ps.velocity
        );
        assert!(sim.ps.origin[2] > 50.0, "{:?}", sim.ps.origin);

        let height = sim.ps.origin[2];
        for _ in 0..10 {
            sim.tick(0, 0, 0, 180.0);
        }
        assert!(
            (sim.ps.origin[2] - height).abs() < 0.5,
            "slid on the ladder"
        );

        sim.tick(0, 0, buttons::JUMP, 180.0);
        assert!(sim.ps.pm_flags & pm_flags::LADDER == 0);
        assert!(sim.ps.velocity[0] > 200.0, "{:?}", sim.ps.velocity);
    }

    #[test]
    fn strafing_into_a_ladder_at_your_side_climbs_it() {
        let mut sim = Sim::new(ladder_world());
        sim.ps.origin = [16.0, 0.0, 0.0];
        sim.settle();
        // Look along the ladder (+y, yaw 90): the ladder (at -x) is on the left. Only strafe left.
        for _ in 0..50 {
            sim.tick(0, -127, 0, 90.0);
        }
        assert!(sim.ps.pm_flags & pm_flags::LADDER != 0, "strafing did not grab the ladder");
        assert!(sim.ps.origin[2] > 50.0, "did not climb: {:?}", sim.ps.origin);
    }

    /// Floor at `z = 0` and a wall at `x = 100` that behaves like an IW4 mesh: a sweep that starts
    /// within the clip epsilon of it and does not move away reports a hit at fraction 0, even when
    /// it runs parallel to the wall.
    struct MeshWall;

    const MESH_WALL_X: f32 = 100.0;
    const MESH_CLIP_EPSILON: f32 = 0.125;

    impl CollisionBackend for MeshWall {
        fn trace(&self, input: GroundTraceInput) -> Trace {
            let floor = Floor.trace(input);
            let gap = |x: f32| MESH_WALL_X - (x + input.maxs[0]);
            let start = gap(input.start[0]);
            let end = gap(input.end[0]);
            let toward = input.end[0] - input.start[0];
            let fraction = if start < MESH_CLIP_EPSILON && toward >= 0.0 {
                0.0
            } else if toward > 0.0 && end < MESH_CLIP_EPSILON {
                ((start - MESH_CLIP_EPSILON) / toward).clamp(0.0, 1.0)
            } else {
                return floor;
            };
            if floor.fraction <= fraction {
                return floor;
            }
            Trace {
                fraction,
                normal: [-1.0, 0.0, 0.0],
                endpos: core::array::from_fn(|axis| {
                    input.start[axis] + (input.end[axis] - input.start[axis]) * fraction
                }),
                hit_type: trace_iw4::HITTYPE_ENTITY,
                hit_id: trace_iw4::ENTITYNUM_WORLD,
                ..Trace::default()
            }
        }
    }

    #[test]
    fn pushing_into_a_mesh_wall_in_the_air_still_falls() {
        let mut sim = Sim::new(MeshWall);
        sim.ps.origin = [MESH_WALL_X - 15.0 - 0.1, 0.0, 50.0];
        for _ in 0..150 {
            sim.tick(127, 0, 0, 0.0);
        }
        assert!(sim.grounded(), "hung at {:?} vel {:?}", sim.ps.origin, sim.ps.velocity);
        assert!(sim.ps.origin[2] < 1.0, "{:?}", sim.ps.origin);
        assert!(sim.ps.origin[0] + 15.0 <= MESH_WALL_X, "{:?}", sim.ps.origin);
    }

    /// Convex brushes: a point is inside one when `dot(p, normal) < offset` for all its planes.
    /// The box sweep is Quake's `CM_TraceThroughBrush` (planes pushed out by the hull).
    /// Planes of every brush back to back, and how many each brush has.
    struct Brushes {
        planes: [([f32; 3], f32); 8],
        lengths: [usize; 2],
    }

    impl CollisionBackend for Brushes {
        fn trace(&self, input: GroundTraceInput) -> Trace {
            const EPS: f32 = 0.031_25;
            let mut best = Trace {
                fraction: 1.0,
                endpos: input.end,
                hit_type: trace_iw4::HITTYPE_ENTITY,
                hit_id: trace_iw4::ENTITYNUM_WORLD,
                ..Trace::default()
            };
            let mut first = 0;
            'brush: for &len in &self.lengths {
                let brush = &self.planes[first..first + len];
                first += len;
                let (mut enter, mut leave) = (-1.0_f32, 1.0_f32);
                let (mut start_out, mut end_out) = (false, false);
                let mut normal = [0.0; 3];
                for &(n, offset) in brush {
                    let corner: [f32; 3] =
                        core::array::from_fn(|i| if n[i] > 0.0 { input.mins[i] } else { input.maxs[i] });
                    let dist = offset - dot(&corner, &n);
                    let d1 = dot(&input.start, &n) - dist;
                    let d2 = dot(&input.end, &n) - dist;
                    if d2 > 0.0 {
                        end_out = true;
                    }
                    if d1 > 0.0 {
                        start_out = true;
                    }
                    if d1 > 0.0 && d2 >= d1 {
                        continue 'brush;
                    }
                    if d1 <= 0.0 && d2 <= 0.0 {
                        continue;
                    }
                    if d1 > d2 {
                        let f = (d1 - EPS) / (d1 - d2);
                        if f > enter {
                            enter = f;
                            normal = n;
                        }
                    } else {
                        let f = (d1 + EPS) / (d1 - d2);
                        leave = leave.min(f);
                    }
                }
                if !start_out {
                    best.startsolid = 1;
                    if !end_out {
                        best.allsolid = 1;
                    }
                    best.fraction = 0.0;
                    best.endpos = input.start;
                    continue;
                }
                if enter < leave && enter > -1.0 && enter < best.fraction {
                    let f = enter.max(0.0);
                    best.fraction = f;
                    best.normal = normal;
                    best.walkable = u8::from(normal[2] >= 0.7);
                    best.endpos =
                        core::array::from_fn(|i| input.start[i] + (input.end[i] - input.start[i]) * f);
                }
            }
            best
        }
    }

    /// A 35 degree slope rising along +y with a flange around it like the pipe on mp_rust: a lip
    /// 3 units proud of the slope and 4 long, its leading top edge bevelled to a 0.58 normal.
    fn flange_slope() -> (Brushes, [f32; 3], [f32; 3]) {
        let (s, c) = 35.0_f32.to_radians().sin_cos();
        let up_slope = [0.0, c, s];
        let normal = [0.0, -s, c];
        let at = |along: f32, above: f32| -> [f32; 3] {
            core::array::from_fn(|i| up_slope[i] * along + normal[i] * above)
        };
        let lip_start = 160.0;
        let front = up_slope.map(|v| -v);
        // Between the lip's front and its top: z of the normal 0.58.
        let phi = (0.58_f32.acos() - 35.0_f32.to_radians()).max(0.0);
        let bevel: [f32; 3] = core::array::from_fn(|i| front[i] * phi.sin() + normal[i] * phi.cos());
        let corner = at(lip_start, 3.0);
        let slope = (normal, 0.0);
        let planes = [
            slope,
            (normal, dot(&normal, &at(0.0, 3.0))),
            (front, dot(&front, &at(lip_start, 0.0))),
            (up_slope, dot(&up_slope, &at(lip_start + 4.0, 0.0))),
            (normal.map(|v| -v), dot(&normal.map(|v| -v), &at(0.0, -1.0))),
            (bevel, dot(&bevel, &corner) - 1.0),
            ([1.0, 0.0, 0.0], 1000.0),
            ([-1.0, 0.0, 0.0], 1000.0),
        ];
        (Brushes { planes, lengths: [1, 7] }, at(20.0, 0.0), up_slope)
    }

    #[test]
    fn climbing_a_slope_over_a_flange_keeps_speed_and_ground() {
        let (world, start, _) = flange_slope();
        let mut sim = Sim::new(world);
        sim.ps.origin = [start[0], start[1], start[2] + 12.0];
        sim.settle();
        assert!(sim.grounded(), "never landed at {:?}", sim.ps.origin);
        let mut slowest = f32::MAX;
        let mut airborne = 0;
        for tick in 0..120 {
            sim.tick(127, 0, 0, 90.0);
            if tick > 40 {
                slowest = slowest.min(sim.flat_speed());
                airborne += usize::from(!sim.grounded());
            }
        }
        let passed = sim.ps.origin[1] > 160.0 * 35.0_f32.to_radians().cos() + 20.0;
        assert!(passed, "never got past the flange: {:?}", sim.ps.origin);
        assert!(airborne <= 2, "launched off the slope for {airborne} ticks");
        assert!(slowest > 150.0, "the flange cut the climb to {slowest}");
    }
}
