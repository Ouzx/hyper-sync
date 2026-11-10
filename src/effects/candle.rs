use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use crate::config::{DeviceConfig, EffectMode, RuntimeConfig};
use crate::daemon::DaemonStatus;
use crate::effects::solid::resolve_pixel_color;
use crate::effects::{noise, swing, Ctx};
use crate::serial::SerialWriter;

/// Candle-flame amber; `warmth` blends the accent color toward it.
const AMBER: [f32; 3] = [255.0, 147.0, 41.0];

/// Standalone `hyper-sync candle` (no daemon).
pub fn run(cfg: DeviceConfig, warmth: f32, speed: f32, fps: u32) -> anyhow::Result<()> {
    let mut rc = RuntimeConfig::default();
    rc.device.port = cfg.port.clone();
    rc.device.leds = cfg.leds;
    rc.effect.mode = EffectMode::Candle;
    rc.effect.brightness = 1.0;
    rc.effect.fps = fps;
    rc.effect.speed = speed;
    rc.solid.color = "ff5032".into();
    rc.candle.warmth = warmth;
    super::run(
        EffectMode::Candle,
        Arc::new(Mutex::new(SerialWriter::new(cfg))),
        Arc::new(RwLock::new(rc)),
        Arc::new(AtomicBool::new(false)),
        Arc::new(Mutex::new(DaemonStatus::default())),
        Arc::new(crate::audio::AudioSnapshot::new()),
    )
}

/// One flame lights the room, so the flicker is shared by every LED (with a little local
/// variation); now and then a gust makes it dip and redden for a moment.
pub fn render(c: &mut Ctx) -> Vec<u8> {
    let (t, warmth) = (c.t, c.cfg.candle.warmth.clamp(0.0, 1.0));
    let gust = ((noise(9001, t * 0.6) - 0.55) / 0.45).max(0.0);
    let flame = 0.55 * noise(9002, t * 7.0) + 0.45 * noise(9003, t * 13.0);
    let depth = c.depth(0.55);
    (0..c.n)
        .flat_map(|i| {
            let accent = resolve_pixel_color(i, c.n, &c.cfg.solid.color);
            let wave = (0.8 * flame + 0.2 * noise(i as u32, t * 5.0)) * (1.0 - 0.6 * gust);
            let g = swing(wave, depth);
            let base: [f32; 3] = std::array::from_fn(|k| f32::from(accent[k]) * (1.0 - warmth) + AMBER[k] * warmth);
            // Dimmer moments burn redder, like a real flame.
            [base[0] * g, base[1] * g * (0.7 + 0.3 * g), base[2] * g * g].map(|v| v.min(255.0) as u8)
        })
        .collect()
}
