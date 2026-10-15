//! Who is an admin: `admin/admins_simple.ini` (one admin per line) and `admin/admin_groups.cfg`
//! (named groups of flags), in the formats server admins already use.

use std::collections::HashMap;
use std::path::Path;

use crate::flags::Flags;

const DEFAULT_ADMINS: &str = r#"// Server admins, one per line:
//
//   "identity"   "flags"
//
// identity: a player's account id (32 hex digits: they type !id in chat, or see it in the
//           panel or `who`), or "!" and an address ("!192.168.1.20").
// flags:    letters, "immunity:letters", "@group" or "immunity:@group".
//           a reservation, b generic, c kick, d ban, e unban, f slay, g map, h cvar,
//           i config, j chat, k vote, l password, m rcon, n cheats, o-t custom, z root (all).
//           A higher immunity cannot be targeted by a lower one.
//
// "0123456789abcdef0123456789abcdef"  "99:z"
// "fedcba9876543210fedcba9876543210"  "@Moderators"
"#;

const DEFAULT_GROUPS: &str = r#"// Admin groups: admins_simple.ini names them as "@group".
Groups
{
	"Full Admins"
	{
		"flags"		"z"
		"immunity"	"99"
	}
	"Moderators"
	{
		"flags"		"bcdefjk"
		"immunity"	"10"
	}
}
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Identity {
    Account([u8; 16]),
    Address(String),
}

#[derive(Clone, Debug)]
struct Admin {
    identity: Identity,
    flags: Flags,
    immunity: i32,
}

/// What an admin may do: their flags and immunity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Access {
    pub(crate) flags: Flags,
    pub(crate) immunity: i32,
}

#[derive(Debug, Default)]
pub(crate) struct Admins {
    list: Vec<Admin>,
}

pub(crate) fn parse_account(text: &str) -> Option<[u8; 16]> {
    let text = text.trim();
    if text.len() != 32 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 16];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub(crate) fn account_hex(account: &[u8; 16]) -> String {
    account.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl Admins {
    /// Reads both files (writing commented defaults when missing): the admins and any warnings.
    pub(crate) fn load(dir: &Path) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let groups_path = dir.join("admin_groups.cfg");
        let admins_path = dir.join("admins_simple.ini");
        for (path, text) in [(&groups_path, DEFAULT_GROUPS), (&admins_path, DEFAULT_ADMINS)] {
            if !path.exists()
                && let Err(error) =
                    std::fs::create_dir_all(dir).and_then(|()| std::fs::write(path, text))
            {
                warnings.push(format!("{}: {error}", path.display()));
            }
        }
        let groups = match std::fs::read_to_string(&groups_path) {
            Ok(text) => parse_groups(&text, &mut warnings),
            Err(error) => {
                warnings.push(format!("{}: {error}", groups_path.display()));
                HashMap::new()
            }
        };
        let list = match std::fs::read_to_string(&admins_path) {
            Ok(text) => parse_admins(&text, &groups, &mut warnings),
            Err(error) => {
                warnings.push(format!("{}: {error}", admins_path.display()));
                Vec::new()
            }
        };
        (Self { list }, warnings)
    }

    pub(crate) fn len(&self) -> usize {
        self.list.len()
    }

    /// The access of a player with `account` connecting from `address`: every matching entry's
    /// flags together, at the highest immunity. None for a player who is no admin.
    pub(crate) fn access(&self, account: Option<&[u8; 16]>, address: Option<&str>) -> Option<Access> {
        self.list
            .iter()
            .filter(|admin| match &admin.identity {
                Identity::Account(id) => account == Some(id),
                Identity::Address(ip) => address == Some(ip.as_str()),
            })
            .fold(None, |access: Option<Access>, admin| {
                let access = access.unwrap_or_default();
                Some(Access {
                    flags: access.flags.union(admin.flags),
                    immunity: access.immunity.max(admin.immunity),
                })
            })
    }
}

/// `"identity" "flags" ["password"]` lines.
fn parse_admins(
    text: &str,
    groups: &HashMap<String, Access>,
    warnings: &mut Vec<String>,
) -> Vec<Admin> {
    let mut list = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let words = match tokens(line) {
            Ok(words) => words,
            Err(error) => {
                warnings.push(format!("admins_simple.ini line {}: {error}", number + 1));
                continue;
            }
        };
        let words: Vec<String> = words
            .into_iter()
            .filter_map(|token| match token {
                Token::Word(word) => Some(word),
                _ => None,
            })
            .collect();
        let [identity, flags, rest @ ..] = words.as_slice() else {
            if !words.is_empty() {
                warnings.push(format!(
                    "admins_simple.ini line {}: needs \"identity\" \"flags\"",
                    number + 1
                ));
            }
            continue;
        };
        if !rest.is_empty() {
            warnings.push(format!(
                "admins_simple.ini line {}: passwords are not used (the account proves who it is)",
                number + 1
            ));
        }
        let identity = if let Some(address) = identity.strip_prefix('!') {
            Identity::Address(address.to_owned())
        } else if let Some(account) = parse_account(identity) {
            Identity::Account(account)
        } else {
            warnings.push(format!(
                "admins_simple.ini line {}: `{identity}` is no account id (32 hex digits) or !address",
                number + 1
            ));
            continue;
        };
        let (immunity, flags) = match flags.split_once(':') {
            Some((immunity, flags)) => (immunity.trim().parse::<i32>().ok(), flags.trim()),
            None => (None, flags.trim()),
        };
        let access = if let Some(group) = flags.strip_prefix('@') {
            match groups.get(&group.to_ascii_lowercase()) {
                Some(access) => *access,
                None => {
                    warnings.push(format!(
                        "admins_simple.ini line {}: no group `{group}` in admin_groups.cfg",
                        number + 1
                    ));
                    continue;
                }
            }
        } else {
            Access {
                flags: Flags::parse(flags),
                immunity: 0,
            }
        };
        list.push(Admin {
            identity,
            flags: access.flags,
            immunity: immunity.unwrap_or(access.immunity),
        });
    }
    list
}

/// `Groups { "name" { "flags" "abc" "immunity" "10" } }`, by lowercase name.
fn parse_groups(text: &str, warnings: &mut Vec<String>) -> HashMap<String, Access> {
    let mut groups = HashMap::new();
    let tokens: Vec<Token> = match text.lines().map(tokens).collect::<Result<Vec<_>, _>>() {
        Ok(lines) => lines.into_iter().flatten().collect(),
        Err(error) => {
            warnings.push(format!("admin_groups.cfg: {error}"));
            return groups;
        }
    };
    // Depth 1 is inside `Groups`, 2 inside a group, deeper is a group's own sections (ignored).
    let mut depth = 0usize;
    let mut last_word: Option<String> = None;
    let mut group: Option<(String, Access)> = None;
    let mut key: Option<String> = None;
    for token in tokens {
        match token {
            Token::Open => {
                depth += 1;
                if depth == 2 {
                    group = last_word.take().map(|name| (name, Access::default()));
                }
                key = None;
                last_word = None;
            }
            Token::Close => {
                if depth == 2
                    && let Some((name, access)) = group.take()
                {
                    groups.insert(name.to_ascii_lowercase(), access);
                }
                depth = depth.saturating_sub(1);
                key = None;
                last_word = None;
            }
            Token::Word(word) => {
                if depth == 2 {
                    match key.take() {
                        None => key = Some(word.clone()),
                        Some(name) => {
                            if let Some((_, access)) = group.as_mut() {
                                match name.to_ascii_lowercase().as_str() {
                                    "flags" => access.flags = Flags::parse(&word),
                                    "immunity" => {
                                        access.immunity = word.trim().parse().unwrap_or(0)
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                last_word = Some(word);
            }
        }
    }
    groups
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Word(String),
    Open,
    Close,
}

/// A line's quoted strings, bare words and braces, up to a `//` comment.
fn tokens(line: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&ch) = chars.peek() {
        match ch {
            c if c.is_whitespace() => {
                chars.next();
            }
            '/' => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    break;
                }
                out.push(Token::Word("/".into()));
            }
            '{' => {
                chars.next();
                out.push(Token::Open);
            }
            '}' => {
                chars.next();
                out.push(Token::Close);
            }
            '"' => {
                chars.next();
                let mut word = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some(c) => word.push(c),
                        None => return Err("a quote is not closed".into()),
                    }
                }
                out.push(Token::Word(word));
            }
            _ => {
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || matches!(c, '{' | '}' | '"') {
                        break;
                    }
                    word.push(c);
                    chars.next();
                }
                out.push(Token::Word(word));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_groups_parse() {
        let mut warnings = Vec::new();
        let groups = parse_groups(DEFAULT_GROUPS, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(groups["full admins"].flags.is_root());
        assert_eq!(groups["full admins"].immunity, 99);
        assert!(groups["moderators"].flags.has(crate::flags::KICK));
        assert!(!groups["moderators"].flags.has(crate::flags::RCON));
    }

    #[test]
    fn admins_take_flags_immunity_and_groups() {
        let mut warnings = Vec::new();
        let groups = parse_groups(DEFAULT_GROUPS, &mut warnings);
        let text = r#"
            // a comment
            "0123456789abcdef0123456789abcdef" "50:cd"
            "fedcba9876543210fedcba9876543210" "@Moderators"
            "!10.0.0.5" "z" // the server owner's PC
            "nobody" "z"
        "#;
        let admins = Admins {
            list: parse_admins(text, &groups, &mut warnings),
        };
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let first = parse_account("0123456789abcdef0123456789abcdef").unwrap();
        let access = admins.access(Some(&first), None).unwrap();
        assert!(access.flags.has('c') && !access.flags.has('m'));
        assert_eq!(access.immunity, 50);
        let moderator = parse_account("fedcba9876543210fedcba9876543210").unwrap();
        assert_eq!(admins.access(Some(&moderator), None).unwrap().immunity, 10);
        assert!(admins.access(None, Some("10.0.0.5")).unwrap().flags.is_root());
        assert!(admins.access(Some(&[0; 16]), Some("10.0.0.6")).is_none());
    }
}
