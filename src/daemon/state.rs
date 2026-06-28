use serde::{Deserialize, Serialize};

use crate::config::RuntimeConfig;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DaemonStatus {
    pub running: bool,
    pub effect: String,
    pub brightness: f32,
    pub fps: u32,
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
}

fn default_reactivity() -> f32 {
    0.3
}

fn default_sensitivity() -> f32 {
    0.3
}

impl DaemonStatus {
    pub fn from_config(cfg: &RuntimeConfig) -> Self {
        Self {
            running: true,
            effect: cfg.effect.mode.as_str().to_string(),
            brightness: cfg.effect.brightness,
            fps: cfg.effect.fps,
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
        }
    }
}
