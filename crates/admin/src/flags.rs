//! Admin flags: one letter each, as server admins already know them.
//!
//! a reservation, b generic, c kick, d ban, e unban, f slay, g map, h cvar, i config, j chat,
//! k vote, l password, m rcon, n cheats, o-t custom, z root (every flag).

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Flags(u32);

pub(crate) const GENERIC: char = 'b';
pub(crate) const KICK: char = 'c';
pub(crate) const BAN: char = 'd';
pub(crate) const UNBAN: char = 'e';
pub(crate) const SLAY: char = 'f';
pub(crate) const MAP: char = 'g';
pub(crate) const CVAR: char = 'h';
pub(crate) const CHAT: char = 'j';
pub(crate) const VOTE: char = 'k';
pub(crate) const RCON: char = 'm';
pub(crate) const ROOT: char = 'z';

const LETTERS: &str = "abcdefghijklmnopqrstz";

fn bit(letter: char) -> Option<u32> {
    LETTERS
        .find(letter.to_ascii_lowercase())
        .map(|index| 1 << index)
}

impl Flags {
    pub(crate) fn root() -> Self {
        Self(bit(ROOT).unwrap_or(0))
    }

    /// The flags in `letters` (unknown letters are skipped).
    pub(crate) fn parse(letters: &str) -> Self {
        Self(letters.chars().filter_map(bit).fold(0, |all, bit| all | bit))
    }

    pub(crate) fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub(crate) fn is_root(self) -> bool {
        bit(ROOT).is_some_and(|root| self.0 & root != 0)
    }

    pub(crate) fn any(self) -> bool {
        self.0 != 0
    }

    /// Whether these flags allow `letter` (root allows all).
    pub(crate) fn has(self, letter: char) -> bool {
        self.is_root() || bit(letter).is_some_and(|bit| self.0 & bit != 0)
    }

    pub(crate) fn letters(self) -> String {
        LETTERS
            .chars()
            .filter(|letter| bit(*letter).is_some_and(|bit| self.0 & bit != 0))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_allows_every_flag() {
        let root = Flags::parse("z");
        assert!(root.has(KICK) && root.has(RCON) && root.is_root());
        let kick = Flags::parse("bc");
        assert!(kick.has(KICK) && !kick.has(BAN) && !kick.is_root());
        assert_eq!(Flags::parse("cbx?").letters(), "bc");
    }
}
