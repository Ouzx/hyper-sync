use std::f32::consts::TAU;

use crate::config::EffectMode;
use crate::effects::solid::{hsv_to_rgb, is_rainbow_color, resolve_pixel_color};
use crate::effects::{hash, noise, Ctx};
use crate::layout::{height, segment_bounds};

pub fn render(mode: EffectMode, c: &mut Ctx) -> Vec<u8> {
    let (n, t, dt) = (c.n, c.t, c.dt);
    let nf = n as f32;
    let accent = c.cfg.solid.color.as_str();
    let color = |i: usize| resolve_pixel_color(i, n, accent);

    // Effects with memory update it once per frame, before drawing.
    match mode {
        EffectMode::Sparkle => {
            for (i, level) in c.mem.iter_mut().enumerate() {
                *level *= (-dt * 4.0).exp();
                if hash(i as u32, c.frame) < dt * 0.4 {
                    *level = 1.0;
                }
            }
        }
        EffectMode::Lightning => {
            let lit = c.mem.iter().copied().fold(0.0, f32::max) > 0.3;
            for level in c.mem.iter_mut() {
                *level *= (-dt * 9.0).exp();
            }
            // Rare strikes, then a burst of re-strikes while the sky is still lit.
            if hash(u32::MAX, c.frame) < dt * if lit { 8.0 } else { 0.4 } {
                let center = hash(1, c.frame) * nf;
                let half = 3.0 + hash(2, c.frame) * 8.0;
                let level = 0.6 + 0.4 * hash(3, c.frame);
                for (i, l) in c.mem.iter_mut().enumerate() {
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
            // A comet: one lap every 2 s at speed 1, tail fading behind the head.
            EffectMode::Chase => {
                let behind = ((t * 0.5).fract() * nf - i as f32).rem_euclid(nf);
                scale(color(i), (1.0 - behind / 10.0).max(0.0).powi(2))
            }
            EffectMode::Wave => {
                scale(color(i), 0.35 + 0.65 * (0.5 + 0.5 * ((x - t) * TAU).sin()))
            }
            EffectMode::Scanner => {
                let phase = (t * 0.5).fract() * 2.0;
                let u = if phase <= 1.0 { phase } else { 2.0 - phase };
                let d = (i as f32 - u * (nf - 1.0)).abs();
                scale(color(i), (1.0 - d / 3.0).max(0.0))
            }
            EffectMode::Sparkle => scale(color(i), c.mem[i] * c.mem[i]),
            EffectMode::Pulse => scale(color(i), 0.3 + 0.7 * (0.5 - 0.5 * (t * TAU).cos())),
            EffectMode::Aurora => {
                let hue = hsv_to_rgb((t * 0.05).fract(), 0.6, 1.0);
                let base = color(i);
                let w1 = 0.5 + 0.5 * (x * 3.0 + t * 0.4).sin();
                let w2 = 0.5 + 0.5 * (x * 5.0 - t * 0.3).sin();
                let blend = (w1 + w2) * 0.5;
                let mixed = std::array::from_fn(|k| {
                    (f32::from(base[k]) * (1.0 - blend) + f32::from(hue[k]) * blend) as u8
                });
                scale(mixed, 0.4 + 0.6 * (x * 2.0 + t * 0.2).sin().max(0.0))
            }
            // Flames rise from the bottom of both sides; neighbours share flicker so it moves in tongues.
            EffectMode::Fire => {
                let fl = |j: usize| noise(j as u32, t * 5.0);
                let flick = 0.5 * fl(i) + 0.25 * (fl(i.wrapping_sub(1)) + fl(i + 1));
                let heat = (1.0 - 0.8 * height(i, n)) * (0.5 + 0.5 * flick) * 0.9;
                heat_color(heat)
            }
            // Lub-dub: two soft bumps per beat, one beat per second at speed 1.
            EffectMode::Heartbeat => {
                let b = t.fract();
                let bump = |at: f32, w: f32| (-((b - at) / w).powi(2)).exp();
                scale(color(i), 0.12 + 0.88 * bump(0.08, 0.05) + 0.55 * bump(0.32, 0.06))
            }
            EffectMode::Segment => {
                let (start, end) = segment_bounds(n)[(t.floor() as usize) % 3];
                scale(color(i), if (start..end).contains(&i) { 1.0 } else { 0.0 })
            }
            EffectMode::Strobe => scale(color(i), if (t * 4.0).fract() < 0.5 { 1.0 } else { 0.0 }),
            EffectMode::Wipe => {
                let fill = (t.fract() * nf).ceil() as usize;
                scale(color(i), if i < fill { 1.0 } else { 0.0 })
            }
            // A rainbow drifting around the strip, one lap every 4 s at speed 1.
            EffectMode::Flow => hsv_to_rgb((x - t * 0.25).rem_euclid(1.0), 1.0, 1.0),
            // Slow organic blobs of the accent color; rainbow accent drifts through hues.
            EffectMode::Lava => {
                let w = 0.5
                    + 0.25 * (TAU * (x * 1.5 + t * 0.07)).sin()
                    + 0.25 * (TAU * (x * 2.5 - t * 0.05) + 1.7).sin();
                let base = if is_rainbow_color(accent) {
                    let hue = x * 0.5 + t * 0.02 + 0.1 * (TAU * (x + t * 0.03)).sin();
                    hsv_to_rgb(hue.rem_euclid(1.0), 1.0, 1.0)
                } else {
                    color(i)
                };
                scale(base, 0.15 + 0.85 * w * w)
            }
            EffectMode::Lightning => scale(color(i), c.mem[i]),
            // Ten minutes from dark to warm daylight at speed 1; the sides light before the top.
            EffectMode::Sunrise => {
                let p = (t / 600.0).min(1.0);
                sunrise_color((p * 1.6 - height(i, n) * 0.6).clamp(0.0, 1.0))
            }
            _ => [0, 0, 0],
        };
        rgb.extend_from_slice(&px);
    }
    rgb
}

fn scale(color: [u8; 3], gain: f32) -> [u8; 3] {
    let g = gain.clamp(0.0, 1.0);
    color.map(|c| (f32::from(c) * g) as u8)
}

/// Black → red → yellow → white as heat goes 0 → 1.
fn heat_color(heat: f32) -> [u8; 3] {
    let h = heat.clamp(0.0, 1.0) * 3.0;
    [h, h - 1.0, h - 2.0].map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8)
}

fn sunrise_color(p: f32) -> [u8; 3] {
    const STOPS: [(f32, [f32; 3]); 5] = [
        (0.0, [0.0, 0.0, 0.0]),
        (0.25, [90.0, 5.0, 0.0]),
        (0.5, [255.0, 60.0, 0.0]),
        (0.75, [255.0, 130.0, 30.0]),
        (1.0, [255.0, 200.0, 140.0]),
    ];
    let k = STOPS.iter().position(|s| s.0 >= p).unwrap_or(STOPS.len() - 1).max(1);
    let ((p0, a), (p1, b)) = (STOPS[k - 1], STOPS[k]);
    let f = ((p - p0) / (p1 - p0)).clamp(0.0, 1.0);
    std::array::from_fn(|j| (a[j] + (b[j] - a[j]) * f) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RuntimeConfig;

    fn frames(mode: EffectMode, count: u32) -> Vec<Vec<u8>> {
        let cfg = RuntimeConfig::default();
        let audio = crate::audio::AudioSnapshot::new();
        let mut mem = vec![0.0; 65];
        (1..=count)
            .map(|frame| {
                let t = frame as f32 * 0.02;
                render(mode, &mut Ctx { n: 65, t, dt: 0.02, frame, cfg: &cfg, mem: &mut mem, audio: &audio })
            })
            .collect()
    }

    #[test]
    fn every_effect_renders_full_frames() {
        use EffectMode::*;
        for mode in [Chase, Wave, Scanner, Sparkle, Pulse, Aurora, Fire, Heartbeat, Segment, Strobe, Wipe, Flow, Lava, Lightning, Sunrise] {
            let out = frames(mode, 3000);
            assert!(out.iter().all(|f| f.len() == 65 * 3), "{mode:?}");
            assert!(out.iter().any(|f| f.iter().any(|&c| c > 0)), "{mode:?} never lights up");
        }
    }

    #[test]
    fn fire_burns_the_same_on_both_sides() {
        let sum = |f: &[u8], r: std::ops::Range<usize>| f[r.start * 3..r.end * 3].iter().map(|&c| c as u32).sum::<u32>();
        let (right, left) = frames(EffectMode::Fire, 500)
            .iter()
            .fold((0, 0), |(r, l), f| (r + sum(f, 0..17), l + sum(f, 48..65)));
        let ratio = right as f32 / left as f32;
        assert!((0.9..1.1).contains(&ratio), "right/left = {ratio}");
    }
}
