use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

use crate::discover::ensure_artifacts_dir;

fn cache_budget_bytes() -> u64 {
    static BUDGET: OnceLock<u64> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        std::env::var("IW4L_CACHE_BUDGET_MIB")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(4096)
            << 20
    })
}

pub fn cache_get(kind: &str, key: &str) -> Option<Vec<u8>> {
    let path = cache_path(kind, key).ok()?;
    fs::read(path).ok()
}

pub fn cache_put(kind: &str, key: &str, bytes: &[u8]) -> Result<(), String> {
    let path = cache_path(kind, key)?;
    let Some(parent) = path.parent() else {
        return Err(format!("cache path has no directory: {}", path.display()));
    };
    fs::create_dir_all(parent)
        .map_err(|error| format!("cache mkdir {}: {error}", parent.display()))?;

    let tmp = unique_temp(parent, key)?;
    let write = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(bytes)
    })();
    if let Err(error) = write {
        let _ = fs::remove_file(&tmp);
        return Err(format!("cache write {}: {error}", tmp.display()));
    }

    match fs::rename(&tmp, &path) {
        Ok(()) => {}

        Err(_) if path.exists() => {
            let _ = fs::remove_file(&tmp);
        }
        Err(error) => {
            let _ = fs::remove_file(&tmp);
            return Err(format!(
                "cache rename {} -> {}: {error}",
                tmp.display(),
                path.display()
            ));
        }
    }
    request_sweep();
    Ok(())
}

fn unique_temp(parent: &Path, key: &str) -> Result<PathBuf, String> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let n = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!("{key}.{}.{n:x}.tmp", std::process::id())))
}

pub struct CacheFlight {
    key: String,
}

fn inflight() -> &'static (Mutex<HashSet<String>>, Condvar) {
    static INFLIGHT: OnceLock<(Mutex<HashSet<String>>, Condvar)> = OnceLock::new();
    INFLIGHT.get_or_init(|| (Mutex::new(HashSet::new()), Condvar::new()))
}

#[must_use]
pub fn cache_flight(kind: &str, key: &str) -> CacheFlight {
    let key = format!("{kind}/{key}");
    let (lock, signal) = inflight();
    let mut live = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    while live.contains(&key) {
        live = signal
            .wait(live)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    live.insert(key.clone());
    CacheFlight { key }
}

impl Drop for CacheFlight {
    fn drop(&mut self) {
        let (lock, signal) = inflight();
        lock.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.key);
        signal.notify_all();
    }
}

/// Where the cache lives: `IW4L_CACHE_DIR` when set; the artifacts folder's `cache` when
/// `IW4L_ARTIFACTS_DIR` names one (a test run's own); on Linux the user's cache folder
/// (`$XDG_CACHE_HOME/iw4strike`, else `~/.cache/iw4strike`), on the system drive rather than
/// wherever the game was put (often a slow data drive, where every shader and mip read
/// stalled); elsewhere the artifacts folder's `cache`.
pub fn cache_root() -> Result<PathBuf, String> {
    let dir = match std::env::var_os("IW4L_CACHE_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => match user_cache_dir().filter(|_| std::env::var_os("IW4L_ARTIFACTS_DIR").is_none()) {
            Some(dir) => dir.join("iw4strike"),
            None => ensure_artifacts_dir()?.join("cache"),
        },
    };
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir)
}

#[cfg(target_os = "linux")]
fn user_cache_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
}

#[cfg(not(target_os = "linux"))]
fn user_cache_dir() -> Option<PathBuf> {
    None
}

fn cache_path(kind: &str, key: &str) -> Result<PathBuf, String> {
    if !kind_ok(kind) || !key_ok(key) {
        return Err("cache kind/key must be ascii [0-9a-z._-]".into());
    }
    Ok(cache_root()?
        .join(kind)
        .join(&key[..2.min(key.len())])
        .join(key))
}

fn kind_ok(kind: &str) -> bool {
    !kind.is_empty() && kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

fn key_ok(key: &str) -> bool {
    !key.is_empty()
        && key.len() < 200
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn request_sweep() {
    static SWEPT: std::sync::Once = std::sync::Once::new();
    SWEPT.call_once(|| {
        let Ok(root) = cache_root() else {
            return;
        };

        let _ = std::thread::Builder::new()
            .name("iw4l cache sweep".to_owned())
            .spawn(move || sweep(&root, cache_budget_bytes()));
    });
}

fn sweep(cache: &Path, budget: u64) {
    let mut entries = Vec::new();
    let mut total = 0u64;
    let stale_temp = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    let mut stack = vec![cache.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(listing) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in listing.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);

            if path.extension().is_some_and(|ext| ext == "tmp") {
                if modified < stale_temp {
                    let _ = fs::remove_file(&path);
                }
                continue;
            }
            total += meta.len();
            entries.push((modified, meta.len(), path));
        }
    }
    if total <= budget {
        return;
    }
    entries.sort_by_key(|(modified, _, _)| *modified);
    let mut freed = 0u64;
    let mut dropped = 0usize;
    for (_, len, path) in entries {
        if total - freed <= budget {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            freed += len;
            dropped += 1;
        }
    }
    diag::info!(
        Zone,
        "artifact cache sweep: {} MiB over {} MiB budget — evicted {dropped} files ({} MiB)",
        total >> 20,
        budget >> 20,
        freed >> 20
    );
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_more(0xcbf29ce484222325u64, bytes)
}

pub fn fnv1a64_more(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}
