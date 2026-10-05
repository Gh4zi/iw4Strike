use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const PCM_LIMIT_BYTES: usize = 256 * 1024 * 1024;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static REFUSED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug)]
pub struct PcmMemory {
    pub limit_bytes: usize,
    pub live_bytes: usize,
    pub peak_bytes: usize,
    pub refused: u64,
}

pub fn pcm_memory() -> PcmMemory {
    PcmMemory {
        limit_bytes: PCM_LIMIT_BYTES,
        live_bytes: LIVE.load(Ordering::Acquire),
        peak_bytes: PEAK.load(Ordering::Relaxed),
        refused: REFUSED.load(Ordering::Relaxed),
    }
}

#[derive(Debug)]
pub(crate) struct PcmReservation(usize);

impl PcmReservation {
    pub fn reserve(bytes: usize) -> Result<Self, crate::media::PcmError> {
        if let Ok(before) = LIVE.fetch_update(Ordering::AcqRel, Ordering::Relaxed, |live| {
            live.checked_add(bytes)
                .filter(|&total| total <= PCM_LIMIT_BYTES)
        }) {
            PEAK.fetch_max(before + bytes, Ordering::Relaxed);
            return Ok(Self(bytes));
        }
        REFUSED.fetch_add(1, Ordering::Relaxed);
        Err(crate::media::PcmError::MemoryLimit)
    }
}

impl PcmReservation {
    pub fn absorb(&mut self, other: Self) {
        self.0 += other.0;
        std::mem::forget(other);
    }
}

impl Drop for PcmReservation {
    fn drop(&mut self) {
        LIVE.fetch_sub(self.0, Ordering::AcqRel);
    }
}
