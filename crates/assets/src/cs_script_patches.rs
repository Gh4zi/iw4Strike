//! Counter-Strike fork edits to MW2's own scripts, applied to their source text as it loads
//! (CS rules only). Each patch replaces one exact line; one that no longer matches is logged
//! and skipped, so a changed script fails loudly instead of silently.

struct Patch {
    module: &'static str,
    find: &'static str,
    replace: &'static str,
    why: &'static str,
}

const PATCHES: &[Patch] = &[Patch {
    module: "maps/mp/gametypes/_gamescore",
    find: "if ( !player rankingEnabled() && !level.hardcoreMode )",
    replace: "if ( 0 )",
    why: "no \"+50\" score popups (CS shows none)",
}];

/// Applies every patch for `module` to its source.
pub(crate) fn apply(module: &str, bytes: &mut Vec<u8>) {
    if !movement_iw4::rules::CS_RULES {
        return;
    }
    for patch in PATCHES.iter().filter(|p| p.module.eq_ignore_ascii_case(module)) {
        let Ok(text) = std::str::from_utf8(bytes) else {
            diag::warn!(Zone, "cs script patch: {module} is not text");
            return;
        };
        match text.matches(patch.find).count() {
            1 => {
                *bytes = text.replacen(patch.find, patch.replace, 1).into_bytes();
                diag::info!(Zone, "cs script patch: {module} — {}", patch.why);
            }
            n => diag::warn!(
                Zone,
                "cs script patch: {module} — `{}` found {n} times, not applied ({})",
                patch.find,
                patch.why
            ),
        }
    }
}
