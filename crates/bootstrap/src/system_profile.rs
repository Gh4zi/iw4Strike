//! Per-system CPU time, aggregated in memory (`bevy-profile` builds only).
//!
//! Bevy's chrome trace writes an event per system run; this runtime runs thousands of systems a
//! frame, so the trace writer falls minutes behind and loses the gameplay it was meant to show.
//! This layer instead totals each system's busy time and logs the heaviest systems every
//! `REPORT_SECONDS`. Run with `RUST_LOG=warn,iw4l=info,bevy_ecs::system=info` so the system
//! spans pass the log filter.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use bevy::log::tracing::field::{Field, Visit};
use bevy::log::tracing::{Subscriber, span};
use bevy::log::tracing_subscriber::Layer;
use bevy::log::tracing_subscriber::layer::Context;
use bevy::log::tracing_subscriber::registry::LookupSpan;
use bevy::prelude::*;

const REPORT_SECONDS: f32 = 10.0;

const REPORT_ROWS: usize = 45;

/// `IW4L_PROFILE_BY_THREAD=1` keys system rows by the thread they ran on as well, so a system
/// that runs on several threads (the sim schedule: listen authority and client prediction) splits.
fn thread_suffix() -> String {
    static BY_THREAD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*BY_THREAD.get_or_init(|| perf::switch("IW4L_PROFILE_BY_THREAD")) {
        return String::new();
    }
    let thread = std::thread::current();
    format!(" @{}", thread.name().unwrap_or("unnamed"))
}

fn all_spans() -> bool {
    static ALL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ALL.get_or_init(|| perf::switch("IW4L_PROFILE_ALL_SPANS"))
}

/// `IW4L_PROFILE_ROWS=<n>` logs more (or fewer) rows than the default.
fn report_rows() -> usize {
    std::env::var("IW4L_PROFILE_ROWS")
        .ok()
        .and_then(|rows| rows.trim().parse().ok())
        .unwrap_or(REPORT_ROWS)
}

/// `IW4L_PROFILE_SPIKE_MS=<ms>` logs every single run longer than that, as it ends: which system
/// a hitch was, and when.
fn spike_ns() -> Option<u64> {
    static SPIKE: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *SPIKE.get_or_init(|| {
        std::env::var("IW4L_PROFILE_SPIKE_MS")
            .ok()
            .and_then(|ms| ms.trim().parse::<f64>().ok())
            .map(|ms| (ms * 1e6) as u64)
    })
}

/// Per span: total ns, runs, longest run ns.
static TOTALS: Mutex<Option<HashMap<String, (u64, u64, u64)>>> = Mutex::new(None);

struct SystemName(String);

struct Entered(Instant);

struct SystemProfileLayer;

impl<S> Layer<S> for SystemProfileLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        // Besides systems: a render system's command encoder is finished in its deferred apply,
        // outside the system span, and wgpu encodes every recorded pass there.
        let (field_name, prefix) = match attrs.metadata().name() {
            "system" => ("name", ""),
            "RenderContextState::apply" => ("system", "encoder finish: "),
            name if name == "queue_submit" || all_spans() => {
                // `IW4L_PROFILE_ALL_SPANS=1`: every other span by its static name, wgpu's
                // `profiling::scope!` markers among them.
                if let Some(span) = ctx.span(id) {
                    span.extensions_mut()
                        .insert(SystemName(format!("span: {name}")));
                }
                return;
            }
            _ => return,
        };
        struct NameField(&'static str, Option<String>);
        impl Visit for NameField {
            fn record_str(&mut self, field: &Field, value: &str) {
                if field.name() == self.0 {
                    self.1 = Some(value.to_owned());
                }
            }
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == self.0 {
                    self.1 = Some(format!("{value:?}"));
                }
            }
        }
        let mut name = NameField(field_name, None);
        attrs.record(&mut name);
        if let (Some(name), Some(span)) = (name.1, ctx.span(id)) {
            span.extensions_mut()
                .insert(SystemName(format!("{prefix}{name}")));
        }
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut extensions = span.extensions_mut();
            if extensions.get_mut::<SystemName>().is_some() {
                extensions.replace(Entered(Instant::now()));
            }
        }
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let extensions = span.extensions();
        let (Some(name), Some(entered)) =
            (extensions.get::<SystemName>(), extensions.get::<Entered>())
        else {
            return;
        };
        let ns = entered.0.elapsed().as_nanos() as u64;
        if spike_ns().is_some_and(|spike| ns > spike) {
            static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
            let at = START.get_or_init(Instant::now).elapsed().as_secs_f64();
            diag::info!(
                Launch,
                "system spike: {:>8.3} ms at {at:>8.3}s  {}{}",
                ns as f64 / 1e6,
                name.0,
                thread_suffix()
            );
        }
        let Ok(mut totals) = TOTALS.lock() else {
            return;
        };
        let totals = totals.get_or_insert_with(HashMap::new);
        // A span is created once, on the thread that built the schedule; the thread it ran on is
        // only known here.
        let suffix = thread_suffix();
        let key = if suffix.is_empty() {
            std::borrow::Cow::Borrowed(name.0.as_str())
        } else {
            std::borrow::Cow::Owned(format!("{}{suffix}", name.0))
        };
        if let Some(row) = totals.get_mut(key.as_ref()) {
            row.0 += ns;
            row.1 += 1;
            row.2 = row.2.max(ns);
        } else {
            totals.insert(key.into_owned(), (ns, 1, ns));
        }
    }
}

#[derive(Default)]
struct ReportClock {
    started: Option<Instant>,
    frames: u64,
}

fn report(mut clock: Local<ReportClock>) {
    let started = *clock.started.get_or_insert_with(Instant::now);
    clock.frames += 1;
    if started.elapsed().as_secs_f32() < REPORT_SECONDS {
        return;
    }
    let frames = clock.frames.max(1);
    clock.started = Some(Instant::now());
    clock.frames = 0;
    let Some(totals) = TOTALS.lock().ok().and_then(|mut totals| totals.take()) else {
        return;
    };
    let mut rows: Vec<_> = totals.into_iter().collect();
    // `IW4L_PROFILE_BY_MAX=1` ranks by the longest single run, which points at a hitch rather
    // than at steady cost.
    if perf::switch("IW4L_PROFILE_BY_MAX") {
        rows.sort_by(|a, b| b.1.2.cmp(&a.1.2));
    } else {
        rows.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    }
    let all_ns: u64 = rows.iter().map(|(_, (ns, _, _))| ns).sum();
    diag::info!(
        Launch,
        "system profile: {frames} frames, all systems {:.3} ms/frame (summed over threads)",
        all_ns as f64 / 1e6 / frames as f64
    );
    for (name, (ns, count, max_ns)) in rows.into_iter().take(report_rows()) {
        diag::info!(
            Launch,
            "system profile: {:>8.3} ms/frame {:>6.2} runs/frame {:>8.3} max  {name}",
            ns as f64 / 1e6 / frames as f64,
            count as f64 / frames as f64,
            max_ns as f64 / 1e6
        );
    }
}

/// `LogPlugin::custom_layer`: install the aggregating layer and its reporter.
pub(crate) fn layer(app: &mut App) -> Option<bevy::log::BoxedLayer> {
    app.add_systems(Last, report);
    Some(Box::new(SystemProfileLayer))
}
