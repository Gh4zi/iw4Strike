//! `admin/admin.cfg`: which parts of the admin system run, and how. One setting per line,
//! `name value`, `//` starts a comment. Written with its defaults when missing.

use std::path::{Path, PathBuf};

pub(crate) const DEFAULT_CONFIG: &str = r#"// Server admin settings. One setting per line; "//" starts a comment.
// Admins are listed in admins_simple.ini, their groups in admin_groups.cfg.

// ---- Parts (1 on, 0 off) ----
chat_commands 1        // !command (said), /command (silent) and @message (to admins) in chat
history 1              // player history: joins, leaves, names, addresses (players.db, logs/)
basecommands 1         // kick, map, rcon, cvar, who, history
basebans 1             // ban, banip, addban, unban
basechat 1             // say, csay, psay, admin chat, gag
funcommands 1          // slay
votes 1                // votekick, votemap (players answer !yes / !no)
antiflood 1            // chat flood: a short gag

// ---- History ----
history_days 90        // sessions and addresses older than this are deleted

// ---- Votes ----
public_votes 0         // 1: any player can start a vote; 0: admins with the vote flag (k)
vote_seconds 30
vote_ratio 0.6         // share of the votes cast that must be yes

// ---- Anti-flood ----
flood_lines 4          // more than this many lines ...
flood_seconds 3        // ... in this many seconds ...
flood_gag_seconds 10   // ... gags the player this long

// ---- Web panel ----
panel 1
panel_address 127.0.0.1:28400   // 127.0.0.1: this PC only. 0.0.0.0 opens it to the network
"#;

#[derive(Clone, Debug)]
pub(crate) struct AdminConfig {
    pub(crate) dir: PathBuf,
    pub(crate) chat_commands: bool,
    pub(crate) history: bool,
    pub(crate) basecommands: bool,
    pub(crate) basebans: bool,
    pub(crate) basechat: bool,
    pub(crate) funcommands: bool,
    pub(crate) votes: bool,
    pub(crate) antiflood: bool,
    pub(crate) history_days: u32,
    pub(crate) public_votes: bool,
    pub(crate) vote_seconds: u32,
    pub(crate) vote_ratio: f32,
    pub(crate) flood_lines: u32,
    pub(crate) flood_seconds: f32,
    pub(crate) flood_gag_seconds: u32,
    pub(crate) panel: bool,
    pub(crate) panel_address: String,
}

impl AdminConfig {
    fn defaults(dir: PathBuf) -> Self {
        Self {
            dir,
            chat_commands: true,
            history: true,
            basecommands: true,
            basebans: true,
            basechat: true,
            funcommands: true,
            votes: true,
            antiflood: true,
            history_days: 90,
            public_votes: false,
            vote_seconds: 30,
            vote_ratio: 0.6,
            flood_lines: 4,
            flood_seconds: 3.0,
            flood_gag_seconds: 10,
            panel: true,
            panel_address: "127.0.0.1:28400".into(),
        }
    }

    /// The admin folder: `admin/` in the server's folder (`IW4L_SERVER_DIR`, else where it runs).
    pub(crate) fn admin_dir() -> PathBuf {
        std::env::var_os("IW4L_SERVER_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default()
            .join("admin")
    }

    /// Reads `admin/admin.cfg`, writing the defaults first when it is missing.
    pub(crate) fn load(dir: &Path) -> (Self, Vec<String>) {
        let mut config = Self::defaults(dir.to_path_buf());
        let mut warnings = Vec::new();
        let path = dir.join("admin.cfg");
        if !path.exists() {
            if let Err(error) = std::fs::create_dir_all(dir)
                .and_then(|()| std::fs::write(&path, DEFAULT_CONFIG))
            {
                warnings.push(format!("{}: {error}", path.display()));
            }
            return (config, warnings);
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                warnings.push(format!("{}: {error}", path.display()));
                return (config, warnings);
            }
        };
        for (number, line) in text.lines().enumerate() {
            let line = line.split("//").next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (key, value) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let value = value.trim().trim_matches('"');
            if let Err(error) = config.set(key, value) {
                warnings.push(format!("admin.cfg line {}: {error}", number + 1));
            }
        }
        (config, warnings)
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let flag = || match value {
            "1" | "on" | "true" | "yes" => Ok(true),
            "0" | "off" | "false" | "no" => Ok(false),
            _ => Err(format!("`{key}` takes 1 or 0, not `{value}`")),
        };
        let number = || {
            value
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite() && *v >= 0.0)
                .ok_or_else(|| format!("`{key}` takes a number, not `{value}`"))
        };
        match key.to_ascii_lowercase().as_str() {
            "chat_commands" => self.chat_commands = flag()?,
            "history" => self.history = flag()?,
            "basecommands" => self.basecommands = flag()?,
            "basebans" => self.basebans = flag()?,
            "basechat" => self.basechat = flag()?,
            "funcommands" => self.funcommands = flag()?,
            "votes" => self.votes = flag()?,
            "antiflood" => self.antiflood = flag()?,
            "history_days" => self.history_days = number()?.round() as u32,
            "public_votes" => self.public_votes = flag()?,
            "vote_seconds" => self.vote_seconds = number()?.round().max(5.0) as u32,
            "vote_ratio" => self.vote_ratio = number()?.clamp(0.0, 1.0),
            "flood_lines" => self.flood_lines = number()?.round().max(1.0) as u32,
            "flood_seconds" => self.flood_seconds = number()?.max(0.1),
            "flood_gag_seconds" => self.flood_gag_seconds = number()?.round() as u32,
            "panel" => self.panel = flag()?,
            "panel_address" => {
                value
                    .parse::<std::net::SocketAddr>()
                    .map_err(|_| format!("`panel_address` takes ip:port, not `{value}`"))?;
                self.panel_address = value.to_owned();
            }
            other => return Err(format!("unknown setting `{other}`")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_file_reads_back_as_the_defaults() {
        let dir = std::env::temp_dir().join(format!("iw4-admin-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (_, warnings) = AdminConfig::load(&dir);
        assert!(warnings.is_empty(), "{warnings:?}");
        let (config, warnings) = AdminConfig::load(&dir);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(config.chat_commands && config.panel && !config.public_votes);
        assert_eq!(config.history_days, 90);
        assert_eq!(config.panel_address, "127.0.0.1:28400");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
