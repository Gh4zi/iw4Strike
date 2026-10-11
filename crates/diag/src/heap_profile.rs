//! `IW4L_HEAP_PROFILE=<seconds>[,<seconds>...]` (Windows x86-64): every live heap allocation of
//! `BIG` bytes or more is kept with the stack that made it, and at each of those times after
//! start the live ones are written to the log by stack (`heap site:` lines, frames as
//! module+offset, with a `stall module:` line per module for the offline symbolizer) and by
//! frame (`heap frame:`, inclusive bytes). Smaller allocations are only totalled.
//!
//! Off, the allocator pays one relaxed load per call.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW,
};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};
use windows_sys::Win32::System::ProcessStatus::{K32GetModuleInformation, MODULEINFO};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// Allocations this big or bigger are kept one by one.
const BIG: usize = 64 * 1024;
const CAPACITY: usize = 1 << 17;
const FRAMES: usize = 24;
/// How many sites and frames each dump lists.
const LISTED: usize = 40;

#[repr(C)]
#[derive(Clone, Copy)]
struct Entry {
    ptr: usize,
    size: usize,
    frames: [usize; FRAMES],
}

/// 0 unknown, 1 off, 2 on, 3 being switched on.
static STATE: AtomicU8 = AtomicU8::new(0);
static TABLE: AtomicUsize = AtomicUsize::new(0);
static LOCK: AtomicBool = AtomicBool::new(false);
static USED: AtomicUsize = AtomicUsize::new(0);
static DROPPED: AtomicUsize = AtomicUsize::new(0);
static SMALL_LIVE: AtomicI64 = AtomicI64::new(0);

thread_local! {
    /// Set while this thread is in the profiler: its own allocations are not followed.
    static INSIDE: Cell<bool> = const { Cell::new(false) };
}

fn enabled() -> bool {
    match STATE.load(Ordering::Relaxed) {
        2 => true,
        0 => switch_on(),
        _ => false,
    }
}

fn switch_on() -> bool {
    if STATE
        .compare_exchange(0, 3, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return STATE.load(Ordering::Relaxed) == 2;
    }
    if !super::alloc_count::env_key_present(b"IW4L_HEAP_PROFILE\0") {
        STATE.store(1, Ordering::Release);
        return false;
    }
    // SAFETY: a fresh zeroed committed region, owned by the profiler for the process's life.
    let table = unsafe {
        VirtualAlloc(
            std::ptr::null(),
            CAPACITY * std::mem::size_of::<Entry>(),
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };
    if table.is_null() {
        STATE.store(1, Ordering::Release);
        return false;
    }
    TABLE.store(table as usize, Ordering::Release);
    STATE.store(2, Ordering::Release);
    true
}

/// Runs `f` unless this thread is already in the profiler (or being torn down).
fn outside(f: impl FnOnce()) {
    let entered = INSIDE
        .try_with(|inside| !inside.replace(true))
        .unwrap_or(false);
    if entered {
        f();
        let _ = INSIDE.try_with(|inside| inside.set(false));
    }
}

fn table() -> &'static mut [Entry] {
    // SAFETY: TABLE is set once to a CAPACITY-entry region before STATE reads 2, and every
    // access is under LOCK.
    unsafe { std::slice::from_raw_parts_mut(TABLE.load(Ordering::Acquire) as *mut Entry, CAPACITY) }
}

fn lock() {
    while LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        std::hint::spin_loop();
    }
}

fn unlock() {
    LOCK.store(false, Ordering::Release);
}

fn home(ptr: usize) -> usize {
    ((ptr as u64 >> 4).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - CAPACITY.trailing_zeros()))
        as usize
}

pub(crate) fn on_alloc(ptr: *mut u8, size: usize) {
    if ptr.is_null() || !enabled() {
        return;
    }
    if size < BIG {
        SMALL_LIVE.fetch_add(size as i64, Ordering::Relaxed);
        return;
    }
    outside(|| {
        let mut entry = Entry {
            ptr: ptr as usize,
            size,
            frames: [0; FRAMES],
        };
        // SAFETY: room for FRAMES frames; the call does not allocate.
        unsafe {
            RtlCaptureStackBackTrace(
                2,
                FRAMES as u32,
                entry.frames.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            );
        }
        lock();
        if USED.load(Ordering::Relaxed) >= CAPACITY * 3 / 4 {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        } else {
            let table = table();
            let mask = CAPACITY - 1;
            let mut i = home(entry.ptr);
            while table[i].ptr != 0 {
                i = (i + 1) & mask;
            }
            table[i] = entry;
            USED.fetch_add(1, Ordering::Relaxed);
        }
        unlock();
    });
}

pub(crate) fn on_dealloc(ptr: *mut u8, size: usize) {
    if ptr.is_null() || !enabled() {
        return;
    }
    if size < BIG {
        SMALL_LIVE.fetch_sub(size as i64, Ordering::Relaxed);
        return;
    }
    // The dump frees its own (never followed) buffers, some while it holds the lock.
    if INSIDE.try_with(Cell::get).unwrap_or(true) {
        return;
    }
    let ptr = ptr as usize;
    lock();
    let table = table();
    let mask = CAPACITY - 1;
    let mut i = home(ptr);
    while table[i].ptr != 0 && table[i].ptr != ptr {
        i = (i + 1) & mask;
    }
    if table[i].ptr == ptr {
        // Backward-shift deletion: no tombstones, so a probe still ends at the first hole.
        let mut j = i;
        loop {
            j = (j + 1) & mask;
            if table[j].ptr == 0 {
                break;
            }
            let k = home(table[j].ptr);
            let between = if i <= j {
                i < k && k <= j
            } else {
                i < k || k <= j
            };
            if !between {
                table[i] = table[j];
                i = j;
            }
        }
        table[i].ptr = 0;
        USED.fetch_sub(1, Ordering::Relaxed);
    }
    unlock();
}

/// Start the timed dumps when `IW4L_HEAP_PROFILE` is set.
pub fn start_dumps() {
    let Ok(times) = std::env::var("IW4L_HEAP_PROFILE") else {
        return;
    };
    if !enabled() {
        return;
    }
    let mut times: Vec<f64> = times
        .split(',')
        .filter_map(|t| t.trim().parse().ok())
        .collect();
    times.sort_by(f64::total_cmp);
    let started = std::time::Instant::now();
    let _ = std::thread::Builder::new()
        .name("iw4l-heap-profile".to_owned())
        .spawn(move || {
            for at in times {
                let wait = std::time::Duration::from_secs_f64(at.max(0.0));
                std::thread::sleep(wait.saturating_sub(started.elapsed()));
                outside(|| dump(at));
            }
        });
}

fn dump(at: f64) {
    let mut live: Vec<Entry> = Vec::with_capacity(USED.load(Ordering::Relaxed) + 4096);
    lock();
    live.extend(table().iter().filter(|e| e.ptr != 0).copied());
    unlock();
    let big: usize = live.iter().map(|e| e.size).sum();
    crate::info!(
        Launch,
        "heap profile at {at:.0} s: {:.0} MiB live in {} allocations of {} KiB or more, {:.0} MiB in smaller ones, {} not followed (table full)",
        big as f64 / 1048576.0,
        live.len(),
        BIG / 1024,
        SMALL_LIVE.load(Ordering::Relaxed) as f64 / 1048576.0,
        DROPPED.load(Ordering::Relaxed)
    );
    let mut modules = Modules::default();
    let mut sites: HashMap<[usize; FRAMES], (usize, usize)> = HashMap::new();
    let mut frames: HashMap<usize, usize> = HashMap::new();
    for entry in &live {
        let site = sites.entry(entry.frames).or_insert((0, 0));
        site.0 += entry.size;
        site.1 += 1;
        let mut seen = Vec::with_capacity(FRAMES);
        for &frame in entry.frames.iter().filter(|f| **f != 0) {
            if !seen.contains(&frame) {
                seen.push(frame);
                *frames.entry(frame).or_insert(0) += entry.size;
            }
        }
    }
    let mut sites: Vec<_> = sites.into_iter().collect();
    sites.sort_by_key(|(_, (bytes, _))| std::cmp::Reverse(*bytes));
    for (stack, (bytes, count)) in sites.into_iter().take(LISTED) {
        let stack = modules.describe(&stack);
        crate::info!(
            Launch,
            "heap site at {at:.0} s: {:.1} MiB in {count}: {stack}",
            bytes as f64 / 1048576.0
        );
    }
    let mut frames: Vec<_> = frames.into_iter().collect();
    frames.sort_by_key(|(_, bytes)| std::cmp::Reverse(*bytes));
    for (frame, bytes) in frames.into_iter().take(LISTED * 2) {
        let frame = modules.describe(&[frame]);
        crate::info!(
            Launch,
            "heap frame at {at:.0} s: {:.1} MiB under {frame}",
            bytes as f64 / 1048576.0
        );
    }
}

/// Modules by address, each logged once as `stall module:` (name, base, size, path) for the
/// offline symbolizer.
#[derive(Default)]
struct Modules {
    known: Vec<(usize, usize, String)>,
}

impl Modules {
    fn describe(&mut self, frames: &[usize]) -> String {
        frames
            .iter()
            .filter(|f| **f != 0)
            .map(|&pc| match self.find(pc) {
                Some((base, name)) => format!("{name}+{:#x}", pc - base),
                None => format!("{pc:#x}"),
            })
            .collect::<Vec<_>>()
            .join(" <- ")
    }

    fn find(&mut self, pc: usize) -> Option<(usize, String)> {
        if let Some((base, _, name)) = self
            .known
            .iter()
            .find(|(base, end, _)| (*base..*end).contains(&pc))
        {
            return Some((*base, name.clone()));
        }
        // SAFETY: a module handle looked up by address without a reference taken, its file name
        // and extent read into sized buffers.
        unsafe {
            let mut module: HMODULE = std::ptr::null_mut();
            if GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                pc as *const u16,
                &mut module,
            ) == 0
            {
                return None;
            }
            let mut info: MODULEINFO = std::mem::zeroed();
            K32GetModuleInformation(
                GetCurrentProcess(),
                module,
                &mut info,
                std::mem::size_of::<MODULEINFO>() as u32,
            );
            let mut text = [0u16; 520];
            let len = GetModuleFileNameW(module, text.as_mut_ptr(), 520) as usize;
            let path = String::from_utf16_lossy(&text[..len.min(520)]);
            let name = path.rsplit('\\').next().unwrap_or(&path).to_owned();
            let base = module as usize;
            crate::info!(
                Launch,
                "stall module: {name} base {base:#x} size {:#x} path {path}",
                info.SizeOfImage
            );
            self.known
                .push((base, base + info.SizeOfImage as usize, name.clone()));
            Some((base, name))
        }
    }
}
