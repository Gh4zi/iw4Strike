//! The CS:GO pack (both animation sets) and the agents, read off the asking thread the first time
//! anyone asks, and shared by everyone after.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use glam::{Mat4, Quat, Vec3};
use mdl_goldsrc::Mat3x4;
use mdl_source::{BoneFrame, StudioModel, Vpk};

use crate::state::{AnimState, Animset};

/// A CS:GO agent's skeleton and hit boxes (no geometry), its bones matched to its animation set's.
pub struct Agent {
    pub name: String,
    /// Which animation set drives it (the Counter-Terrorists' or the Terrorists').
    pub ct: bool,
    pub model: StudioModel,
    /// For each of its bones, the animation set's bone of the same name.
    pub from_animset: Vec<Option<usize>>,
}

impl Agent {
    #[must_use]
    pub fn new(name: &str, model: StudioModel, animset: &Animset) -> Self {
        let from_animset = from_animset(&model, animset);
        Self {
            name: name.to_owned(),
            ct: name.starts_with("ctm_"),
            model,
            from_animset,
        }
    }

    /// The agent's local transforms for `state`'s pose: the animation set's pose carried over
    /// bone by bone (a bone the set does not have keeps its rest). `scratch` holds the set's pose.
    #[must_use]
    pub fn locals(
        &self,
        animset: &Animset,
        state: &AnimState,
        scratch: &mut Vec<BoneFrame>,
    ) -> Vec<BoneFrame> {
        state.pose(animset, scratch);
        agent_locals(&self.from_animset, &self.model.rest, scratch)
    }
}

/// For each bone of `model`, the bone of `animset` with its name.
#[must_use]
pub fn from_animset(model: &StudioModel, animset: &Animset) -> Vec<Option<usize>> {
    model
        .bones
        .iter()
        .map(|bone| {
            animset
                .model
                .bones
                .iter()
                .position(|b| b.name.eq_ignore_ascii_case(&bone.name))
        })
        .collect()
}

/// An agent's local transforms from its animation set's (`animset_locals`), by bone name.
#[must_use]
pub fn agent_locals(
    from_animset: &[Option<usize>],
    rest: &[BoneFrame],
    animset_locals: &[BoneFrame],
) -> Vec<BoneFrame> {
    from_animset
        .iter()
        .zip(rest)
        .map(|(from, rest)| from.and_then(|b| animset_locals.get(b).copied()).unwrap_or(*rest))
        .collect()
}

/// Where a body posed by `state` stands: turned to its feet's yaw, at `origin` (its feet).
#[must_use]
pub fn placement(state: &AnimState, origin: Vec3) -> Mat4 {
    Mat4::from_rotation_translation(Quat::from_rotation_z(state.foot_yaw.to_radians()), origin)
}

/// A row-major 3x4 bone transform as a matrix.
#[must_use]
pub fn mat4(m: &Mat3x4) -> Mat4 {
    Mat4::from_cols_array_2d(&[
        [m[0][0], m[1][0], m[2][0], 0.0],
        [m[0][1], m[1][1], m[2][1], 0.0],
        [m[0][2], m[1][2], m[2][2], 0.0],
        [m[0][3], m[1][3], m[2][3], 1.0],
    ])
}

enum Slot<T> {
    Loading,
    Ready(T),
}

/// The CS:GO pack and both animation sets, and the agents read so far.
pub struct Pack {
    pub vpk: Arc<Vpk>,
    /// The Counter-Terrorists' and the Terrorists'.
    pub animsets: [Option<Arc<Animset>>; 2],
    agents: Mutex<HashMap<String, Slot<Option<Arc<Agent>>>>>,
}

impl Pack {
    /// The animation set driving the Counter-Terrorists' agents (`ct`) or the Terrorists'.
    #[must_use]
    pub fn animset(&self, ct: bool) -> Option<Arc<Animset>> {
        self.animsets[usize::from(!ct)].clone()
    }

    /// Agent `name` (`ctm_sas`), its skeleton and hit boxes: read on a thread of its own the
    /// first time it is asked for, `None` until then (and when it cannot be read).
    pub fn agent(self: &Arc<Self>, name: &str) -> Option<Arc<Agent>> {
        let mut agents = self.agents.lock().ok()?;
        match agents.get(name) {
            Some(Slot::Ready(agent)) => return agent.clone(),
            Some(Slot::Loading) => return None,
            None => {}
        }
        agents.insert(name.to_owned(), Slot::Loading);
        let pack = self.clone();
        let name = name.to_owned();
        std::thread::spawn(move || {
            let agent = read_agent(&pack, &name);
            if let Ok(mut agents) = pack.agents.lock() {
                agents.insert(name, Slot::Ready(agent));
            }
        });
        None
    }
}

fn read_agent(pack: &Pack, name: &str) -> Option<Arc<Agent>> {
    let animset = pack.animset(name.starts_with("ctm_"))?;
    let path = format!("models/player/custom_player/legacy/{name}.mdl");
    let mdl = pack.vpk.read(&path)?;
    match StudioModel::parse(&mdl, &[], &[]) {
        Ok(model) => {
            diag::info!(
                World,
                "csgo agent {name}: {} bones, {} hit boxes",
                model.bones.len(),
                model.hitboxes.len()
            );
            Some(Arc::new(Agent::new(name, model, &animset)))
        }
        Err(error) => {
            diag::warn!(World, "csgo agent {name}: {error}");
            None
        }
    }
}

fn open_pack() -> Option<Arc<Pack>> {
    let path = asset_transport::find_csgo_pak()?;
    let vpk = match Vpk::open(&path) {
        Ok(vpk) => Arc::new(vpk),
        Err(error) => {
            diag::warn!(World, "csgo pack: {error}");
            return None;
        }
    };
    let animset = |name: &str| {
        let path = format!("models/player/custom_player/{name}.mdl");
        let mdl = vpk.read(&path)?;
        let ani = vpk.read(&path.replace(".mdl", ".ani"));
        match StudioModel::parse_with_ani(&mdl, &[], &[], ani.as_deref()) {
            Ok(model) => {
                diag::info!(
                    World,
                    "csgo pack: {name}: {} bones, {} sequences, {} animations",
                    model.bones.len(),
                    model.sequences.len(),
                    model.animations.len()
                );
                Some(Arc::new(Animset::new(model)))
            }
            Err(error) => {
                diag::warn!(World, "csgo pack: {name}: {error}");
                None
            }
        }
    };
    let animsets = [animset("animset_ct"), animset("animset_t")];
    Some(Arc::new(Pack {
        vpk,
        animsets,
        agents: Mutex::new(HashMap::new()),
    }))
}

static PACK: Mutex<Option<Slot<Option<Arc<Pack>>>>> = Mutex::new(None);

/// The CS:GO pack: read on a thread of its own the first time it is asked for, `None` until
/// then (and when no CS:GO install is found).
#[must_use]
pub fn pack() -> Option<Arc<Pack>> {
    let mut slot = PACK.lock().ok()?;
    match &*slot {
        Some(Slot::Ready(pack)) => return pack.clone(),
        Some(Slot::Loading) => return None,
        None => {}
    }
    *slot = Some(Slot::Loading);
    std::thread::spawn(|| {
        let pack = open_pack();
        if let Ok(mut slot) = PACK.lock() {
            *slot = Some(Slot::Ready(pack));
        }
    });
    None
}
