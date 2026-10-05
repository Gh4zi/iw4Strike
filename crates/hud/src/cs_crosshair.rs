//! Counter-Strike crosshair for CS rules: four bars at the screen centre, CS 1.6 green. The gap
//! rests at the weapon's value, doubles in the air, halves crouched, grows by half running, opens
//! by the weapon's delta per shot (to 15) and eases back, as CS 1.6's dynamic crosshair does.
//! `cl_dynamiccrosshair 0` holds it at rest. Snipers show none: scoped, the scope overlay draws.

use std::sync::atomic::{AtomicBool, Ordering};

use assets::PreparedWeapons;
use bevy::prelude::*;
use bevy::ui::Display;
use frame::ViewSubject;
use net::{LocalPresentClient, PresentedSnapshot};
use playerstate_iw4::{ENTITYNUM_NONE, PlayerState, pm_flags};
use weapon_iw4::cs::{CS_KNIFE_CROSSHAIR, CsCrosshair, CsSpread};

use crate::presentation_scale::PresentationScale;
use crate::ui_write::adopt_display;

static DYNAMIC: AtomicBool = AtomicBool::new(true);

/// Whether the gap follows movement and shots (`cl_dynamiccrosshair`).
pub fn dynamic_crosshair() -> bool {
    DYNAMIC.load(Ordering::Relaxed)
}

pub fn set_dynamic_crosshair(on: bool) {
    DYNAMIC.store(on, Ordering::Relaxed);
}

const COLOR: Color = Color::srgb(50.0 / 255.0, 250.0 / 255.0, 50.0 / 255.0);
/// Widest the shots open the gap.
const MAX_GAP: f32 = 15.0;
/// Horizontal speed above which the gap grows by half.
const RUN_SPEED: f32 = 140.0;
/// CS 1.6 shrinks the gap once per client frame; this is the rate those steps are counted at.
const SHRINK_STEPS_PER_SECOND: f32 = 100.0;
/// HUD units: CS 1.6 sizes its crosshair in a 640x480 screen.
const VIRTUAL_HEIGHT: f32 = 480.0;

#[derive(Component, Clone, Copy)]
pub(crate) struct CsCrosshairBar(u8);

#[derive(Resource, Default)]
pub(crate) struct CsCrosshairState {
    gap: f32,
    last_fire_ms: i32,
}

pub(crate) fn spawn_cs_crosshair(root: &mut ChildSpawnerCommands) {
    for bar in 0..4 {
        root.spawn((
            CsCrosshairBar(bar),
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                ..default()
            },
            BackgroundColor(COLOR),
        ));
    }
}

/// What crosshair `ps` gets: `Some` while CS rules apply and the held weapon is a CS gun or
/// nothing (the knife's crosshair); `None` leaves the MW2 reticle in charge.
pub(crate) fn cs_crosshair_for(
    ps: &PlayerState,
    weapons: &PreparedWeapons,
) -> Option<Option<CsCrosshair>> {
    movement_iw4::rules::active_for(ps)?;
    let viewmodel = weapon_iw4::get_viewmodel_weapon_index(ps);
    if viewmodel == 0 {
        return Some(Some(CS_KNIFE_CROSSHAIR));
    }
    let index = weapon_iw4::cs::cs_weapon_index_for(&weapons.0.script_name_of(viewmodel))?;
    if weapon_iw4::cs::is_knife(index) || weapon_iw4::cs::is_grenade(index) {
        return Some(Some(CS_KNIFE_CROSSHAIR));
    }
    let weapon = weapon_iw4::cs::cs_weapon(index)?;
    Some(match weapon.spread {
        CsSpread::Sniper { .. } => None,
        _ => Some(weapon.crosshair),
    })
}

fn hide(bars: &mut Query<(&CsCrosshairBar, &mut Node)>) {
    for (_, mut node) in bars.iter_mut() {
        adopt_display(&mut node, Display::None);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_cs_crosshair(
    surface: Res<crate::surface::Hud2dSurface>,
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    weapons: Option<Res<PreparedWeapons>>,
    view: Res<ViewSubject>,
    time: Res<Time>,
    mut state: ResMut<CsCrosshairState>,
    mut bars: Query<(&CsCrosshairBar, &mut Node)>,
) {
    if !surface.is_ready() || view.in_killcam() {
        hide(&mut bars);
        return;
    }
    let (Some(ps), Some(weapons)) = (presented.player(local.0), weapons.as_ref()) else {
        hide(&mut bars);
        return;
    };
    let crosshair = match cs_crosshair_for(ps, weapons) {
        Some(Some(crosshair))
            if ps.pm_type < playerstate_iw4::PM_TYPE_DEAD && ps.f_weapon_pos_frac <= 0.0 =>
        {
            crosshair
        }
        _ => {
            hide(&mut bars);
            return;
        }
    };

    let mut rest = crosshair.gap;
    if dynamic_crosshair() {
        if ps.ground_entity_num == ENTITYNUM_NONE {
            rest *= 2.0;
        } else if ps.pm_flags & pm_flags::CROUCH != 0 {
            rest *= 0.5;
        } else if ps.velocity[0].hypot(ps.velocity[1]) > RUN_SPEED {
            rest *= 1.5;
        }
        if ps.cs_last_fire_ms != state.last_fire_ms && ps.cs_last_fire_ms != 0 {
            state.gap = (state.gap + crosshair.delta).min(MAX_GAP);
        } else {
            let steps = time.delta_secs() * SHRINK_STEPS_PER_SECOND;
            state.gap -= (0.1 + 0.013 * state.gap) * steps;
        }
    }
    state.last_fire_ms = ps.cs_last_fire_ms;
    state.gap = state.gap.max(rest);
    if !dynamic_crosshair() {
        state.gap = rest;
    }

    let scale = PresentationScale::from_window(surface.width(), surface.height());
    let unit = scale.height() / VIRTUAL_HEIGHT;
    let gap = (state.gap * unit).round();
    let length = ((5.0 + (state.gap - rest) * 0.5) * unit).round().max(1.0);
    let thickness = unit.round().max(1.0);
    let cx = (scale.width() * 0.5).round();
    let cy = (scale.height() * 0.5).round();
    let half = (thickness * 0.5).floor();
    for (bar, mut node) in bars.iter_mut() {
        let (left, top, width, height) = match bar.0 {
            0 => (cx - gap - length, cy - half, length, thickness),
            1 => (cx + gap, cy - half, length, thickness),
            2 => (cx - half, cy - gap - length, thickness, length),
            _ => (cx - half, cy + gap, thickness, length),
        };
        adopt_display(&mut node, Display::Flex);
        node.left = Val::Px(left);
        node.top = Val::Px(top);
        node.width = Val::Px(width);
        node.height = Val::Px(height);
    }
}
