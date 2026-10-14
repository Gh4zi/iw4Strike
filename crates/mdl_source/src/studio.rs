//! Source studio models (`.mdl` v44-49 with its `.vvd` vertices and `.vtx` strips), as
//! Counter-Strike: Source and CS:GO ship their viewmodels. Layouts follow the public SDK headers.
//! Animations are decoded once into per-frame bone poses; geometry is expanded into one triangle
//! list per mesh with up to three weighted bones per vertex (LOD 0).
//!
//! CS:GO's (v49) additions follow CS-Craft (github.com/FrosttysBots/CS-Craft, Apache-2.0): the
//! longer strip group records, animations kept in the model's `.ani` file and split into
//! sections, and the per-frame "frame animation" encoding.

use mdl_goldsrc::{Mat3x4, angle_quaternion, concat, slerp};

const BONE_LEN: usize = 216;
const ANIMDESC_LEN: usize = 100;
const SEQ_LEN: usize = 212;
const EVENT_LEN: usize = 80;
const TEXTURE_LEN: usize = 64;
const BODYPART_LEN: usize = 16;
/// `mstudiomodel_t`: name[64], type, radius, mesh/vertex/attachment/eyeball tables, vertex data,
/// 8 unused ints.
const MODEL_LEN: usize = 148;
const MESH_LEN: usize = 116;
const VVD_VERTEX_LEN: usize = 48;

const ANIM_RAWPOS: u8 = 0x01;
const ANIM_RAWROT: u8 = 0x02;
const ANIM_ANIMPOS: u8 = 0x04;
const ANIM_ANIMROT: u8 = 0x08;
const ANIM_DELTA: u8 = 0x10;
const ANIM_RAWROT2: u8 = 0x20;

const STRIP_IS_TRILIST: u8 = 0x01;

/// `mstudioanimdesc_t` flags.
const STUDIO_DELTA: i32 = 0x0004;
const STUDIO_ALLZEROS: i32 = 0x0020;
const STUDIO_FRAMEANIM: i32 = 0x0040;

/// Per-bone flags of a frame animation (CS:GO).
const FRAME_CONST_POS: u8 = 0x01;
const FRAME_CONST_ROT: u8 = 0x02;
const FRAME_ANIM_POS: u8 = 0x04;
const FRAME_ANIM_ROT: u8 = 0x08;
const FRAME_FULL_ANIM_POS: u8 = 0x10;
const FRAME_CONST_ROT2: u8 = 0x40;
const FRAME_ANIM_ROT2: u8 = 0x80;

/// Event id GoldSrc/Source clients play a sound script entry for.
pub const EVENT_CLIENT_SOUND: i32 = 5004;

#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    /// Model (bind) space to this bone's space.
    pub pose_to_bone: Mat3x4,
}

/// A hit box of the model's first hit box set (`mstudiobbox_t`): a capsule of
/// [`capsule_radius`](Self::capsule_radius) between `min` and `max` in its bone's space, or,
/// without a radius, the box they span, turned within the bone by `angle_offset`.
#[derive(Clone, Debug)]
pub struct Hitbox {
    pub name: String,
    pub bone: usize,
    /// `HITGROUP_*`: generic 0, head 1, chest 2, stomach 3, left arm 4, right arm 5, left leg 6,
    /// right leg 7, neck 8.
    pub group: i32,
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// Pitch, yaw and roll (degrees) of the box within its bone (`angOffsetOrientation`).
    pub angle_offset: [f32; 3],
    pub capsule_radius: f32,
}

/// A named point on the model (`mstudioattachment_t`): the muzzle, the shell port.
#[derive(Clone, Debug)]
pub struct Attachment {
    pub name: String,
    pub bone: usize,
    /// The attachment's frame in bind space relative to its bone's skinning matrix: a skinning
    /// matrix (as [`StudioModel::pose`] returns them) times this places it in model space.
    pub in_bind: Mat3x4,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneFrame {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
}

impl BoneFrame {
    /// The transform as a 3x4 matrix.
    #[must_use]
    pub fn matrix(&self) -> Mat3x4 {
        matrix(self.rotation, self.position)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// Position in the sequence, 0..1.
    pub cycle: f32,
    pub event: i32,
    pub options: String,
}

/// A pose parameter (`mstudioposeparamdesc_t`): a named value a model's blend grids and
/// layers read, from `start` to `end` (wrapping every `wrap` when non-zero).
#[derive(Clone, Debug, PartialEq)]
pub struct PoseParam {
    pub name: String,
    pub start: f32,
    pub end: f32,
    pub wrap: f32,
}

/// A sequence played over another wherever that one plays (`mstudioautolayer_t`): its weight
/// follows a pose parameter (`AL_POSE`) or the other's cycle between `start` and `end`.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoLayer {
    pub sequence: usize,
    /// The pose parameter of an `AL_POSE` layer.
    pub pose: Option<usize>,
    pub flags: i32,
    pub start: f32,
    pub peak: f32,
    pub tail: f32,
    pub end: f32,
}

/// `Sequence::flags`: changing to the sequence cuts to it at once, with no fade from the one
/// before (`STUDIO_SNAP`).
pub const SEQ_SNAP: i32 = 0x0002;
/// The sequence adds to the pose under it (`STUDIO_DELTA`).
pub const SEQ_DELTA: i32 = 0x0004;
/// A delta sequence applied after (in the frame of) the pose under it (`STUDIO_POST`).
pub const SEQ_POST: i32 = 0x0010;
/// The sequence moves the root by one of its bones' transforms (`STUDIO_ROOTXFORM`, CS:GO's
/// lean layer): see [`Sequence::root_driver`].
pub const SEQ_ROOTXFORM: i32 = 0x40000;
/// `AutoLayer::flags`.
pub const AL_SPLINE: i32 = 0x0040;
pub const AL_XFADE: i32 = 0x0080;
pub const AL_NOBLEND: i32 = 0x0200;
pub const AL_LOCAL: i32 = 0x1000;
pub const AL_POSE: i32 = 0x4000;

#[derive(Clone, Debug)]
pub struct Sequence {
    pub label: String,
    /// `ACT_VM_…` activity name, empty when none.
    pub activity: String,
    /// How often it is chosen among the sequences playing its activity (`actweight`; its
    /// size counts, the sign does not).
    pub activity_weight: i32,
    /// `STUDIO_*` flags (`SEQ_DELTA`, `SEQ_POST`, looping, …).
    pub flags: i32,
    /// The pose parameters (`StudioModel::pose_params`) choosing a cell of the blend grid, by
    /// axis, and the range each spans (`paramstart`/`paramend`).
    pub params: [Option<usize>; 2],
    pub param_range: [(f32, f32); 2],
    /// The value of each grid column and row (`posekey`), when the grid is not even: the
    /// first axis's `blend_size.0` values, then the second's `blend_size.1`.
    pub pose_keys: Vec<f32>,
    /// Activity modifiers (`rifle`, `crouch`, `moving`) the sequence is chosen by.
    pub act_mods: Vec<String>,
    /// The autolayers in full ([`autolayers`](Self::autolayers) holds their sequences).
    pub layers: Vec<AutoLayer>,
    /// Where (in cycle) a layer playing it has eased in, and starts easing out.
    pub fade_in: f32,
    pub fade_out: f32,
    pub fps: f32,
    pub looping: bool,
    pub num_frames: usize,
    pub events: Vec<Event>,
    /// The animations the sequence blends between (`StudioModel::animations` indices), a
    /// `blend_size.0` × `blend_size.1` grid row by row (an aim matrix is 3 × 3); one for most.
    pub blends: Vec<usize>,
    pub blend_size: (usize, usize),
    /// How much the sequence moves each bone (0 leaves it to whatever plays under it, 1 takes
    /// it over), by bone index.
    pub weights: Vec<f32>,
    /// With [`SEQ_ROOTXFORM`]: the bone whose local transform in this sequence moves the root
    /// of the pose under it (the sequence's own root is not used).
    pub root_driver: Option<usize>,
    /// Sequences played over this one wherever it plays (`StudioModel::sequences` indices), in
    /// order: CS:S's player poses carry their aim and hand-position layers this way.
    pub autolayers: Vec<usize>,
    /// Its first animation's frames, shared with `StudioModel::animations` (not a copy).
    frames: std::sync::Arc<[BoneFrame]>,
}

/// One decoded animation (`mstudioanimdesc_t`): `[frame][bone]` local transforms. A delta
/// animation's transforms are offsets to compose onto another pose (identity where it leaves a
/// bone alone).
#[derive(Clone, Debug)]
pub struct Animation {
    pub fps: f32,
    pub num_frames: usize,
    pub delta: bool,
    /// No data at all (`STUDIO_ALLZEROS`): a sequence playing it only carries layers.
    pub all_zeros: bool,
    /// How far the animation carries the model over one cycle (its last linear movement
    /// record's position; zero without one).
    pub movement: [f32; 3],
    frames: std::sync::Arc<[BoneFrame]>,
}

impl Animation {
    /// Frame `frame`'s local transform for every bone (the last frame past the end).
    #[must_use]
    pub fn frame(&self, frame: usize) -> &[BoneFrame] {
        let bones = self.frames.len() / self.num_frames.max(1);
        let frame = frame.min(self.num_frames.saturating_sub(1));
        &self.frames[frame * bones..(frame + 1) * bones]
    }
}

impl Sequence {
    #[must_use]
    pub fn duration(&self) -> f32 {
        if self.fps > 0.0 && self.num_frames > 1 {
            (self.num_frames - 1) as f32 / self.fps
        } else {
            0.0
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub bones: [u8; 3],
    pub weights: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mesh {
    /// Index into [`StudioModel::materials`].
    pub material: usize,
    pub first_vertex: usize,
    pub vertex_count: usize,
    /// Which body part and which of its models (bodygroup choice) the mesh belongs to; a
    /// model draws model 0 of every part unless told otherwise.
    pub body_part: usize,
    pub body_model: usize,
}

/// A body part (`mstudiobodyparts_t`): one of its models is drawn, e.g. a silencer or none.
#[derive(Clone, Debug)]
pub struct BodyPart {
    pub name: String,
    pub models: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct StudioModel {
    pub name: String,
    pub bones: Vec<Bone>,
    pub sequences: Vec<Sequence>,
    /// Material names as the model names them (without directory).
    pub materials: Vec<String>,
    /// Directories to look for materials in (`materials/<dir><name>.vmt`).
    pub material_dirs: Vec<String>,
    pub meshes: Vec<Mesh>,
    pub vertices: Vec<Vertex>,
    pub attachments: Vec<Attachment>,
    pub body_parts: Vec<BodyPart>,
    /// Every animation the sequences use.
    pub animations: Vec<Animation>,
    /// Skinning slots after the bones' own, for a bone-merged model's bones whose bind pose is
    /// not their namesake's: (the bone it follows, its own bind-to-bone matrix).
    pub merged: Vec<(usize, Mat3x4)>,
    /// Models whose sequences this one takes as well (`$includemodel`, CS:GO's knives).
    pub includes: Vec<String>,
    /// Each bone's local rest transform.
    pub rest: Vec<BoneFrame>,
    /// The pose parameters the sequences' blend grids and layers read.
    pub pose_params: Vec<PoseParam>,
    /// The first hit box set's hit boxes.
    pub hitboxes: Vec<Hitbox>,
}

struct R<'a>(&'a [u8]);

impl R<'_> {
    fn bytes(&self, at: usize, len: usize) -> Result<&[u8], String> {
        at.checked_add(len)
            .and_then(|end| self.0.get(at..end))
            .ok_or_else(|| format!("read past end at {at}+{len}"))
    }
    fn i32(&self, at: usize) -> Result<i32, String> {
        let b = self.bytes(at, 4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn usize(&self, at: usize) -> Result<usize, String> {
        usize::try_from(self.i32(at)?).map_err(|_| format!("negative count at {at}"))
    }
    /// A struct-relative offset added to its base.
    fn rel(&self, base: usize, at: usize) -> Result<usize, String> {
        let off = self.i32(base + at)?;
        usize::try_from(base as i64 + i64::from(off)).map_err(|_| format!("bad offset at {at}"))
    }
    fn f32(&self, at: usize) -> Result<f32, String> {
        Ok(f32::from_bits(self.i32(at)? as u32))
    }
    fn vec3(&self, at: usize) -> Result<[f32; 3], String> {
        Ok([self.f32(at)?, self.f32(at + 4)?, self.f32(at + 8)?])
    }
    fn u16(&self, at: usize) -> Result<u16, String> {
        let b = self.bytes(at, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn i16(&self, at: usize) -> Result<i16, String> {
        Ok(self.u16(at)? as i16)
    }
    fn u8(&self, at: usize) -> Result<u8, String> {
        self.0
            .get(at)
            .copied()
            .ok_or_else(|| format!("read past end at {at}"))
    }
    fn cstr(&self, at: usize) -> Result<String, String> {
        let rest = self.0.get(at..).ok_or("string past end")?;
        let len = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        Ok(String::from_utf8_lossy(&rest[..len]).into_owned())
    }
}

struct BoneDefaults {
    pos: [f32; 3],
    quat: [f32; 4],
    rot: [f32; 3],
    posscale: [f32; 3],
    rotscale: [f32; 3],
}

impl StudioModel {
    /// Parse a model from its three files (`.mdl`, `.vvd`, `.dx90.vtx`).
    pub fn parse(mdl: &[u8], vvd: &[u8], vtx: &[u8]) -> Result<Self, String> {
        Self::parse_with_ani(mdl, vvd, vtx, None)
    }

    /// [`parse`](Self::parse) with the model's `.ani` file, which holds the animations a model
    /// keeps outside the `.mdl` (CS:GO's); without it those animations hold the rest pose.
    pub fn parse_with_ani(
        mdl: &[u8],
        vvd: &[u8],
        vtx: &[u8],
        ani: Option<&[u8]>,
    ) -> Result<Self, String> {
        let r = R(mdl);
        if r.bytes(0, 4)? != b"IDST" {
            return Err("not a studio model".into());
        }
        let version = r.i32(4)?;
        if !(44..=49).contains(&version) {
            return Err(format!("studio version {version} unsupported"));
        }
        let name = r.cstr(12)?;
        let (bones, defaults) = parse_bones(&r)?;
        let data = AnimData {
            ani: ani.map(R),
            blocks: anim_blocks(&r).unwrap_or_default(),
        };
        let (sequences, animations) = parse_sequences(&r, &defaults, &data)?;
        let (materials, material_dirs, skin) = parse_materials(&r)?;
        // An animation-only model (`cs_player_shared.mdl`) comes without its geometry files.
        let (meshes, vertices) = if vvd.is_empty() && vtx.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            parse_geometry(&r, &R(vvd), &R(vtx), &skin, version)?
        };
        let attachments = parse_attachments(&r, &bones).unwrap_or_default();
        let hitboxes = parse_hitboxes(&r, bones.len()).unwrap_or_default();
        let body_parts = parse_body_parts(&r).unwrap_or_default();
        Ok(Self {
            name,
            bones,
            sequences,
            materials,
            material_dirs,
            meshes,
            vertices,
            attachments,
            body_parts,
            animations,
            merged: Vec::new(),
            includes: parse_includes(&r).unwrap_or_default(),
            rest: defaults
                .iter()
                .map(|d| BoneFrame {
                    position: d.pos,
                    rotation: d.quat,
                })
                .collect(),
            pose_params: parse_pose_params(&r).unwrap_or_default(),
            hitboxes,
        })
    }

    /// Adds the sequences of `other`, an animation-only model this one includes, its bones
    /// matched to these by name; a bone it does not have holds its rest pose and none of the
    /// sequence's weight.
    pub fn include_sequences(&mut self, other: &StudioModel) {
        let map: Vec<Option<usize>> = self
            .bones
            .iter()
            .map(|bone| {
                other
                    .bones
                    .iter()
                    .position(|o| o.name.eq_ignore_ascii_case(&bone.name))
            })
            .collect();
        let base = self.sequences.len();
        let other_bones = other.bones.len().max(1);
        for seq in &other.sequences {
            let mut frames = Vec::with_capacity(seq.num_frames * map.len());
            for frame in seq.frames.chunks_exact(other_bones) {
                frames.extend(
                    map.iter()
                        .zip(&self.rest)
                        .map(|(m, rest)| m.and_then(|o| frame.get(o).copied()).unwrap_or(*rest)),
                );
            }
            let weights = map
                .iter()
                .map(|m| m.and_then(|o| seq.weights.get(o).copied()).unwrap_or(0.0))
                .collect();
            self.sequences.push(Sequence {
                frames: frames.into(),
                weights,
                root_driver: seq
                    .root_driver
                    .and_then(|driver| map.iter().position(|m| *m == Some(driver))),
                blends: Vec::new(),
                autolayers: seq.autolayers.iter().map(|a| a + base).collect(),
                layers: seq
                    .layers
                    .iter()
                    .map(|l| AutoLayer {
                        sequence: l.sequence + base,
                        pose: None,
                        ..l.clone()
                    })
                    .collect(),
                params: [None; 2],
                ..seq.clone()
            });
        }
    }

    /// Draws `other` with this model, as Source's bone merge does (CS:GO's arms on its guns):
    /// each of its bones follows this model's bone of the same name (`Bip01` matches
    /// `v_weapon.Bip01`), one without a namesake its nearest ancestor that has one. A bone whose
    /// bind pose matches its namesake's skins through that bone; the others through extra slots
    /// after this model's bones (which [`pose`](Self::pose) fills). Fails when the slots would not
    /// fit `max_slots`.
    pub fn bone_merge(&mut self, other: &StudioModel, max_slots: usize) -> Result<(), String> {
        let short = |name: &str| name.rsplit('.').next().unwrap_or(name).to_ascii_lowercase();
        let find = |name: &str| {
            self.bones
                .iter()
                .position(|b| b.name.eq_ignore_ascii_case(name))
                .or_else(|| {
                    let wanted = short(name);
                    self.bones.iter().position(|b| short(&b.name) == wanted)
                })
        };
        // Per bone of `other`: the bone here it follows and the bone of `other` whose bind pose
        // places its vertices.
        let mut follows: Vec<(usize, usize)> = Vec::with_capacity(other.bones.len());
        for (index, bone) in other.bones.iter().enumerate() {
            let entry = match find(&bone.name) {
                Some(target) => (target, index),
                None => bone
                    .parent
                    .and_then(|p| follows.get(p).copied())
                    .unwrap_or((0, index)),
            };
            follows.push(entry);
        }
        let same = |a: &Mat3x4, b: &Mat3x4| {
            a.iter()
                .flatten()
                .zip(b.iter().flatten())
                .all(|(x, y)| (x - y).abs() < 1e-3)
        };
        let mut merged = self.merged.clone();
        let slots: Vec<usize> = follows
            .iter()
            .map(|&(target, source)| {
                let bind = other.bones[source].pose_to_bone;
                if same(&bind, &self.bones[target].pose_to_bone) {
                    target
                } else {
                    merged.push((target, bind));
                    self.bones.len() + merged.len() - 1
                }
            })
            .collect();
        if self.bones.len() + merged.len() > max_slots.min(256) {
            return Err(format!(
                "{} bones and {} merged slots are more than {max_slots}",
                self.bones.len(),
                merged.len()
            ));
        }
        self.merged = merged;
        let vertex_base = self.vertices.len();
        let material_base = self.materials.len();
        let part_base = self.body_parts.len();
        self.vertices.extend(other.vertices.iter().map(|v| Vertex {
            bones: v.bones.map(|b| slots.get(usize::from(b)).copied().unwrap_or(0) as u8),
            ..*v
        }));
        self.meshes.extend(other.meshes.iter().map(|m| Mesh {
            material: material_base + m.material,
            first_vertex: vertex_base + m.first_vertex,
            body_part: part_base + m.body_part,
            ..*m
        }));
        self.materials.extend(other.materials.iter().cloned());
        self.body_parts.extend(other.body_parts.iter().cloned());
        Ok(())
    }

    /// Model-space bone matrices for local transforms `locals` (one per bone).
    #[must_use]
    pub fn world_from_locals(&self, locals: &[BoneFrame]) -> Vec<Mat3x4> {
        let mut world: Vec<Mat3x4> = Vec::with_capacity(self.bones.len());
        for (bone, local) in self.bones.iter().zip(locals) {
            let local = matrix(local.rotation, local.position);
            let w = match bone.parent {
                Some(p) if p < world.len() => concat(&world[p], &local),
                _ => local,
            };
            world.push(w);
        }
        world
    }

    /// The body part named `name` (case-insensitive) and the index of its model named `model`.
    #[must_use]
    pub fn body_model(&self, part: &str, model: &str) -> Option<(usize, usize)> {
        let p = self
            .body_parts
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(part))?;
        let m = self.body_parts[p]
            .models
            .iter()
            .position(|n| n.eq_ignore_ascii_case(model))?;
        Some((p, m))
    }

    /// The muzzle attachment (`"1"` on Counter-Strike: Source viewmodels, else `muzzle`).
    #[must_use]
    pub fn muzzle(&self) -> Option<&Attachment> {
        let named = |name: &str| {
            self.attachments
                .iter()
                .find(|a| a.name.eq_ignore_ascii_case(name))
        };
        named("1").or_else(|| named("muzzle"))
    }

    /// The first sequence playing `activity` (`ACT_VM_RELOAD`, …).
    #[must_use]
    pub fn sequence_for_activity(&self, activity: &str) -> Option<usize> {
        self.sequences
            .iter()
            .position(|s| s.activity.eq_ignore_ascii_case(activity))
    }

    /// Every sequence playing `activity`.
    pub fn sequences_for_activity<'a>(
        &'a self,
        activity: &'a str,
    ) -> impl Iterator<Item = usize> + 'a {
        self.sequences
            .iter()
            .enumerate()
            .filter(move |(_, s)| s.activity.eq_ignore_ascii_case(activity))
            .map(|(i, _)| i)
    }

    /// Skinning matrices (bone pose × pose-to-bone) for `sequence` at `seconds`: a bind-space
    /// vertex times its bone's matrix lands in model space.
    pub fn pose(&self, sequence: usize, seconds: f32, out: &mut Vec<Mat3x4>) {
        self.pose_fading(sequence, seconds, &[], out);
    }

    /// [`pose`](Self::pose) with the sequences played before it still fading out over it, as
    /// Source's sequence transitions do: each of `fading` (sequence, seconds into it, weight) is
    /// blended over the pose by its weight in turn, bone by bone.
    pub fn pose_fading(
        &self,
        sequence: usize,
        seconds: f32,
        fading: &[(usize, f32, f32)],
        out: &mut Vec<Mat3x4>,
    ) {
        out.clear();
        let bones = self.bones.len();
        let mut world: Vec<Mat3x4> = Vec::with_capacity(bones);
        let seq = self.sequences.get(sequence).filter(|s| s.num_frames > 0);
        for (b, bone) in self.bones.iter().enumerate() {
            let mut frame = seq.map(|seq| self.sequence_frame(seq, seconds, b));
            for &(old, at, weight) in fading {
                if let (Some(current), Some(old)) = (
                    frame.as_mut(),
                    self.sequences.get(old).filter(|s| s.num_frames > 0),
                ) {
                    let old = self.sequence_frame(old, at, b);
                    current.rotation = slerp(current.rotation, old.rotation, weight);
                    current.position = core::array::from_fn(|i| {
                        current.position[i] + (old.position[i] - current.position[i]) * weight
                    });
                }
            }
            let local = frame.map_or(mdl_goldsrc::IDENTITY, |f| matrix(f.rotation, f.position));
            let w = match bone.parent {
                Some(p) if p < world.len() => concat(&world[p], &local),
                _ => local,
            };
            world.push(w);
        }
        out.extend(
            world
                .iter()
                .zip(&self.bones)
                .map(|(w, bone)| concat(w, &bone.pose_to_bone)),
        );
        out.extend(
            self.merged
                .iter()
                .map(|(bone, pose_to_bone)| concat(&world[*bone], pose_to_bone)),
        );
    }
}

impl StudioModel {
    /// Bone `bone`'s local transform in `seq` at `seconds` (between its two nearest frames).
    fn sequence_frame(&self, seq: &Sequence, seconds: f32, bone: usize) -> BoneFrame {
        let bones = self.bones.len();
        let last = seq.num_frames - 1;
        let mut frame = (seconds.max(0.0) * seq.fps).max(0.0);
        if seq.looping && last > 0 {
            frame %= last as f32;
        } else {
            frame = frame.min(last as f32);
        }
        let first = (frame as usize).min(last);
        let next = (first + 1).min(last);
        let t = frame - first as f32;
        let a = seq.frames[first * bones + bone];
        let c = seq.frames[next * bones + bone];
        BoneFrame {
            rotation: slerp(a.rotation, c.rotation, t),
            position: core::array::from_fn(|i| a.position[i] + (c.position[i] - a.position[i]) * t),
        }
    }
}

/// `mstudioposeparamdesc_t`s: name offset, flags, start, end, loop (20 bytes each).
fn parse_pose_params(r: &R<'_>) -> Result<Vec<PoseParam>, String> {
    let count = r.usize(300)?.min(64);
    let base = r.usize(304)?;
    (0..count)
        .map(|p| {
            let at = base + p * 20;
            Ok(PoseParam {
                name: r.cstr(r.rel(at, 0)?)?,
                start: r.f32(at + 8)?,
                end: r.f32(at + 12)?,
                wrap: r.f32(at + 16)?,
            })
        })
        .collect()
}

/// Inverse of a rigid transform (rotation and translation).
fn rigid_inverse(m: &Mat3x4) -> Mat3x4 {
    let mut out = [[0.0; 4]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().take(3).enumerate() {
            *v = m[c][r];
        }
        row[3] = -(row[0] * m[0][3] + row[1] * m[1][3] + row[2] * m[2][3]);
    }
    out
}

/// `mstudiomodelgroup_t`s (`includemodelindex`): label offset, model name offset.
fn parse_includes(r: &R<'_>) -> Result<Vec<String>, String> {
    let count = r.usize(336)?;
    let base = r.usize(340)?;
    (0..count).map(|i| r.cstr(r.rel(base + i * 8, 4)?)).collect()
}

/// `mstudioattachment_t`: name offset, flags, bone, a 3x4 local matrix, 8 unused ints.
const ATTACHMENT_LEN: usize = 92;

/// Bytes in one `mstudiobbox_t`.
const HITBOX_LEN: usize = 68;

/// The first hit box set (`numhitboxsets` at 172, `hitboxsetindex` at 176 of the header):
/// the one a player's hits are traced against.
fn parse_hitboxes(r: &R<'_>, bone_count: usize) -> Result<Vec<Hitbox>, String> {
    if r.usize(172)? == 0 {
        return Ok(Vec::new());
    }
    let set = r.usize(176)?;
    let count = r.usize(set + 4)?;
    let base = r.rel(set, 8)?;
    let mut out = Vec::with_capacity(count.min(256));
    for i in 0..count.min(256) {
        let at = base + i * HITBOX_LEN;
        let Some(bone) = usize::try_from(r.i32(at)?)
            .ok()
            .filter(|&b| b < bone_count)
        else {
            continue;
        };
        let name = if r.i32(at + 32)? == 0 {
            String::new()
        } else {
            r.cstr(r.rel(at, 32)?)?
        };
        out.push(Hitbox {
            name,
            bone,
            group: r.i32(at + 4)?,
            min: r.vec3(at + 8)?,
            max: r.vec3(at + 20)?,
            angle_offset: r.vec3(at + 36)?,
            capsule_radius: r.f32(at + 48)?,
        });
    }
    Ok(out)
}

fn parse_attachments(r: &R<'_>, bones: &[Bone]) -> Result<Vec<Attachment>, String> {
    let count = r.usize(240)?;
    let base = r.usize(244)?;
    let mut out = Vec::with_capacity(count.min(64));
    for i in 0..count.min(64) {
        let at = base + i * ATTACHMENT_LEN;
        let Some(bone) = usize::try_from(r.i32(at + 8)?)
            .ok()
            .filter(|&b| b < bones.len())
        else {
            continue;
        };
        let mut local = [[0.0; 4]; 3];
        for (row, out) in local.iter_mut().enumerate() {
            for (col, v) in out.iter_mut().enumerate() {
                *v = r.f32(at + 12 + (row * 4 + col) * 4)?;
            }
        }
        out.push(Attachment {
            name: r.cstr(r.rel(at, 0)?)?,
            bone,
            // skin = world * pose_to_bone, so world * local = skin * bone_to_pose * local.
            in_bind: concat(&rigid_inverse(&bones[bone].pose_to_bone), &local),
        });
    }
    Ok(out)
}

fn matrix(q: [f32; 4], p: [f32; 3]) -> Mat3x4 {
    let [x, y, z, w] = q;
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
            p[0],
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
            p[1],
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
            p[2],
        ],
    ]
}

fn parse_bones(r: &R<'_>) -> Result<(Vec<Bone>, Vec<BoneDefaults>), String> {
    let count = r.usize(156)?;
    let base = r.usize(160)?;
    if count == 0 || count > 256 {
        return Err(format!("bone count {count}"));
    }
    let mut bones = Vec::with_capacity(count);
    let mut defaults = Vec::with_capacity(count);
    for i in 0..count {
        let at = base + i * BONE_LEN;
        let parent = usize::try_from(r.i32(at + 4)?).ok().filter(|&p| p < i);
        let mut pose_to_bone = [[0.0; 4]; 3];
        for (row, out) in pose_to_bone.iter_mut().enumerate() {
            for (col, v) in out.iter_mut().enumerate() {
                *v = r.f32(at + 96 + (row * 4 + col) * 4)?;
            }
        }
        bones.push(Bone {
            name: r.cstr(r.rel(at, 0)?)?,
            parent,
            pose_to_bone,
        });
        let q = [
            r.f32(at + 44)?,
            r.f32(at + 48)?,
            r.f32(at + 52)?,
            r.f32(at + 56)?,
        ];
        defaults.push(BoneDefaults {
            pos: r.vec3(at + 32)?,
            quat: q,
            rot: r.vec3(at + 60)?,
            posscale: r.vec3(at + 72)?,
            rotscale: r.vec3(at + 84)?,
        });
    }
    Ok((bones, defaults))
}

/// Run-length channel value at `frame` (same scheme as GoldSrc).
fn anim_value(r: &R<'_>, start: usize, frame: usize) -> Result<i16, String> {
    let mut at = start;
    let mut k = frame;
    loop {
        let valid = usize::from(r.u8(at)?);
        let total = usize::from(r.u8(at + 1)?);
        if total == 0 {
            return Err("animation run with no frames".into());
        }
        if total > k {
            let index = if valid > k { k + 1 } else { valid };
            return r.i16(at + index * 2);
        }
        k -= total;
        at += (valid + 1) * 2;
    }
}

fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1f);
    let mant = f32::from(bits & 0x3ff);
    sign * match exp {
        0 => mant * 2f32.powi(-24),
        31 => f32::INFINITY,
        e => (1.0 + mant / 1024.0) * 2f32.powi(e - 15),
    }
}

fn quat48(r: &R<'_>, at: usize) -> Result<[f32; 4], String> {
    let x = f32::from(r.u16(at)?);
    let y = f32::from(r.u16(at + 2)?);
    let zw = r.u16(at + 4)?;
    let z = f32::from(zw & 0x7fff);
    let (x, y, z) = (
        (x - 32768.0) / 32768.0,
        (y - 32768.0) / 32768.0,
        (z - 16384.0) / 16384.0,
    );
    let mut w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
    if zw & 0x8000 != 0 {
        w = -w;
    }
    Ok([x, y, z, w])
}

/// CS:GO's `Quaternion48S`: the three smallest components, the dropped one's position in the top
/// bits of the first two words and its sign in the third's.
fn quat48s(r: &R<'_>, at: usize) -> Result<[f32; 4], String> {
    let words = [r.u16(at)?, r.u16(at + 2)?, r.u16(at + 4)?];
    let small = |v: u16| {
        (f32::from(v & 0x7fff) - 16384.0) / 16384.0 * core::f32::consts::FRAC_1_SQRT_2
    };
    let (a, b, c) = (small(words[0]), small(words[1]), small(words[2]));
    let mut largest = (1.0 - a * a - b * b - c * c).max(0.0).sqrt();
    if words[2] & 0x8000 != 0 {
        largest = -largest;
    }
    let dropped = usize::from(((words[0] >> 15) << 1) | (words[1] >> 15));
    let mut wxyz = [0.0f32; 4];
    wxyz[dropped] = largest;
    wxyz[(dropped + 1) % 4] = a;
    wxyz[(dropped + 2) % 4] = b;
    wxyz[(dropped + 3) % 4] = c;
    Ok([wxyz[1], wxyz[2], wxyz[3], wxyz[0]])
}

/// Three half floats.
fn vector48(r: &R<'_>, at: usize) -> Result<[f32; 3], String> {
    Ok([half(r.u16(at)?), half(r.u16(at + 2)?), half(r.u16(at + 4)?)])
}

fn normalized(q: [f32; 4]) -> [f32; 4] {
    let length = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if length > 0.0 {
        q.map(|c| c / length)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

fn quat64(r: &R<'_>, at: usize) -> Result<[f32; 4], String> {
    let b = r.bytes(at, 8)?;
    let v = u64::from_le_bytes(b.try_into().map_err(|_| "quat64")?);
    let part = |shift: u32| ((v >> shift) & 0x1f_ffff) as f32;
    let x = (part(0) - 1_048_576.0) / 1_048_576.5;
    let y = (part(21) - 1_048_576.0) / 1_048_576.5;
    let z = (part(42) - 1_048_576.0) / 1_048_576.5;
    let mut w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
    if v >> 63 != 0 {
        w = -w;
    }
    Ok([x, y, z, w])
}

/// Where animation data outside the `.mdl` lives: the `.ani` file and its blocks' (start, end).
struct AnimData<'a> {
    ani: Option<R<'a>>,
    blocks: Vec<(usize, usize)>,
}

/// The header's anim blocks (`numanimblocks`, `animblockindex`).
fn anim_blocks(r: &R<'_>) -> Result<Vec<(usize, usize)>, String> {
    let count = r.usize(352)?;
    let base = r.usize(356)?;
    (0..count)
        .map(|b| Ok((r.usize(base + b * 8)?, r.usize(base + b * 8 + 4)?)))
        .collect()
}

/// Decode one animation into `[frame][bone]` local transforms.
fn decode_animation(
    r: &R<'_>,
    desc: usize,
    defaults: &[BoneDefaults],
    data: &AnimData<'_>,
) -> Result<Animation, String> {
    let fps = r.f32(desc + 8)?;
    let flags = r.i32(desc + 12)?;
    // `STUDIO_DELTA`: offsets to another pose; bones it leaves alone stay put.
    let delta_anim = flags & STUDIO_DELTA != 0;
    let num_frames = r.usize(desc + 16)?.max(1);
    let anim_block = r.i32(desc + 52)?;
    let anim_index = r.i32(desc + 56)?;
    let section_index = r.i32(desc + 80)?;
    let section_frames = r.usize(desc + 84).unwrap_or(0);
    let bones = defaults.len();
    let rest: Vec<BoneFrame> = defaults
        .iter()
        .map(|d| {
            if delta_anim {
                BoneFrame {
                    position: [0.0; 3],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                }
            } else {
                BoneFrame {
                    position: d.pos,
                    rotation: d.quat,
                }
            }
        })
        .collect();
    // A block's data: block 0 is the `.mdl` (the index counts from the descriptor), the others
    // the `.ani` (counting from the block's start). Missing data holds the rest pose.
    let locate = |block: i32, index: i32| -> Option<(&R<'_>, usize)> {
        if block == 0 {
            (index != 0).then(|| (r, (desc as i64 + i64::from(index)) as usize))
        } else {
            let ani = data.ani.as_ref()?;
            let (start, _) = *data.blocks.get(usize::try_from(block).ok()?)?;
            Some((ani, start + usize::try_from(index).ok()?))
        }
    };
    // A long animation comes in sections of `section_frames`, each with its own block and index.
    let sections: Vec<(i32, i32)> = if section_frames > 0 && section_index != 0 {
        let base = (desc as i64 + i64::from(section_index)) as usize;
        (0..num_frames / section_frames + 2)
            .map(|s| Ok((r.i32(base + s * 8)?, r.i32(base + s * 8 + 4)?)))
            .collect::<Result<_, String>>()?
    } else {
        Vec::new()
    };
    let mut frames = Vec::with_capacity(num_frames * bones);
    for frame in 0..num_frames {
        let mut pose = rest.clone();
        let (block, index, local) = match sections.len() {
            0 => (anim_block, anim_index, frame),
            n => {
                let s = (frame / section_frames).min(n - 1);
                (sections[s].0, sections[s].1, frame - s * section_frames)
            }
        };
        if flags & STUDIO_ALLZEROS == 0
            && let Some((buf, start)) = locate(block, index)
        {
            if flags & STUDIO_FRAMEANIM != 0 {
                decode_frame_anim(buf, start, local, defaults, &mut pose)?;
            } else {
                decode_bone_records(buf, start, local, defaults, &mut pose)?;
            }
        }
        frames.extend_from_slice(&pose);
    }
    Ok(Animation {
        fps,
        num_frames,
        delta: delta_anim,
        all_zeros: flags & STUDIO_ALLZEROS != 0,
        movement: match r.usize(desc + 20) {
            // `mstudiomovement_t` (44 bytes): the position sits at 32.
            Ok(count) if count > 0 => {
                let last = (desc as i64 + i64::from(r.i32(desc + 24)?)) as usize + (count - 1) * 44;
                r.vec3(last + 32).unwrap_or([0.0; 3])
            }
            _ => [0.0; 3],
        },
        frames: frames.into(),
    })
}

/// One frame of a frame animation (CS:GO): a header, then per bone its flags, constant values
/// shared by every frame and values stored per frame.
fn decode_frame_anim(
    r: &R<'_>,
    start: usize,
    frame: usize,
    defaults: &[BoneDefaults],
    pose: &mut [BoneFrame],
) -> Result<(), String> {
    let mut constant = r.rel(start, 0)?;
    let frame_len = r.usize(start + 8)?;
    let mut per_frame = r.rel(start, 4)? + frame * frame_len;
    for (bone, slot) in pose.iter_mut().enumerate().take(defaults.len()) {
        let flags = r.u8(start + 24 + bone)?;
        let mut rotation = slot.rotation;
        let mut position = slot.position;
        if flags & FRAME_CONST_ROT2 != 0 {
            rotation = quat48s(r, constant)?;
            constant += 6;
        } else if flags & FRAME_CONST_ROT != 0 {
            rotation = quat48(r, constant)?;
            constant += 6;
        }
        if flags & FRAME_CONST_POS != 0 {
            position = vector48(r, constant)?;
            constant += 6;
        }
        if flags & FRAME_ANIM_ROT2 != 0 {
            rotation = quat48s(r, per_frame)?;
            per_frame += 6;
        } else if flags & FRAME_ANIM_ROT != 0 {
            rotation = quat48(r, per_frame)?;
            per_frame += 6;
        }
        if flags & FRAME_ANIM_POS != 0 {
            position = vector48(r, per_frame)?;
            per_frame += 6;
        } else if flags & FRAME_FULL_ANIM_POS != 0 {
            position = r.vec3(per_frame)?;
            per_frame += 12;
        }
        *slot = BoneFrame {
            position,
            rotation: normalized(rotation),
        };
    }
    Ok(())
}

/// One frame of the per-bone records (bone, flags, offset to the next) of `mstudioanim_t`.
fn decode_bone_records(
    r: &R<'_>,
    first: usize,
    frame: usize,
    defaults: &[BoneDefaults],
    pose: &mut [BoneFrame],
) -> Result<(), String> {
    {
        let mut at = first;
        loop {
            let bone = usize::from(r.u8(at)?);
            let flags = r.u8(at + 1)?;
            let next = r.i16(at + 2)?;
            let data = at + 4;
            if let Some(d) = defaults.get(bone) {
                let delta = flags & ANIM_DELTA != 0;
                let rotation = if flags & ANIM_RAWROT != 0 {
                    quat48(r, data)?
                } else if flags & ANIM_RAWROT2 != 0 {
                    quat64(r, data)?
                } else if flags & ANIM_ANIMROT != 0 {
                    let mut angles = [0.0f32; 3];
                    for (j, angle) in angles.iter_mut().enumerate() {
                        let off = r.i16(data + j * 2)?;
                        if off > 0 {
                            let v = anim_value(r, data + off as usize, frame)?;
                            *angle = f32::from(v) * d.rotscale[j];
                        }
                        if !delta {
                            *angle += d.rot[j];
                        }
                    }
                    angle_quaternion(angles)
                } else if delta {
                    [0.0, 0.0, 0.0, 1.0]
                } else {
                    d.quat
                };
                let position = if flags & ANIM_RAWPOS != 0 {
                    let p = data
                        + if flags & ANIM_RAWROT != 0 { 6 } else { 0 }
                        + if flags & ANIM_RAWROT2 != 0 { 8 } else { 0 };
                    [half(r.u16(p)?), half(r.u16(p + 2)?), half(r.u16(p + 4)?)]
                } else if flags & ANIM_ANIMPOS != 0 {
                    let p = data + if flags & ANIM_ANIMROT != 0 { 6 } else { 0 };
                    let mut pos = [0.0f32; 3];
                    for (j, v) in pos.iter_mut().enumerate() {
                        let off = r.i16(p + j * 2)?;
                        if off > 0 {
                            *v = f32::from(anim_value(r, p + off as usize, frame)?) * d.posscale[j];
                        }
                        if !delta {
                            *v += d.pos[j];
                        }
                    }
                    pos
                } else if delta {
                    [0.0; 3]
                } else {
                    d.pos
                };
                pose[bone] = BoneFrame { position, rotation };
            }
            if next <= 0 {
                break;
            }
            at += next as usize;
        }
    }
    Ok(())
}

/// `mstudioautolayer_t`'s size.
const AUTOLAYER_LEN: usize = 24;

fn parse_sequences(
    r: &R<'_>,
    defaults: &[BoneDefaults],
    data: &AnimData<'_>,
) -> Result<(Vec<Sequence>, Vec<Animation>), String> {
    let anim_count = r.usize(180)?;
    let anim_base = r.usize(184)?;
    let mut animations = Vec::with_capacity(anim_count);
    for a in 0..anim_count {
        animations.push(decode_animation(
            r,
            anim_base + a * ANIMDESC_LEN,
            defaults,
            data,
        )?);
    }
    let count = r.usize(188)?;
    let base = r.usize(192)?;
    let mut sequences = Vec::with_capacity(count);
    for s in 0..count {
        let at = base + s * SEQ_LEN;
        let label = r.cstr(r.rel(at, 4)?)?;
        let activity = r.cstr(r.rel(at, 8)?)?;
        let flags = r.i32(at + 12)?;
        let activity_weight = r.i32(at + 20)?;
        let event_count = r.usize(at + 24)?;
        let event_base = r.rel(at, 28)?;
        let anim_slot = r.rel(at, 60)?;
        let anim = usize::try_from(r.i16(anim_slot)?).unwrap_or(0);
        let blend_size = (
            usize::try_from(r.i32(at + 68)?).unwrap_or(1).clamp(1, 16),
            usize::try_from(r.i32(at + 72)?).unwrap_or(1).clamp(1, 16),
        );
        let blends = (0..blend_size.0 * blend_size.1)
            .map(|i| Ok(usize::try_from(r.i16(anim_slot + i * 2)?).unwrap_or(0)))
            .collect::<Result<Vec<_>, String>>()?;
        let autolayer_base = r.rel(at, 152)?;
        // Only player animations use these; a model without them still loads.
        let layers = (0..r.usize(at + 148).unwrap_or(0).min(16))
            .map(|l| {
                let la = autolayer_base + l * AUTOLAYER_LEN;
                Ok(AutoLayer {
                    sequence: usize::try_from(r.i16(la)?).unwrap_or(0),
                    pose: usize::try_from(r.i16(la + 2)?).ok(),
                    flags: r.i32(la + 4)?,
                    start: r.f32(la + 8)?,
                    peak: r.f32(la + 12)?,
                    tail: r.f32(la + 16)?,
                    end: r.f32(la + 20)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()
            .unwrap_or_else(|_| Vec::new());
        let autolayers = layers.iter().map(|l| l.sequence).collect();
        let param = |off: usize| r.i32(at + off).ok().and_then(|p| usize::try_from(p).ok());
        let params = [param(76), param(80)];
        let param_range = [
            (r.f32(at + 84).unwrap_or(0.0), r.f32(at + 92).unwrap_or(0.0)),
            (r.f32(at + 88).unwrap_or(0.0), r.f32(at + 96).unwrap_or(0.0)),
        ];
        let pose_keys = match r.i32(at + 160) {
            Ok(index) if index != 0 => (0..blend_size.0 + blend_size.1)
                .map(|k| r.f32(r.rel(at, 160)? + k * 4))
                .collect::<Result<Vec<_>, String>>()
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let act_mods = (0..r.usize(at + 188).unwrap_or(0).min(16))
            .map(|m| {
                let ma = r.rel(at, 184)? + m * 4;
                r.cstr(r.rel(ma, 0)?)
            })
            .collect::<Result<Vec<_>, String>>()
            .unwrap_or_default();
        let weight_base = r.rel(at, 156)?;
        let weights = (0..defaults.len())
            .map(|b| r.f32(weight_base + b * 4))
            .collect::<Result<Vec<_>, String>>()
            .unwrap_or_else(|_| Vec::new());
        let events = (0..event_count)
            .map(|e| {
                let ev = event_base + e * EVENT_LEN;
                Ok(Event {
                    cycle: r.f32(ev)?,
                    event: r.i32(ev + 4)?,
                    options: r.cstr(ev + 12)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let Animation {
            fps,
            num_frames,
            frames,
            ..
        } = animations
            .get(anim)
            .cloned()
            .ok_or_else(|| format!("sequence {label} names animation {anim}"))?;
        sequences.push(Sequence {
            label,
            activity,
            activity_weight,
            flags,
            params,
            param_range,
            pose_keys,
            act_mods,
            layers,
            fade_in: r.f32(at + 104).unwrap_or(0.2),
            fade_out: r.f32(at + 108).unwrap_or(0.2),
            fps,
            looping: flags & 1 != 0,
            num_frames,
            events,
            blends,
            blend_size,
            weights,
            root_driver: usize::try_from(r.i32(at + 200).unwrap_or(0))
                .ok()
                .filter(|&bone| bone > 0 && flags & SEQ_ROOTXFORM != 0),
            autolayers,
            frames,
        });
    }
    Ok((sequences, animations))
}

/// Material names, search directories, and the skin family 0 table (mesh material → name).
fn parse_materials(r: &R<'_>) -> Result<(Vec<String>, Vec<String>, Vec<usize>), String> {
    let tex_count = r.usize(204)?;
    let tex_base = r.usize(208)?;
    let materials = (0..tex_count)
        .map(|t| {
            let at = tex_base + t * TEXTURE_LEN;
            r.cstr(r.rel(at, 0)?)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let cd_count = r.usize(212)?;
    let cd_base = r.usize(216)?;
    let dirs = (0..cd_count)
        .map(|c| r.cstr(r.usize(cd_base + c * 4)?))
        .collect::<Result<Vec<_>, String>>()?;
    let refs = r.usize(220)?;
    let skin_base = r.usize(228)?;
    let skin = (0..refs)
        .map(|i| usize::try_from(r.i16(skin_base + i * 2)?).map_err(|_| "skin".to_owned()))
        .collect::<Result<Vec<_>, String>>()?;
    Ok((materials, dirs, skin))
}

fn vvd_vertices(vvd: &R<'_>) -> Result<Vec<Vertex>, String> {
    if vvd.bytes(0, 4)? != b"IDSV" {
        return Err("not a VVD".into());
    }
    let lod0 = vvd.usize(16)?;
    let fixups = vvd.usize(48)?;
    let fixup_base = vvd.usize(52)?;
    let data = vvd.usize(56)?;
    let read = |i: usize| -> Result<Vertex, String> {
        let at = data + i * VVD_VERTEX_LEN;
        let n = usize::from(vvd.u8(at + 15)?).min(3);
        let mut weights = [0.0; 3];
        let mut bones = [0u8; 3];
        for k in 0..n {
            weights[k] = vvd.f32(at + k * 4)?;
            bones[k] = vvd.u8(at + 12 + k)?;
        }
        Ok(Vertex {
            position: vvd.vec3(at + 16)?,
            normal: vvd.vec3(at + 28)?,
            uv: [vvd.f32(at + 40)?, vvd.f32(at + 44)?],
            bones,
            weights,
        })
    };
    let mut out = Vec::with_capacity(lod0);
    if fixups == 0 {
        for i in 0..lod0 {
            out.push(read(i)?);
        }
    } else {
        for f in 0..fixups {
            let at = fixup_base + f * 12;
            if vvd.i32(at)? < 0 {
                continue;
            }
            let source = vvd.usize(at + 4)?;
            let count = vvd.usize(at + 8)?;
            for i in source..source + count {
                out.push(read(i)?);
            }
        }
    }
    Ok(out)
}

/// Walk the MDL body part → model → mesh tree alongside the VTX one, emitting LOD 0 triangles.
fn parse_body_parts(r: &R<'_>) -> Result<Vec<BodyPart>, String> {
    let parts = r.usize(232)?;
    let base = r.usize(236)?;
    let mut out = Vec::with_capacity(parts.min(32));
    for p in 0..parts.min(32) {
        let part = base + p * BODYPART_LEN;
        let count = r.usize(part + 4)?;
        let first = r.rel(part, 12)?;
        let mut models = Vec::with_capacity(count.min(16));
        for m in 0..count.min(16) {
            let name = r.cstr(first + m * MODEL_LEN)?;
            models.push(name.chars().take(64).collect());
        }
        out.push(BodyPart {
            name: r.cstr(r.rel(part, 0)?)?,
            models,
        });
    }
    Ok(out)
}

fn parse_geometry(
    r: &R<'_>,
    vvd: &R<'_>,
    vtx: &R<'_>,
    skin: &[usize],
    version: i32,
) -> Result<(Vec<Mesh>, Vec<Vertex>), String> {
    // v49's strip groups and strips carry topology counts too; its strip groups are one
    // triangle list each.
    let group_len = if version >= 49 { 33 } else { 25 };
    let source = vvd_vertices(vvd)?;
    let parts = r.usize(232)?;
    let parts_base = r.usize(236)?;
    if vtx.i32(0)? != 7 {
        return Err(format!("VTX version {}", vtx.i32(0)?));
    }
    let vtx_parts = vtx.usize(28)?;
    let vtx_parts_base = vtx.usize(32)?;
    let mut meshes = Vec::new();
    let mut vertices = Vec::new();
    for p in 0..parts.min(vtx_parts) {
        let part = parts_base + p * BODYPART_LEN;
        let vtx_part = vtx_parts_base + p * 8;
        let model_count = r.usize(part + 4)?.min(vtx.usize(vtx_part)?);
        for body_model in 0..model_count {
            let model = r.rel(part, 12)? + body_model * MODEL_LEN;
            let vtx_model = vtx.rel(vtx_part, 4)? + body_model * 8;
            let vtx_lod = vtx.rel(vtx_model, 4)?;
            let model_vertex_base = r.usize(model + 84)? / VVD_VERTEX_LEN;
            let mesh_count = r.usize(model + 72)?;
            let mesh_base = r.rel(model, 76)?;
            let vtx_mesh_count = vtx.usize(vtx_lod)?;
            let vtx_mesh_base = vtx.rel(vtx_lod, 4)?;
            for m in 0..mesh_count.min(vtx_mesh_count) {
                let mesh = mesh_base + m * MESH_LEN;
                let material_ref = r.usize(mesh)?;
                let material = skin.get(material_ref).copied().unwrap_or(material_ref);
                let mesh_vertex_base = model_vertex_base + r.usize(mesh + 12)?;
                let vtx_mesh = vtx_mesh_base + m * 9;
                let groups = vtx.usize(vtx_mesh)?;
                let group_base = vtx.rel(vtx_mesh, 4)?;
                let first_vertex = vertices.len();
                for g in 0..groups {
                    let sg = group_base + g * group_len;
                    let sg_verts = vtx.rel(sg, 4)?;
                    let sg_indices = vtx.rel(sg, 12)?;
                    let strips = vtx.usize(sg + 16)?;
                    let strip_base = vtx.rel(sg, 20)?;
                    let corner = |index: usize| -> Result<Vertex, String> {
                        let local = usize::from(vtx.u16(sg_indices + index * 2)?);
                        let orig = usize::from(vtx.u16(sg_verts + local * 9 + 4)?);
                        source
                            .get(mesh_vertex_base + orig)
                            .copied()
                            .ok_or_else(|| "vtx vertex past vvd".to_owned())
                    };
                    if version >= 49 {
                        let count = vtx.usize(sg + 8)?;
                        for t in 0..count / 3 * 3 {
                            vertices.push(corner(t)?);
                        }
                        continue;
                    }
                    for s in 0..strips {
                        let strip = strip_base + s * 27;
                        let count = vtx.usize(strip)?;
                        let first = vtx.usize(strip + 4)?;
                        let flags = vtx.u8(strip + 18)?;
                        if flags & STRIP_IS_TRILIST != 0 {
                            for t in (0..count / 3 * 3).step_by(3) {
                                for c in 0..3 {
                                    vertices.push(corner(first + t + c)?);
                                }
                            }
                        } else {
                            for k in 0..count.saturating_sub(2) {
                                let (a, b) = if k % 2 == 0 { (k, k + 1) } else { (k + 1, k) };
                                vertices.push(corner(first + a)?);
                                vertices.push(corner(first + b)?);
                                vertices.push(corner(first + k + 2)?);
                            }
                        }
                    }
                }
                let vertex_count = vertices.len() - first_vertex;
                if vertex_count > 0 {
                    meshes.push(Mesh {
                        material,
                        first_vertex,
                        vertex_count,
                        body_part: p,
                        body_model,
                    });
                }
            }
        }
    }
    Ok((meshes, vertices))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quaternion48s_keeps_the_dropped_component_and_its_sign() {
        // Raw words from CS:GO's AK-47 idle: every component is ±0.5.
        let words: Vec<u8> = [0xed40u16, 0xed3f, 0x6d3f]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let q = quat48s(&R(&words), 0).unwrap();
        for c in q {
            assert!((c.abs() - 0.5).abs() < 1e-3, "{q:?}");
        }
        assert!((q.iter().map(|c| c * c).sum::<f32>() - 1.0).abs() < 1e-3);
    }
}
