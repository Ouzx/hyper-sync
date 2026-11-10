use std::f32::consts::TAU;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use crate::config::{DeviceConfig, EffectMode, RuntimeConfig};
use crate::daemon::DaemonStatus;
use crate::effects::solid::resolve_pixel_color;
use crate::effects::{noise, Ctx};
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
        #[cfg(feature = "audio")]
        Arc::new(crate::audio::AudioSnapshot::new()),
    )
}

pub fn render(c: &mut Ctx) -> Vec<u8> {
    let warmth = c.cfg.candle.warmth.clamp(0.0, 1.0);
    let breathe = 0.85 + 0.15 * (c.t * 0.3 * TAU).sin();
    (0..c.n)
        .flat_map(|i| {
            let accent = resolve_pixel_color(i, c.n, &c.cfg.solid.color);
            let gain = breathe * (0.78 + 0.22 * noise(i as u32, c.t * 7.0));
            std::array::from_fn::<u8, 3, _>(|k| {
                ((f32::from(accent[k]) * (1.0 - warmth) + AMBER[k] * warmth) * gain) as u8
            })
        })
        .collect()
}
