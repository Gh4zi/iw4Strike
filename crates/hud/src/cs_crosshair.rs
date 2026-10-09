//! Counter-Strike crosshair for CS rules: four bars at the screen centre, CS 1.6 green by
//! default. A dynamic crosshair's gap rests at the weapon's value, doubles in the air, halves
//! crouched, grows by half running, opens by the weapon's delta per shot (to 15) and eases back,
//! as CS 1.6's does; a static one holds still. Size, gap, thickness, colour, dot, T and outline
//! are the player's (`frame::Crosshair`, CS:GO's `cl_crosshair*`). Snipers show none: scoped, the
//! scope overlay draws.

use assets::PreparedWeapons;
use bevy::prelude::*;
use bevy::ui::Display;
use frame::ViewSubject;
use net::{LocalPresentClient, PresentedSnapshot};
use playerstate_iw4::{ENTITYNUM_NONE, PlayerState, pm_flags};
use weapon_iw4::cs::{CS_KNIFE_CROSSHAIR, CsCrosshair, CsSpread};

use crate::presentation_scale::PresentationScale;
use crate::ui_write::adopt_display;

/// Widest the shots open the gap.
const MAX_GAP: f32 = 15.0;
/// Horizontal speed above which the gap grows by half.
const RUN_SPEED: f32 = 140.0;
/// CS 1.6 shrinks the gap once per client frame; this is the rate those steps are counted at.
const SHRINK_STEPS_PER_SECOND: f32 = 100.0;
/// HUD units: CS sizes its crosshair in a 640x480 screen.
const VIRTUAL_HEIGHT: f32 = 480.0;

/// A bar (0 left, 1 right, 2 top, 3 bottom) or the centre dot (4).
#[derive(Component, Clone, Copy)]
pub(crate) struct CsCrosshairBar(u8);

const TOP: u8 = 2;
const DOT: u8 = 4;

#[derive(Resource, Default)]
pub(crate) struct CsCrosshairState {
    gap: f32,
    last_fire_ms: i32,
}

pub(crate) fn spawn_cs_crosshair(root: &mut ChildSpawnerCommands) {
    for bar in 0..=DOT {
        root.spawn((
            CsCrosshairBar(bar),
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::NONE),
            Outline::new(Val::Px(0.0), Val::ZERO, Color::NONE),
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
        // The auto snipers show theirs until the scope comes up.
        CsSpread::AutoSniper { .. } if ps.cs_zoom != 0 => None,
        _ => Some(weapon.crosshair),
    })
}

type CrosshairParts<'w, 's> = Query<
    'w,
    's,
    (
        &'static CsCrosshairBar,
        &'static mut Node,
        &'static mut BackgroundColor,
        &'static mut Outline,
    ),
>;

fn hide(parts: &mut CrosshairParts) {
    for (_, mut node, _, _) in parts.iter_mut() {
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
    settings: Res<frame::GameSettings>,
    mut state: ResMut<CsCrosshairState>,
    mut parts: CrosshairParts,
) {
    if !surface.is_ready() || view.in_killcam() {
        hide(&mut parts);
        return;
    }
    let (Some(ps), Some(weapons)) = (presented.player(local.0), weapons.as_ref()) else {
        hide(&mut parts);
        return;
    };
    let crosshair = match cs_crosshair_for(ps, weapons) {
        Some(Some(crosshair))
            if ps.pm_type < playerstate_iw4::PM_TYPE_DEAD && ps.f_weapon_pos_frac <= 0.0 =>
        {
            crosshair
        }
        _ => {
            hide(&mut parts);
            return;
        }
    };
    let player = settings.crosshair;

    let mut rest = crosshair.gap;
    if player.dynamic {
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
    if !player.dynamic {
        state.gap = rest;
    }

    let scale = PresentationScale::from_window(surface.width(), surface.height());
    let unit = scale.height() / VIRTUAL_HEIGHT;
    // A dynamic gap starts at the weapon's and the shots lengthen the bars, as CS 1.6's do; a
    // static one starts at `STATIC_GAP`. The player's gap moves either.
    let (gap, spread) = if player.dynamic {
        (state.gap + player.gap, (state.gap - rest) * 0.5)
    } else {
        (frame::Crosshair::STATIC_GAP + player.gap, 0.0)
    };
    let gap = (gap * unit).round();
    let length = ((player.size + spread) * unit).round().max(0.0);
    let thickness = (player.thickness * unit).round().max(1.0);
    let cx = (scale.width() * 0.5).round();
    let cy = (scale.height() * 0.5).round();
    let half = (thickness * 0.5).floor();
    let [r, g, b, a] = player.color;
    let color = BackgroundColor(Color::srgba_u8(r, g, b, a));
    let outline = if player.outline && player.outline_thickness > 0.0 {
        Outline::new(
            Val::Px(player.outline_thickness.round().max(1.0)),
            Val::ZERO,
            Color::srgba_u8(0, 0, 0, a),
        )
    } else {
        Outline::new(Val::Px(0.0), Val::ZERO, Color::NONE)
    };
    for (part, mut node, mut background, mut edge) in parts.iter_mut() {
        let shown = match part.0 {
            DOT => player.dot,
            TOP => !player.t_style && length >= 1.0,
            _ => length >= 1.0,
        };
        if !shown {
            adopt_display(&mut node, Display::None);
            continue;
        }
        let (left, top, width, height) = match part.0 {
            0 => (cx - gap - length, cy - half, length, thickness),
            1 => (cx + gap, cy - half, length, thickness),
            TOP => (cx - half, cy - gap - length, thickness, length),
            DOT => (cx - half, cy - half, thickness, thickness),
            _ => (cx - half, cy + gap, thickness, length),
        };
        adopt_display(&mut node, Display::Flex);
        node.left = Val::Px(left);
        node.top = Val::Px(top);
        node.width = Val::Px(width);
        node.height = Val::Px(height);
        background.set_if_neq(color);
        edge.set_if_neq(outline);
    }
}
