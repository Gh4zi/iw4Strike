//! The container every compiled Source 2 file shares: a 16-byte header, then a table of typed
//! blocks (`DATA`, `RERL`, `MDAT`, `MVTX`, ...), each an offset and a size into the file.

use crate::kv3;

/// The only header version Source 2 has shipped.
const HEADER_VERSION: u16 = 12;

/// One block: its four-letter type and where its bytes are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: [u8; 4],
    pub offset: usize,
    pub size: usize,
}

impl Block {
    #[must_use]
    pub fn kind_str(&self) -> &str {
        std::str::from_utf8(&self.kind).unwrap_or("????")
    }
}

/// A parsed resource file, borrowing its bytes.
#[derive(Clone, Debug)]
pub struct Resource<'a> {
    pub data: &'a [u8],
    /// The resource type's own version (a model's, a texture's...).
    pub version: u16,
    pub blocks: Vec<Block>,
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

impl<'a> Resource<'a> {
    /// Read the header and block table.
    pub fn parse(data: &'a [u8]) -> Result<Self, String> {
        let bad = |what: &str| format!("resource: {what}");
        let header_version = u16_at(data, 4).ok_or_else(|| bad("too short"))?;
        if header_version != HEADER_VERSION {
            return Err(bad(&format!(
                "header version {header_version}, not a compiled resource"
            )));
        }
        let version = u16_at(data, 6).ok_or_else(|| bad("too short"))?;
        let table = 8 + u32_at(data, 8).ok_or_else(|| bad("too short"))? as usize;
        let count = u32_at(data, 12).ok_or_else(|| bad("too short"))? as usize;
        let mut blocks = Vec::with_capacity(count);
        for i in 0..count {
            let at = table + i * 12;
            let kind: [u8; 4] = data
                .get(at..at + 4)
                .and_then(|k| k.try_into().ok())
                .ok_or_else(|| bad("block table past the end"))?;
            // The offset counts from where it is stored.
            let offset = at + 4 + u32_at(data, at + 4).ok_or_else(|| bad("block table"))? as usize;
            let size = u32_at(data, at + 8).ok_or_else(|| bad("block table"))? as usize;
            if offset + size > data.len() {
                return Err(bad("block past the end of the file"));
            }
            blocks.push(Block { kind, offset, size });
        }
        Ok(Self {
            data,
            version,
            blocks,
        })
    }

    /// The first block of a type.
    #[must_use]
    pub fn block(&self, kind: &[u8; 4]) -> Option<&Block> {
        self.blocks.iter().find(|b| &b.kind == kind)
    }

    /// Every block of a type, in file order (a model has one `MDAT` per mesh).
    pub fn blocks_of<'s>(
        &'s self,
        kind: &'s [u8; 4],
    ) -> impl Iterator<Item = (usize, &'s Block)> + 's {
        self.blocks
            .iter()
            .enumerate()
            .filter(move |(_, b)| &b.kind == kind)
    }

    /// A block's bytes.
    #[must_use]
    pub fn bytes(&self, block: &Block) -> &'a [u8] {
        &self.data[block.offset..block.offset + block.size]
    }

    /// The first block of a type, parsed as binary KeyValues3.
    pub fn kv3(&self, kind: &[u8; 4]) -> Result<kv3::Value, String> {
        let block = self
            .block(kind)
            .ok_or_else(|| format!("resource: no {} block", String::from_utf8_lossy(kind)))?;
        kv3::parse(self.bytes(block))
    }

    /// The `DATA` block as KeyValues3, where most resource types keep their contents.
    pub fn data_kv3(&self) -> Result<kv3::Value, String> {
        self.kv3(b"DATA")
    }

    /// External references (`RERL`): the files this one names, by id.
    #[must_use]
    pub fn external_refs(&self) -> Vec<(u64, String)> {
        let Some(block) = self.block(b"RERL") else {
            return Vec::new();
        };
        let bytes = self.bytes(block);
        let (Some(list), Some(count)) = (u32_at(bytes, 0), u32_at(bytes, 4)) else {
            return Vec::new();
        };
        let list = list as usize;
        let mut refs = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let at = list + i * 16;
            let Some(id) = bytes
                .get(at..at + 8)
                .and_then(|b| b.try_into().ok())
                .map(u64::from_le_bytes)
            else {
                break;
            };
            let Some(name_at) = u32_at(bytes, at + 8).map(|o| at + 8 + o as usize) else {
                break;
            };
            let name = bytes
                .get(name_at..)
                .map(|rest| rest.split(|&c| c == 0).next().unwrap_or(&[]))
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .unwrap_or_else(String::new);
            refs.push((id, name));
        }
        refs
    }
}
