//! Our own animated effects (Hyperion.ng ports live in hyperion.rs). Speeds are given at speed 1;
//! `c.t` already includes the speed setting (and the music surge when a sound mode is on).

use std::f32::consts::TAU;

use crate::config::EffectMode;
use crate::effects::solid::{hsv_to_rgb, is_rainbow_color, parse_color, resolve_pixel_color, rgb_to_hsv};
use crate::effects::{hash, noise, swing, Ctx};
use crate::layout::{height, segment_bounds};

pub fn render(mode: EffectMode, c: &mut Ctx) -> Vec<u8> {
    let (n, t, dt) = (c.n, c.t, c.dt);
    let nf = n as f32;
    let accent = c.cfg.solid.color.as_str();
    let rainbow = is_rainbow_color(accent);
    let color = |i: usize| resolve_pixel_color(i, n, accent);
    // Shades around the accent's hue; a rainbow accent drifts through every hue (one cycle / 50 s).
    let (hue0, sat0) = if rainbow {
        ((t * 0.02).fract(), 1.0)
    } else {
        let (h, s, _) = rgb_to_hsv(parse_color(accent).unwrap_or([255, 51, 0]));
        (h, s)
    };
    let shade = |dh: f32| hsv_to_rgb((hue0 + dh).rem_euclid(1.0), sat0, 1.0);

    // Effects with memory update it once per frame, before drawing.
    match mode {
        EffectMode::Sparkle => {
            for (i, level) in c.mem[..n].iter_mut().enumerate() {
                *level *= (-dt * 4.0).exp();
                if hash(i as u32, c.frame) < dt * 0.4 {
                    *level = 1.0;
                }
            }
        }
        EffectMode::Lightning => {
            let lit = c.mem[..n].iter().copied().fold(0.0, f32::max) > 0.3;
            for level in c.mem[..n].iter_mut() {
                *level *= (-dt * 9.0).exp();
            }
            // Rare strikes, then a burst of re-strikes while the sky is still lit.
            if hash(u32::MAX, c.frame) < dt * if lit { 8.0 } else { 0.4 } {
                let center = hash(1, c.frame) * nf;
                let half = 3.0 + hash(2, c.frame) * 8.0;
                let level = 0.6 + 0.4 * hash(3, c.frame);
                for (i, l) in c.mem[..n].iter_mut().enumerate() {
                    if (i as f32 - center).abs() <= half {
                        *l = l.max(level);
                    }
                }
            }
        }
        _ => {}
    }

    let mut rgb = Vec::with_capacity(n * 3);
    for i in 0..n {
        let x = i as f32 / nf;
        let px = match mode {
            // A comet: one lap every 2 s, tail fading behind the head.
            EffectMode::Chase => {
                let behind = ((t * 0.5).fract() * nf - i as f32).rem_euclid(nf);
                scale(color(i), swing((1.0 - behind / 10.0).max(0.0).powi(2), c.depth(1.0)))
            }
            // Two soft waves rolling around the strip; each LED rises and falls every 2 s.
            EffectMode::Wave => {
                let w = 0.5 + 0.5 * (TAU * (2.0 * x - t * 0.5)).sin();
                scale(color(i), swing(w, c.depth(0.65)))
            }
            EffectMode::Scanner => {
                let phase = (t * 0.5).fract() * 2.0;
                let u = if phase <= 1.0 { phase } else { 2.0 - phase };
                let d = (i as f32 - u * (nf - 1.0)).abs();
                scale(color(i), swing((1.0 - d / 3.0).max(0.0), c.depth(1.0)))
            }
            EffectMode::Sparkle => scale(color(i), swing(c.mem[i] * c.mem[i], c.depth(1.0))),
            // Slow breathing, one breath every 3 s; power sets how deep it goes.
            EffectMode::Pulse => {
                scale(color(i), swing(0.5 - 0.5 * (TAU * t / 3.0).cos(), c.depth(0.5)))
            }
            // Curtains drifting along the strip in two layers (~8 LEDs/s each way) with a fine shimmer.
            EffectMode::Aurora => {
                let a = noise(7, i as f32 / 7.0 - t * 1.2);
                let b = noise(11, i as f32 / 11.0 + t * 0.7 + 50.0);
                let glow = (0.6 * a + 0.4 * b).powf(1.5) * 0.85 + 0.15 * noise(i as u32, t * 3.0);
                let px = if rainbow {
                    // Green and teal curtains that flare violet at their brightest.
                    let hue = 0.3 + 0.2 * b + 0.35 * ((a - 0.7).max(0.0) / 0.3);
                    hsv_to_rgb(hue, 0.85, 1.0)
                } else {
                    shade((b - 0.5) * 0.25)
                };
                scale(px, swing(glow, c.depth(0.85)))
            }
            // Red at the bottom, orange toward the top; tongues of flame climb both sides (~9 LEDs/s).
            EffectMode::Fire => {
                let h = height(i, n);
                let [_, (t0, t1), _] = segment_bounds(n);
                let along = if (t0..t1).contains(&i) { (i - t0) as f32 / 6.0 } else { 0.0 };
                let side = u32::from(2 * i >= n);
                let rise = noise(3 + side, h * 4.0 + along - t * 2.2);
                let lick = 0.65 * rise + 0.35 * noise(i as u32, t * 9.0);
                let warm = (h * 0.75 + (lick - 0.5) * 0.3).clamp(0.0, 1.0);
                scale([255, (15.0 + 105.0 * warm) as u8, 0], swing(lick, c.depth(0.6)))
            }
            EffectMode::Segment => {
                let (start, end) = segment_bounds(n)[(t.floor() as usize) % 3];
                scale(color(i), swing(if (start..end).contains(&i) { 1.0 } else { 0.0 }, c.depth(1.0)))
            }
            // A rainbow drifting around the strip, one lap every 8 s.
            EffectMode::Flow => hsv_to_rgb((x - t / 8.0).rem_euclid(1.0), 1.0, 1.0),
            // Wax blobs rising up both sides (~4–6 LEDs/s) to meet at the top, swelling as they go.
            EffectMode::Lava => {
                let half = (nf - 1.0) / 2.0;
                let u = (i as f32).min(nf - 1.0 - i as f32) / half;
                let side = u32::from(i as f32 > half);
                let heat: f32 = (0..5)
                    .map(|k| {
                        let seed = side * 10 + k;
                        let speed = 0.12 + 0.08 * hash(seed, 1);
                        let size = 0.06 + 0.05 * (0.5 + 0.5 * (t * 0.8 + k as f32).sin());
                        let center = (t * speed + hash(seed, 2) * 1.3).rem_euclid(1.3) - 0.15;
                        (-((u - center) / size).powi(2)).exp()
                    })
                    .sum::<f32>()
                    .min(1.0);
                let px = if rainbow { hsv_to_rgb((hue0 + u * 0.15).fract(), 1.0, 1.0) } else { shade(0.04 * (heat - 0.5)) };
                scale(px, swing(heat, c.depth(0.85)))
            }
            EffectMode::Lightning => scale(color(i), swing(c.mem[i], c.depth(1.0))),
            _ => [0, 0, 0],
        };
        rgb.extend_from_slice(&px);
    }
    rgb
}

fn scale<T: Into<f32> + Copy>(color: [T; 3], gain: f32) -> [u8; 3] {
    let g = gain.clamp(0.0, 1.0);
    color.map(|c| (c.into() * g).min(255.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RuntimeConfig;

    /// Our effects must keep moving; the Hyperion ports run at Hyperion's own pace (some slow).
    const OURS: [EffectMode; 13] = {
        use EffectMode::*;
        [Chase, Wave, Scanner, Sparkle, Pulse, Aurora, Fire, Segment, Flow, Lava, Lightning, Candle, Solid]
    };
    const PORTS: [EffectMode; 29] = {
        use EffectMode::*;
        [MoodBlobs, Swirl, DoubleSwirl, Plasma, SeaWaves, ColorWaves, Juggler, Collision, LightClock, Traces, Flags, KnightRider, LedTest, LedTestSeq, Police, PoliceSolid, RainbowMood, Random, Snake, Sparks, Trails, Xmas, HyperionFire, Lights, Matrix, Breath, NotifyBlue, CinemaBrighten, CinemaDim]
    };

    /// 20 s of frames at 45 fps with default settings.
    fn frames(mode: EffectMode, cfg: &RuntimeConfig) -> Vec<Vec<u8>> {
        let audio = crate::audio::AudioSnapshot::new();
        let mut mem = vec![0.0; 65 * 3];
        let dt = 1.0 / 45.0;
        (1..=900)
            .map(|frame| {
                let t = frame as f32 * dt;
                let mut c = Ctx { n: 65, t, dt, frame, cfg, mem: &mut mem, seed: 7, audio: &audio };
                crate::effects::render(mode, &mut c)
            })
            .collect()
    }

    /// How fast LEDs change: per LED and frame the largest change of any channel (so hue shifts
    /// count, not just brightness), averaged, in full-scale units per second.
    fn motion(f: &[Vec<u8>]) -> f32 {
        let step = |a: &[u8], b: &[u8]| -> f32 {
            a.chunks(3).zip(b.chunks(3)).map(|(p, q)| (0..3).map(|k| p[k].abs_diff(q[k])).max().unwrap_or(0) as f32 / 255.0).sum()
        };
        let total: f32 = f.windows(2).map(|w| step(&w[0], &w[1])).sum();
        total / 65.0 / (f.len() - 1) as f32 * 45.0
    }

    #[test]
    fn every_effect_renders_and_moves() {
        let cfg = RuntimeConfig::default();
        for mode in OURS.into_iter().chain(PORTS) {
            let out = frames(mode, &cfg);
            assert!(out.iter().all(|f| f.len() == 65 * 3), "{mode:?}");
            assert!(out.iter().any(|f| f.iter().any(|&c| c > 0)), "{mode:?} never lights up");
            let m = motion(&out);
            let mut warm = cfg.clone();
            warm.solid.color = "ff3300".into();
            println!("{:>10}: motion {m:.2}/s rainbow, {:.2}/s ff3300", mode.as_str(), motion(&frames(mode, &warm)));
            if OURS.contains(&mode) && mode != EffectMode::Solid {
                assert!(m > 0.15, "{mode:?} barely moves ({m:.2}/s)");
            }
        }
    }

    #[test]
    fn power_scales_the_swing() {
        let mut cfg = RuntimeConfig::default();
        cfg.effect.power = 0.0;
        assert!(motion(&frames(EffectMode::Pulse, &cfg)) < 0.01, "power 0 is steady");
        cfg.effect.power = 0.5;
        let default = motion(&frames(EffectMode::Pulse, &cfg));
        cfg.effect.power = 1.0;
        assert!(motion(&frames(EffectMode::Pulse, &cfg)) > default * 1.8);
    }

    #[test]
    fn fire_is_red_at_the_bottom_and_orange_at_the_top() {
        let cfg = RuntimeConfig::default();
        let out = frames(EffectMode::Fire, &cfg);
        let green_share = |leds: &[usize]| {
            let (g, r) = out.iter().flat_map(|f| leds.iter().map(move |&i| (f[i * 3 + 1] as f32, f[i * 3] as f32))).fold((0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
            g / r
        };
        let (bottom, top) = (green_share(&[0, 1, 2, 62, 63, 64]), green_share(&[25, 30, 35, 40]));
        assert!(bottom < 0.2 && top > 0.3, "bottom {bottom:.2}, top {top:.2}");
    }
}
