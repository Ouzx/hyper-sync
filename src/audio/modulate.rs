use crate::audio::dynamics;
use crate::audio::state::AudioSnapshot;
use crate::config::SoundMode;
use crate::layout::{led_zone, LedZone};

pub fn modulate_rgb(
    rgb: &mut [u8],
    n: usize,
    base_brightness: f32,
    mode: SoundMode,
    reactivity: f32,
    sensitivity: f32,
    snap: &AudioSnapshot,
) {
    if mode == SoundMode::Off || base_brightness <= 0.0 {
        return;
    }
    let react = reactivity.clamp(0.0, 1.0);
    match mode {
        SoundMode::Off => {}
        SoundMode::Level => {
            let gain = brightness_gain(base_brightness, react, sensitivity, snap.bass_level());
            apply_gain(rgb, gain);
        }
        SoundMode::Balance => {
            let l = snap.left();
            let r = snap.right();
            let sum = l + r + 1e-6;
            let side = (r - l) / sum;
            let right_w = (0.5 + side).clamp(0.0, 1.0);
            let left_w = (0.5 - side).clamp(0.0, 1.0);
            let center_w = (1.0 - side.abs()).clamp(0.0, 1.0);
            let level = snap.bass_level();
            for i in 0..n {
                let pan = match led_zone(i, n) {
                    LedZone::Right => right_w,
                    LedZone::Top => center_w,
                    LedZone::Left => left_w,
                };
                let audio = (level * (0.35 + 0.65 * pan)).clamp(0.0, 1.0);
                let gain = brightness_gain(base_brightness, react, sensitivity, audio);
                scale_pixel(rgb, i, gain);
            }
        }
    }
}

/// Map audio level + reactivity into a gain that reaches full brightness at peak audio.
fn brightness_gain(
    base_brightness: f32,
    reactivity: f32,
    sensitivity: f32,
    audio_level: f32,
) -> f32 {
    let base = base_brightness.clamp(0.01, 1.0);
    let shape = dynamics::from_sensitivity(sensitivity).shape_pow;
    let shaped = audio_level.clamp(0.0, 1.0).powf(shape);
    let target = (base + reactivity * shaped * (1.0 - base)).min(1.0);
    target / base
}

fn apply_gain(rgb: &mut [u8], gain: f32) {
    for c in rgb.iter_mut() {
        *c = (f32::from(*c) * gain).min(255.0) as u8;
    }
}

fn scale_pixel(rgb: &mut [u8], i: usize, gain: f32) {
    let base_idx = i * 3;
    if base_idx + 2 >= rgb.len() {
        return;
    }
    for c in &mut rgb[base_idx..base_idx + 3] {
        *c = (f32::from(*c) * gain).min(255.0) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::brightness_gain;

    #[test]
    fn boost_scales_to_full_brightness() {
        assert!((brightness_gain(0.25, 1.0, 0.5, 0.0) - 1.0).abs() < 1e-5);
        assert!((brightness_gain(0.25, 1.0, 0.5, 1.0) - 4.0).abs() < 1e-5);
    }
}
