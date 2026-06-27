use std::f32::consts::TAU;
use std::time::Instant;

use crate::config::DeviceConfig;
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

// ponytail: deterministic pseudo-random — no rand crate for one effect
fn rand_unit(seed: usize, t: f32) -> f32 {
    let x = (seed as u32).wrapping_mul(374761393) ^ (t.to_bits());
    let x = x.wrapping_mul(668265263);
    (x as f32 / u32::MAX as f32).clamp(0.0, 1.0)
}
