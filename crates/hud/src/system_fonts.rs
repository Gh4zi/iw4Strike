//! The PC's own fonts for the CS HUD, scoreboard and buy menu: the Windows font a panel names
//! (Verdana, Tahoma, Arial) from the Windows font folder, or on Linux DejaVu Sans, which every
//! desktop distribution ships, in the same weight.

use std::path::PathBuf;

/// Linux font folders that hold DejaVu Sans (Debian/Ubuntu, Arch, Fedora, openSUSE).
#[cfg(not(windows))]
const DEJAVU_DIRS: [&str; 4] = [
    "/usr/share/fonts/truetype/dejavu",
    "/usr/share/fonts/TTF",
    "/usr/share/fonts/dejavu-sans-fonts",
    "/usr/share/fonts/dejavu",
];

/// The first of `names` (Windows font file names, best first) that this PC has, as file bytes.
/// On Linux, DejaVu Sans stands in, bold when the first name is a bold face (`…bd.ttf`,
/// `…b.ttf`).
pub(crate) fn read(names: &[&str]) -> Option<Vec<u8>> {
    let windows = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:/Windows"))
        .join("Fonts");
    if let Some(bytes) = names
        .iter()
        .find_map(|name| std::fs::read(windows.join(name)).ok())
    {
        return Some(bytes);
    }
    #[cfg(not(windows))]
    {
        let bold = names.first().is_some_and(|name| {
            let stem = name.trim_end_matches(".ttf");
            stem.ends_with("bd") || stem.ends_with('b')
        });
        let file = if bold {
            "DejaVuSans-Bold.ttf"
        } else {
            "DejaVuSans.ttf"
        };
        if let Some(bytes) = DEJAVU_DIRS
            .iter()
            .find_map(|dir| std::fs::read(std::path::Path::new(dir).join(file)).ok())
        {
            return Some(bytes);
        }
    }
    None
}
