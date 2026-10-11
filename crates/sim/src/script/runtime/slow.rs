//! `IW4L_GSC_SPIKE_MS=<ms>`: a scheduler pass longer than that logs where it went — the script
//! threads it resumed (by the function each was resumed in), the builtins they called, and the
//! scheduler's own phases. Off, every hook is one `OnceLock` read.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

const TOP: usize = 8;

fn threshold_ns() -> Option<u64> {
    static SPIKE: OnceLock<Option<u64>> = OnceLock::new();
    *SPIKE.get_or_init(|| {
        std::env::var("IW4L_GSC_SPIKE_MS")
            .ok()
            .and_then(|ms| ms.trim().parse::<f64>().ok())
            .map(|ms| (ms * 1e6) as u64)
    })
}

/// Total ns, calls, and the name, keyed by function or builtin index.
type Rows = HashMap<usize, (u64, u32, String)>;

#[derive(Default)]
struct Pass {
    threads: Rows,
    builtins: Rows,
    phases: Vec<(&'static str, u64)>,
}

thread_local! {
    // The authority steps on the main thread or the overlap worker, never on both at once.
    static PASS: RefCell<Option<Pass>> = const { RefCell::new(None) };
}

/// Opens a scheduler pass; `None` when profiling is off.
pub(super) fn begin() -> Option<Instant> {
    threshold_ns()?;
    PASS.with(|pass| *pass.borrow_mut() = Some(Pass::default()));
    Some(Instant::now())
}

/// A start time to hand to one of the `*_ran` calls, when profiling is on.
pub(super) fn clock() -> Option<Instant> {
    threshold_ns().map(|_| Instant::now())
}

pub(super) fn thread_ran(started: Option<Instant>, function: usize, name: impl FnOnce() -> String) {
    record(started, function, name, |pass| &mut pass.threads);
}

pub(super) fn builtin_ran(started: Option<Instant>, index: usize, name: &str) {
    record(
        started,
        index,
        || name.to_owned(),
        |pass| &mut pass.builtins,
    );
}

pub(super) fn phase_ran(started: Option<Instant>, phase: &'static str) {
    let Some(started) = started else {
        return;
    };
    let ns = started.elapsed().as_nanos() as u64;
    PASS.with(|pass| {
        if let Some(pass) = pass.borrow_mut().as_mut() {
            pass.phases.push((phase, ns));
        }
    });
}

fn record(
    started: Option<Instant>,
    key: usize,
    name: impl FnOnce() -> String,
    lane: impl FnOnce(&mut Pass) -> &mut Rows,
) {
    let Some(started) = started else {
        return;
    };
    let ns = started.elapsed().as_nanos() as u64;
    PASS.with(|pass| {
        if let Some(pass) = pass.borrow_mut().as_mut() {
            let row = lane(pass).entry(key).or_insert_with(|| (0, 0, name()));
            row.0 += ns;
            row.1 += 1;
        }
    });
}

/// Closes the pass `begin` opened and logs it when it ran over the threshold.
pub(super) fn end(started: Option<Instant>, tick: u32) {
    let Some(started) = started else {
        return;
    };
    let Some(pass) = PASS.with(|pass| pass.borrow_mut().take()) else {
        return;
    };
    let ns = started.elapsed().as_nanos() as u64;
    if threshold_ns().is_none_or(|limit| ns < limit) {
        return;
    }
    let ms = |ns: u64| ns as f64 / 1e6;
    let top = |rows: Rows| {
        let mut rows: Vec<_> = rows.into_values().collect();
        rows.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        rows.truncate(TOP);
        rows.iter()
            .map(|(ns, n, name)| format!("{name} {:.1}ms x{n}", ms(*ns)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let phases = pass
        .phases
        .iter()
        .map(|(phase, ns)| format!("{phase} {:.1}ms", ms(*ns)))
        .collect::<Vec<_>>()
        .join(", ");
    diag::info!(
        Sim,
        "gsc spike: tick {tick} scheduler {:.1} ms | phases: {phases} | threads: {} | builtins: {}",
        ms(ns),
        top(pass.threads),
        top(pass.builtins)
    );
}
