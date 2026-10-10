//! Binary KeyValues3, the tree most Source 2 blocks hold (versions 1 to 5).
//!
//! A KV3 block is a header, then one buffer (two from version 5) of typed lanes: one-, two-,
//! four- and eight-byte values each in their own run, the strings, and a stream of type bytes.
//! The tree is read by walking the type stream and taking each value from its lane. Buffers may
//! be LZ4 or zstd compressed; binary blobs (mesh data in older files) follow the buffers.

use std::fmt::Write as _;

/// A KeyValues3 value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Double(f64),
    String(String),
    Blob(Vec<u8>),
    Array(Vec<Value>),
    /// Members in file order.
    Object(Vec<(String, Value)>),
}

impl Value {
    /// A member of an object.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Follow a `/`-separated path of members and array indices (`m_meshes/0/m_name`).
    #[must_use]
    pub fn path(&self, path: &str) -> Option<&Value> {
        path.split('/')
            .filter(|p| !p.is_empty())
            .try_fold(self, |value, part| match value {
                Self::Array(items) => part.parse::<usize>().ok().and_then(|i| items.get(i)),
                _ => value.get(part),
            })
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Self::Int(v) => Some(v),
            Self::UInt(v) => i64::try_from(v).ok(),
            Self::Double(v) => Some(v as i64),
            Self::Bool(v) => Some(i64::from(v)),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Self::Int(v) => Some(v as f64),
            Self::UInt(v) => Some(v as f64),
            Self::Double(v) => Some(v),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_f32(&self) -> Option<f32> {
        self.as_f64().map(|v| v as f32)
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Self::Bool(v) => Some(v),
            Self::Int(v) => Some(v != 0),
            Self::UInt(v) => Some(v != 0),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_array(&self) -> &[Value] {
        match self {
            Self::Array(items) => items,
            _ => &[],
        }
    }

    #[must_use]
    pub fn as_blob(&self) -> Option<&[u8]> {
        match self {
            Self::Blob(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// Members of an object (empty for anything else).
    #[must_use]
    pub fn members(&self) -> &[(String, Value)] {
        match self {
            Self::Object(members) => members,
            _ => &[],
        }
    }

    /// A numeric array as `f32`s (`[x, y, z]` vectors, quaternions, matrices).
    #[must_use]
    pub fn as_f32s(&self) -> Vec<f32> {
        self.as_array().iter().filter_map(Self::as_f32).collect()
    }

    /// A member as a string, `""` when missing.
    #[must_use]
    pub fn str_of(&self, key: &str) -> &str {
        self.get(key).and_then(Self::as_str).unwrap_or("")
    }

    /// A member as an integer, 0 when missing.
    #[must_use]
    pub fn int_of(&self, key: &str) -> i64 {
        self.get(key).and_then(Self::as_i64).unwrap_or(0)
    }

    /// A member as a float, 0 when missing.
    #[must_use]
    pub fn float_of(&self, key: &str) -> f32 {
        self.get(key).and_then(Self::as_f32).unwrap_or(0.0)
    }

    /// A text dump for debugging, `depth` levels deep.
    #[must_use]
    pub fn dump(&self, depth: usize) -> String {
        let mut out = String::new();
        self.dump_into(&mut out, 0, depth);
        out
    }

    fn dump_into(&self, out: &mut String, indent: usize, depth: usize) {
        let pad = "  ".repeat(indent);
        match self {
            Self::Object(members) if depth > 0 => {
                out.push_str("{\n");
                for (k, v) in members {
                    let _ = write!(out, "{pad}  {k} = ");
                    v.dump_into(out, indent + 1, depth - 1);
                    out.push('\n');
                }
                let _ = write!(out, "{pad}}}");
            }
            Self::Array(items) if depth > 0 && items.len() <= 128 => {
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    v.dump_into(out, indent + 1, depth - 1);
                }
                out.push(']');
            }
            Self::Array(items) => {
                let _ = write!(out, "[{} items]", items.len());
            }
            Self::Object(members) => {
                let _ = write!(out, "{{{} members}}", members.len());
            }
            Self::Blob(bytes) => {
                let _ = write!(out, "<blob {} bytes>", bytes.len());
            }
            Self::String(s) => {
                let _ = write!(out, "{s:?}");
            }
            other => {
                let _ = write!(out, "{other:?}");
            }
        }
    }
}

const MAGIC_LEGACY: u32 = 0x0356_4B56; // "VKV\x03"
const TRAILER: u32 = 0xFFEE_DD00;

/// Lanes of one buffer.
#[derive(Clone, Copy, Default)]
struct Lanes<'b> {
    bytes1: &'b [u8],
    bytes2: &'b [u8],
    bytes4: &'b [u8],
    bytes8: &'b [u8],
}

fn take<'b>(lane: &mut &'b [u8], n: usize) -> Result<&'b [u8], String> {
    if lane.len() < n {
        return Err("kv3: lane overrun".to_owned());
    }
    let (head, rest) = lane.split_at(n);
    *lane = rest;
    Ok(head)
}

fn take_i32(lane: &mut &[u8]) -> Result<i32, String> {
    Ok(i32::from_le_bytes(
        take(lane, 4)?.try_into().unwrap_or([0; _]),
    ))
}

struct Context<'b> {
    version: u32,
    types: &'b [u8],
    object_lengths: &'b [u8],
    blobs: &'b [u8],
    blob_lengths: &'b [u8],
    strings: Vec<String>,
    buffer: Lanes<'b>,
    auxiliary: Lanes<'b>,
}

/// Node types in the type stream.
mod node {
    pub const NULL: u8 = 1;
    pub const BOOLEAN: u8 = 2;
    pub const INT64: u8 = 3;
    pub const UINT64: u8 = 4;
    pub const DOUBLE: u8 = 5;
    pub const STRING: u8 = 6;
    pub const BINARY_BLOB: u8 = 7;
    pub const ARRAY: u8 = 8;
    pub const OBJECT: u8 = 9;
    pub const ARRAY_TYPED: u8 = 10;
    pub const INT32: u8 = 11;
    pub const UINT32: u8 = 12;
    pub const BOOLEAN_TRUE: u8 = 13;
    pub const BOOLEAN_FALSE: u8 = 14;
    pub const INT64_ZERO: u8 = 15;
    pub const INT64_ONE: u8 = 16;
    pub const DOUBLE_ZERO: u8 = 17;
    pub const DOUBLE_ONE: u8 = 18;
    pub const FLOAT: u8 = 19;
    pub const INT16: u8 = 20;
    pub const UINT16: u8 = 21;
    pub const INT8: u8 = 22;
    pub const UINT8: u8 = 23;
    pub const ARRAY_TYPE_BYTE_LENGTH: u8 = 24;
    pub const ARRAY_TYPE_AUXILIARY_BUFFER: u8 = 25;
}

impl<'b> Context<'b> {
    /// Next type byte, skipping any flag byte (flags only mark string subtypes for tools).
    fn read_type(&mut self) -> Result<u8, String> {
        let byte = *take(&mut self.types, 1)?.first().unwrap_or(&0);
        if self.version >= 3 {
            if byte & 0x80 != 0 {
                take(&mut self.types, 1)?;
            }
            if byte & 0x40 != 0 {
                take(&mut self.types, 1)?;
            }
            Ok(byte & 0x3F)
        } else {
            if byte & 0x80 != 0 {
                take(&mut self.types, 1)?;
            }
            Ok(byte & 0x7F)
        }
    }

    fn string(&self, id: i32) -> String {
        usize::try_from(id)
            .ok()
            .and_then(|i| self.strings.get(i))
            .cloned()
            .unwrap_or_else(String::new)
    }

    fn read_member(&mut self, into: &mut Vec<(String, Value)>) -> Result<(), String> {
        let kind = self.read_type()?;
        let name_id = take_i32(&mut self.buffer.bytes4)?;
        let name = self.string(name_id);
        let value = self.read_value(kind, false)?;
        // A repeated member name replaces the earlier value.
        if let Some(slot) = into.iter_mut().find(|(k, _)| *k == name) {
            slot.1 = value;
        } else {
            into.push((name, value));
        }
        Ok(())
    }

    /// One value of `kind`; primitive values come from the auxiliary buffer when `aux`.
    fn read_value(&mut self, kind: u8, aux: bool) -> Result<Value, String> {
        let lane = if aux {
            &mut self.auxiliary
        } else {
            &mut self.buffer
        };
        Ok(match kind {
            node::NULL => Value::Null,
            node::BOOLEAN_TRUE => Value::Bool(true),
            node::BOOLEAN_FALSE => Value::Bool(false),
            node::INT64_ZERO => Value::Int(0),
            node::INT64_ONE => Value::Int(1),
            node::DOUBLE_ZERO => Value::Double(0.0),
            node::DOUBLE_ONE => Value::Double(1.0),
            node::BOOLEAN => Value::Bool(take(&mut lane.bytes1, 1)?[0] != 0),
            node::INT8 => Value::Int(i64::from(take(&mut lane.bytes1, 1)?[0] as i8)),
            node::UINT8 => Value::UInt(u64::from(take(&mut lane.bytes1, 1)?[0])),
            node::INT16 => Value::Int(i64::from(i16::from_le_bytes(
                take(&mut lane.bytes2, 2)?.try_into().unwrap_or([0; _]),
            ))),
            node::UINT16 => Value::UInt(u64::from(u16::from_le_bytes(
                take(&mut lane.bytes2, 2)?.try_into().unwrap_or([0; _]),
            ))),
            node::INT32 => Value::Int(i64::from(take_i32(&mut lane.bytes4)?)),
            node::UINT32 => Value::UInt(u64::from(u32::from_le_bytes(
                take(&mut lane.bytes4, 4)?.try_into().unwrap_or([0; _]),
            ))),
            node::FLOAT => Value::Double(f64::from(f32::from_le_bytes(
                take(&mut lane.bytes4, 4)?.try_into().unwrap_or([0; _]),
            ))),
            node::INT64 => Value::Int(i64::from_le_bytes(
                take(&mut lane.bytes8, 8)?.try_into().unwrap_or([0; _]),
            )),
            node::UINT64 => Value::UInt(u64::from_le_bytes(
                take(&mut lane.bytes8, 8)?.try_into().unwrap_or([0; _]),
            )),
            node::DOUBLE => Value::Double(f64::from_le_bytes(
                take(&mut lane.bytes8, 8)?.try_into().unwrap_or([0; _]),
            )),
            node::STRING => {
                let id = take_i32(&mut self.buffer.bytes4)?;
                Value::String(self.string(id))
            }
            node::BINARY_BLOB if self.version < 2 => {
                let length = take_i32(&mut self.buffer.bytes4)?.max(0) as usize;
                Value::Blob(take(&mut self.buffer.bytes1, length)?.to_vec())
            }
            node::BINARY_BLOB => {
                let length = take_i32(&mut self.blob_lengths)?.max(0) as usize;
                Value::Blob(take(&mut self.blobs, length)?.to_vec())
            }
            node::ARRAY => {
                let length = take_i32(&mut self.buffer.bytes4)?.max(0) as usize;
                let mut items = Vec::with_capacity(length.min(1 << 16));
                for _ in 0..length {
                    let kind = self.read_type()?;
                    items.push(self.read_value(kind, false)?);
                }
                Value::Array(items)
            }
            node::ARRAY_TYPED
            | node::ARRAY_TYPE_BYTE_LENGTH
            | node::ARRAY_TYPE_AUXILIARY_BUFFER => {
                let length = if kind == node::ARRAY_TYPED {
                    take_i32(&mut self.buffer.bytes4)?.max(0) as usize
                } else {
                    usize::from(take(&mut self.buffer.bytes1, 1)?[0])
                };
                let element = self.read_type()?;
                let aux = kind == node::ARRAY_TYPE_AUXILIARY_BUFFER;
                let mut items = Vec::with_capacity(length.min(1 << 16));
                for _ in 0..length {
                    items.push(self.read_value(element, aux)?);
                }
                Value::Array(items)
            }
            node::OBJECT => {
                let length = if self.version >= 5 {
                    take_i32(&mut self.object_lengths)?
                } else {
                    take_i32(&mut self.buffer.bytes4)?
                }
                .max(0) as usize;
                let mut members = Vec::with_capacity(length.min(1 << 16));
                for _ in 0..length {
                    self.read_member(&mut members)?;
                }
                Value::Object(members)
            }
            other => return Err(format!("kv3: unknown node type {other}")),
        })
    }
}

struct Reader<'b> {
    data: &'b [u8],
    at: usize,
}

impl<'b> Reader<'b> {
    fn bytes(&mut self, n: usize) -> Result<&'b [u8], String> {
        let out = self
            .data
            .get(self.at..self.at + n)
            .ok_or_else(|| "kv3: header past the end".to_owned())?;
        self.at += n;
        Ok(out)
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(
            self.bytes(2)?.try_into().unwrap_or([0; _]),
        ))
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(
            self.bytes(4)?.try_into().unwrap_or([0; _]),
        ))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.bytes(4)?.try_into().unwrap_or([0; _]),
        ))
    }
    fn count(&mut self) -> Result<usize, String> {
        Ok(self.i32()?.max(0) as usize)
    }
}

fn lz4(input: &[u8], size: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0; size];
    let written =
        lz4_flex::block::decompress_into(input, &mut out).map_err(|e| format!("kv3: lz4: {e}"))?;
    if written != size {
        return Err(format!("kv3: lz4 gave {written} bytes, expected {size}"));
    }
    Ok(out)
}

/// Decode every zstd frame of `input` into exactly `size` bytes.
pub(crate) fn zstd(input: &[u8], size: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0; size];
    let mut decoder = ruzstd::decoding::FrameDecoder::new();
    let written = decoder
        .decode_all(input, &mut out)
        .map_err(|e| format!("zstd: {e}"))?;
    out.truncate(written);
    Ok(out)
}

fn align(offset: usize, to: usize) -> usize {
    offset.div_ceil(to) * to
}

/// Lanes laid out one after another from `offset`, each aligned to its size.
fn split_lanes(
    buffer: &[u8],
    mut offset: usize,
    counts: [usize; 4],
    align_empty: bool,
) -> Result<(Lanes<'_>, usize), String> {
    let mut lanes = Lanes::default();
    let get = |from: usize, to: usize| {
        buffer
            .get(from..to)
            .ok_or_else(|| "kv3: lane past the buffer".to_owned())
    };
    if counts[0] > 0 {
        lanes.bytes1 = get(offset, offset + counts[0])?;
        offset += counts[0];
    }
    if counts[1] > 0 {
        offset = align(offset, 2);
        lanes.bytes2 = get(offset, offset + counts[1] * 2)?;
        offset += counts[1] * 2;
    }
    if counts[2] > 0 {
        offset = align(offset, 4);
        lanes.bytes4 = get(offset, offset + counts[2] * 4)?;
        offset += counts[2] * 4;
    }
    if counts[3] > 0 {
        offset = align(offset, 8);
        lanes.bytes8 = get(offset, offset + counts[3] * 8)?;
        offset += counts[3] * 8;
    } else if align_empty {
        offset = align(offset, 8);
    }
    Ok((lanes, offset))
}

fn read_strings(bytes: &mut &[u8], count: usize) -> Result<(Vec<String>, usize), String> {
    let mut strings = Vec::with_capacity(count.min(1 << 16));
    let mut used = 0;
    for _ in 0..count {
        let end = bytes
            .iter()
            .position(|&c| c == 0)
            .ok_or_else(|| "kv3: unterminated string".to_owned())?;
        strings.push(String::from_utf8_lossy(&bytes[..end]).into_owned());
        *bytes = &bytes[end + 1..];
        used += end + 1;
    }
    Ok((strings, used))
}

/// Parse a binary KV3 block.
pub fn parse(data: &[u8]) -> Result<Value, String> {
    let mut r = Reader { data, at: 0 };
    let magic = r.u32()?;
    if magic == MAGIC_LEGACY {
        return Err("kv3: legacy VKV3 encoding not supported".to_owned());
    }
    let version = magic & 0xFF;
    if magic & 0xFFFF_FF00 != 0x4B56_3300 || !(1..=5).contains(&version) {
        return Err(format!("kv3: bad signature {magic:#x}"));
    }
    r.bytes(16)?; // format guid
    let compression = r.u32()?;
    let mut counts1 = [0usize; 4];
    let (mut count_types, mut count_blocks, mut size_blobs) = (0usize, 0usize, 0usize);
    let size_uncompressed_total;
    let mut size_compressed_total = 0usize;
    let mut frame_size = 0usize;
    if version == 1 {
        counts1[0] = r.count()?;
        counts1[2] = r.count()?;
        counts1[3] = r.count()?;
        size_uncompressed_total = r.count()?;
    } else {
        let dictionary = r.u16()?;
        frame_size = usize::from(r.u16()?);
        if dictionary != 0 {
            return Err("kv3: compression dictionaries not supported".to_owned());
        }
        counts1[0] = r.count()?;
        counts1[2] = r.count()?;
        counts1[3] = r.count()?;
        count_types = r.count()?;
        r.u16()?; // objects
        r.u16()?; // arrays
        size_uncompressed_total = r.count()?;
        size_compressed_total = r.count()?;
        count_blocks = r.count()?;
        size_blobs = r.count()?;
    }
    let mut size_block_sizes = 0usize;
    if version >= 4 {
        counts1[1] = r.count()?;
        size_block_sizes = r.count()?;
    }
    let (mut size_u1, mut size_c1) = (size_uncompressed_total, size_compressed_total);
    let (mut size_u2, mut size_c2) = (0usize, 0usize);
    let mut counts2 = [0usize; 4];
    let mut objects2 = 0usize;
    if version >= 5 {
        size_u1 = r.count()?;
        size_c1 = r.count()?;
        size_u2 = r.count()?;
        size_c2 = r.count()?;
        counts2 = [r.count()?, r.count()?, r.count()?, r.count()?];
        r.count()?; // nodes
        objects2 = r.count()?;
        r.count()?; // arrays
        r.count()?; // array elements
    }
    if version == 1 {
        size_c1 = data.len() - r.at;
    }

    // Buffer 1 (and, before v5 with zstd, the blobs after it in the same frames).
    let zstd_joined = version < 5 && compression == 2;
    let raw1 = match compression {
        0 => r.bytes(size_u1)?.to_vec(),
        1 => lz4(r.bytes(size_c1)?, size_u1)?,
        2 => zstd(
            r.bytes(size_c1)?,
            size_u1 + if zstd_joined { size_blobs } else { 0 },
        )?,
        other => return Err(format!("kv3: compression {other}")),
    };
    let (buffer1, joined_blobs) = raw1.split_at(size_u1.min(raw1.len()));
    let raw2 = if version >= 5 {
        match compression {
            0 => r.bytes(size_u2)?.to_vec(),
            1 => lz4(r.bytes(size_c2)?, size_u2)?,
            _ => zstd(r.bytes(size_c2)?, size_u2)?,
        }
    } else {
        Vec::new()
    };

    let (lanes1, offset1) = split_lanes(buffer1, 0, counts1, version < 5)?;
    let mut lanes1 = lanes1;
    let count_strings = take_i32(&mut lanes1.bytes4)?.max(0) as usize;
    let mut context = Context {
        version,
        types: &[],
        object_lengths: &[],
        blobs: &[],
        blob_lengths: &[],
        strings: Vec::new(),
        buffer: lanes1,
        auxiliary: lanes1,
    };
    let blob_sizes: &[u8] = if version >= 5 {
        let (strings, _) = read_strings(&mut context.auxiliary.bytes1, count_strings)?;
        context.strings = strings;
        let end = objects2 * 4;
        context.object_lengths = raw2.get(..end).ok_or("kv3: object lengths")?;
        let (lanes2, offset2) = split_lanes(&raw2, end, counts2, false)?;
        context.buffer = lanes2;
        context.types = raw2
            .get(offset2..offset2 + count_types)
            .ok_or("kv3: types past the buffer")?;
        let after = offset2 + count_types;
        raw2.get(after..).unwrap_or(&[])
    } else {
        let mut strings_area = buffer1.get(offset1..).unwrap_or(&[]);
        let (strings, used) = read_strings(&mut strings_area, count_strings)?;
        context.strings = strings;
        let types_start = offset1 + used;
        let types_len = if version == 1 {
            size_uncompressed_total.saturating_sub(types_start + 4)
        } else {
            count_types.saturating_sub(used)
        };
        context.types = buffer1
            .get(types_start..types_start + types_len)
            .ok_or("kv3: types past the buffer")?;
        buffer1.get(types_start + types_len..).unwrap_or(&[])
    };

    // Binary blobs.
    let blobs_storage: Vec<u8>;
    if count_blocks > 0 {
        let mut sizes = blob_sizes;
        context.blob_lengths = take(&mut sizes, count_blocks * 4)?;
        let trailer = take_i32(&mut sizes)? as u32;
        if trailer != TRAILER {
            return Err("kv3: bad blob table trailer".to_owned());
        }
        blobs_storage = match compression {
            0 => r.bytes(size_blobs)?.to_vec(),
            1 => {
                // Each blob is split into LZ4 frames of up to `frame_size`, chained so a frame
                // may refer back into everything decoded before it.
                let mut out = vec![0u8; size_blobs];
                let mut at = 0usize;
                let mut lengths = context.blob_lengths;
                while !lengths.is_empty() {
                    let length = take_i32(&mut lengths)?.max(0) as usize;
                    let end = at + length;
                    while at < end {
                        let compressed = usize::from(u16::from_le_bytes(
                            take(&mut sizes, 2)?.try_into().unwrap_or([0; _]),
                        ));
                        let input = r.bytes(compressed)?;
                        let (done, rest) = out.split_at_mut(at);
                        let window = &done[done.len().saturating_sub(65536)..];
                        let limit = (end - at).min(frame_size.max(1));
                        let written = lz4_flex::block::decompress_into_with_dict(
                            input,
                            &mut rest[..limit],
                            window,
                        )
                        .map_err(|e| format!("kv3: lz4 blob: {e}"))?;
                        if written == 0 {
                            return Err("kv3: empty lz4 blob frame".to_owned());
                        }
                        at += written;
                    }
                }
                out
            }
            _ if version >= 5 => {
                let compressed = size_compressed_total.saturating_sub(size_c1 + size_c2);
                zstd(r.bytes(compressed)?, size_blobs)?
            }
            _ => joined_blobs.to_vec(),
        };
        context.blobs = &[];
        let _ = size_block_sizes;
        let trailer = r.u32()?;
        if trailer != TRAILER {
            return Err("kv3: bad blob trailer".to_owned());
        }
    } else {
        blobs_storage = Vec::new();
    }
    context.blobs = &blobs_storage;

    let kind = context.read_type()?;
    context.read_value(kind, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built v4 block: `{ a = 7, b = "hi", c = [1.5, 2.5] }`, uncompressed.
    fn sample_v4() -> Vec<u8> {
        // Lanes: bytes1 empty, bytes2 empty, bytes4: [strings=3, len(obj)=3, name a, int 7,
        // name b, str 'hi', name c, array len 2, float 1.5, float 2.5], bytes8 empty.
        let mut buffer = Vec::new();
        let mut b4: Vec<i32> = vec![4, 3, 0, 7, 1, 3, 2, 2];
        let floats = [1.5f32, 2.5f32];
        for v in b4.drain(..) {
            buffer.extend_from_slice(&v.to_le_bytes());
        }
        for f in floats {
            buffer.extend_from_slice(&f.to_le_bytes());
        }
        let count4 = buffer.len() / 4;
        // Align the (empty) 8-byte lane.
        while buffer.len() % 8 != 0 {
            buffer.push(0);
        }
        let strings_start = buffer.len();
        for s in ["a", "b", "c", "hi"] {
            buffer.extend_from_slice(s.as_bytes());
            buffer.push(0);
        }
        // Types: object, int32, string, array(typed? no: plain array) float, float.
        let types = [
            node::OBJECT,
            node::INT32,
            node::STRING,
            node::ARRAY,
            node::FLOAT,
            node::FLOAT,
        ];
        buffer.extend_from_slice(&types);
        let count_types = buffer.len() - strings_start;
        buffer.extend_from_slice(&TRAILER.to_le_bytes());
        let mut out = Vec::new();
        out.extend_from_slice(&0x4B56_3304u32.to_le_bytes());
        out.extend_from_slice(&[0; 16]);
        out.extend_from_slice(&0u32.to_le_bytes()); // uncompressed
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        for v in [0i32, count4 as i32, 0, count_types as i32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        for v in [buffer.len() as i32, buffer.len() as i32, 0, 0, 0, 0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&buffer);
        out
    }

    #[test]
    fn reads_an_uncompressed_v4_object() {
        let value = parse(&sample_v4()).expect("parse");
        assert_eq!(value.int_of("a"), 7);
        assert_eq!(value.str_of("b"), "hi");
        assert_eq!(value.get("c").map(Value::as_f32s), Some(vec![1.5, 2.5]));
        assert_eq!(value.path("c/1").and_then(Value::as_f32), Some(2.5));
    }
}
