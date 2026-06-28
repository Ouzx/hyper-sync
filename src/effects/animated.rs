use std::f32::consts::TAU;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{EffectMode, RuntimeConfig};
use crate::daemon::DaemonStatus;
use crate::effects::solid::{resolve_pixel_color, scale_rgb};
use crate::protocol::build_frame;
use crate::serial::SerialWriter;

pub fn run_controlled(
    mode: EffectMode,
    writer: Arc<Mutex<SerialWriter>>,
    config: Arc<RwLock<RuntimeConfig>>,
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<DaemonStatus>>,
) -> anyhow::Result<()> {
    let n = config.read().unwrap().device.leds as usize;
    let mut fire_flicker = vec![1.0f32; n];
    let start = Instant::now();

    while !cancel.load(Ordering::Relaxed) {
        let cfg = config.read().unwrap().clone();
        if cfg.effect.mode != mode {
            break;
        }

        let interval = Duration::from_micros(1_000_000 / u64::from(cfg.effect.fps.max(1)));
        let speed = cfg.effect.speed.max(0.1);
        let t = start.elapsed().as_secs_f32() * speed;
        let accent = cfg.solid.color.as_str();
        let rgb = render_frame(
            mode,
            n,
            t,
            accent,
            cfg.effect.brightness,
            &mut fire_flicker,
        );
        let frame = build_frame(cfg.device.leds, &rgb)?;
        writer.lock().unwrap().write_frame(&frame)?;
        {
            let mut st = status.lock().unwrap();
            st.brightness = cfg.effect.brightness;
            st.fps = cfg.effect.fps;
            st.speed = cfg.effect.speed;
            st.serial_ok = true;
            st.detail = mode.as_str().into();
            st.color = cfg.solid.color.clone();
        }
        thread::sleep(interval);
    }
    Ok(())
}

fn render_frame(
    mode: EffectMode,
    n: usize,
    t: f32,
    accent: &str,
    brightness: f32,
    fire_flicker: &mut [f32],
) -> Vec<u8> {
    let rgb = match mode {
        EffectMode::Chase => render_chase(n, t, accent),
        EffectMode::Wave => render_wave(n, t, accent),
        EffectMode::Scanner => render_scanner(n, t, accent),
        EffectMode::Sparkle => render_sparkle(n, t, accent),
        EffectMode::Pulse => render_pulse(n, t, accent),
        EffectMode::Aurora => render_aurora(n, t, accent),
        EffectMode::Fire => render_fire(n, t, accent, fire_flicker),
        EffectMode::Heartbeat => render_heartbeat(n, t, accent),
        EffectMode::Segment => render_segment(n, t, accent),
        EffectMode::Strobe => render_strobe(n, t, accent),
        EffectMode::Wipe => render_wipe(n, t, accent),
        _ => vec![0; n * 3],
    };
    apply_brightness(&rgb, brightness)
}

fn apply_brightness(rgb: &[u8], brightness: f32) -> Vec<u8> {
    let b = brightness.clamp(0.0, 1.0);
    rgb.iter()
        .map(|c| (f32::from(*c) * b) as u8)
        .collect()
}

fn dist_loop(i: usize, pos: f32, n: usize) -> f32 {
    let d = (i as f32 - pos).abs();
    let n = n as f32;
    d.min(n - d)
}

fn segment_bounds(n: usize) -> [(usize, usize); 3] {
    let right = n * 17 / 65;
    let top = n * 31 / 65;
    let left = n.saturating_sub(right + top);
    [(0, right), (right, right + top), (right + top, right + top + left)]
}

fn set_pixel(rgb: &mut [u8], i: usize, color: [u8; 3], gain: f32) {
    let g = gain.clamp(0.0, 1.0);
    let base = i * 3;
    if base + 2 < rgb.len() {
        rgb[base] = (f32::from(color[0]) * g) as u8;
        rgb[base + 1] = (f32::from(color[1]) * g) as u8;
        rgb[base + 2] = (f32::from(color[2]) * g) as u8;
    }
}

fn render_chase(n: usize, t: f32, accent: &str) -> Vec<u8> {
    let mut rgb = vec![0u8; n * 3];
    let pos = t.rem_euclid(n as f32);
    let tail = 8.0f32;
    for i in 0..n {
        let d = dist_loop(i, pos, n);
        let gain = ((1.0 - d / tail).max(0.0)).powi(2);
        set_pixel(&mut rgb, i, resolve_pixel_color(i, n, accent), gain);
    }
    rgb
}

fn render_wave(n: usize, t: f32, accent: &str) -> Vec<u8> {
    let mut rgb = vec![0u8; n * 3];
    for i in 0..n {
        let phase = i as f32 / n as f32 - t;
        let gain = 0.35 + 0.65 * (phase * TAU).sin().max(0.0);
        set_pixel(&mut rgb, i, resolve_pixel_color(i, n, accent), gain);
    }
    rgb
}

fn render_scanner(n: usize, t: f32, accent: &str) -> Vec<u8> {
    let mut rgb = vec![0u8; n * 3];
    let phase = (t * 0.5).fract() * 2.0;
    let u = if phase <= 1.0 { phase } else { 2.0 - phase };
    let pos = u * (n.saturating_sub(1) as f32);
    for i in 0..n {
        let d = (i as f32 - pos).abs();
        let gain = (1.0 - d / 3.0).max(0.0);
        set_pixel(&mut rgb, i, resolve_pixel_color(i, n, accent), gain);
    }
    rgb
}

fn render_sparkle(n: usize, t: f32, accent: &str) -> Vec<u8> {
    let mut rgb = vec![0u8; n * 3];
    for i in 0..n {
        let spark = rand_unit(i, t);
        let gain = if spark > 0.92 { (spark - 0.92) / 0.08 } else { 0.0 };
        set_pixel(&mut rgb, i, resolve_pixel_color(i, n, accent), gain);
    }
    rgb
}

fn render_pulse(n: usize, t: f32, accent: &str) -> Vec<u8> {
    let gain = 0.3 + 0.7 * (t * TAU).sin().max(0.0);
    let mut rgb = vec![0u8; n * 3];
    for i in 0..n {
        set_pixel(&mut rgb, i, resolve_pixel_color(i, n, accent), gain);
    }
    rgb
}

fn render_aurora(n: usize, t: f32, accent: &str) -> Vec<u8> {
    let mut rgb = vec![0u8; n * 3];
    let accent_hue = crate::effects::solid::hsv_to_rgb((t * 0.05).fract(), 0.6, 1.0);
    for i in 0..n {
        let base = resolve_pixel_color(i, n, accent);
        let x = i as f32 / n as f32;
        let w1 = 0.5 + 0.5 * (x * 3.0 + t * 0.4).sin();
        let w2 = 0.5 + 0.5 * (x * 5.0 - t * 0.3).sin();
        let blend = (w1 + w2) * 0.5;
        let mixed = [
            ((f32::from(base[0]) * (1.0 - blend) + f32::from(accent_hue[0]) * blend) as u8),
            ((f32::from(base[1]) * (1.0 - blend) + f32::from(accent_hue[1]) * blend) as u8),
            ((f32::from(base[2]) * (1.0 - blend) + f32::from(accent_hue[2]) * blend) as u8),
        ];
        let gain = 0.4 + 0.6 * (x * 2.0 + t * 0.2).sin().max(0.0);
        set_pixel(&mut rgb, i, mixed, gain);
    }
    rgb
}

fn render_fire(n: usize, t: f32, accent: &str, flicker: &mut [f32]) -> Vec<u8> {
    let mut rgb = vec![0u8; n * 3];
    let bounds = segment_bounds(n);
    let top_start = bounds[1].0;
    for i in 0..n {
        let warm = scale_rgb(resolve_pixel_color(i, n, accent), 1.0);
        let height = if i >= top_start {
            1.0
        } else if i >= bounds[0].1 {
            0.6 + 0.4 * (i - bounds[0].1) as f32 / (top_start - bounds[0].1).max(1) as f32
        } else {
            0.3 + 0.3 * i as f32 / bounds[0].1.max(1) as f32
        };
        let target = 0.85 + rand_unit(i, t) * 0.3;
        flicker[i] = flicker[i] * 0.8 + target * 0.2;
        let gain = flicker[i] * height;
        set_pixel(&mut rgb, i, warm, gain);
    }
    rgb
}

fn render_heartbeat(n: usize, t: f32, accent: &str) -> Vec<u8> {
    let beat = t.fract();
    let gain = if beat < 0.12 {
        1.0
    } else if beat < 0.2 {
        0.35
    } else if beat < 0.32 {
        0.85
    } else {
        0.15 + 0.1 * (beat * TAU * 2.0).sin().max(0.0)
    };
    let mut rgb = vec![0u8; n * 3];
    for i in 0..n {
        set_pixel(&mut rgb, i, resolve_pixel_color(i, n, accent), gain);
    }
    rgb
}

fn render_segment(n: usize, t: f32, accent: &str) -> Vec<u8> {
    let mut rgb = vec![0u8; n * 3];
    let bounds = segment_bounds(n);
    let active = (t.floor() as usize) % 3;
    let (start, end) = bounds[active];
    for i in start..end.min(n) {
        set_pixel(&mut rgb, i, resolve_pixel_color(i, n, accent), 1.0);
    }
    rgb
}

fn render_strobe(n: usize, t: f32, accent: &str) -> Vec<u8> {
    let on = (t * 4.0).fract() < 0.5;
    let gain = if on { 1.0 } else { 0.0 };
    let mut rgb = vec![0u8; n * 3];
    for i in 0..n {
        set_pixel(&mut rgb, i, resolve_pixel_color(i, n, accent), gain);
    }
    rgb
}

fn render_wipe(n: usize, t: f32, accent: &str) -> Vec<u8> {
    let mut rgb = vec![0u8; n * 3];
    let progress = t.fract();
    let fill = (progress * n as f32).ceil() as usize;
    for i in 0..fill.min(n) {
        set_pixel(&mut rgb, i, resolve_pixel_color(i, n, accent), 1.0);
    }
    rgb
}

fn rand_unit(seed: usize, t: f32) -> f32 {
    let x = (seed as u32).wrapping_mul(374761393) ^ (t.to_bits());
    let x = x.wrapping_mul(668265263);
    (x as f32 / u32::MAX as f32).clamp(0.0, 1.0)
}
