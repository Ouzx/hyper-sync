use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use crate::config::{EffectMode, RuntimeConfig};
use crate::daemon::DaemonStatus;
use crate::config::DeviceConfig;
use crate::protocol::build_frame;
use crate::serial::SerialWriter;

pub fn parse_color(hex: &str) -> anyhow::Result<[u8; 3]> {
    let s = hex.trim_start_matches('#');
    anyhow::ensure!(s.len() == 6, "color must be 6 hex digits, got {s}");
    Ok([
        u8::from_str_radix(&s[0..2], 16)?,
        u8::from_str_radix(&s[2..4], 16)?,
        u8::from_str_radix(&s[4..6], 16)?,
    ])
}

pub fn is_rainbow_color(s: &str) -> bool {
    s.eq_ignore_ascii_case("rainbow")
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.fract().max(0.0);
    let i = (h * 6.0).floor() as i32;
    let f = h * 6.0 - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [
        (r * 255.0) as u8,
        (g * 255.0) as u8,
        (b * 255.0) as u8,
    ]
}

/// Fixed hue per LED index along the strip (not animated).
pub fn rainbow_pixel(i: usize, n: usize) -> [u8; 3] {
    let hue = i as f32 / n.max(1) as f32;
    hsv_to_rgb(hue, 1.0, 1.0)
}

pub fn resolve_pixel_color(i: usize, n: usize, accent: &str) -> [u8; 3] {
    if is_rainbow_color(accent) {
        rainbow_pixel(i, n)
    } else {
        parse_color(accent).unwrap_or([255, 51, 0])
    }
}

pub fn scale_rgb(rgb: [u8; 3], brightness: f32) -> [u8; 3] {
    let b = brightness.clamp(0.0, 1.0);
    [
        (f32::from(rgb[0]) * b) as u8,
        (f32::from(rgb[1]) * b) as u8,
        (f32::from(rgb[2]) * b) as u8,
    ]
}

pub fn scale_rgb_buf(rgb: &mut [u8], brightness: f32) {
    let b = brightness.clamp(0.0, 1.0);
    for c in rgb.iter_mut() {
        *c = (f32::from(*c) * b) as u8;
    }
}

pub fn run(
    cfg: DeviceConfig,
    color: [u8; 3],
    brightness: f32,
    fps: u32,
) -> anyhow::Result<()> {
    let scaled = scale_rgb(color, brightness);
    let n = usize::from(cfg.leds);
    let rgb: Vec<u8> = scaled
        .iter()
        .cycle()
        .take(n * 3)
        .copied()
        .collect();

    let frame = build_frame(cfg.leds, &rgb)?;
    let mut writer = SerialWriter::new(cfg);
    let interval = std::time::Duration::from_micros(1_000_000 / u64::from(fps.max(1)));

    loop {
        writer.write_frame(&frame)?;
        std::thread::sleep(interval);
    }
}

pub fn run_off(cfg: DeviceConfig) -> anyhow::Result<()> {
    let frame = crate::protocol::black_frame(cfg.leds);
    let mut writer = SerialWriter::new(cfg);
    writer.write_frame(&frame)?;
    Ok(())
}

pub fn run_controlled(
    writer: Arc<Mutex<SerialWriter>>,
    config: Arc<RwLock<RuntimeConfig>>,
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<DaemonStatus>>,
    _preview: Arc<Mutex<Vec<u8>>>,
) -> anyhow::Result<()> {
    while !cancel.load(Ordering::Relaxed)
        && config.read().unwrap().effect.mode == EffectMode::Solid
    {
        let cfg = config.read().unwrap().clone();
        let interval = Duration::from_micros(1_000_000 / u64::from(cfg.effect.fps.max(1)));
        let n = usize::from(cfg.device.leds);
        let rgb: Vec<u8> = if is_rainbow_color(&cfg.solid.color) {
            (0..n)
                .flat_map(|i| {
                    scale_rgb(rainbow_pixel(i, n), cfg.effect.brightness).into_iter()
                })
                .collect()
        } else {
            let color = parse_color(&cfg.solid.color)?;
            let scaled = scale_rgb(color, cfg.effect.brightness);
            scaled.iter().cycle().take(n * 3).copied().collect()
        };
        let frame = build_frame(cfg.device.leds, &rgb)?;
        writer.lock().unwrap().write_frame(&frame)?;
        {
            let mut st = status.lock().unwrap();
            st.brightness = cfg.effect.brightness;
            st.fps = cfg.effect.fps;
            st.serial_ok = true;
            st.detail = if is_rainbow_color(&cfg.solid.color) {
                "rainbow".into()
            } else {
                format!("#{}", cfg.solid.color)
            };
            st.color = cfg.solid.color.clone();
        }
        thread::sleep(interval);
    }
    Ok(())
}
