use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ZoneGame;
use crate::discover::{GamesRoot, env_or_dotenv};
use crate::discover::{search_roots, zone_game_for_path};
use crate::game_paths::{CSS_PAK, GameFolder, saved};

/// A game shortcut's file name in the games root: a Windows shortcut (`.lnk`), or on Linux a
/// symbolic link to the game folder.
macro_rules! shortcut {
    ($name:literal) => {
        if cfg!(windows) {
            concat!($name, ".lnk")
        } else {
            $name
        }
    };
}

pub const MW2_SHORTCUT: &str = shortcut!("Modern Warfare 2");

#[derive(Clone, Debug, Default)]
pub struct SteamProbe {
    pub steam_found: bool,
    pub tried: Vec<(ZoneGame, PathBuf, SteamCandidate)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SteamCandidate {
    Missing,
    NoMultiplayerData,
    Linked,
    OneOfSeveral,
    ShortcutFailed(String),
}

pub fn link_steam_games(root: &GamesRoot) -> SteamProbe {
    let mut probe = SteamProbe::default();
    let roots = search_roots(&root.0);
    let titles = [
        (ZoneGame::Iw4, MW2_SHORTCUT, "Call of Duty Modern Warfare 2"),
        (
            ZoneGame::Iw5,
            shortcut!("Modern Warfare 3"),
            "Call of Duty Modern Warfare 3",
        ),
        (
            ZoneGame::T5,
            shortcut!("Black Ops"),
            "Call of Duty Black Ops",
        ),
    ];
    let only_mw2 = crate::discover::only_mw2();
    let missing = titles
        .into_iter()
        .filter(|(game, _, _)| !only_mw2 || *game == ZoneGame::Iw4)
        .filter(|(game, shortcut, _)| !root.0.join(shortcut).exists() && !has_game(&roots, *game))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return probe;
    }
    let libraries = steam_libraries();
    probe.steam_found = !libraries.is_empty();
    for (game, shortcut, folder) in missing {
        let link = root.0.join(shortcut);
        let mut valid = Vec::new();
        for library in &libraries {
            let path = library.join("steamapps").join("common").join(folder);
            let candidate = if !path.is_dir() {
                SteamCandidate::Missing
            } else if has_game(std::slice::from_ref(&path), game) {
                valid.push(probe.tried.len());
                SteamCandidate::Linked
            } else {
                SteamCandidate::NoMultiplayerData
            };
            probe.tried.push((game, path, candidate));
        }
        match valid[..] {
            [index] => {
                let (_, target, candidate) = &mut probe.tried[index];
                match create_shortcut(&link, target) {
                    Ok(()) => diag::info!(
                        Zone,
                        "linked Steam {} as {}",
                        target.display(),
                        link.display()
                    ),
                    Err(error) => {
                        diag::warn!(Zone, "{error}");
                        *candidate = SteamCandidate::ShortcutFailed(error);
                    }
                }
            }
            [] => {}
            _ => {
                diag::warn!(
                    Zone,
                    "several Steam installs of {folder}; create {shortcut} to select one"
                );
                for index in valid {
                    probe.tried[index].2 = SteamCandidate::OneOfSeveral;
                }
            }
        }
    }
    probe
}

/// Environment override naming a Counter-Strike: Source `cstrike` folder.
pub const CSS_ENV: &str = "IW4L_CSS";

/// Counter-Strike: Source's main pack (`cstrike/cstrike_pak_dir.vpk`): `IW4L_CSS` (its `cstrike`
/// folder, from the environment or `.env`) when set, else the folder saved in the game folders
/// window, else `steamapps/common/Counter-Strike Source/cstrike` in any Steam library — unless
/// the player cleared it in the window.
#[must_use]
pub fn find_css_pak() -> Option<PathBuf> {
    // Turned off in the game folders window: not used, whatever names it.
    if crate::game_paths::turned_off(GameFolder::Css) {
        return None;
    }
    if let Some(dir) = css_env_override() {
        return css_pak_in(Path::new(&dir));
    }
    saved(GameFolder::Css)
        .and_then(|dir| css_pak_in(&dir))
        .or_else(steam_css_pak)
}

/// `IW4L_CSS` from the environment or `.env`, when set.
#[must_use]
pub fn css_env_override() -> Option<String> {
    env_or_dotenv(CSS_ENV).filter(|dir| !dir.trim().is_empty())
}

fn css_pak_in(dir: &Path) -> Option<PathBuf> {
    let file = dir.join(CSS_PAK);
    file.is_file().then_some(file)
}

/// Counter-Strike: Source's pack in a Steam library.
#[must_use]
pub fn steam_css_pak() -> Option<PathBuf> {
    for library in steam_libraries() {
        let dir = library
            .join("steamapps")
            .join("common")
            .join("Counter-Strike Source")
            .join("cstrike");
        if let Some(found) = css_pak_in(&dir) {
            return Some(found);
        }
    }
    None
}

/// Environment override naming Counter-Strike 2's `game/csgo` folder.
pub const CS2_ENV: &str = "IW4L_CS2";

/// `IW4L_CS2` from the environment or `.env`, when set.
#[must_use]
pub fn cs2_env_override() -> Option<String> {
    env_or_dotenv(CS2_ENV).filter(|dir| !dir.trim().is_empty())
}

/// Counter-Strike 2's main pack (`game/csgo/pak01_dir.vpk`): `IW4L_CS2` (its `game/csgo`
/// folder) when set, else the folder saved in the game folders window, else
/// `steamapps/common/Counter-Strike Global Offensive/game/csgo` in any Steam library — unless the
/// player turned it off in the window.
#[must_use]
pub fn find_cs2_pak() -> Option<PathBuf> {
    if crate::game_paths::turned_off(GameFolder::Cs2) {
        return None;
    }
    let pak_in = |dir: &Path| {
        crate::game_paths::is_cs2_game_folder(dir).then(|| dir.join(crate::game_paths::CS2_PAK))
    };
    if let Some(dir) = cs2_env_override() {
        return pak_in(Path::new(&dir));
    }
    saved(GameFolder::Cs2)
        .and_then(|dir| pak_in(&dir))
        .or_else(steam_cs2_pak)
}

/// Counter-Strike 2's pack in a Steam library (CS2 installs where CS:GO did).
#[must_use]
pub fn steam_cs2_pak() -> Option<PathBuf> {
    steam_libraries().into_iter().find_map(|library| {
        let dir = library
            .join("steamapps")
            .join("common")
            .join("Counter-Strike Global Offensive")
            .join("game")
            .join("csgo");
        crate::game_paths::is_cs2_game_folder(&dir).then(|| dir.join(crate::game_paths::CS2_PAK))
    })
}

/// Environment override naming a Counter-Strike 1.6 `cstrike` folder.
pub const CSTRIKE_ENV: &str = "IW4L_CSTRIKE";

/// `IW4L_CSTRIKE` from the environment or `.env`, when set.
#[must_use]
pub fn cstrike_env_override() -> Option<String> {
    env_or_dotenv(CSTRIKE_ENV).filter(|dir| !dir.trim().is_empty())
}

/// The Counter-Strike 1.6 `cstrike` folder whose models and sounds IW4L reads at runtime, the
/// stand-in for a missing Counter-Strike: Source. It is never searched for: the player selects
/// it in the game folders window (or with `IW4L_CSTRIKE`, environment or `.env`, which wins),
/// and it is ignored whenever CS:S is found. Nothing is copied.
#[must_use]
pub fn find_cstrike() -> Option<PathBuf> {
    if find_css_pak().is_some() || crate::game_paths::turned_off(GameFolder::Cs16) {
        return None;
    }
    if let Some(dir) = cstrike_env_override() {
        let dir = PathBuf::from(dir);
        return dir.join("models").is_dir().then_some(dir);
    }
    saved(GameFolder::Cs16)
}

/// Environment override naming a Counter-Strike: Condition Zero `czero` folder.
pub const CZERO_ENV: &str = "IW4L_CZERO";

/// `IW4L_CZERO` from the environment or `.env`, when set.
#[must_use]
pub fn czero_env_override() -> Option<String> {
    env_or_dotenv(CZERO_ENV).filter(|dir| !dir.trim().is_empty())
}

/// Condition Zero's `czero` folder, chosen like Counter-Strike 1.6's (the game folders window,
/// or `IW4L_CZERO`, which wins) and likewise ignored whenever CS:S is found.
#[must_use]
pub fn find_czero() -> Option<PathBuf> {
    if find_css_pak().is_some() || crate::game_paths::turned_off(GameFolder::Cz) {
        return None;
    }
    if let Some(dir) = czero_env_override() {
        let dir = PathBuf::from(dir);
        return dir.join("models").is_dir().then_some(dir);
    }
    saved(GameFolder::Cz)
}

/// The GoldSrc Counter-Strike folders IW4L reads when there is no CS:S, best first: Condition
/// Zero's `czero`, then Counter-Strike 1.6's `cstrike`. CZ falls back to `cstrike` for what it
/// lacks (its HUD sprites, a few models), so with CZ on and 1.6 off or unset, the `cstrike` beside
/// `czero` still fills in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoldSrcDirs(pub Vec<PathBuf>);

impl GoldSrcDirs {
    /// The first of the folders holding `relative` (`sprites/hud.txt`).
    #[must_use]
    pub fn file(&self, relative: &str) -> Option<PathBuf> {
        self.0
            .iter()
            .map(|dir| dir.join(relative))
            .find(|path| path.is_file())
    }

    /// `relative`'s bytes from the first folder holding it.
    #[must_use]
    pub fn read(&self, relative: &str) -> Option<Vec<u8>> {
        self.file(relative)
            .and_then(|path| std::fs::read(path).ok())
    }

    /// Whether Condition Zero is among them.
    #[must_use]
    pub fn has_czero(&self) -> bool {
        self.0
            .first()
            .is_some_and(|dir| crate::game_paths::is_condition_zero(dir))
    }
}

/// The GoldSrc folders to read (see [`GoldSrcDirs`]); `None` with CS:S found or neither GoldSrc
/// game on.
#[must_use]
pub fn find_goldsrc() -> Option<GoldSrcDirs> {
    let czero = find_czero();
    let cstrike = find_cstrike().or_else(|| {
        let beside = czero.as_ref()?.parent()?.join("cstrike");
        beside.join("models").is_dir().then_some(beside)
    });
    let dirs: Vec<PathBuf> = czero.into_iter().chain(cstrike).collect();
    (!dirs.is_empty()).then_some(GoldSrcDirs(dirs))
}

fn has_game(roots: &[PathBuf], game: ZoneGame) -> bool {
    roots.iter().any(|root| {
        std::iter::once(root.clone())
            .chain(subdirs(root))
            .flat_map(|tree| {
                let zone = tree.join("zone");
                std::iter::once(zone.clone()).chain(subdirs(&zone))
            })
            .any(|language| zone_game_for_path(&language.join("common_mp.ff")) == Some(game))
    })
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect(),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                diag::warn!(Zone, "game probe {}: {error}", dir.display());
            }
            Vec::new()
        }
    }
}

#[cfg(windows)]
fn create_shortcut(link: &Path, target: &Path) -> Result<(), String> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize, IPersistFile,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface};
    // SAFETY: COM is initialized on this thread for the duration of the calls and
    // uninitialized only when this call initialized it.
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = (|| -> windows::core::Result<()> {
            let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            shell_link.SetPath(&HSTRING::from(target.as_os_str()))?;
            shell_link
                .cast::<IPersistFile>()?
                .Save(&HSTRING::from(link.as_os_str()), true)
        })();
        if initialized {
            CoUninitialize();
        }
        result.map_err(|error| format!("cannot create {}: {error}", link.display()))
    }
}

#[cfg(unix)]
fn create_shortcut(link: &Path, target: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(target, link)
        .map_err(|error| format!("cannot create {}: {error}", link.display()))
}

#[cfg(not(any(windows, unix)))]
fn create_shortcut(link: &Path, _target: &Path) -> Result<(), String> {
    Err(format!("cannot create {} on this system", link.display()))
}

/// Steam's own folders on this PC: the registry's paths and the default installs.
#[cfg(windows)]
fn steam_installs() -> Vec<PathBuf> {
    let mut installs = Vec::new();
    for install in [
        registry_string(true, "Software\\Valve\\Steam", "SteamPath"),
        registry_string(false, "SOFTWARE\\WOW6432Node\\Valve\\Steam", "InstallPath"),
        Some(PathBuf::from("C:\\Program Files (x86)\\Steam")),
        Some(PathBuf::from("C:\\Steam")),
    ]
    .into_iter()
    .flatten()
    {
        let install = PathBuf::from(install.to_string_lossy().replace('/', "\\"));
        if install.is_dir() && !installs.contains(&install) {
            installs.push(install);
        }
    }
    installs
}

/// Steam's own folders on this PC: the native install (`~/.local/share/Steam`, also reached
/// through `~/.steam/steam`), Flatpak's (in the app's home, or its data folder on some
/// setups) and Snap's.
#[cfg(not(windows))]
fn steam_installs() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let xdg_data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    [
        xdg_data.join("Steam"),
        home.join(".steam/steam"),
        home.join(".steam/root"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/data/Steam"),
        home.join("snap/steam/common/.local/share/Steam"),
    ]
    .into_iter()
    .filter(|install| install.join("steamapps").is_dir())
    .collect()
}

/// Every Steam library: each install and the folders its `libraryfolders.vdf` lists, once each.
fn steam_libraries() -> Vec<PathBuf> {
    let installs = steam_installs();
    let mut libraries: Vec<PathBuf> = Vec::new();
    let mut seen = HashSet::new();
    for install in &installs {
        let vdf = install.join("steamapps").join("libraryfolders.vdf");
        let listed = match std::fs::read_to_string(&vdf) {
            Ok(text) => steam_library_paths(&text),
            Err(error) => {
                diag::warn!(Zone, "steam libraries {}: {error}", vdf.display());
                Vec::new()
            }
        };
        for library in std::iter::once(install.clone()).chain(listed) {
            if let Ok(canonical) = std::fs::canonicalize(&library)
                && seen.insert(canonical)
            {
                libraries.push(library);
            }
        }
    }
    libraries
}

fn steam_library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| line.trim().strip_prefix("\"path\""))
        .filter_map(|rest| {
            let value = rest.trim().strip_prefix('"')?.strip_suffix('"')?;
            Some(PathBuf::from(value.replace("\\\\", "\\")))
        })
        .collect()
}

#[cfg(windows)]
fn registry_string(current_user: bool, key: &str, value: &str) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
    };
    let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (key, value) = (wide(key), wide(value));
    let hive = if current_user {
        HKEY_CURRENT_USER
    } else {
        HKEY_LOCAL_MACHINE
    };
    let mut buffer = [0u16; 1024];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: both names are NUL-terminated and `bytes` is the buffer's size in bytes.
    let status = unsafe {
        RegGetValueW(
            hive,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != 0 {
        return None;
    }
    let len = (bytes as usize / 2).saturating_sub(1);
    Some(PathBuf::from(std::ffi::OsString::from_wide(&buffer[..len])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_paths_read_windows_and_linux_vdf() {
        let vdf = r#""libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
	}
	"1"
	{
		"path"		"/home/player/Games/SteamLibrary"
		"label"		""
	}
}
"#;
        assert_eq!(
            steam_library_paths(vdf),
            [
                PathBuf::from(r"C:\Program Files (x86)\Steam"),
                PathBuf::from("/home/player/Games/SteamLibrary"),
            ]
        );
    }
}
