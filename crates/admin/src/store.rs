//! The server's records in one local file, `admin/players.db` (SQLite: nothing to install or
//! run): players, their names, addresses and sessions, bans, notes, the admin audit log and
//! the panel's users. The game and the panel each open their own connection to it.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS players(
    account TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    ip TEXT,
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    sessions INTEGER NOT NULL DEFAULT 0,
    playtime INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS names(
    account TEXT NOT NULL,
    name TEXT NOT NULL,
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    PRIMARY KEY(account, name)
);
CREATE TABLE IF NOT EXISTS ips(
    account TEXT NOT NULL,
    ip TEXT NOT NULL,
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    PRIMARY KEY(account, ip)
);
CREATE INDEX IF NOT EXISTS ips_by_ip ON ips(ip);
CREATE TABLE IF NOT EXISTS sessions(
    id INTEGER PRIMARY KEY,
    account TEXT NOT NULL,
    name TEXT NOT NULL,
    ip TEXT,
    map TEXT,
    joined_at INTEGER NOT NULL,
    left_at INTEGER,
    reason TEXT
);
CREATE INDEX IF NOT EXISTS sessions_by_account ON sessions(account, joined_at);
CREATE TABLE IF NOT EXISTS bans(
    id INTEGER PRIMARY KEY,
    account TEXT,
    ip TEXT,
    name TEXT,
    reason TEXT NOT NULL,
    admin TEXT NOT NULL,
    created INTEGER NOT NULL,
    expires INTEGER,
    removed INTEGER,
    removed_by TEXT
);
CREATE TABLE IF NOT EXISTS notes(
    id INTEGER PRIMARY KEY,
    account TEXT NOT NULL,
    author TEXT NOT NULL,
    text TEXT NOT NULL,
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS audit(
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    admin TEXT NOT NULL,
    action TEXT NOT NULL,
    target TEXT,
    detail TEXT
);
CREATE TABLE IF NOT EXISTS panel_users(
    name TEXT PRIMARY KEY,
    salt BLOB NOT NULL,
    hash BLOB NOT NULL,
    flags TEXT NOT NULL,
    created INTEGER NOT NULL,
    last_login INTEGER
);
";

pub(crate) type Result<T> = rusqlite::Result<T>;

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

pub(crate) struct Store {
    db: Connection,
}

/// The game's connection, as a resource can hold it (a connection is used by one thread at a
/// time; the game's systems reach it through `&mut`, which needs no lock).
pub(crate) struct Db(std::sync::Mutex<Store>);

impl Db {
    pub(crate) fn new(store: Store) -> Self {
        Self(std::sync::Mutex::new(store))
    }

    pub(crate) fn get(&mut self) -> &mut Store {
        self.0.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PlayerRow {
    pub(crate) account: String,
    pub(crate) name: String,
    pub(crate) ip: Option<String>,
    pub(crate) first_seen: i64,
    pub(crate) last_seen: i64,
    pub(crate) sessions: i64,
    pub(crate) playtime: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionRow {
    pub(crate) name: String,
    pub(crate) ip: Option<String>,
    pub(crate) map: Option<String>,
    pub(crate) joined_at: i64,
    pub(crate) left_at: Option<i64>,
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Ban {
    pub(crate) id: i64,
    pub(crate) account: Option<String>,
    pub(crate) ip: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) reason: String,
    pub(crate) admin: String,
    pub(crate) created: i64,
    pub(crate) expires: Option<i64>,
    pub(crate) removed: Option<i64>,
    pub(crate) removed_by: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct NewBan<'a> {
    pub(crate) account: Option<&'a str>,
    pub(crate) ip: Option<&'a str>,
    pub(crate) name: Option<&'a str>,
    pub(crate) reason: &'a str,
    pub(crate) admin: &'a str,
    pub(crate) minutes: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct Note {
    pub(crate) author: String,
    pub(crate) text: String,
    pub(crate) created: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct AuditRow {
    pub(crate) at: i64,
    pub(crate) admin: String,
    pub(crate) action: String,
    pub(crate) target: Option<String>,
    pub(crate) detail: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PanelUser {
    pub(crate) name: String,
    pub(crate) salt: Vec<u8>,
    pub(crate) hash: Vec<u8>,
    pub(crate) flags: String,
    pub(crate) last_login: Option<i64>,
}

/// Where `(name, first, last)` and `(ip, first, last)` rows come from.
pub(crate) type Seen = (String, i64, i64);

fn ban_row(row: &rusqlite::Row<'_>) -> Result<Ban> {
    Ok(Ban {
        id: row.get(0)?,
        account: row.get(1)?,
        ip: row.get(2)?,
        name: row.get(3)?,
        reason: row.get(4)?,
        admin: row.get(5)?,
        created: row.get(6)?,
        expires: row.get(7)?,
        removed: row.get(8)?,
        removed_by: row.get(9)?,
    })
}

const BAN_COLUMNS: &str =
    "id, account, ip, name, reason, admin, created, expires, removed, removed_by";

fn player_row(row: &rusqlite::Row<'_>) -> Result<PlayerRow> {
    Ok(PlayerRow {
        account: row.get(0)?,
        name: row.get(1)?,
        ip: row.get(2)?,
        first_seen: row.get(3)?,
        last_seen: row.get(4)?,
        sessions: row.get(5)?,
        playtime: row.get(6)?,
    })
}

const PLAYER_COLUMNS: &str = "account, name, ip, first_seen, last_seen, sessions, playtime";

impl Store {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(2))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.execute_batch(SCHEMA)?;
        Ok(Self { db })
    }

    /// A player joined: their row, name and address seen now, and a new session (its id).
    pub(crate) fn record_join(
        &mut self,
        account: &str,
        name: &str,
        ip: Option<&str>,
        map: &str,
        at: i64,
    ) -> Result<i64> {
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO players(account, name, ip, first_seen, last_seen, sessions)
             VALUES(?1, ?2, ?3, ?4, ?4, 1)
             ON CONFLICT(account) DO UPDATE SET name = ?2, ip = COALESCE(?3, ip),
                 last_seen = ?4, sessions = sessions + 1",
            params![account, name, ip, at],
        )?;
        seen_name(&tx, account, name, at)?;
        if let Some(ip) = ip {
            seen_ip(&tx, account, ip, at)?;
        }
        tx.execute(
            "INSERT INTO sessions(account, name, ip, map, joined_at) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![account, name, ip, map, at],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    pub(crate) fn record_name(&mut self, account: &str, session: i64, name: &str, at: i64) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute(
            "UPDATE players SET name = ?2, last_seen = ?3 WHERE account = ?1",
            params![account, name, at],
        )?;
        tx.execute("UPDATE sessions SET name = ?2 WHERE id = ?1", params![session, name])?;
        seen_name(&tx, account, name, at)?;
        tx.commit()
    }

    pub(crate) fn record_ip(&mut self, account: &str, session: i64, ip: &str, at: i64) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute(
            "UPDATE players SET ip = ?2, last_seen = ?3 WHERE account = ?1",
            params![account, ip, at],
        )?;
        tx.execute("UPDATE sessions SET ip = ?2 WHERE id = ?1", params![session, ip])?;
        seen_ip(&tx, account, ip, at)?;
        tx.commit()
    }

    pub(crate) fn record_leave(&mut self, account: &str, session: i64, at: i64, reason: &str) -> Result<()> {
        let tx = self.db.transaction()?;
        let joined: Option<i64> = tx
            .query_row(
                "SELECT joined_at FROM sessions WHERE id = ?1",
                params![session],
                |row| row.get(0),
            )
            .optional()?;
        tx.execute(
            "UPDATE sessions SET left_at = ?2, reason = ?3 WHERE id = ?1",
            params![session, at, reason],
        )?;
        tx.execute(
            "UPDATE players SET last_seen = ?2, playtime = playtime + ?3 WHERE account = ?1",
            params![account, at, joined.map_or(0, |joined| (at - joined).max(0))],
        )?;
        tx.commit()
    }

    /// Sessions the server was stopped during (no leave written): closed at their last sight.
    pub(crate) fn close_open_sessions(&mut self, at: i64) -> Result<usize> {
        self.db.execute(
            "UPDATE sessions SET left_at = MAX(joined_at, (SELECT last_seen FROM players
                 WHERE players.account = sessions.account)), reason = 'server stopped'
             WHERE left_at IS NULL AND joined_at <= ?1",
            params![at],
        )
    }

    /// Deletes sessions and addresses last seen before `before`.
    pub(crate) fn purge(&mut self, before: i64) -> Result<usize> {
        let tx = self.db.transaction()?;
        let mut gone = tx.execute("DELETE FROM sessions WHERE joined_at < ?1", params![before])?;
        gone += tx.execute("DELETE FROM ips WHERE last_seen < ?1", params![before])?;
        gone += tx.execute(
            "UPDATE players SET ip = NULL WHERE ip IS NOT NULL AND last_seen < ?1",
            params![before],
        )?;
        tx.commit()?;
        Ok(gone)
    }

    pub(crate) fn add_ban(&mut self, ban: &NewBan<'_>, at: i64) -> Result<i64> {
        let expires = (ban.minutes > 0).then(|| at + (ban.minutes as i64).saturating_mul(60));
        self.db.execute(
            "INSERT INTO bans(account, ip, name, reason, admin, created, expires)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![ban.account, ban.ip, ban.name, ban.reason, ban.admin, at, expires],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    /// Lifts a ban: true when it was in force.
    pub(crate) fn remove_ban(&mut self, id: i64, by: &str, at: i64) -> Result<bool> {
        Ok(self.db.execute(
            "UPDATE bans SET removed = ?2, removed_by = ?3 WHERE id = ?1 AND removed IS NULL",
            params![id, at, by],
        )? > 0)
    }

    /// The bans in force at `at` that match an account or address.
    pub(crate) fn bans_matching(&self, account: Option<&str>, ip: Option<&str>, at: i64) -> Result<Vec<Ban>> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {BAN_COLUMNS} FROM bans WHERE removed IS NULL
                 AND (expires IS NULL OR expires > ?3)
                 AND ((?1 IS NOT NULL AND account = ?1) OR (?2 IS NOT NULL AND ip = ?2))
             ORDER BY id"
        ))?;
        statement
            .query_map(params![account, ip, at], ban_row)?
            .collect()
    }

    pub(crate) fn active_bans(&self, at: i64) -> Result<Vec<Ban>> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {BAN_COLUMNS} FROM bans WHERE removed IS NULL
                 AND (expires IS NULL OR expires > ?1) ORDER BY id DESC"
        ))?;
        statement.query_map(params![at], ban_row)?.collect()
    }

    /// Every ban, newest first (lifted and expired ones too).
    pub(crate) fn bans(&self, limit: u32) -> Result<Vec<Ban>> {
        let mut statement = self
            .db
            .prepare(&format!("SELECT {BAN_COLUMNS} FROM bans ORDER BY id DESC LIMIT ?1"))?;
        statement.query_map(params![limit], ban_row)?.collect()
    }

    pub(crate) fn bans_of(&self, account: &str) -> Result<Vec<Ban>> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {BAN_COLUMNS} FROM bans WHERE account = ?1
                 OR ip IN (SELECT ip FROM ips WHERE account = ?1) ORDER BY id DESC"
        ))?;
        statement.query_map(params![account], ban_row)?.collect()
    }

    pub(crate) fn audit(
        &mut self,
        at: i64,
        admin: &str,
        action: &str,
        target: Option<&str>,
        detail: Option<&str>,
    ) -> Result<()> {
        self.db.execute(
            "INSERT INTO audit(at, admin, action, target, detail) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![at, admin, action, target, detail],
        )?;
        Ok(())
    }

    pub(crate) fn audit_log(&self, limit: u32) -> Result<Vec<AuditRow>> {
        let mut statement = self.db.prepare(
            "SELECT at, admin, action, target, detail FROM audit ORDER BY id DESC LIMIT ?1",
        )?;
        statement
            .query_map(params![limit], |row| {
                Ok(AuditRow {
                    at: row.get(0)?,
                    admin: row.get(1)?,
                    action: row.get(2)?,
                    target: row.get(3)?,
                    detail: row.get(4)?,
                })
            })?
            .collect()
    }

    pub(crate) fn player(&self, account: &str) -> Result<Option<PlayerRow>> {
        self.db
            .query_row(
                &format!("SELECT {PLAYER_COLUMNS} FROM players WHERE account = ?1"),
                params![account],
                player_row,
            )
            .optional()
    }

    /// Players whose account starts with `query`, or who used a name (or, with `addresses`, an
    /// address) containing it; the latest seen first. An empty query: the latest seen.
    pub(crate) fn find_players(&self, query: &str, addresses: bool, limit: u32) -> Result<Vec<PlayerRow>> {
        let like = format!(
            "%{}%",
            query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
        );
        let prefix = format!("{}%", query.to_ascii_lowercase().replace(['%', '_', '\\'], ""));
        let mut statement = self.db.prepare(&format!(
            "SELECT {PLAYER_COLUMNS} FROM players WHERE ?1 = ''
                 OR account LIKE ?2
                 OR account IN (SELECT account FROM names WHERE name LIKE ?3 ESCAPE '\\')
                 OR (?4 AND account IN (SELECT account FROM ips WHERE ip LIKE ?3 ESCAPE '\\'))
             ORDER BY last_seen DESC LIMIT ?5"
        ))?;
        statement
            .query_map(params![query, prefix, like, addresses, limit], player_row)?
            .collect()
    }

    pub(crate) fn names(&self, account: &str) -> Result<Vec<Seen>> {
        self.seen("names", "name", account)
    }

    pub(crate) fn ips(&self, account: &str) -> Result<Vec<Seen>> {
        self.seen("ips", "ip", account)
    }

    fn seen(&self, table: &str, column: &str, account: &str) -> Result<Vec<Seen>> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {column}, first_seen, last_seen FROM {table} WHERE account = ?1
             ORDER BY last_seen DESC"
        ))?;
        statement
            .query_map(params![account], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect()
    }

    pub(crate) fn sessions(&self, account: &str, limit: u32) -> Result<Vec<SessionRow>> {
        let mut statement = self.db.prepare(
            "SELECT name, ip, map, joined_at, left_at, reason FROM sessions WHERE account = ?1
             ORDER BY joined_at DESC LIMIT ?2",
        )?;
        statement
            .query_map(params![account, limit], |row| {
                Ok(SessionRow {
                    name: row.get(0)?,
                    ip: row.get(1)?,
                    map: row.get(2)?,
                    joined_at: row.get(3)?,
                    left_at: row.get(4)?,
                    reason: row.get(5)?,
                })
            })?
            .collect()
    }

    /// Other accounts that used one of `account`'s addresses: (account, last name, address).
    pub(crate) fn alts(&self, account: &str) -> Result<Vec<(String, String, String)>> {
        let mut statement = self.db.prepare(
            "SELECT DISTINCT other.account, players.name, other.ip FROM ips mine
                 JOIN ips other ON other.ip = mine.ip AND other.account != mine.account
                 JOIN players ON players.account = other.account
             WHERE mine.account = ?1 ORDER BY other.last_seen DESC LIMIT 50",
        )?;
        statement
            .query_map(params![account], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect()
    }

    pub(crate) fn add_note(&mut self, account: &str, author: &str, text: &str, at: i64) -> Result<()> {
        self.db.execute(
            "INSERT INTO notes(account, author, text, created) VALUES(?1, ?2, ?3, ?4)",
            params![account, author, text, at],
        )?;
        Ok(())
    }

    pub(crate) fn notes(&self, account: &str) -> Result<Vec<Note>> {
        let mut statement = self.db.prepare(
            "SELECT author, text, created FROM notes WHERE account = ?1 ORDER BY id DESC",
        )?;
        statement
            .query_map(params![account], |row| {
                Ok(Note {
                    author: row.get(0)?,
                    text: row.get(1)?,
                    created: row.get(2)?,
                })
            })?
            .collect()
    }

    pub(crate) fn panel_user(&self, name: &str) -> Result<Option<PanelUser>> {
        self.db
            .query_row(
                "SELECT name, salt, hash, flags, last_login FROM panel_users WHERE name = ?1",
                params![name],
                |row| {
                    Ok(PanelUser {
                        name: row.get(0)?,
                        salt: row.get(1)?,
                        hash: row.get(2)?,
                        flags: row.get(3)?,
                        last_login: row.get(4)?,
                    })
                },
            )
            .optional()
    }

    pub(crate) fn panel_users(&self) -> Result<Vec<PanelUser>> {
        let mut statement = self
            .db
            .prepare("SELECT name, salt, hash, flags, last_login FROM panel_users ORDER BY name")?;
        statement
            .query_map([], |row| {
                Ok(PanelUser {
                    name: row.get(0)?,
                    salt: row.get(1)?,
                    hash: row.get(2)?,
                    flags: row.get(3)?,
                    last_login: row.get(4)?,
                })
            })?
            .collect()
    }

    pub(crate) fn set_panel_user(&mut self, name: &str, salt: &[u8], hash: &[u8], flags: &str, at: i64) -> Result<()> {
        self.db.execute(
            "INSERT INTO panel_users(name, salt, hash, flags, created) VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(name) DO UPDATE SET salt = ?2, hash = ?3, flags = ?4",
            params![name, salt, hash, flags, at],
        )?;
        Ok(())
    }

    pub(crate) fn remove_panel_user(&mut self, name: &str) -> Result<bool> {
        Ok(self
            .db
            .execute("DELETE FROM panel_users WHERE name = ?1", params![name])?
            > 0)
    }

    pub(crate) fn panel_login(&mut self, name: &str, at: i64) -> Result<()> {
        self.db.execute(
            "UPDATE panel_users SET last_login = ?2 WHERE name = ?1",
            params![name, at],
        )?;
        Ok(())
    }
}

fn seen_name(db: &Connection, account: &str, name: &str, at: i64) -> Result<()> {
    db.execute(
        "INSERT INTO names(account, name, first_seen, last_seen) VALUES(?1, ?2, ?3, ?3)
         ON CONFLICT(account, name) DO UPDATE SET last_seen = ?3",
        params![account, name, at],
    )?;
    Ok(())
}

fn seen_ip(db: &Connection, account: &str, ip: &str, at: i64) -> Result<()> {
    db.execute(
        "INSERT INTO ips(account, ip, first_seen, last_seen) VALUES(?1, ?2, ?3, ?3)
         ON CONFLICT(account, ip) DO UPDATE SET last_seen = ?3",
        params![account, ip, at],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn joins_names_addresses_and_leaves_are_kept() {
        let mut store = store();
        let a = "0123456789abcdef0123456789abcdef";
        let b = "fedcba9876543210fedcba9876543210";
        let session = store.record_join(a, "Ghost", Some("10.0.0.5"), "mp_rust", 100).unwrap();
        store.record_name(a, session, "Soap", 160).unwrap();
        store.record_leave(a, session, 400, "left").unwrap();
        store.record_join(b, "Roach", Some("10.0.0.5"), "mp_rust", 500).unwrap();

        let player = store.player(a).unwrap().unwrap();
        assert_eq!((player.name.as_str(), player.sessions, player.playtime), ("Soap", 1, 300));
        let names: Vec<String> = store.names(a).unwrap().into_iter().map(|n| n.0).collect();
        assert_eq!(names, ["Soap", "Ghost"]);
        assert_eq!(store.alts(a).unwrap(), [(b.to_owned(), "Roach".to_owned(), "10.0.0.5".to_owned())]);
        assert_eq!(store.find_players("gho", false, 10).unwrap().len(), 1);
        assert_eq!(store.find_players("10.0.0", false, 10).unwrap().len(), 0);
        assert_eq!(store.find_players("10.0.0", true, 10).unwrap().len(), 2);
        assert_eq!(store.find_players("fedc", false, 10).unwrap()[0].account, b);
        let sessions = store.sessions(a, 10).unwrap();
        assert_eq!((sessions[0].left_at, sessions[0].reason.as_deref()), (Some(400), Some("left")));
    }

    #[test]
    fn bans_expire_and_lift() {
        let mut store = store();
        let a = "0123456789abcdef0123456789abcdef";
        let ban = |account, ip, minutes| NewBan {
            account,
            ip,
            name: None,
            reason: "cheating",
            admin: "Console",
            minutes,
        };
        let forever = store.add_ban(&ban(Some(a), None, 0), 100).unwrap();
        store.add_ban(&ban(None, Some("10.0.0.5"), 10), 100).unwrap();
        assert_eq!(store.active_bans(200).unwrap().len(), 2);
        assert_eq!(store.active_bans(100 + 600).unwrap().len(), 1);
        assert_eq!(store.bans_matching(None, Some("10.0.0.5"), 200).unwrap().len(), 1);
        assert!(store.remove_ban(forever, "Console", 300).unwrap());
        assert!(!store.remove_ban(forever, "Console", 300).unwrap());
        assert!(store.bans_matching(Some(a), None, 400).unwrap().is_empty());
    }

    #[test]
    fn purge_drops_old_sessions_and_addresses() {
        let mut store = store();
        let a = "0123456789abcdef0123456789abcdef";
        let session = store.record_join(a, "Ghost", Some("10.0.0.5"), "mp_rust", 100).unwrap();
        store.record_leave(a, session, 200, "left").unwrap();
        store.purge(1000).unwrap();
        assert!(store.sessions(a, 10).unwrap().is_empty());
        assert!(store.ips(a).unwrap().is_empty());
        assert_eq!(store.player(a).unwrap().unwrap().ip, None);
    }
}
