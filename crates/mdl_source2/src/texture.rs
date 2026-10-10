//! Compiled textures (`.vtex_c`): the header in the `DATA` block, then every mip level after
//! it, smallest first, each block compressed (BC1/3/4/5/6H/7) or plain, optionally LZ4 per mip.

use crate::Resource;

/// Pixel formats CS2 ships that the renderer can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bc1,
    Bc3,
    Bc4,
    Bc5,
    Bc6h,
    Bc7,
    Rgba8,
    Bgra8,
    R8,
    Rgba16F,
}

impl Format {
    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::Bc1,
            2 => Self::Bc3,
            3 | 33 => Self::R8,
            4 => Self::Rgba8,
            10 => Self::Rgba16F,
            19 => Self::Bc6h,
            20 => Self::Bc7,
            21 => Self::Bc5,
            27 => Self::Bc4,
            28 => Self::Bgra8,
            _ => return None,
        })
    }

    /// Bytes per 4×4 block for block-compressed formats, per pixel otherwise.
    #[must_use]
    pub fn block_bytes(self) -> usize {
        match self {
            Self::Bc1 | Self::Bc4 => 8,
            Self::Bc3 | Self::Bc5 | Self::Bc6h | Self::Bc7 => 16,
            Self::Rgba8 | Self::Bgra8 => 4,
            Self::R8 => 1,
            Self::Rgba16F => 8,
        }
    }

    #[must_use]
    pub fn is_block_compressed(self) -> bool {
        matches!(
            self,
            Self::Bc1 | Self::Bc3 | Self::Bc4 | Self::Bc5 | Self::Bc6h | Self::Bc7
        )
    }

    /// Bytes for a `width`×`height` level.
    #[must_use]
    pub fn level_size(self, width: usize, height: usize) -> usize {
        if self.is_block_compressed() {
            width.div_ceil(4).max(1) * height.div_ceil(4).max(1) * self.block_bytes()
        } else {
            width * height * self.block_bytes()
        }
    }
}

/// A decoded texture: level 0 first.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub format: Format,
    pub mips: Vec<Vec<u8>>,
    /// The header's flags (bit 4 cube map, bit 5 volume).
    pub flags: u16,
}

impl Texture {
    #[must_use]
    pub fn mip_size(&self, level: usize) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }
}

const CUBE: u16 = 1 << 4;
const VOLUME: u16 = 1 << 5;
const EXTRA_COMPRESSED_MIP_SIZE: u32 = 4;

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(
        b.get(at..at + 2)
            .and_then(|s| s.try_into().ok())
            .unwrap_or([0; _]),
    )
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(
        b.get(at..at + 4)
            .and_then(|s| s.try_into().ok())
            .unwrap_or([0; _]),
    )
}

/// Decode a texture with every mip it has.
pub fn load(bytes: &[u8]) -> Result<Texture, String> {
    load_max(bytes, u32::MAX)
}

/// Decode a texture's mips no wider or taller than `max_size`: the larger ones are neither
/// kept nor decompressed (they come last), and level 0 is the largest that fits.
pub fn load_max(bytes: &[u8], max_size: u32) -> Result<Texture, String> {
    let resource = Resource::parse(bytes)?;
    let data = resource.block(b"DATA").ok_or("vtex: no DATA block")?;
    let header = resource.bytes(data);
    if u16_at(header, 0) != 1 {
        return Err("vtex: unknown version".to_owned());
    }
    let flags = u16_at(header, 2);
    if flags & (CUBE | VOLUME) != 0 {
        return Err("vtex: cube and volume textures not supported".to_owned());
    }
    let width = u32::from(u16_at(header, 20));
    let height = u32::from(u16_at(header, 22));
    let depth = usize::from(u16_at(header, 24)).max(1);
    let format_code = *header.get(26).ok_or("vtex: short header")?;
    let format =
        Format::from_code(format_code).ok_or_else(|| format!("vtex: format {format_code}"))?;
    let levels = usize::from(*header.get(27).ok_or("vtex: short header")?).max(1);
    // Extra data: compressed mip sizes, when the mips are LZ4 packed.
    let extra_offset = 32 + u32_at(header, 32) as usize;
    let extra_count = u32_at(header, 36) as usize;
    let mut compressed: Option<(u32, Vec<usize>)> = None;
    for i in 0..extra_count {
        let at = extra_offset + i * 12;
        let kind = u32_at(header, at);
        let offset = at + 4 + u32_at(header, at + 4) as usize;
        if kind == EXTRA_COMPRESSED_MIP_SIZE {
            let method = u32_at(header, offset);
            let sizes_at = offset + 4 + u32_at(header, offset + 4) as usize;
            let count = u32_at(header, offset + 8) as usize;
            let sizes = (0..count)
                .map(|m| u32_at(header, sizes_at + m * 4) as usize)
                .collect();
            compressed = Some((method, sizes));
        }
    }
    let mut at = data.offset + data.size;
    let mut mips: Vec<Vec<u8>> = vec![Vec::new(); levels];
    let mut top = 0;
    for level in (0..levels).rev() {
        let w = (width as usize >> level).max(1);
        let h = (height as usize >> level).max(1);
        if w.max(h) > max_size as usize && level + 1 < levels {
            top = level + 1;
            break;
        }
        let size = format.level_size(w, h) * depth;
        let stored = compressed
            .as_ref()
            .and_then(|(_, sizes)| sizes.get(level).copied())
            .map_or(size, |c| c.min(size));
        let raw = bytes.get(at..at + stored).ok_or("vtex: mip past the end")?;
        at += stored;
        mips[level] = if stored < size {
            match compressed.as_ref().map(|(m, _)| *m) {
                Some(1) => {
                    let mut out = vec![0u8; size];
                    lz4_flex::block::decompress_into(raw, &mut out)
                        .map_err(|e| format!("vtex: lz4 mip: {e}"))?;
                    out
                }
                Some(2) => crate::kv3::zstd(raw, size)?,
                _ => return Err("vtex: unknown mip compression".to_owned()),
            }
        } else {
            raw.to_vec()
        };
    }
    mips.drain(..top);
    Ok(Texture {
        width: (width >> top).max(1),
        height: (height >> top).max(1),
        format,
        mips,
        flags,
    })
}
