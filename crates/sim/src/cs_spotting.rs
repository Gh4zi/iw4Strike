//! CS:GO's radar spotting (`CCSEntitySpotting`): twice a second every living player is checked
//! against each living, unblinded enemy; one looking their way with a clear line from eye to eye,
//! not through smoke, spots them for the whole enemy team, whose radar then shows them
//! (`ClientSnapshotMeta::cs_spotted`).

use crate::frame::FrameWorld;
use crate::match_state::ClientLifecycle;
use crate::world::{ClientId, Tick};
use entity_iw4::{TEAM_ALLIES, TEAM_AXIS};

/// How often spotting is worked out (`ENTITY_SPOT_FREQUENCY`).
const SPOT_INTERVAL_MS: i32 = 500;
/// Half the spotter's field of view (CS:GO's `fov` 90).
const SPOT_HALF_FOV_DEGREES: f32 = 45.0;
/// From this far on the spotter must look right at the target (`radarvisdistance`).
const SPOT_DISTANCE: f32 = 1000.0;
/// How quickly that narrows with distance (`radarvispow`).
const SPOT_POW: f32 = 0.4;
/// How closely a far spotter must look at the target (`radarvismaxdot`).
const SPOT_MAX_DOT: f32 = 0.996;
/// What blocks sight: solid brushes and MW2's no-sight brushes, not glass.
const SPOT_CONTENTS: u32 = 0x1 | 0x1000;

/// A CS smoke's cloud as sight sees it (`SmokeGrenadeRadius`, `CheckTotalSmokedLength`): this
/// round, centred [`CS_SMOKE_SIGHT_RAISE`] above where the grenade popped, for
/// [`CS_SMOKE_SIGHT_MS`] (12.5 s full, 4.55 s fading, gone a second later).
const CS_SMOKE_SIGHT_RADIUS: f32 = 166.0;
const CS_SMOKE_SIGHT_RAISE: f32 = 60.0;
pub(crate) const CS_SMOKE_SIGHT_MS: i32 = 18_050;
/// How much smoke a line of sight gets through (`IsLineBlockedBySmoke`).
const CS_SMOKE_SEE_THROUGH: f32 = 0.7 * CS_SMOKE_SIGHT_RADIUS;

struct Viewer {
    id: ClientId,
    team: i32,
    alive: bool,
    blind: bool,
    eye: [f32; 3],
    forward: [f32; 3],
}

pub(crate) fn update(world: &mut FrameWorld, tick: Tick) {
    let now = crate::level_time_ms(tick);
    if now.rem_euclid(SPOT_INTERVAL_MS) >= crate::MATCH_TICK_MS as i32 {
        return;
    }
    world.cs_smoke_clouds.retain(|(_, until)| *until > now);
    let clouds = world.cs_smoke_clouds.clone();
    let viewers: Vec<Viewer> = world
        .client_ids_sorted()
        .into_iter()
        .filter_map(|id| {
            let meta = world.client_meta(id)?;
            let ps = world.player(id)?;
            let flash = weapon_iw4::cs::CsFlash {
                duration_ms: ps.cs_flash_duration_ms,
                end_ms: ps.cs_flash_end_ms,
                alpha: ps.cs_flash_alpha,
            };
            // Blind while the screen is fully white (`IsBlind`).
            let blind = ps.cs_flash_alpha > 0
                && weapon_iw4::cs::flash_screen(flash, now - ps.cs_flash_start_ms)
                    .is_some_and(|(white, _)| white >= 0.99);
            Some(Viewer {
                id,
                team: meta.client_state_team,
                alive: meta.lifecycle == ClientLifecycle::Alive,
                blind,
                eye: [ps.origin[0], ps.origin[1], ps.origin[2] + ps.view_height_current],
                forward: math_iw4::angle_vectors(ps.viewangles).0,
            })
        })
        .collect();
    let mut spotted = Vec::new();
    for target in &viewers {
        // The dead keep what they were (their radar cross shows only if they were spotted).
        if !target.alive {
            continue;
        }
        let seen = matches!(target.team, TEAM_AXIS | TEAM_ALLIES)
            && viewers
                .iter()
                .any(|spotter| sees(world, &clouds, spotter, target));
        spotted.push((target.id, seen));
    }
    for (id, seen) in spotted {
        world.client_meta_mut(id).cs_spotted = seen;
    }
}

/// Whether `spotter` spots `target` (`CanPlayerSeeTargetEntityFunctor`): an enemy, alive and
/// not blind, looking within a cone that narrows with distance, with a clear line to the
/// target's eyes that smoke does not hide.
fn sees(world: &FrameWorld, clouds: &[([f32; 3], i32)], spotter: &Viewer, target: &Viewer) -> bool {
    if !spotter.alive
        || spotter.blind
        || spotter.team == target.team
        || !matches!(spotter.team, TEAM_AXIS | TEAM_ALLIES)
    {
        return false;
    }
    let path: [f32; 3] = core::array::from_fn(|i| target.eye[i] - spotter.eye[i]);
    let distance = path.iter().map(|v| v * v).sum::<f32>().sqrt();
    if distance < 1e-3 {
        return false;
    }
    let dot = path.iter().zip(spotter.forward).map(|(a, b)| a * b).sum::<f32>() / distance;
    let need = (distance / SPOT_DISTANCE)
        .powf(SPOT_POW)
        .clamp(SPOT_HALF_FOV_DEGREES.to_radians().cos(), SPOT_MAX_DOT);
    if dot < 0.0 || dot <= need {
        return false;
    }
    let trace = world.trace_world_except(spotter.eye, target.eye, SPOT_CONTENTS, None);
    trace.fraction >= 1.0 && trace.startsolid == 0 && !smoked(clouds, spotter.eye, target.eye)
}

/// Whether smoke hides the line `from`→`to` (`IsLineBlockedBySmoke`): it starts or ends in a
/// cloud, or crosses more than [`CS_SMOKE_SEE_THROUGH`] of smoke in all.
fn smoked(clouds: &[([f32; 3], i32)], from: [f32; 3], to: [f32; 3]) -> bool {
    let sight: [f32; 3] = core::array::from_fn(|i| to[i] - from[i]);
    let length = sight.iter().map(|v| v * v).sum::<f32>().sqrt();
    if length < 1e-3 {
        return false;
    }
    let dir = sight.map(|v| v / length);
    let radius_sq = CS_SMOKE_SIGHT_RADIUS * CS_SMOKE_SIGHT_RADIUS;
    let dist_sq = |a: [f32; 3], b: [f32; 3]| (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum::<f32>();
    let mut smoked = 0.0;
    for (origin, _) in clouds {
        let centre = [origin[0], origin[1], origin[2] + CS_SMOKE_SIGHT_RAISE];
        if dist_sq(centre, from) < (CS_SMOKE_SIGHT_RADIUS * 0.95).powi(2)
            || dist_sq(centre, to) < radius_sq
        {
            return true;
        }
        let along = (0..3).map(|i| (centre[i] - from[i]) * dir[i]).sum::<f32>();
        let close = if along < 0.0 {
            from
        } else if along >= length {
            to
        } else {
            core::array::from_fn(|i| from[i] + dir[i] * along)
        };
        let off_sq = dist_sq(close, centre);
        if off_sq < radius_sq {
            smoked += 2.0 * (radius_sq - off_sq).sqrt();
        }
    }
    smoked > CS_SMOKE_SEE_THROUGH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_hides_a_line_through_its_middle_but_not_one_past_it() {
        let clouds = [([0.0, 0.0, 0.0], i32::MAX)];
        assert!(smoked(&clouds, [-500.0, 0.0, 60.0], [500.0, 0.0, 60.0]));
        assert!(!smoked(&clouds, [-500.0, 400.0, 60.0], [500.0, 400.0, 60.0]));
        // Only grazing it: a short chord gets through.
        assert!(!smoked(&clouds, [-500.0, 160.0, 60.0], [500.0, 160.0, 60.0]));
        // Standing in it.
        assert!(smoked(&clouds, [0.0, 0.0, 60.0], [500.0, 900.0, 60.0]));
    }
}
