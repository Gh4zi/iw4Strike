//! Admin commands: `<name>` in the server console, `!name` (said) or `/name` (silent) in
//! chat, `@` admin chat, and the panel's command box. Each needs a flag letter (none: anyone)
//! and belongs to a part `admin.cfg` can turn off.

use std::time::{Duration, Instant};

use bevy::prelude::*;
use sim::ClientId;

use crate::admins::{Access, Admins, account_hex, parse_account};
use crate::flags::{self, Flags};
use crate::store::{Db, NewBan, now};
use crate::{AdminState, Online, Server, Vote, VoteKind, date_text, duration_text, kick};

/// Who runs a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Caller {
    Console,
    Player(ClientId),
    Panel(String),
}

pub(crate) struct Ctx {
    pub(crate) caller: Caller,
    pub(crate) access: Access,
    pub(crate) replies: Vec<String>,
}

impl Ctx {
    fn reply(&mut self, line: impl Into<String>) {
        self.replies.push(line.into());
    }

    fn me(&self) -> Option<ClientId> {
        match self.caller {
            Caller::Player(id) => Some(id),
            _ => None,
        }
    }

    fn is_console(&self) -> bool {
        self.caller == Caller::Console
    }

    fn name(&self, state: &AdminState) -> String {
        match &self.caller {
            Caller::Console => "Console".into(),
            Caller::Player(id) => state
                .online
                .get(&id.0)
                .map(|online| online.name.clone())
                .unwrap_or_else(|| format!("#{}", id.0)),
            Caller::Panel(user) => format!("{user} (panel)"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Core,
    Base,
    Bans,
    Chat,
    Fun,
    Votes,
    History,
    /// The server console only (the panel's users).
    ConsoleOnly,
}

type Run = fn(&mut AdminState, &mut Server, &mut Ctx, &[String]) -> Result<(), String>;

struct Command {
    name: &'static str,
    flag: Option<char>,
    part: Part,
    usage: &'static str,
    run: Run,
}

const COMMANDS: &[Command] = &[
    Command { name: "help", flag: None, part: Part::Core, usage: "the commands you can use", run: help },
    Command { name: "id", flag: None, part: Part::Core, usage: "your account id (for the admins file)", run: id },
    Command { name: "who", flag: Some(flags::GENERIC), part: Part::Base, usage: "the players on, with account, ping and admin flags", run: who },
    Command { name: "kick", flag: Some(flags::KICK), part: Part::Base, usage: "<target> [reason]", run: kick_command },
    Command { name: "map", flag: Some(flags::MAP), part: Part::Base, usage: "<map>", run: map },
    Command { name: "rcon", flag: Some(flags::RCON), part: Part::Base, usage: "<server console line>", run: rcon },
    Command { name: "cvar", flag: Some(flags::CVAR), part: Part::Base, usage: "<setting> [value]  (a game script setting)", run: cvar },
    Command { name: "reloadadmins", flag: Some(flags::GENERIC), part: Part::Core, usage: "read the admins files again", run: reload_admins },
    Command { name: "ban", flag: Some(flags::BAN), part: Part::Bans, usage: "<target> <minutes|0> [reason]  (0: forever)", run: ban },
    Command { name: "banip", flag: Some(flags::BAN), part: Part::Bans, usage: "<target|address> <minutes|0> [reason]", run: ban_ip },
    Command { name: "addban", flag: Some(flags::BAN), part: Part::Bans, usage: "<account|name> <minutes|0> [reason]  (a player who is not on)", run: add_ban },
    Command { name: "unban", flag: Some(flags::UNBAN), part: Part::Bans, usage: "<account|address|#ban>", run: unban },
    Command { name: "bans", flag: Some(flags::BAN), part: Part::Bans, usage: "the bans in force", run: bans },
    Command { name: "say", flag: Some(flags::CHAT), part: Part::Chat, usage: "<text>  (to everyone)", run: say },
    Command { name: "csay", flag: Some(flags::CHAT), part: Part::Chat, usage: "<text>  (middle of the screen)", run: csay },
    Command { name: "psay", flag: Some(flags::CHAT), part: Part::Chat, usage: "<target> <text>  (private)", run: psay },
    Command { name: "chat", flag: Some(flags::CHAT), part: Part::Chat, usage: "<text>  (to admins)", run: admin_chat },
    Command { name: "gag", flag: Some(flags::CHAT), part: Part::Chat, usage: "<target> [minutes]", run: gag },
    Command { name: "ungag", flag: Some(flags::CHAT), part: Part::Chat, usage: "<target>", run: ungag },
    Command { name: "slay", flag: Some(flags::SLAY), part: Part::Fun, usage: "<target>", run: slay },
    Command { name: "votekick", flag: Some(flags::VOTE), part: Part::Votes, usage: "<target>", run: vote_kick },
    Command { name: "votemap", flag: Some(flags::VOTE), part: Part::Votes, usage: "<map>", run: vote_map },
    Command { name: "yes", flag: None, part: Part::Votes, usage: "vote yes", run: vote_yes },
    Command { name: "no", flag: None, part: Part::Votes, usage: "vote no", run: vote_no },
    Command { name: "cancelvote", flag: Some(flags::VOTE), part: Part::Votes, usage: "stop the running vote", run: cancel_vote },
    Command { name: "history", flag: Some(flags::GENERIC), part: Part::History, usage: "<target|account|name>", run: history },
    Command { name: "lastseen", flag: Some(flags::GENERIC), part: Part::History, usage: "<name>", run: last_seen },
    Command { name: "ips", flag: Some(flags::ROOT), part: Part::History, usage: "<target|account>  (addresses used)", run: ips },
    Command { name: "alts", flag: Some(flags::ROOT), part: Part::History, usage: "<target|account>  (accounts sharing an address)", run: alts },
    Command { name: "panel_adduser", flag: Some(flags::ROOT), part: Part::ConsoleOnly, usage: "<name> [flags]  (a web panel login; prints its password)", run: panel_add_user },
    Command { name: "panel_passwd", flag: Some(flags::ROOT), part: Part::ConsoleOnly, usage: "<name>  (a new password)", run: panel_new_password },
    Command { name: "panel_deluser", flag: Some(flags::ROOT), part: Part::ConsoleOnly, usage: "<name>", run: panel_remove_user },
    Command { name: "panel_users", flag: Some(flags::ROOT), part: Part::ConsoleOnly, usage: "the web panel's logins", run: panel_users },
];

fn find(name: &str) -> Option<&'static Command> {
    let name = name.to_ascii_lowercase();
    COMMANDS.iter().find(|command| command.name == name)
}

/// Admin commands whose name the server console already has: there the console's own runs
/// (no second map change, a plain `say`); from chat and the panel they are the admin ones.
const CONSOLE_OWNED: [&str; 2] = ["map", "say"];

pub(crate) fn register(mut registry: ResMut<console::ConsoleRegistry>) {
    for command in COMMANDS {
        if !CONSOLE_OWNED.contains(&command.name) && registry.resolve(command.name).is_none() {
            registry.register(
                console::CommandSpec::new(command.name)
                    .usage(format!("admin: {}", command.usage)),
            );
        }
    }
}

fn enabled(state: &AdminState, part: Part) -> bool {
    let config = &state.config;
    match part {
        Part::Core | Part::ConsoleOnly => true,
        Part::Base => config.basecommands,
        Part::Bans => config.basebans && state.store.is_some(),
        Part::Chat => config.basechat,
        Part::Fun => config.funcommands,
        Part::Votes => config.votes,
        Part::History => state.store.is_some(),
    }
}

fn allowed(state: &AdminState, command: &Command, ctx: &Ctx) -> bool {
    if command.part == Part::ConsoleOnly && !ctx.is_console() {
        return false;
    }
    if command.part == Part::Votes && state.config.public_votes && ctx.me().is_some() {
        return true;
    }
    command.flag.is_none_or(|flag| ctx.access.flags.has(flag))
}

/// Runs `name args` for `ctx`: its replies are in `ctx.replies`.
pub(crate) fn execute(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, name: &str, args: &[String]) -> bool {
    let Some(command) = find(name) else {
        return false;
    };
    if !enabled(state, command.part) {
        ctx.reply(format!("{} is turned off on this server.", command.name));
        return true;
    }
    if !allowed(state, command, ctx) {
        ctx.reply("You do not have access to this command.");
        return true;
    }
    if let Err(error) = (command.run)(state, server, ctx, args) {
        ctx.reply(error);
    }
    true
}

fn console_access() -> Access {
    Access {
        flags: Flags::root(),
        immunity: i32::MAX,
    }
}

pub(crate) fn run_console(
    state: &mut AdminState,
    server: &mut Server,
    messages: &mut MessageReader<console::ConsoleCommand>,
    console: &mut console::ConsoleState,
    capacity: usize,
) {
    for message in messages.read() {
        if CONSOLE_OWNED.contains(&message.name.to_ascii_lowercase().as_str())
            || find(&message.name).is_none()
        {
            continue;
        }
        let mut ctx = Ctx {
            caller: Caller::Console,
            access: console_access(),
            replies: Vec::new(),
        };
        if execute(state, server, &mut ctx, &message.name, &message.args) {
            for line in ctx.replies {
                diag::info!(Console, "{line}");
                console.echo(line, capacity);
            }
        }
    }
}

fn player_access(state: &AdminState, id: ClientId) -> Access {
    state
        .online
        .get(&id.0)
        .and_then(|online| online.access)
        .unwrap_or_default()
}

/// Sends a player their replies in chat (the first lines; chat is short).
fn reply_in_chat(server: &mut Server, to: ClientId, replies: &[String]) {
    const LINES: usize = 10;
    for line in replies.iter().take(LINES) {
        server.chat(Some(to), &format!("^3[Admin]^7 {line}"));
    }
    if replies.len() > LINES {
        server.chat(
            Some(to),
            &format!("^3[Admin]^7 ... {} more line(s)", replies.len() - LINES),
        );
    }
}

pub(crate) fn run_chat(state: &mut AdminState, server: &mut Server) {
    let Some(world) = server.world.as_mut() else {
        return;
    };
    let lines = world.0.take_chat_commands();
    for (id, line) in lines {
        let mut ctx = Ctx {
            caller: Caller::Player(id),
            access: player_access(state, id),
            replies: Vec::new(),
        };
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('@') {
            if state.config.basechat {
                at_chat(state, server, &mut ctx, rest);
            }
        } else {
            let silent = trimmed.starts_with('/');
            let rest = &trimmed[1..];
            let Some(command) = console::ConsoleCommand::parse(rest) else {
                continue;
            };
            let name = command.name.to_ascii_lowercase();
            let name = match name.as_str() {
                "y" => "yes",
                "n" => "no",
                other => other,
            };
            let state_name = name.to_owned();
            // `!yes` and `!no` are only votes while one runs.
            if matches!(state_name.as_str(), "yes" | "no") && state.vote.is_none() {
                continue;
            }
            if !execute(state, server, &mut ctx, &state_name, &command.args) && silent {
                ctx.reply(format!("Unknown command: {}", command.name));
            }
            if ctx.replies.is_empty() {
                continue;
            }
        }
        let replies = std::mem::take(&mut ctx.replies);
        reply_in_chat(server, id, &replies);
    }
}

/// `@text` to admins, `@@target text` private, `@@@text` to the middle of the screen.
fn at_chat(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, rest: &str) {
    if let Some(rest) = rest.strip_prefix("@@") {
        if ctx.access.flags.has(flags::CHAT) {
            let _ = csay(state, server, ctx, &[rest.trim().to_owned()]);
        } else {
            ctx.reply("You do not have access to this command.");
        }
    } else if let Some(rest) = rest.strip_prefix('@') {
        if ctx.access.flags.has(flags::CHAT) {
            let words: Vec<String> = rest.split_whitespace().map(str::to_owned).collect();
            if let Err(error) = psay(state, server, ctx, &words) {
                ctx.reply(error);
            }
        } else {
            ctx.reply("You do not have access to this command.");
        }
    } else {
        let _ = admin_chat(state, server, ctx, &[rest.trim().to_owned()]);
    }
}

/// The panel's command lines, as its user.
pub(crate) fn run_panel(state: &mut AdminState, server: &mut Server) {
    let requests = match state.panel.as_ref() {
        Some(panel) => panel.take_requests(),
        None => return,
    };
    for request in requests {
        let flags = Flags::parse(&request.flags);
        let mut ctx = Ctx {
            caller: Caller::Panel(request.user.clone()),
            access: Access {
                flags,
                immunity: if flags.is_root() { i32::MAX - 1 } else { 0 },
            },
            replies: Vec::new(),
        };
        let line = request.line.trim();
        match console::ConsoleCommand::parse(line) {
            Some(command) if find(&command.name).is_some() => {
                execute(state, server, &mut ctx, &command.name, &command.args);
            }
            Some(_) if flags.has(flags::RCON) => {
                server.queue.push_line(line);
                audit(state, &ctx, "rcon", None, Some(line));
                ctx.reply(format!("> {line}"));
            }
            Some(_) => ctx.reply("Not an admin command (help lists yours); other console lines need the rcon flag (m)."),
            None => {}
        }
        let _ = request.reply.send(ctx.replies);
    }
}

/// Writes an admin action to the audit log, the day's log and the server log.
fn audit(state: &mut AdminState, ctx: &Ctx, action: &str, target: Option<&str>, detail: Option<&str>) {
    let admin = ctx.name(state);
    let line = format!(
        "ADMIN \"{admin}\" {action}{}{}",
        target.map(|target| format!(" {target}")).unwrap_or_default(),
        detail.map(|detail| format!(": {detail}")).unwrap_or_default()
    );
    state.log.line(&line);
    diag::info!(Console, "admin: {line}");
    if let Some(store) = state.store.as_mut().map(Db::get)
        && let Err(error) = store.audit(now(), &admin, action, target, detail)
    {
        diag::warn!(Console, "admin: players.db: {error}");
    }
}

/// Tells everyone what an admin did.
fn activity(state: &AdminState, server: &mut Server, ctx: &Ctx, text: &str) {
    server.chat(None, &format!("^3[Admin]^7 {}: {text}", ctx.name(state)));
}

fn rest(args: &[String]) -> String {
    args.join(" ")
}

/// The players `pattern` names: `#slot`, `@all`, `@bots`, `@humans`, `@alive`, `@dead`, `@me`,
/// `@!me`, an account id's start, or part of a name. A group only where `many` allows it.
fn targets(state: &AdminState, ctx: &Ctx, pattern: &str, many: bool) -> Result<Vec<ClientId>, String> {
    let on: Vec<&Online> = state
        .online
        .values()
        .filter(|online| online.recorded && !online.closed)
        .collect();
    let lower = pattern.to_ascii_lowercase();
    let group: Option<Vec<&Online>> = match lower.as_str() {
        "@all" => Some(on.clone()),
        "@bots" => Some(on.iter().copied().filter(|o| o.bot).collect()),
        "@humans" => Some(on.iter().copied().filter(|o| !o.bot).collect()),
        "@alive" => Some(on.iter().copied().filter(|o| o.alive).collect()),
        "@dead" => Some(on.iter().copied().filter(|o| !o.alive).collect()),
        "@!me" => Some(
            on.iter()
                .copied()
                .filter(|o| Some(o.client) != ctx.me())
                .collect(),
        ),
        _ => None,
    };
    if let Some(group) = group {
        if !many {
            return Err("Name one player here.".into());
        }
        let list: Vec<ClientId> = group
            .into_iter()
            .filter(|o| can_target(ctx, o))
            .map(|o| o.client)
            .collect();
        if list.is_empty() {
            return Err(format!("No player matches `{pattern}`."));
        }
        return Ok(list);
    }
    let found: Vec<&Online> = if lower == "@me" {
        ctx.me()
            .and_then(|me| state.online.get(&me.0))
            .into_iter()
            .collect()
    } else if let Some(slot) = lower.strip_prefix('#') {
        let slot: u32 = slot
            .parse()
            .map_err(|_| format!("`{pattern}` is no slot number."))?;
        on.iter().copied().filter(|o| o.client.0 == slot).collect()
    } else {
        let exact: Vec<&Online> = on
            .iter()
            .copied()
            .filter(|o| o.name.eq_ignore_ascii_case(pattern))
            .collect();
        if !exact.is_empty() {
            exact
        } else {
            on.iter()
                .copied()
                .filter(|o| {
                    o.name.to_ascii_lowercase().contains(&lower)
                        || (lower.len() >= 4
                            && o.account_hex().is_some_and(|account| account.starts_with(&lower)))
                })
                .collect()
        }
    };
    match found.as_slice() {
        [] => Err(format!("No player matches `{pattern}`.")),
        [one] => {
            if can_target(ctx, one) {
                Ok(vec![one.client])
            } else {
                Err(format!("You cannot target {} (immunity).", one.name))
            }
        }
        many => Err(format!(
            "More than one player matches `{pattern}`: {}. Use #slot.",
            many.iter()
                .map(|o| format!("#{} {}", o.client.0, o.name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// An admin targets themselves, or players with no more immunity than theirs.
fn can_target(ctx: &Ctx, target: &Online) -> bool {
    Some(target.client) == ctx.me() || target.immunity() <= ctx.access.immunity
}

fn target_one(state: &AdminState, ctx: &Ctx, pattern: &str) -> Result<Online, String> {
    let id = targets(state, ctx, pattern, false)?[0];
    state
        .online
        .get(&id.0)
        .cloned()
        .ok_or_else(|| "That player left.".into())
}

/// An account from a player on the server, an account id, or a stored player (by the start of
/// their account or part of a name they used): (account, last name).
fn resolve_account(state: &AdminState, ctx: &Ctx, pattern: &str) -> Result<(String, String), String> {
    if let Some(account) = parse_account(pattern) {
        let account = account_hex(&account);
        let name = state
            .store
            .as_ref()
            .and_then(|store| store.lock().player(&account).ok().flatten())
            .map(|player| player.name)
            .unwrap_or_default();
        return Ok((account, name));
    }
    if let Ok(found) = targets(state, ctx, pattern, false)
        && let Some(online) = state.online.get(&found[0].0)
    {
        return online
            .account_hex()
            .map(|account| (account, online.name.clone()))
            .ok_or_else(|| format!("{} has no account (a bot?).", online.name));
    }
    let store = state.store.as_ref().map(Db::lock).ok_or("The player history is off.")?;
    let found = store
        .find_players(pattern, false, 6)
        .map_err(|error| error.to_string())?;
    match found.as_slice() {
        [] => Err(format!("No player matches `{pattern}`.")),
        [one] => Ok((one.account.clone(), one.name.clone())),
        many => Err(format!(
            "More than one player matches `{pattern}`: {}.",
            many.iter()
                .map(|p| format!("{} ({})", p.name, &p.account[..8]))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn minutes_arg(text: Option<&String>) -> Result<u64, String> {
    text.ok_or("Give the minutes (0: forever).")?
        .parse::<u64>()
        .map_err(|_| "The minutes are a whole number (0: forever).".to_owned())
}

fn length_text(minutes: u64) -> String {
    if minutes == 0 {
        "forever".into()
    } else {
        format!("for {}", duration_text(minutes as i64 * 60))
    }
}

fn valid_map(map: &str) -> Result<(), String> {
    if !map.is_empty()
        && map.len() <= 64
        && map.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Ok(())
    } else {
        Err(format!("`{map}` is no map name."))
    }
}

// ---- Core ----

fn help(state: &mut AdminState, _: &mut Server, ctx: &mut Ctx, _: &[String]) -> Result<(), String> {
    let prefix = if ctx.me().is_some() { "!" } else { "" };
    let lines: Vec<String> = COMMANDS
        .iter()
        .filter(|command| enabled(state, command.part) && allowed(state, command, ctx))
        .filter(|command| !matches!(command.name, "yes" | "no") || state.vote.is_some())
        .map(|command| format!("{prefix}{} {}", command.name, command.usage))
        .collect();
    ctx.replies.extend(lines);
    if ctx.me().is_some() {
        ctx.reply("! is seen by everyone, / is silent, @text goes to the admins.");
    }
    Ok(())
}

fn id(state: &mut AdminState, _: &mut Server, ctx: &mut Ctx, _: &[String]) -> Result<(), String> {
    match ctx.me().and_then(|me| state.online.get(&me.0)) {
        Some(online) => match online.account_hex() {
            Some(account) => ctx.reply(format!("Your account id: {account}")),
            None => ctx.reply("Your account is not proven yet."),
        },
        None => ctx.reply("Only players have an account id."),
    }
    Ok(())
}

fn who(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, _: &[String]) -> Result<(), String> {
    let show_ip = ctx.access.flags.is_root();
    let lines: Vec<String> = state
        .online
        .values()
        .filter(|online| online.recorded && !online.closed)
        .map(|online| {
            let account = online
                .account_hex()
                .map_or_else(|| if online.bot { "bot".into() } else { "-".into() }, |a| a[..8].to_owned());
            let ping = server
                .ping(online.client)
                .map(|ping| format!("{ping}ms"))
                .unwrap_or_else(|| "-".into());
            let admin = online
                .access
                .map(|access| format!(" [{}]", access.flags.letters()))
                .unwrap_or_default();
            let ip = if show_ip && !online.bot {
                format!(" {}", online.ip.as_deref().unwrap_or("-"))
            } else {
                String::new()
            };
            format!("#{} {}  {account}  {ping}{admin}{ip}", online.client.0, online.name)
        })
        .collect();
    ctx.reply(format!(
        "{} on {}, {} player(s)",
        crate::panel::host_name(),
        if state.map.is_empty() { "-" } else { &state.map },
        lines.len()
    ));
    ctx.replies.extend(lines);
    Ok(())
}

fn reload_admins(state: &mut AdminState, _: &mut Server, ctx: &mut Ctx, _: &[String]) -> Result<(), String> {
    let (admins, warnings) = Admins::load(&state.config.dir);
    state.admins = admins;
    for online in state.online.values_mut() {
        online.access = state
            .admins
            .access(online.account.as_ref(), online.ip.as_deref());
    }
    ctx.reply(format!("{} admin(s) loaded.", state.admins.len()));
    ctx.replies.extend(warnings);
    audit(state, ctx, "reloadadmins", None, None);
    Ok(())
}

// ---- Base ----

fn kick_command(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let pattern = args.first().ok_or("Usage: kick <target> [reason]")?;
    let reason = match rest(&args[1..]) {
        reason if reason.is_empty() => "Kicked by an admin".to_owned(),
        reason => reason,
    };
    for client in targets(state, ctx, pattern, true)? {
        let Some(online) = state.online.get(&client.0).cloned() else {
            continue;
        };
        kick(state, server, client, &reason);
        audit(state, ctx, "kick", Some(&online.name), Some(&reason));
        activity(state, server, ctx, &format!("kicked {} ({reason})", online.name));
    }
    Ok(())
}

fn map(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let map = args.first().ok_or("Usage: map <map>")?;
    valid_map(map)?;
    activity(state, server, ctx, &format!("changing the map to {map}"));
    audit(state, ctx, "map", Some(map), None);
    server.queue.push_line(&format!("map {map}"));
    Ok(())
}

fn rcon(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let line = rest(args);
    if line.is_empty() {
        return Err("Usage: rcon <server console line>".into());
    }
    server.queue.push_line(&line);
    audit(state, ctx, "rcon", None, Some(&line));
    ctx.reply(format!("> {line}"));
    Ok(())
}

fn cvar(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let name = args.first().ok_or("Usage: cvar <setting> [value]")?;
    let world = server.world.as_mut().ok_or("No match is running.")?;
    if args.len() == 1 {
        let value = world.0.gsc_dvar(name);
        ctx.reply(format!("{name} = \"{}\"", value.unwrap_or_default()));
        return Ok(());
    }
    if name.to_ascii_lowercase().contains("password") && !ctx.access.flags.is_root() {
        return Err("That setting needs the root flag (z).".into());
    }
    let value = rest(&args[1..]);
    world.0.set_gsc_dvar(name, &value);
    audit(state, ctx, "cvar", Some(name), Some(&value));
    activity(state, server, ctx, &format!("set {name} to \"{value}\""));
    Ok(())
}

// ---- Bans ----

fn add_ban_row(state: &mut AdminState, ctx: &Ctx, ban: NewBan<'_>) -> Result<i64, String> {
    let store = state.store.as_mut().map(Db::get).ok_or("The player history is off.")?;
    let id = store.add_ban(&ban, now()).map_err(|error| error.to_string())?;
    let target = ban
        .account
        .or(ban.ip)
        .map(str::to_owned)
        .unwrap_or_default();
    let detail = format!(
        "#{id} {} {}: {}",
        ban.name.unwrap_or(""),
        length_text(ban.minutes),
        ban.reason
    );
    audit(state, ctx, "ban", Some(&target), Some(&detail));
    Ok(id)
}

fn ban(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let pattern = args.first().ok_or("Usage: ban <target> <minutes|0> [reason]")?;
    let target = target_one(state, ctx, pattern)?;
    let minutes = minutes_arg(args.get(1))?;
    let reason = match rest(args.get(2..).unwrap_or_default()) {
        reason if reason.is_empty() => "Banned by an admin".to_owned(),
        reason => reason,
    };
    let account = target
        .account_hex()
        .ok_or_else(|| format!("{} has no account (a bot?).", target.name))?;
    let admin = ctx.name(state);
    let id = add_ban_row(
        state,
        ctx,
        NewBan {
            account: Some(&account),
            ip: None,
            name: Some(&target.name),
            reason: &reason,
            admin: &admin,
            minutes,
        },
    )?;
    activity(state, server, ctx, &format!("banned {} {} ({reason})", target.name, length_text(minutes)));
    crate::refresh_bans(state, server);
    ctx.reply(format!("Ban #{id} written."));
    Ok(())
}

fn ban_ip(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let pattern = args.first().ok_or("Usage: banip <target|address> <minutes|0> [reason]")?;
    let (ip, name) = if pattern.parse::<std::net::IpAddr>().is_ok() {
        (pattern.clone(), None)
    } else {
        let target = target_one(state, ctx, pattern)?;
        let ip = target
            .ip
            .clone()
            .ok_or_else(|| format!("{}'s address is not known.", target.name))?;
        (ip, Some(target.name))
    };
    let minutes = minutes_arg(args.get(1))?;
    let reason = match rest(args.get(2..).unwrap_or_default()) {
        reason if reason.is_empty() => "Banned by an admin".to_owned(),
        reason => reason,
    };
    let admin = ctx.name(state);
    let id = add_ban_row(
        state,
        ctx,
        NewBan {
            account: None,
            ip: Some(&ip),
            name: name.as_deref(),
            reason: &reason,
            admin: &admin,
            minutes,
        },
    )?;
    if let Some(name) = name {
        activity(state, server, ctx, &format!("banned {name} {} ({reason})", length_text(minutes)));
    }
    crate::refresh_bans(state, server);
    ctx.reply(format!("Address ban #{id} written."));
    Ok(())
}

fn add_ban(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let pattern = args.first().ok_or("Usage: addban <account|name> <minutes|0> [reason]")?;
    let (account, name) = resolve_account(state, ctx, pattern)?;
    let minutes = minutes_arg(args.get(1))?;
    let reason = match rest(args.get(2..).unwrap_or_default()) {
        reason if reason.is_empty() => "Banned by an admin".to_owned(),
        reason => reason,
    };
    let admin = ctx.name(state);
    let id = add_ban_row(
        state,
        ctx,
        NewBan {
            account: Some(&account),
            ip: None,
            name: (!name.is_empty()).then_some(name.as_str()),
            reason: &reason,
            admin: &admin,
            minutes,
        },
    )?;
    crate::refresh_bans(state, server);
    ctx.reply(format!(
        "Ban #{id} written for {} ({account}) {}.",
        if name.is_empty() { "an unseen player" } else { &name },
        length_text(minutes)
    ));
    Ok(())
}

fn unban(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let pattern = args.first().ok_or("Usage: unban <account|address|#ban>")?;
    let admin = ctx.name(state);
    let at = now();
    let store = state.store.as_mut().map(Db::get).ok_or("The player history is off.")?;
    let ids: Vec<i64> = if let Some(id) = pattern.strip_prefix('#') {
        vec![id.parse().map_err(|_| format!("`{pattern}` is no ban number."))?]
    } else {
        let account = parse_account(pattern).map(|account| account_hex(&account));
        let ip = pattern
            .parse::<std::net::IpAddr>()
            .is_ok()
            .then(|| pattern.clone());
        if account.is_none() && ip.is_none() {
            return Err("Give an account id (32 hex digits), an address or #ban.".into());
        }
        store
            .bans_matching(account.as_deref(), ip.as_deref(), at)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|ban| ban.id)
            .collect()
    };
    let mut lifted = 0;
    for id in &ids {
        if store.remove_ban(*id, &admin, at).map_err(|error| error.to_string())? {
            lifted += 1;
        }
    }
    if lifted == 0 {
        return Err(format!("No ban in force matches `{pattern}`."));
    }
    audit(state, ctx, "unban", Some(pattern), None);
    crate::refresh_bans(state, server);
    ctx.reply(format!("{lifted} ban(s) lifted."));
    Ok(())
}

fn bans(state: &mut AdminState, _: &mut Server, ctx: &mut Ctx, _: &[String]) -> Result<(), String> {
    let at = now();
    let store = state.store.as_ref().map(Db::lock).ok_or("The player history is off.")?;
    let bans = store.active_bans(at).map_err(|error| error.to_string())?;
    if bans.is_empty() {
        ctx.reply("No bans in force.");
    }
    let show_ip = ctx.access.flags.is_root();
    for ban in bans.iter().take(30) {
        let who = match (&ban.account, &ban.ip) {
            (Some(account), _) => account[..8].to_owned(),
            (None, Some(ip)) if show_ip => ip.clone(),
            (None, Some(_)) => "address".into(),
            (None, None) => "-".into(),
        };
        let left = ban
            .expires
            .map_or_else(|| "forever".into(), |expires| format!("{} left", duration_text(expires - at)));
        ctx.reply(format!(
            "#{} {} {} ({left}) by {}: {}",
            ban.id,
            ban.name.as_deref().unwrap_or(""),
            who,
            ban.admin,
            ban.reason
        ));
    }
    Ok(())
}

// ---- Chat ----

fn say(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let text = rest(args);
    if text.is_empty() {
        return Err("Usage: say <text>".into());
    }
    server.chat(None, &format!("^3[Admin] {}:^7 {text}", ctx.name(state)));
    state.log.line(&format!("SAY \"{}\": {text}", ctx.name(state)));
    Ok(())
}

fn csay(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let text = rest(args);
    if text.is_empty() {
        return Err("Usage: csay <text>".into());
    }
    if let Some(world) = server.world.as_mut() {
        world.0.print_center(&format!("{}: {text}", ctx.name(state)));
    }
    state.log.line(&format!("CSAY \"{}\": {text}", ctx.name(state)));
    Ok(())
}

fn psay(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let pattern = args.first().ok_or("Usage: psay <target> <text>")?;
    let text = rest(&args[1..]);
    if text.is_empty() {
        return Err("Usage: psay <target> <text>".into());
    }
    let target = target_one(state, ctx, pattern)?;
    server.chat(
        Some(target.client),
        &format!("^3[Private] {}:^7 {text}", ctx.name(state)),
    );
    state
        .log
        .line(&format!("PSAY \"{}\" -> \"{}\": {text}", ctx.name(state), target.name));
    ctx.reply(format!("To {}: {text}", target.name));
    Ok(())
}

fn admin_chat(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let text = rest(args);
    if text.is_empty() {
        return Err("Usage: chat <text>".into());
    }
    let from_admin = ctx.access.flags.any();
    let line = format!(
        "^3({}) {}:^7 {text}",
        if from_admin { "Admins" } else { "To admins" },
        ctx.name(state)
    );
    let admins: Vec<ClientId> = state
        .online
        .values()
        .filter(|online| online.access.is_some_and(|access| access.flags.has(flags::CHAT)))
        .map(|online| online.client)
        .collect();
    for admin in &admins {
        server.chat(Some(*admin), &line);
    }
    state.log.line(&format!("ADMINCHAT \"{}\": {text}", ctx.name(state)));
    diag::info!(Console, "admin chat: {}: {text}", ctx.name(state));
    if !from_admin {
        ctx.reply(if admins.is_empty() {
            "No admin is on; your message is in the server log."
        } else {
            "Sent to the admins."
        });
    }
    Ok(())
}

fn gag(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let pattern = args.first().ok_or("Usage: gag <target> [minutes]")?;
    let minutes = args.get(1).map(|m| m.parse::<u64>()).transpose().map_err(|_| "The minutes are a whole number.")?;
    for client in targets(state, ctx, pattern, true)? {
        let until = minutes
            .filter(|minutes| *minutes > 0)
            .map(|minutes| Instant::now() + Duration::from_secs(minutes * 60));
        state.gags.insert(client, until);
        if let Some(world) = server.world.as_mut() {
            world.0.set_gagged(client, true);
        }
        let name = state.online.get(&client.0).map(|o| o.name.clone()).unwrap_or_default();
        audit(state, ctx, "gag", Some(&name), minutes.map(|m| format!("{m} min")).as_deref());
        activity(state, server, ctx, &format!("gagged {name}"));
    }
    Ok(())
}

fn ungag(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let pattern = args.first().ok_or("Usage: ungag <target>")?;
    for client in targets(state, ctx, pattern, true)? {
        state.gags.remove(&client);
        if let Some(world) = server.world.as_mut() {
            world.0.set_gagged(client, false);
        }
        let name = state.online.get(&client.0).map(|o| o.name.clone()).unwrap_or_default();
        audit(state, ctx, "ungag", Some(&name), None);
        activity(state, server, ctx, &format!("ungagged {name}"));
    }
    Ok(())
}

// ---- Fun ----

fn slay(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let pattern = args.first().ok_or("Usage: slay <target>")?;
    for client in targets(state, ctx, pattern, true)? {
        let Some(online) = state.online.get(&client.0).cloned() else {
            continue;
        };
        if !online.alive {
            continue;
        }
        if let Some(world) = server.world.as_mut() {
            world.0.admin_slay(client);
        }
        audit(state, ctx, "slay", Some(&online.name), None);
        activity(state, server, ctx, &format!("slayed {}", online.name));
    }
    Ok(())
}

// ---- Votes ----

fn start_vote(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, kind: VoteKind) -> Result<(), String> {
    if state.vote.is_some() {
        return Err("A vote is already running.".into());
    }
    if !ctx.access.flags.any()
        && let Some(last) = state.last_vote
        && last.elapsed() < Duration::from_secs(60)
    {
        return Err(format!(
            "Wait {} s for the next vote.",
            60 - last.elapsed().as_secs()
        ));
    }
    let seconds = state.config.vote_seconds;
    let question = match &kind {
        VoteKind::Kick { name, .. } => format!("kick {name}"),
        VoteKind::Map(map) => format!("change the map to {map}"),
    };
    let mut vote = Vote {
        kind,
        ends: Instant::now() + Duration::from_secs(u64::from(seconds)),
        yes: Default::default(),
        no: Default::default(),
    };
    if let Some(me) = ctx.me() {
        vote.yes.insert(me);
    }
    state.vote = Some(vote);
    state.last_vote = Some(Instant::now());
    server.chat(
        None,
        &format!(
            "^3[Vote]^7 {} wants to {question}. Say ^2!yes^7 or ^1!no^7 ({seconds} s).",
            ctx.name(state)
        ),
    );
    audit(state, ctx, "vote", None, Some(&question));
    Ok(())
}

fn vote_kick(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let pattern = args.first().ok_or("Usage: votekick <target>")?;
    let target = target_one(state, ctx, pattern)?;
    if Some(target.client) == ctx.me() {
        return Err("You cannot vote yourself off.".into());
    }
    if target.immunity() > 0 && !ctx.is_console() {
        return Err(format!("{} cannot be voted off (admin).", target.name));
    }
    start_vote(
        state,
        server,
        ctx,
        VoteKind::Kick {
            client: target.client,
            name: target.name,
        },
    )
}

fn vote_map(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let map = args.first().ok_or("Usage: votemap <map>")?;
    valid_map(map)?;
    start_vote(state, server, ctx, VoteKind::Map(map.clone()))
}

fn cast(state: &mut AdminState, ctx: &mut Ctx, yes: bool) -> Result<(), String> {
    let me = ctx.me().ok_or("Only players vote.")?;
    let vote = state.vote.as_mut().ok_or("No vote is running.")?;
    if yes {
        vote.no.remove(&me);
        vote.yes.insert(me);
    } else {
        vote.yes.remove(&me);
        vote.no.insert(me);
    }
    ctx.reply(format!("Voted {}. Yes {}, no {}.", if yes { "yes" } else { "no" }, vote.yes.len(), vote.no.len()));
    Ok(())
}

fn vote_yes(state: &mut AdminState, _: &mut Server, ctx: &mut Ctx, _: &[String]) -> Result<(), String> {
    cast(state, ctx, true)
}

fn vote_no(state: &mut AdminState, _: &mut Server, ctx: &mut Ctx, _: &[String]) -> Result<(), String> {
    cast(state, ctx, false)
}

fn cancel_vote(state: &mut AdminState, server: &mut Server, ctx: &mut Ctx, _: &[String]) -> Result<(), String> {
    state.vote.take().ok_or("No vote is running.")?;
    server.chat(None, &format!("^3[Vote]^7 {} stopped the vote.", ctx.name(state)));
    audit(state, ctx, "cancelvote", None, None);
    Ok(())
}

/// Ends the vote when its time is up or every player has voted.
pub(crate) fn tick_vote(state: &mut AdminState, server: &mut Server) {
    let Some(vote) = state.vote.as_mut() else {
        return;
    };
    let humans: Vec<ClientId> = state
        .online
        .values()
        .filter(|online| online.recorded && !online.closed && !online.bot)
        .map(|online| online.client)
        .collect();
    vote.yes.retain(|id| humans.contains(id));
    vote.no.retain(|id| humans.contains(id));
    let everyone = vote.yes.len() + vote.no.len() >= humans.len();
    if Instant::now() < vote.ends && !everyone {
        return;
    }
    let Some(vote) = state.vote.take() else {
        return;
    };
    let (yes, no) = (vote.yes.len(), vote.no.len());
    let passed = yes > 0 && yes as f32 >= state.config.vote_ratio * (yes + no) as f32;
    let question = match &vote.kind {
        VoteKind::Kick { name, .. } => format!("kick {name}"),
        VoteKind::Map(map) => format!("change the map to {map}"),
    };
    server.chat(
        None,
        &format!(
            "^3[Vote]^7 {question}: {} (yes {yes}, no {no}).",
            if passed { "^2passed^7" } else { "^1failed^7" }
        ),
    );
    state.log.line(&format!(
        "VOTE {question}: {} (yes {yes}, no {no})",
        if passed { "passed" } else { "failed" }
    ));
    if !passed {
        return;
    }
    match vote.kind {
        VoteKind::Kick { client, .. } => kick(state, server, client, "Kicked by vote"),
        VoteKind::Map(map) => server.queue.push_line(&format!("map {map}")),
    }
}

// ---- History ----

fn history(state: &mut AdminState, _: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let pattern = args.first().ok_or("Usage: history <target|account|name>")?;
    let (account, _) = resolve_account(state, ctx, pattern)?;
    let store = state.store.as_ref().map(Db::lock).ok_or("The player history is off.")?;
    let player = store
        .player(&account)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("No history for {account}."))?;
    ctx.reply(format!(
        "{} ({account}): first seen {}, last seen {}, {} session(s), played {}",
        player.name,
        date_text(player.first_seen),
        date_text(player.last_seen),
        player.sessions,
        duration_text(player.playtime)
    ));
    let names = store.names(&account).map_err(|error| error.to_string())?;
    ctx.reply(format!(
        "Names: {}",
        names.iter().map(|(name, _, _)| name.as_str()).collect::<Vec<_>>().join(", ")
    ));
    for session in store.sessions(&account, 5).map_err(|error| error.to_string())? {
        ctx.reply(format!(
            "{} {} {}{}",
            date_text(session.joined_at),
            session.map.as_deref().unwrap_or("-"),
            session
                .left_at
                .map_or_else(|| "on now".into(), |left| duration_text(left - session.joined_at)),
            session.reason.map(|reason| format!(" ({reason})")).unwrap_or_default()
        ));
    }
    let bans = store.bans_of(&account).map_err(|error| error.to_string())?;
    if !bans.is_empty() {
        ctx.reply(format!("{} ban(s) on record; the latest: {}", bans.len(), bans[0].reason));
    }
    Ok(())
}

fn last_seen(state: &mut AdminState, _: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let name = rest(args);
    if name.is_empty() {
        return Err("Usage: lastseen <name>".into());
    }
    let store = state.store.as_ref().map(Db::lock).ok_or("The player history is off.")?;
    let found = store.find_players(&name, false, 8).map_err(|error| error.to_string())?;
    if found.is_empty() {
        ctx.reply(format!("Nobody named like `{name}` has played here."));
    }
    for player in found {
        ctx.reply(format!(
            "{} ({}) last seen {}",
            player.name,
            &player.account[..8],
            date_text(player.last_seen)
        ));
    }
    Ok(())
}

fn ips(state: &mut AdminState, _: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let pattern = args.first().ok_or("Usage: ips <target|account>")?;
    let (account, name) = resolve_account(state, ctx, pattern)?;
    let store = state.store.as_ref().map(Db::lock).ok_or("The player history is off.")?;
    let ips = store.ips(&account).map_err(|error| error.to_string())?;
    ctx.reply(format!("{name} ({account}): {} address(es)", ips.len()));
    for (ip, first, last) in ips {
        ctx.reply(format!("{ip}  {} .. {}", date_text(first), date_text(last)));
    }
    Ok(())
}

fn alts(state: &mut AdminState, _: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let pattern = args.first().ok_or("Usage: alts <target|account>")?;
    let (account, name) = resolve_account(state, ctx, pattern)?;
    let store = state.store.as_ref().map(Db::lock).ok_or("The player history is off.")?;
    let alts = store.alts(&account).map_err(|error| error.to_string())?;
    if alts.is_empty() {
        ctx.reply(format!("No other account shares an address with {name}."));
    }
    for (other, other_name, ip) in alts {
        ctx.reply(format!("{other_name} ({other}) via {ip}"));
    }
    Ok(())
}

// ---- Panel users (server console only) ----

fn panel_add_user(state: &mut AdminState, _: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let name = args.first().ok_or("Usage: panel_adduser <name> [flags]")?;
    if name.is_empty() || name.len() > 32 || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b)) {
        return Err("A panel name is 1-32 letters, digits, _ - or .".into());
    }
    let flags = Flags::parse(args.get(1).map_or("z", String::as_str));
    if !flags.any() {
        return Err("No known flag letters given.".into());
    }
    let store = state.store.as_mut().map(Db::get).ok_or("The panel needs the player history (history 1).")?;
    if store.panel_user(name).map_err(|e| e.to_string())?.is_some() {
        return Err(format!("{name} exists: panel_passwd {name} gives a new password."));
    }
    let password = crate::panel::new_password();
    let (salt, hash) = crate::panel::hash_new(&password);
    store
        .set_panel_user(name, &salt, &hash, &flags.letters(), now())
        .map_err(|error| error.to_string())?;
    audit(state, ctx, "panel_adduser", Some(name), Some(&flags.letters()));
    print_secret(&format!("panel login {name}, password {password}"));
    ctx.reply(format!("Panel login {name} added with flags {}; its password is printed in this window only.", flags.letters()));
    Ok(())
}

fn panel_new_password(state: &mut AdminState, _: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let name = args.first().ok_or("Usage: panel_passwd <name>")?;
    let store = state.store.as_mut().map(Db::get).ok_or("The panel needs the player history (history 1).")?;
    let user = store
        .panel_user(name)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("No panel login {name}."))?;
    let password = crate::panel::new_password();
    let (salt, hash) = crate::panel::hash_new(&password);
    store
        .set_panel_user(name, &salt, &hash, &user.flags, now())
        .map_err(|error| error.to_string())?;
    if let Some(panel) = state.panel.as_ref() {
        panel.end_sessions(name);
    }
    audit(state, ctx, "panel_passwd", Some(name), None);
    print_secret(&format!("panel login {name}, new password {password}"));
    ctx.reply(format!("New password for {name} printed in this window only."));
    Ok(())
}

fn panel_remove_user(state: &mut AdminState, _: &mut Server, ctx: &mut Ctx, args: &[String]) -> Result<(), String> {
    let name = args.first().ok_or("Usage: panel_deluser <name>")?;
    let store = state.store.as_mut().map(Db::get).ok_or("The panel needs the player history (history 1).")?;
    if !store.remove_panel_user(name).map_err(|error| error.to_string())? {
        return Err(format!("No panel login {name}."));
    }
    if let Some(panel) = state.panel.as_ref() {
        panel.end_sessions(name);
    }
    audit(state, ctx, "panel_deluser", Some(name), None);
    ctx.reply(format!("Panel login {name} removed."));
    Ok(())
}

fn panel_users(state: &mut AdminState, _: &mut Server, ctx: &mut Ctx, _: &[String]) -> Result<(), String> {
    let store = state.store.as_ref().map(Db::lock).ok_or("The panel needs the player history (history 1).")?;
    let users = store.panel_users().map_err(|error| error.to_string())?;
    if users.is_empty() {
        ctx.reply("No panel logins: panel_adduser <name> adds one.");
    }
    for user in users {
        ctx.reply(format!(
            "{} [{}] last login {}",
            user.name,
            user.flags,
            user.last_login.map_or_else(|| "never".into(), date_text)
        ));
    }
    if let Some(panel) = state.panel.as_ref() {
        ctx.reply(format!("Panel: http://{}", panel.address));
    }
    Ok(())
}

/// A password goes to the server's own window only: not the log file, not the panel's console.
fn print_secret(line: &str) {
    println!("\n  *** {line} ***\n");
}
