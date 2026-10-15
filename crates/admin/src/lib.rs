//! A dedicated server's admin system: who the admins are (an admins file and groups of flag
//! letters), player history (joins, leaves, names, addresses) in one local database file, bans
//! by account and address, commands from the server console, from chat (`!kick`, `/ban`, `@`
//! admin chat) and from the web panel, chat votes and a flood guard.
//!
//! Gameplay is untouched: this layer reads the match (who is on, names, accounts, addresses
//! the master tells the server) and acts through the same paths the server already has (kick,
//! admission refusal, chat prints, console lines).

mod admins;
mod commands;
mod config;
mod daylog;
mod flags;
mod panel;
mod store;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, Instant};

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use sim::ClientId;

use crate::admins::{Access, Admins, account_hex};
use crate::config::AdminConfig;
use crate::daylog::DayLog;
use crate::store::{Db, Store, now};

/// The admin system, for a dedicated server.
pub struct AdminPlugin;

impl Plugin for AdminPlugin {
    fn build(&self, app: &mut App) {
        diag::enable_log_tap(2000);
        let dir = AdminConfig::admin_dir();
        let (config, mut warnings) = AdminConfig::load(&dir);
        let (admins, admin_warnings) = Admins::load(&dir);
        warnings.extend(admin_warnings);
        for warning in &warnings {
            diag::warn!(Console, "admin: {warning}");
        }
        let store = if config.history {
            match Store::open(&dir.join("players.db")) {
                Ok(mut store) => {
                    let at = now();
                    if let Err(error) = store.close_open_sessions(at) {
                        diag::warn!(Console, "admin: players.db: {error}");
                    }
                    Some(Db::new(store))
                }
                Err(error) => {
                    diag::error!(Console, "admin: players.db: {error}");
                    None
                }
            }
        } else {
            None
        };
        diag::info!(
            Console,
            "admin: {} admin(s) from {}, history {}",
            admins.len(),
            dir.display(),
            if store.is_some() { "on (players.db)" } else { "off" }
        );
        let panel = config
            .panel
            .then(|| panel::start(&config, &dir))
            .flatten();
        app.insert_resource(AdminState {
            log: DayLog::new(dir.join("logs")),
            config,
            admins,
            store,
            online: BTreeMap::new(),
            map: String::new(),
            started: Instant::now(),
            started_at: now(),
            gags: HashMap::new(),
            flood: HashMap::new(),
            vote: None,
            last_vote: None,
            bans_refreshed: None,
            purged: None,
            tracked: None,
            panel,
        })
        .add_systems(Startup, commands::register)
        .add_systems(
            Update,
            admin_frame.after(console::ConsoleDispatchSet),
        );
    }
}

/// A player on the server, as the admin system follows them.
#[derive(Clone, Debug)]
pub(crate) struct Online {
    pub(crate) client: ClientId,
    pub(crate) account: Option<[u8; 16]>,
    pub(crate) name: String,
    pub(crate) ip: Option<String>,
    pub(crate) bot: bool,
    /// When the server first saw them (their join is written once they have a name and an
    /// account, or after a few seconds without).
    pub(crate) seen: Instant,
    pub(crate) joined_at: i64,
    pub(crate) session: Option<i64>,
    pub(crate) recorded: bool,
    /// Kicked (their leave is written): gone at the next look.
    pub(crate) closed: bool,
    pub(crate) access: Option<Access>,
    /// Name, side and score, as the match last showed them.
    pub(crate) team: i32,
    pub(crate) alive: bool,
    pub(crate) kills: i32,
    pub(crate) deaths: i32,
    pub(crate) score: i32,
}

impl Online {
    pub(crate) fn account_hex(&self) -> Option<String> {
        self.account.as_ref().map(account_hex)
    }

    pub(crate) fn immunity(&self) -> i32 {
        self.access.map_or(0, |access| access.immunity)
    }
}

/// A running vote: what it decides, who said yes and no, and when it ends.
#[derive(Clone, Debug)]
pub(crate) struct Vote {
    pub(crate) kind: VoteKind,
    pub(crate) ends: Instant,
    pub(crate) yes: std::collections::HashSet<ClientId>,
    pub(crate) no: std::collections::HashSet<ClientId>,
}

#[derive(Clone, Debug)]
pub(crate) enum VoteKind {
    Kick { client: ClientId, name: String },
    Map(String),
}

#[derive(Resource)]
pub(crate) struct AdminState {
    pub(crate) config: AdminConfig,
    pub(crate) admins: Admins,
    pub(crate) store: Option<Db>,
    pub(crate) log: DayLog,
    pub(crate) online: BTreeMap<u32, Online>,
    pub(crate) map: String,
    pub(crate) started: Instant,
    pub(crate) started_at: i64,
    /// Gagged players, until when (none: until ungagged or they leave).
    pub(crate) gags: HashMap<ClientId, Option<Instant>>,
    pub(crate) flood: HashMap<ClientId, VecDeque<Instant>>,
    pub(crate) vote: Option<Vote>,
    pub(crate) last_vote: Option<Instant>,
    pub(crate) bans_refreshed: Option<Instant>,
    pub(crate) purged: Option<Instant>,
    pub(crate) tracked: Option<Instant>,
    pub(crate) panel: Option<panel::Bridge>,
}

/// What the admin system acts on in the server.
#[derive(SystemParam)]
pub(crate) struct Server<'w> {
    pub(crate) world: Option<ResMut<'w, net::AuthorityWorld>>,
    pub(crate) hub: Option<ResMut<'w, net::UdpAuthorityHub>>,
    pub(crate) faults: Option<ResMut<'w, net::PendingConnectionFaults>>,
    pub(crate) reliable: Option<ResMut<'w, net::ReliableEventHub>>,
    pub(crate) bans: Option<ResMut<'w, net::AdmissionBans>>,
    pub(crate) addresses: Option<Res<'w, net::MemberAddresses>>,
    pub(crate) refused: Option<ResMut<'w, net::RefusedAdmissions>>,
    pub(crate) inbox: Option<Res<'w, net::ClientCommandInbox>>,
    pub(crate) bots: Option<ResMut<'w, bots::BotRoster>>,
    pub(crate) queue: ResMut<'w, console::ConsoleQueue>,
}

impl Server<'_> {
    /// A chat line to one player, or to everyone.
    pub(crate) fn chat(&mut self, to: Option<ClientId>, text: &str) {
        if let Some(world) = self.world.as_mut() {
            world.0.print_chat(to, text);
        }
    }

    pub(crate) fn ping(&self, client: ClientId) -> Option<i32> {
        self.inbox.as_ref()?.latency_ms(client)
    }
}

fn admin_frame(
    mut state: ResMut<AdminState>,
    mut server: Server,
    mut installed: MessageReader<frame::MatchInstalled>,
    mut console_commands: MessageReader<console::ConsoleCommand>,
    mut console: ResMut<console::ConsoleState>,
    settings: Res<console::ConsoleSettings>,
) {
    let state = &mut *state;
    for match_installed in installed.read() {
        state.map = match_installed.zone.clone();
        state.log.line(&format!("MAP {}", state.map));
    }
    if let Some(world) = server.world.as_mut() {
        world.0.set_chat_commands(state.config.chat_commands);
    }
    track_players(state, &mut server);
    commands::run_console(state, &mut server, &mut console_commands, &mut console, settings.log_capacity);
    commands::run_chat(state, &mut server);
    commands::run_panel(state, &mut server);
    chat_log(state, &mut server);
    commands::tick_vote(state, &mut server);
    expire_gags(state, &mut server);
    if state
        .bans_refreshed
        .is_none_or(|at| at.elapsed() >= Duration::from_secs(30))
    {
        refresh_bans(state, &mut server);
    }
    if state
        .purged
        .is_none_or(|at| at.elapsed() >= Duration::from_secs(6 * 3600))
    {
        state.purged = Some(Instant::now());
        let before = now() - i64::from(state.config.history_days) * 86_400;
        if let Some(store) = state.store.as_mut().map(Db::get)
            && state.config.history_days > 0
        {
            match store.purge(before) {
                Ok(0) => {}
                Ok(gone) => diag::info!(Console, "admin: history older than {} days: {gone} row(s) deleted", state.config.history_days),
                Err(error) => diag::warn!(Console, "admin: purge: {error}"),
            }
        }
    }
    panel::publish(state, &server);
}

/// Follows who is on: writes joins (once a player has a name and an account), name and
/// address changes, and leaves; refused admissions too.
fn track_players(state: &mut AdminState, server: &mut Server) {
    if let Some(refused) = server.refused.as_mut() {
        for refusal in refused.0.drain(..) {
            let account = refusal
                .account
                .map(|account| account_hex(&account.0))
                .unwrap_or_else(|| "-".into());
            let ip = refusal.address.as_deref().unwrap_or("-");
            // Addresses go to the day's log and players.db only, never the shared server log.
            state
                .log
                .line(&format!("REFUSED {account} {ip}: {}", refusal.reason));
            diag::info!(Console, "admin: refused {account}: {}", refusal.reason);
        }
    }
    if state
        .tracked
        .is_some_and(|at| at.elapsed() < Duration::from_millis(250))
    {
        return;
    }
    state.tracked = Some(Instant::now());
    let Some(world) = server.world.as_ref() else {
        return;
    };
    let roster = world.0.roster();
    let peers = server
        .hub
        .as_ref()
        .map(|hub| hub.connected_peers())
        .unwrap_or_default();
    let bots: Vec<ClientId> = server
        .bots
        .as_ref()
        .map(|bots| bots.bots.iter().map(|bot| bot.id).collect())
        .unwrap_or_default();
    // Players are the connections with a slot; bots are the roster's.
    let mut present: Vec<u32> = peers.iter().map(|peer| peer.client.0).collect();
    for row in &roster {
        if bots.contains(&row.id) && !present.contains(&row.id.0) {
            present.push(row.id.0);
        }
    }
    let at = now();
    for &client in &present {
        let id = ClientId(client);
        let peer = peers.iter().find(|peer| peer.client == id);
        let row = roster.iter().find(|row| row.id == id);
        let account = peer.and_then(|peer| peer.account).map(|account| account.0);
        let ip = peer.and_then(|peer| {
            server
                .addresses
                .as_ref()
                .and_then(|addresses| addresses.0.get(&peer.member).cloned())
        });
        let name = row.map(|row| row.name.clone()).unwrap_or_default();
        let online = state.online.entry(client).or_insert_with(|| Online {
            client: id,
            account: None,
            name: String::new(),
            ip: None,
            bot: bots.contains(&id),
            seen: Instant::now(),
            joined_at: at,
            session: None,
            recorded: false,
            closed: false,
            access: None,
            team: 0,
            alive: false,
            kills: 0,
            deaths: 0,
            score: 0,
        });
        if online.closed {
            continue;
        }
        if let Some(row) = row {
            online.team = row.team;
            online.alive = row.alive;
            online.kills = row.kills;
            online.deaths = row.deaths;
            online.score = row.score;
        }
        if online.bot {
            online.name = name;
            online.recorded = true;
            continue;
        }
        // A player proves their account once their map has loaded, which can take a while.
        let proven_late = account.is_some() && online.account.is_none() && online.recorded;
        if account.is_some() && online.account.is_none() {
            online.account = account;
        }
        let new_ip = ip.filter(|ip| online.ip.as_ref() != Some(ip));
        if proven_late {
            online.access = state
                .admins
                .access(online.account.as_ref(), online.ip.as_deref());
            let account = online.account_hex().unwrap_or_default();
            state.log.line(&format!("ACCOUNT #{client} \"{}\" {account}", online.name));
            if let Some(store) = state.store.as_mut().map(Db::get) {
                match store.record_join(&account, &online.name, online.ip.as_deref(), &state.map, online.joined_at) {
                    Ok(session) => online.session = Some(session),
                    Err(error) => diag::warn!(Console, "admin: players.db: {error}"),
                }
            }
        }
        if !online.recorded {
            if !name.is_empty() {
                online.name = name.clone();
            }
            if let Some(ip) = new_ip {
                online.ip = Some(ip);
            }
            // Written once they have a name and their account (a server without accounts:
            // after a minute and a half).
            let ready = !online.name.is_empty()
                && (online.account.is_some() || online.seen.elapsed() >= Duration::from_secs(90));
            if !ready {
                continue;
            }
            online.recorded = true;
            online.joined_at = at;
            online.access = state
                .admins
                .access(online.account.as_ref(), online.ip.as_deref());
            let account = online.account_hex();
            let line = format!(
                "JOIN #{client} \"{}\" {}{}",
                online.name,
                account.as_deref().unwrap_or("(no account)"),
                online
                    .access
                    .map(|access| format!(" admin:{}", access.flags.letters()))
                    .unwrap_or_default()
            );
            state.log.line(&format!(
                "{line} {}",
                online.ip.as_deref().unwrap_or("-")
            ));
            diag::info!(Console, "admin: {line}");
            if let (Some(store), Some(account)) = (state.store.as_mut().map(Db::get), account.as_deref()) {
                match store.record_join(account, &online.name, online.ip.as_deref(), &state.map, at) {
                    Ok(session) => online.session = Some(session),
                    Err(error) => diag::warn!(Console, "admin: players.db: {error}"),
                }
            }
            continue;
        }
        if !name.is_empty() && name != online.name {
            let line = format!("NAME #{client} \"{}\" -> \"{name}\"", online.name);
            state.log.line(&line);
            diag::info!(Console, "admin: {line}");
            online.name = name;
            if let (Some(store), Some(account), Some(session)) =
                (state.store.as_mut().map(Db::get), online.account_hex(), online.session)
                && let Err(error) = store.record_name(&account, session, &online.name, at)
            {
                diag::warn!(Console, "admin: players.db: {error}");
            }
        }
        if let Some(ip) = new_ip {
            online.ip = Some(ip.clone());
            online.access = state
                .admins
                .access(online.account.as_ref(), online.ip.as_deref());
            if let (Some(store), Some(account), Some(session)) =
                (state.store.as_mut().map(Db::get), online.account_hex(), online.session)
                && let Err(error) = store.record_ip(&account, session, &ip, at)
            {
                diag::warn!(Console, "admin: players.db: {error}");
            }
        }
    }
    let gone: Vec<u32> = state
        .online
        .keys()
        .copied()
        .filter(|client| !present.contains(client))
        .collect();
    for client in gone {
        let Some(online) = state.online.remove(&client) else {
            continue;
        };
        state.gags.remove(&online.client);
        state.flood.remove(&online.client);
        if online.bot || !online.recorded || online.closed {
            continue;
        }
        let line = format!(
            "LEAVE #{client} \"{}\" {} after {}",
            online.name,
            online.account_hex().as_deref().unwrap_or("(no account)"),
            duration_text(at - online.joined_at)
        );
        state.log.line(&line);
        diag::info!(Console, "admin: {line}");
        if let (Some(store), Some(account), Some(session)) =
            (state.store.as_mut().map(Db::get), online.account_hex(), online.session)
            && let Err(error) = store.record_leave(&account, session, at, "left")
        {
            diag::warn!(Console, "admin: players.db: {error}");
        }
    }
    // A ban written while a player was already on (or before they proved their account).
    if server.bans.as_ref().is_some_and(|bans| {
        !bans.accounts.is_empty() || !bans.addresses.is_empty()
    }) {
        enforce_bans(state, server);
    }
}

/// Lines said, into the day's log; and the flood guard.
fn chat_log(state: &mut AdminState, server: &mut Server) {
    let Some(world) = server.world.as_mut() else {
        return;
    };
    let said = world.0.take_said();
    for (client, text) in said {
        let name = state
            .online
            .get(&client.0)
            .map(|online| online.name.clone())
            .unwrap_or_else(|| format!("#{}", client.0));
        state.log.line(&format!("CHAT \"{name}\": {text}"));
        if !state.config.antiflood
            || state
                .online
                .get(&client.0)
                .is_some_and(|online| online.access.is_some())
        {
            continue;
        }
        let window = Duration::from_secs_f32(state.config.flood_seconds);
        let lines = state.flood.entry(client).or_default();
        lines.push_back(Instant::now());
        while lines.front().is_some_and(|at| at.elapsed() > window) {
            lines.pop_front();
        }
        if lines.len() as u32 > state.config.flood_lines && state.config.flood_gag_seconds > 0 {
            lines.clear();
            let seconds = state.config.flood_gag_seconds;
            state.gags.insert(
                client,
                Some(Instant::now() + Duration::from_secs(u64::from(seconds))),
            );
            if let Some(world) = server.world.as_mut() {
                world.0.set_gagged(client, true);
                world.0.print_chat(
                    Some(client),
                    &format!("^3[Admin]^7 You are flooding the chat: quiet for {seconds} s."),
                );
            }
        }
    }
}

fn expire_gags(state: &mut AdminState, server: &mut Server) {
    let ended: Vec<ClientId> = state
        .gags
        .iter()
        .filter(|(_, until)| until.is_some_and(|until| Instant::now() >= until))
        .map(|(client, _)| *client)
        .collect();
    for client in ended {
        state.gags.remove(&client);
        if let Some(world) = server.world.as_mut() {
            world.0.set_gagged(client, false);
        }
    }
    // A new map is a new match world: the gags carry over.
    if let Some(world) = server.world.as_mut() {
        for client in state.gags.keys() {
            world.0.set_gagged(*client, true);
        }
    }
}

/// The refusal line a ban gives.
fn ban_message(ban: &store::Ban, at: i64) -> String {
    match ban.expires {
        Some(expires) => format!(
            "You are banned from this server ({} left): {}",
            duration_text(expires - at),
            ban.reason
        ),
        None => format!("You are banned from this server: {}", ban.reason),
    }
}

/// Reads the bans in force into the server's admission check, and kicks banned players on.
pub(crate) fn refresh_bans(state: &mut AdminState, server: &mut Server) {
    state.bans_refreshed = Some(Instant::now());
    let at = now();
    let Some(bans) = state
        .store
        .as_mut()
        .map(|store| store.get().active_bans(at))
    else {
        return;
    };
    let bans = match bans {
        Ok(bans) => bans,
        Err(error) => {
            diag::warn!(Console, "admin: bans: {error}");
            return;
        }
    };
    let Some(admission) = server.bans.as_mut() else {
        return;
    };
    admission.accounts.clear();
    admission.addresses.clear();
    for ban in &bans {
        let message = ban_message(ban, at);
        if let Some(account) = ban.account.as_deref().and_then(admins::parse_account) {
            admission
                .accounts
                .entry(sim::AccountId(account))
                .or_insert_with(|| message.clone());
        }
        if let Some(ip) = ban.ip.as_ref() {
            admission
                .addresses
                .entry(ip.clone())
                .or_insert(message);
        }
    }
    enforce_bans(state, server);
}

/// Kicks every player on the server whom a ban in force names.
fn enforce_bans(state: &mut AdminState, server: &mut Server) {
    let Some(admission) = server.bans.as_ref() else {
        return;
    };
    let kicks: Vec<(ClientId, String)> = state
        .online
        .values()
        .filter(|online| !online.bot && !online.closed)
        .filter_map(|online| {
            admission
                .refusal(
                    online.account.map(sim::AccountId),
                    online.ip.as_deref(),
                )
                .map(|reason| (online.client, reason.to_owned()))
        })
        .collect();
    for (client, reason) in kicks {
        kick(state, server, client, &reason);
    }
}

/// Removes a player from the server with `reason` on their screen (a bot leaves the roster).
pub(crate) fn kick(state: &mut AdminState, server: &mut Server, client: ClientId, reason: &str) {
    let Some(online) = state.online.get(&client.0).cloned() else {
        return;
    };
    if online.bot {
        if let Some(bots) = server.bots.as_mut() {
            bots.bots.retain(|bot| bot.id != client);
        }
        if let Some(world) = server.world.as_mut() {
            world.0.retire_client(client);
        }
        if let Some(online) = state.online.get_mut(&client.0) {
            online.closed = true;
        }
        return;
    }
    if let Some(reliable) = server.reliable.as_mut() {
        reliable
            .queue_mut(client)
            .push(net::ReliableRow::Failure(reason.to_owned()));
    }
    if let Some(faults) = server.faults.as_mut()
        && !faults.0.iter().any(|(queued, _)| *queued == client)
    {
        faults.0.push((client, reason.to_owned()));
    }
    // Their leave is written now, with the reason; the next look finds them gone.
    let at = now();
    if online.recorded {
        let line = format!(
            "LEAVE #{} \"{}\" {}: {reason}",
            client.0,
            online.name,
            online.account_hex().as_deref().unwrap_or("(no account)")
        );
        state.log.line(&line);
        if let (Some(store), Some(account), Some(session)) =
            (state.store.as_mut().map(Db::get), online.account_hex(), online.session)
            && let Err(error) = store.record_leave(&account, session, at, reason)
        {
            diag::warn!(Console, "admin: players.db: {error}");
        }
    }
    if let Some(online) = state.online.get_mut(&client.0) {
        online.closed = true;
    }
}

pub(crate) fn duration_text(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{}d {}h", seconds / 86_400, seconds % 86_400 / 3600),
    }
}

pub(crate) fn date_text(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}
