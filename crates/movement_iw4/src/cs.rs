//! Counter-Strike 1.6 player movement on IW4 collision.
//!
//! The rules follow GoldSrc `pm_shared` as CS 1.6 shipped it: ground and air acceleration with
//! the 30 u/s air wish cap that makes strafing and surfing work, stop-speed friction with edge
//! friction, jump stamina, the bunny-hop speed cap, and the duck state machine with duck-jumps
//! and the double-duck hop. Every number lives in [`MovementProfile`] so another ruleset (CS:S)
//! can sit beside [`CS16`].
//!
//! GoldSrc keeps the player origin at the hull centre; IW4 keeps it at the feet. Duck code works
//! in centre terms and converts, so the feet move exactly as the hull centre would in GoldSrc.

use playerstate_iw4::{ENTITYNUM_NONE, PlayerState, UserCmd, buttons, cs_duck, eflags, pm_flags};

use crate::{
    CollisionBackend, GroundTraceInput, LadderAttachBackend, LadderMoveContext,
    LadderTraceHit, MoveBounds, Pml, PmoveResult, PmoveSingleContext, check_ladder_move,
    complete_ground_trace, drop_timers, footstep_event, footsteps_bob_cycle, jump,
    ladder_footsteps, ladder_move, should_make_footsteps, slide_move, update_ads_frac,
    update_ads_intent, update_view_angles,
};

/// Tunables of one movement ruleset. Units are IW4/GoldSrc inches and seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovementProfile {
    /// Speed cap without a weapon override (`sv_maxspeed` and the knife's 250).
    pub max_speed: f32,
    /// Client key speeds (`cl_forwardspeed`, `cl_backspeed`, `cl_sidespeed`) for a full stick.
    pub forward_speed: f32,
    pub back_speed: f32,
    pub side_speed: f32,
    /// Walk key multiplier on the clamped command (`cl_movespeedkey`).
    pub walk_scale: f32,
    /// Command multiplier while ducking (`PLAYER_DUCKING_MULTIPLIER`).
    pub duck_scale: f32,
    /// `sv_accelerate`.
    pub accelerate: f32,
    /// `sv_airaccelerate`.
    pub air_accelerate: f32,
    /// Wish speed cap inside air acceleration. The source of air-strafing and surfing.
    pub air_wish_cap: f32,
    /// `sv_friction`, `edgefriction`, `sv_stopspeed`.
    pub friction: f32,
    pub edge_friction: f32,
    pub stop_speed: f32,
    /// Jump apex height; launch speed is `sqrt(2 * jump_gravity * jump_height)`.
    pub jump_height: f32,
    pub jump_gravity: f32,
    /// Jumping above `bhop_cap_factor * max_speed` scales velocity to that times `bhop_cap_scale`.
    pub bhop_cap_factor: f32,
    pub bhop_cap_scale: f32,
    /// Stamina set by a jump (ms) and the per-ms slowdown it applies (`fuser2`).
    pub jump_stamina_ms: f32,
    pub stamina_slow_per_ms: f32,
    /// Frame time the stamina slowdown was tuned at; other tick lengths compound to match it.
    pub stamina_reference_frametime: f32,
    /// Hull heights and eye heights above the feet.
    pub stand_height: f32,
    pub duck_height: f32,
    pub stand_eye: f32,
    pub duck_eye: f32,
    /// Ground duck transition time (`TIME_TO_DUCK`).
    pub duck_seconds: f32,
    /// A walkable ground snap distance (`PM_CategorizePosition` probes 2 units down).
    pub ground_snap: f32,
    /// Upward speed above which the player can never be on ground.
    pub ground_launch_speed: f32,
    pub step_size: f32,
    /// `sv_maxvelocity`, per axis.
    pub max_velocity: f32,
}

/// Counter-Strike 1.6 defaults (ReGameDLL `pm_shared`, CS 1.6 server cvars).
pub const CS16: MovementProfile = MovementProfile {
    max_speed: 250.0,
    forward_speed: 400.0,
    back_speed: 400.0,
    side_speed: 400.0,
    walk_scale: 0.52,
    duck_scale: 0.333,
    accelerate: 5.0,
    air_accelerate: 10.0,
    air_wish_cap: 30.0,
    friction: 4.0,
    edge_friction: 2.0,
    stop_speed: 75.0,
    jump_height: 45.0,
    jump_gravity: 800.0,
    bhop_cap_factor: 1.2,
    bhop_cap_scale: 0.8,
    jump_stamina_ms: 1_315.789_4,
    stamina_slow_per_ms: 0.019,
    stamina_reference_frametime: 0.01,
    stand_height: 72.0,
    duck_height: 36.0,
    stand_eye: 53.0,
    duck_eye: 30.0,
    duck_seconds: 0.4,
    ground_snap: 2.0,
    ground_launch_speed: 180.0,
    step_size: 18.0,
    max_velocity: 2000.0,
};

const VIEW_HEIGHT_STAND: i32 = 0x3c;

const VIEW_HEIGHT_CROUCH: i32 = 0x28;

const ANIM_EVENT_STAND_TO_CROUCH: u8 = 13;

const ANIM_EVENT_CROUCH_TO_STAND: u8 = 14;

/// Hull for the player's current duck state.
#[must_use]
pub fn hull(ps: &PlayerState, profile: &MovementProfile, base: MoveBounds) -> MoveBounds {
    let mut bounds = base;
    bounds.mins[2] = 0.0;
    bounds.maxs[2] = if ps.cs_duck_state & cs_duck::DUCKED != 0 {
        profile.duck_height
    } else {
        profile.stand_height
    };
    bounds
}

/// Commanded movement in GoldSrc units per second, before ducking or clamping.
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
    profile: MovementProfile,
    mut pml: Pml,
) -> PmoveResult {
    let profile = MovementProfile {
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
    let mut bounds = hull(ps, &profile, context.bounds);

    categorize(ps, &mut pml, cmd, &profile, bounds, collision);
    if pml.walking == 0 {
        ps.cs_fall_velocity = -ps.velocity[2];
    }
    drop_timers(ps, &pml);

    let mut wish = command_wish(cmd, &profile, walk_key);
    duck(
        ps,
        &mut pml,
        cmd,
        context.old_buttons,
        &profile,
        &mut bounds,
        &mut wish,
        collision,
    );

    {
        let mut ladder_backend = LadderBackend { collision, bounds };
        if check_ladder_move(
            ps,
            crate::ladder::cs_ladder_context(cmd, pml.walking != 0, pml.forward, pml.right),
            &mut ladder_backend,
        ) {
            pml.record_jump_animation(crate::JumpAnimation::Forward, true);
        }
    }

    if ps.pm_flags & pm_flags::LADDER != 0 {
        ladder_move(
            ps,
            &mut pml,
            cmd,
            LadderMoveContext {
                jump: context.walk.jump,
                old_buttons: context.old_buttons,
                player_spectate_speed_scale: context.air.player_spectate_speed_scale,
            },
            bounds,
            collision,
        );
    } else {
        jump(ps, &mut pml, cmd, context.old_buttons, &profile);
        if pml.walking != 0 {
            ps.velocity[2] = 0.0;
            friction(ps, &pml, &profile, bounds, collision);
        }
        check_velocity(ps, &profile);
        if pml.walking != 0 {
            walk_move(ps, &mut pml, wish, &profile, bounds, collision);
        } else {
            air_move(ps, &pml, wish, &profile, bounds, collision);
        }
    }

    categorize(ps, &mut pml, cmd, &profile, bounds, collision);
    check_velocity(ps, &profile);
    let mut landing_speed = 0.0;
    if pml.walking != 0 {
        ps.velocity[2] = 0.0;
        landing_speed = ps.cs_fall_velocity.max(0.0);
        ps.cs_fall_velocity = 0.0;
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

    // GoldSrc keeps float velocity; IW4's integer snap would bias air-strafe gains.
    PmoveResult {
        pml,
        bounds,
        stance_event,
        reset_torso: stance_event.is_some(),
        landing_speed,
        unstuck: false,
    }
}

/// CS 1.6 multiplayer fall damage for a landing at `speed` (`FlPlayerFallDamage`): nothing up to
/// 500 u/s, then 100 points per 600 u/s scaled by 1.25.
#[must_use]
pub fn fall_damage(speed: f32) -> i32 {
    const SAFE_FALL_SPEED: f32 = 500.0;
    const FATAL_FALL_SPEED: f32 = 1100.0;
    if speed <= SAFE_FALL_SPEED {
        return 0;
    }
    let damage = (speed - SAFE_FALL_SPEED) * (100.0 / (FATAL_FALL_SPEED - SAFE_FALL_SPEED)) * 1.25;
    if damage < 1.0 { 0 } else { damage as i32 }
}

fn reduce_timers(ps: &mut PlayerState, msec: i32) {
    let msec = msec as f32;
    ps.cs_duck_time = (ps.cs_duck_time - msec).max(0.0);
    ps.cs_stamina = (ps.cs_stamina - msec).max(0.0);
}

/// `PM_CategorizePosition`: snap onto walkable ground within reach, then let the IW4 ground
/// trace record the plane, ground entity and landing.
fn categorize<C: CollisionBackend>(
    ps: &mut PlayerState,
    pml: &mut Pml,
    cmd: &UserCmd,
    profile: &MovementProfile,
    bounds: MoveBounds,
    collision: &C,
) {
    if ps.velocity[2] <= profile.ground_launch_speed {
        let below = [
            ps.origin[0],
            ps.origin[1],
            ps.origin[2] - profile.ground_snap,
        ];
        let trace = hull_trace(collision, ps.origin, below, bounds);
        if trace.startsolid == 0
            && trace.allsolid == 0
            && trace.fraction < 1.0
            && trace.normal[2] >= 0.7
        {
            ps.origin = trace.endpos;
        }
    }
    complete_ground_trace(ps, pml, bounds, cmd.forwardmove, collision);
    if ps.velocity[2] > profile.ground_launch_speed && pml.walking != 0 {
        ps.ground_entity_num = ENTITYNUM_NONE;
        pml.walking = 0;
        pml.ground_plane = 0;
        pml.almost_ground_plane = 0;
    }
}

/// Key movement in GoldSrc units: a full stick is `cl_forwardspeed`, the total is clamped to the
/// speed cap (`PM_CheckParameters`), and the walk key scales what is left.
fn command_wish(cmd: &UserCmd, profile: &MovementProfile, walk_key: bool) -> Wish {
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

/// `PM_Duck` / `PM_UnDuck`.
#[allow(clippy::too_many_arguments)]
fn duck<C: CollisionBackend>(
    ps: &mut PlayerState,
    pml: &mut Pml,
    cmd: &UserCmd,
    old_buttons: u32,
    profile: &MovementProfile,
    bounds: &mut MoveBounds,
    wish: &mut Wish,
    collision: &C,
) {
    let held = cmd.buttons & buttons::CROUCH != 0;
    let pressed = held && old_buttons & buttons::CROUCH == 0;
    let mut state = ps.cs_duck_state;
    let on_ground = pml.walking != 0;
    let half_gap = (profile.stand_height - profile.duck_height) * 0.5;

    if held || state != 0 {
        wish.forward *= profile.duck_scale;
        wish.side *= profile.duck_scale;
    }

    if held {
        if pressed && state & cs_duck::DUCKED == 0 {
            ps.cs_duck_time = 1000.0;
            state |= cs_duck::IN_DUCK;
        }
        if state & cs_duck::IN_DUCK != 0 {
            let elapsed_done = ps.cs_duck_time / 1000.0 <= 1.0 - profile.duck_seconds;
            if elapsed_done || !on_ground {
                // The duck hull keeps the GoldSrc centre: in the air that lifts the feet.
                if !on_ground {
                    ps.origin[2] += half_gap;
                }
                state = (state & !cs_duck::IN_DUCK) | cs_duck::DUCKED;
                bounds.maxs[2] = profile.duck_height;
            }
        }
    } else if state != 0 {
        // Standing up raises the GoldSrc centre by half the gap on ground, whichever hull is
        // active. From a half-finished duck that is the double-duck hop.
        let old_height = if state & cs_duck::DUCKED != 0 {
            profile.duck_height
        } else {
            profile.stand_height
        };
        let mut centre = ps.origin[2] + old_height * 0.5;
        if on_ground {
            centre += half_gap;
        }
        let before = ps.origin[2];
        let feet = [
            ps.origin[0],
            ps.origin[1],
            centre - profile.stand_height * 0.5,
        ];
        let mut stand = *bounds;
        stand.maxs[2] = profile.stand_height;
        let trace = hull_trace(collision, feet, feet, stand);
        if trace.startsolid == 0 && trace.allsolid == 0 {
            ps.origin = feet;
            state = 0;
            ps.cs_duck_time = 0.0;
            *bounds = stand;
            if feet[2] > before + profile.ground_snap {
                pml.walking = 0;
                pml.ground_plane = 0;
                pml.almost_ground_plane = 0;
                ps.ground_entity_num = ENTITYNUM_NONE;
            }
        }
    }
    ps.cs_duck_state = state;

    ps.view_height_current = if state & cs_duck::DUCKED != 0 {
        profile.duck_eye
    } else if state & cs_duck::IN_DUCK != 0 {
        let time = 1.0 - ps.cs_duck_time / 1000.0;
        let fraction = spline_fraction(time, 1.0 / profile.duck_seconds);
        profile.duck_eye * fraction + profile.stand_eye * (1.0 - fraction)
    } else {
        profile.stand_eye
    };
    ps.view_height_lerp_time = 0;

    let crouched = state != 0;
    if crouched {
        ps.pm_flags |= pm_flags::CROUCH;
        ps.e_flags |= eflags::DUCK;
        ps.view_height_target = VIEW_HEIGHT_CROUCH;
    } else {
        ps.pm_flags &= !pm_flags::CROUCH;
        ps.e_flags &= !eflags::DUCK;
        ps.view_height_target = VIEW_HEIGHT_STAND;
    }
}

fn spline_fraction(value: f32, scale: f32) -> f32 {
    let value = scale * value;
    let squared = value * value;
    3.0 * squared - 2.0 * squared * value
}

/// `PM_Jump` with `PM_PreventMegaBunnyJumping` and stamina.
fn jump(
    ps: &mut PlayerState,
    pml: &mut Pml,
    cmd: &UserCmd,
    old_buttons: u32,
    profile: &MovementProfile,
) {
    if cmd.buttons & buttons::JUMP == 0
        || pml.walking == 0
        || old_buttons & buttons::JUMP != 0
        || ps.cs_duck_state == cs_duck::IN_DUCK | cs_duck::DUCKED
    {
        return;
    }

    pml.walking = 0;
    pml.ground_plane = 0;
    pml.almost_ground_plane = 0;
    ps.ground_entity_num = ENTITYNUM_NONE;
    ps.jump_origin_z = ps.origin[2];
    ps.jump_time = cmd.server_time;

    let cap = profile.bhop_cap_factor * profile.max_speed;
    let speed = length(&ps.velocity);
    if cap > 0.0 && speed > cap {
        let fraction = cap / speed * profile.bhop_cap_scale;
        for axis in &mut ps.velocity {
            *axis *= fraction;
        }
    }

    ps.velocity[2] = libm::sqrtf(2.0 * profile.jump_gravity * profile.jump_height);
    if ps.cs_stamina > 0.0 {
        ps.velocity[2] *= stamina_ratio(ps.cs_stamina, profile);
    }
    ps.cs_stamina = profile.jump_stamina_ms;

    jump::event(ps, pml.ground_trace[4]);
    let animation = if cmd.forwardmove >= 0 {
        crate::JumpAnimation::Forward
    } else {
        crate::JumpAnimation::Backward
    };
    pml.record_jump_animation(animation, true);
}

fn stamina_ratio(stamina: f32, profile: &MovementProfile) -> f32 {
    (100.0 - stamina * profile.stamina_slow_per_ms) * 0.01
}

/// `PM_Friction` with edge friction: twice the friction when the ground ahead drops away.
fn friction<C: CollisionBackend>(
    ps: &mut PlayerState,
    pml: &Pml,
    profile: &MovementProfile,
    bounds: MoveBounds,
    collision: &C,
) {
    let speed = length(&ps.velocity);
    if speed < 0.1 {
        return;
    }

    // GoldSrc probes with the hull centred on the feet, 16 units ahead, 34 units down.
    let half = bounds.maxs[2] * 0.5;
    let start = [
        ps.origin[0] + ps.velocity[0] / speed * 16.0,
        ps.origin[1] + ps.velocity[1] / speed * 16.0,
        ps.origin[2] - half,
    ];
    let stop = [start[0], start[1], start[2] - 34.0];
    let edge = hull_trace(collision, start, stop, bounds);
    let mut friction = profile.friction;
    if edge.fraction >= 1.0 && edge.startsolid == 0 {
        friction *= profile.edge_friction;
    }

    let control = speed.max(profile.stop_speed);
    let drop = friction * control * pml.frametime;
    let scale = (speed - drop).max(0.0) / speed;
    for axis in &mut ps.velocity {
        *axis *= scale;
    }
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

fn accelerate(ps: &mut PlayerState, frametime: f32, dir: [f32; 3], wish_speed: f32, accel: f32) {
    let current = dot(&ps.velocity, &dir);
    let add = wish_speed - current;
    if add <= 0.0 {
        return;
    }
    let step = (accel * frametime * wish_speed).min(add);
    for (axis, component) in ps.velocity.iter_mut().zip(dir) {
        *axis += step * component;
    }
}

fn air_accelerate(
    ps: &mut PlayerState,
    frametime: f32,
    dir: [f32; 3],
    wish_speed: f32,
    profile: &MovementProfile,
) {
    let capped = wish_speed.min(profile.air_wish_cap);
    let current = dot(&ps.velocity, &dir);
    let add = capped - current;
    if add <= 0.0 {
        return;
    }
    let step = (profile.air_accelerate * wish_speed * frametime).min(add);
    for (axis, component) in ps.velocity.iter_mut().zip(dir) {
        *axis += step * component;
    }
}

/// `PM_WalkMove`: accelerate on the plane, then move directly or take the better of a plain
/// slide and a stepped slide.
fn walk_move<C: CollisionBackend>(
    ps: &mut PlayerState,
    pml: &mut Pml,
    wish: Wish,
    profile: &MovementProfile,
    bounds: MoveBounds,
    collision: &C,
) {
    if ps.cs_stamina > 0.0 {
        let ratio = libm::powf(
            stamina_ratio(ps.cs_stamina, profile).max(0.0),
            pml.frametime / profile.stamina_reference_frametime,
        );
        ps.velocity[0] *= ratio;
        ps.velocity[1] *= ratio;
    }

    let (dir, mut wish_speed) = wish_direction(pml, wish);
    wish_speed = wish_speed.min(profile.max_speed);

    ps.velocity[2] = 0.0;
    accelerate(ps, pml.frametime, dir, wish_speed, profile.accelerate);
    ps.velocity[2] = 0.0;

    if length(&ps.velocity) < 1.0 {
        ps.velocity = [0.0; 3];
        return;
    }

    let dest = [
        ps.origin[0] + ps.velocity[0] * pml.frametime,
        ps.origin[1] + ps.velocity[1] * pml.frametime,
        ps.origin[2],
    ];
    let direct = hull_trace(collision, ps.origin, dest, bounds);
    if direct.fraction >= 1.0 && direct.startsolid == 0 {
        ps.origin = dest;
        return;
    }

    let original = ps.origin;
    let original_velocity = ps.velocity;

    let _ = slide_move(
        ps,
        pml,
        collision,
        bounds.mins,
        bounds.maxs,
        bounds.tracemask,
        None,
    );
    let down = ps.origin;
    let down_velocity = ps.velocity;

    ps.origin = original;
    ps.velocity = original_velocity;
    let raised = [original[0], original[1], original[2] + profile.step_size];
    let up_trace = hull_trace(collision, original, raised, bounds);
    if up_trace.startsolid == 0 && up_trace.allsolid == 0 {
        ps.origin = up_trace.endpos;
    }
    let _ = slide_move(
        ps,
        pml,
        collision,
        bounds.mins,
        bounds.maxs,
        bounds.tracemask,
        None,
    );

    let lowered = [ps.origin[0], ps.origin[1], ps.origin[2] - profile.step_size];
    let down_trace = hull_trace(collision, ps.origin, lowered, bounds);
    let stepped_onto_ground = down_trace.fraction < 1.0 && down_trace.normal[2] >= 0.7;
    if stepped_onto_ground && down_trace.startsolid == 0 && down_trace.allsolid == 0 {
        ps.origin = down_trace.endpos;
    }
    let down_dist = flat_distance_sq(&down, &original);
    let up_dist = flat_distance_sq(&ps.origin, &original);
    if !stepped_onto_ground || down_dist > up_dist {
        ps.origin = down;
        ps.velocity = down_velocity;
    } else {
        ps.velocity[2] = down_velocity[2];
    }
}

/// `PM_AirMove`: capped air acceleration, then a gravity slide with no stepping.
fn air_move<C: CollisionBackend>(
    ps: &mut PlayerState,
    pml: &Pml,
    wish: Wish,
    profile: &MovementProfile,
    bounds: MoveBounds,
    collision: &C,
) {
    let (dir, mut wish_speed) = wish_direction(pml, wish);
    wish_speed = wish_speed.min(profile.max_speed);
    air_accelerate(ps, pml.frametime, dir, wish_speed, profile);
    let gravity = ps.gravity as f32;
    let _ = slide_move(
        ps,
        pml,
        collision,
        bounds.mins,
        bounds.maxs,
        bounds.tracemask,
        Some(gravity),
    );
}

fn check_velocity(ps: &mut PlayerState, profile: &MovementProfile) {
    for axis in &mut ps.velocity {
        if axis.is_nan() {
            *axis = 0.0;
        }
        *axis = axis.clamp(-profile.max_velocity, profile.max_velocity);
    }
}

fn hull_trace<C: CollisionBackend>(
    collision: &C,
    start: [f32; 3],
    end: [f32; 3],
    bounds: MoveBounds,
) -> trace_iw4::Trace {
    collision.trace(GroundTraceInput {
        start,
        end,
        mins: bounds.mins,
        maxs: bounds.maxs,
        tracemask: bounds.tracemask,
    })
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

fn flat_distance_sq(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        AdsFracContext, AdsIntentContext, AirMoveContext, CmdScaleWalkContext, JumpLaunchContext,
        MeleeChargeWeaponDelays, SprintContext, ViewAngleClamp, WalkMoveContext,
    };
    use trace_iw4::Trace;

    const TICK_MS: i32 = 10;

    const GOLDSRC: Option<crate::rules::Ruleset> = Some(crate::rules::Ruleset::GoldSrc(CS16));

    /// Open world with one floor at `z = 0`.
    pub(crate) struct Floor;

    impl CollisionBackend for Floor {
        fn trace(&self, input: GroundTraceInput) -> Trace {
            let start = input.start[2] + input.mins[2];
            let end = input.end[2] + input.mins[2];
            let mut trace = Trace {
                fraction: 1.0,
                endpos: input.end,
                hit_type: trace_iw4::HITTYPE_ENTITY,
                hit_id: trace_iw4::ENTITYNUM_WORLD,
                ..Trace::default()
            };
            if start < -0.001 {
                trace.startsolid = 1;
                trace.allsolid = u8::from(end < -0.001);
                trace.fraction = 0.0;
                trace.endpos = input.start;
                trace.normal = [0.0, 0.0, 1.0];
                trace.walkable = 1;
                return trace;
            }
            if end >= 0.0 {
                return trace;
            }
            let fraction = (start / (start - end)).clamp(0.0, 1.0);
            trace.fraction = fraction;
            for axis in 0..3 {
                trace.endpos[axis] =
                    input.start[axis] + (input.end[axis] - input.start[axis]) * fraction;
            }
            trace.endpos[2] = -input.mins[2];
            trace.normal = [0.0, 0.0, 1.0];
            trace.walkable = 1;
            trace
        }
    }

    pub(crate) fn context(old_buttons: u32) -> PmoveSingleContext {
        let air = AirMoveContext {
            player_spectate_speed_scale: 1.0,
            shellshock_gravity_scale: 1.0,
            shellshock_gravity_bias: 0.0,
        };
        PmoveSingleContext {
            walk: WalkMoveContext {
                cmd_scale: CmdScaleWalkContext {
                    player_back_speed_scale: 0.7,
                    player_strafe_speed_scale: 0.8,
                    player_sprint_speed_scale: 1.5,
                    player_last_stand_crawl_speed_scale: 0.15,
                    weapon_move_speed_scale: 1.0,
                    weapon_ads_move_speed_scale: 1.0,
                    shellshock_affects_movement: false,
                },
                weapon_move_scale: 1.0,
                old_buttons,
                jump: JumpLaunchContext {
                    jump_height: 39.0,
                    dive: false,
                    crouch_jump_scale: 1.0,
                    jump_ladder_push_vel: 128.0,
                },
                air,
            },
            air,
            bounds: MoveBounds {
                mins: [-15.0, -15.0, 0.0],
                maxs: [15.0, 15.0, 70.0],
                tracemask: 0x0281_0011,
            },
            view_angles: ViewAngleClamp {
                pitch_up: 85.0,
                pitch_down: 85.0,
                unclamped_pitch_bit: false,
            },
            sprint: SprintContext {
                weapon_max_sprint_time: 4000,
                sprint_forever: false,
                min_sprint_time_seconds: 1.0,
                sprint_delay_seconds: 0.0,
                sprint_forward_minimum: 105,
                stand_up_clear: true,
                sprint_recharge_pause_seconds: 0.0,
            },
            ads_intent: AdsIntentContext {
                ads_allowed: false,
                weapon_def_scope: false,
                sprint_hold_ads: false,
            },
            ads_frac: AdsFracContext::default(),
            melee_charge: MeleeChargeWeaponDelays {
                melee_delay_ms: 0,
                melee_charge_delay_ms: 0,
            },
            player_melee_range: 64.0,
            old_buttons,
            weapon_blocks_prone: false,
            can_hold_breath: false,
        }
    }

    struct Sim {
        ps: PlayerState,
        time: i32,
        old_buttons: u32,
    }

    impl Sim {
        fn new() -> Self {
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
                &Floor,
                &crate::FlatMantleAnimLength::default(),
                &crate::ZeroMantleRootDelta,
                GOLDSRC,
            );
            self.old_buttons = buttons;
        }

        fn flat_speed(&self) -> f32 {
            libm::sqrtf(
                self.ps.velocity[0] * self.ps.velocity[0]
                    + self.ps.velocity[1] * self.ps.velocity[1],
            )
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

    #[test]
    fn running_tops_out_at_the_knife_speed() {
        let mut sim = Sim::new();
        sim.settle();
        for _ in 0..300 {
            sim.tick(127, 0, 0, 0.0);
        }
        assert!(
            (sim.flat_speed() - 250.0).abs() < 1.0,
            "{}",
            sim.flat_speed()
        );
    }

    #[test]
    fn diagonal_running_is_no_faster() {
        let mut sim = Sim::new();
        sim.settle();
        for _ in 0..300 {
            sim.tick(127, 127, 0, 0.0);
        }
        assert!(
            (sim.flat_speed() - 250.0).abs() < 1.0,
            "{}",
            sim.flat_speed()
        );
    }

    #[test]
    fn walk_and_duck_speeds() {
        let mut sim = Sim::new();
        sim.settle();
        for _ in 0..300 {
            sim.tick(127, 0, buttons::SPRINT, 0.0);
        }
        assert!(
            (sim.flat_speed() - 130.0).abs() < 1.0,
            "walk {}",
            sim.flat_speed()
        );

        let mut sim = Sim::new();
        sim.settle();
        for _ in 0..300 {
            sim.tick(127, 0, buttons::CROUCH, 0.0);
        }
        assert!(
            (sim.flat_speed() - 83.25).abs() < 1.0,
            "duck {}",
            sim.flat_speed()
        );
    }

    #[test]
    fn friction_stops_a_runner_in_about_half_a_second() {
        let mut sim = Sim::new();
        sim.settle();
        for _ in 0..300 {
            sim.tick(127, 0, 0, 0.0);
        }
        let mut ticks = 0;
        while sim.flat_speed() > 0.0 && ticks < 200 {
            sim.tick(0, 0, 0, 0.0);
            ticks += 1;
        }
        assert!((45..=65).contains(&ticks), "stopped after {ticks} ticks");
    }

    #[test]
    fn jump_reaches_45_units_and_needs_a_fresh_press() {
        let mut sim = Sim::new();
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
        assert!((apex - 45.0).abs() < 1.5, "apex {apex}");
        assert_eq!(jumps, 1, "holding jump must not pogo");
    }

    #[test]
    fn back_to_back_jumps_lose_height_to_stamina() {
        let mut sim = Sim::new();
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
            // Release while airborne so the landing tick can press again.
            pressed = if grounded { buttons::JUMP } else { 0 };
        }
        assert!(landings >= 2);
        assert!(apexes[1] < apexes[0] - 2.0, "{apexes:?}");
    }

    #[test]
    fn bhop_above_the_cap_is_cut_to_240() {
        let mut sim = Sim::new();
        sim.settle();
        sim.ps.velocity = [400.0, 0.0, 0.0];
        sim.tick(0, 0, buttons::JUMP, 0.0);
        assert!(
            (sim.flat_speed() - 240.0).abs() < 3.0,
            "{}",
            sim.flat_speed()
        );
    }

    #[test]
    fn air_strafing_gains_speed() {
        let mut sim = Sim::new();
        sim.ps.origin = [0.0, 0.0, 5000.0];
        sim.ps.velocity = [250.0, 0.0, 0.0];
        for _ in 0..100 {
            let yaw = libm::atan2f(sim.ps.velocity[1], sim.ps.velocity[0]).to_degrees();
            // Strafe left while turning with the velocity, the classic strafe.
            sim.tick(0, -127, 0, yaw);
        }
        assert!(sim.flat_speed() > 300.0, "{}", sim.flat_speed());
    }

    #[test]
    fn ducking_in_the_air_lifts_the_feet() {
        let mut sim = Sim::new();
        sim.ps.origin = [0.0, 0.0, 500.0];
        sim.tick(0, 0, 0, 0.0);
        let before = sim.ps.origin[2];
        sim.tick(0, 0, buttons::CROUCH, 0.0);
        let rise = sim.ps.origin[2] - before;
        // 18 units of lift minus one tick of fall.
        assert!(rise > 16.0 && rise < 18.5, "rise {rise}");
        assert_eq!(sim.ps.cs_duck_state, cs_duck::DUCKED);
    }

    #[test]
    fn tapping_duck_on_the_ground_hops() {
        let mut sim = Sim::new();
        sim.settle();
        sim.tick(0, 0, buttons::CROUCH, 0.0);
        sim.tick(0, 0, 0, 0.0);
        assert!(sim.ps.origin[2] > 15.0, "{}", sim.ps.origin[2]);
        assert_eq!(sim.ps.cs_duck_state, 0);
    }

    #[test]
    fn full_duck_on_the_ground_keeps_the_feet_down() {
        let mut sim = Sim::new();
        sim.settle();
        for _ in 0..60 {
            sim.tick(0, 0, buttons::CROUCH, 0.0);
        }
        assert_eq!(sim.ps.cs_duck_state, cs_duck::DUCKED);
        assert!(sim.ps.origin[2].abs() < 0.5);
        assert!((sim.ps.view_height_current - CS16.duck_eye).abs() < 0.01);
        sim.tick(0, 0, 0, 0.0);
        assert_eq!(sim.ps.cs_duck_state, 0);
        assert!(sim.ps.origin[2].abs() < 0.5);
    }

    #[test]
    fn fall_damage_follows_cs_rules() {
        assert_eq!(fall_damage(500.0), 0);
        assert_eq!(fall_damage(740.0), 50);
        assert_eq!(fall_damage(1100.0), 125);
    }

    /// One infinite plane through the origin, solid on its negative side.
    pub(crate) struct Ramp {
        pub(crate) normal: [f32; 3],
    }

    impl Ramp {
        pub(crate) fn clearance(&self, origin: [f32; 3], mins: [f32; 3], maxs: [f32; 3]) -> f32 {
            let mut lowest = 0.0;
            for axis in 0..3 {
                let corner = if self.normal[axis] > 0.0 {
                    mins[axis]
                } else {
                    maxs[axis]
                };
                lowest += (origin[axis] + corner) * self.normal[axis];
            }
            lowest
        }
    }

    impl CollisionBackend for Ramp {
        fn trace(&self, input: GroundTraceInput) -> Trace {
            let start = self.clearance(input.start, input.mins, input.maxs);
            let end = self.clearance(input.end, input.mins, input.maxs);
            let mut trace = Trace {
                fraction: 1.0,
                endpos: input.end,
                hit_type: trace_iw4::HITTYPE_ENTITY,
                hit_id: trace_iw4::ENTITYNUM_WORLD,
                ..Trace::default()
            };
            let walkable = u8::from(self.normal[2] >= 0.7);
            if start < -0.001 {
                trace.startsolid = 1;
                trace.allsolid = u8::from(end < -0.001);
                trace.fraction = 0.0;
                trace.endpos = input.start;
                trace.normal = self.normal;
                trace.walkable = walkable;
                return trace;
            }
            if end >= 0.0 {
                return trace;
            }
            // Stop a hair short of the plane, as the real tracer's surface epsilon does.
            let fraction = ((start - 0.03125) / (start - end)).clamp(0.0, 1.0);
            trace.fraction = fraction;
            for axis in 0..3 {
                trace.endpos[axis] =
                    input.start[axis] + (input.end[axis] - input.start[axis]) * fraction;
            }
            trace.normal = self.normal;
            trace.walkable = walkable;
            trace
        }
    }

    #[test]
    fn surfing_a_steep_ramp_keeps_its_speed() {
        // 60 degree ramp facing +x: too steep to stand on, the CS surf case.
        let ramp = Ramp {
            normal: [0.866_025_4, 0.0, 0.5],
        };
        let mut ps = Sim::new().ps;
        ps.origin = [15.0 + 0.1, 0.0, 0.1];
        ps.velocity = [0.0, 600.0, 0.0];
        let mut time = 0;
        let mut old_buttons = 0;
        for _ in 0..100 {
            time += TICK_MS;
            let mut cmd = UserCmd {
                server_time: time,
                // Holding the strafe key that pushes into the ramp, as surfers do.
                rightmove: 127,
                ..UserCmd::default()
            };
            cmd.angles[1] = (180.0 * crate::ANGLE2SHORT) as i32;
            crate::pmove_with_rules(
                &mut ps,
                &mut cmd,
                context(old_buttons),
                &ramp,
                &crate::FlatMantleAnimLength::default(),
                &crate::ZeroMantleRootDelta,
                GOLDSRC,
            );
            old_buttons = cmd.buttons;
            let bounds = hull(&ps, &CS16, context(0).bounds);
            assert!(
                ramp.clearance(ps.origin, bounds.mins, bounds.maxs) > -0.01,
                "fell into the ramp at {:?}",
                ps.origin
            );
        }
        assert_eq!(
            ps.ground_entity_num, ENTITYNUM_NONE,
            "a 60 degree ramp is not ground"
        );
        assert!(ps.velocity[1] > 595.0, "lost surf speed: {:?}", ps.velocity);
    }
}
