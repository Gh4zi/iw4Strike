//! Valve pack files (VPK v1/v2): a `*_dir.vpk` directory naming every file, whose bytes live in
//! the directory itself or in numbered `*_NNN.vpk` archives beside it.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const SIGNATURE: u32 = 0x55AA_1234;
/// Archive index meaning "the data follows the directory tree in the `_dir` file".
const IN_DIRECTORY: u16 = 0x7FFF;

#[derive(Clone, Debug)]
struct Entry {
    preload: Vec<u8>,
    archive: u16,
    offset: u32,
    length: u32,
}

#[derive(Debug)]
pub struct Vpk {
    dir: PathBuf,
    /// Where data stored in the directory file starts (header + tree).
    data_start: u64,
    entries: HashMap<String, Entry>,
}

impl Vpk {
    /// Open a `*_dir.vpk`. Paths are looked up lower-case with forward slashes.
    pub fn open(dir: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let u32_at = |at: usize| -> Result<u32, String> {
            bytes
                .get(at..at + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .ok_or_else(|| "vpk header truncated".to_owned())
        };
        if u32_at(0)? != SIGNATURE {
            return Err(format!("{}: not a VPK directory", dir.display()));
        }
        let version = u32_at(4)?;
        let tree_size = u32_at(8)? as usize;
        let header = match version {
            1 => 12,
            2 => 28,
            v => return Err(format!("VPK version {v} unsupported")),
        };
        let mut at = header;
        let end = (header + tree_size).min(bytes.len());
        let read_str = |at: &mut usize| -> Result<String, String> {
            let rest = bytes.get(*at..end).ok_or("vpk tree truncated")?;
            let len = rest
                .iter()
                .position(|&b| b == 0)
                .ok_or("vpk string unterminated")?;
            let s = String::from_utf8_lossy(&rest[..len]).into_owned();
            *at += len + 1;
            Ok(s)
        };
        let mut entries = HashMap::new();
        loop {
            let ext = read_str(&mut at)?;
            if ext.is_empty() {
                break;
            }
            loop {
                let path = read_str(&mut at)?;
                if path.is_empty() {
                    break;
                }
                loop {
                    let name = read_str(&mut at)?;
                    if name.is_empty() {
                        break;
                    }
                    let meta = bytes.get(at..at + 18).ok_or("vpk entry truncated")?;
                    let preload_len = usize::from(u16::from_le_bytes([meta[4], meta[5]]));
                    let entry = Entry {
                        archive: u16::from_le_bytes([meta[6], meta[7]]),
                        offset: u32::from_le_bytes([meta[8], meta[9], meta[10], meta[11]]),
                        length: u32::from_le_bytes([meta[12], meta[13], meta[14], meta[15]]),
                        preload: bytes
                            .get(at + 18..at + 18 + preload_len)
                            .ok_or("vpk preload truncated")?
                            .to_vec(),
                    };
                    at += 18 + preload_len;
                    let full = if path == " " {
                        format!("{name}.{ext}")
                    } else {
                        format!("{path}/{name}.{ext}")
                    };
                    entries.insert(full.to_ascii_lowercase(), entry);
                }
            }
        }
        Ok(Self {
            dir: dir.to_owned(),
            data_start: (header + tree_size) as u64,
            entries,
        })
    }

    #[must_use]
    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(&normalize(path))
    }

    /// Every file path in the pack (lower-case).
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// The bytes of `path`, read from whichever archive holds them.
    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        let entry = self.entries.get(&normalize(path))?;
        if entry.length == 0 {
            return Some(entry.preload.clone());
        }
        let (file, offset) = self.located(entry)?;
        read_entry(&mut File::open(file).ok()?, entry, offset)
    }

    /// The bytes of each of `paths` the pack holds, by path. Read in archive order with one open
    /// per archive and ascending offsets: on a hard drive that is one sweep, where `read` per file
    /// costs an open and a seek each (5-20 ms).
    pub fn read_sorted(&self, paths: &[String]) -> Vec<(String, Vec<u8>)> {
        let mut wanted: Vec<(&str, &Entry)> = paths
            .iter()
            .filter_map(|path| self.entries.get_key_value(&normalize(path)))
            .map(|(path, entry)| (path.as_str(), entry))
            .collect();
        wanted.sort_by_key(|(path, entry)| (entry.archive, entry.offset, *path));
        wanted.dedup_by_key(|(path, _)| *path);
        let mut open: Option<(u16, File)> = None;
        let mut out = Vec::with_capacity(wanted.len());
        for (path, entry) in wanted {
            if entry.length == 0 {
                out.push((path.to_owned(), entry.preload.clone()));
                continue;
            }
            let Some((file, offset)) = self.located(entry) else {
                continue;
            };
            if open.as_ref().is_none_or(|(archive, _)| *archive != entry.archive) {
                open = File::open(file).ok().map(|f| (entry.archive, f));
            }
            let Some((_, f)) = open.as_mut() else {
                continue;
            };
            if let Some(bytes) = read_entry(f, entry, offset) {
                out.push((path.to_owned(), bytes));
            }
        }
        out
    }

    /// The file holding `entry`'s bytes and where they start in it.
    fn located(&self, entry: &Entry) -> Option<(PathBuf, u64)> {
        if entry.archive == IN_DIRECTORY {
            return Some((self.dir.clone(), self.data_start + u64::from(entry.offset)));
        }
        let name = self.dir.file_name()?.to_str()?;
        let archive = name.replace("_dir.vpk", &format!("_{:03}.vpk", entry.archive));
        Some((self.dir.with_file_name(archive), u64::from(entry.offset)))
    }
}

fn read_entry(f: &mut File, entry: &Entry, offset: u64) -> Option<Vec<u8>> {
    let mut out = entry.preload.clone();
    f.seek(SeekFrom::Start(offset)).ok()?;
    let start = out.len();
    out.resize(start + entry.length as usize, 0);
    f.read_exact(&mut out[start..]).ok()?;
    Some(out)
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches('/')
        .to_ascii_lowercase()
}
