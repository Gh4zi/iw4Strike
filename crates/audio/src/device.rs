use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::render_core::{QUANTUM, RenderShared, SAMPLE_RATE};

/// How long the device may go without asking for audio before the stream is rebuilt (a device
/// that hangs without reporting an error).
const CALLBACK_STALL: Duration = Duration::from_secs(2);

/// How often the supervisor refreshes the device list and looks for a moved system default.
const DEVICE_POLL: Duration = Duration::from_secs(1);

/// Why the stream was closed, as `failed` carries it.
const FAILED_UNAVAILABLE: u8 = 1;
const FAILED_INVALIDATED: u8 = 2;
const FAILED_STALLED: u8 = 4;
const SWITCH_PICKED: u8 = 5;
const SWITCH_DEVICE_MOVED: u8 = 6;

/// The output device the player picked (`audio_device`), the devices there are to pick from,
/// and the one the stream runs on. A pick takes effect at once: the supervisor reopens the
/// stream, mid-match included, so a headset connected after launch can be chosen without a
/// restart.
#[derive(Default)]
pub(crate) struct DeviceSelection {
    /// The picked device's name; `None` follows the system default.
    wanted: Mutex<Option<String>>,
    /// Bumped on every new pick, which is how the supervisor notices it.
    revision: AtomicU64,
    /// Every output device's name, refreshed by the supervisor.
    devices: Mutex<Vec<String>>,
    /// The device the stream runs on; `None` while there is no stream.
    current: Mutex<Option<String>>,
}

impl DeviceSelection {
    /// Pick a device by name; `None` (or an empty name) follows the system default.
    pub(crate) fn pick(&self, name: Option<&str>) {
        let name = name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        let mut wanted = lock(&self.wanted);
        if *wanted != name {
            *wanted = name;
            self.revision.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub(crate) fn devices(&self) -> Vec<String> {
        lock(&self.devices).clone()
    }

    pub(crate) fn current(&self) -> Option<String> {
        lock(&self.current).clone()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|description| {
            // WASAPI's first extended line is the endpoint's full friendly name ("Headphones
            // (AirPods Pro)"); the name alone ("Headphones") cannot tell two headsets apart.
            if cfg!(windows)
                && let Some(full) = description.extended().first()
            {
                return full.clone();
            }
            description.name().to_string()
        })
        .unwrap_or_else(|_| "unknown".into())
}

fn output_device_names(host: &cpal::Host) -> Vec<String> {
    host.output_devices()
        .map(|devices| devices.map(|device| device_name(&device)).collect())
        .unwrap_or_default()
}

/// The picked device, else the system default, and whether a pick had to fall back.
fn choose_device(host: &cpal::Host, wanted: Option<&str>) -> Option<(cpal::Device, bool)> {
    if let Some(wanted) = wanted
        && let Some(device) = host
            .output_devices()
            .ok()
            .and_then(|mut devices| devices.find(|device| device_name(device) == wanted))
    {
        return Some((device, false));
    }
    host.default_output_device()
        .map(|device| (device, wanted.is_some()))
}

pub(crate) fn supervise(shared: Arc<RenderShared>, shutdown: Arc<AtomicBool>) {
    let host = cpal::default_host();
    let selection = &shared.device_selection;
    let failed = Arc::new(AtomicU8::new(0));
    let epoch = Instant::now();
    // When the device last asked for audio, in ms since `epoch`.
    let heartbeat = Arc::new(AtomicU64::new(0));
    while !shutdown.load(Ordering::Acquire) {
        failed.store(0, Ordering::Release);
        heartbeat.store(epoch.elapsed().as_millis() as u64, Ordering::Release);
        let revision = selection.revision.load(Ordering::Acquire);
        let wanted = lock(&selection.wanted).clone();
        *lock(&selection.devices) = output_device_names(&host);
        let mut switching = false;
        match open(
            &host,
            wanted.as_deref(),
            shared.clone(),
            failed.clone(),
            epoch,
            heartbeat.clone(),
        ) {
            Ok((stream, running_on, fell_back)) => {
                if fell_back && let Some(wanted) = &wanted {
                    diag::warn!(
                        Audio,
                        "audio: output device `{wanted}` not found; on the system default `{running_on}` until it is back"
                    );
                }
                *lock(&selection.current) = Some(running_on.clone());
                let mut next_poll = Instant::now() + DEVICE_POLL;
                while !shutdown.load(Ordering::Acquire) && failed.load(Ordering::Acquire) == 0 {
                    std::thread::sleep(Duration::from_millis(20));
                    let quiet = (epoch.elapsed().as_millis() as u64)
                        .saturating_sub(heartbeat.load(Ordering::Acquire));
                    if quiet > CALLBACK_STALL.as_millis() as u64 {
                        failed.store(FAILED_STALLED, Ordering::Release);
                    } else if selection.revision.load(Ordering::Acquire) != revision {
                        failed.store(SWITCH_PICKED, Ordering::Release);
                    } else if Instant::now() >= next_poll {
                        next_poll = Instant::now() + DEVICE_POLL;
                        let names = output_device_names(&host);
                        let moved = match wanted.as_deref() {
                            // Following the system default, which moved (a headset connected).
                            None => host
                                .default_output_device()
                                .is_some_and(|device| device_name(&device) != running_on),
                            // The picked device was missing and is back.
                            Some(wanted) => fell_back && names.iter().any(|name| name == wanted),
                        };
                        *lock(&selection.devices) = names;
                        if moved {
                            failed.store(SWITCH_DEVICE_MOVED, Ordering::Release);
                        }
                    }
                }
                drop(stream);
                *lock(&selection.current) = None;
                match failed.swap(0, Ordering::AcqRel) {
                    0 => {}
                    SWITCH_PICKED => {
                        switching = true;
                        diag::info!(
                            Audio,
                            "audio: switching output device (picked in the settings)"
                        );
                    }
                    SWITCH_DEVICE_MOVED => {
                        switching = true;
                        diag::info!(
                            Audio,
                            "audio: switching output device (system default moved, or the picked device is back)"
                        );
                    }
                    failure => {
                        let reason = match failure {
                            FAILED_UNAVAILABLE => "device unavailable",
                            FAILED_INVALIDATED => "stream invalidated",
                            _ => "device stopped asking for audio",
                        };
                        diag::warn!(Audio, "audio: output interrupted: {reason}");
                    }
                }
            }
            Err(error) => diag::warn!(
                Audio,
                "audio: device unavailable, using null transport: {error}"
            ),
        }
        shared.device_active.store(false, Ordering::Release);
        if switching {
            continue;
        }
        // A device that just failed gets a second before it is tried again, unless a pick
        // comes in meanwhile.
        for _ in 0..50 {
            if shutdown.load(Ordering::Acquire)
                || selection.revision.load(Ordering::Acquire) != revision
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Open the output stream on the picked device (else the system default): the stream, the
/// device's name, and whether the pick had to fall back to the default.
pub(crate) fn open(
    host: &cpal::Host,
    wanted: Option<&str>,
    shared: Arc<RenderShared>,
    failed: Arc<AtomicU8>,
    epoch: Instant,
    heartbeat: Arc<AtomicU64>,
) -> Result<(cpal::Stream, String, bool), String> {
    let (device, fell_back) = choose_device(host, wanted).ok_or("no output device")?;
    let config = device
        .default_output_config()
        .map_err(|error| error.to_string())?;
    if config.channels() == 0 || config.sample_rate() < 8000 {
        return Err("invalid output channel count or unsupported low device rate".into());
    }
    let format = config.sample_format();
    let config = config.config();
    let transport = shared.clone();
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config, shared, failed, epoch, heartbeat),
        SampleFormat::F64 => build::<f64>(&device, &config, shared, failed, epoch, heartbeat),
        SampleFormat::I8 => build::<i8>(&device, &config, shared, failed, epoch, heartbeat),
        SampleFormat::I16 => build::<i16>(&device, &config, shared, failed, epoch, heartbeat),
        SampleFormat::I24 => build::<cpal::I24>(&device, &config, shared, failed, epoch, heartbeat),
        SampleFormat::I32 => build::<i32>(&device, &config, shared, failed, epoch, heartbeat),
        SampleFormat::I64 => build::<i64>(&device, &config, shared, failed, epoch, heartbeat),
        SampleFormat::U8 => build::<u8>(&device, &config, shared, failed, epoch, heartbeat),
        SampleFormat::U16 => build::<u16>(&device, &config, shared, failed, epoch, heartbeat),
        SampleFormat::U32 => build::<u32>(&device, &config, shared, failed, epoch, heartbeat),
        SampleFormat::U64 => build::<u64>(&device, &config, shared, failed, epoch, heartbeat),
        _ => return Err(format!("unsupported device format {format:?}")),
    }
    .map_err(|error| error.to_string())?;
    transport.device_active.store(true, Ordering::Release);
    stream.play().map_err(|error| error.to_string())?;
    // Which device: a Bluetooth headset can come back as another endpoint (its hands-free one)
    // after the stream is rebuilt.
    let device_name = device_name(&device);
    diag::info!(
        Audio,
        "audio: output running on `{device_name}` rate={} channels={} format={format:?}",
        config.sample_rate,
        config.channels
    );
    Ok((stream, device_name, fell_back))
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<RenderShared>,
    failed: Arc<AtomicU8>,
    epoch: Instant,
    heartbeat: Arc<AtomicU64>,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let channels = usize::from(config.channels);
    let step = f64::from(SAMPLE_RATE) / f64::from(config.sample_rate);
    let mut block = [[0.0; 2]; QUANTUM];
    let mut position = QUANTUM;
    let mut fractional = 0.0;
    let mut a = [0.0; 2];
    let mut b = [0.0; 2];
    let mut primed = false;
    let errors = shared.clone();
    let backend_errors = AtomicU32::new(0);
    device.build_output_stream(
        config,
        move |output: &mut [T], _: &cpal::OutputCallbackInfo| {
            heartbeat.store(epoch.elapsed().as_millis() as u64, Ordering::Release);
            let mut next = || {
                if position == QUANTUM {
                    shared.render_for(&mut block, Some(true));
                    position = 0;
                }
                let sample = block[position];
                position += 1;
                sample
            };
            if !primed {
                a = next();
                b = next();
                primed = true;
            }
            let mut frames = output.chunks_exact_mut(channels);
            for frame in frames.by_ref() {
                let t = fractional as f32;
                let left = a[0] + (b[0] - a[0]) * t;
                let right = a[1] + (b[1] - a[1]) * t;
                frame[0] = T::from_sample(if channels == 1 {
                    (left + right) * 0.5
                } else {
                    left
                });
                if channels > 1 {
                    frame[1] = T::from_sample(right);
                }
                for sample in &mut frame[2.min(channels)..] {
                    *sample = T::EQUILIBRIUM;
                }
                fractional += step;
                while fractional >= 1.0 {
                    a = b;
                    b = next();
                    fractional -= 1.0;
                }
            }
            frames.into_remainder().fill(T::EQUILIBRIUM);
        },
        move |error| match error {
            cpal::StreamError::BufferUnderrun => {
                errors.device_underruns.fetch_add(1, Ordering::Relaxed);
            }
            cpal::StreamError::DeviceNotAvailable => {
                failed.store(FAILED_UNAVAILABLE, Ordering::Release);
            }
            cpal::StreamError::StreamInvalidated => {
                failed.store(FAILED_INVALIDATED, Ordering::Release);
            }
            // A backend's own report (ALSA / PipeWire glitches, an xrun it recovered from) isn't
            // the stream ending: it plays on. A device that does stop gets rebuilt by the
            // supervisor once it stops asking for audio.
            cpal::StreamError::BackendSpecific { err } => {
                let n = backend_errors.fetch_add(1, Ordering::Relaxed) + 1;
                if n.is_power_of_two() {
                    diag::warn!(Audio, "audio: backend error #{n} (stream kept): {err}");
                }
            }
        },
        Some(Duration::from_secs(1)),
    )
}
