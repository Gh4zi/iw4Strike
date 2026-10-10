//! meshoptimizer's vertex and index buffer codecs (decoding only), as CS2 compresses model mesh
//! buffers (`MVTX` blocks start `0xa1`, `MIDX` blocks `0xe1`).

const VERTEX_HEADER: u8 = 0xa0;
const INDEX_HEADER: u8 = 0xe0;
const VERTEX_BLOCK_SIZE_BYTES: usize = 8192;
const VERTEX_BLOCK_MAX_SIZE: usize = 256;
const BYTE_GROUP_SIZE: usize = 16;
const BYTE_GROUP_DECODE_LIMIT: usize = 24;
const TAIL_MIN_SIZE_V0: usize = 32;
const TAIL_MIN_SIZE_V1: usize = 24;
const BITS_V0: [u32; 4] = [0, 2, 4, 8];
const BITS_V1: [u32; 5] = [0, 1, 2, 4, 8];

fn err<T>(what: &str) -> Result<T, String> {
    Err(format!("meshopt: {what}"))
}

fn vertex_block_size(vertex_size: usize) -> usize {
    let result = (VERTEX_BLOCK_SIZE_BYTES / vertex_size) & !(BYTE_GROUP_SIZE - 1);
    result.min(VERTEX_BLOCK_MAX_SIZE)
}

/// One group of 16 bytes encoded at `bits` per byte, with escapes for bytes that do not fit.
fn decode_bytes_group<'d>(data: &'d [u8], out: &mut [u8], bits: u32) -> Result<&'d [u8], String> {
    match bits {
        0 => {
            out[..BYTE_GROUP_SIZE].fill(0);
            Ok(data)
        }
        8 => {
            out[..BYTE_GROUP_SIZE]
                .copy_from_slice(data.get(..BYTE_GROUP_SIZE).ok_or("meshopt: group")?);
            Ok(&data[BYTE_GROUP_SIZE..])
        }
        1 | 2 | 4 => {
            let header_bytes = (BYTE_GROUP_SIZE * bits as usize) / 8;
            let per_byte = 8 / bits as usize;
            let mut extra = header_bytes;
            let max = (1u32 << bits) - 1;
            for (i, slot_out) in out.iter_mut().enumerate().take(BYTE_GROUP_SIZE) {
                let byte = u32::from(*data.get(i / per_byte).ok_or("meshopt: group")?);
                // One-bit groups store their bits least significant first; the others most
                // significant first.
                let slot = i % per_byte;
                let shift = if bits == 1 {
                    slot as u32
                } else {
                    8 - bits * (slot as u32 + 1)
                };
                let encoded = (byte >> shift) & max;
                *slot_out = if encoded == max {
                    let value = *data.get(extra).ok_or("meshopt: escape")?;
                    extra += 1;
                    value
                } else {
                    encoded as u8
                };
            }
            Ok(&data[extra..])
        }
        _ => err("bit width"),
    }
}

fn decode_bytes<'d>(mut data: &'d [u8], out: &mut [u8], bits: &[u32]) -> Result<&'d [u8], String> {
    if !out.len().is_multiple_of(BYTE_GROUP_SIZE) {
        return err("byte group size");
    }
    let groups = out.len() / BYTE_GROUP_SIZE;
    let header_size = groups.div_ceil(4);
    let header = data.get(..header_size).ok_or("meshopt: header")?;
    data = &data[header_size..];
    for group in 0..groups {
        if data.len() < BYTE_GROUP_DECODE_LIMIT {
            return err("truncated byte group");
        }
        let code = (header[group / 4] >> ((group % 4) * 2)) & 3;
        let width = *bits.get(usize::from(code)).ok_or("meshopt: code")?;
        data = decode_bytes_group(data, &mut out[group * BYTE_GROUP_SIZE..], width)?;
    }
    Ok(data)
}

fn unzigzag8(v: u32) -> u32 {
    (0u32.wrapping_sub(v & 1) ^ (v >> 1)) & 0xFF
}

fn unzigzag16(v: u32) -> u32 {
    (0u32.wrapping_sub(v & 1) ^ (v >> 1)) & 0xFFFF
}

/// Undo the delta coding of one four-byte column, `size` bytes per element.
#[allow(clippy::too_many_arguments)]
fn decode_deltas(
    size: usize,
    buffer: &[u8],
    transposed: &mut [u8],
    column: usize,
    vertex_count: usize,
    vertex_size: usize,
    last_vertex: &[u8],
    rot: u32,
) {
    let mut lanes = buffer;
    for k in (0..4).step_by(size) {
        let mut p: u32 = 0;
        for j in 0..size {
            p |= u32::from(last_vertex[column + k + j]) << (8 * j);
        }
        let mut offset = column + k;
        for i in 0..vertex_count {
            let mut v: u32 = 0;
            for j in 0..size {
                v |= u32::from(lanes[i + vertex_count * j]) << (8 * j);
            }
            v = match size {
                1 => unzigzag8(v).wrapping_add(p) & 0xFF,
                2 => unzigzag16(v).wrapping_add(p) & 0xFFFF,
                _ => v.rotate_left(rot) ^ p,
            };
            for j in 0..size {
                transposed[offset + j] = (v >> (8 * j)) as u8;
            }
            p = v;
            offset += vertex_size;
        }
        lanes = &lanes[vertex_count * size..];
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_vertex_block<'d>(
    mut data: &'d [u8],
    out: &mut [u8],
    vertex_count: usize,
    vertex_size: usize,
    last_vertex: &mut [u8],
    channels: &[u8],
    version: u8,
    buffer: &mut [u8],
    transposed: &mut [u8],
) -> Result<&'d [u8], String> {
    let aligned = vertex_count.div_ceil(BYTE_GROUP_SIZE) * BYTE_GROUP_SIZE;
    let control_size = if version == 0 { 0 } else { vertex_size / 4 };
    let control = data.get(..control_size).ok_or("meshopt: control")?;
    data = &data[control_size..];
    for k in (0..vertex_size).step_by(4) {
        let control_byte = if version == 0 { 0 } else { control[k / 4] };
        for j in 0..4 {
            let lane = &mut buffer[j * vertex_count..];
            match (control_byte >> (j * 2)) & 3 {
                3 => {
                    // literal
                    lane[..vertex_count]
                        .copy_from_slice(data.get(..vertex_count).ok_or("meshopt: literal")?);
                    data = &data[vertex_count..];
                }
                2 => lane[..vertex_count].fill(0),
                ctrl => {
                    let bits: &[u32] = if version == 0 {
                        &BITS_V0
                    } else {
                        &BITS_V1[usize::from(ctrl)..]
                    };
                    // Groups are decoded into the aligned width; the tail spills into the
                    // next lane's space, which is decoded after.
                    let mut scratch = vec![0u8; aligned];
                    data = decode_bytes(data, &mut scratch, bits)?;
                    lane[..vertex_count].copy_from_slice(&scratch[..vertex_count]);
                }
            }
        }
        let channel = if version == 0 { 0 } else { channels[k / 4] };
        let (size, rot) = match channel & 3 {
            0 => (1, 0),
            1 => (2, 0),
            2 => (4, (32 - u32::from(channel >> 4)) & 31),
            _ => return err("channel"),
        };
        decode_deltas(
            size,
            buffer,
            transposed,
            k,
            vertex_count,
            vertex_size,
            last_vertex,
            rot,
        );
    }
    out[..vertex_count * vertex_size].copy_from_slice(&transposed[..vertex_count * vertex_size]);
    last_vertex
        .copy_from_slice(&transposed[vertex_size * (vertex_count - 1)..vertex_size * vertex_count]);
    Ok(data)
}

/// Decode a meshoptimizer vertex buffer of `vertex_count` vertices of `vertex_size` bytes.
pub fn decode_vertex_buffer(
    vertex_count: usize,
    vertex_size: usize,
    data: &[u8],
) -> Result<Vec<u8>, String> {
    if vertex_size == 0 || vertex_size > 256 || !vertex_size.is_multiple_of(4) {
        return err("vertex size");
    }
    let header = *data.first().ok_or("meshopt: empty")?;
    if header & 0xF0 != VERTEX_HEADER {
        return err("not a vertex buffer");
    }
    let version = header & 0x0F;
    if version > 1 {
        return err("vertex codec version");
    }
    let mut data = &data[1..];
    let tail_size = vertex_size + if version == 0 { 0 } else { vertex_size / 4 };
    let tail_min = if version == 0 {
        TAIL_MIN_SIZE_V0
    } else {
        TAIL_MIN_SIZE_V1
    };
    let tail_padded = tail_size.max(tail_min);
    if data.len() < tail_padded {
        return err("tail");
    }
    let tail = &data[data.len() - tail_size..];
    let mut last_vertex = tail[..vertex_size].to_vec();
    let channels = if version == 0 {
        Vec::new()
    } else {
        tail[vertex_size..vertex_size + vertex_size / 4].to_vec()
    };
    let block = vertex_block_size(vertex_size);
    let mut out = vec![0u8; vertex_count * vertex_size];
    let mut buffer = vec![0u8; VERTEX_BLOCK_MAX_SIZE * 4];
    let mut transposed = vec![0u8; VERTEX_BLOCK_SIZE_BYTES];
    let mut offset = 0;
    while offset < vertex_count {
        let count = block.min(vertex_count - offset);
        data = decode_vertex_block(
            data,
            &mut out[offset * vertex_size..],
            count,
            vertex_size,
            &mut last_vertex,
            &channels,
            version,
            &mut buffer,
            &mut transposed,
        )?;
        offset += count;
    }
    if data.len() != tail_padded {
        return err("tail size");
    }
    Ok(out)
}

fn decode_vbyte(data: &[u8], at: &mut usize) -> Result<u32, String> {
    let lead = u32::from(*data.get(*at).ok_or("meshopt: vbyte")?);
    *at += 1;
    if lead < 128 {
        return Ok(lead);
    }
    let mut result = lead & 127;
    let mut shift = 7;
    for _ in 0..4 {
        let group = u32::from(*data.get(*at).ok_or("meshopt: vbyte")?);
        *at += 1;
        result |= (group & 127) << shift;
        shift += 7;
        if group < 128 {
            break;
        }
    }
    Ok(result)
}

fn decode_index(data: &[u8], last: u32, at: &mut usize) -> Result<u32, String> {
    let v = decode_vbyte(data, at)?;
    let d = (v >> 1) ^ 0u32.wrapping_sub(v & 1);
    Ok(last.wrapping_add(d))
}

/// Decode a meshoptimizer index buffer into `u32` indices.
pub fn decode_index_buffer(index_count: usize, buffer: &[u8]) -> Result<Vec<u32>, String> {
    if !index_count.is_multiple_of(3) {
        return err("index count");
    }
    let data_offset = 1 + index_count / 3;
    if buffer.len() < data_offset + 16 {
        return err("index buffer too short");
    }
    if buffer[0] & 0xF0 != INDEX_HEADER {
        return err("not an index buffer");
    }
    let version = buffer[0] & 0x0F;
    if version > 1 {
        return err("index codec version");
    }
    let mut edge = [(0u32, 0u32); 16];
    let mut vertex = [0u32; 16];
    let (mut edge_at, mut vertex_at) = (0usize, 0usize);
    let (mut next, mut last) = (0u32, 0u32);
    let fecmax = if version >= 1 { 13 } else { 15 };
    let code = &buffer[1..data_offset];
    let data = &buffer[data_offset..];
    let safe_end = data.len() - 16;
    let codeaux = &data[safe_end..];
    let mut at = 0usize;
    let mut out = Vec::with_capacity(index_count);
    let push_edge = |edge: &mut [(u32, u32); 16], edge_at: &mut usize, a: u32, b: u32| {
        edge[*edge_at] = (a, b);
        *edge_at = (*edge_at + 1) & 15;
    };
    let push_vertex = |vertex: &mut [u32; 16], vertex_at: &mut usize, v: u32, cond: bool| {
        vertex[*vertex_at] = v;
        *vertex_at = (*vertex_at + usize::from(cond)) & 15;
    };
    for &codetri in code {
        if codetri < 0xf0 {
            let fe = usize::from(codetri >> 4);
            let (a, b) = edge[(edge_at.wrapping_sub(1 + fe)) & 15];
            let fec = codetri & 15;
            let c;
            if fec < fecmax {
                let cf = vertex[(vertex_at.wrapping_sub(1 + usize::from(fec))) & 15];
                c = if fec == 0 { next } else { cf };
                let fec0 = fec == 0;
                next += u32::from(fec0);
                push_vertex(&mut vertex, &mut vertex_at, c, fec0);
            } else {
                if at > safe_end {
                    return err("index data truncated");
                }
                c = if fec != 15 {
                    last.wrapping_add((u32::from(fec) * 2).wrapping_sub(27))
                } else {
                    decode_index(data, last, &mut at)?
                };
                last = c;
                push_vertex(&mut vertex, &mut vertex_at, c, true);
            }
            push_edge(&mut edge, &mut edge_at, c, b);
            push_edge(&mut edge, &mut edge_at, a, c);
            out.extend_from_slice(&[a, b, c]);
        } else if codetri < 0xfe {
            let aux = codeaux[usize::from(codetri & 15)];
            let feb = usize::from(aux >> 4);
            let fec = usize::from(aux & 15);
            let a = next;
            next += 1;
            let bf = vertex[(vertex_at.wrapping_sub(feb)) & 15];
            let b = if feb == 0 { next } else { bf };
            next += u32::from(feb == 0);
            let cf = vertex[(vertex_at.wrapping_sub(fec)) & 15];
            let c = if fec == 0 { next } else { cf };
            next += u32::from(fec == 0);
            out.extend_from_slice(&[a, b, c]);
            push_vertex(&mut vertex, &mut vertex_at, a, true);
            push_vertex(&mut vertex, &mut vertex_at, b, feb == 0);
            push_vertex(&mut vertex, &mut vertex_at, c, fec == 0);
            push_edge(&mut edge, &mut edge_at, b, a);
            push_edge(&mut edge, &mut edge_at, c, b);
            push_edge(&mut edge, &mut edge_at, a, c);
        } else {
            if at > safe_end {
                return err("index data truncated");
            }
            let aux = *data.get(at).ok_or("meshopt: codeaux")?;
            at += 1;
            let fea = if codetri == 0xfe { 0 } else { 15 };
            let feb = usize::from(aux >> 4);
            let fec = usize::from(aux & 15);
            if aux == 0 {
                next = 0;
            }
            let mut a = if fea == 0 {
                next += 1;
                next - 1
            } else {
                0
            };
            let mut b = if feb == 0 {
                next += 1;
                next - 1
            } else {
                vertex[(vertex_at.wrapping_sub(feb)) & 15]
            };
            let mut c = if fec == 0 {
                next += 1;
                next - 1
            } else {
                vertex[(vertex_at.wrapping_sub(fec)) & 15]
            };
            if fea == 15 {
                a = decode_index(data, last, &mut at)?;
                last = a;
            }
            if feb == 15 {
                b = decode_index(data, last, &mut at)?;
                last = b;
            }
            if fec == 15 {
                c = decode_index(data, last, &mut at)?;
                last = c;
            }
            out.extend_from_slice(&[a, b, c]);
            push_vertex(&mut vertex, &mut vertex_at, a, true);
            push_vertex(&mut vertex, &mut vertex_at, b, feb == 0 || feb == 15);
            push_vertex(&mut vertex, &mut vertex_at, c, fec == 0 || fec == 15);
            push_edge(&mut edge, &mut edge_at, b, a);
            push_edge(&mut edge, &mut edge_at, c, b);
            push_edge(&mut edge, &mut edge_at, a, c);
        }
    }
    if at != safe_end {
        return err("index data malformed");
    }
    Ok(out)
}
