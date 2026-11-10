use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::RuntimeConfig;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DaemonStatus {
    pub running: bool,
    pub effect: String,
    pub brightness: f32,
    pub fps: u32,
    /// Measured LED frames/sec over the last ~1s window.
    #[serde(default)]
    pub actual_fps: f32,
    pub speed: f32,
    pub serial_ok: bool,
    pub detail: String,
    pub color: String,
    pub width: u32,
    pub height: u32,
    pub last_error: Option<String>,
    #[serde(default)]
    pub sound_mode: String,
    #[serde(default)]
    pub audio_level: f32,
    #[serde(default = "default_reactivity")]
    pub reactivity: f32,
    #[serde(default = "default_sensitivity")]
    pub sensitivity: f32,
    #[serde(default)]
    pub warmth: f32,
    #[serde(default)]
    pub gamma: f32,
    /// What the LEDs show right now: hex RGB, one triple per LED.
    #[serde(default)]
    pub leds: String,
    #[serde(default)]
    pub spectrum: Vec<f32>,
}

fn default_reactivity() -> f32 {
    0.3
}

fn default_sensitivity() -> f32 {
    0.3
}

/// Rolling ~1s frame-rate meter. Call [`tick`] after each successful LED frame.
pub struct FpsMeter {
    window_start: Instant,
    count: u32,
    rate: f32,
}

impl Default for FpsMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl FpsMeter {
    pub fn new() -> Self {
        Self {
            window_start: Instant::now(),
            count: 0,
            rate: 0.0,
        }
    }

    pub fn tick(&mut self) -> f32 {
        self.count += 1;
        let elapsed = self.window_start.elapsed();
        if elapsed >= Duration::from_millis(500) {
            self.rate = self.count as f32 / elapsed.as_secs_f32();
            self.count = 0;
            self.window_start = Instant::now();
        }
        self.rate
    }
}

impl DaemonStatus {
    pub fn from_config(cfg: &RuntimeConfig) -> Self {
        Self {
            running: true,
            effect: cfg.effect.mode.as_str().to_string(),
            brightness: cfg.effect.brightness,
            fps: cfg.effect.fps,
            actual_fps: 0.0,
            speed: cfg.effect.speed,
            serial_ok: true,
            detail: String::new(),
            color: cfg.solid.color.clone(),
            width: 0,
            height: 0,
            last_error: None,
            sound_mode: cfg.audio.sound_mode.as_str().to_string(),
            audio_level: 0.0,
            reactivity: cfg.audio.reactivity,
            sensitivity: cfg.audio.sensitivity,
            warmth: cfg.candle.warmth,
            #[cfg(feature = "screen")]
            gamma: cfg.screen.gamma,
            #[cfg(not(feature = "screen"))]
            gamma: 1.0,
            leds: String::new(),
            spectrum: Vec::new(),
        }
    }
}
