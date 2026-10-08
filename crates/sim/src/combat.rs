use crate::bullet::bullet_damage_at_distance;
use crate::bullet_collision::{
    BulletTraceQuery, ColliderId, EntityCollisionEpoch, EntityCollisionTraceGeom,
    HistorySampleVerdict, MASK_BULLET_WORLD, bullet_trace_segments_filtered, glass_piece_from_hit,
};
use crate::frame::FrameWorld;
use crate::identities::{DamageSource, LifeSequence, MatchRng, PelletId, ShotId};
use crate::match_state::{ClientLifecycle, EventAudience};
use crate::world::{ClientId, Tick};
use crate::world_objects::glass_piece_is_solid;
use anim_iw4::{
    ANIM_COND_RIOTSHIELDNEXT, ANIM_ET_DROPWEAPON, ANIM_ET_FIREWEAPON, ANIM_ET_KNIFE_MELEE,
    ANIM_ET_KNIFE_MELEE_CHARGE, ANIM_ET_MELEEATTACK, ANIM_ET_RAISEWEAPON, ANIM_ET_RELOAD,
};
use entity_iw4::glass_add_damage;
use movement_iw4::{Pml, is_in_air, mantle::is_weapon_inactive};
use playerstate_iw4::{ENTITYNUM_NONE, PlayerState, mantle_flags};
use std::cell::RefCell;
use std::collections::HashMap;
use weapon_iw4::{
    AIM_SPREAD_MOVE_SPEED_THRESHOLD_DEFAULT, AimSpreadMotion, AimSpreadState, CURSOR_HINT_NONE,
    FireWeaponKind, MELEE_TRACE_OFFSETS, MeleeChargeState, OFFHAND_INV_SLOTS, OffhandCmd,
    OffhandInvRow, PLAYER_MELEE_HEIGHT_DEFAULT, PLAYER_MELEE_RANGE_DEFAULT,
    PLAYER_MELEE_WIDTH_DEFAULT, SpreadOverrideState, WeaponCmd, WeaponHandState, WeaponTickEvent,
    add_aim_spread_fire, adjust_aim_spread_scale, ammo_row_present, ammo_table_key,
    begin_reload_event, clip_row_present, clip_table_key, fire_weapon_kind,
    fire_weapon_spread_degrees, get_ammo_not_in_clip, get_clip_for_hand, get_spread_for_weapon,
    melee_trace_count, melee_trace_end, num_hands_for_held, reload_insert_event,
    set_ammo_not_in_clip, set_clip_for_hand, weapon_hands,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AcceptedShot {
    pub shot_id: ShotId,
    pub attacker: ClientId,
    pub attacker_life: LifeSequence,

    pub hand: u8,
    pub weapon: u32,
    pub ammo_used: i32,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub ads_frac: f32,

    pub view_height_current: f32,

    pub aim_spread_scale: f32,

    pub perks0: u32,
    pub combat_seed: u32,
    pub owner_velocity: [f32; 3],
    pub spread_degrees: f32,
    /// CS 1.6 weapons: `FireBullets3` spread, replacing `spread_degrees`.
    pub cs_spread: Option<f32>,
    /// Fired with its silencer on (CS damage and range of the silenced gun).
    pub cs_silenced: bool,
    /// 1 for the first bullet of a burst, 2 for a later one, else 0 (CS burst damage and range).
    pub cs_burst: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emission {
    pub combat_seed: u32,
    pub shot_id: ShotId,
    pub pellet: PelletId,
    pub attacker: ClientId,
    pub attacker_life: LifeSequence,
    pub hand: u8,
    pub weapon: u32,
    pub origin: [f32; 3],
    pub direction: [f32; 3],
    pub max_range: f32,
    pub base_damage: i32,
    pub cs_silenced: bool,
    pub cs_burst: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerCollisionRepresentation {
    StandingAabbV1,
    PosedBonesV1,
}

impl PlayerCollisionRepresentation {
    pub const fn dump_label(self) -> &'static str {
        match self {
            Self::StandingAabbV1 => "aabb",
            Self::PosedBonesV1 => "bones",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityClipKind {
    CollTris,
    BoneBoxes,
    LinkedBrush,
}

impl EntityClipKind {
    pub const fn dump_label(self) -> &'static str {
        match self {
            Self::CollTris => "colltris",
            Self::BoneBoxes => "boxes",
            Self::LinkedBrush => "brush",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShotCollisionGeometry {
    Miss,
    World,
    Player {
        representation: PlayerCollisionRepresentation,
        history: HistorySampleVerdict,
    },
    Entity {
        epoch: EntityCollisionEpoch,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShotCollisionVerdict {
    pub shot_id: ShotId,
    pub pellet: PelletId,
    pub attacker: ClientId,
    pub geometry: ShotCollisionGeometry,
    pub terminal: Option<ColliderId>,
    pub startsolid: bool,
    pub bone_center: Option<[f32; 3]>,
    pub bone_half_size: Option<[f32; 3]>,
    pub xmodel_contents: Option<u32>,
    pub model_key: Option<String>,

    pub end: Option<[f32; 3]>,

    pub entity_clip: Option<EntityClipKind>,

    pub impact_n: u32,

    pub event_n: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TracePhaseOutput {
    pub shot_verdicts: Vec<ShotCollisionVerdict>,
}

pub(crate) fn advance_weapon_command(
    world: &mut FrameWorld,
    tick: Tick,
    id: ClientId,
    cmd: playerstate_iw4::UserCmd,
    msec: i32,
) -> Vec<AcceptedShot> {
    let msec = msec.clamp(1, 200);
    let frametime = msec as f32 / 1000.0;
    let mut accepted = Vec::new();
    {
        let id = &id;
        let cmd = &cmd;
        if !world
            .client_meta(*id)
            .is_some_and(|m| m.lifecycle == ClientLifecycle::Alive)
        {
            return accepted;
        }
        if let Some(mut ps) = world.player(*id).copied() {
            let parent = u32::from(cmd.weapon_mapped);
            let target = u32::from(cmd.weapon);
            if target != 0
                && parent != target
                && ps.weapons.contains(&(parent as i32))
                && world
                    .combat_facts_for(parent)
                    .is_some_and(|f| f.alternate_weapon == target)
            {
                if let Some(facts) = world.combat_facts_for(target) {
                    let ammo = weapon_iw4::ammo_table_key(facts.ammo_index, target);
                    let clip = weapon_iw4::clip_table_key(facts.clip_index, target);
                    let (initial_clip, _, initial_stock) = weapon_iw4::spawn_clip_stock(&facts, 0);
                    if !ammo_row_present(&ps.ammo, ammo) {
                        set_ammo_not_in_clip(&mut ps.ammo, ammo, initial_stock);
                    }
                    if !clip_row_present(&ps.ammoclip, clip) {
                        set_clip_for_hand(&mut ps.ammoclip, clip, 0, initial_clip);
                    }
                    world.client_meta_mut(*id).set_ammo(
                        target,
                        get_clip_for_hand(&ps.ammoclip, clip, 0),
                        get_ammo_not_in_clip(&ps.ammo, ammo),
                    );
                    *world.player_mut(*id).expect("present player") = ps;
                }
            }
        }
        let old_buttons = world
            .old_buttons_mut()
            .iter()
            .find(|(c, _)| c == id)
            .map_or(0, |(_, b)| *b);
        let old_angles = world
            .old_cmd_angles_mut()
            .iter()
            .find(|(c, _)| c == id)
            .map_or(cmd.angles, |(_, a)| *a);

        if let Some(ps) = world.player_mut(*id) {
            weapon_iw4::cs::drop_punch(&mut ps.cs_punch, frametime);
        }
        let Some(ps) = world.player(*id).copied() else {
            return accepted;
        };

        let facts_weapon = if ps.weapon != 0 {
            ps.weapon
        } else if cmd.weapon != 0 {
            u32::from(cmd.weapon)
        } else {
            return accepted;
        };
        let Some(facts) = world.combat_facts_for(facts_weapon) else {
            return accepted;
        };
        let mut fire_gate = CsGate::Pass;
        if let Some(ps) = world.player_mut(*id) {
            fire_gate = cs_weapon_frame(ps, &facts, facts_weapon, cmd, old_buttons);
        }

        {
            let mut state = AimSpreadState {
                aim_spread_scale: ps.aim_spread_scale,
                spread_override: ps.spread_override,
                spread_override_state: ps.spread_override_state,
            };
            let motion = AimSpreadMotion {
                frametime,
                cmd_angles: cmd.angles,
                old_angles,
                forwardmove: cmd.forwardmove,
                rightmove: cmd.rightmove,
                velocity_xy: [ps.velocity[0], ps.velocity[1]],
                speed: ps.speed,
                move_speed_threshold: AIM_SPREAD_MOVE_SPEED_THRESHOLD_DEFAULT,
            };
            adjust_aim_spread_scale(
                &mut state,
                &facts.spread_facts(),
                &facts.aim_spread_decay_facts(),
                ps.ground_entity_num,
                ps.pm_type,
                ps.e_flags,
                ps.f_weapon_pos_frac,
                &motion,
            );
            if let Some(ps_mut) = world.player_mut(*id) {
                ps_mut.aim_spread_scale = state.aim_spread_scale;
                ps_mut.spread_override_state = state.spread_override_state;
            }
        }
        let Some(ps) = world.player(*id).copied() else {
            return accepted;
        };
        let meta = world.client_meta(*id).cloned().unwrap_or_default();
        let started_weapon = ps.weapon;
        let ammo_weapon = if started_weapon != 0 {
            started_weapon
        } else {
            facts_weapon
        };
        let last_hand = num_hands_for_held(&ps.weapons, &ps.weapon_data, ammo_weapon);
        let (meta_clip, meta_stock) = meta.ammo_for(ammo_weapon);
        let ammo_index = weapon_iw4::ammo_table_key(facts.ammo_index, ammo_weapon);
        let clip_index = weapon_iw4::clip_table_key(facts.clip_index, ammo_weapon);

        let stock = if ammo_row_present(&ps.ammo, ammo_index) {
            get_ammo_not_in_clip(&ps.ammo, ammo_index)
        } else {
            meta_stock
        };
        let clip0 = if clip_row_present(&ps.ammoclip, clip_index) {
            get_clip_for_hand(&ps.ammoclip, clip_index, 0)
        } else {
            meta_clip
        };
        let clip1 = if clip_row_present(&ps.ammoclip, clip_index) {
            get_clip_for_hand(&ps.ammoclip, clip_index, 1)
        } else {
            0
        };

        let quick_reload = facts.dual_mag.is_some() && meta.quick_reload_ready(ammo_weapon);
        let mut hands = [
            WeaponHandState {
                weapon: started_weapon,
                weaponstate: ps.weaponstate_primary,
                weapon_time: ps.weapon_time,
                weapon_delay: ps.weapon_delay,
                weap_anim: ps.weap_anim,
                hand_index: 0,
                clip: clip0,
                stock,
                shot_count: meta.weapon_shot_count,
                burst_latch: meta.burst_latch,
                rechamber_pending: meta.rechamber_pending,
                delayed_rechamber: false,
                weapon_restrict_kick_time: ps.weapon_restrict_kick_time,
                quick_reload,
            },
            WeaponHandState {
                weapon: started_weapon,
                weaponstate: ps.weaponstate_secondary,
                weapon_time: ps.weapon_time_secondary,
                weapon_delay: ps.weapon_delay_secondary,
                weap_anim: ps.weap_anim_secondary,
                hand_index: 1,
                clip: if last_hand >= 1 { clip1 } else { 0 },
                stock,
                shot_count: ps.weapon_shot_count_secondary as u8,
                burst_latch: meta.burst_latch_secondary,
                rechamber_pending: meta.rechamber_pending_secondary,
                delayed_rechamber: false,
                weapon_restrict_kick_time: ps.weapon_restrict_kick_time_secondary,
                quick_reload,
            },
        ];
        let locked_fire = world.missile_launch_facts(ps.weapon).is_none_or(|f| {
            !f.require_lock_to_fire || meta.weapon_lock.can_fire(ps.weapon, meta.life_sequence.0)
        });
        let fire_buttons = if locked_fire {
            cmd.buttons
        } else {
            cmd.buttons & !playerstate_iw4::buttons::ATTACK
        };
        let knife_held = weapon_iw4::cs::is_knife(facts.cs_weapon);
        // CS guns have no melee of their own (the knife is slot 3).
        let grenade_held = weapon_iw4::cs::is_grenade(facts.cs_weapon);
        // The C4 never fires: its attack plants it, which the bomb mode's script reads.
        let c4_held = weapon_iw4::cs::is_c4(facts.cs_weapon);
        // Screwing a silencer on or off: no shooting or reloading until it is done.
        let adjusting = facts.cs_weapon != 0
            && ps.cs_gun_weapon == ps.weapon
            && cmd.server_time < ps.cs_adjust_ms;
        let taken = if knife_held || grenade_held || c4_held {
            CS_KNIFE_BUTTONS
        } else if adjusting {
            playerstate_iw4::buttons::MELEE_CHARGE
                | playerstate_iw4::buttons::ATTACK
                | playerstate_iw4::buttons::RELOAD
        } else if facts.cs_weapon != 0 {
            playerstate_iw4::buttons::MELEE_CHARGE
        } else {
            0
        };
        let (fire_buttons, old_buttons) = (fire_buttons & !taken, old_buttons & !taken);
        // A gun whose shots the CS layer times (burst, scope-dependent cycle) only sees the
        // fire button when it is time, as a fresh press.
        let attack = playerstate_iw4::buttons::ATTACK;
        let (fire_buttons, old_buttons) = match fire_gate {
            CsGate::Pass => (fire_buttons, old_buttons),
            CsGate::Block => (fire_buttons & !attack, old_buttons),
            CsGate::Press => (fire_buttons | attack, old_buttons & !attack),
        };
        let selected_airdrop_marker =
            world.weapon_script_name(ps.weapon) == crate::equipment::AIRDROP_MARKER_WEAPON;
        let marker_offhand_class = i32::MAX;
        let mut wcmd = WeaponCmd {
            msec,
            server_time: cmd.server_time,
            stun_time: ps.stun_time,
            buttons: fire_buttons
                | if selected_airdrop_marker && cmd.buttons & playerstate_iw4::buttons::ATTACK != 0
                {
                    playerstate_iw4::buttons::FRAG
                } else {
                    0
                },
            old_buttons: old_buttons
                | if selected_airdrop_marker && old_buttons & playerstate_iw4::buttons::ATTACK != 0
                {
                    playerstate_iw4::buttons::FRAG
                } else {
                    0
                },

            cmd_weapon: if cmd.weapon != 0 {
                cmd.weapon
            } else {
                ps.weapon as u16
            },
            pm_flags: ps.pm_flags,
            weap_flags: ps.weap_flags,
            pm_type: ps.pm_type,
            e_flags: ps.e_flags,
            last_weapon_hand: last_hand,
            f_weapon_pos_frac: ps.f_weapon_pos_frac,
            melee_charge_yaw: cmd.melee_charge_yaw,
            melee_charge_dist: cmd.melee_charge_dist,
            player_melee_range: PLAYER_MELEE_RANGE_DEFAULT,

            is_in_air: {
                let pml = Pml {
                    forward: [0.0; 3],
                    right: [0.0; 3],
                    up: [0.0; 3],
                    frametime,
                    msec,
                    walking: 0,
                    ground_plane: u32::from(ps.ground_entity_num != ENTITYNUM_NONE),
                    almost_ground_plane: 0,
                    ground_trace: [0; 11],
                    previous_origin: [0.0; 3],
                    previous_velocity: [0.0; 3],
                    holdrand: 0,
                    jump_animations: [None; 4],
                    mantle_movetype: None,
                    landing_animation: false,
                };
                is_in_air(&ps, &pml)
            },
            melee_charge: MeleeChargeState {
                pm_flags: ps.pm_flags,
                pm_type: ps.pm_type,
                e_flags: ps.e_flags,
                melee_charge_yaw: ps.melee_charge_yaw,
                melee_charge_dist: ps.melee_charge_dist,
                melee_charge_time: ps.melee_charge_time,
            },
            melee_started: None,

            mantle_weapon_inactive: is_weapon_inactive(&ps, true),
            ladder_keeps_weapon: movement_iw4::rules::CS_RULES,
            mantle_quick_raise: (ps.mantle_flags & mantle_flags::QUICK) != 0,
            cmd_weapon_owned: {
                let w = if cmd.weapon != 0 {
                    u32::from(cmd.weapon)
                } else {
                    ps.weapon
                };
                w == 0
                    || ps.weapons.contains(&(w as i32))
                    || (ps.weapons.contains(&(i32::from(cmd.weapon_mapped)))
                        && world
                            .combat_facts_for(u32::from(cmd.weapon_mapped))
                            .is_some_and(|f| f.alternate_weapon == w))
            },

            cmd_weapon_pistol_quick: world
                .combat_facts_for(u32::from(cmd.weapon))
                .is_some_and(|f| f.weap_class == 5),
            alternate_switch: cmd.weapon != 0
                && (facts.alternate_weapon == u32::from(cmd.weapon)
                    || (facts.inventory_type == 3 && ps.weapon_primary == u32::from(cmd.weapon))),
            switch_alternate_raise_time_ms: world
                .combat_facts_for(u32::from(cmd.weapon))
                .map_or(0, |f| f.alternate_raise_time_ms),
            switch_raise_time_ms: {
                let w = if cmd.weapon != 0 {
                    u32::from(cmd.weapon)
                } else {
                    ps.weapon
                };
                world
                    .combat_facts_for(w)
                    .map(|f| f.raise_time_ms)
                    .unwrap_or(0)
            },
            switch_quick_raise_time_ms: {
                let w = if cmd.weapon != 0 {
                    u32::from(cmd.weapon)
                } else {
                    ps.weapon
                };
                world
                    .combat_facts_for(w)
                    .map(|f| f.quick_raise_time_ms)
                    .unwrap_or(0)
            },
            perks0: ps.perks[0],
            perk_weap_reload_multiplier: weapon_iw4::PERK_WEAP_RELOAD_MULTIPLIER_DEFAULT,
            offhand: {
                let mut inventory = [OffhandInvRow::default(); OFFHAND_INV_SLOTS];
                for (i, &slot) in ps.weapons.iter().enumerate() {
                    if slot <= 0 {
                        continue;
                    }
                    let weapon = slot as u32;
                    let (clip, stock) = meta.ammo_for(weapon);
                    let eq = world.equipment_facts_for(weapon);
                    let combat = world.combat_facts_for(weapon);
                    inventory[i] = OffhandInvRow {
                        weapon,
                        offhand_class: if selected_airdrop_marker && weapon == ps.weapon {
                            marker_offhand_class
                        } else {
                            eq.map(|e| e.offhand_class).unwrap_or(0)
                        },
                        ammo: clip + stock,
                        hold_fire_time_ms: eq.map(|e| e.hold_fire_time_ms).unwrap_or(0),
                        fire_time_ms: combat.map(|f| f.fire_time_ms).unwrap_or(0),
                        fire_delay_ms: combat.map(|f| f.fire_delay_ms).unwrap_or(0),
                        fuse_time_ms: eq.map(|e| e.fuse_time_ms).unwrap_or(0),
                        cook_off_hold: eq.map(|e| e.cook_off_hold).unwrap_or(false),
                        offhand_hold_is_cancelable: combat
                            .and_then(|f| f.offhand_hold_is_cancelable),
                        weap_type: combat.map(|f| f.weap_type).unwrap_or(0),
                        has_detonator: eq.is_some_and(|e| e.has_detonator),
                        detonate_delay_ms: eq.map_or(0, |e| e.detonate_delay_ms),
                        detonate_time_ms: eq.map_or(0, |e| e.detonate_time_ms),
                    };
                }
                OffhandCmd {
                    inventory,
                    offhand_primary: if selected_airdrop_marker {
                        marker_offhand_class
                    } else {
                        ps.offhand_primary
                    },
                    offhand_secondary: ps.offhand_secondary,
                    cmd_off_hand_index: cmd.off_hand_index,
                    cmd_off_hand_owned: cmd.off_hand_index != 0
                        && ps
                            .weapons
                            .iter()
                            .any(|&slot| slot == i32::from(cmd.off_hand_index)),
                    cursor_hint_ent: CURSOR_HINT_NONE,
                    held_quick_drop_time_ms: facts.quick_drop_time_ms,
                    off_hand_index: ps.off_hand_index,
                    grenade_time_left: ps.grenade_time_left,
                }
            },
        };
        let clip_before = hands[0].clip;
        let events = weapon_hands(&mut hands, &facts, &mut wcmd, last_hand);
        let hand0 = hands[0];
        for &(hand, event) in events.iter().flatten() {
            if hand != 0 {
                continue;
            }
            match event {
                WeaponTickEvent::PutawayStarted
                    if ps.pm_flags & playerstate_iw4::pm_flags::MANTLE == 0 =>
                {
                    apply_player_anim_event_with_target(
                        world,
                        *id,
                        ANIM_ET_DROPWEAPON,
                        Some(u32::from(wcmd.cmd_weapon)),
                    );
                }
                WeaponTickEvent::RaiseStarted
                    if started_weapon != 0
                        && hand0.weaponstate
                            != weapon_iw4::WeaponState::RaisingAltswitch as i32 =>
                {
                    apply_player_anim_event(world, *id, ANIM_ET_RAISEWEAPON);
                }
                _ => {}
            }
        }
        if let Some(charge) = wcmd.melee_started {
            if let Some(ps_mut) = world.player_mut(*id) {
                movement_iw4::add_predictable_event(
                    ps_mut,
                    entity_iw4::EntityEventKind::MELEE_SWIPE.0,
                    0,
                );
            }
            let anim = match (facts.knife_model != 0, charge) {
                (true, true) => ANIM_ET_KNIFE_MELEE_CHARGE,
                (true, false) => ANIM_ET_KNIFE_MELEE,
                (false, _) => ANIM_ET_MELEEATTACK,
            };
            apply_player_anim_event(world, *id, anim);
        }

        if let Some(ps_mut) = world.player_mut(*id) {
            ps_mut.weapon = hand0.weapon;
            if hand0.weapon != started_weapon {
                if hand0.weaponstate == weapon_iw4::WeaponState::RaisingAltswitch as i32 {
                    ps_mut.aim_spread_scale = ps_mut.aim_spread_scale.max(128.0);
                }
                ps_mut.weapon_primary = if hand0.weapon == u32::from(cmd.weapon) {
                    u32::from(cmd.weapon_mapped)
                } else {
                    0
                };
            }
            ps_mut.weaponstate_primary = hand0.weaponstate;
            ps_mut.weapon_time = hand0.weapon_time;
            ps_mut.weapon_delay = hand0.weapon_delay;
            ps_mut.weap_anim = hand0.weap_anim;
            ps_mut.weapon_restrict_kick_time = hand0.weapon_restrict_kick_time;
            ps_mut.last_weapon_hand = last_hand;
            ps_mut.pm_flags = wcmd.pm_flags;
            ps_mut.weap_flags = wcmd.weap_flags;
            ps_mut.off_hand_index = wcmd.offhand.off_hand_index;
            ps_mut.grenade_time_left = wcmd.offhand.grenade_time_left;
            ps_mut.melee_charge_yaw = wcmd.melee_charge.melee_charge_yaw;
            ps_mut.melee_charge_dist = wcmd.melee_charge.melee_charge_dist;
            ps_mut.melee_charge_time = wcmd.melee_charge.melee_charge_time;
            if last_hand >= 1 {
                let h1 = hands[1];
                ps_mut.weaponstate_secondary = h1.weaponstate;
                ps_mut.weapon_time_secondary = h1.weapon_time;
                ps_mut.weapon_delay_secondary = h1.weapon_delay;
                ps_mut.weap_anim_secondary = h1.weap_anim;
                ps_mut.weapon_shot_count_secondary = i32::from(h1.shot_count);
                ps_mut.weapon_restrict_kick_time_secondary = h1.weapon_restrict_kick_time;
            }
        }
        let life = world
            .client_meta(*id)
            .map(|m| m.life_sequence)
            .unwrap_or_default();
        let meta = world.client_meta_mut(*id);
        if hand0.weapon != started_weapon {
            if started_weapon != 0 {
                meta.set_ammo(started_weapon, hand0.clip, hand0.stock);
            }
        } else if hand0.weapon != 0 {
            meta.set_ammo(hand0.weapon, hand0.clip, hand0.stock);
        }
        if hand0.weapon != 0 {
            meta.mirror_held_ammo(hand0.weapon);
        }
        meta.weapon_shot_count = hand0.shot_count;
        meta.burst_latch = hand0.burst_latch;
        meta.rechamber_pending = hand0.rechamber_pending;
        meta.burst_latch_secondary = last_hand >= 1 && hands[1].burst_latch;
        meta.rechamber_pending_secondary = last_hand >= 1 && hands[1].rechamber_pending;
        if facts.dual_mag.is_some() && started_weapon != 0 {
            meta.set_quick_reload_ready(started_weapon, hand0.quick_reload);
        }

        if ammo_index != 0 || clip_index != 0 {
            if let Some(ps_mut) = world.player_mut(*id) {
                if ammo_index != 0 {
                    let _ = set_ammo_not_in_clip(&mut ps_mut.ammo, ammo_index, hands[0].stock);
                }
                if clip_index != 0 {
                    let _ = set_clip_for_hand(&mut ps_mut.ammoclip, clip_index, 0, hands[0].clip);
                    if last_hand >= 1 {
                        let _ =
                            set_clip_for_hand(&mut ps_mut.ammoclip, clip_index, 1, hands[1].clip);
                    }
                }
            }
        }

        // CS knife and grenades act after the MW2 machine's ammo is written back, so their own
        // counts (a thrown grenade) stick.
        if knife_held
            && hand0.weapon == facts_weapon
            && hand0.weaponstate == weapon_iw4::WeaponState::Ready as i32
        {
            cs_knife_frame(world, tick, *id, facts_weapon, cmd);
        }
        if grenade_held
            && hand0.weapon == facts_weapon
            && hand0.weaponstate == weapon_iw4::WeaponState::Ready as i32
        {
            cs_grenade_frame(world, tick, *id, facts_weapon, cmd);
        } else if world.player(*id).is_some_and(|ps| ps.cs_grenade != 0)
            && (!grenade_held || hand0.weapon != facts_weapon)
        {
            // Switching away drops a pulled pin back in (CS `Holster`).
            if let Some(ps) = world.player_mut(*id) {
                ps.cs_grenade = 0;
            }
        }
        if hands[0].delayed_rechamber {
            if let Some(ps) = world.player_mut(*id) {
                movement_iw4::add_predictable_event(
                    ps,
                    entity_iw4::EntityEventKind::RECHAMBER_WEAPON.0,
                    0,
                );
            }
        }
        if hands[0].clip > clip_before {
            if let Some(ps) = world.player_mut(*id) {
                movement_iw4::add_predictable_event(
                    ps,
                    entity_iw4::EntityEventKind::RELOAD_ADDAMMO.0,
                    0,
                );
            }
        }

        for slot in events.into_iter().flatten() {
            let (_hand_i, ev) = slot;
            match ev {
                WeaponTickEvent::Detonated { weapon } => {
                    world
                        .weapon_notes
                        .push(crate::equipment::WeaponNote::DetonationRequested {
                            owner: *id,
                            weapon,
                        });
                    if let Some(ps) = world.player_mut(*id) {
                        movement_iw4::add_predictable_event(
                            ps,
                            entity_iw4::EntityEventKind::DETONATE.0,
                            weapon as i32,
                        );
                    }
                }
                WeaponTickEvent::ReloadAmmoAdded { shells: _ } => {}
                WeaponTickEvent::RechamberWeapon => {
                    if !hands[_hand_i as usize].delayed_rechamber {
                        if let Some(ps) = world.player_mut(*id) {
                            movement_iw4::add_predictable_event(
                                ps,
                                entity_iw4::EntityEventKind::RECHAMBER_WEAPON.0,
                                0,
                            );
                        }
                    }
                }
                WeaponTickEvent::EjectBrass => {
                    let kind = if _hand_i == 1 {
                        entity_iw4::EntityEventKind::EJECT_BRASS_LEFT
                    } else {
                        entity_iw4::EntityEventKind::EJECT_BRASS
                    };
                    if let Some(ps) = world.player_mut(*id) {
                        movement_iw4::add_predictable_event(ps, kind.0, 0);
                    }
                }
                WeaponTickEvent::ShotAccepted { ammo_used } => {
                    let hand = &hands[_hand_i as usize];
                    let shot_id = world.alloc_shot_id();
                    let weapon = hand.weapon;
                    let Some(ps) = world.player(*id).copied() else {
                        continue;
                    };
                    apply_player_anim_event(world, *id, ANIM_ET_FIREWEAPON);
                    let combat_seed = world.combat_rng_mut().next_u32();
                    let origin = world
                        .client_meta(*id)
                        .and_then(|meta| meta.linked_weapon_view)
                        .map_or(
                            [
                                ps.origin[0],
                                ps.origin[1],
                                ps.origin[2] + ps.view_height_current,
                            ],
                            |view| view.origin,
                        );

                    let cs_silenced = ps.cs_silencers
                        & weapon_iw4::cs::silencer_bit(facts.cs_weapon)
                        != 0;
                    // The burst bullet this command's gate let through (0 when not in a burst).
                    let cs_burst = ps.cs_burst_shot.min(2) as u8;
                    let cs_gun = weapon_iw4::cs::cs_weapon(facts.cs_weapon)
                        .map(|cs| cs.with_silencer(cs_silenced).with_burst(cs_burst));
                    let cs = cs_gun.as_ref();
                    let mut shot_angles = ps.viewangles;
                    if cs.is_some() {
                        // CS fires along the view plus the recoil punch; MW2 gun sway is ignored.
                        for (angle, punch) in shot_angles.iter_mut().zip(ps.cs_punch) {
                            *angle += punch;
                        }
                    } else {
                        for (angle, offset) in shot_angles.iter_mut().zip(cmd.gun_angle_offset) {
                            if offset.is_finite() {
                                *angle += offset.clamp(-45.0, 45.0);
                            }
                        }
                    }
                    let last_shot = hand.clip == 0 && facts.fire_type != 5;
                    world.push_entity_event(
                        tick,
                        EventAudience::All,
                        entity_iw4::predicted_weapon_fire_event(_hand_i as i32, last_shot),
                        crate::EntityEventPayload {
                            number: id.0 as i32,
                            weapon,
                            correlation: shot_id.0,
                            origin,
                            direction: shot_angles,
                            simulation_flags: (if cs_silenced {
                                weapon_iw4::cs::SILENCED_SHOT_FLAG
                            } else {
                                0
                            }) | (if cs_burst != 0 {
                                weapon_iw4::cs::BURST_SHOT_FLAG
                            } else {
                                0
                            }),
                            ..Default::default()
                        },
                    );
                    world
                        .weapon_notes
                        .push(crate::equipment::WeaponNote::Fired { owner: *id });

                    if let Some(ps_mut) = world.player_mut(*id) {
                        add_aim_spread_fire(
                            &mut ps_mut.aim_spread_scale,
                            ps.f_weapon_pos_frac,
                            facts.hip_spread_fire_add,
                        );
                    }
                    let aim_spread_scale = world
                        .player(*id)
                        .map(|p| p.aim_spread_scale)
                        .unwrap_or(ps.aim_spread_scale);
                    let ads_frac = ps.f_weapon_pos_frac.clamp(0.0, 1.0);
                    let override_state = SpreadOverrideState::from_i32(ps.spread_override_state);
                    let cone = get_spread_for_weapon(
                        ps.view_height_current,
                        ps.spread_override,
                        override_state,
                        &facts.spread_facts(),
                        weapon_iw4::perk_weap_spread_multiplier(ps.perks[0]),
                    );
                    let spread_degrees = fire_weapon_spread_degrees(
                        cone,
                        facts.ads_spread,
                        ads_frac,
                        aim_spread_scale,
                    );
                    let cs_spread = cs
                        .zip(world.player_mut(*id))
                        .map(|(cs, ps_mut)| {
                            cs_weapon_fire(cs, ps_mut, *id, cmd.server_time, cs_burst, facts.cs_weapon)
                        });
                    accepted.push(AcceptedShot {
                        shot_id,
                        attacker: *id,
                        attacker_life: life,
                        hand: _hand_i,
                        weapon,
                        ammo_used,
                        origin,
                        angles: shot_angles,
                        ads_frac,
                        view_height_current: ps.view_height_current,
                        aim_spread_scale,
                        perks0: ps.perks[0],
                        combat_seed,
                        owner_velocity: ps.velocity,
                        spread_degrees,
                        cs_spread,
                        cs_silenced,
                        cs_burst,
                    });
                }
                WeaponTickEvent::OffhandPrepare { weapon } => {
                    world
                        .weapon_notes
                        .push(crate::equipment::WeaponNote::Pullback { owner: *id, weapon });
                    if let Some(ps) = world.player_mut(*id) {
                        movement_iw4::add_predictable_event(
                            ps,
                            entity_iw4::EntityEventKind::PREP_OFFHAND.0,
                            weapon as i32,
                        );
                    }
                }
                WeaponTickEvent::OffhandUsed {
                    weapon,
                    remaining_fuse_ms,
                } => {
                    let combat = world.weapon_combat_row(weapon);
                    let meta = world.client_meta_mut(*id);
                    let (clip, stock) = meta.ammo_for(weapon);
                    if clip + stock > 0 {
                        if clip > 0 {
                            meta.set_ammo(weapon, clip - 1, stock);
                        } else {
                            meta.set_ammo(weapon, 0, stock - 1);
                        }
                    }
                    if let Some(facts) = combat {
                        if let Some(ps) = world.player_mut(*id) {
                            spend_ps_offhand_round(ps, weapon, facts);
                        }
                    }
                    if crate::equipment::spawn_offhand_projectile(
                        world,
                        *id,
                        weapon,
                        tick,
                        remaining_fuse_ms,
                    ) {
                        if let Some(ps) = world.player(*id).copied() {
                            let origin = [
                                ps.origin[0],
                                ps.origin[1],
                                ps.origin[2] + ps.view_height_current,
                            ];
                            world.push_entity_event(
                                tick,
                                EventAudience::All,
                                entity_iw4::EntityEventKind::USE_OFFHAND,
                                crate::EntityEventPayload {
                                    number: id.0 as i32,
                                    weapon,
                                    origin,
                                    event_parm: weapon as i32,
                                    ..Default::default()
                                },
                            );
                        }
                    }
                }
                WeaponTickEvent::OffhandCookedOff { weapon } => {
                    let combat = world.weapon_combat_row(weapon);
                    let meta = world.client_meta_mut(*id);
                    let (clip, stock) = meta.ammo_for(weapon);
                    if clip + stock > 0 {
                        if clip > 0 {
                            meta.set_ammo(weapon, clip - 1, stock);
                        } else {
                            meta.set_ammo(weapon, 0, stock - 1);
                        }
                    }
                    if let Some(facts) = combat {
                        if let Some(ps) = world.player_mut(*id) {
                            spend_ps_offhand_round(ps, weapon, facts);
                        }
                    }
                    crate::equipment::explode_offhand_in_hand(world, *id, weapon, tick);
                }
                WeaponTickEvent::ReloadStarted => {
                    apply_player_anim_event(world, *id, ANIM_ET_RELOAD);
                    let hand = &hands[_hand_i as usize];
                    let event = begin_reload_event(&facts, hand.clip);
                    if let Some(ps) = world.player_mut(*id) {
                        movement_iw4::add_predictable_event(
                            ps,
                            entity_iw4::EntityEventKind::RESET_ADS.0,
                            0,
                        );
                        movement_iw4::add_predictable_event(ps, event, 0);
                    }
                    world
                        .weapon_notes
                        .push(crate::equipment::WeaponNote::ReloadStarted { owner: *id });
                }
                WeaponTickEvent::ReloadInsert => {
                    let hand = &hands[_hand_i as usize];
                    let event = reload_insert_event(&facts, hand.clip);
                    if let Some(ps) = world.player_mut(*id) {
                        movement_iw4::add_predictable_event(ps, event, 0);
                    }
                }
                WeaponTickEvent::ReloadEnded => {
                    if let Some(ps) = world.player_mut(*id) {
                        movement_iw4::add_predictable_event(
                            ps,
                            entity_iw4::EntityEventKind::RELOAD_END.0,
                            0,
                        );
                    }
                }
                WeaponTickEvent::EmptyClick => {
                    if let Some(ps) = world.player_mut(*id) {
                        movement_iw4::add_predictable_event(
                            ps,
                            entity_iw4::EntityEventKind::NOAMMO.0,
                            0,
                        );
                    }
                }
                WeaponTickEvent::AlternateStarted => {
                    if let Some(ps) = world.player_mut(*id) {
                        movement_iw4::add_predictable_event(
                            ps,
                            entity_iw4::EntityEventKind::WEAPON_ALT.0,
                            i32::from(cmd.weapon),
                        );
                    }
                }
                WeaponTickEvent::PutawayStarted => {
                    if let Some(ps) = world.player_mut(*id) {
                        movement_iw4::add_predictable_event(
                            ps,
                            entity_iw4::EntityEventKind::PUTAWAY_WEAPON.0,
                            0,
                        );
                    }
                }
                WeaponTickEvent::RaiseStarted => {
                    if let Some(ps) = world.player_mut(*id) {
                        movement_iw4::add_predictable_event(
                            ps,
                            entity_iw4::EntityEventKind::RAISE_WEAPON.0,
                            0,
                        );
                    }
                }
                WeaponTickEvent::MeleeFired => {
                    let hand = &hands[_hand_i as usize];
                    let weapon = hand.weapon;
                    let Some(ps) = world.player(*id).copied() else {
                        continue;
                    };
                    let origin = [
                        ps.origin[0],
                        ps.origin[1],
                        ps.origin[2] + ps.view_height_current,
                    ];
                    world.push_entity_event(
                        tick,
                        EventAudience::All,
                        entity_iw4::EntityEventKind::FIRE_MELEE,
                        crate::EntityEventPayload {
                            number: id.0 as i32,
                            weapon,
                            origin,
                            direction: ps.viewangles,
                            ..Default::default()
                        },
                    );
                    if world.publishes_snapshot() {
                        fire_weapon_melee(world, tick, *id, life, weapon, origin, ps.viewangles);
                    }
                }
                WeaponTickEvent::StunnedStarted => {
                    apply_player_anim_event(world, *id, 21);
                }
                WeaponTickEvent::RaiseFinished | WeaponTickEvent::DropFinished => {}
            }
        }
    }
    accepted
}

pub(crate) fn apply_player_anim_event(world: &mut FrameWorld, id: ClientId, event: u8) {
    apply_player_anim_event_with_target(world, id, event, None);
}

fn apply_player_anim_event_with_target(
    world: &mut FrameWorld,
    id: ClientId,
    event: u8,
    next_weapon: Option<u32>,
) {
    apply_player_anim_event_inner(
        world,
        id,
        event,
        next_weapon,
        !matches!(event, ANIM_ET_RAISEWEAPON | 11..=16),
    );
}

pub(crate) fn apply_player_anim_event_forced(
    world: &mut FrameWorld,
    id: ClientId,
    event: u8,
    force: bool,
) {
    apply_player_anim_event_inner(world, id, event, None, force);
}

fn apply_player_anim_event_inner(
    world: &mut FrameWorld,
    id: ClientId,
    event: u8,
    next_weapon: Option<u32>,
    force: bool,
) {
    let Some(script) = world.player_anim_script() else {
        return;
    };
    let mut seed = world.anim_event_seed();
    let Some(ps) = world.player(id).copied() else {
        return;
    };
    let (view_w, primary) = crate::pmove_anim_weapon_ids(&ps);
    let view_facts = world.combat_facts_for(view_w);
    let primary_facts = world.combat_facts_for(primary);
    let movetype =
        crate::player_anim_script::event_anim_movetype(&ps, world.last_anim_movetype(id));
    let strafing = world.last_anim_strafing(id);
    let mut conds = crate::anim_conditions_from_pmove(
        &ps,
        view_facts,
        primary_facts,
        Some(movetype),
        strafing,
        world.anim_command_buttons(id),
    );
    if let Some(next_weapon) = next_weapon {
        conds.set_value(
            ANIM_COND_RIOTSHIELDNEXT,
            u32::from(
                world
                    .combat_facts_for(next_weapon)
                    .is_some_and(|facts| facts.player_anim_type == 15),
            ),
        );
    }
    {
        let Some(ps) = world.player_mut(id) else {
            return;
        };
        script.apply_event(ps, event, &conds, &mut seed, force);
    }
    world.set_anim_event_seed(seed);
}

fn cs_gun_state(ps: &PlayerState) -> weapon_iw4::cs::CsGunState {
    weapon_iw4::cs::CsGunState {
        shots_fired: ps.cs_shots_fired,
        accuracy: ps.cs_accuracy,
        last_fire_ms: ps.cs_last_fire_ms,
        direction: ps.cs_recoil_dir,
    }
}

fn store_cs_gun_state(ps: &mut PlayerState, state: weapon_iw4::cs::CsGunState) {
    ps.cs_shots_fired = state.shots_fired;
    ps.cs_accuracy = state.accuracy;
    ps.cs_last_fire_ms = state.last_fire_ms;
    ps.cs_recoil_dir = state.direction;
}

fn cs_shooter(ps: &PlayerState) -> weapon_iw4::cs::CsShooter {
    weapon_iw4::cs::CsShooter {
        on_ground: ps.ground_entity_num != ENTITYNUM_NONE,
        ducked: ps.pm_flags & playerstate_iw4::pm_flags::CROUCH != 0,
        speed: ps.velocity[0].hypot(ps.velocity[1]),
        zoomed: ps.cs_zoom != 0 || ps.f_weapon_pos_frac >= 1.0,
    }
}

/// CS 1.6 weapon state between shots, once per command: a weapon switch or reload starts a
/// fresh spray and releasing the trigger lets the spray count fall.
fn cs_weapon_frame(
    ps: &mut PlayerState,
    facts: &weapon_iw4::WeaponCombatFacts,
    weapon: u32,
    cmd: &playerstate_iw4::UserCmd,
    old_buttons: u32,
) -> CsGate {
    ps.cs_burst_shot = 0;
    let Some(cs) = weapon_iw4::cs::cs_weapon(facts.cs_weapon) else {
        // Only a scoped CS gun stays zoomed.
        ps.cs_zoom = 0;
        ps.cs_last_zoom = 0;
        ps.cs_burst_left = 0;
        return CsGate::Pass;
    };
    let reloading = weapon_iw4::WeaponState::from_i32(ps.weaponstate_primary)
        .is_ok_and(weapon_iw4::WeaponState::is_reload_family);
    if reloading {
        // `Reload`: a fresh magazine starts a fresh spray, unzoomed.
        ps.cs_shots_fired = 0;
        ps.cs_accuracy = weapon_iw4::cs::initial_accuracy(cs);
        ps.cs_delay_fire = 0;
        ps.cs_zoom = 0;
        ps.cs_last_zoom = 0;
        ps.cs_burst_left = 0;
    }
    if ps.cs_gun_weapon != weapon {
        ps.cs_burst_left = 0;
        ps.cs_fire_gate_ms = 0;
        if cs.dual {
            // `Deploy`: the left gun fires first.
            ps.cs_burst_modes |= weapon_iw4::cs::silencer_bit(facts.cs_weapon);
        }
        ps.cs_gun_weapon = weapon;
        ps.cs_shots_fired = 0;
        ps.cs_accuracy = weapon_iw4::cs::initial_accuracy(cs);
        ps.cs_last_fire_ms = 0;
        ps.cs_decrease_shots_ms = 0;
        ps.cs_delay_fire = 0;
        // `DefaultDeploy`: drawn unzoomed; the scope waits a second.
        ps.cs_zoom = 0;
        ps.cs_last_zoom = 0;
        ps.cs_next_attack2_ms = cmd.server_time + weapon_iw4::cs::CS_DEPLOY_ZOOM_DELAY_MS;
    }
    cs_zoom_frame(cs, ps, cmd, reloading);
    cs_silencer_frame(cs, facts.cs_weapon, ps, cmd, reloading);
    let gate = cs_fire_gate(cs, facts.cs_weapon, ps, cmd, old_buttons, reloading);
    let mut state = cs_gun_state(ps);
    let mut delay_fire = ps.cs_delay_fire != 0;
    weapon_iw4::cs::post_frame(
        cs,
        &mut state,
        cmd.buttons & playerstate_iw4::buttons::ATTACK != 0,
        cmd.server_time,
        &mut ps.cs_decrease_shots_ms,
        &mut delay_fire,
    );
    ps.cs_delay_fire = u32::from(delay_fire);
    store_cs_gun_state(ps, state);
    gate
}

/// What a gun whose shots the CS layer times does with the fire button this command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CsGate {
    /// The MW2 weapon machine sees the buttons as they are.
    Pass,
    /// Not time yet: it does not see the fire button.
    Block,
    /// Time for a shot: it sees a fresh press.
    Press,
}

/// CS shot timing for guns with a burst mode or a scope-dependent cycle (`cs::CsWeapon::gated`),
/// once per command. Right click switches burst on and off (`SecondaryAttack`, 0.3 s apart); in
/// burst a press fires `shots` bullets on its own timetable whatever the button does; otherwise
/// a shot goes out when the gun's cycle (the zoomed one while scoped) is up, and a semi-auto
/// gun needs a fresh press for each.
fn cs_fire_gate(
    cs: &weapon_iw4::cs::CsWeapon,
    index: u8,
    ps: &mut PlayerState,
    cmd: &playerstate_iw4::UserCmd,
    old_buttons: u32,
    reloading: bool,
) -> CsGate {
    if !cs.gated() {
        return CsGate::Pass;
    }
    let now = cmd.server_time;
    let ms = |seconds: f32| (seconds * 1000.0).round() as i32;
    let bit = weapon_iw4::cs::silencer_bit(index);
    let ready = ps.weaponstate_primary == weapon_iw4::WeaponState::Ready as i32;
    if cs.burst.is_some()
        && cmd.buttons & playerstate_iw4::buttons::ADS != 0
        && ready
        && !reloading
        && now >= ps.cs_next_attack2_ms
    {
        ps.cs_burst_modes ^= bit;
        ps.cs_next_attack2_ms = now + 300;
    }
    let attack = playerstate_iw4::buttons::ATTACK;
    let pressing = cmd.buttons & attack != 0;
    let fresh = pressing && old_buttons & attack == 0;
    if let Some(burst) = cs.burst {
        if ps.cs_burst_left > 0 {
            // A burst under way: the button no longer matters.
            if now >= ps.cs_burst_next_ms {
                ps.cs_burst_left -= 1;
                ps.cs_burst_next_ms = now + ms(burst.gap);
                ps.cs_burst_shot = 2;
                return CsGate::Press;
            }
            return CsGate::Block;
        }
        if ps.cs_burst_modes & bit != 0 {
            if fresh && now >= ps.cs_fire_gate_ms {
                ps.cs_burst_left = burst.shots.saturating_sub(1);
                ps.cs_burst_next_ms = now + ms(burst.first_gap);
                ps.cs_fire_gate_ms = now + ms(burst.cycle);
                ps.cs_burst_shot = 1;
                return CsGate::Press;
            }
            return CsGate::Block;
        }
    }
    let cycle = if ps.cs_zoom != 0 && cs.cycle_zoomed > 0.0 {
        cs.cycle_zoomed
    } else {
        cs.cycle
    };
    if pressing && now >= ps.cs_fire_gate_ms && (fresh || !cs.semi_auto) {
        ps.cs_fire_gate_ms = now + ms(cycle);
        return CsGate::Press;
    }
    CsGate::Block
}

/// CS scope, once per command: the zoom a shot dropped returns once the gun is ready to fire
/// again (`ItemPostFrame`), and holding right click steps through the zoom levels every 0.3 s
/// (`SecondaryAttack`).
fn cs_zoom_frame(
    cs: &weapon_iw4::cs::CsWeapon,
    ps: &mut PlayerState,
    cmd: &playerstate_iw4::UserCmd,
    reloading: bool,
) {
    if cs.zoom.is_empty() {
        ps.cs_zoom = 0;
        ps.cs_last_zoom = 0;
        return;
    }
    let ready = ps.weaponstate_primary == weapon_iw4::WeaponState::Ready as i32;
    if ps.cs_last_zoom != 0 && ready {
        ps.cs_zoom = ps.cs_last_zoom;
        ps.cs_last_zoom = 0;
    }
    if cmd.buttons & playerstate_iw4::buttons::ADS != 0
        && ready
        && !reloading
        && cmd.server_time >= ps.cs_next_attack2_ms
    {
        ps.cs_zoom = weapon_iw4::cs::next_zoom(cs, ps.cs_zoom);
        ps.cs_next_attack2_ms = cmd.server_time + weapon_iw4::cs::CS_ZOOM_DELAY_MS;
    }
}

/// CS silencer, once per command (`SecondaryAttack` on the M4A1 and USP): right click on a ready
/// gun screws it on or off; nothing fires until that is done, and holding right click toggles
/// again only after it.
fn cs_silencer_frame(
    cs: &weapon_iw4::cs::CsWeapon,
    index: u8,
    ps: &mut PlayerState,
    cmd: &playerstate_iw4::UserCmd,
    reloading: bool,
) {
    let Some(silencer) = cs.silencer else {
        return;
    };
    let ready = ps.weaponstate_primary == weapon_iw4::WeaponState::Ready as i32;
    if cmd.buttons & playerstate_iw4::buttons::ADS != 0
        && ready
        && !reloading
        && cmd.server_time >= ps.cs_adjust_ms
    {
        ps.cs_silencers ^= weapon_iw4::cs::silencer_bit(index);
        ps.cs_adjust_ms = cmd.server_time + (silencer.adjust * 1000.0).round() as i32;
    }
}

/// A CS 1.6 shot leaves: accuracy and spread for it, then the recoil punch for the next one.
/// Returns the spread. The kick side flip is hashed from the shot so prediction agrees with it.
/// A scoped gun drops its zoom for the bolt (`AWPFire`) and takes it back when ready.
fn cs_weapon_fire(
    cs: &weapon_iw4::cs::CsWeapon,
    ps: &mut PlayerState,
    id: ClientId,
    server_time: i32,
    burst_shot: u8,
    index: u8,
) -> f32 {
    if cs.dual {
        // The Elites fire left, right, left...: the bit says which hand is next.
        ps.cs_burst_modes ^= weapon_iw4::cs::silencer_bit(index);
    }
    let shooter = cs_shooter(ps);
    let mut state = cs_gun_state(ps);
    if burst_shot == 2 {
        // `FireRemaining`: the later bullets of a burst keep a fixed spread (or the first
        // bullet's) and neither wear the accuracy nor kick the view again. The shot still shows
        // (viewmodel animation, muzzle flash, crosshair) through `cs_last_fire_ms`.
        ps.cs_last_fire_ms = server_time;
        return cs
            .burst
            .and_then(|burst| burst.follow_spread)
            .unwrap_or_else(|| weapon_iw4::cs::spread(cs, state.accuracy, shooter));
    }
    let spread = weapon_iw4::cs::fire(cs, &mut state, shooter, server_time);
    if !cs.zoom.is_empty() {
        if ps.cs_zoom != 0 && cs.unzoom_on_fire {
            ps.cs_last_zoom = ps.cs_zoom;
            ps.cs_zoom = 0;
        }
        ps.cs_next_attack2_ms = server_time + (cs.cycle * 1000.0).round() as i32;
    }
    let mut roll = (id.0 as u32)
        .wrapping_mul(0x9e37_79b9)
        .wrapping_add(server_time as u32)
        .wrapping_mul(0x85eb_ca6b)
        ^ (state.shots_fired as u32).wrapping_mul(0xc2b2_ae35);
    roll ^= roll >> 15;
    weapon_iw4::cs::recoil(cs, &mut state, shooter, &mut ps.cs_punch, roll);
    ps.cs_delay_fire = 1;
    store_cs_gun_state(ps, state);
    spread
}

/// A CS grenade in hand, once per command (`CHEGrenade::PrimaryAttack` / `WeaponIdle`): attack
/// pulls the pin, letting go throws (no sooner than half a second after the pull), then the next
/// grenade comes up, or the empty hand retires to the best weapon left. Prediction tracks the
/// pin and the count; only the authority launches the grenade.
fn cs_grenade_frame(
    world: &mut FrameWorld,
    tick: Tick,
    id: ClientId,
    weapon: u32,
    cmd: &playerstate_iw4::UserCmd,
) {
    use playerstate_iw4::cs_grenade::{IDLE, PULLED, THROWN};
    use weapon_iw4::cs::{
        CS_GRENADE_PULL_MS, CS_GRENADE_REDEPLOY_MS, CS_GRENADE_RETIRE_MS, cs_grenade,
    };
    let (Some(ps), Some(facts)) = (world.player(id).copied(), world.combat_facts_for(weapon))
    else {
        return;
    };
    let Some(grenade) = cs_grenade(facts.cs_weapon) else {
        return;
    };
    let now = cmd.server_time;
    let attack = cmd.buttons & playerstate_iw4::buttons::ATTACK != 0;
    let clip_key = clip_table_key(facts.clip_index, weapon);
    let count = get_clip_for_hand(&ps.ammoclip, clip_key, 0);
    match ps.cs_grenade {
        IDLE => {
            if count <= 0 {
                // An empty grenade is not kept: pressing 4 (or switching back) before the retire
                // timer ran left it in the hand with 0 left.
                crate::script_player::take_weapon(world, id, weapon);
                crate::item::raise_best_cs_weapon(world, id);
            } else if attack && now >= ps.cs_next_attack_ms {
                let ps = world.player_mut(id).expect("present player");
                ps.cs_grenade = PULLED;
                ps.cs_next_attack2_ms = now;
                ps.cs_next_attack_ms = now + CS_GRENADE_PULL_MS;
            }
        }
        PULLED => {
            if attack || now < ps.cs_next_attack_ms {
                return;
            }
            let left = (count - 1).max(0);
            {
                let ps = world.player_mut(id).expect("present player");
                ps.cs_grenade = THROWN;
                ps.cs_last_fire_ms = now;
                ps.cs_next_attack_ms = now
                    + if left > 0 {
                        CS_GRENADE_REDEPLOY_MS
                    } else {
                        CS_GRENADE_RETIRE_MS
                    };
                set_clip_for_hand(&mut ps.ammoclip, clip_key, 0, left);
            }
            world.client_meta_mut(id).set_ammo(weapon, left, 0);
            apply_player_anim_event(world, id, ANIM_ET_FIREWEAPON);
            if world.publishes_snapshot() {
                throw_cs_grenade(world, tick, id, &ps, grenade);
            }
        }
        _ => {
            if now < ps.cs_next_attack_ms {
                return;
            }
            if let Some(ps) = world.player_mut(id) {
                ps.cs_grenade = IDLE;
            }
            if count <= 0 {
                crate::script_player::take_weapon(world, id, weapon);
                crate::item::raise_best_cs_weapon(world, id);
            }
        }
    }
}

/// Launch `grenade`'s MW2 projectile along CS's throw: from 16 units in front of the eye, at the
/// lifted pitch and its speed, plus the thrower's own velocity.
fn throw_cs_grenade(
    world: &mut FrameWorld,
    tick: Tick,
    id: ClientId,
    ps: &PlayerState,
    grenade: &weapon_iw4::cs::CsGrenade,
) {
    let Some(projectile) = world.weapon_index_by_script_name(grenade.projectile) else {
        diag::warn!(Sim, "cs grenade: no MW2 `{}` to throw", grenade.projectile);
        return;
    };
    let (pitch, speed) = weapon_iw4::cs::grenade_throw(ps.viewangles[0] + ps.cs_punch[0]);
    let angles = [pitch, ps.viewangles[1] + ps.cs_punch[1], 0.0];
    let (direction, _, _) = math_iw4::angle_vectors(angles);
    let origin: [f32; 3] = core::array::from_fn(|i| {
        ps.origin[i]
            + direction[i] * 16.0
            + if i == 2 { ps.view_height_current } else { 0.0 }
    });
    let velocity: [f32; 3] = core::array::from_fn(|i| direction[i] * speed + ps.velocity[i]);
    let thrown = crate::equipment::spawn_grenade_projectile_with_velocity(
        world,
        id,
        projectile,
        tick,
        origin,
        angles,
        velocity,
        crate::equipment::GrenadeLaunchKind::Thrown {
            remaining_fuse_ms: None,
        },
    );
    diag::debug!(
        Sim,
        "cs grenade: client {} threw {} at {speed:.0} u/s (pitch {pitch:.1}) {}",
        id.0,
        grenade.name,
        if thrown { "" } else { "— not launched" }
    );
}

/// Buttons the CS knife takes from the MW2 weapon it wears: it never fires, aims, reloads or
/// melees; [`cs_knife_frame`] reads attack and aim as slash and stab instead.
const CS_KNIFE_BUTTONS: u32 = playerstate_iw4::buttons::ATTACK
    | playerstate_iw4::buttons::ADS
    | playerstate_iw4::buttons::MELEE_CHARGE
    | playerstate_iw4::buttons::RELOAD;

/// The CS 1.6 knife, once per command (`CKnife::ItemPostFrame`): the stab button wins over the
/// slash, each attack restarts both timers, and only the authority deals damage. Prediction runs
/// the same trace so the slash/stab animation and hit sound agree with the authority.
fn cs_knife_frame(
    world: &mut FrameWorld,
    tick: Tick,
    id: ClientId,
    weapon: u32,
    cmd: &playerstate_iw4::UserCmd,
) {
    use playerstate_iw4::{buttons, cs_knife};
    use weapon_iw4::cs::{CS_KNIFE, KnifeAttack};
    let Some(ps) = world.player(id).copied() else {
        return;
    };
    let now = cmd.server_time;
    let Some(attack) = weapon_iw4::cs::knife_attack(
        now,
        ps.cs_next_attack_ms,
        ps.cs_next_attack2_ms,
        cmd.buttons & buttons::ATTACK != 0,
        cmd.buttons & buttons::ADS != 0,
    ) else {
        return;
    };
    let origin = [
        ps.origin[0],
        ps.origin[1],
        ps.origin[2] + ps.view_height_current,
    ];
    let range = weapon_iw4::cs::knife_range(&CS_KNIFE, attack);
    let hit = cs_knife_trace(world, tick, id, origin, ps.viewangles, range);
    let (slash_delay, stab_delay) = weapon_iw4::cs::knife_delays(&CS_KNIFE, attack, hit.is_some());
    let ms = |seconds: f32| (seconds * 1000.0).round() as i32;
    let swing = ps.cs_knife >> cs_knife::SWING_SHIFT;
    let (anim, swing) = match attack {
        KnifeAttack::Slash if swing % 2 == 0 => (cs_knife::ANIM_SLASH1, swing.wrapping_add(1)),
        KnifeAttack::Slash => (cs_knife::ANIM_SLASH2, swing.wrapping_add(1)),
        KnifeAttack::Stab if hit.is_some() => (cs_knife::ANIM_STAB, swing),
        KnifeAttack::Stab => (cs_knife::ANIM_STAB_MISS, swing),
    };
    let met = match hit.as_ref().map(|(segment, _)| segment.collider) {
        Some(Some(ColliderId::Player { .. })) => cs_knife::HIT_PLAYER,
        Some(_) => cs_knife::HIT_WORLD,
        None => cs_knife::HIT_NOTHING,
    };
    if let Some(ps) = world.player_mut(id) {
        ps.cs_next_attack_ms = now + ms(slash_delay);
        ps.cs_next_attack2_ms = now + ms(stab_delay);
        ps.cs_last_fire_ms = now;
        ps.cs_knife =
            anim | (met << cs_knife::HIT_SHIFT) | ((swing & 0xff_ffff) << cs_knife::SWING_SHIFT);
    }
    apply_player_anim_event(world, id, ANIM_ET_KNIFE_MELEE);
    if !world.publishes_snapshot() {
        return;
    }
    let Some((segment, line)) = hit else {
        return;
    };
    let (forward, _, _) = math_iw4::angle_vectors(ps.viewangles);
    match segment.collider {
        Some(ColliderId::Player {
            client: victim,
            life: victim_life,
            hitloc,
        }) => {
            if !world
                .client_meta(victim)
                .is_some_and(|m| m.lifecycle == ClientLifecycle::Alive)
            {
                return;
            }
            let backstab = attack == KnifeAttack::Stab
                && world.player(victim).is_some_and(|v| {
                    weapon_iw4::cs::is_backstab(ps.origin, v.origin, v.viewangles[1])
                });
            // Only the centre line finds a hitgroup; a hull hit is a generic body hit.
            let (hitloc, scale) = if line {
                let scale = weapon_iw4::cs::CS_LOCATION_DAMAGE
                    .get(usize::from(hitloc))
                    .copied()
                    .unwrap_or(1.0);
                (hitloc, scale)
            } else {
                (0, 1.0)
            };
            let amount = (weapon_iw4::cs::knife_damage(&CS_KNIFE, attack, backstab) * scale) as i32;
            let attacker_life = world
                .client_meta(id)
                .map(|m| m.life_sequence)
                .unwrap_or_default();
            diag::debug!(
                Sim,
                "cs knife: client {} {:?} hit client {} hitloc {hitloc} backstab {backstab} for {amount}",
                id.0,
                attack,
                victim.0
            );
            let attempt = crate::DamageAttempt {
                splash: false,
                source: DamageSource::Melee,
                pellet: PelletId(0),
                attacker: id,
                attacker_life,
                target: victim,
                target_life: victim_life,
                weapon,
                amount,
                killcam_entity_start_time: 0,
                inflictor_origin: Some(origin),
                hitloc,
            };
            let _ = crate::damage::apply_damage_attempt(world, tick, &attempt);
            world.push_entity_event(
                tick,
                EventAudience::All,
                entity_iw4::EntityEventKind::MELEE_BLOOD,
                crate::EntityEventPayload {
                    number: id.0 as i32,
                    attacker_entity_num: id.0 as i32,
                    other_entity_num: victim.0 as i32,
                    weapon,
                    origin: segment.end,
                    direction: forward,
                    surf_type: segment.surf_type,
                    surface_flags: segment.surface_flags,
                    ..Default::default()
                },
            );
        }
        Some(ColliderId::World { .. }) => {
            if let Some(piece) = glass_piece_from_hit(segment.hit_type, segment.hit_id) {
                let at_time_ms =
                    i32::try_from(tick.0.saturating_mul(crate::MATCH_TICK_MS)).unwrap_or(i32::MAX);
                let mut holdrand = *world.stuck_holdrand_mut();
                world.world_objects_mut().apply_glass_hit(
                    piece,
                    u32::from(crate::world_objects::GLASS_MELEE_DAMAGE),
                    at_time_ms,
                    segment.end,
                    forward,
                    &mut || crate::item::random_unit(&mut holdrand),
                );
                *world.stuck_holdrand_mut() = holdrand;
            }
        }
        _ => {}
    }
}

/// The CS knife's reach (`CKnife::Swing` / `Stab`): a line from the eye, then, when it meets
/// nothing, a fan of lines spanning the `head_hull` the original sweeps. Returns what the
/// nearest line met and whether it was the centre line.
fn cs_knife_trace(
    world: &mut FrameWorld,
    tick: Tick,
    attacker: ClientId,
    origin: [f32; 3],
    angles: [f32; 3],
    range: f32,
) -> Option<(crate::bullet_collision::BulletTraceSegment, bool)> {
    let (forward, right, up) = math_iw4::angle_vectors(angles);
    let (width, height) = weapon_iw4::cs::CS_KNIFE.hull;
    let query = world.lagcomp_query_for(attacker, tick);
    let glass_pairs = world.world_objects().glass_damage_pairs();
    let is_solid = |piece| {
        crate::world_objects::glass_piece_is_solid(
            glass_pairs
                .iter()
                .find(|(id, _)| *id == u32::from(piece))
                .map(|(_, d)| *d)
                .unwrap_or(0),
        )
    };
    let distance = |a: [f32; 3], b: [f32; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    let mut best: Option<(f32, crate::bullet_collision::BulletTraceSegment)> = None;
    for (index, offset) in MELEE_TRACE_OFFSETS.iter().enumerate() {
        let end = melee_trace_end(origin, forward, right, up, range, width, height, *offset);
        let (segments, _) = bullet_trace_segments_filtered(
            world.clip_brushes(),
            world.clip_bsp(),
            world.clip_cmodels(),
            world.clip_mesh(),
            &query.players.poses,
            &query.entities.rows,
            &BulletTraceQuery {
                start: origin,
                end,
                mask: MASK_BULLET_WORLD,
                ignore: Some(attacker),
                ignore_hit: None,
                ignore_model: None,
            },
            weapon_iw4::BulletPenFacts::default(),
            world.penetration_table(),
            &is_solid,
            None,
        );
        let Some(segment) = segments.iter().find(|s| s.collider.is_some()) else {
            continue;
        };
        if segment.surface_flags & 0x10 != 0 {
            continue;
        }
        let reach = distance(origin, segment.end);
        if reach > distance(origin, end) + 0.01 {
            continue;
        }
        if index == 0 {
            return Some((*segment, true));
        }
        if best.as_ref().is_none_or(|(nearest, _)| reach < *nearest) {
            best = Some((reach, *segment));
        }
    }
    // CS sweeps `head_hull` against players' bounding boxes, not their hitboxes: MW2's bone
    // volumes sit well inside the body, so without this a stab (32 units) only connects when
    // pressed against the victim. The nearer of a box and anything the lines met wins.
    if let Some((reach, segment)) = knife_box_hit(&query.players.poses, attacker, origin, forward, range)
        && best.as_ref().is_none_or(|(nearest, _)| reach < *nearest)
    {
        best = Some((reach, segment));
    }
    best.map(|(_, segment)| (segment, false))
}

/// The nearest player box the knife's hull (`head_hull`: ±16 sideways, ±18 up and down) runs
/// into along `forward` within `range`, with how far along it met it.
fn knife_box_hit(
    poses: &[crate::bullet_collision::PlayerCollisionPose],
    attacker: ClientId,
    origin: [f32; 3],
    forward: [f32; 3],
    range: f32,
) -> Option<(f32, crate::bullet_collision::BulletTraceSegment)> {
    let (width, height) = weapon_iw4::cs::CS_KNIFE.hull;
    let half = [width, width, height];
    let mut best: Option<(f32, crate::bullet_collision::BulletTraceSegment)> = None;
    for pose in poses.iter().filter(|p| p.client != attacker) {
        let (mut enter, mut leave) = (0.0_f32, 1.0_f32);
        let mut inside = true;
        for axis in 0..3 {
            let lo = pose.origin[axis] + pose.mins[axis] - half[axis];
            let hi = pose.origin[axis] + pose.maxs[axis] + half[axis];
            let d = forward[axis] * range;
            if d.abs() < 1e-6 {
                if origin[axis] < lo || origin[axis] > hi {
                    inside = false;
                    break;
                }
                continue;
            }
            let (t0, t1) = ((lo - origin[axis]) / d, (hi - origin[axis]) / d);
            enter = enter.max(t0.min(t1));
            leave = leave.min(t0.max(t1));
        }
        if !inside || enter > leave {
            continue;
        }
        let reach = enter * range;
        if best.as_ref().is_some_and(|(nearest, _)| *nearest <= reach) {
            continue;
        }
        let end: [f32; 3] = core::array::from_fn(|i| origin[i] + forward[i] * reach);
        best = Some((
            reach,
            crate::bullet_collision::BulletTraceSegment {
                start: origin,
                end,
                normal: forward.map(|v| -v),
                surf_type: weapon_iw4::SURF_TYPE_FLESH as u8,
                surface_flags: 0,
                penetrated: false,
                thickness: 0.0,
                damage_mult: 1.0,
                path: crate::bullet_collision::BulletPath::Extended,
                collider: Some(ColliderId::Player {
                    client: pose.client,
                    life: pose.life_sequence,
                    hitloc: 0,
                }),
                startsolid: false,
                glass_encoded: 0,
                hit_type: trace_iw4::HITTYPE_ENTITY,
                hit_id: pose.client.0 as u16,
            },
        ));
    }
    best
}

pub fn spread_pellet_direction(
    angles: [f32; 3],
    spread_degrees: f32,
    rng: &mut MatchRng,
) -> [f32; 3] {
    spread_direction_on_plane(angles, spread_degrees, rng, 1.0)
}

pub fn spread_direction_on_plane(
    angles: [f32; 3],
    spread_degrees: f32,
    rng: &mut MatchRng,
    plane: f32,
) -> [f32; 3] {
    let (forward, right, up) = math_iw4::angle_vectors(angles);
    if spread_degrees <= 0.0 {
        return forward;
    }
    let unit = |draw: u32| draw as f32 / u32::MAX as f32;
    let radius = unit(rng.next_u32());
    let theta = unit(rng.next_u32()) * core::f32::consts::TAU;
    let lateral = plane * spread_degrees.to_radians().tan() * radius;
    let q = [
        plane * forward[0] + lateral * (theta.cos() * right[0] + theta.sin() * up[0]),
        plane * forward[1] + lateral * (theta.cos() * right[1] + theta.sin() * up[1]),
        plane * forward[2] + lateral * (theta.cos() * right[2] + theta.sin() * up[2]),
    ];
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
    if len > 0.0 {
        [q[0] / len, q[1] / len, q[2] / len]
    } else {
        forward
    }
}

fn cs_bullet_direction(angles: [f32; 3], spread: f32, rng: &mut MatchRng) -> [f32; 3] {
    let (forward, right, up) = math_iw4::angle_vectors(angles);
    let unit = |draw: u32| draw as f32 / u32::MAX as f32;
    let rolls = [
        unit(rng.next_u32()),
        unit(rng.next_u32()),
        unit(rng.next_u32()),
        unit(rng.next_u32()),
    ];
    weapon_iw4::cs::bullet_direction(forward, right, up, spread, rolls)
}

pub(crate) fn phase_emit(world: &FrameWorld, shots: &[AcceptedShot]) -> Vec<Emission> {
    let mut out = Vec::new();
    for shot in shots {
        let Some(facts) = world.combat_facts_for(shot.weapon) else {
            continue;
        };

        match fire_weapon_kind(facts.weap_type, facts.weap_class) {
            Some(FireWeaponKind::Bullet) => {
                if facts.damage <= 0 || facts.bullet_range() <= 0.0 {
                    continue;
                }
            }
            _ => continue,
        }
        let mut rng = MatchRng::new(shot.combat_seed as u64);
        let barrels = if facts.fire_type == 5 {
            shot.ammo_used.max(1)
        } else {
            1
        };
        let pellet_count = (facts.pellet_count() * barrels).clamp(1, u16::MAX as i32) as u16;
        for pellet in 0..pellet_count {
            out.push(Emission {
                combat_seed: shot.combat_seed,
                shot_id: shot.shot_id,
                pellet: PelletId(pellet),
                attacker: shot.attacker,
                attacker_life: shot.attacker_life,
                hand: shot.hand,
                weapon: shot.weapon,
                origin: shot.origin,
                direction: match shot.cs_spread {
                    Some(spread) => cs_bullet_direction(shot.angles, spread, &mut rng),
                    None => spread_pellet_direction(shot.angles, shot.spread_degrees, &mut rng),
                },
                max_range: weapon_iw4::cs::cs_weapon(facts.cs_weapon)
                    .map_or(facts.bullet_range(), |cs| cs.distance),
                base_damage: facts.damage,
                cs_silenced: shot.cs_silenced,
                cs_burst: shot.cs_burst,
            });
        }
    }
    out
}

fn segment_is_shield(collider: Option<ColliderId>) -> bool {
    matches!(
        collider,
        Some(ColliderId::Player {
            hitloc: crate::shield::HITLOC,
            ..
        })
    )
}

pub(crate) fn phase_trace(
    world: &mut FrameWorld,
    tick: Tick,
    emissions: &[Emission],
) -> TracePhaseOutput {
    let mut output = TracePhaseOutput::default();
    let mut pending: std::collections::VecDeque<_> =
        emissions.iter().copied().map(|em| (em, 0u8)).collect();
    while let Some((em, bounces)) = pending.pop_front() {
        let Some(facts) = world.combat_facts_for(em.weapon) else {
            continue;
        };
        let query = world.lagcomp_query_for(em.attacker, tick);
        let end = [
            em.origin[0] + em.direction[0] * em.max_range,
            em.origin[1] + em.direction[1] * em.max_range,
            em.origin[2] + em.direction[2] * em.max_range,
        ];
        let pen = world.bullet_pen_facts_for(em.weapon);
        let glass_damage = RefCell::new(
            world
                .world_objects()
                .glass_damage_pairs()
                .into_iter()
                .collect::<HashMap<u32, u16>>(),
        );
        let glass_seen = RefCell::new(Vec::<u32>::new());
        let on_glass_hit = |piece: u16, end: [f32; 3]| {
            let pane = u32::from(piece);
            if glass_seen.borrow().contains(&pane) {
                return;
            }
            glass_seen.borrow_mut().push(pane);
            let dist = {
                let dx = end[0] - em.origin[0];
                let dy = end[1] - em.origin[1];
                let dz = end[2] - em.origin[2];
                (dx * dx + dy * dy + dz * dz).sqrt()
            };
            let scaled =
                bullet_damage_at_distance(&facts, dist, em.cs_silenced, em.cs_burst).max(0) as u32;
            let mut map = glass_damage.borrow_mut();
            let cur = map.entry(pane).or_insert(0);
            *cur = glass_add_damage(*cur, scaled);
        };
        let (segments, terminal) = bullet_trace_segments_filtered(
            world.clip_brushes(),
            world.clip_bsp(),
            world.clip_cmodels(),
            world.clip_mesh(),
            &query.players.poses,
            &query.entities.rows,
            &BulletTraceQuery {
                start: em.origin,
                end,
                mask: MASK_BULLET_WORLD,
                ignore: Some(em.attacker),
                ignore_hit: None,
                ignore_model: None,
            },
            pen,
            world.penetration_table(),
            &|piece| {
                glass_piece_is_solid(
                    glass_damage
                        .borrow()
                        .get(&(u32::from(piece)))
                        .copied()
                        .unwrap_or(0),
                )
            },
            Some(&on_glass_hit),
        );
        if bounces < 8
            && facts.weap_type == weapon_iw4::WEAPTYPE_BULLET
            && facts.weap_class != weapon_iw4::WEAPCLASS_SPREAD
            && !pen.explosive_bullet
            && let Some(hit) = segments
                .last()
                .filter(|hit| !hit.startsolid && segment_is_shield(hit.collider))
        {
            let mut rng = MatchRng::new(
                u64::from(em.combat_seed) ^ (u64::from(em.pellet.0) << 32) ^ u64::from(bounces),
            );
            let chance = (pen.ricochet_chance * 0.5).clamp(0.0, 1.0);
            let roll = (rng.next_u32() >> 8) as f32 / 16_777_216.0;
            if roll <= chance
                && chance > 0.0
                && let Some(direction) = crate::shield::ricochet_direction(em.direction, hit.normal)
            {
                pending.push_back((
                    Emission {
                        origin: std::array::from_fn(|i| hit.end[i] + direction[i]),
                        direction,
                        ..em
                    },
                    bounces + 1,
                ));
            }
        }
        let entity_epoch = entity_collision_epoch(terminal, &query.entities.rows);
        let startsolid = segments.first().is_some_and(|s| s.startsolid);
        let impact_n = segments
            .iter()
            .filter(|s| bullet_process_on_hit(s.collider))
            .count() as u32;
        let event_n = segments
            .iter()
            .filter(|s| {
                bullet_process_on_hit(s.collider)
                    && entity_iw4::bullet_hit_event(
                        if segment_is_shield(s.collider) {
                            3
                        } else {
                            facts.impact_type
                        },
                        false,
                    )
                    .is_some()
            })
            .count() as u32;
        let (bone_center, bone_half_size, xmodel_contents, model_key) =
            hit_dump(terminal, &query.entities.rows);
        output.shot_verdicts.push(ShotCollisionVerdict {
            shot_id: em.shot_id,
            pellet: em.pellet,
            attacker: em.attacker,
            geometry: shot_collision_geometry(
                terminal,
                query.players.verdict,
                entity_epoch,
                player_representation(terminal, &query.players.poses),
            ),
            terminal,
            startsolid,
            bone_center,
            bone_half_size,
            xmodel_contents,
            model_key,
            end: segments.last().map(|s| s.end),
            entity_clip: entity_clip_kind(terminal, &query.entities.rows),
            impact_n,
            event_n,
        });
        if facts.cs_weapon != 0 && world.publishes_snapshot() {
            let end = segments.last().map_or(end, |s| s.end);
            diag::debug!(
                Sim,
                "cs shot: weapon {} from [{:.0}, {:.0}, {:.0}] to [{:.0}, {:.0}, {:.0}] dist {:.0} hit {:?} segments {} lagcomp {:?}",
                em.weapon,
                em.origin[0],
                em.origin[1],
                em.origin[2],
                end[0],
                end[1],
                end[2],
                (0..3)
                    .map(|a| (end[a] - em.origin[a]).powi(2))
                    .sum::<f32>()
                    .sqrt(),
                segments.last().and_then(|s| s.collider),
                segments.len(),
                query.players.verdict
            );
            for s in &segments {
                diag::debug!(
                    Sim,
                    "cs shot segment: end [{:.0}, {:.0}, {:.0}] mult {:.3} flags {:#x} startsolid {} collider {:?}",
                    s.end[0],
                    s.end[1],
                    s.end[2],
                    s.damage_mult,
                    s.surface_flags,
                    s.startsolid,
                    s.collider
                );
            }
        }
        if segments.is_empty() {
            continue;
        }
        let mut glass_hit: Vec<u32> = Vec::new();
        for segment in &segments {
            let exit = segment.surface_flags & fx_iw4::FX_IMPACT_EXIT_SURFACE_FLAG != 0;
            let dist = {
                let dx = segment.end[0] - em.origin[0];
                let dy = segment.end[1] - em.origin[1];
                let dz = segment.end[2] - em.origin[2];
                (dx * dx + dy * dy + dz * dz).sqrt()
            };
            let scaled =
                ((bullet_damage_at_distance(&facts, dist, em.cs_silenced, em.cs_burst) as f32)
                    * segment.damage_mult) as i32;
            if !exit && world.publishes_snapshot() {
                let means = crate::script_player::means(
                    world,
                    DamageSource::Shot(em.shot_id),
                    em.weapon,
                    0,
                    false,
                );
                crate::script::host::triggers::damage_line(
                    world.ecs(),
                    segment.start,
                    segment.end,
                    scaled,
                    em.attacker,
                    None,
                    means,
                );
            }
            let mut flesh_flags = 0u8;
            if !exit
                && let Some(ColliderId::Player {
                    client: victim,
                    life: victim_life,
                    hitloc,
                }) = segment.collider
            {
                let head = hud_iw4::obituary_is_headshot(hitloc);
                let mut fatal = false;
                if world.publishes_snapshot()
                    && scaled > 0
                    && let Some(vmeta) = world.client_meta(victim)
                    && vmeta.lifecycle == ClientLifecycle::Alive
                {
                    let attempt = crate::DamageAttempt {
                        splash: false,
                        source: DamageSource::Shot(em.shot_id),
                        pellet: em.pellet,
                        attacker: em.attacker,
                        attacker_life: em.attacker_life,
                        target: victim,
                        target_life: victim_life,
                        weapon: em.weapon,
                        amount: scaled,
                        killcam_entity_start_time: 0,
                        inflictor_origin: None,
                        hitloc,
                    };
                    fatal = matches!(
                        crate::damage::apply_damage_attempt(world, tick, &attempt),
                        crate::DamageOutcome::Died(_)
                    );
                }
                if hitloc != crate::shield::HITLOC {
                    flesh_flags = fx_iw4::flesh_hit_flags(head, fatal) as u8;
                }
            }
            let payload = crate::EntityEventPayload {
                number: em.attacker.0 as i32,
                attacker_entity_num: em.attacker.0 as i32,
                other_entity_num: match segment.collider {
                    Some(
                        ColliderId::EntityDObjBone { owner, .. }
                        | ColliderId::EntityLinkedBrush { owner, .. },
                    ) => owner
                        .script_model()
                        .and_then(|id| world.gentity_number(id))
                        .unwrap_or(ENTITYNUM_NONE),
                    Some(ColliderId::Player { client, .. }) => client.0 as i32,
                    _ => ENTITYNUM_NONE,
                },
                event_parm: i32::from(flesh_flags),
                weapon: em.weapon,
                correlation: em.shot_id.0,
                pellet: em.pellet.0,
                hand: em.hand,
                origin: segment.end,
                origin2: segment.start,
                direction: segment.normal,
                surf_type: segment.surf_type,
                surface_flags: segment.surface_flags,
                simulation_flags: u8::from(segment.penetrated),
            };
            let victim = match segment.collider {
                Some(ColliderId::Player { client, .. }) => Some(client),
                _ => None,
            };
            if bullet_process_on_hit(segment.collider) {
                if let Some(world_event) = entity_iw4::bullet_hit_event(
                    if segment_is_shield(segment.collider) {
                        3
                    } else {
                        facts.impact_type
                    },
                    false,
                ) {
                    let world_audience =
                        victim.map_or(EventAudience::All, EventAudience::AllExcept);
                    world.push_entity_event(tick, world_audience, world_event, payload);
                    if let Some(victim) = victim
                        && let Some(local_event) = entity_iw4::bullet_hit_event(
                            if segment_is_shield(segment.collider) {
                                3
                            } else {
                                facts.impact_type
                            },
                            true,
                        )
                    {
                        world.push_entity_event(
                            tick,
                            EventAudience::Client(victim),
                            local_event,
                            payload,
                        );
                    }
                } else if fx_iw4::impact_table_row(facts.impact_type, false).is_some() {
                    world.push_pellet_fx(crate::PelletFxRecord {
                        attacker: em.attacker.0 as i32,
                        weapon: em.weapon,
                        correlation: em.shot_id.0,
                        pellet: em.pellet.0,
                        hand: em.hand,
                        start: segment.start,
                        end: segment.end,
                        normal: segment.normal,
                        surf_type: segment.surf_type,
                        surface_flags: segment.surface_flags,
                        flesh_flags,
                    });
                }
            } else {
                world.push_pellet_fx(crate::PelletFxRecord {
                    attacker: em.attacker.0 as i32,
                    weapon: em.weapon,
                    correlation: em.shot_id.0,
                    pellet: em.pellet.0,
                    hand: em.hand,
                    start: segment.start,
                    end: segment.end,
                    normal: [0.0; 3],
                    surf_type: segment.surf_type,
                    surface_flags: segment.surface_flags,
                    flesh_flags: 0,
                });
            }
            if exit || !world.publishes_snapshot() || scaled <= 0 {
                continue;
            }
            match segment.collider {
                Some(ColliderId::Player { .. }) => {}
                Some(
                    ColliderId::EntityDObjBone { owner, .. }
                    | ColliderId::EntityLinkedBrush { owner, .. },
                ) => {
                    if let Some(ColliderId::EntityDObjBone { bone, .. }) = segment.collider
                        && crate::t5_destructible::apply_hit(
                            world,
                            tick,
                            owner,
                            bone,
                            scaled as u32,
                            Some(em.attacker),
                        )
                    {
                        continue;
                    }
                    if let Some(target) = owner.script_model() {
                        let bone = match segment.collider {
                            Some(ColliderId::EntityDObjBone { bone, .. }) => {
                                Some(usize::from(bone))
                            }
                            _ => None,
                        };
                        let means = crate::script_player::means(
                            world,
                            DamageSource::Shot(em.shot_id),
                            em.weapon,
                            0,
                            false,
                        );
                        crate::script::damage_entity(
                            world.ecs(),
                            &crate::script::EntityHit {
                                target,
                                amount: scaled,
                                attacker: Some(em.attacker),
                                means,
                                weapon: em.weapon,
                                point: segment.end,
                                dir: em.direction,
                                bone,
                                flags: 0,
                            },
                        );
                    }
                }
                Some(ColliderId::World { .. }) => {
                    if let Some(piece) = glass_piece_from_hit(segment.hit_type, segment.hit_id) {
                        if glass_hit.contains(&piece) {
                            continue;
                        }
                        glass_hit.push(piece);
                        let at_time_ms = i32::try_from(tick.0.saturating_mul(crate::MATCH_TICK_MS))
                            .unwrap_or(i32::MAX);
                        let mut holdrand = *world.stuck_holdrand_mut();
                        world.world_objects_mut().apply_glass_hit(
                            piece,
                            scaled as u32,
                            at_time_ms,
                            segment.end,
                            em.direction,
                            &mut || crate::item::random_unit(&mut holdrand),
                        );
                        *world.stuck_holdrand_mut() = holdrand;
                    }
                }
                None => {}
            }
        }
    }
    world.record_shot_collision_verdicts(&output.shot_verdicts);
    output
}

fn fire_weapon_melee(
    world: &mut FrameWorld,
    tick: Tick,
    attacker: ClientId,
    attacker_life: LifeSequence,
    weapon: u32,
    origin: [f32; 3],
    angles: [f32; 3],
) {
    let Some(facts) = world.combat_facts_for(weapon) else {
        return;
    };
    if facts.melee_damage <= 0 {
        return;
    }
    let (forward, right, up) = math_iw4::angle_vectors(angles);
    let range = PLAYER_MELEE_RANGE_DEFAULT;
    let width = PLAYER_MELEE_WIDTH_DEFAULT;
    let height = PLAYER_MELEE_HEIGHT_DEFAULT;
    let query = world.lagcomp_query_for(attacker, tick);
    let glass_pairs = world.world_objects().glass_damage_pairs();
    let is_solid = |piece| {
        crate::world_objects::glass_piece_is_solid(
            glass_pairs
                .iter()
                .find(|(id, _)| *id == u32::from(piece))
                .map(|(_, d)| *d)
                .unwrap_or(0),
        )
    };
    let mut best_frac = 1.0f32;
    let mut best_hit: Option<crate::bullet_collision::BulletTraceSegment> = None;
    let n = melee_trace_count(width, height);
    for (index, offset) in MELEE_TRACE_OFFSETS.iter().take(n).enumerate() {
        let end = melee_trace_end(origin, forward, right, up, range, width, height, *offset);
        let (segments, _) = bullet_trace_segments_filtered(
            world.clip_brushes(),
            world.clip_bsp(),
            world.clip_cmodels(),
            world.clip_mesh(),
            &query.players.poses,
            &query.entities.rows,
            &BulletTraceQuery {
                start: origin,
                end,
                mask: MASK_BULLET_WORLD,
                ignore: Some(attacker),
                ignore_hit: None,
                ignore_model: None,
            },
            weapon_iw4::BulletPenFacts::default(),
            world.penetration_table(),
            &is_solid,
            None,
        );
        if index == 0
            && world.publishes_snapshot()
            && let Some(segment) = segments.first()
        {
            crate::script::host::triggers::damage_line(
                world.ecs(),
                origin,
                segment.end,
                facts.melee_damage,
                attacker,
                None,
                "MOD_MELEE",
            );
        }
        let Some(segment) = segments.iter().find(|s| s.collider.is_some()) else {
            continue;
        };
        if segment.surface_flags & 0x10 != 0 {
            continue;
        }
        let ray = [end[0] - origin[0], end[1] - origin[1], end[2] - origin[2]];
        let hit = [
            segment.end[0] - origin[0],
            segment.end[1] - origin[1],
            segment.end[2] - origin[2],
        ];
        let ray_len2 = ray[0] * ray[0] + ray[1] * ray[1] + ray[2] * ray[2];
        if ray_len2 <= 0.0 {
            continue;
        }
        let frac = (hit[0] * hit[0] + hit[1] * hit[1] + hit[2] * hit[2]).sqrt() / ray_len2.sqrt();
        if frac >= 1.0 || frac > best_frac {
            continue;
        }
        best_frac = frac;
        best_hit = Some(*segment);
    }
    let Some(segment) = best_hit else {
        return;
    };
    let amount = facts.melee_damage + (world.combat_rng_mut().next_u32() % 5) as i32;
    let (kind, other) = match segment.collider {
        Some(ColliderId::Player { client, .. }) => {
            (entity_iw4::EntityEventKind::MELEE_HIT, client.0 as i32)
        }
        _ => (
            entity_iw4::EntityEventKind::MELEE_MISS,
            i32::from(trace_iw4::ENTITYNUM_WORLD),
        ),
    };
    world.push_entity_event(
        tick,
        EventAudience::All,
        kind,
        crate::EntityEventPayload {
            number: attacker.0 as i32,
            attacker_entity_num: attacker.0 as i32,
            other_entity_num: other,
            event_parm: i32::from(facts.knife_model != 0),
            weapon,
            origin: segment.end,
            direction: forward,
            surf_type: segment.surf_type,
            surface_flags: segment.surface_flags,
            ..Default::default()
        },
    );
    match segment.collider {
        Some(ColliderId::Player {
            client: victim,
            life: victim_life,
            hitloc,
        }) => {
            if let Some(vmeta) = world.client_meta(victim)
                && vmeta.lifecycle == ClientLifecycle::Alive
            {
                let attempt = crate::DamageAttempt {
                    splash: false,
                    source: DamageSource::Melee,
                    pellet: PelletId(0),
                    attacker,
                    attacker_life,
                    target: victim,
                    target_life: victim_life,
                    weapon,
                    amount,
                    killcam_entity_start_time: 0,
                    inflictor_origin: Some(origin),
                    hitloc,
                };
                let _ = crate::damage::apply_damage_attempt(world, tick, &attempt);
                world.push_entity_event(
                    tick,
                    EventAudience::All,
                    entity_iw4::EntityEventKind::MELEE_BLOOD,
                    crate::EntityEventPayload {
                        number: attacker.0 as i32,
                        attacker_entity_num: attacker.0 as i32,
                        other_entity_num: victim.0 as i32,
                        weapon,
                        origin: segment.end,
                        direction: forward,
                        surf_type: segment.surf_type,
                        surface_flags: segment.surface_flags,
                        ..Default::default()
                    },
                );
            }
        }
        Some(ColliderId::World { .. }) => {
            if let Some(piece) = glass_piece_from_hit(segment.hit_type, segment.hit_id) {
                let at_time_ms =
                    i32::try_from(tick.0.saturating_mul(crate::MATCH_TICK_MS)).unwrap_or(i32::MAX);
                let mut holdrand = *world.stuck_holdrand_mut();
                world.world_objects_mut().apply_glass_hit(
                    piece,
                    u32::from(crate::world_objects::GLASS_MELEE_DAMAGE),
                    at_time_ms,
                    segment.end,
                    forward,
                    &mut || crate::item::random_unit(&mut holdrand),
                );
                *world.stuck_holdrand_mut() = holdrand;
            }
        }
        _ => {}
    }
}

fn entity_collision_epoch(
    terminal: Option<ColliderId>,
    rows: &[EntityCollisionTraceGeom],
) -> Option<EntityCollisionEpoch> {
    let owner = match terminal? {
        ColliderId::EntityDObjBone { owner, .. } | ColliderId::EntityLinkedBrush { owner, .. } => {
            owner
        }
        ColliderId::World { .. } | ColliderId::Player { .. } => return None,
    };
    rows.iter()
        .find(|row| row.owner == owner)
        .map(|row| row.epoch)
}

fn hit_dump(
    terminal: Option<ColliderId>,
    rows: &[EntityCollisionTraceGeom],
) -> (
    Option<[f32; 3]>,
    Option<[f32; 3]>,
    Option<u32>,
    Option<String>,
) {
    let Some(ColliderId::EntityDObjBone { owner, bone, .. }) = terminal else {
        return (None, None, None, None);
    };
    let Some(row) = rows.iter().find(|row| row.owner == owner) else {
        return (None, None, None, None);
    };
    let bone = row
        .collision
        .as_ref()
        .and_then(|collision| collision.bones.iter().find(|b| b.bone == bone));
    (
        bone.map(|b| b.center),
        bone.map(|b| b.half_size),
        row.dobj_contents,
        row.model_key.clone(),
    )
}

fn player_representation(
    terminal: Option<ColliderId>,
    poses: &[crate::bullet_collision::PlayerCollisionPose],
) -> PlayerCollisionRepresentation {
    let Some(ColliderId::Player { client, .. }) = terminal else {
        return PlayerCollisionRepresentation::StandingAabbV1;
    };
    match poses.iter().find(|pose| pose.client == client) {
        Some(pose) if !pose.bones.is_empty() => PlayerCollisionRepresentation::PosedBonesV1,
        _ => PlayerCollisionRepresentation::StandingAabbV1,
    }
}

fn entity_clip_kind(
    terminal: Option<ColliderId>,
    rows: &[EntityCollisionTraceGeom],
) -> Option<EntityClipKind> {
    match terminal {
        Some(ColliderId::EntityLinkedBrush { .. }) => Some(EntityClipKind::LinkedBrush),
        Some(ColliderId::EntityDObjBone { owner, .. }) => {
            let coll = rows
                .iter()
                .find(|row| row.owner == owner)
                .and_then(|row| row.collision.as_ref())
                .and_then(|collision| collision.coll.as_ref());
            Some(
                if crate::bullet_collision::coll_tris_clip_available(coll, MASK_BULLET_WORLD) {
                    EntityClipKind::CollTris
                } else {
                    EntityClipKind::BoneBoxes
                },
            )
        }
        _ => None,
    }
}

fn shot_collision_geometry(
    terminal: Option<ColliderId>,
    player_history: HistorySampleVerdict,
    entity_epoch: Option<EntityCollisionEpoch>,
    representation: PlayerCollisionRepresentation,
) -> ShotCollisionGeometry {
    match terminal {
        Some(ColliderId::Player { .. }) => ShotCollisionGeometry::Player {
            representation,
            history: player_history,
        },
        Some(ColliderId::EntityDObjBone { .. } | ColliderId::EntityLinkedBrush { .. }) => {
            match entity_epoch {
                Some(epoch) => ShotCollisionGeometry::Entity { epoch },
                None => ShotCollisionGeometry::Miss,
            }
        }
        Some(ColliderId::World { .. }) => ShotCollisionGeometry::World,
        None => match player_history {
            HistorySampleVerdict::Refused { .. } => ShotCollisionGeometry::Player {
                representation,
                history: player_history,
            },
            _ => ShotCollisionGeometry::Miss,
        },
    }
}

pub(crate) fn bullet_process_on_hit(collider: Option<ColliderId>) -> bool {
    collider.is_some()
}

fn spend_ps_offhand_round(ps: &mut PlayerState, weapon: u32, facts: weapon_iw4::WeaponCombatFacts) {
    let clip_key = clip_table_key(facts.clip_index, weapon);
    if clip_key != 0 && clip_row_present(&ps.ammoclip, clip_key) {
        let clip = get_clip_for_hand(&ps.ammoclip, clip_key, 0);
        if clip > 0 {
            let _ = set_clip_for_hand(&mut ps.ammoclip, clip_key, 0, clip - 1);
            return;
        }
    }
    let ammo_key = ammo_table_key(facts.ammo_index, weapon);
    if ammo_key != 0 && ammo_row_present(&ps.ammo, ammo_key) {
        let stock = get_ammo_not_in_clip(&ps.ammo, ammo_key);
        if stock > 0 {
            let _ = set_ammo_not_in_clip(&mut ps.ammo, ammo_key, stock - 1);
        }
    }
}
