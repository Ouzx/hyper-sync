use std::f32::consts::TAU;

use crate::audio::state::SPECTRUM_BANDS;

/// Longest analysis window (43 ms at 48 kHz) — what the capture loop keeps in history.
pub const WINDOW_MAX: usize = 2048;

pub fn band_frequencies() -> [f32; SPECTRUM_BANDS] {
    let min_f = 60.0f32;
    let max_f = 16_000.0f32;
    std::array::from_fn(|i| {
        let t = i as f32 / (SPECTRUM_BANDS - 1) as f32;
        min_f * (max_f / min_f).powf(t)
    })
}

/// One Goertzel filter per band, each with a Hann window about four periods long: bass needs a
/// long window to resolve, treble gets a short one and reacts within a couple of milliseconds.
pub struct Analyzer {
    bands: Vec<(f32, Vec<f32>)>,
}

impl Analyzer {
    pub fn new(sample_rate: f32) -> Self {
        let bands = band_frequencies()
            .iter()
            .map(|&f| {
                let len = ((4.0 * sample_rate / f) as usize).clamp(64, WINDOW_MAX);
                let hann = (0..len)
                    .map(|i| 0.5 - 0.5 * (TAU * i as f32 / (len - 1) as f32).cos())
                    .collect();
                (TAU * f / sample_rate, hann)
            })
            .collect();
        Self { bands }
    }

    /// Band magnitudes of the newest samples in `history` (signed mono), relative to the loudest band.
    pub fn analyze(&self, history: &[f32]) -> [f32; SPECTRUM_BANDS] {
        let mut out = [0.0f32; SPECTRUM_BANDS];
        for (o, (omega, window)) in out.iter_mut().zip(&self.bands) {
            let recent = &history[history.len().saturating_sub(window.len())..];
            *o = goertzel(recent, window, *omega);
        }
        // Floor ≈ -66 dBFS so hiss in a quiet pause doesn't get stretched to full-height bars.
        let peak = out.iter().copied().fold(0.0f32, f32::max).max(2e-4);
        out.map(|v| (v / peak).clamp(0.0, 1.0))
    }
}

fn goertzel(samples: &[f32], window: &[f32], omega: f32) -> f32 {
    let coeff = 2.0 * omega.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for (&x, &w) in samples.iter().zip(window) {
        let s0 = x * w + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let real = s1 - s2 * omega.cos();
    let imag = s2 * omega.sin();
    (real * real + imag * imag).sqrt() / window.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_lands_in_its_own_band() {
        let analyzer = Analyzer::new(48_000.0);
        let freqs = band_frequencies();
        for band in [1, 4, 6, 9, 12, 14] {
            let f = freqs[band];
            let tone: Vec<f32> = (0..WINDOW_MAX)
                .map(|n| 0.5 * (TAU * f * n as f32 / 48_000.0).sin())
                .collect();
            let out = analyzer.analyze(&tone);
            let loudest = (0..SPECTRUM_BANDS).max_by(|&a, &b| out[a].total_cmp(&out[b])).unwrap();
            assert_eq!(loudest, band, "{f:.0} Hz -> {out:?}");
        }
    }

    #[test]
    fn silence_stays_dark() {
        let out = Analyzer::new(48_000.0).analyze(&[1e-5; WINDOW_MAX]);
        assert!(out.iter().all(|&v| v < 0.1), "{out:?}");
    }
}
