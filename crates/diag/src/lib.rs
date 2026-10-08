mod alloc_count;
pub mod gap;
pub mod wgsl_dump;

pub use alloc_count::{
    ProcessAllocationStats, ProcessCountingAllocator, counting_enabled,
    process_allocation_saturated, process_allocation_slots_used, process_allocations,
    process_live_heap_bytes, release_freed_heap,
};

use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

static SINK: OnceLock<Mutex<DiagState>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    Launch,
    Zone,
    World,
    Fpv,
    Input,
    Sim,
    Net,
    Ui,
    Audio,
    Console,
}

impl Channel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Zone => "zone",
            Self::World => "world",
            Self::Fpv => "fpv",
            Self::Input => "input",
            Self::Sim => "sim",
            Self::Net => "net",
            Self::Ui => "ui",
            Self::Audio => "audio",
            Self::Console => "console",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl Level {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            _ => None,
        }
    }
}

enum WriterMsg {
    Line(String),
    Flush(std::sync::mpsc::SyncSender<()>),
}

/// A log file written by its own thread. Game threads only queue lines: a disk write that
/// stalls (antivirus scanning a growing log is common on Windows) never stalls a frame.
struct AsyncFile {
    tx: std::sync::mpsc::Sender<WriterMsg>,
}

impl AsyncFile {
    fn spawn(file: File, thread_name: &str) -> Option<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<WriterMsg>();
        std::thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || {
                let mut out = std::io::BufWriter::with_capacity(64 * 1024, file);
                let mut acks = Vec::new();
                while let Ok(first) = rx.recv() {
                    let mut next = Some(first);
                    while let Some(msg) = next {
                        match msg {
                            WriterMsg::Line(line) => {
                                let _ = out.write_all(line.as_bytes());
                                let _ = out.write_all(b"\n");
                            }
                            WriterMsg::Flush(ack) => acks.push(ack),
                        }
                        next = rx.try_recv().ok();
                    }
                    let _ = out.flush();
                    for ack in acks.drain(..) {
                        let _ = ack.send(());
                    }
                }
                let _ = out.flush();
            })
            .ok()?;
        Some(Self { tx })
    }

    fn line(&self, line: String) {
        let _ = self.tx.send(WriterMsg::Line(line));
    }

    /// Block until every queued line is on disk, bounded so a wedged disk cannot hang exit.
    fn flush_wait(&self) {
        let (ack, done) = std::sync::mpsc::sync_channel(1);
        if self.tx.send(WriterMsg::Flush(ack)).is_ok() {
            let _ = done.recv_timeout(std::time::Duration::from_secs(2));
        }
    }
}

struct DiagState {
    file: Option<AsyncFile>,
    file_path: PathBuf,

    latest: Option<PathBuf>,
    traces: Option<AsyncFile>,
    traces_path: Option<PathBuf>,
    stderr_threshold: Level,
    file_threshold: Level,

    last_key: Option<(Channel, Level, String)>,
    last_count: u32,

    started: Instant,
}

pub const LATEST_LOG_NAME: &str = "latest.log";

fn link_latest(path: &Path) -> Option<PathBuf> {
    let dir = path.parent()?;
    let name = path.file_name()?;
    if name == LATEST_LOG_NAME {
        return None;
    }
    let link = dir.join(LATEST_LOG_NAME);
    #[cfg(unix)]
    {
        match std::fs::symlink_metadata(&link) {
            Ok(md) if md.is_symlink() => std::fs::remove_file(&link).ok()?,
            Ok(_) => return None,
            Err(_) => {}
        }
        std::os::unix::fs::symlink(name, &link).ok()?;
        Some(link)
    }
    #[cfg(not(unix))]
    {
        // A symlink needs a privilege Windows withholds by default; a hard link does not.
        match std::fs::remove_file(&link) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
        std::fs::hard_link(path, &link).ok()?;
        Some(link)
    }
}

pub fn init_log(artifacts_root: &Path) -> PathBuf {
    let _ = START.set(Instant::now());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let path = std::env::var_os("IW4L_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|| artifacts_root.join("logs").join(format!("{stamp}.log")));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
        .and_then(|file| AsyncFile::spawn(file, "iw4l-log"));

    let (traces, traces_path) = match std::env::var_os("IW4L_TRACES_DIR") {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            let _ = std::fs::create_dir_all(&dir);
            let tp = dir.join(format!("{stamp}.jsonl"));
            let f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&tp)
                .ok()
                .and_then(|file| AsyncFile::spawn(file, "iw4l-traces"));
            (f, Some(tp))
        }
        None => (None, None),
    };

    let bump = std::env::var("IW4L_LOG_LEVEL")
        .ok()
        .and_then(|s| Level::parse(&s));
    let stderr_threshold = bump.unwrap_or(Level::Error);
    let file_threshold = bump.unwrap_or(Level::Info);

    let started = START.get().copied().unwrap_or_else(Instant::now);
    let state = DiagState {
        file,
        file_path: path.clone(),
        latest: link_latest(&path),
        traces,
        traces_path,
        stderr_threshold,
        file_threshold,
        last_key: None,
        last_count: 0,
        started,
    };

    if state.traces.is_some() {
        let zone = std::env::var("IW4L_ZONE").unwrap_or_default();
        let role = std::env::var("IW4L_ROLE").unwrap_or_else(|_| "Listen".into());
        let games = std::env::var("IW4L_GAMES").unwrap_or_default();
        let git = option_env!("IW4L_GIT_HASH").unwrap_or("unknown");
        let header = serde_json::json!({
            "t": 0,
            "ch": "launch",
            "lvl": "info",
            "msg": "session",
            "ev": "session",
            "f": {
                "format": "iw4l-traces-v1",
                "zone": zone,
                "role": role,
                "IW4L_GAMES": games,
                "git": git,
            }
        });
        if let Some(f) = state.traces.as_ref() {
            f.line(header.to_string());
        }
    }

    let _ = SINK.set(Mutex::new(state));
    path
}

pub fn latest_log_path() -> Option<PathBuf> {
    SINK.get()
        .and_then(|s| s.lock().ok())
        .and_then(|g| g.latest.clone())
}

pub fn exit_launch_error(message: &str) -> ! {
    write_event(Channel::Launch, Level::Error, message, None, None);
    if let Some(sink) = SINK.get()
        && let Ok(state) = sink.lock()
    {
        if let Some(file) = state.file.as_ref() {
            file.flush_wait();
            eprintln!("log: {}", state.file_path.display());
        } else {
            eprintln!("log unavailable: {}", state.file_path.display());
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (text, caption) = (wide(message), wide("iw4Strike"));
        // SAFETY: both strings are NUL-terminated and outlive the modal call.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                caption.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    std::process::exit(2);
}

pub fn traces_path() -> Option<PathBuf> {
    SINK.get()
        .and_then(|s| s.lock().ok())
        .and_then(|g| g.traces_path.clone())
}

fn now_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

fn flush_collapsed(state: &mut DiagState) {
    if state.last_count <= 1 {
        return;
    }
    let Some((ch, lvl, msg)) = state.last_key.clone() else {
        return;
    };
    let n = state.last_count;
    let line = format!("{}  {}  ↑ repeated {n}× — {msg}", ch.as_str(), lvl.as_str());
    emit_raw(state, ch, lvl, &line, Some(n), None, None, true);
    state.last_count = 1;
}

fn emit_raw(
    state: &mut DiagState,
    ch: Channel,
    lvl: Level,
    msg: &str,
    n: Option<u32>,
    ev: Option<&str>,
    fields: Option<&serde_json::Value>,
    collapsed_banner: bool,
) {
    let text = if collapsed_banner {
        msg.to_owned()
    } else {
        format!("{}  {}  {msg}", ch.as_str(), lvl.as_str())
    };

    if lvl <= state.stderr_threshold {
        eprintln!("{text}");
    }
    if lvl <= state.file_threshold
        && let Some(file) = state.file.as_ref()
    {
        file.line(text);
        // An error may be the last thing the process says; get it on disk before going on.
        if lvl == Level::Error {
            file.flush_wait();
        }
    }
    if let Some(file) = state.traces.as_ref() {
        let t = now_ms(state.started);
        let mut obj = serde_json::json!({
            "t": t,
            "ch": ch.as_str(),
            "lvl": lvl.as_str(),
            "msg": if collapsed_banner { msg } else { msg },
        });
        if let Some(ev) = ev {
            obj["ev"] = serde_json::Value::String(ev.to_owned());
        }
        if let Some(n) = n.filter(|&n| n > 1) {
            obj["n"] = serde_json::Value::from(n);
        }
        if let Some(f) = fields {
            obj["f"] = f.clone();
        }

        if !collapsed_banner {
            obj["msg"] = serde_json::Value::String(msg.to_owned());
        }
        file.line(obj.to_string());
    }
}

pub fn write_event(
    ch: Channel,
    lvl: Level,
    msg: &str,
    ev: Option<&str>,
    fields: Option<&serde_json::Value>,
) {
    let Some(sink) = SINK.get() else {
        eprintln!("{}  {}  {msg}", ch.as_str(), lvl.as_str());
        return;
    };
    let Ok(mut state) = sink.lock() else {
        return;
    };

    let key = (ch, lvl, msg.to_owned());
    if state.last_key.as_ref() == Some(&key) {
        state.last_count = state.last_count.saturating_add(1);

        if state.last_count.is_multiple_of(64) {
            let t = now_ms(state.started);
            let n = state.last_count;
            let obj = serde_json::json!({
                "t": t,
                "ch": ch.as_str(),
                "lvl": lvl.as_str(),
                "msg": msg,
                "n": n,
            });
            if let Some(file) = state.traces.as_ref() {
                file.line(obj.to_string());
            }
        }
        return;
    }

    flush_collapsed(&mut state);
    state.last_key = Some(key);
    state.last_count = 1;
    emit_raw(&mut state, ch, lvl, msg, None, ev, fields, false);
}

/// Write out any collapsed repeat and wait until every queued line is on disk.
pub fn flush() {
    if let Some(sink) = SINK.get()
        && let Ok(mut state) = sink.lock()
    {
        flush_collapsed(&mut state);
        for file in [state.file.as_ref(), state.traces.as_ref()]
            .into_iter()
            .flatten()
        {
            file.flush_wait();
        }
    }
}

pub fn announce_log_stdout(path: &Path, latest: Option<&Path>) {
    let line = match latest {
        Some(latest) => format!("log: {} ({})", path.display(), latest.display()),
        None => format!("log: {}", path.display()),
    };
    announce_stdout(&line);
}

pub fn process_elapsed_ns() -> u128 {
    START
        .get()
        .map(|start| start.elapsed().as_nanos())
        .unwrap_or(0)
}

pub fn lifecycle_boundary(name: &str, detail: &str) {
    controller_line("lifecycle", name, detail);
}

pub fn script_boundary(name: &str, detail: &str) {
    controller_line("gsc", name, detail);
}

fn controller_line(prefix: &str, name: &str, detail: &str) {
    let line = format!(
        "{prefix}: {name} pid={} ns={}{detail}",
        std::process::id(),
        process_elapsed_ns()
    );
    write_event(Channel::Launch, Level::Info, &line, None, None);
    announce_stdout(&line);
}

pub fn announce_stdout(line: &str) {
    let _ = writeln!(std::io::stdout(), "{line}");
    let _ = std::io::stdout().flush();
}

pub fn write_line(category: &str, message: &str) {
    let ch = match category {
        "launch" => Channel::Launch,
        "zone" => Channel::Zone,
        "world" => Channel::World,
        "fpv" => Channel::Fpv,
        "input" => Channel::Input,
        "sim" | "spawn" | "score" => Channel::Sim,
        "net" => Channel::Net,
        "ui" | "menu" | "hud" => Channel::Ui,
        "audio" => Channel::Audio,
        "console" => Channel::Console,
        _ => Channel::Launch,
    };
    let msg = if category.is_empty() {
        message.to_owned()
    } else if message.starts_with(category) {
        message.to_owned()
    } else {
        format!("{category}: {message}")
    };
    write_event(ch, Level::Info, &msg, None, None);
}

#[macro_export]
macro_rules! log_line {
    ($cat:ident, $($arg:tt)*) => {{
        $crate::write_line(stringify!($cat), &format!($($arg)*));
    }};
    ($($arg:tt)*) => {{
        $crate::write_line("", &format!($($arg)*));
    }};
}

#[macro_export]
macro_rules! error {
    ($ch:ident, $($arg:tt)*) => {{
        $crate::write_event(
            $crate::Channel::$ch,
            $crate::Level::Error,
            &format!($($arg)*),
            None,
            None,
        );
    }};
}

#[macro_export]
macro_rules! warn {
    ($ch:ident, $($arg:tt)*) => {{
        $crate::write_event(
            $crate::Channel::$ch,
            $crate::Level::Warn,
            &format!($($arg)*),
            None,
            None,
        );
    }};
}

#[macro_export]
macro_rules! info {
    ($ch:ident, $($arg:tt)*) => {{
        $crate::write_event(
            $crate::Channel::$ch,
            $crate::Level::Info,
            &format!($($arg)*),
            None,
            None,
        );
    }};
}

#[macro_export]
macro_rules! debug {
    ($ch:ident, $($arg:tt)*) => {{
        $crate::write_event(
            $crate::Channel::$ch,
            $crate::Level::Debug,
            &format!($($arg)*),
            None,
            None,
        );
    }};
}

#[macro_export]
macro_rules! event {
    ($ch:ident, $lvl:ident, $ev:literal, $($arg:tt)*) => {{
        $crate::write_event(
            $crate::Channel::$ch,
            $crate::Level::$lvl,
            &format!($($arg)*),
            Some($ev),
            None,
        );
    }};
}
