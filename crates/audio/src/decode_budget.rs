use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const DECODE_LIMIT_BYTES: usize = 64 * 1024 * 1024;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static REFUSED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug)]
pub struct DecodeMemory {
    pub limit_bytes: usize,
    pub live_bytes: usize,
    pub peak_bytes: usize,
    pub refused: u64,
}

pub fn decode_memory() -> DecodeMemory {
    DecodeMemory {
        limit_bytes: DECODE_LIMIT_BYTES,
        live_bytes: LIVE.load(Ordering::Acquire),
        peak_bytes: PEAK.load(Ordering::Relaxed),
        refused: REFUSED.load(Ordering::Relaxed),
    }
}

pub(crate) struct DecodeReservation(usize);

impl DecodeReservation {
    pub fn reserve(bytes: usize) -> Result<Self, crate::media::PcmError> {
        if let Ok(before) = LIVE.fetch_update(Ordering::AcqRel, Ordering::Relaxed, |live| {
            live.checked_add(bytes)
                .filter(|&total| total <= DECODE_LIMIT_BYTES)
        }) {
            PEAK.fetch_max(before + bytes, Ordering::Relaxed);
            return Ok(Self(bytes));
        }
        REFUSED.fetch_add(1, Ordering::Relaxed);
        Err(crate::media::PcmError::MemoryLimit)
    }
}

impl Drop for DecodeReservation {
    fn drop(&mut self) {
        LIVE.fetch_sub(self.0, Ordering::AcqRel);
    }
}

pub(crate) struct DecodeSamples {
    chunks: Vec<Box<[i16]>>,
    current: Vec<i16>,
    len: usize,
    reservation: crate::pcm_budget::PcmReservation,
}

impl DecodeSamples {
    pub fn new() -> Result<Self, crate::media::PcmError> {
        Ok(Self {
            chunks: Vec::new(),
            current: Vec::new(),
            len: 0,
            reservation: crate::pcm_budget::PcmReservation::reserve(0)?,
        })
    }

    pub fn extend(&mut self, mut samples: &[i16]) -> Result<(), crate::media::PcmError> {
        use crate::media::CHUNK_SAMPLES;
        while !samples.is_empty() {
            if self.current.len() == CHUNK_SAMPLES || self.current.capacity() == 0 {
                if !self.current.is_empty() {
                    self.chunks
                        .push(std::mem::take(&mut self.current).into_boxed_slice());
                }
                self.reservation
                    .absorb(crate::pcm_budget::PcmReservation::reserve(
                        CHUNK_SAMPLES * size_of::<i16>(),
                    )?);
                self.current = Vec::with_capacity(CHUNK_SAMPLES);
            }
            let take = samples.len().min(CHUNK_SAMPLES - self.current.len());
            self.current.extend_from_slice(&samples[..take]);
            self.len += take;
            samples = &samples[take..];
        }
        Ok(())
    }

    pub fn into_pcm(
        mut self,
        channels: u16,
        rate: u32,
    ) -> Result<crate::media::PcmBuffer, crate::media::PcmError> {
        if !self.current.is_empty() {
            self.chunks.push(self.current.into_boxed_slice());
        }
        crate::media::PcmBuffer::from_chunks(
            self.chunks.into_boxed_slice(),
            self.len,
            channels,
            rate,
            self.reservation,
        )
    }
}
