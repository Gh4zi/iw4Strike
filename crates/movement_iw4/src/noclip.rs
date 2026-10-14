//! Noclip (MW2's `noclip`, `PM_NoclipMove`, as Quake 3's): the player flies where they look,
//! through everything, and slows to a stop when let go. Jump rises, crouch sinks, the walk key
//! halves the speed.

use playerstate_iw4::{ENTITYNUM_NONE, PlayerState, UserCmd, buttons, pm_flags};

use crate::pml::Pml;

/// Full speed, units a second.
const SPEED: f32 = 1000.0;
/// Speed lost a second, as a share of the speed (at least [`STOP_SPEED`]'s): ground friction
/// half again.
const FRICTION: f32 = 9.0;
const STOP_SPEED: f32 = 100.0;
const ACCELERATE: f32 = 10.0;

pub(crate) fn pmove(ps: &mut PlayerState, cmd: &UserCmd, pml: &Pml) {
    let dt = pml.frametime;
    ps.ground_entity_num = ENTITYNUM_NONE;
    ps.pm_flags &= !(pm_flags::LADDER | pm_flags::JUMPING);

    let speed = length(ps.velocity);
    if speed < 1.0 {
        ps.velocity = [0.0; 3];
    } else {
        let drop = speed.max(STOP_SPEED) * FRICTION * dt;
        let scale = (speed - drop).max(0.0) / speed;
        for v in &mut ps.velocity {
            *v *= scale;
        }
    }

    let forward = f32::from(cmd.forwardmove) / 127.0;
    let side = f32::from(cmd.rightmove) / 127.0;
    let mut up = 0.0;
    if cmd.buttons & buttons::JUMP != 0 {
        up += 1.0;
    }
    if cmd.buttons & buttons::CROUCH != 0 {
        up -= 1.0;
    }
    let mut wish = [0.0; 3];
    for (i, w) in wish.iter_mut().enumerate() {
        *w = pml.forward[i] * forward + pml.right[i] * side;
    }
    wish[2] += up;
    let amount = length(wish);
    if amount > 0.0 {
        let full = if cmd.buttons & buttons::SPRINT != 0 { SPEED * 0.5 } else { SPEED };
        let wish_speed = full * amount.min(1.0);
        let dir = wish.map(|w| w / amount);
        let current: f32 = ps.velocity.iter().zip(dir).map(|(v, d)| v * d).sum();
        let add = wish_speed - current;
        if add > 0.0 {
            let accel = (ACCELERATE * dt * wish_speed).min(add);
            for (v, d) in ps.velocity.iter_mut().zip(dir) {
                *v += accel * d;
            }
        }
    }

    for (o, v) in ps.origin.iter_mut().zip(ps.velocity) {
        *o += v * dt;
    }
}

fn length(v: [f32; 3]) -> f32 {
    libm::sqrtf(v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
}
