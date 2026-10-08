//! GoldSrc studio models (`.mdl` version 10), the format of Half-Life and Counter-Strike 1.6.
//!
//! [`StudioModel::parse`] reads a model whose textures and animations live in the same file (the
//! case for every CS 1.6 `v_` viewmodel). Animations are decoded once into per-frame bone poses;
//! [`StudioModel::pose`] samples a sequence at any time and returns each bone's model-space
//! transform. Geometry is expanded from the format's strip/fan command lists into one triangle
//! list per mesh, every vertex carrying the bone it rides on (GoldSrc skinning is rigid: one bone
//! per vertex, one per normal), so a renderer can skin it with the [`StudioModel::pose`] matrices.
//!
//! The layouts follow the public `studio.h` structure definitions; the code is our own.

pub mod spr;

use std::fmt;

/// A row-major 3x4 transform: rotation in the first three columns, translation in the fourth.
pub type Mat3x4 = [[f32; 4]; 3];

pub const IDENTITY: Mat3x4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// Texture flag: palette index 255 is see-through (`STUDIO_NF_TRANSPARENT`).
pub const TEXTURE_MASKED: u16 = 0x0040;
/// Texture flag: drawn additively (`STUDIO_NF_ADDITIVE`).
pub const TEXTURE_ADDITIVE: u16 = 0x0020;
/// Texture flag: not lit (`STUDIO_NF_FULLBRIGHT`).
pub const TEXTURE_FULLBRIGHT: u16 = 0x0004;
/// Texture flag: environment-mapped (`STUDIO_NF_CHROME`).
pub const TEXTURE_CHROME: u16 = 0x0002;

const IDST: u32 = u32::from_le_bytes(*b"IDST");
const VERSION: i32 = 10;
const MAX_BONES: usize = 128;

const HEADER_LEN: usize = 244;
const BONE_LEN: usize = 112;
const SEQUENCE_LEN: usize = 176;
const BODYPART_LEN: usize = 76;
const MESH_LEN: usize = 20;
const TEXTURE_LEN: usize = 80;
/// Six `u16` channel offsets per bone (`mstudioanim_t`).
const ANIM_LEN: usize = 12;
const EVENT_LEN: usize = 76;

/// Event id GoldSrc clients play a sound for (`options` is the wave path under `sound/`).
pub const EVENT_CLIENT_SOUND: i32 = 5004;
/// Event id for the muzzle flash at attachment 0.
pub const EVENT_MUZZLE_FLASH: i32 = 5001;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MdlError {
    NotStudio,
    Version(i32),
    Truncated(&'static str),
    Bad(&'static str),
    /// Textures live in a separate `<name>T.mdl`, which this reader does not load.
    ExternalTextures,
}

impl fmt::Display for MdlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotStudio => write!(f, "not a studio model (no IDST header)"),
            Self::Version(v) => write!(f, "studio version {v}, expected {VERSION}"),
            Self::Truncated(what) => write!(f, "file ends inside {what}"),
            Self::Bad(what) => write!(f, "malformed {what}"),
            Self::ExternalTextures => write!(f, "textures are in a separate T.mdl"),
        }
    }
}

impl std::error::Error for MdlError {}

#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    /// Index of the parent bone, `None` for a root.
    pub parent: Option<usize>,
}

/// One bone's local transform for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneFrame {
    pub position: [f32; 3],
    /// Unit quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
}

/// Something a sequence asks for at a frame: a sound, a muzzle flash, a shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub frame: usize,
    pub event: i32,
    pub options: String,
}

#[derive(Clone, Debug)]
pub struct Sequence {
    pub label: String,
    pub fps: f32,
    pub looping: bool,
    pub num_frames: usize,
    pub events: Vec<Event>,
    /// `[frame][bone]` local transforms.
    frames: Vec<BoneFrame>,
}

impl Sequence {
    /// Seconds the sequence takes to play once.
    #[must_use]
    pub fn duration(&self) -> f32 {
        if self.fps > 0.0 && self.num_frames > 1 {
            (self.num_frames - 1) as f32 / self.fps
        } else {
            0.0
        }
    }
}

#[derive(Clone, Debug)]
pub struct Texture {
    pub name: String,
    pub flags: u16,
    pub width: u32,
    pub height: u32,
    /// `width * height` RGBA8 texels (sRGB); masked textures have alpha 0 at palette index 255.
    pub rgba: Vec<u8>,
}

/// One vertex of the expanded triangle list, in the model's bind space (not yet posed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// Texture coordinates, 0..1.
    pub uv: [f32; 2],
    pub bone: u8,
    pub normal_bone: u8,
}

/// A run of triangles drawn with one texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mesh {
    pub texture: usize,
    /// Range in [`StudioModel::vertices`]; three vertices per triangle.
    pub first_vertex: usize,
    pub vertex_count: usize,
}

#[derive(Clone, Debug)]
pub struct StudioModel {
    pub name: String,
    pub bones: Vec<Bone>,
    pub sequences: Vec<Sequence>,
    pub textures: Vec<Texture>,
    pub meshes: Vec<Mesh>,
    pub vertices: Vec<Vertex>,
}

struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    fn slice(&self, at: usize, len: usize, what: &'static str) -> Result<&'a [u8], MdlError> {
        at.checked_add(len)
            .and_then(|end| self.data.get(at..end))
            .ok_or(MdlError::Truncated(what))
    }

    fn i32(&self, at: usize, what: &'static str) -> Result<i32, MdlError> {
        let b = self.slice(at, 4, what)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u32(&self, at: usize, what: &'static str) -> Result<u32, MdlError> {
        let b = self.slice(at, 4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn count(&self, at: usize, what: &'static str) -> Result<usize, MdlError> {
        usize::try_from(self.i32(at, what)?).map_err(|_| MdlError::Bad(what))
    }

    fn f32(&self, at: usize, what: &'static str) -> Result<f32, MdlError> {
        Ok(f32::from_bits(self.u32(at, what)?))
    }

    fn vec3(&self, at: usize, what: &'static str) -> Result<[f32; 3], MdlError> {
        Ok([
            self.f32(at, what)?,
            self.f32(at + 4, what)?,
            self.f32(at + 8, what)?,
        ])
    }

    fn u16(&self, at: usize, what: &'static str) -> Result<u16, MdlError> {
        let b = self.slice(at, 2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn i16(&self, at: usize, what: &'static str) -> Result<i16, MdlError> {
        let b = self.slice(at, 2, what)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    fn u8(&self, at: usize, what: &'static str) -> Result<u8, MdlError> {
        self.data.get(at).copied().ok_or(MdlError::Truncated(what))
    }

    fn name(&self, at: usize, len: usize, what: &'static str) -> Result<String, MdlError> {
        let bytes = self.slice(at, len, what)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(len);
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}

/// Per-bone defaults the animation channels are relative to.
struct BoneChannels {
    value: [f32; 6],
    scale: [f32; 6],
}

impl StudioModel {
    pub fn parse(data: &[u8]) -> Result<Self, MdlError> {
        let r = Reader { data };
        if data.len() < HEADER_LEN || r.u32(0, "header")? != IDST {
            return Err(MdlError::NotStudio);
        }
        let version = r.i32(4, "header")?;
        if version != VERSION {
            return Err(MdlError::Version(version));
        }
        let name = r.name(8, 64, "header")?;

        let (bones, channels) = parse_bones(&r)?;
        let sequences = parse_sequences(&r, &channels)?;
        let textures = parse_textures(&r)?;
        let skins = parse_skins(&r)?;
        let (meshes, vertices) = parse_geometry(&r, &skins, &textures, bones.len())?;
        Ok(Self {
            name,
            bones,
            sequences,
            textures,
            meshes,
            vertices,
        })
    }

    #[must_use]
    pub fn sequence_index(&self, label: &str) -> Option<usize> {
        self.sequences
            .iter()
            .position(|s| s.label.eq_ignore_ascii_case(label))
    }

    /// Model-space bone transforms for `sequence` at `seconds` into it. Looping sequences wrap,
    /// others hold their last frame. Writes one matrix per bone into `out`.
    pub fn pose(&self, sequence: usize, seconds: f32, out: &mut Vec<Mat3x4>) {
        out.clear();
        let bones = self.bones.len();
        let Some(seq) = self.sequences.get(sequence).filter(|s| s.num_frames > 0) else {
            out.resize(bones, IDENTITY);
            return;
        };
        let last = seq.num_frames - 1;
        let mut frame = (seconds.max(0.0) * seq.fps).max(0.0);
        if seq.looping && last > 0 {
            frame %= last as f32;
        } else {
            frame = frame.min(last as f32);
        }
        let first = (frame as usize).min(last);
        let next = if first < last {
            first + 1
        } else if seq.looping {
            0
        } else {
            last
        };
        let t = frame - first as f32;
        for bone in 0..bones {
            let a = seq.frames[first * bones + bone];
            let b = seq.frames[next * bones + bone];
            let rotation = slerp(a.rotation, b.rotation, t);
            let position = core::array::from_fn(|i| a.position[i] + (b.position[i] - a.position[i]) * t);
            let local = matrix(rotation, position);
            let world = match self.bones[bone].parent {
                Some(parent) if parent < out.len() => concat(&out[parent], &local),
                _ => local,
            };
            out.push(world);
        }
    }
}

fn parse_bones(r: &Reader<'_>) -> Result<(Vec<Bone>, Vec<BoneChannels>), MdlError> {
    let count = r.count(140, "header")?;
    let base = r.count(144, "header")?;
    if count == 0 || count > MAX_BONES {
        return Err(MdlError::Bad("bone count"));
    }
    let mut bones = Vec::with_capacity(count);
    let mut channels = Vec::with_capacity(count);
    for i in 0..count {
        let at = base + i * BONE_LEN;
        let parent = r.i32(at + 32, "bone")?;
        let parent = usize::try_from(parent).ok();
        if parent.is_some_and(|p| p >= i) {
            return Err(MdlError::Bad("bone parent order"));
        }
        bones.push(Bone {
            name: r.name(at, 32, "bone")?,
            parent,
        });
        let mut value = [0.0; 6];
        let mut scale = [0.0; 6];
        for c in 0..6 {
            value[c] = r.f32(at + 64 + c * 4, "bone")?;
            scale[c] = r.f32(at + 88 + c * 4, "bone")?;
        }
        channels.push(BoneChannels { value, scale });
    }
    Ok((bones, channels))
}

fn parse_sequences(r: &Reader<'_>, channels: &[BoneChannels]) -> Result<Vec<Sequence>, MdlError> {
    let count = r.count(164, "header")?;
    let base = r.count(168, "header")?;
    let mut sequences = Vec::with_capacity(count);
    for i in 0..count {
        let at = base + i * SEQUENCE_LEN;
        let label = r.name(at, 32, "sequence")?;
        let fps = r.f32(at + 32, "sequence")?;
        let flags = r.i32(at + 36, "sequence")?;
        let num_frames = r.count(at + 56, "sequence")?;
        let anim_index = r.count(at + 124, "sequence")?;
        let event_count = r.count(at + 48, "sequence")?;
        let event_at = r.count(at + 52, "sequence")?;
        let events = (0..event_count)
            .map(|e| {
                let ev = event_at + e * EVENT_LEN;
                Ok(Event {
                    frame: r.count(ev, "event")?,
                    event: r.i32(ev + 4, "event")?,
                    options: r.name(ev + 12, 64, "event")?,
                })
            })
            .collect::<Result<Vec<_>, MdlError>>()?;
        let group = r.i32(at + 156, "sequence")?;
        // Sequences kept in demand-loaded group files (`<name>01.mdl`) are skipped: viewmodels
        // keep everything in group 0.
        let frames = if group == 0 {
            decode_frames(r, anim_index, num_frames, channels)?
        } else {
            Vec::new()
        };
        let num_frames = if frames.is_empty() { 0 } else { num_frames };
        sequences.push(Sequence {
            label,
            fps,
            looping: flags & 1 != 0,
            num_frames,
            events,
            frames,
        });
    }
    Ok(sequences)
}

/// Decode every frame of one sequence (first blend) into `[frame][bone]` local transforms.
fn decode_frames(
    r: &Reader<'_>,
    anim_index: usize,
    num_frames: usize,
    channels: &[BoneChannels],
) -> Result<Vec<BoneFrame>, MdlError> {
    let bones = channels.len();
    let mut frames = Vec::with_capacity(num_frames * bones);
    for frame in 0..num_frames {
        for (bone, chan) in channels.iter().enumerate() {
            let anim = anim_index + bone * ANIM_LEN;
            let mut v = [0.0_f32; 6];
            for c in 0..6 {
                let offset = usize::from(r.u16(anim + c * 2, "animation")?);
                v[c] = chan.value[c];
                if offset != 0 {
                    let raw = anim_value(r, anim + offset, frame)?;
                    v[c] += f32::from(raw) * chan.scale[c];
                }
            }
            frames.push(BoneFrame {
                position: [v[0], v[1], v[2]],
                rotation: angle_quaternion([v[3], v[4], v[5]]),
            });
        }
    }
    Ok(frames)
}

/// Value of a run-length encoded channel at `frame`. Each run header holds how many explicit
/// values follow (`valid`) and how many frames the run covers (`total`); frames past the
/// explicit values repeat the last one.
fn anim_value(r: &Reader<'_>, start: usize, frame: usize) -> Result<i16, MdlError> {
    let mut at = start;
    let mut k = frame;
    loop {
        let valid = usize::from(r.u8(at, "animation run")?);
        let total = usize::from(r.u8(at + 1, "animation run")?);
        if total == 0 {
            return Err(MdlError::Bad("animation run"));
        }
        if total > k {
            let index = if valid > k { k + 1 } else { valid };
            return r.i16(at + index * 2, "animation value");
        }
        k -= total;
        at += (valid + 1) * 2;
    }
}

fn parse_textures(r: &Reader<'_>) -> Result<Vec<Texture>, MdlError> {
    let count = r.count(180, "header")?;
    let base = r.count(184, "header")?;
    if count == 0 {
        return Err(MdlError::ExternalTextures);
    }
    let mut textures = Vec::with_capacity(count);
    for i in 0..count {
        let at = base + i * TEXTURE_LEN;
        let name = r.name(at, 64, "texture")?;
        let flags = r.u16(at + 64, "texture")?;
        let width = r.count(at + 68, "texture")?;
        let height = r.count(at + 72, "texture")?;
        let pixels_at = r.count(at + 76, "texture")?;
        if width == 0 || height == 0 || width > 4096 || height > 4096 {
            return Err(MdlError::Bad("texture size"));
        }
        let pixels = r.slice(pixels_at, width * height, "texture pixels")?;
        let palette = r.slice(pixels_at + width * height, 768, "texture palette")?;
        let masked = flags & TEXTURE_MASKED != 0;
        let mut rgba = Vec::with_capacity(width * height * 4);
        for &index in pixels {
            let p = usize::from(index) * 3;
            rgba.extend_from_slice(&palette[p..p + 3]);
            rgba.push(if masked && index == 255 { 0 } else { 255 });
        }
        textures.push(Texture {
            name,
            flags,
            width: width as u32,
            height: height as u32,
            rgba,
        });
    }
    Ok(textures)
}

/// Skin family 0: mesh skin reference → texture index.
fn parse_skins(r: &Reader<'_>) -> Result<Vec<usize>, MdlError> {
    let refs = r.count(192, "header")?;
    let base = r.count(200, "header")?;
    (0..refs)
        .map(|i| {
            let index = r.i16(base + i * 2, "skin table")?;
            usize::try_from(index).map_err(|_| MdlError::Bad("skin table"))
        })
        .collect()
}

/// Expand every body part's first submodel into triangle lists, one [`Mesh`] per source mesh.
fn parse_geometry(
    r: &Reader<'_>,
    skins: &[usize],
    textures: &[Texture],
    bone_count: usize,
) -> Result<(Vec<Mesh>, Vec<Vertex>), MdlError> {
    let parts = r.count(204, "header")?;
    let parts_at = r.count(208, "header")?;
    let mut meshes = Vec::new();
    let mut vertices = Vec::new();
    for part in 0..parts {
        let at = parts_at + part * BODYPART_LEN;
        if r.count(at + 64, "body part")? == 0 {
            continue;
        }
        let model_at = r.count(at + 72, "body part")?;
        let mesh_count = r.count(model_at + 72, "submodel")?;
        let mesh_at = r.count(model_at + 76, "submodel")?;
        let vert_count = r.count(model_at + 80, "submodel")?;
        let vert_bones = r.count(model_at + 84, "submodel")?;
        let verts_at = r.count(model_at + 88, "submodel")?;
        let norm_count = r.count(model_at + 92, "submodel")?;
        let norm_bones = r.count(model_at + 96, "submodel")?;
        let norms_at = r.count(model_at + 100, "submodel")?;
        for m in 0..mesh_count {
            let mesh = mesh_at + m * MESH_LEN;
            let commands_at = r.count(mesh + 4, "mesh")?;
            let skin_ref = r.count(mesh + 8, "mesh")?;
            let texture = *skins.get(skin_ref).ok_or(MdlError::Bad("mesh skin"))?;
            let tex = textures.get(texture).ok_or(MdlError::Bad("mesh texture"))?;
            let first_vertex = vertices.len();
            let corner = |at: usize| -> Result<Vertex, MdlError> {
                let v = usize::try_from(r.i16(at, "triangle")?).map_err(|_| MdlError::Bad("vertex"))?;
                let n = usize::try_from(r.i16(at + 2, "triangle")?).map_err(|_| MdlError::Bad("normal"))?;
                if v >= vert_count || n >= norm_count {
                    return Err(MdlError::Bad("triangle index"));
                }
                let bone = r.u8(vert_bones + v, "vertex bone")?;
                let normal_bone = r.u8(norm_bones + n, "normal bone")?;
                if usize::from(bone) >= bone_count || usize::from(normal_bone) >= bone_count {
                    return Err(MdlError::Bad("vertex bone"));
                }
                let s = f32::from(r.i16(at + 4, "triangle")?);
                let t = f32::from(r.i16(at + 6, "triangle")?);
                Ok(Vertex {
                    position: r.vec3(verts_at + v * 12, "vertex")?,
                    normal: r.vec3(norms_at + n * 12, "normal")?,
                    uv: [s / tex.width as f32, t / tex.height as f32],
                    bone,
                    normal_bone,
                })
            };
            let mut at = commands_at;
            loop {
                let command = r.i16(at, "triangle commands")?;
                at += 2;
                if command == 0 {
                    break;
                }
                let fan = command < 0;
                let n = usize::from(command.unsigned_abs());
                let mut run = Vec::with_capacity(n);
                for _ in 0..n {
                    run.push(corner(at)?);
                    at += 8;
                }
                for k in 0..n.saturating_sub(2) {
                    let tri = if fan {
                        [run[0], run[k + 1], run[k + 2]]
                    } else if k % 2 == 0 {
                        [run[k], run[k + 1], run[k + 2]]
                    } else {
                        [run[k + 1], run[k], run[k + 2]]
                    };
                    vertices.extend_from_slice(&tri);
                }
            }
            let vertex_count = vertices.len() - first_vertex;
            if vertex_count > 0 {
                meshes.push(Mesh {
                    texture,
                    first_vertex,
                    vertex_count,
                });
            }
        }
    }
    Ok((meshes, vertices))
}

/// Euler angles (radians: roll about X, pitch about Y, yaw about Z) to a quaternion, in the
/// GoldSrc `AngleQuaternion` convention.
#[must_use]
pub fn angle_quaternion(angles: [f32; 3]) -> [f32; 4] {
    let (sy, cy) = (angles[2] * 0.5).sin_cos();
    let (sp, cp) = (angles[1] * 0.5).sin_cos();
    let (sr, cr) = (angles[0] * 0.5).sin_cos();
    [
        sr * cp * cy - cr * sp * sy,
        cr * sp * cy + sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
        cr * cp * cy + sr * sp * sy,
    ]
}

/// Spherical interpolation along the shorter arc.
#[must_use]
pub fn slerp(p: [f32; 4], q: [f32; 4], t: f32) -> [f32; 4] {
    let mut q = q;
    let mut cos = p[0] * q[0] + p[1] * q[1] + p[2] * q[2] + p[3] * q[3];
    if cos < 0.0 {
        q = q.map(|v| -v);
        cos = -cos;
    }
    let (a, b) = if 1.0 - cos > 0.001 {
        let omega = cos.acos();
        let sin = omega.sin();
        (((1.0 - t) * omega).sin() / sin, (t * omega).sin() / sin)
    } else {
        (1.0 - t, t)
    };
    let mut out: [f32; 4] = core::array::from_fn(|i| a * p[i] + b * q[i]);
    let len = (out.iter().map(|v| v * v).sum::<f32>()).sqrt();
    if len > 0.0 {
        out = out.map(|v| v / len);
    }
    out
}

fn matrix(q: [f32; 4], position: [f32; 3]) -> Mat3x4 {
    let [x, y, z, w] = q;
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
            position[0],
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
            position[1],
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
            position[2],
        ],
    ]
}

/// `a * b` for affine 3x4 transforms.
#[must_use]
pub fn concat(a: &Mat3x4, b: &Mat3x4) -> Mat3x4 {
    core::array::from_fn(|row| {
        core::array::from_fn(|col| {
            let mut v = a[row][0] * b[0][col] + a[row][1] * b[1][col] + a[row][2] * b[2][col];
            if col == 3 {
                v += a[row][3];
            }
            v
        })
    })
}

/// Apply a pose matrix to a point.
#[must_use]
pub fn transform_point(m: &Mat3x4, p: [f32; 3]) -> [f32; 3] {
    core::array::from_fn(|row| m[row][0] * p[0] + m[row][1] * p[1] + m[row][2] * p[2] + m[row][3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quaternion_of_a_quarter_yaw_turns_x_into_y() {
        let q = angle_quaternion([0.0, 0.0, core::f32::consts::FRAC_PI_2]);
        let p = transform_point(&matrix(q, [0.0; 3]), [1.0, 0.0, 0.0]);
        assert!((p[0]).abs() < 1e-5 && (p[1] - 1.0).abs() < 1e-5, "{p:?}");
    }

    #[test]
    fn slerp_halfway_is_half_the_angle() {
        let a = angle_quaternion([0.0, 0.0, 0.0]);
        let b = angle_quaternion([0.0, 0.0, 1.0]);
        let mid = slerp(a, b, 0.5);
        let expect = angle_quaternion([0.0, 0.0, 0.5]);
        for i in 0..4 {
            assert!((mid[i] - expect[i]).abs() < 1e-5);
        }
    }

    #[test]
    fn run_length_values_repeat_the_last_explicit_one() {
        // One run: 2 explicit values covering 5 frames.
        let mut data = vec![2u8, 5u8];
        data.extend_from_slice(&7i16.to_le_bytes());
        data.extend_from_slice(&9i16.to_le_bytes());
        // Second run: 1 value for 3 frames.
        data.extend_from_slice(&[1u8, 3u8]);
        data.extend_from_slice(&(-4i16).to_le_bytes());
        let r = Reader { data: &data };
        let values: Vec<i16> = (0..8).map(|f| anim_value(&r, 0, f).unwrap()).collect();
        assert_eq!(values, [7, 9, 9, 9, 9, -4, -4, -4]);
    }

    /// Parses the CS 1.6 viewmodels from a local install when `IW4L_CSTRIKE` names its
    /// `cstrike` folder; skipped otherwise.
    #[test]
    fn retail_viewmodels_parse_when_installed() {
        let Some(dir) = std::env::var_os("IW4L_CSTRIKE") else {
            return;
        };
        for name in ["v_ak47", "v_m4a1", "v_awp", "v_deagle", "v_usp", "v_glock18", "v_knife"] {
            let path = std::path::Path::new(&dir).join("models").join(format!("{name}.mdl"));
            let bytes = std::fs::read(&path).expect("read model");
            let model = StudioModel::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!model.meshes.is_empty() && !model.sequences.is_empty(), "{name}");
            let mut pose = Vec::new();
            for (i, seq) in model.sequences.iter().enumerate() {
                model.pose(i, seq.duration() * 0.5, &mut pose);
                assert_eq!(pose.len(), model.bones.len());
                assert!(pose.iter().flatten().flatten().all(|v| v.is_finite()), "{name} {}", seq.label);
            }
            let labels: Vec<_> = model
                .sequences
                .iter()
                .map(|s| format!("{}({}f@{})", s.label, s.num_frames, s.fps))
                .collect();
            let textures: Vec<_> = model
                .textures
                .iter()
                .map(|t| format!("{}[{}x{} {:#x}]", t.name, t.width, t.height, t.flags))
                .collect();
            println!("{name} textures: {}", textures.join(" "));
            for seq in &model.sequences {
                for ev in &seq.events {
                    println!("{name} {} frame {} event {} {}", seq.label, ev.frame, ev.event, ev.options);
                }
            }
            println!(
                "{name}: {} bones, {} meshes, {} tris, {} textures; {}",
                model.bones.len(),
                model.meshes.len(),
                model.vertices.len() / 3,
                model.textures.len(),
                labels.join(" ")
            );
        }
    }
}
