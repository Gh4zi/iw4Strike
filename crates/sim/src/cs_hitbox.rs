//! CS:GO's hit boxes (CS:GO shooting): each living player's CS:GO animation state, the same one
//! the clients draw them with, taken in every step, and their agent's hit boxes placed on the pose
//! it makes. The collision history keeps each step's pose, so a shot traces the bodies as the
//! shooter saw them, and against them as CS:GO's server traces one. As in CS:GO, a body's bones
//! are only built when a shot comes near it.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use csgo_anim::hitbox::WorldHitbox;
use csgo_anim::{Agent, AnimState, Animset, Observation};
use entity_iw4::TEAM_ALLIES;
use glam::Vec3;

use crate::frame::FrameWorld;
use crate::match_state::ClientLifecycle;
use crate::world::ClientId;

#[derive(Clone)]
struct HitBody {
    agent: Arc<Agent>,
    animset: Arc<Animset>,
    state: AnimState,
    pose: Option<Arc<CsgoPose>>,
}

/// One step of a player's CS:GO body: their animation state as it was, where they stood, and the
/// hit boxes on it, placed the first time a shot needs them.
pub struct CsgoPose {
    agent: Arc<Agent>,
    animset: Arc<Animset>,
    state: AnimState,
    origin: Vec3,
    hitboxes: OnceLock<Vec<WorldHitbox>>,
}

impl CsgoPose {
    /// The hit boxes in the world.
    pub fn hitboxes(&self) -> &[WorldHitbox] {
        self.hitboxes.get_or_init(|| {
            let mut scratch = Vec::new();
            let locals = self.agent.locals(&self.animset, &self.state, &mut scratch);
            let bones = self.agent.model.world_from_locals(&locals);
            let world_from_model = csgo_anim::placement(&self.state, self.origin);
            csgo_anim::hitbox::world_hitboxes(&self.agent, &bones, world_from_model)
        })
    }

    /// Where the player stood (their feet).
    pub fn origin(&self) -> Vec3 {
        self.origin
    }
}

impl fmt::Debug for CsgoPose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CsgoPose")
            .field("agent", &self.agent.name)
            .field("origin", &self.origin)
            .finish()
    }
}

/// The same step of the same body (the history's poses are shared, not copied).
impl PartialEq for CsgoPose {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// Each living player's CS:GO body and hit boxes, by client.
#[derive(Clone, Default)]
pub(crate) struct CsgoHitBodies {
    bodies: HashMap<ClientId, HitBody>,
}

impl fmt::Debug for CsgoHitBodies {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CsgoHitBodies")
            .field("bodies", &self.bodies.len())
            .finish()
    }
}

impl CsgoHitBodies {
    /// `id`'s CS:GO pose this step; none when they have none (their hits use the MW2 body's).
    pub(crate) fn pose(&self, id: ClientId) -> Option<Arc<CsgoPose>> {
        self.bodies.get(&id)?.pose.clone()
    }

    pub(crate) fn all(&self) -> Vec<WorldHitbox> {
        self.bodies
            .values()
            .filter_map(|body| body.pose.as_ref())
            .flat_map(|pose| pose.hitboxes().iter().copied())
            .collect()
    }
}

/// `sv_showhitboxes`: whether the authority's world hands its hit boxes out each step, and the
/// last it handed out (the world itself is off on its own thread while it steps).
static SHOWN: AtomicBool = AtomicBool::new(false);
static SHOWN_HITBOXES: Mutex<Vec<WorldHitbox>> = Mutex::new(Vec::new());

/// Hands the authority's CS:GO hit boxes out each step (`sv_showhitboxes 1`), or stops.
pub fn show_csgo_hitboxes(on: bool) {
    if !SHOWN.swap(on, Ordering::Relaxed) || on {
        return;
    }
    if let Ok(mut shown) = SHOWN_HITBOXES.lock() {
        shown.clear();
    }
}

/// The CS:GO hit boxes the authority handed out last (while [`show_csgo_hitboxes`] is on).
pub fn shown_csgo_hitboxes() -> Vec<WorldHitbox> {
    SHOWN_HITBOXES.lock().map(|shown| shown.clone()).unwrap_or_default()
}

/// Whether shots are traced against CS:GO's hit boxes: CS:GO shooting under Counter-Strike rules.
pub(crate) fn csgo_hitboxes_on() -> bool {
    movement_iw4::rules::CS_RULES
        && crate::cs_settings::shooting_mode() == weapon_iw4::csgo::SHOOTING_CSGO
}

/// One step of every living player's CS:GO body (the authority's world only): their animation
/// state takes in what they did, and the step's pose is kept. A player whose agent is not read
/// yet keeps the MW2 body's hit volumes until it is.
pub(crate) fn tick_csgo_hit_bodies(world: &mut FrameWorld, msec: i32) {
    if !csgo_hitboxes_on() || !world.publishes_snapshot() {
        world.csgo_hit_bodies_mut().bodies.clear();
        return;
    }
    let Some(pack) = csgo_anim::pack() else {
        return;
    };
    let allies = crate::cs_radio::game_faction(world, true).unwrap_or_default();
    let axis = crate::cs_radio::game_faction(world, false).unwrap_or_default();
    let dt = msec as f32 / 1000.0;
    let mut bodies = std::mem::take(&mut world.csgo_hit_bodies_mut().bodies);
    let mut live = Vec::new();
    for id in world.client_ids_sorted() {
        let Some(meta) = world.client_meta(id) else {
            continue;
        };
        if meta.lifecycle != ClientLifecycle::Alive {
            continue;
        }
        let is_allies = meta.client_state_team == TEAM_ALLIES;
        let Some(ps) = world.player(id).copied() else {
            continue;
        };
        let faction = if is_allies { &allies } else { &axis };
        let name = csgo_anim::agent_model(weapon_iw4::cs::csgo_radio_voice(faction, is_allies));
        let Some(agent) = pack.agent(name) else {
            continue;
        };
        let Some(animset) = pack.animset(agent.ct) else {
            continue;
        };
        live.push(id);
        let body = bodies
            .entry(id)
            .and_modify(|body| {
                if !Arc::ptr_eq(&body.agent, &agent) {
                    *body = HitBody::new(&agent, &animset, id);
                }
            })
            .or_insert_with(|| HitBody::new(&agent, &animset, id));
        let weapon_name = world.weapon_script_name(ps.weapon).to_owned();
        body.state.observe(
            &body.animset,
            &Observation {
                ps: &ps,
                position: Vec3::from(ps.origin),
                eye_yaw: ps.viewangles[1],
                eye_pitch: ps.viewangles[0],
                weapon_name: &weapon_name,
            },
            dt,
        );
        body.pose = Some(Arc::new(CsgoPose {
            agent: body.agent.clone(),
            animset: body.animset.clone(),
            state: body.state.clone(),
            origin: Vec3::from(ps.origin),
            hitboxes: OnceLock::new(),
        }));
    }
    bodies.retain(|id, _| live.contains(id));
    world.csgo_hit_bodies_mut().bodies = bodies;
    if SHOWN.load(Ordering::Relaxed)
        && let Ok(mut shown) = SHOWN_HITBOXES.lock()
    {
        *shown = world.csgo_hit_bodies_mut().all();
    }
}

impl HitBody {
    fn new(agent: &Arc<Agent>, animset: &Arc<Animset>, id: ClientId) -> Self {
        Self {
            agent: agent.clone(),
            animset: animset.clone(),
            // Seeded by entity number, as the clients' are, so both draw the same idle loops.
            state: AnimState::new(animset, agent.ct, id.0),
            pose: None,
        }
    }
}

/// The MW2 hit location (`hitLocation_t`) standing for CS:GO hit group `group`, so the damage,
/// armour and headshot rules that read hit locations read CS:GO's groups: head, neck, chest
/// (`torso_upper`), stomach (`torso_lower`), the arms and legs, anything else none.
pub(crate) fn hitloc_for_group(group: i32) -> u8 {
    use csgo_anim::hitbox::{
        HITGROUP_CHEST, HITGROUP_HEAD, HITGROUP_LEFTARM, HITGROUP_LEFTLEG, HITGROUP_NECK,
        HITGROUP_RIGHTARM, HITGROUP_RIGHTLEG, HITGROUP_STOMACH,
    };
    match group {
        HITGROUP_HEAD => 2,
        HITGROUP_NECK => 3,
        HITGROUP_CHEST => 4,
        HITGROUP_STOMACH => 5,
        HITGROUP_RIGHTARM => 6,
        HITGROUP_LEFTARM => 7,
        HITGROUP_RIGHTLEG => 12,
        HITGROUP_LEFTLEG => 13,
        _ => 0,
    }
}
