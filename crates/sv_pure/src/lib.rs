//! File consistency (`sv_pure`).
//!
//! Players read Counter-Strike models, materials, sounds and HUD art from their own installs of
//! CS:GO, CS2, CS:S, CS 1.6 and Condition Zero. A changed file there (a see-through smoke sprite,
//! a bright player skin, louder footsteps) is an advantage only that player has. Every such file
//! this process reads is noted here, by game and path, with a hash of its bytes; a client sends
//! its notes to the server it plays on, which hashes its own copy of each file and removes a
//! player whose file differs (or only logs it, as the server's `sv_pure` says).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, Mutex, PoisonError};

/// `sv_pure 0`: no checks.
pub const MODE_OFF: u8 = 0;
/// `sv_pure 1`: a player whose files differ is named in the server's console and log.
pub const MODE_LOG: u8 = 1;
/// `sv_pure 2` (the default): a player whose files differ is removed, told which file and why.
pub const MODE_KICK: u8 = 2;

static MODE: AtomicU8 = AtomicU8::new(MODE_KICK);

/// This server's `sv_pure`.
#[must_use]
pub fn mode() -> u8 {
    MODE.load(Ordering::Relaxed)
}

pub fn set_mode(mode: u8) {
    MODE.store(mode.min(MODE_KICK), Ordering::Relaxed);
}

/// The games whose files are checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Game {
    Csgo,
    Cs2,
    Css,
    Cs16,
    Cz,
}

impl Game {
    pub const ALL: [Self; 5] = [Self::Csgo, Self::Cs2, Self::Css, Self::Cs16, Self::Cz];

    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Csgo => 0,
            Self::Cs2 => 1,
            Self::Css => 2,
            Self::Cs16 => 3,
            Self::Cz => 4,
        }
    }

    #[must_use]
    pub const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Csgo),
            1 => Some(Self::Cs2),
            2 => Some(Self::Css),
            3 => Some(Self::Cs16),
            4 => Some(Self::Cz),
            _ => None,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Csgo => "CS:GO",
            Self::Cs2 => "CS2",
            Self::Css => "CS:S",
            Self::Cs16 => "CS 1.6",
            Self::Cz => "Condition Zero",
        }
    }
}

/// A file as a server finds it in its own install of the same game: the pack holding it (its
/// `*_dir.vpk` as a path from the game's install folder, `csgo/pak01_dir.vpk`; empty for a loose
/// file) and its path (in the pack, lower-case; for a loose file, from the install folder,
/// `cstrike/sprites/hud.txt`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileKey {
    pub game: Game,
    pub pack: String,
    pub path: String,
}

impl FileKey {
    /// `CS:GO models/player/...mdl`, as a player is told.
    #[must_use]
    pub fn describe(&self) -> String {
        format!("{} {}", self.game.name(), self.path)
    }
}

/// The folders of each game's install a checked file may come from (a server reads only there,
/// whatever path a player sends).
const fn game_folders(game: Game) -> &'static [&'static str] {
    match game {
        Game::Csgo => &["csgo/"],
        Game::Cs2 => &["game/csgo/"],
        Game::Css => &["cstrike/", "hl2/", "platform/"],
        // CS 1.6 also reads Half-Life's own `valve` folder beside it (its grenade blasts).
        Game::Cs16 => &["cstrike/", "valve/"],
        Game::Cz => &["czero/"],
    }
}

fn plain_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with(['/', '\\'])
        && !path.contains(':')
        && path.split(['/', '\\']).all(|part| !part.is_empty() && part != "." && part != "..")
}

impl FileKey {
    /// Whether a server may look this file up: a plain path inside the game's own folders (a
    /// pack there named `*_dir.vpk`), never `..`, a drive or an absolute path.
    #[must_use]
    pub fn plausible(&self) -> bool {
        let in_game = |path: &str| {
            plain_relative(path)
                && game_folders(self.game)
                    .iter()
                    .any(|folder| path.to_ascii_lowercase().starts_with(folder))
        };
        if self.pack.is_empty() {
            in_game(&self.path)
        } else {
            in_game(&self.pack)
                && self.pack.to_ascii_lowercase().ends_with("_dir.vpk")
                && plain_relative(&self.path)
        }
    }
}

pub const DIGEST_BYTES: usize = 16;

/// The first 128 bits of a file's BLAKE3 hash.
pub type Digest = [u8; DIGEST_BYTES];

#[must_use]
pub fn digest(bytes: &[u8]) -> Digest {
    let mut out = [0; DIGEST_BYTES];
    out.copy_from_slice(&blake3::hash(bytes).as_bytes()[..DIGEST_BYTES]);
    out
}

fn name_of(path: &Path) -> Option<String> {
    Some(path.file_name()?.to_str()?.to_ascii_lowercase())
}

/// A CS:S install: the folder holding `cstrike/cstrike_pak_dir.vpk`.
fn is_css_install(dir: &Path) -> bool {
    dir.join("cstrike").join("cstrike_pak_dir.vpk").is_file()
}

/// The game of the pack `dir` (a `*_dir.vpk`) and its path from that game's install folder:
/// `<install>/csgo/pak01_dir.vpk` is CS:GO's, `<install>/game/csgo/pak01_dir.vpk` CS2's, and the
/// packs of a CS:S install's `cstrike`, `hl2` and `platform` folders CS:S's.
#[must_use]
pub fn locate_pack(dir: &Path) -> Option<(Game, String)> {
    let file = name_of(dir)?;
    let folder = dir.parent()?;
    let folder_name = name_of(folder)?;
    let install = folder.parent()?;
    match folder_name.as_str() {
        "csgo" if name_of(install).as_deref() == Some("game") => {
            Some((Game::Cs2, format!("game/csgo/{file}")))
        }
        "csgo" => Some((Game::Csgo, format!("csgo/{file}"))),
        "cstrike" | "hl2" | "platform" if is_css_install(install) => {
            Some((Game::Css, format!("{folder_name}/{file}")))
        }
        _ => None,
    }
}

/// The game of the loose file `path` and its path from that game's install folder (separators
/// `/`, case kept): under `czero` Condition Zero's, under a CS:S install's `cstrike` or `hl2`
/// CS:S's, under any other `cstrike` (or Half-Life's `valve`) CS 1.6's, under `game/csgo` CS2's
/// and under `csgo` CS:GO's.
#[must_use]
pub fn locate_file(path: &Path) -> Option<(Game, String)> {
    for folder in path.ancestors().skip(1) {
        let Some(name) = name_of(folder) else {
            break;
        };
        let parent = folder.parent()?;
        let (game, install) = match name.as_str() {
            "czero" => (Game::Cz, parent),
            "cstrike" | "hl2" if is_css_install(parent) => (Game::Css, parent),
            "cstrike" | "valve" => (Game::Cs16, parent),
            "csgo" if name_of(parent).as_deref() == Some("game") => (Game::Cs2, parent.parent()?),
            "csgo" => (Game::Csgo, parent),
            _ => continue,
        };
        let relative = path.strip_prefix(install).ok()?;
        let relative: Vec<String> = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        return Some((game, relative.join("/")));
    }
    None
}

#[derive(Default)]
struct Ledger {
    /// Every file read, in order; a file read again with other bytes is listed again.
    files: Vec<(FileKey, Digest)>,
    latest: HashMap<FileKey, Digest>,
}

static LEDGER: LazyLock<Mutex<Ledger>> = LazyLock::new(Mutex::default);

fn ledger() -> std::sync::MutexGuard<'static, Ledger> {
    LEDGER.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Notes that this process read `bytes` as `key`.
pub fn note(key: FileKey, bytes: &[u8]) {
    let digest = digest(bytes);
    let mut ledger = ledger();
    if ledger.latest.get(&key) == Some(&digest) {
        return;
    }
    ledger.latest.insert(key.clone(), digest);
    ledger.files.push((key, digest));
}

/// Notes a loose file read from a CS game's folder (any other file is not noted).
pub fn note_file(path: &Path, bytes: &[u8]) {
    if let Some((game, path)) = locate_file(path) {
        note(
            FileKey {
                game,
                pack: String::new(),
                path,
            },
            bytes,
        );
    }
}

/// `std::fs::read`, noting the file when it is a CS game's.
pub fn read_file(path: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)?;
    note_file(path, &bytes);
    Ok(bytes)
}

/// How many files have been noted.
#[must_use]
pub fn noted() -> usize {
    ledger().files.len()
}

/// Up to `max` of the noted files, from the `from`th on.
#[must_use]
pub fn noted_since(from: usize, max: usize) -> Vec<(FileKey, Digest)> {
    let ledger = ledger();
    ledger
        .files
        .iter()
        .skip(from)
        .take(max)
        .cloned()
        .collect()
}

/// The hash of `key`'s bytes as this process last read them.
#[must_use]
pub fn noted_digest(key: &FileKey) -> Option<Digest> {
    ledger().latest.get(key).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sv_pure_test_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn packs_are_told_apart_by_their_folders() {
        let legacy = Path::new("X:/games/csgo legacy/csgo/pak01_dir.vpk");
        assert_eq!(
            locate_pack(legacy),
            Some((Game::Csgo, "csgo/pak01_dir.vpk".to_owned()))
        );
        let cs2 = Path::new("C:/Steam/Counter-Strike Global Offensive/game/csgo/pak01_dir.vpk");
        assert_eq!(
            locate_pack(cs2),
            Some((Game::Cs2, "game/csgo/pak01_dir.vpk".to_owned()))
        );
        let css = scratch("css");
        std::fs::create_dir_all(css.join("cstrike")).unwrap();
        std::fs::create_dir_all(css.join("hl2")).unwrap();
        std::fs::write(css.join("cstrike/cstrike_pak_dir.vpk"), b"x").unwrap();
        assert_eq!(
            locate_pack(&css.join("cstrike/cstrike_pak_dir.vpk")),
            Some((Game::Css, "cstrike/cstrike_pak_dir.vpk".to_owned()))
        );
        assert_eq!(
            locate_pack(&css.join("hl2/hl2_sound_misc_dir.vpk")),
            Some((Game::Css, "hl2/hl2_sound_misc_dir.vpk".to_owned()))
        );
        assert_eq!(locate_pack(Path::new("C:/elsewhere/hl2/x_dir.vpk")), None);
        let _ = std::fs::remove_dir_all(css);
    }

    #[test]
    fn loose_files_are_told_apart_by_their_folders() {
        let half_life = scratch("goldsrc");
        let sprite = half_life.join("cstrike").join("sprites").join("hud.txt");
        assert_eq!(
            locate_file(&sprite),
            Some((Game::Cs16, "cstrike/sprites/hud.txt".to_owned()))
        );
        let model = half_life.join("czero").join("models").join("v_ak47.mdl");
        assert_eq!(
            locate_file(&model),
            Some((Game::Cz, "czero/models/v_ak47.mdl".to_owned()))
        );
        let css = scratch("css_loose");
        std::fs::create_dir_all(css.join("cstrike")).unwrap();
        std::fs::write(css.join("cstrike/cstrike_pak_dir.vpk"), b"x").unwrap();
        assert_eq!(
            locate_file(&css.join("cstrike").join("sound").join("a.wav")),
            Some((Game::Css, "cstrike/sound/a.wav".to_owned()))
        );
        assert_eq!(locate_file(Path::new("C:/games/mw2/main/iw_00.iwd")), None);
        let _ = std::fs::remove_dir_all(half_life);
        let _ = std::fs::remove_dir_all(css);
    }

    #[test]
    fn only_paths_inside_the_game_folders_are_plausible() {
        let key = |game, pack: &str, path: &str| FileKey {
            game,
            pack: pack.to_owned(),
            path: path.to_owned(),
        };
        assert!(key(Game::Csgo, "csgo/pak01_dir.vpk", "models/a.mdl").plausible());
        assert!(key(Game::Cs16, "", "cstrike/sprites/hud.txt").plausible());
        assert!(key(Game::Css, "hl2/hl2_sound_misc_dir.vpk", "sound/a.wav").plausible());
        assert!(!key(Game::Cs16, "", "cstrike/../../secret.txt").plausible());
        assert!(!key(Game::Cs16, "", "C:/Windows/win.ini").plausible());
        assert!(!key(Game::Cz, "", "cstrike/models/a.mdl").plausible());
        assert!(!key(Game::Csgo, "csgo/../x_dir.vpk", "a").plausible());
        assert!(!key(Game::Csgo, "csgo/pak01_000.vpk", "a").plausible());
        assert!(!key(Game::Csgo, "csgo/pak01_dir.vpk", "../a").plausible());
    }

    #[test]
    fn a_file_is_noted_again_only_with_other_bytes() {
        let key = FileKey {
            game: Game::Cs16,
            pack: String::new(),
            path: "cstrike/test_only/ledger.txt".to_owned(),
        };
        let before = noted();
        note(key.clone(), b"one");
        note(key.clone(), b"one");
        note(key.clone(), b"two");
        let since = noted_since(before, 10);
        let ours: Vec<_> = since.iter().filter(|(k, _)| *k == key).collect();
        assert_eq!(ours.len(), 2);
        assert_eq!(noted_digest(&key), Some(digest(b"two")));
    }
}
