//! A CS2 first-person weapon: CS2's default arms (`weapon_arms`, bare arms in fingerless
//! gloves) and the weapon model, the animation skeletons they ride (the arms' viewmodel
//! skeleton, the weapon's hung under its `wpn` bone), and the clips the weapon's viewmodel
//! graph plays, by name (`draw_ak`, `shoot1_ak`, `reload_ak`, `lookat01_ak`...).

use mdl_source::Vpk;

use crate::anim::{self, Clip, Skeleton};
use crate::model::{self, Model};
use crate::pose::{self, Mat3x4, Pose};

/// CS2's default first-person arms.
pub const ARMS_MODEL: &str = "weapons/models/shared/arms/weapon_arms.vmdl_c";
/// The bone weapon skeletons hang from when the arms' skeleton does not say.
const DEFAULT_ATTACH_BONE: &str = "wpn";

/// A clip and the name it goes by (its file name: `shoot1_ak`).
#[derive(Clone, Debug)]
pub struct NamedClip {
    pub name: String,
    pub clip: Clip,
}

#[derive(Clone, Debug)]
pub struct Viewmodel {
    pub arms: Model,
    pub weapon: Model,
    pub arms_skeleton: Skeleton,
    /// Skeletons the clips' secondary animations sample (the weapon's, or a shared one).
    pub secondary_skeletons: Vec<Skeleton>,
    pub clips: Vec<NamedClip>,
}

fn read(vpk: &Vpk, path: &str) -> Result<Vec<u8>, String> {
    let compiled = if path.ends_with("_c") {
        path.to_owned()
    } else {
        format!("{path}_c")
    };
    vpk.read(&compiled)
        .ok_or_else(|| format!("{compiled} not in the pack"))
}

/// Every clip a graph refers to, and the clips of the graphs it refers to in turn (the
/// inspects graph).
fn graph_clips(vpk: &Vpk, graph: &str, depth: usize, out: &mut Vec<String>) {
    let Ok(bytes) = read(vpk, graph) else {
        return;
    };
    let Ok(resource) = crate::Resource::parse(&bytes) else {
        return;
    };
    for (_, name) in resource.external_refs() {
        if name.ends_with(".vnmclip") {
            if !out.contains(&name) {
                out.push(name);
            }
        } else if name.ends_with(".vnmgraph") && depth > 0 {
            graph_clips(vpk, &name, depth - 1, out);
        }
    }
}

/// Load the arms, the weapon model and the clips of its viewmodel graph
/// (`animation/graphs/viewmodel/viewmodel_gun.vnmgraph+ak47.vnmgraph`). `graph#part` keeps only
/// the clips the graph itself lists whose path holds `part` (CS2's C4 clips sit in the main
/// viewmodel graph, beside every weapon's own graph).
pub fn load(vpk: &Vpk, weapon_model: &str, graph: &str) -> Result<Viewmodel, String> {
    let arms = model::load(&read(vpk, ARMS_MODEL)?)?;
    let mut weapon = model::load(&read(vpk, weapon_model)?)?;
    let (graph, part) = graph
        .split_once('#')
        .map_or((graph, None), |(g, p)| (g, Some(p)));
    let mut paths = Vec::new();
    graph_clips(vpk, graph, usize::from(part.is_none()), &mut paths);
    if let Some(part) = part {
        paths.retain(|path| path.contains(part));
    }
    if paths.is_empty() {
        return Err(format!("{graph}: no clips"));
    }
    let mut clips = Vec::with_capacity(paths.len());
    for path in &paths {
        match read(vpk, path).and_then(|b| anim::load_clip(&b)) {
            Ok(clip) => {
                let name = path
                    .rsplit('/')
                    .next()
                    .unwrap_or(path)
                    .trim_end_matches(".vnmclip")
                    .to_owned();
                clips.push(NamedClip { name, clip });
            }
            Err(e) => return Err(format!("{path}: {e}")),
        }
    }
    let arms_id = clips
        .iter()
        .map(|c| c.clip.skeleton.clone())
        .find(|s| !s.is_empty())
        .or_else(|| arms.skeleton.clone())
        .ok_or("viewmodel: no arms skeleton")?;
    let arms_skeleton = anim::load_skeleton(&read(vpk, &arms_id)?)?;
    let mut secondary_skeletons: Vec<Skeleton> = Vec::new();
    for clip in &clips {
        for secondary in &clip.clip.secondary {
            if secondary.skeleton.is_empty()
                || secondary_skeletons
                    .iter()
                    .any(|s| s.id == secondary.skeleton)
            {
                continue;
            }
            secondary_skeletons.push(anim::load_skeleton(&read(vpk, &secondary.skeleton)?)?);
        }
    }
    drop_unanimated_meshes(&mut weapon, &secondary_skeletons);
    Ok(Viewmodel {
        arms,
        weapon,
        arms_skeleton,
        secondary_skeletons,
        clips,
    })
}

/// Drops the weapon meshes no animated bone carries: parts of the third-person model that hang off
/// bones the viewmodel clips never pose (the Elites' thigh holster), which would otherwise sit at
/// the eye. A weapon none of whose bones the clips name keeps every mesh.
fn drop_unanimated_meshes(weapon: &mut Model, skeletons: &[Skeleton]) {
    let mut carried: Vec<bool> = Vec::with_capacity(weapon.bones.len());
    for bone in &weapon.bones {
        let animated = skeletons.iter().any(|s| s.bone(&bone.name).is_some());
        let parent = bone
            .parent
            .is_some_and(|p| carried.get(p).copied().unwrap_or(false));
        carried.push(animated || parent);
    }
    if !carried.contains(&true) {
        return;
    }
    weapon.meshes.retain(|mesh| {
        mesh.vertex_buffers.iter().flatten().any(|v| {
            v.bones
                .iter()
                .zip(&v.weights)
                .any(|(b, w)| *w > 0.0 && carried.get(usize::from(*b)).copied().unwrap_or(false))
        })
    });
}

impl Viewmodel {
    /// The first clip whose name starts with `prefix` and passes `keep`.
    #[must_use]
    pub fn find(&self, prefix: &str, keep: impl Fn(&str) -> bool) -> Option<usize> {
        self.clips
            .iter()
            .position(|c| c.name.starts_with(prefix) && keep(&c.name))
    }

    /// Bones the skinning matrices cover: the arms' then the weapon's.
    #[must_use]
    pub fn bone_count(&self) -> usize {
        self.arms.bones.len() + self.weapon.bones.len()
    }

    /// Skinning matrices at `seconds` into clip `clip`: the arms' bones, then the weapon's
    /// (vertices of the weapon model are numbered after the arms').
    #[must_use]
    pub fn skin(&self, clip: usize, seconds: f32) -> Vec<Mat3x4> {
        let Some(named) = self.clips.get(clip) else {
            return vec![Mat3x4::IDENTITY; self.bone_count()];
        };
        let clip = &named.clip;
        let mut pose = Pose::default();
        let arms_world =
            pose::world_matrices(&self.arms_skeleton, &clip.sample(seconds), Mat3x4::IDENTITY);
        pose.add(&self.arms_skeleton, &arms_world);
        for secondary in &clip.secondary {
            let Some(skeleton) = self
                .secondary_skeletons
                .iter()
                .find(|s| s.id == secondary.skeleton)
            else {
                continue;
            };
            let attach = self
                .arms_skeleton
                .secondary
                .iter()
                .find(|(s, _)| *s == secondary.skeleton)
                .map_or(DEFAULT_ATTACH_BONE, |(_, bone)| bone.as_str());
            let root = self
                .arms_skeleton
                .bone(attach)
                .or_else(|| self.arms_skeleton.bone(DEFAULT_ATTACH_BONE))
                .map_or(Mat3x4::IDENTITY, |i| arms_world[i]);
            let world = pose::world_matrices(skeleton, &secondary.sample(seconds), root);
            pose.add(skeleton, &world);
        }
        let mut out = pose::skinning_matrices(&self.arms, &pose);
        out.extend(pose::skinning_matrices(&self.weapon, &pose));
        out
    }

    /// Where an attachment of the weapon (`muzzle_flash`, `shell_eject`) sits in bind space,
    /// and the combined bone it rides: skinning matrix × this gives it posed.
    #[must_use]
    pub fn weapon_attachment(&self, name: &str) -> Option<(usize, Mat3x4)> {
        let attachment = self.weapon.attachment(name)?;
        let bone = self.weapon.bone(&attachment.bone)?;
        let inverse_bind = self
            .weapon
            .inverse_bind
            .get(bone)
            .copied()
            .flatten()
            .map(Mat3x4)?;
        let local = Mat3x4::from_transform(&anim::Transform {
            translation: attachment.offset,
            rotation: attachment.rotation,
            scale: 1.0,
        });
        Some((
            self.arms.bones.len() + bone,
            inverse_bind.inverse().mul(&local),
        ))
    }
}
