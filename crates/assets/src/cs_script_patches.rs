//! Counter-Strike fork edits to MW2's own scripts, applied to their source text as it loads
//! (CS rules only). Each patch replaces one exact line; one that no longer matches is logged
//! and skipped, so a changed script fails loudly instead of silently.

struct Patch {
    module: &'static str,
    find: &'static str,
    replace: &'static str,
    why: &'static str,
}

const PATCHES: &[Patch] = &[
    Patch {
        module: "maps/mp/gametypes/_gamescore",
        find: "if ( !player rankingEnabled() && !level.hardcoreMode )",
        replace: "if ( 0 )",
        why: "no \"+50\" score popups (CS shows none)",
    },
    Patch {
        module: "maps/mp/_utility",
        find: "if ( isDefined( self.perks[perkName] ) )",
        replace: "if ( 0 )",
        why: "no perks: `_hasPerk` is always false (the class perks the scripts still record \
              cancelled fall damage through Commando Pro, among others)",
    },
];

/// Applies every patch for `module` to its source.
pub(crate) fn apply(module: &str, bytes: &mut Vec<u8>) {
    if !movement_iw4::rules::CS_RULES {
        return;
    }
    for patch in PATCHES.iter().filter(|p| p.module.eq_ignore_ascii_case(module)) {
        // Byte-wise: some MW2 scripts carry stray non-UTF-8 bytes in their comments.
        let find = patch.find.as_bytes();
        let hits = bytes
            .windows(find.len())
            .enumerate()
            .filter(|(_, window)| *window == find)
            .map(|(at, _)| at)
            .collect::<Vec<_>>();
        match hits[..] {
            [at] => {
                bytes.splice(at..at + find.len(), patch.replace.bytes());
                diag::info!(Zone, "cs script patch: {module} — {}", patch.why);
            }
            _ => diag::warn!(
                Zone,
                "cs script patch: {module} — `{}` found {} times, not applied ({})",
                patch.find,
                hits.len(),
                patch.why
            ),
        }
    }
}
