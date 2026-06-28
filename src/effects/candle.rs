use std::f32::consts::TAU;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{EffectMode, RuntimeConfig};
use crate::daemon::DaemonStatus;
use crate::config::DeviceConfig;
use crate::effects::solid::{is_rainbow_color, resolve_pixel_color, scale_rgb};
use crate::protocol::build_frame;
use crate::serial::SerialWriter;

pub fn run(cfg: DeviceConfig, warmth: f32, speed: f32, fps: u32) -> anyhow::Result<()> {
    let n = usize::from(cfg.leds);
    let mut flicker: Vec<f32> = vec![1.0; n];
    let mut writer = SerialWriter::new(cfg.clone());
    let interval = std::time::Duration::from_micros(1_000_000 / u64::from(fps.max(1)));
    let start = Instant::now();
    let warmth = warmth.clamp(0.0, 1.0);
    let speed = speed.max(0.1);

    // warm orange base, shifted toward white as warmth drops
    let base = [
        255.0,
        80.0 + 120.0 * warmth,
        10.0 + 40.0 * (1.0 - warmth),
    ];

    loop {
        let t = start.elapsed().as_secs_f32() * speed;
        let breathe = 0.85 + 0.15 * (t * 0.3 * TAU).sin();

        let mut rgb = Vec::with_capacity(n * 3);
        for i in 0..n {
            // low-pass flicker per LED
            let target = 0.88 + rand_unit(i, t) * 0.24;
            flicker[i] = flicker[i] * 0.82 + target * 0.18;
            let gain = breathe * flicker[i];

            rgb.push((base[0] * gain).min(255.0) as u8);
            rgb.push((base[1] * gain).min(255.0) as u8);
            rgb.push((base[2] * gain).min(255.0) as u8);
        }

        let frame = build_frame(cfg.leds, &rgb)?;
        writer.write_frame(&frame)?;
        std::thread::sleep(interval);
    }
}

pub fn run_controlled(
    writer: Arc<Mutex<SerialWriter>>,
    config: Arc<RwLock<RuntimeConfig>>,
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<DaemonStatus>>,
    preview: Arc<Mutex<Vec<u8>>>,
) -> anyhow::Result<()> {
    let n = config.read().unwrap().device.leds as usize;
    let mut flicker: Vec<f32> = vec![1.0; n];
    let start = Instant::now();

    while !cancel.load(Ordering::Relaxed)
        && config.read().unwrap().effect.mode == EffectMode::Candle
    {
        let cfg = config.read().unwrap().clone();
        let interval = Duration::from_micros(1_000_000 / u64::from(cfg.effect.fps.max(1)));
        let warmth = cfg.candle.warmth.clamp(0.0, 1.0);
        let speed = cfg.effect.speed.max(0.1);
        let accent = cfg.solid.color.as_str();
        let warmth_gain = 0.35 + 0.65 * warmth;
        let t = start.elapsed().as_secs_f32() * speed;
        let breathe = 0.85 + 0.15 * (t * 0.3 * TAU).sin();
        let mut rgb = Vec::with_capacity(n * 3);
        for i in 0..n {
            let pixel = resolve_pixel_color(i, n, accent);
            let warm = if is_rainbow_color(accent) {
                pixel
            } else {
                scale_rgb(pixel, warmth_gain)
            };
            let base = [warm[0] as f32, warm[1] as f32, warm[2] as f32];
            let target = 0.88 + rand_unit(i, t) * 0.24;
            flicker[i] = flicker[i] * 0.82 + target * 0.18;
            let gain = breathe * flicker[i] * cfg.effect.brightness;
            rgb.push((base[0] * gain).min(255.0) as u8);
            rgb.push((base[1] * gain).min(255.0) as u8);
            rgb.push((base[2] * gain).min(255.0) as u8);
        }
        let frame = build_frame(cfg.device.leds, &rgb)?;
        writer.lock().unwrap().write_frame(&frame)?;
        *preview.lock().unwrap() = rgb.clone();
        {
            let mut st = status.lock().unwrap();
            st.brightness = cfg.effect.brightness;
            st.fps = cfg.effect.fps;
            st.speed = cfg.effect.speed;
            st.serial_ok = true;
            st.detail = "candle".into();
            st.color = cfg.solid.color.clone();
        }
        thread::sleep(interval);
    }
    Ok(())
}

// ponytail: deterministic pseudo-random — no rand crate for one effect
fn rand_unit(seed: usize, t: f32) -> f32 {
    let x = (seed as u32).wrapping_mul(374761393) ^ (t.to_bits());
    let x = x.wrapping_mul(668265263);
    (x as f32 / u32::MAX as f32).clamp(0.0, 1.0)
}
