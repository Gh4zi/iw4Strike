//! `IW4L_STALL_STACKS_MS=<ms>` (`bevy-profile` builds, Windows x86-64): when one system has run
//! that long without returning, a watchdog thread suspends the thread running it for a moment,
//! walks its stack and logs it (again at twice and four times the threshold). A spike that used
//! no CPU (`system spike: … (cpu 0 ms)`) was waiting, and its stack names what on.
//!
//! The clock is per span from its own enter, and the innermost running span is the one named;
//! each span run gives at most `SAMPLES` stacks. At the second, every other thread of the
//! process is sampled too (`stall peer:`): a thread waiting with no CPU is often waiting on one
//! of them.
//!
//! Frames are logged as `module+0xoffset`, with a `stall module:` line per module the first
//! time one appears; names come afterwards, off the log, from the binaries' symbols. Looking
//! them up in here (dbghelp mapping a 500 MB PDB) stalled the very threads being watched.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, HMODULE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    CONTEXT, CONTEXT_FULL_AMD64, GetThreadContext, RtlLookupFunctionEntry, RtlVirtualUnwind,
    UNW_FLAG_NHANDLER,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32EnumProcessModules, K32GetModuleBaseNameW, K32GetModuleFileNameExW, K32GetModuleInformation,
    K32GetProcessMemoryInfo, MODULEINFO, PROCESS_MEMORY_COUNTERS,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, GetThreadDescription, OpenThread,
    ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_LIMITED_INFORMATION,
    THREAD_SUSPEND_RESUME,
};
use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;

const SLOTS: usize = 128;
const FRAMES: usize = 64;
/// Frames kept per peer thread.
const PEER_FRAMES: usize = 24;
/// Stacks per run: at the threshold, twice it and four times it.
const SAMPLES: u32 = 3;

/// One thread's innermost running system.
struct Slot {
    tid: AtomicU32,
    /// When the innermost running span was entered, ns since `epoch()` plus one; 0 when the
    /// thread runs none.
    started: AtomicU64,
    name: AtomicU32,
    /// Which run of a span is innermost (unique per enter), so a stack taken while it is
    /// unchanged belongs to the run the watchdog timed.
    instance: AtomicU64,
}

static INSTANCES: AtomicU64 = AtomicU64::new(1);

static SLOT_TABLE: [Slot; SLOTS] = [const {
    Slot {
        tid: AtomicU32::new(0),
        started: AtomicU64::new(0),
        name: AtomicU32::new(0),
        instance: AtomicU64::new(0),
    }
}; SLOTS];
static SLOTS_USED: AtomicUsize = AtomicUsize::new(0);
static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static THREAD_NAMES: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

pub(crate) fn threshold() -> Option<Duration> {
    static THRESHOLD: OnceLock<Option<Duration>> = OnceLock::new();
    *THRESHOLD.get_or_init(|| {
        std::env::var("IW4L_STALL_STACKS_MS")
            .ok()
            .and_then(|ms| ms.trim().parse::<f64>().ok())
            .filter(|ms| *ms > 0.0)
            .map(|ms| Duration::from_secs_f64(ms / 1e3))
    })
}

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn now_ns() -> u64 {
    epoch().elapsed().as_nanos() as u64
}

/// A span name's index for `enter`; called once per span, when it is created.
pub(crate) fn intern(name: &str) -> u32 {
    if threshold().is_none() {
        return 0;
    }
    let mut names = NAMES.lock().unwrap_or_else(|poison| poison.into_inner());
    names.push(name.to_owned());
    (names.len() - 1) as u32
}

struct Local {
    slot: &'static Slot,
    /// The enclosing spans: name, enter time, instance.
    parents: Vec<(u32, u64, u64)>,
}

thread_local! {
    static LOCAL: RefCell<Option<Local>> = const { RefCell::new(None) };
}

fn with_local(f: impl FnOnce(&mut Local)) {
    LOCAL.with(|local| {
        let mut local = local.borrow_mut();
        if local.is_none() {
            let index = SLOTS_USED.fetch_add(1, Ordering::Relaxed);
            let Some(slot) = SLOT_TABLE.get(index) else {
                return;
            };
            // SAFETY: no arguments.
            let tid = unsafe { GetCurrentThreadId() };
            let name = std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_owned();
            THREAD_NAMES
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .get_or_insert_with(HashMap::new)
                .insert(tid, name);
            slot.tid.store(tid, Ordering::Release);
            *local = Some(Local {
                slot,
                parents: Vec::with_capacity(16),
            });
        }
        if let Some(local) = local.as_mut() {
            f(local);
        }
    });
}

pub(crate) fn enter(name: u32) {
    if threshold().is_none() {
        return;
    }
    with_local(|local| {
        let slot = local.slot;
        let started = slot.started.load(Ordering::Relaxed);
        if started != 0 {
            local.parents.push((
                slot.name.load(Ordering::Relaxed),
                started,
                slot.instance.load(Ordering::Relaxed),
            ));
        }
        set(
            slot,
            name,
            now_ns() + 1,
            INSTANCES.fetch_add(1, Ordering::Relaxed),
        );
    });
}

pub(crate) fn exit() {
    if threshold().is_none() {
        return;
    }
    with_local(|local| {
        let (name, started, instance) = local.parents.pop().unwrap_or((0, 0, 0));
        set(local.slot, name, started, instance);
    });
}

fn set(slot: &Slot, name: u32, started: u64, instance: u64) {
    // Instance first: the watchdog reads it last, and a stack is only kept while it holds.
    slot.instance.store(instance, Ordering::Release);
    slot.name.store(name, Ordering::Relaxed);
    slot.started.store(started, Ordering::Release);
}

/// Start the watchdog, once, when the threshold is set.
pub(crate) fn start() {
    let Some(threshold) = threshold() else {
        return;
    };
    epoch();
    let started = std::thread::Builder::new()
        .name("iw4l-stall-stacks".to_owned())
        .spawn(move || watch(threshold));
    if started.is_ok() {
        diag::info!(
            Launch,
            "stall stacks: on, systems running over {:.0} ms are sampled",
            threshold.as_secs_f64() * 1e3
        );
    }
}

fn watch(threshold: Duration) {
    let threshold_ns = threshold.as_nanos() as u64;
    let tick = (threshold / 4).clamp(Duration::from_millis(2), Duration::from_millis(20));
    // Per slot: the span run last sampled, how many stacks it has given, and the thread's
    // cycle count and the time at the last one.
    let mut sampled = [(0u64, 0u32, 0u64, 0u64); SLOTS];
    let mut modules = Modules::default();
    // The process's page-fault count each tick of the last few seconds, to say how many fell
    // inside a stalled span.
    let mut faults: std::collections::VecDeque<(u64, u32)> = std::collections::VecDeque::new();
    let mut last_wake = now_ns();
    loop {
        std::thread::sleep(tick);
        let now = now_ns();
        // This thread does nothing but sleep: a wake far later than asked means the whole
        // process (or the machine) was held up, not one system.
        let late = now
            .saturating_sub(last_wake)
            .saturating_sub(tick.as_nanos() as u64);
        last_wake = now;
        if late > 40_000_000 {
            let unix_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            diag::info!(
                Launch,
                "stall stacks: the watchdog itself woke {:.0} ms late (unix_ms {unix_ms})",
                late as f64 / 1e6
            );
        }
        faults.push_back((now, page_faults()));
        while faults.len() > 1 && faults[0].0 + 10_000_000_000 < now {
            faults.pop_front();
        }
        let used = SLOTS_USED.load(Ordering::Relaxed).min(SLOTS);
        for (index, slot) in SLOT_TABLE.iter().enumerate().take(used) {
            let started = slot.started.load(Ordering::Acquire);
            let tid = slot.tid.load(Ordering::Acquire);
            if started == 0 || tid == 0 {
                continue;
            }
            let instance = slot.instance.load(Ordering::Acquire);
            let elapsed = now_ns().saturating_sub(started - 1);
            let (last, count, last_cycles, last_at) = sampled[index];
            let count = if last == instance { count } else { 0 };
            if count >= SAMPLES || elapsed < threshold_ns << count {
                continue;
            }
            let name = slot.name.load(Ordering::Relaxed);
            let Some(sample) = capture(tid, || slot.instance.load(Ordering::Acquire) == instance)
            else {
                continue;
            };
            sampled[index] = (instance, count + 1, sample.cycles, now);
            // Since the previous stack of this run: how much of that time the thread ran.
            let ran = (count > 0).then(|| {
                let cycles = sample.cycles.saturating_sub(last_cycles) as f64;
                let wall = now.saturating_sub(last_at).max(1) as f64;
                100.0 * cycles / (wall * timestamp_per_ns())
            });
            let span_faults = faults
                .iter()
                .find(|(at, _)| *at + 1 >= started)
                .map_or(0, |(_, at_start)| {
                    faults.back().map_or(0, |b| b.1).wrapping_sub(*at_start)
                });
            let name = NAMES
                .lock()
                .ok()
                .and_then(|names| names.get(name as usize).cloned())
                .unwrap_or_else(|| "?".to_owned());
            let thread = THREAD_NAMES
                .lock()
                .ok()
                .and_then(|names| names.as_ref()?.get(&tid).cloned())
                .unwrap_or_else(|| tid.to_string());
            let stack = modules.describe(&sample.frames);
            let ran = ran.map_or_else(String::new, |ran| format!(", ran {ran:.0}% since the last"));
            // When the span began, on the wall clock, to line it up with tools outside the
            // process.
            let began_unix_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis())
                .saturating_sub(u128::from(elapsed / 1_000_000));
            // A thread found already suspended was stopped by someone else: a debugger, or an
            // injected overlay hooking the process.
            diag::info!(
                Launch,
                "stall stack: {name} @{thread} running {:.0} ms since unix_ms {began_unix_ms} (sample {}{ran}, suspended by others {}, {span_faults} process page faults in it): {stack}",
                elapsed as f64 / 1e6,
                count + 1,
                sample.suspended_before,
            );
            if count == 1 {
                log_peers(tid, &mut modules);
            }
        }
    }
}

/// Every other thread of the process, sampled once: its name, stack and whether something
/// else had it suspended. Threads with the same stack are logged once, with their count.
fn log_peers(stalled: u32, modules: &mut Modules) {
    // SAFETY: no arguments.
    let (me, process) = unsafe { (GetCurrentThreadId(), GetCurrentProcessId()) };
    let mut tids = Vec::new();
    // SAFETY: a thread snapshot walked with a sized entry, then closed.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return;
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut more = Thread32First(snapshot, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == process
                && entry.th32ThreadID != me
                && entry.th32ThreadID != stalled
            {
                tids.push(entry.th32ThreadID);
            }
            more = Thread32Next(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    let mut stacks: Vec<(String, Vec<String>)> = Vec::new();
    for tid in tids {
        let Some(sample) = capture(tid, || true) else {
            continue;
        };
        let frames = &sample.frames[..sample.frames.len().min(PEER_FRAMES)];
        let mut stack = modules.describe(frames);
        if sample.suspended_before > 0 {
            stack = format!("[suspended by others {}] {stack}", sample.suspended_before);
        }
        let name = thread_description(tid).unwrap_or_else(|| tid.to_string());
        match stacks.iter_mut().find(|(known, _)| *known == stack) {
            Some((_, names)) => names.push(name),
            None => stacks.push((stack, vec![name])),
        }
    }
    for (stack, names) in stacks {
        diag::info!(
            Launch,
            "stall peer: {} x{}: {stack}",
            names.join(","),
            names.len()
        );
    }
}

fn thread_description(tid: u32) -> Option<String> {
    // SAFETY: a query-only handle, and the description string freed with LocalFree as its
    // documentation asks.
    unsafe {
        let handle = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, tid);
        if handle.is_null() {
            return None;
        }
        let mut text = std::ptr::null_mut();
        let ok = GetThreadDescription(handle, &mut text) >= 0;
        CloseHandle(handle);
        if !ok || text.is_null() {
            return None;
        }
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        let name = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast());
        (!name.is_empty()).then_some(name)
    }
}

/// The process's loaded modules by address range, read again when an address falls outside all
/// of them; each new one is logged once (`stall module:`) for the offline symbolizer.
#[derive(Default)]
struct Modules {
    ranges: Vec<(u64, u64, String)>,
}

impl Modules {
    fn describe(&mut self, frames: &[u64]) -> String {
        if frames.iter().any(|pc| self.find(*pc).is_none()) {
            self.refresh();
        }
        frames
            .iter()
            .map(|&pc| match self.find(pc) {
                Some((base, name)) => format!("{name}+{:#x}", pc - base),
                None => format!("{pc:#x}"),
            })
            .collect::<Vec<_>>()
            .join(" <- ")
    }

    fn find(&self, pc: u64) -> Option<(u64, &str)> {
        self.ranges
            .iter()
            .find(|(base, end, _)| (*base..*end).contains(&pc))
            .map(|(base, _, name)| (*base, name.as_str()))
    }

    fn refresh(&mut self) {
        // SAFETY: module handles of this process, read into sized buffers.
        unsafe {
            let process = GetCurrentProcess();
            let mut handles: Vec<HMODULE> = vec![std::ptr::null_mut(); 1024];
            let mut needed = 0u32;
            if K32EnumProcessModules(
                process,
                handles.as_mut_ptr(),
                (handles.len() * std::mem::size_of::<HMODULE>()) as u32,
                &mut needed,
            ) == 0
            {
                return;
            }
            handles.truncate(needed as usize / std::mem::size_of::<HMODULE>());
            for module in handles {
                let mut info: MODULEINFO = std::mem::zeroed();
                if K32GetModuleInformation(
                    process,
                    module,
                    &mut info,
                    std::mem::size_of::<MODULEINFO>() as u32,
                ) == 0
                {
                    continue;
                }
                let base = info.lpBaseOfDll as u64;
                if self.ranges.iter().any(|(known, _, _)| *known == base) {
                    continue;
                }
                let mut text = [0u16; 520];
                let len = K32GetModuleBaseNameW(process, module, text.as_mut_ptr(), 520) as usize;
                let name = String::from_utf16_lossy(&text[..len.min(520)]);
                let len = K32GetModuleFileNameExW(process, module, text.as_mut_ptr(), 520) as usize;
                let path = String::from_utf16_lossy(&text[..len.min(520)]);
                diag::info!(
                    Launch,
                    "stall module: {name} base {base:#x} size {:#x} path {path}",
                    info.SizeOfImage
                );
                self.ranges
                    .push((base, base + u64::from(info.SizeOfImage), name));
            }
        }
    }
}

/// Context records have to be 16-byte aligned.
#[repr(C, align(16))]
struct AlignedContext(CONTEXT);

fn page_faults() -> u32 {
    // SAFETY: a zeroed counters struct with its size set, for the current process.
    unsafe {
        let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) == 0 {
            return 0;
        }
        counters.PageFaultCount
    }
}

/// Timestamp-counter ticks per ns (`QueryThreadCycleTime`'s unit), measured once.
fn timestamp_per_ns() -> f64 {
    static RATE: OnceLock<f64> = OnceLock::new();
    *RATE.get_or_init(|| {
        let start = Instant::now();
        // SAFETY: rdtsc has no preconditions on x86-64.
        let first = unsafe { core::arch::x86_64::_rdtsc() };
        std::thread::sleep(Duration::from_millis(20));
        let ticks = unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(first);
        (ticks as f64 / start.elapsed().as_nanos().max(1) as f64).max(1e-3)
    })
}

struct Sample {
    frames: Vec<u64>,
    /// The thread's suspend count before ours: above zero, something else had stopped it.
    suspended_before: u32,
    /// Cycles the thread has run in its life.
    cycles: u64,
}

/// Suspend thread `tid`, walk its stack while `still_same()` holds, resume it. Nothing between
/// the suspend and the resume allocates: the suspended thread may hold the heap's lock.
fn capture(tid: u32, still_same: impl Fn() -> bool) -> Option<Sample> {
    // SAFETY: plain Win32 calls on a thread handle this function opens and closes; the context
    // is read while the thread is suspended, and the stack words read lie in that thread's
    // stack, which outlives the suspension.
    unsafe {
        let handle: HANDLE = OpenThread(
            THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_LIMITED_INFORMATION,
            0,
            tid,
        );
        if handle.is_null() {
            return None;
        }
        let suspended_before = SuspendThread(handle);
        if suspended_before == u32::MAX {
            CloseHandle(handle);
            return None;
        }
        let mut cycles = 0u64;
        QueryThreadCycleTime(handle, &mut cycles);
        let mut frames = [0u64; FRAMES];
        let mut count = 0;
        let mut context: AlignedContext = std::mem::zeroed();
        context.0.ContextFlags = CONTEXT_FULL_AMD64;
        if still_same() && GetThreadContext(handle, &mut context.0) != 0 {
            let context = &mut context.0;
            while count < FRAMES {
                let pc = context.Rip;
                if pc == 0 {
                    break;
                }
                frames[count] = pc;
                count += 1;
                let mut image_base = 0u64;
                let function = RtlLookupFunctionEntry(pc, &mut image_base, std::ptr::null_mut());
                let sp = context.Rsp;
                if function.is_null() {
                    // A leaf: the return address is on top of the stack.
                    if sp == 0 || !sp.is_multiple_of(8) {
                        break;
                    }
                    context.Rip = *(sp as *const u64);
                    context.Rsp = sp + 8;
                } else {
                    let mut handler_data = std::ptr::null_mut();
                    let mut establisher = 0u64;
                    RtlVirtualUnwind(
                        UNW_FLAG_NHANDLER,
                        image_base,
                        pc,
                        function,
                        context,
                        &mut handler_data,
                        &mut establisher,
                        std::ptr::null_mut(),
                    );
                }
                if context.Rsp <= sp {
                    break;
                }
            }
        }
        let same = still_same();
        ResumeThread(handle);
        CloseHandle(handle);
        (same && count > 0).then(|| Sample {
            frames: frames[..count].to_vec(),
            suspended_before,
            cycles,
        })
    }
}
