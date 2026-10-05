use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::render_core::{QUANTUM, RenderShared, SAMPLE_RATE};

pub(crate) fn supervise(shared: Arc<RenderShared>, shutdown: Arc<AtomicBool>) {
    let failed = Arc::new(AtomicU8::new(0));
    while !shutdown.load(Ordering::Acquire) {
        failed.store(0, Ordering::Release);
        match open(shared.clone(), failed.clone()) {
            Ok(stream) => {
                while !shutdown.load(Ordering::Acquire) && failed.load(Ordering::Acquire) == 0 {
                    std::thread::sleep(Duration::from_millis(20));
                }
                drop(stream);
                let failure = failed.swap(0, Ordering::AcqRel);
                if failure != 0 {
                    let reason = match failure {
                        1 => "device unavailable",
                        2 => "stream invalidated",
                        _ => "backend failure",
                    };
                    diag::warn!(Audio, "audio: output interrupted: {reason}");
                }
            }
            Err(error) => diag::warn!(
                Audio,
                "audio: device unavailable, using null transport: {error}"
            ),
        }
        shared.device_active.store(false, Ordering::Release);
        for _ in 0..50 {
            if shutdown.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

pub(crate) fn open(
    shared: Arc<RenderShared>,
    failed: Arc<AtomicU8>,
) -> Result<cpal::Stream, String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("no default output device")?;
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
        SampleFormat::F32 => build::<f32>(&device, &config, shared, failed),
        SampleFormat::F64 => build::<f64>(&device, &config, shared, failed),
        SampleFormat::I8 => build::<i8>(&device, &config, shared, failed),
        SampleFormat::I16 => build::<i16>(&device, &config, shared, failed),
        SampleFormat::I24 => build::<cpal::I24>(&device, &config, shared, failed),
        SampleFormat::I32 => build::<i32>(&device, &config, shared, failed),
        SampleFormat::I64 => build::<i64>(&device, &config, shared, failed),
        SampleFormat::U8 => build::<u8>(&device, &config, shared, failed),
        SampleFormat::U16 => build::<u16>(&device, &config, shared, failed),
        SampleFormat::U32 => build::<u32>(&device, &config, shared, failed),
        SampleFormat::U64 => build::<u64>(&device, &config, shared, failed),
        _ => return Err(format!("unsupported device format {format:?}")),
    }
    .map_err(|error| error.to_string())?;
    transport.device_active.store(true, Ordering::Release);
    stream.play().map_err(|error| error.to_string())?;
    diag::info!(
        Audio,
        "audio: output running rate={} channels={} format={format:?}",
        config.sample_rate,
        config.channels
    );
    Ok(stream)
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<RenderShared>,
    failed: Arc<AtomicU8>,
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
    device.build_output_stream(
        config,
        move |output: &mut [T], _: &cpal::OutputCallbackInfo| {
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
            cpal::StreamError::DeviceNotAvailable => failed.store(1, Ordering::Release),
            cpal::StreamError::StreamInvalidated => failed.store(2, Ordering::Release),
            _ => failed.store(3, Ordering::Release),
        },
        Some(Duration::from_secs(1)),
    )
}
