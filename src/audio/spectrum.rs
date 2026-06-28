use std::f32::consts::TAU;

use crate::audio::state::SPECTRUM_BANDS;

const SAMPLE_RATE: f32 = 48_000.0;

pub fn band_frequencies() -> [f32; SPECTRUM_BANDS] {
    let min_f = 60.0f32;
    let max_f = 16_000.0f32;
    std::array::from_fn(|i| {
        let t = i as f32 / (SPECTRUM_BANDS - 1) as f32;
        min_f * (max_f / min_f).powf(t)
    })
}

pub fn analyze_mono(samples: &[f32], sample_rate: f32) -> [f32; SPECTRUM_BANDS] {
    let freqs = band_frequencies();
    let mut out = [0.0f32; SPECTRUM_BANDS];
    if samples.is_empty() {
        return out;
    }
    for (i, &freq) in freqs.iter().enumerate() {
        out[i] = goertzel_mag(samples, sample_rate, freq);
    }
    let peak = out.iter().copied().fold(0.0f32, f32::max).max(1e-6);
    for v in &mut out {
        *v = (*v / peak).clamp(0.0, 1.0);
    }
    out
}

fn goertzel_mag(samples: &[f32], sample_rate: f32, freq: f32) -> f32 {
    let omega = TAU * freq / sample_rate;
    let coeff = 2.0 * omega.cos();
    let mut s0 = 0.0f32;
    let mut s1 = 0.0f32;
    let mut s2 = 0.0f32;
    for &x in samples {
        s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let real = s1 - s2 * omega.cos();
    let imag = s2 * omega.sin();
    (real * real + imag * imag).sqrt() / samples.len() as f32
}

pub const DEFAULT_SAMPLE_RATE: f32 = SAMPLE_RATE;
