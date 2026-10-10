//! Compiled models (`.vmdl_c`): the skeleton, the default mesh group's meshes with their
//! decoded vertex and index buffers and draw calls, and the attachments.
//!
//! A model's meshes are embedded: the `CTRL` block lists each mesh's data block (`MDAT`: draw
//! calls, attachments, its own bone list) and its vertex and index buffer blocks (`MVTX`,
//! `MIDX`), meshoptimizer and optionally zstd compressed.

use crate::kv3::Value;
use crate::{Resource, meshopt};

/// DXGI formats the mesh buffers use.
mod dxgi {
    pub const R32G32B32A32_FLOAT: u32 = 2;
    pub const R32G32B32_FLOAT: u32 = 6;
    pub const R16G16B16A16_UNORM: u32 = 11;
    pub const R16G16B16A16_UINT: u32 = 12;
    pub const R16G16B16A16_SINT: u32 = 14;
    pub const R32G32_FLOAT: u32 = 16;
    pub const R8G8B8A8_UNORM: u32 = 28;
    pub const R8G8B8A8_UINT: u32 = 30;
    pub const R16G16_FLOAT: u32 = 34;
    pub const R16G16_UNORM: u32 = 35;
    pub const R16G16_SNORM: u32 = 37;
    pub const R16G16_SINT: u32 = 38;
    pub const R32_UINT: u32 = 42;
    pub const R32G32B32A32_SINT: u32 = 4;
}

/// A bone of the model skeleton, relative to its parent.
#[derive(Clone, Debug, PartialEq)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    pub position: [f32; 3],
    /// Quaternion, `[x, y, z, w]`.
    pub rotation: [f32; 4],
}

/// A named point riding on a bone (`muzzle_flash`, `shell_eject`).
#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub bone: String,
    pub offset: [f32; 3],
    pub rotation: [f32; 4],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// Tangent and bitangent sign.
    pub tangent: [f32; 4],
    pub uv: [f32; 2],
    /// Model skeleton bones (remapped from the mesh's own list).
    pub bones: [u16; 4],
    pub weights: [f32; 4],
}

/// One draw: a material over a range of the mesh's indices.
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    pub material: String,
    pub start_index: u32,
    pub index_count: u32,
    pub base_vertex: i32,
    /// Which of the mesh's vertex buffers the draw reads.
    pub vertex_buffer: usize,
    /// Which of the mesh's index buffers.
    pub index_buffer: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub name: String,
    /// Decoded vertex buffers.
    pub vertex_buffers: Vec<Vec<Vertex>>,
    pub index_buffers: Vec<Vec<u32>>,
    pub draws: Vec<Draw>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub name: String,
    pub bones: Vec<Bone>,
    pub meshes: Vec<Mesh>,
    pub attachments: Vec<Attachment>,
    /// The animation skeleton the model's clips are made for (`.vnmskel`).
    pub skeleton: Option<String>,
    /// Per bone, the inverse of its bind pose (3×4 rows) from the meshes, when they give one.
    pub inverse_bind: Vec<Option<[f32; 12]>>,
}

impl Model {
    #[must_use]
    pub fn bone(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name)
    }

    #[must_use]
    pub fn attachment(&self, name: &str) -> Option<&Attachment> {
        self.attachments.iter().find(|a| a.name == name)
    }
}

fn vec3(value: Option<&Value>) -> [f32; 3] {
    let v = value.map_or_else(Vec::new, Value::as_f32s);
    [0, 1, 2].map(|i| v.get(i).copied().unwrap_or(0.0))
}

fn quat(value: Option<&Value>) -> [f32; 4] {
    let v = value.map_or_else(Vec::new, Value::as_f32s);
    if v.len() < 4 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    [v[0], v[1], v[2], v[3]]
}

fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1F);
    let mantissa = f32::from(bits & 0x3FF);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => f32::INFINITY,
        e => (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
    }
}

/// One vertex attribute of a buffer.
struct Field {
    semantic: String,
    index: i64,
    format: u32,
    offset: usize,
}

fn fields(buffer: &Value) -> Vec<Field> {
    buffer
        .get("m_inputLayoutFields")
        .map(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .map(|f| {
            let semantic = match f.get("m_pSemanticName") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Blob(b)) => {
                    String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or(&[])).into_owned()
                }
                _ => String::new(),
            };
            Field {
                semantic: semantic.to_ascii_uppercase(),
                index: f.int_of("m_nSemanticIndex"),
                format: f.int_of("m_Format") as u32,
                offset: f.int_of("m_nOffset").max(0) as usize,
            }
        })
        .collect()
}

/// The decoded bytes of a buffer described in `CTRL`.
fn buffer_bytes(
    resource: &Resource<'_>,
    buffer: &Value,
    vertex: bool,
) -> Result<(Vec<u8>, usize, usize), String> {
    let count = buffer.int_of("m_nElementCount").max(0) as usize;
    let stride = buffer.int_of("m_nElementSizeInBytes").max(0) as usize;
    let block = resource
        .blocks
        .get(buffer.int_of("m_nBlockIndex").max(0) as usize)
        .ok_or("vmdl: buffer block missing")?;
    let mut bytes = resource.bytes(block).to_vec();
    if buffer.get("m_bCompressedZSTD").and_then(Value::as_bool) == Some(true) {
        bytes = crate::kv3::zstd(&bytes, count * stride + 4096)?;
    }
    if buffer.get("m_bMeshoptCompressed").and_then(Value::as_bool) == Some(true) {
        if vertex {
            bytes = meshopt::decode_vertex_buffer(count, stride, &bytes)?;
        } else {
            let indices = meshopt::decode_index_buffer(count, &bytes)?;
            return Ok((
                indices.iter().flat_map(|i| i.to_le_bytes()).collect(),
                count,
                4,
            ));
        }
    }
    if bytes.len() < count * stride {
        return Err("vmdl: buffer shorter than its elements".to_owned());
    }
    Ok((bytes, count, stride))
}

/// CS2's packed normal and tangent frame (`R32_UINT`).
fn unpack_frame(packed: u32) -> ([f32; 3], [f32; 4]) {
    let sign_bit = packed & 1;
    let t_bits = ((packed >> 1) & 0x7FF) as f32;
    let x = (((packed >> 12) & 0x3FF) as f32 / 1023.0) * 2.0 - 1.0;
    let y = (((packed >> 22) & 0x3FF) as f32 / 1023.0) * 2.0 - 1.0;
    let z = 1.0 - x.abs() - y.abs();
    let compensation = (-z).clamp(0.0, 1.0);
    let nx = x + if x >= 0.0 {
        -compensation
    } else {
        compensation
    };
    let ny = y + if y >= 0.0 {
        -compensation
    } else {
        compensation
    };
    let length = (nx * nx + ny * ny + z * z).sqrt().max(1e-12);
    let n = [nx / length, ny / length, z / length];
    let tangent_sign = if n[2] >= 0.0 { 1.0 } else { -1.0 };
    let rcp = 1.0 / (tangent_sign + n[2]);
    let unaligned = [
        -tangent_sign * (n[0] * n[0]) * rcp + 1.0,
        -tangent_sign * ((n[0] * n[1]) * rcp),
        -tangent_sign * n[0],
    ];
    let angle = t_bits / 2047.0 * std::f32::consts::TAU;
    let cross = [
        n[1] * unaligned[2] - n[2] * unaligned[1],
        n[2] * unaligned[0] - n[0] * unaligned[2],
        n[0] * unaligned[1] - n[1] * unaligned[0],
    ];
    let (s, c) = angle.sin_cos();
    let t = [0, 1, 2].map(|i| unaligned[i] * c + cross[i] * s);
    (
        n,
        [t[0], t[1], t[2], if sign_bit == 0 { -1.0 } else { 1.0 }],
    )
}

/// The older packed normal (`R8G8B8A8_UNORM`, two bytes each for normal and tangent).
fn unpack_normal_v1(x: f32, y: f32) -> [f32; 3] {
    let (mut x, mut y) = (x - 128.0, y - 128.0);
    let z_sign_bit = if x < 0.0 { 1.0 } else { 0.0 };
    let t_sign_bit = if y < 0.0 { 1.0 } else { 0.0 };
    let z_sign = -(2.0 * z_sign_bit - 1.0);
    let t_sign = -(2.0 * t_sign_bit - 1.0);
    x = x * z_sign - z_sign_bit - 64.0;
    y = y * t_sign - t_sign_bit - 64.0;
    let x_sign_bit = if x < 0.0 { 1.0 } else { 0.0 };
    let y_sign_bit = if y < 0.0 { 1.0 } else { 0.0 };
    let x_sign = -(2.0 * x_sign_bit - 1.0);
    let y_sign = -(2.0 * y_sign_bit - 1.0);
    x = (x * x_sign - x_sign_bit) / 63.0;
    y = (y * y_sign - y_sign_bit) / 63.0;
    let z = 1.0 - x - y;
    let inv = 1.0 / (x * x + y * y + z * z).sqrt().max(1e-12);
    [x * inv * x_sign, y * inv * y_sign, z * inv * z_sign]
}

fn read_f32(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap_or([0; _]))
}

fn read_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap_or([0; _]))
}

fn decode_vertices(
    bytes: &[u8],
    count: usize,
    stride: usize,
    fields: &[Field],
    remap: &[i64],
) -> Vec<Vertex> {
    let mut out = vec![
        Vertex {
            tangent: [1.0, 0.0, 0.0, 1.0],
            weights: [1.0, 0.0, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            ..Vertex::default()
        };
        count
    ];
    let has_weights = fields.iter().any(|f| f.semantic.starts_with("BLENDWEIGHT"));
    for (i, vertex) in out.iter_mut().enumerate() {
        let base = i * stride;
        for f in fields {
            let at = base + f.offset;
            match (f.semantic.as_str(), f.format) {
                ("POSITION", dxgi::R32G32B32_FLOAT | dxgi::R32G32B32A32_FLOAT) => {
                    vertex.position = [
                        read_f32(bytes, at),
                        read_f32(bytes, at + 4),
                        read_f32(bytes, at + 8),
                    ];
                }
                ("NORMAL", dxgi::R32_UINT) => {
                    let packed = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or([0; _]));
                    (vertex.normal, vertex.tangent) = unpack_frame(packed);
                }
                ("NORMAL", dxgi::R8G8B8A8_UNORM) => {
                    let b = &bytes[at..at + 4];
                    vertex.normal = unpack_normal_v1(f32::from(b[0]), f32::from(b[1]));
                    let t = unpack_normal_v1(f32::from(b[2]), f32::from(b[3]));
                    vertex.tangent = [t[0], t[1], t[2], if b[3] < 128 { -1.0 } else { 1.0 }];
                }
                ("NORMAL", dxgi::R32G32B32_FLOAT) => {
                    vertex.normal = [
                        read_f32(bytes, at),
                        read_f32(bytes, at + 4),
                        read_f32(bytes, at + 8),
                    ];
                }
                ("TANGENT", dxgi::R32G32B32A32_FLOAT) => {
                    vertex.tangent = [0, 1, 2, 3].map(|k| read_f32(bytes, at + k * 4));
                }
                ("TEXCOORD", format) if f.index == 0 => {
                    vertex.uv = match format {
                        dxgi::R32G32_FLOAT => [read_f32(bytes, at), read_f32(bytes, at + 4)],
                        dxgi::R16G16_FLOAT => {
                            [half(read_u16(bytes, at)), half(read_u16(bytes, at + 2))]
                        }
                        dxgi::R16G16_UNORM => [
                            f32::from(read_u16(bytes, at)) / 65535.0,
                            f32::from(read_u16(bytes, at + 2)) / 65535.0,
                        ],
                        dxgi::R16G16_SNORM => [
                            f32::from(read_u16(bytes, at) as i16) / 32767.0,
                            f32::from(read_u16(bytes, at + 2) as i16) / 32767.0,
                        ],
                        _ => vertex.uv,
                    };
                }
                ("BLENDINDICES", format) => {
                    let raw: [u16; 4] = match format {
                        // Byte indices; the 16-bit unsigned form holds eight of them.
                        dxgi::R8G8B8A8_UINT | dxgi::R16G16B16A16_UINT => {
                            [0, 1, 2, 3].map(|k| u16::from(bytes[at + k]))
                        }
                        dxgi::R16G16_SINT => {
                            let a = read_u16(bytes, at);
                            let b = read_u16(bytes, at + 2);
                            [a, b, b, b]
                        }
                        dxgi::R16G16B16A16_SINT | dxgi::R32G32B32A32_SINT => {
                            [0, 1, 2, 3].map(|k| read_u16(bytes, at + k * 2))
                        }
                        _ => [0; 4],
                    };
                    vertex.bones =
                        raw.map(|b| remap.get(usize::from(b)).copied().unwrap_or(0).max(0) as u16);
                }
                ("BLENDWEIGHT" | "BLENDWEIGHTS", dxgi::R8G8B8A8_UNORM) => {
                    vertex.weights = [0, 1, 2, 3].map(|k| f32::from(bytes[at + k]) / 255.0);
                }
                ("BLENDWEIGHT" | "BLENDWEIGHTS", dxgi::R16G16B16A16_UNORM) => {
                    vertex.weights = [0, 1, 2, 3].map(|k| f32::from(bytes[at + k]) / 255.0);
                }
                ("BLENDWEIGHT" | "BLENDWEIGHTS", dxgi::R16G16_UNORM) => {
                    let a = f32::from(read_u16(bytes, at)) / 65535.0;
                    let b = f32::from(read_u16(bytes, at + 2)) / 65535.0;
                    vertex.weights = [a, b, 0.0, 0.0];
                }
                _ => {}
            }
        }
        if !has_weights {
            vertex.weights = [1.0, 0.0, 0.0, 0.0];
        }
        let total: f32 = vertex.weights.iter().sum();
        if total > 1e-6 {
            vertex.weights = vertex.weights.map(|w| w / total);
        } else {
            vertex.weights = [1.0, 0.0, 0.0, 0.0];
        }
    }
    out
}

fn parse_attachment(value: &Value) -> Option<Attachment> {
    let value = value.get("value").unwrap_or(value);
    let names = value
        .get("m_influenceNames")
        .map(Value::as_array)
        .unwrap_or(&[]);
    let bone = names
        .first()
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    Some(Attachment {
        name: value.str_of("m_name").to_owned(),
        bone,
        offset: vec3(
            value
                .get("m_vInfluenceOffsets")
                .and_then(|o| o.as_array().first()),
        ),
        rotation: quat(
            value
                .get("m_vInfluenceRotations")
                .and_then(|o| o.as_array().first()),
        ),
    })
}

/// Load a model's default mesh group.
pub fn load(bytes: &[u8]) -> Result<Model, String> {
    let resource = Resource::parse(bytes)?;
    let data = resource.data_kv3()?;
    let skeleton = data.get("m_modelSkeleton").ok_or("vmdl: no skeleton")?;
    let names = skeleton
        .get("m_boneName")
        .map(Value::as_array)
        .unwrap_or(&[]);
    let parents = skeleton
        .get("m_nParent")
        .map(Value::as_array)
        .unwrap_or(&[]);
    let positions = skeleton
        .get("m_bonePosParent")
        .map(Value::as_array)
        .unwrap_or(&[]);
    let rotations = skeleton
        .get("m_boneRotParent")
        .map(Value::as_array)
        .unwrap_or(&[]);
    let bones = names
        .iter()
        .enumerate()
        .map(|(i, name)| Bone {
            name: name.as_str().unwrap_or("").to_owned(),
            parent: parents
                .get(i)
                .and_then(Value::as_i64)
                .and_then(|p| usize::try_from(p).ok()),
            position: vec3(positions.get(i)),
            rotation: quat(rotations.get(i)),
        })
        .collect();
    let remap_table: Vec<i64> = data
        .get("m_remappingTable")
        .map(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(Value::as_i64)
        .collect();
    let remap_starts: Vec<usize> = data
        .get("m_remappingTableStarts")
        .map(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(Value::as_i64)
        .map(|v| v.max(0) as usize)
        .collect();
    let group_masks: Vec<u64> = data
        .get("m_refMeshGroupMasks")
        .map(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(Value::as_i64)
        .map(|v| v as u64)
        .collect();
    let default_mask = data.int_of("m_nDefaultMeshGroupMask") as u64;
    let skeleton_ref = resource
        .external_refs()
        .into_iter()
        .map(|(_, name)| name)
        .find(|name| name.ends_with(".vnmskel"));

    let mut model = Model {
        name: data.str_of("m_name").to_owned(),
        bones,
        skeleton: skeleton_ref,
        ..Model::default()
    };
    let ctrl = resource.kv3(b"CTRL")?;
    for embedded in ctrl
        .get("embedded_meshes")
        .map(Value::as_array)
        .unwrap_or(&[])
    {
        let mesh_index = embedded.int_of("m_nMeshIndex").max(0) as usize;
        let in_default = group_masks
            .get(mesh_index)
            .is_none_or(|mask| default_mask == 0 || mask & default_mask != 0);
        if !in_default {
            continue;
        }
        let block = resource
            .blocks
            .get(embedded.int_of("m_nDataBlock").max(0) as usize)
            .ok_or("vmdl: mesh data block missing")?;
        let mesh_data = crate::kv3::parse(resource.bytes(block))?;
        let remap_start = remap_starts.get(mesh_index).copied().unwrap_or(0);
        let remap = remap_table.get(remap_start..).unwrap_or(&[]);
        let mut mesh = Mesh {
            name: embedded.str_of("m_Name").to_owned(),
            ..Mesh::default()
        };
        for buffer in embedded
            .get("m_vertexBuffers")
            .map(Value::as_array)
            .unwrap_or(&[])
        {
            let (bytes, count, stride) = buffer_bytes(&resource, buffer, true)?;
            mesh.vertex_buffers.push(decode_vertices(
                &bytes,
                count,
                stride,
                &fields(buffer),
                remap,
            ));
        }
        for buffer in embedded
            .get("m_indexBuffers")
            .map(Value::as_array)
            .unwrap_or(&[])
        {
            let (bytes, count, stride) = buffer_bytes(&resource, buffer, false)?;
            let indices = (0..count)
                .map(|i| match stride {
                    2 => u32::from(read_u16(&bytes, i * 2)),
                    _ => u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap_or([0; _])),
                })
                .collect();
            mesh.index_buffers.push(indices);
        }
        for object in mesh_data
            .get("m_sceneObjects")
            .map(Value::as_array)
            .unwrap_or(&[])
        {
            for call in object
                .get("m_drawCalls")
                .map(Value::as_array)
                .unwrap_or(&[])
            {
                mesh.draws.push(Draw {
                    material: call.str_of("m_material").to_owned(),
                    start_index: call.int_of("m_nStartIndex").max(0) as u32,
                    index_count: call.int_of("m_nIndexCount").max(0) as u32,
                    base_vertex: call.int_of("m_nBaseVertex") as i32,
                    vertex_buffer: call
                        .path("m_vertexBuffers/0/m_hBuffer")
                        .and_then(Value::as_i64)
                        .unwrap_or(0)
                        .max(0) as usize,
                    index_buffer: call
                        .path("m_indexBuffer/m_hBuffer")
                        .and_then(Value::as_i64)
                        .unwrap_or(0)
                        .max(0) as usize,
                });
            }
        }
        for bone in mesh_data
            .path("m_skeleton/m_bones")
            .map(Value::as_array)
            .unwrap_or(&[])
        {
            let m = bone
                .get("m_invBindPose")
                .map(Value::as_f32s)
                .unwrap_or_else(Vec::new);
            if let (Some(i), Ok(m)) = (
                model.bone(bone.str_of("m_boneName")),
                <[f32; 12]>::try_from(m),
            ) {
                if model.inverse_bind.len() < model.bones.len() {
                    model.inverse_bind.resize(model.bones.len(), None);
                }
                model.inverse_bind[i].get_or_insert(m);
            }
        }
        for attachment in mesh_data
            .get("m_attachments")
            .map(Value::as_array)
            .unwrap_or(&[])
        {
            if let Some(a) = parse_attachment(attachment)
                && model.attachment(&a.name).is_none()
            {
                model.attachments.push(a);
            }
        }
        model.meshes.push(mesh);
    }
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats() {
        assert_eq!(half(0x3C00), 1.0);
        assert_eq!(half(0xC000), -2.0);
        assert_eq!(half(0x3800), 0.5);
        assert_eq!(half(0), 0.0);
    }

    #[test]
    fn packed_frame_normals_are_unit_length() {
        for packed in [0u32, 0x1234_5678, 0xFFFF_FFFF, 0x8000_0001] {
            let (n, t) = unpack_frame(packed);
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "{packed:#x}: {n:?}");
            let dot = n[0] * t[0] + n[1] * t[1] + n[2] * t[2];
            assert!(dot.abs() < 1e-3, "tangent not perpendicular: {dot}");
        }
    }
}
