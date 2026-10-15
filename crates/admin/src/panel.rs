//! The web panel: a page and a small JSON API on its own thread, at `panel_address` (this PC
//! only by default). Logins are the panel's users (`panel_adduser` in the server console);
//! passwords are kept as PBKDF2 hashes, a session is a random cookie, repeated wrong passwords
//! lock the name and the address for a while. Actions go to the game as admin command lines
//! run with the user's flags; history reads use the panel's own connection to `players.db`.
//! Addresses are shown to root (z) users only.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use ring::rand::SecureRandom;
use serde_json::{Value, json};

use crate::config::AdminConfig;
use crate::flags::{self, Flags};
use crate::store::{Store, now};
use crate::{AdminState, Server, date_text};

const PAGE: &str = include_str!("panel.html");
const COOKIE: &str = "iw4panel";
const PBKDF2_ROUNDS: u32 = 300_000;
const SESSION_IDLE: Duration = Duration::from_secs(12 * 3600);
const LOCK_AFTER: u32 = 5;
const LOCK_FOR: Duration = Duration::from_secs(10 * 60);

/// A command line from a panel user, for the game to run.
pub(crate) struct Request {
    pub(crate) user: String,
    pub(crate) flags: String,
    pub(crate) line: String,
    pub(crate) reply: tokio::sync::oneshot::Sender<Vec<String>>,
}

#[derive(Clone, Debug)]
struct Session {
    user: String,
    flags: String,
    last: Instant,
}

/// The game's side of the panel.
pub(crate) struct Bridge {
    pub(crate) address: String,
    requests: Mutex<std::sync::mpsc::Receiver<Request>>,
    snapshot: Arc<Mutex<Value>>,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    published: Mutex<Option<Instant>>,
}

impl Bridge {
    pub(crate) fn take_requests(&self) -> Vec<Request> {
        self.requests
            .lock()
            .map(|requests| requests.try_iter().take(32).collect())
            .unwrap_or_default()
    }

    /// Signs `user` out everywhere (password changed, login removed).
    pub(crate) fn end_sessions(&self, user: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.retain(|_, session| session.user != user);
        }
    }
}

pub(crate) fn host_name() -> String {
    std::env::var("IW4L_MASTER_HOST_NAME").unwrap_or_else(|_| "Server".into())
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .expect("the system's random source works");
    bytes
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A new password: 16 letters and digits that are hard to mix up.
pub(crate) fn new_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyzACDEFGHJKLMNPQRSTUVWXYZ23456789";
    random_bytes::<16>()
        .iter()
        .map(|byte| ALPHABET[*byte as usize % ALPHABET.len()] as char)
        .collect()
}

fn rounds() -> NonZeroU32 {
    NonZeroU32::new(PBKDF2_ROUNDS).expect("rounds are not zero")
}

/// A fresh salt and the password's hash under it.
pub(crate) fn hash_new(password: &str) -> (Vec<u8>, Vec<u8>) {
    let salt = random_bytes::<16>().to_vec();
    let mut hash = vec![0u8; 32];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        rounds(),
        &salt,
        password.as_bytes(),
        &mut hash,
    );
    (salt, hash)
}

fn verify(password: &str, salt: &[u8], hash: &[u8]) -> bool {
    ring::pbkdf2::verify(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        rounds(),
        salt,
        password.as_bytes(),
        hash,
    )
    .is_ok()
}

/// Starts the panel's thread; None when it is off or its address cannot be bound.
pub(crate) fn start(config: &AdminConfig, dir: &Path) -> Option<Bridge> {
    let address: SocketAddr = config.panel_address.parse().ok()?;
    let store = match Store::open(&dir.join("players.db")) {
        Ok(store) => Some(Mutex::new(store)),
        Err(error) => {
            diag::warn!(Console, "admin panel: players.db: {error}");
            None
        }
    };
    let (sender, receiver) = std::sync::mpsc::sync_channel(64);
    let snapshot = Arc::new(Mutex::new(json!({})));
    let sessions = Arc::new(Mutex::new(HashMap::new()));
    let shared = Arc::new(Shared {
        snapshot: snapshot.clone(),
        sessions: sessions.clone(),
        failures: Mutex::new(HashMap::new()),
        store,
        requests: sender,
    });
    let listener = match std::net::TcpListener::bind(address) {
        Ok(listener) => listener,
        Err(error) => {
            diag::warn!(Console, "admin panel: cannot listen on {address}: {error}");
            return None;
        }
    };
    if listener.set_nonblocking(true).is_err() {
        return None;
    }
    let spawned = std::thread::Builder::new()
        .name("admin-panel".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    diag::warn!(Console, "admin panel: {error}");
                    return;
                }
            };
            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::from_std(listener) {
                    Ok(listener) => listener,
                    Err(error) => {
                        diag::warn!(Console, "admin panel: {error}");
                        return;
                    }
                };
                let app = router(shared);
                if let Err(error) = axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await
                {
                    diag::warn!(Console, "admin panel stopped: {error}");
                }
            });
        });
    if let Err(error) = spawned {
        diag::warn!(Console, "admin panel: {error}");
        return None;
    }
    diag::info!(Console, "admin panel: http://{address}");
    println!("admin panel: http://{address}  (logins: panel_adduser <name> in this console)");
    Some(Bridge {
        address: address.to_string(),
        requests: Mutex::new(receiver),
        snapshot,
        sessions,
        published: Mutex::new(None),
    })
}

/// What the panel shows of the running server, refreshed once a second.
pub(crate) fn publish(state: &AdminState, server: &Server) {
    let Some(bridge) = state.panel.as_ref() else {
        return;
    };
    {
        let Ok(mut published) = bridge.published.lock() else {
            return;
        };
        if published.is_some_and(|at| at.elapsed() < Duration::from_secs(1)) {
            return;
        }
        *published = Some(Instant::now());
    }
    let players: Vec<Value> = state
        .online
        .values()
        .filter(|online| online.recorded && !online.closed)
        .map(|online| {
            json!({
                "slot": online.client.0,
                "name": online.name,
                "account": online.account_hex(),
                "ip": online.ip,
                "bot": online.bot,
                "ping": server.ping(online.client),
                "team": online.team,
                "alive": online.alive,
                "kills": online.kills,
                "deaths": online.deaths,
                "score": online.score,
                "admin": online.access.map(|access| access.flags.letters()),
                "joined": online.joined_at,
            })
        })
        .collect();
    let bans = server
        .bans
        .as_ref()
        .map_or(0, |bans| bans.accounts.len() + bans.addresses.len());
    let vote = state.vote.as_ref().map(|vote| match &vote.kind {
        crate::VoteKind::Kick { name, .. } => format!("kick {name}"),
        crate::VoteKind::Map(map) => format!("map {map}"),
    });
    let value = json!({
        "host": host_name(),
        "map": state.map,
        "gametype": std::env::var("IW4L_GAMETYPE").unwrap_or_default(),
        "max": std::env::var("IW4L_MASTER_MAX_PLAYERS").ok().and_then(|max| max.parse::<u32>().ok()),
        "started": state.started_at,
        "uptime": state.started.elapsed().as_secs(),
        "players": players,
        "bans": bans,
        "vote": vote,
        "history": state.store.is_some(),
        "admins": state.admins.len(),
    });
    if let Ok(mut snapshot) = bridge.snapshot.lock() {
        *snapshot = value;
    }
}

// ---- The web side ----

struct Shared {
    snapshot: Arc<Mutex<Value>>,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    /// Wrong passwords by name and by address: how many, the latest.
    failures: Mutex<HashMap<String, (u32, Instant)>>,
    store: Option<Mutex<Store>>,
    requests: std::sync::mpsc::SyncSender<Request>,
}

type AppState = State<Arc<Shared>>;

fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/me", get(me))
        .route("/api/status", get(status))
        .route("/api/players", get(players))
        .route("/api/player/{account}", get(player))
        .route("/api/player/{account}/note", post(add_note))
        .route("/api/console", get(console_feed))
        .route("/api/command", post(command))
        .route("/api/bans", get(bans))
        .route("/api/audit", get(audit))
        .with_state(shared)
}

fn json_response(status: StatusCode, value: Value) -> Response {
    let mut response = (status, value.to_string()).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn error(status: StatusCode, message: &str) -> Response {
    json_response(status, json!({ "error": message }))
}

async fn page() -> Response {
    let mut response = PAGE.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self' data:; frame-ancestors 'none'",
        ),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == COOKIE).then(|| value.to_owned())
        })
}

/// The signed-in user, if any (their session's idle clock restarts).
fn session(shared: &Shared, headers: &HeaderMap) -> Option<Session> {
    let token = cookie_token(headers)?;
    let mut sessions = shared.sessions.lock().ok()?;
    sessions.retain(|_, session| session.last.elapsed() < SESSION_IDLE);
    let session = sessions.get_mut(&token)?;
    session.last = Instant::now();
    Some(session.clone())
}

/// A signed-in user for a read; for a change, also the panel's own header (another site's
/// page cannot send it).
fn require(shared: &Shared, headers: &HeaderMap, change: bool) -> Result<Session, Response> {
    if change && headers.get("x-panel").is_none() {
        return Err(error(StatusCode::FORBIDDEN, "missing panel header"));
    }
    session(shared, headers).ok_or_else(|| error(StatusCode::UNAUTHORIZED, "sign in"))
}

fn has(session: &Session, flag: char) -> bool {
    Flags::parse(&session.flags).has(flag)
}

fn is_root(session: &Session) -> bool {
    Flags::parse(&session.flags).is_root()
}

fn query_param(uri: &Uri, name: &str) -> Option<String> {
    uri.query()?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == name).then(|| url_decode(value))
    })
}

fn url_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let digits = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(digits, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn locked(shared: &Shared, keys: &[String]) -> Option<Duration> {
    let failures = shared.failures.lock().ok()?;
    keys.iter()
        .filter_map(|key| failures.get(key))
        .filter(|(count, at)| *count >= LOCK_AFTER && at.elapsed() < LOCK_FOR)
        .map(|(_, at)| LOCK_FOR.saturating_sub(at.elapsed()))
        .max()
}

async fn login(
    State(shared): AppState,
    ConnectInfo(from): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if headers.get("x-panel").is_none() {
        return error(StatusCode::FORBIDDEN, "missing panel header");
    }
    let Ok(body) = serde_json::from_slice::<Value>(&body) else {
        return error(StatusCode::BAD_REQUEST, "bad request");
    };
    let name = body["name"].as_str().unwrap_or("").trim().to_owned();
    let password = body["password"].as_str().unwrap_or("").to_owned();
    let keys = [format!("ip:{}", from.ip()), format!("name:{}", name.to_ascii_lowercase())];
    if let Some(wait) = locked(&shared, &keys) {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            &format!("Too many wrong passwords: try again in {} min.", wait.as_secs() / 60 + 1),
        );
    }
    let Some(store) = shared.store.as_ref() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "The panel needs the player history (history 1 in admin.cfg).");
    };
    let user = store.lock().ok().and_then(|store| store.panel_user(&name).ok().flatten());
    let no_users = store
        .lock()
        .ok()
        .and_then(|store| store.panel_users().ok())
        .is_some_and(|users| users.is_empty());
    if no_users {
        return error(
            StatusCode::UNAUTHORIZED,
            "No panel logins yet: type panel_adduser <name> in the server console.",
        );
    }
    let ok = match user.as_ref() {
        Some(user) => verify(&password, &user.salt, &user.hash),
        None => {
            // The same work for a name that does not exist.
            let _ = verify(&password, &[0; 16], &[0; 32]);
            false
        }
    };
    let Some(user) = user.filter(|_| ok) else {
        if let Ok(mut failures) = shared.failures.lock() {
            for key in &keys {
                let entry = failures.entry(key.clone()).or_insert((0, Instant::now()));
                if entry.1.elapsed() >= LOCK_FOR {
                    entry.0 = 0;
                }
                entry.0 += 1;
                entry.1 = Instant::now();
            }
        }
        diag::warn!(Console, "admin panel: wrong password for `{name}`");
        return error(StatusCode::UNAUTHORIZED, "Wrong name or password.");
    };
    if let Ok(mut failures) = shared.failures.lock() {
        for key in &keys {
            failures.remove(key);
        }
    }
    let token = hex(&random_bytes::<32>());
    if let Ok(mut sessions) = shared.sessions.lock() {
        sessions.insert(
            token.clone(),
            Session {
                user: user.name.clone(),
                flags: user.flags.clone(),
                last: Instant::now(),
            },
        );
    }
    if let Ok(mut store) = store.lock() {
        let at = now();
        let _ = store.panel_login(&user.name, at);
        let _ = store.audit(at, &format!("{} (panel)", user.name), "login", None, Some(&from.ip().to_string()));
    }
    diag::info!(Console, "admin panel: {} signed in", user.name);
    let mut response = json_response(StatusCode::OK, json!({ "user": user.name, "flags": user.flags }));
    if let Ok(cookie) = HeaderValue::from_str(&format!(
        "{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=43200"
    )) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

async fn logout(State(shared): AppState, headers: HeaderMap) -> Response {
    if let Some(token) = cookie_token(&headers)
        && let Ok(mut sessions) = shared.sessions.lock()
    {
        sessions.remove(&token);
    }
    let mut response = json_response(StatusCode::OK, json!({}));
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("iw4panel=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"),
    );
    response
}

async fn me(State(shared): AppState, headers: HeaderMap) -> Response {
    match require(&shared, &headers, false) {
        Ok(session) => json_response(
            StatusCode::OK,
            json!({ "user": session.user, "flags": session.flags, "root": is_root(&session) }),
        ),
        Err(response) => response,
    }
}

async fn status(State(shared): AppState, headers: HeaderMap) -> Response {
    let session = match require(&shared, &headers, false) {
        Ok(session) => session,
        Err(response) => return response,
    };
    let mut value = shared
        .snapshot
        .lock()
        .map(|snapshot| snapshot.clone())
        .unwrap_or_default();
    if !is_root(&session)
        && let Some(players) = value["players"].as_array_mut()
    {
        for player in players {
            player["ip"] = Value::Null;
        }
    }
    json_response(StatusCode::OK, value)
}

fn online_accounts(shared: &Shared) -> HashMap<String, u64> {
    shared
        .snapshot
        .lock()
        .ok()
        .and_then(|snapshot| {
            snapshot["players"].as_array().map(|players| {
                players
                    .iter()
                    .filter_map(|p| Some((p["account"].as_str()?.to_owned(), p["slot"].as_u64()?)))
                    .collect()
            })
        })
        .unwrap_or_default()
}

fn store_error(failure: impl std::fmt::Display) -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, &failure.to_string())
}

async fn players(State(shared): AppState, headers: HeaderMap, uri: Uri) -> Response {
    let session = match require(&shared, &headers, false) {
        Ok(session) => session,
        Err(response) => return response,
    };
    let Some(store) = shared.store.as_ref() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "The player history is off.");
    };
    let query = query_param(&uri, "q").unwrap_or_default();
    let root = is_root(&session);
    let found = match store.lock() {
        Ok(store) => store.find_players(query.trim(), root, 100),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "busy"),
    };
    let found = match found {
        Ok(found) => found,
        Err(error) => return store_error(error),
    };
    let online = online_accounts(&shared);
    let rows: Vec<Value> = found
        .into_iter()
        .map(|player| {
            json!({
                "account": player.account,
                "name": player.name,
                "ip": if root { player.ip } else { None },
                "first_seen": player.first_seen,
                "last_seen": player.last_seen,
                "sessions": player.sessions,
                "playtime": player.playtime,
                "online": online.get(&player.account),
            })
        })
        .collect();
    json_response(StatusCode::OK, json!({ "players": rows }))
}

fn ban_json(ban: &crate::store::Ban, root: bool, at: i64) -> Value {
    let state = if ban.removed.is_some() {
        "lifted"
    } else if ban.expires.is_some_and(|expires| expires <= at) {
        "expired"
    } else {
        "active"
    };
    json!({
        "id": ban.id,
        "account": ban.account,
        "ip": if root { ban.ip.clone() } else { ban.ip.as_ref().map(|_| "(address)".to_owned()) },
        "name": ban.name,
        "reason": ban.reason,
        "admin": ban.admin,
        "created": ban.created,
        "expires": ban.expires,
        "removed": ban.removed,
        "removed_by": ban.removed_by,
        "state": state,
    })
}

async fn player(State(shared): AppState, headers: HeaderMap, UrlPath(account): UrlPath<String>) -> Response {
    let session = match require(&shared, &headers, false) {
        Ok(session) => session,
        Err(response) => return response,
    };
    let Some(store) = shared.store.as_ref() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "The player history is off.");
    };
    let root = is_root(&session);
    let Ok(store) = store.lock() else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "busy");
    };
    let result = (|| -> crate::store::Result<Option<Value>> {
        let Some(player) = store.player(&account)? else {
            return Ok(None);
        };
        let at = now();
        let seen = |rows: Vec<crate::store::Seen>| -> Vec<Value> {
            rows.into_iter()
                .map(|(value, first, last)| json!({ "value": value, "first": first, "last": last }))
                .collect()
        };
        let sessions: Vec<Value> = store
            .sessions(&account, 50)?
            .into_iter()
            .map(|session| {
                json!({
                    "name": session.name,
                    "ip": if root { session.ip } else { None },
                    "map": session.map,
                    "joined": session.joined_at,
                    "left": session.left_at,
                    "reason": session.reason,
                })
            })
            .collect();
        let notes: Vec<Value> = store
            .notes(&account)?
            .into_iter()
            .map(|note| json!({ "author": note.author, "text": note.text, "created": note.created }))
            .collect();
        let bans: Vec<Value> = store
            .bans_of(&account)?
            .iter()
            .map(|ban| ban_json(ban, root, at))
            .collect();
        let alts: Vec<Value> = if root {
            store
                .alts(&account)?
                .into_iter()
                .map(|(account, name, ip)| json!({ "account": account, "name": name, "ip": ip }))
                .collect()
        } else {
            Vec::new()
        };
        Ok(Some(json!({
            "account": player.account,
            "name": player.name,
            "ip": if root { player.ip } else { None },
            "first_seen": player.first_seen,
            "last_seen": player.last_seen,
            "sessions_count": player.sessions,
            "playtime": player.playtime,
            "names": seen(store.names(&account)?),
            "ips": if root { seen(store.ips(&account)?) } else { Vec::new() },
            "sessions": sessions,
            "notes": notes,
            "bans": bans,
            "alts": alts,
        })))
    })();
    drop(store);
    match result {
        Ok(Some(mut value)) => {
            value["online"] = json!(online_accounts(&shared).get(&account));
            json_response(StatusCode::OK, value)
        }
        Ok(None) => error(StatusCode::NOT_FOUND, "No such player."),
        Err(error) => store_error(error),
    }
}

async fn add_note(
    State(shared): AppState,
    headers: HeaderMap,
    UrlPath(account): UrlPath<String>,
    body: Bytes,
) -> Response {
    let session = match require(&shared, &headers, true) {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !has(&session, flags::GENERIC) {
        return error(StatusCode::FORBIDDEN, "Notes need the generic flag (b).");
    }
    let text = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|body| body["text"].as_str().map(|text| text.trim().to_owned()))
        .unwrap_or_default();
    if text.is_empty() || text.len() > 2000 {
        return error(StatusCode::BAD_REQUEST, "A note is 1-2000 characters.");
    }
    let Some(store) = shared.store.as_ref() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "The player history is off.");
    };
    let Ok(mut store) = store.lock() else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "busy");
    };
    let at = now();
    let author = format!("{} (panel)", session.user);
    if let Err(error) = store
        .add_note(&account, &session.user, &text, at)
        .and_then(|()| store.audit(at, &author, "note", Some(&account), Some(&text)))
    {
        return store_error(error);
    }
    json_response(StatusCode::OK, json!({}))
}

async fn console_feed(State(shared): AppState, headers: HeaderMap, uri: Uri) -> Response {
    let session = match require(&shared, &headers, false) {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !has(&session, flags::GENERIC) {
        return error(StatusCode::FORBIDDEN, "The console needs the generic flag (b).");
    }
    let since = query_param(&uri, "since")
        .and_then(|since| since.parse::<u64>().ok())
        .unwrap_or(0);
    let (lines, next) = diag::log_tap_since(since);
    let lines: Vec<Value> = lines
        .into_iter()
        .map(|(number, at, text)| json!([number, at, text]))
        .collect();
    json_response(StatusCode::OK, json!({ "lines": lines, "next": next }))
}

async fn command(State(shared): AppState, headers: HeaderMap, body: Bytes) -> Response {
    let session = match require(&shared, &headers, true) {
        Ok(session) => session,
        Err(response) => return response,
    };
    let line = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|body| body["line"].as_str().map(|line| line.trim().to_owned()))
        .unwrap_or_default();
    if line.is_empty() || line.len() > 512 || line.contains(['\n', '\r']) {
        return error(StatusCode::BAD_REQUEST, "One line, 512 characters at most.");
    }
    let (reply, replied) = tokio::sync::oneshot::channel();
    let request = Request {
        user: session.user,
        flags: session.flags,
        line,
        reply,
    };
    if shared.requests.try_send(request).is_err() {
        return error(StatusCode::SERVICE_UNAVAILABLE, "The server is busy.");
    }
    match tokio::time::timeout(Duration::from_secs(5), replied).await {
        Ok(Ok(lines)) => json_response(StatusCode::OK, json!({ "lines": lines })),
        _ => error(StatusCode::GATEWAY_TIMEOUT, "The server did not answer."),
    }
}

async fn bans(State(shared): AppState, headers: HeaderMap) -> Response {
    let session = match require(&shared, &headers, false) {
        Ok(session) => session,
        Err(response) => return response,
    };
    let Some(store) = shared.store.as_ref() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "The player history is off.");
    };
    let root = is_root(&session);
    let at = now();
    let bans = match store.lock() {
        Ok(store) => store.bans(300),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "busy"),
    };
    match bans {
        Ok(bans) => json_response(
            StatusCode::OK,
            json!({ "bans": bans.iter().map(|ban| ban_json(ban, root, at)).collect::<Vec<_>>() }),
        ),
        Err(error) => store_error(error),
    }
}

async fn audit(State(shared): AppState, headers: HeaderMap) -> Response {
    let session = match require(&shared, &headers, false) {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !has(&session, flags::GENERIC) {
        return error(StatusCode::FORBIDDEN, "The audit log needs the generic flag (b).");
    }
    let Some(store) = shared.store.as_ref() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "The player history is off.");
    };
    let rows = match store.lock() {
        Ok(store) => store.audit_log(300),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "busy"),
    };
    let root = is_root(&session);
    match rows {
        Ok(rows) => json_response(
            StatusCode::OK,
            json!({ "audit": rows.into_iter().map(|row| {
                // A sign-in's detail is the address it came from.
                let detail = if row.action == "login" && !root { None } else { row.detail };
                json!({
                    "at": row.at,
                    "when": date_text(row.at),
                    "admin": row.admin,
                    "action": row.action,
                    "target": row.target,
                    "detail": detail,
                })
            }).collect::<Vec<_>>() }),
        ),
        Err(error) => store_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_verify_against_their_own_hash_only() {
        let (salt, hash) = hash_new("correct horse");
        assert!(verify("correct horse", &salt, &hash));
        assert!(!verify("correct horsf", &salt, &hash));
        assert_eq!(new_password().len(), 16);
        assert_ne!(new_password(), new_password());
    }

    #[test]
    fn query_values_decode() {
        let uri: Uri = "/api/players?q=Soap%20Mac+Tavish&since=4".parse().unwrap();
        assert_eq!(query_param(&uri, "q").as_deref(), Some("Soap Mac Tavish"));
        assert_eq!(query_param(&uri, "since").as_deref(), Some("4"));
        assert_eq!(query_param(&uri, "none"), None);
        assert_eq!(url_decode("100%"), "100%");
    }
}
