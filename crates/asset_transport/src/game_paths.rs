//! The game folders the player selects in the game folders window, saved in `settings.cfg`
//! (`game_path_mw2`, `game_path_css`, `game_path_cz`, `game_path_cs16`), and whether each
//! Counter-Strike game is used at all (`use_css`, `use_cz`, `use_cs16`, the window's "Use"
//! boxes). An environment / `.env` override (`IW4L_CSS`, `IW4L_CZERO`, `IW4L_CSTRIKE`) wins over
//! a saved folder; Steam is searched only for CS:S. A game turned off is not used whatever names
//! it. CS:S wins over both GoldSrc games; without it Condition Zero is read first and
//! Counter-Strike 1.6 fills in what CZ lacks, as CZ itself does.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GameFolder {
    Mw2,
    Css,
    Cs16,
    Cz,
}

impl GameFolder {
    /// How many there are: the length of the arrays indexed by [`index`](Self::index).
    pub const COUNT: usize = 4;
    /// In the order the game folders window lists them, which is the order they are used in, and
    /// [`index`](Self::index) order (arrays built by mapping over this are indexed by it).
    pub const ALL: [Self; Self::COUNT] = [Self::Mw2, Self::Css, Self::Cz, Self::Cs16];

    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Mw2 => 0,
            Self::Css => 1,
            Self::Cz => 2,
            Self::Cs16 => 3,
        }
    }

    /// Its `settings.cfg` key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Mw2 => "game_path_mw2",
            Self::Css => "game_path_css",
            Self::Cs16 => "game_path_cs16",
            Self::Cz => "game_path_cz",
        }
    }

    /// Its `settings.cfg` on/off key (the game folders window's "Use" box); MW2 has none.
    #[must_use]
    pub const fn use_key(self) -> Option<&'static str> {
        match self {
            Self::Mw2 => None,
            Self::Css => Some("use_css"),
            Self::Cs16 => Some("use_cs16"),
            Self::Cz => Some("use_cz"),
        }
    }

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Mw2 => "Call of Duty: Modern Warfare 2",
            Self::Css => "Counter-Strike: Source",
            Self::Cs16 => "Counter-Strike 1.6",
            Self::Cz => "Counter-Strike: Condition Zero",
        }
    }

    /// The folder the game reads, from a folder the player picked: MW2's install folder (the one
    /// with `zone`), or a Counter-Strike `cstrike` (CZ: `czero`) folder — the game's own folder
    /// is accepted too, as is a folder picked one level too deep.
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
            Self::Cz => vec![
                Some(picked.join("czero")),
                Some(picked.to_path_buf()),
                parent,
            ],
        };
        candidates
            .into_iter()
            .flatten()
            .find(|dir| self.holds_data(dir))
    }

    fn holds_data(self, dir: &Path) -> bool {
        let goldsrc = || dir.join("models").join("v_ak47.mdl").is_file();
        match self {
            Self::Mw2 => crate::discover::holds_mw2(dir),
            Self::Css => dir.join(CSS_PAK).is_file(),
            Self::Cs16 => goldsrc() && !is_condition_zero(dir),
            Self::Cz => goldsrc() && is_condition_zero(dir),
        }
    }

    /// What [`resolve`](Self::resolve) looks for, for a "not found" message.
    #[must_use]
    pub const fn expected(self) -> &'static str {
        match self {
            Self::Mw2 => "zone\\english\\common_mp.ff",
            Self::Css => "cstrike\\cstrike_pak_dir.vpk",
            Self::Cs16 => "cstrike\\models\\v_ak47.mdl",
            Self::Cz => "czero\\models\\v_ak47.mdl",
        }
    }
}

/// Whether `dir` is Condition Zero's game folder: its `liblist.gam` names the game so.
#[must_use]
pub fn is_condition_zero(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("liblist.gam")).is_ok_and(|list| {
        list.lines().any(|line| {
            let mut words = line.split_whitespace();
            words
                .next()
                .is_some_and(|key| key.eq_ignore_ascii_case("game"))
                && words.collect::<Vec<_>>().join(" ").trim_matches('"') == "Condition Zero"
        })
    })
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
pub fn read_saved(file: &Path) -> Option<[String; GameFolder::COUNT]> {
    let text = std::fs::read_to_string(file).ok()?;
    let mut saved: [Option<String>; GameFolder::COUNT] = Default::default();
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

/// One `key=value` line of the saved `settings.cfg`, for the few settings that must be known before
/// the window exists (the console loads the rest once the app is running).
#[must_use]
pub fn saved_setting(key: &str) -> Option<String> {
    let text = std::fs::read_to_string(default_settings_file()?).ok()?;
    text.lines().find_map(|line| {
        let (name, value) = line.trim().split_once('=')?;
        (name.trim() == key).then(|| value.trim().to_owned())
    })
}

fn saved_at_start() -> &'static Option<[String; GameFolder::COUNT]> {
    static SAVED: OnceLock<Option<[String; GameFolder::COUNT]>> = OnceLock::new();
    SAVED.get_or_init(|| default_settings_file().and_then(|file| read_saved(&file)))
}

/// Whether the player has confirmed the game folders once (the window wrote its keys).
#[must_use]
pub fn confirmed() -> bool {
    saved_at_start().is_some()
}

/// What older builds saved a turned-off Counter-Strike folder as (still read).
pub const TURNED_OFF: &str = "none";

/// Which games are on (`use_css` / `use_cz` / `use_cs16`, the window's "Use" boxes); a game
/// without the line is on, unless its folder was saved as `none` by an older build. MW2 is
/// always on.
#[must_use]
pub fn read_used(file: &Path) -> [bool; GameFolder::COUNT] {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let mut used = [true; GameFolder::COUNT];
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        let value = value.trim();
        for folder in GameFolder::ALL {
            if folder.use_key() == Some(key.trim()) {
                used[folder.index()] = value != "0";
            } else if folder.key() == key.trim()
                && folder != GameFolder::Mw2
                && value.eq_ignore_ascii_case(TURNED_OFF)
            {
                used[folder.index()] = false;
            }
        }
    }
    used
}

fn used_at_start() -> &'static [bool; GameFolder::COUNT] {
    static USED: OnceLock<[bool; GameFolder::COUNT]> = OnceLock::new();
    USED.get_or_init(|| {
        default_settings_file().map_or([true; GameFolder::COUNT], |file| read_used(&file))
    })
}

/// Whether the player turned `folder` off in the game folders window: the game is not used at
/// all, whatever folder is saved, found in Steam or named in `.env`.
#[must_use]
pub fn turned_off(folder: GameFolder) -> bool {
    !used_at_start()[folder.index()]
}

/// The folder saved for `folder`, resolved, when it still holds the game's data.
#[must_use]
pub fn saved(folder: GameFolder) -> Option<PathBuf> {
    let raw = &saved_at_start().as_ref()?[folder.index()];
    if raw.is_empty() || raw.trim().eq_ignore_ascii_case(TURNED_OFF) {
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

/// Writes the game-path lines and the on/off lines into `file`, keeping every other line.
pub fn write_saved(
    file: &Path,
    paths: &[String; GameFolder::COUNT],
    used: [bool; GameFolder::COUNT],
) -> Result<(), String> {
    let old = std::fs::read_to_string(file).unwrap_or_default();
    let mut lines = old
        .lines()
        .filter(|line| {
            line.split_once('=').is_none_or(|(key, _)| {
                !GameFolder::ALL.iter().any(|folder| {
                    folder.key() == key.trim() || folder.use_key() == Some(key.trim())
                })
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
    let mut written = Vec::new();
    for folder in GameFolder::ALL {
        let value = paths[folder.index()].replace(['\n', '\r'], "");
        written.push(format!("{}={value}", folder.key()));
        if let Some(key) = folder.use_key() {
            written.push(format!("{key}={}", u8::from(used[folder.index()])));
        }
    }
    for (offset, line) in written.into_iter().enumerate() {
        lines.insert(at + offset, line);
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

/// Asks for a folder with the desktop's folder picker — `zenity` (GNOME and most desktops), else
/// `kdialog` (KDE); `None` when cancelled or when neither is installed. Blocks the calling thread
/// until the dialog closes.
#[cfg(not(windows))]
#[must_use]
pub fn pick_folder(title: &str, start: Option<&Path>) -> Option<PathBuf> {
    use std::process::Command;
    let start = start.filter(|dir| dir.is_dir());
    let mut zenity = Command::new("zenity");
    zenity.args(["--file-selection", "--directory", "--title", title]);
    if let Some(dir) = start {
        // A trailing slash opens the folder itself rather than selecting it in its parent.
        zenity.arg(format!("--filename={}/", dir.display()));
    }
    let mut kdialog = Command::new("kdialog");
    kdialog
        .arg("--getexistingdirectory")
        .arg(start.unwrap_or(Path::new(".")))
        .args(["--title", title]);
    for mut picker in [zenity, kdialog] {
        match picker.output() {
            Ok(output) => {
                let picked = String::from_utf8_lossy(&output.stdout);
                let picked = picked.trim_end_matches(['\n', '\r']);
                return (output.status.success() && !picked.is_empty())
                    .then(|| PathBuf::from(picked));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                diag::warn!(Zone, "folder picker failed: {error}");
                return None;
            }
        }
    }
    diag::warn!(
        Zone,
        "no folder picker: install zenity or kdialog, or set the folders in .env"
    );
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
            Some([String::new(), "old".into(), String::new(), String::new()])
        );
        let paths = [
            "C:/MW2".into(),
            String::new(),
            "D:/Half-Life/czero".into(),
            "D:/Half-Life/cstrike".into(),
        ];
        write_saved(&file, &paths, [true, false, false, true]).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            text,
            "fov=80\ngame_path_mw2=C:/MW2\ngame_path_css=\nuse_css=0\ngame_path_cz=D:/Half-Life/czero\nuse_cz=0\ngame_path_cs16=D:/Half-Life/cstrike\nuse_cs16=1\nbind G drop\n"
        );
        assert_eq!(read_saved(&file), Some(paths));
        assert_eq!(read_used(&file), [true, false, false, true]);
        // Older builds saved a turned-off game as `none`.
        std::fs::write(&file, "game_path_css=none\n").unwrap();
        assert_eq!(read_used(&file), [true, false, true, true]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn listed_in_index_order() {
        // The game folders window saves `ALL.map(..)`, which `write_saved` reads by index.
        for (at, folder) in GameFolder::ALL.into_iter().enumerate() {
            assert_eq!(folder.index(), at, "{folder:?}");
        }
    }

    #[test]
    fn condition_zero_resolves_to_czero_and_not_as_cs16() {
        let dir = std::env::temp_dir().join(format!("iw4l-cz-folder-{}", std::process::id()));
        let top = dir.join("Half-Life");
        for game in ["cstrike", "czero"] {
            std::fs::create_dir_all(top.join(game).join("models")).unwrap();
            std::fs::write(top.join(game).join("models").join("v_ak47.mdl"), b"IDST").unwrap();
        }
        let czero = top.join("czero");
        std::fs::write(
            czero.join("liblist.gam"),
            "game \"Condition Zero\"\nfallback_dir \"cstrike\"\n",
        )
        .unwrap();
        assert_eq!(GameFolder::Cz.resolve(&top), Some(czero.clone()));
        assert_eq!(GameFolder::Cz.resolve(&czero), Some(czero.clone()));
        assert_eq!(GameFolder::Cz.resolve(&top.join("cstrike")), None);
        assert_eq!(GameFolder::Cs16.resolve(&czero), None);
        assert_eq!(GameFolder::Cs16.resolve(&top), Some(top.join("cstrike")));
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
