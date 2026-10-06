//! The game folders the player selects in the game folders window, saved in `settings.cfg`
//! (`game_path_mw2`, `game_path_css`, `game_path_cs16`). An environment / `.env` override
//! (`IW4L_CSS`, `IW4L_CSTRIKE`) still wins over a saved folder; Steam is searched only when
//! neither names one.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GameFolder {
    Mw2,
    Css,
    Cs16,
}

impl GameFolder {
    pub const ALL: [Self; 3] = [Self::Mw2, Self::Css, Self::Cs16];

    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Mw2 => 0,
            Self::Css => 1,
            Self::Cs16 => 2,
        }
    }

    /// Its `settings.cfg` key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Mw2 => "game_path_mw2",
            Self::Css => "game_path_css",
            Self::Cs16 => "game_path_cs16",
        }
    }

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Mw2 => "Call of Duty: Modern Warfare 2",
            Self::Css => "Counter-Strike: Source",
            Self::Cs16 => "Counter-Strike 1.6",
        }
    }

    /// The folder the game reads, from a folder the player picked: MW2's install folder (the one
    /// with `zone`), or a Counter-Strike `cstrike` folder — the game's own folder is accepted
    /// too, as is a folder picked one level too deep.
    #[must_use]
    pub fn resolve(self, picked: &Path) -> Option<PathBuf> {
        #[cfg(windows)]
        let picked = &PathBuf::from(picked.to_string_lossy().replace('/', "\\"));
        let parent = picked.parent().map(Path::to_path_buf);
        let candidates = match self {
            Self::Mw2 => vec![Some(picked.to_path_buf()), parent],
            Self::Css | Self::Cs16 => vec![
                Some(picked.join("cstrike")),
                Some(picked.to_path_buf()),
                parent,
            ],
        };
        candidates.into_iter().flatten().find(|dir| self.holds_data(dir))
    }

    fn holds_data(self, dir: &Path) -> bool {
        match self {
            Self::Mw2 => crate::discover::holds_mw2(dir),
            Self::Css => dir.join(CSS_PAK).is_file(),
            Self::Cs16 => dir.join("models").join("v_ak47.mdl").is_file(),
        }
    }

    /// What [`resolve`](Self::resolve) looks for, for a "not found" message.
    #[must_use]
    pub const fn expected(self) -> &'static str {
        match self {
            Self::Mw2 => "zone\\english\\common_mp.ff",
            Self::Css => "cstrike\\cstrike_pak_dir.vpk",
            Self::Cs16 => "cstrike\\models\\v_ak47.mdl",
        }
    }
}

/// Counter-Strike: Source's main pack inside its `cstrike` folder.
pub const CSS_PAK: &str = "cstrike_pak_dir.vpk";

/// `settings.cfg`: `IW4L_SETTINGS_PATH`, else in the artifacts folder (Windows) or
/// `~/.config/iw4l` (elsewhere).
#[must_use]
pub fn settings_file(artifacts: &Path) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("IW4L_SETTINGS_PATH") {
        return Some(PathBuf::from(path));
    }
    if cfg!(windows) {
        return Some(artifacts.join("settings.cfg"));
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|path| PathBuf::from(path).join(".config")))
        .map(|path| path.join("iw4l/settings.cfg"))
}

fn default_settings_file() -> Option<PathBuf> {
    settings_file(&crate::discover::artifacts_dir())
}

/// The game-path lines of `settings.cfg`, `None` when the file has no such line at all.
#[must_use]
pub fn read_saved(file: &Path) -> Option<[String; 3]> {
    let text = std::fs::read_to_string(file).ok()?;
    let mut saved: [Option<String>; 3] = Default::default();
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        if let Some(folder) = GameFolder::ALL.into_iter().find(|f| f.key() == key.trim()) {
            saved[folder.index()] = Some(value.trim().to_owned());
        }
    }
    saved
        .iter()
        .any(Option::is_some)
        .then(|| saved.map(Option::unwrap_or_default))
}

fn saved_at_start() -> &'static Option<[String; 3]> {
    static SAVED: OnceLock<Option<[String; 3]>> = OnceLock::new();
    SAVED.get_or_init(|| default_settings_file().and_then(|file| read_saved(&file)))
}

/// Whether the player has confirmed the game folders once (the window wrote its keys).
#[must_use]
pub fn confirmed() -> bool {
    saved_at_start().is_some()
}

/// The folder saved for `folder`, resolved, when it still holds the game's data.
#[must_use]
pub fn saved(folder: GameFolder) -> Option<PathBuf> {
    let raw = &saved_at_start().as_ref()?[folder.index()];
    if raw.is_empty() {
        return None;
    }
    let found = folder.resolve(Path::new(raw));
    if found.is_none() {
        static WARNED: OnceLock<()> = OnceLock::new();
        WARNED.get_or_init(|| {
            diag::warn!(
                Zone,
                "saved {} folder {raw} no longer holds {}",
                folder.title(),
                folder.expected()
            );
        });
    }
    found
}

/// Writes the three game-path lines into `file`, keeping every other line.
pub fn write_saved(file: &Path, paths: &[String; 3]) -> Result<(), String> {
    let old = std::fs::read_to_string(file).unwrap_or_default();
    let mut lines = old
        .lines()
        .filter(|line| {
            line.split_once('=').is_none_or(|(key, _)| {
                !GameFolder::ALL.iter().any(|folder| folder.key() == key.trim())
            })
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let at = lines
        .iter()
        .position(|line| line.starts_with("bind ") || line == "unbindall")
        .unwrap_or(lines.len());
    for (offset, folder) in GameFolder::ALL.into_iter().enumerate() {
        let value = paths[folder.index()].replace(['\n', '\r'], "");
        lines.insert(at + offset, format!("{}={value}", folder.key()));
    }
    lines.push(String::new());
    if let Some(dir) = file.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let temporary = file.with_extension("cfg.tmp");
    std::fs::write(&temporary, lines.join("\n"))
        .and_then(|()| std::fs::rename(&temporary, file))
        .map_err(|error| format!("cannot save {}: {error}", file.display()))
}

/// Asks for a folder with the standard Windows folder picker; `None` when cancelled. Blocks the
/// calling thread until the dialog closes.
#[cfg(windows)]
#[must_use]
pub fn pick_folder(title: &str, start: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoTaskMemFree, CoUninitialize,
    };
    use windows::Win32::UI::Shell::{
        FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem,
        SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    };
    use windows::core::HSTRING;
    // SAFETY: COM is initialized on this thread for the duration of the calls and
    // uninitialized only when this call initialized it; the display name is freed once.
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = (|| -> windows::core::Result<Option<PathBuf>> {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            dialog.SetOptions(dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
            dialog.SetTitle(&HSTRING::from(title))?;
            if let Some(dir) = start.filter(|dir| dir.is_dir())
                && let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(
                    &HSTRING::from(dir.as_os_str()),
                    None,
                )
            {
                dialog.SetFolder(&item)?;
            }
            if dialog.Show(None).is_err() {
                return Ok(None);
            }
            let name = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = name.to_string();
            CoTaskMemFree(Some(name.0 as *const _));
            Ok(path.ok().map(PathBuf::from))
        })();
        if initialized {
            CoUninitialize();
        }
        result.unwrap_or_else(|error| {
            diag::warn!(Zone, "folder picker failed: {error}");
            None
        })
    }
}

#[cfg(not(windows))]
#[must_use]
pub fn pick_folder(_title: &str, _start: Option<&Path>) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_paths_replace_their_lines_and_keep_the_rest() {
        let dir = std::env::temp_dir().join(format!("iw4l-game-paths-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.cfg");
        std::fs::write(&file, "fov=80\ngame_path_css=old\nbind G drop\n").unwrap();
        assert_eq!(
            read_saved(&file),
            Some([String::new(), "old".into(), String::new()])
        );
        write_saved(
            &file,
            &["C:/MW2".into(), String::new(), "D:/Half-Life/cstrike".into()],
        )
        .unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            text,
            "fov=80\ngame_path_mw2=C:/MW2\ngame_path_css=\ngame_path_cs16=D:/Half-Life/cstrike\nbind G drop\n"
        );
        assert_eq!(
            read_saved(&file),
            Some(["C:/MW2".into(), String::new(), "D:/Half-Life/cstrike".into()])
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn counter_strike_folders_resolve_to_cstrike() {
        let dir = std::env::temp_dir().join(format!("iw4l-cs-folder-{}", std::process::id()));
        let cstrike = dir.join("Half-Life").join("cstrike");
        std::fs::create_dir_all(cstrike.join("models")).unwrap();
        std::fs::write(cstrike.join("models").join("v_ak47.mdl"), b"IDST").unwrap();
        let top = dir.join("Half-Life");
        assert_eq!(GameFolder::Cs16.resolve(&top), Some(cstrike.clone()));
        assert_eq!(GameFolder::Cs16.resolve(&cstrike), Some(cstrike.clone()));
        assert_eq!(
            GameFolder::Cs16.resolve(&cstrike.join("models")),
            Some(cstrike.clone())
        );
        assert_eq!(GameFolder::Css.resolve(&top), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
