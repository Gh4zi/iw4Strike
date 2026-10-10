//! Source studio models (`.mdl` v44-48 with its `.vvd` vertices and `.vtx` strips), as
//! Counter-Strike: Source ships its viewmodels. Layouts follow the public SDK headers; the code
//! is our own. Animations are decoded once into per-frame bone poses; geometry is expanded into
//! one triangle list per mesh with up to three weighted bones per vertex (LOD 0).

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

/// Event id GoldSrc/Source clients play a sound script entry for.
pub const EVENT_CLIENT_SOUND: i32 = 5004;

#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    /// Model (bind) space to this bone's space.
    pub pose_to_bone: Mat3x4,
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

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// Position in the sequence, 0..1.
    pub cycle: f32,
    pub event: i32,
    pub options: String,
}

#[derive(Clone, Debug)]
pub struct Sequence {
    pub label: String,
    /// `ACT_VM_…` activity name, empty when none.
    pub activity: String,
    pub fps: f32,
    pub looping: bool,
    pub num_frames: usize,
    pub events: Vec<Event>,
    /// The animations the sequence blends between (`StudioModel::animations` indices), a
    /// `blend_size.0` × `blend_size.1` grid row by row (an aim matrix is 3 × 3); one for most.
    pub blends: Vec<usize>,
    pub blend_size: (usize, usize),
    frames: Vec<BoneFrame>,
}

/// One decoded animation (`mstudioanimdesc_t`): `[frame][bone]` local transforms. A delta
/// animation's transforms are offsets to compose onto another pose (identity where it leaves a
/// bone alone).
#[derive(Clone, Debug)]
pub struct Animation {
    pub fps: f32,
    pub num_frames: usize,
    pub delta: bool,
    frames: Vec<BoneFrame>,
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
        let r = R(mdl);
        if r.bytes(0, 4)? != b"IDST" {
            return Err("not a studio model".into());
        }
        let version = r.i32(4)?;
        if !(44..=48).contains(&version) {
            return Err(format!("studio version {version} unsupported"));
        }
        let name = r.cstr(12)?;
        let (bones, defaults) = parse_bones(&r)?;
        let (sequences, animations) = parse_sequences(&r, &defaults)?;
        let (materials, material_dirs, skin) = parse_materials(&r)?;
        // An animation-only model (`cs_player_shared.mdl`) comes without its geometry files.
        let (meshes, vertices) = if vvd.is_empty() && vtx.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            parse_geometry(&r, &R(vvd), &R(vtx), &skin)?
        };
        let attachments = parse_attachments(&r, &bones).unwrap_or_default();
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
        })
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
        out.clear();
        let bones = self.bones.len();
        let mut world: Vec<Mat3x4> = Vec::with_capacity(bones);
        let seq = self.sequences.get(sequence).filter(|s| s.num_frames > 0);
        for (b, bone) in self.bones.iter().enumerate() {
            let local = match seq {
                Some(seq) => {
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
                    let a = seq.frames[first * bones + b];
                    let c = seq.frames[next * bones + b];
                    matrix(
                        slerp(a.rotation, c.rotation, t),
                        core::array::from_fn(|i| {
                            a.position[i] + (c.position[i] - a.position[i]) * t
                        }),
                    )
                }
                None => mdl_goldsrc::IDENTITY,
            };
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
    }
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

/// `mstudioattachment_t`: name offset, flags, bone, a 3x4 local matrix, 8 unused ints.
const ATTACHMENT_LEN: usize = 92;

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

/// Decode one animation into `[frame][bone]` local transforms.
fn decode_animation(r: &R<'_>, desc: usize, defaults: &[BoneDefaults]) -> Result<Animation, String> {
    let fps = r.f32(desc + 8)?;
    // `STUDIO_DELTA`: offsets to another pose; bones it leaves alone stay put.
    let delta_anim = r.i32(desc + 12)? & 0x4 != 0;
    let num_frames = r.usize(desc + 16)?.max(1);
    let anim_block = r.i32(desc + 52)?;
    let anim_index = r.i32(desc + 56)?;
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
    let mut frames = Vec::with_capacity(num_frames * bones);
    if anim_block != 0 || anim_index == 0 {
        // Data in a separate .ani file: hold the rest pose.
        for _ in 0..num_frames {
            frames.extend_from_slice(&rest);
        }
        return Ok(Animation {
            fps,
            num_frames,
            delta: delta_anim,
            frames,
        });
    }
    let first = r.rel(desc, 56)?;
    for frame in 0..num_frames {
        let mut pose = rest.clone();
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
        frames.extend_from_slice(&pose);
    }
    Ok(Animation {
        fps,
        num_frames,
        delta: delta_anim,
        frames,
    })
}

fn parse_sequences(
    r: &R<'_>,
    defaults: &[BoneDefaults],
) -> Result<(Vec<Sequence>, Vec<Animation>), String> {
    let anim_count = r.usize(180)?;
    let anim_base = r.usize(184)?;
    let mut animations = Vec::with_capacity(anim_count);
    for a in 0..anim_count {
        animations.push(decode_animation(r, anim_base + a * ANIMDESC_LEN, defaults)?);
    }
    let count = r.usize(188)?;
    let base = r.usize(192)?;
    let mut sequences = Vec::with_capacity(count);
    for s in 0..count {
        let at = base + s * SEQ_LEN;
        let label = r.cstr(r.rel(at, 4)?)?;
        let activity = r.cstr(r.rel(at, 8)?)?;
        let flags = r.i32(at + 12)?;
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
            fps,
            looping: flags & 1 != 0,
            num_frames,
            events,
            blends,
            blend_size,
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
) -> Result<(Vec<Mesh>, Vec<Vertex>), String> {
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
                    let sg = group_base + g * 25;
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
