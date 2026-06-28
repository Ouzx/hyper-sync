use serde::{Deserialize, Serialize};

use crate::config::RuntimeConfig;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DaemonStatus {
    pub running: bool,
    pub effect: String,
    pub brightness: f32,
    pub fps: u32,
    pub serial_ok: bool,
    pub detail: String,
    pub color: String,
    pub width: u32,
    pub height: u32,
    pub last_error: Option<String>,
}

impl DaemonStatus {
    pub fn from_config(cfg: &RuntimeConfig) -> Self {
        Self {
            running: true,
            effect: cfg.effect.mode.as_str().to_string(),
            brightness: cfg.effect.brightness,
            fps: cfg.effect.fps,
            serial_ok: true,
            detail: String::new(),
            color: cfg.solid.color.clone(),
            width: 0,
            height: 0,
            last_error: None,
        }
    }
}
