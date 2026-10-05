//! Valve texture files (VTF 7.0-7.5): the largest mip of the first frame, decoded to RGBA8.

#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// The format carries a real alpha channel.
    pub has_alpha: bool,
}

const FLAG_ENVMAP: u32 = 0x4000;
const HIGH_RES_TAG: [u8; 3] = [0x30, 0, 0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Rgba8888,
    Abgr8888,
    Rgb888,
    Bgr888,
    I8,
    Ia88,
    A8,
    Argb8888,
    Bgra8888,
    Dxt1,
    Dxt3,
    Dxt5,
    Bgrx8888,
}

impl Format {
    fn from_id(id: i32) -> Option<Self> {
        Some(match id {
            0 => Self::Rgba8888,
            1 => Self::Abgr8888,
            2 => Self::Rgb888,
            3 => Self::Bgr888,
            5 => Self::I8,
            6 => Self::Ia88,
            8 => Self::A8,
            11 => Self::Argb8888,
            12 => Self::Bgra8888,
            13 | 20 => Self::Dxt1,
            14 => Self::Dxt3,
            15 => Self::Dxt5,
            16 => Self::Bgrx8888,
            _ => return None,
        })
    }

    fn size(self, w: u32, h: u32) -> usize {
        let (w, h) = (w as usize, h as usize);
        let blocks = w.div_ceil(4).max(1) * h.div_ceil(4).max(1);
        match self {
            Self::Dxt1 => blocks * 8,
            Self::Dxt3 | Self::Dxt5 => blocks * 16,
            Self::Rgba8888 | Self::Abgr8888 | Self::Argb8888 | Self::Bgra8888 | Self::Bgrx8888 => {
                w * h * 4
            }
            Self::Rgb888 | Self::Bgr888 => w * h * 3,
            Self::Ia88 => w * h * 2,
            Self::I8 | Self::A8 => w * h,
        }
    }

    fn has_alpha(self) -> bool {
        matches!(
            self,
            Self::Rgba8888
                | Self::Abgr8888
                | Self::Argb8888
                | Self::Bgra8888
                | Self::Dxt3
                | Self::Dxt5
                | Self::Ia88
                | Self::A8
        )
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Decode the top mip of frame 0 (face 0, slice 0).
pub fn decode(bytes: &[u8]) -> Result<Image, String> {
    if bytes.get(0..4) != Some(b"VTF\0") {
        return Err("not a VTF".into());
    }
    let minor = u32_at(bytes, 8).ok_or("vtf header")?;
    let header_size = u32_at(bytes, 12).ok_or("vtf header")? as usize;
    let width = u32::from(u16_at(bytes, 16).ok_or("vtf header")?);
    let height = u32::from(u16_at(bytes, 18).ok_or("vtf header")?);
    let flags = u32_at(bytes, 20).ok_or("vtf header")?;
    let frames = usize::from(u16_at(bytes, 24).ok_or("vtf header")?.max(1));
    let format_id = u32_at(bytes, 52).ok_or("vtf header")? as i32;
    let mips = u32::from(*bytes.get(56).ok_or("vtf header")?).max(1);
    let low_id = u32_at(bytes, 57).ok_or("vtf header")? as i32;
    let low_w = u32::from(*bytes.get(61).ok_or("vtf header")?);
    let low_h = u32::from(*bytes.get(62).ok_or("vtf header")?);
    let depth = if minor >= 2 {
        u32::from(u16_at(bytes, 63).ok_or("vtf header")?.max(1))
    } else {
        1
    };
    let format = Format::from_id(format_id).ok_or(format!("vtf image format {format_id}"))?;
    let faces = if flags & FLAG_ENVMAP != 0 {
        if minor < 5 { 7 } else { 6 }
    } else {
        1
    };

    let high_start = if minor >= 3 {
        let count = u32_at(bytes, 68).ok_or("vtf resources")? as usize;
        (0..count)
            .filter_map(|i| {
                let at = 80 + i * 8;
                let tag = bytes.get(at..at + 3)?;
                (tag == HIGH_RES_TAG).then(|| u32_at(bytes, at + 4).map(|v| v as usize))?
            })
            .next()
            .ok_or("vtf has no high-res image")?
    } else {
        let low = if low_id < 0 || low_w == 0 || low_h == 0 {
            0
        } else {
            Format::from_id(low_id).map_or(0, |f| f.size(low_w, low_h))
        };
        header_size + low
    };
    // Mips run smallest first; skip to mip 0.
    let mut at = high_start;
    for mip in (1..mips).rev() {
        let w = (width >> mip).max(1);
        let h = (height >> mip).max(1);
        let d = (depth >> mip).max(1) as usize;
        at += format.size(w, h) * frames * faces * d;
    }
    let len = format.size(width, height);
    let data = bytes.get(at..at + len).ok_or("vtf image data truncated")?;
    Ok(Image {
        width,
        height,
        rgba: to_rgba(format, width, height, data),
        has_alpha: format.has_alpha() || format_id == 20,
    })
}

fn to_rgba(format: Format, width: u32, height: u32, data: &[u8]) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0u8; w * h * 4];
    let pixel = |i: usize| -> [u8; 4] {
        match format {
            Format::Rgba8888 => [
                data[i * 4],
                data[i * 4 + 1],
                data[i * 4 + 2],
                data[i * 4 + 3],
            ],
            Format::Abgr8888 => [
                data[i * 4 + 3],
                data[i * 4 + 2],
                data[i * 4 + 1],
                data[i * 4],
            ],
            Format::Argb8888 => [
                data[i * 4 + 1],
                data[i * 4 + 2],
                data[i * 4 + 3],
                data[i * 4],
            ],
            Format::Bgra8888 => [
                data[i * 4 + 2],
                data[i * 4 + 1],
                data[i * 4],
                data[i * 4 + 3],
            ],
            Format::Bgrx8888 => [data[i * 4 + 2], data[i * 4 + 1], data[i * 4], 255],
            Format::Rgb888 => [data[i * 3], data[i * 3 + 1], data[i * 3 + 2], 255],
            Format::Bgr888 => [data[i * 3 + 2], data[i * 3 + 1], data[i * 3], 255],
            Format::I8 => [data[i], data[i], data[i], 255],
            Format::Ia88 => [data[i * 2], data[i * 2], data[i * 2], data[i * 2 + 1]],
            Format::A8 => [255, 255, 255, data[i]],
            Format::Dxt1 | Format::Dxt3 | Format::Dxt5 => [0; 4],
        }
    };
    match format {
        Format::Dxt1 | Format::Dxt3 | Format::Dxt5 => {
            let block_bytes = if format == Format::Dxt1 { 8 } else { 16 };
            let bw = w.div_ceil(4).max(1);
            for (b, block) in data.chunks_exact(block_bytes).enumerate() {
                let (bx, by) = ((b % bw) * 4, (b / bw) * 4);
                let texels = decode_block(format, block);
                for (t, texel) in texels.iter().enumerate() {
                    let (x, y) = (bx + t % 4, by + t / 4);
                    if x < w && y < h {
                        let o = (y * w + x) * 4;
                        out[o..o + 4].copy_from_slice(texel);
                    }
                }
            }
        }
        _ => {
            for i in 0..w * h {
                out[i * 4..i * 4 + 4].copy_from_slice(&pixel(i));
            }
        }
    }
    out
}

fn rgb565(c: u16) -> [u32; 3] {
    let r = u32::from((c >> 11) & 31);
    let g = u32::from((c >> 5) & 63);
    let b = u32::from(c & 31);
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    ]
}

/// One 4x4 block, row-major texels.
fn decode_block(format: Format, block: &[u8]) -> [[u8; 4]; 16] {
    let colour = if format == Format::Dxt1 {
        block
    } else {
        &block[8..16]
    };
    let c0 = u16::from_le_bytes([colour[0], colour[1]]);
    let c1 = u16::from_le_bytes([colour[2], colour[3]]);
    let (a, b) = (rgb565(c0), rgb565(c1));
    let mix = |wa: u32, wb: u32, d: u32| -> [u8; 4] {
        [
            ((a[0] * wa + b[0] * wb) / d) as u8,
            ((a[1] * wa + b[1] * wb) / d) as u8,
            ((a[2] * wa + b[2] * wb) / d) as u8,
            255,
        ]
    };
    let palette = if format != Format::Dxt1 || c0 > c1 {
        [mix(1, 0, 1), mix(0, 1, 1), mix(2, 1, 3), mix(1, 2, 3)]
    } else {
        [mix(1, 0, 1), mix(0, 1, 1), mix(1, 1, 2), [0, 0, 0, 0]]
    };
    let bits = u32::from_le_bytes([colour[4], colour[5], colour[6], colour[7]]);
    let mut out = [[0u8; 4]; 16];
    for (i, texel) in out.iter_mut().enumerate() {
        *texel = palette[((bits >> (i * 2)) & 3) as usize];
    }
    match format {
        Format::Dxt3 => {
            let alpha = u64::from_le_bytes(block[0..8].try_into().unwrap_or([0; 8]));
            for (i, texel) in out.iter_mut().enumerate() {
                let a = ((alpha >> (i * 4)) & 15) as u8;
                texel[3] = a * 17;
            }
        }
        Format::Dxt5 => {
            let (a0, a1) = (u32::from(block[0]), u32::from(block[1]));
            let mut levels = [0u32; 8];
            levels[0] = a0;
            levels[1] = a1;
            if a0 > a1 {
                for k in 1..7 {
                    levels[k + 1] = (a0 * (7 - k as u32) + a1 * k as u32) / 7;
                }
            } else {
                for k in 1..5 {
                    levels[k + 1] = (a0 * (5 - k as u32) + a1 * k as u32) / 5;
                }
                levels[6] = 0;
                levels[7] = 255;
            }
            let mut bits = 0u64;
            for k in 0..6 {
                bits |= u64::from(block[2 + k]) << (8 * k);
            }
            for (i, texel) in out.iter_mut().enumerate() {
                texel[3] = levels[((bits >> (i * 3)) & 7) as usize] as u8;
            }
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dxt1_block_picks_its_two_endpoints() {
        // c0 = pure red, c1 = pure blue, texel 0 -> c0, texel 1 -> c1.
        let mut block = Vec::new();
        block.extend_from_slice(&0xF800u16.to_le_bytes());
        block.extend_from_slice(&0x001Fu16.to_le_bytes());
        block.extend_from_slice(&0b0100u32.to_le_bytes());
        let t = decode_block(Format::Dxt1, &block);
        assert_eq!(t[0], [255, 0, 0, 255]);
        assert_eq!(t[1], [0, 0, 255, 255]);
    }

    #[test]
    fn dxt5_alpha_endpoints() {
        let mut block = vec![200u8, 100u8];
        // texel 0 index 0 (200), texel 1 index 1 (100).
        block.extend_from_slice(&[0b0000_1000, 0, 0, 0, 0, 0]);
        block.extend_from_slice(&[0; 8]);
        let t = decode_block(Format::Dxt5, &block);
        assert_eq!(t[0][3], 200);
        assert_eq!(t[1][3], 100);
    }
}
