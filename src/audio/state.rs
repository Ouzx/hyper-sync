use std::sync::atomic::{AtomicU32, Ordering};

pub const SPECTRUM_BANDS: usize = 16;

pub struct AudioSnapshot {
    level: AtomicU32,
    left: AtomicU32,
    right: AtomicU32,
    spectrum: [AtomicU32; SPECTRUM_BANDS],
}

impl Default for AudioSnapshot {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioSnapshot {
    pub fn new() -> Self {
        Self {
            level: AtomicU32::new(0),
            left: AtomicU32::new(0),
            right: AtomicU32::new(0),
            spectrum: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn left(&self) -> f32 {
        f32::from_bits(self.left.load(Ordering::Relaxed))
    }

    pub fn right(&self) -> f32 {
        f32::from_bits(self.right.load(Ordering::Relaxed))
    }

    pub fn spectrum(&self) -> [f32; SPECTRUM_BANDS] {
        std::array::from_fn(|i| f32::from_bits(self.spectrum[i].load(Ordering::Relaxed)))
    }

    pub fn store_levels(&self, level: f32, left: f32, right: f32) {
        self.level.store(level.to_bits(), Ordering::Relaxed);
        self.left.store(left.to_bits(), Ordering::Relaxed);
        self.right.store(right.to_bits(), Ordering::Relaxed);
    }

    pub fn store_spectrum(&self, bands: &[f32; SPECTRUM_BANDS]) {
        for (i, v) in bands.iter().enumerate() {
            self.spectrum[i].store(v.to_bits(), Ordering::Relaxed);
        }
    }
}
