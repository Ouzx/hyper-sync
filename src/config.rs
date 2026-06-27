use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

pub const DEFAULT_PORT: &str = "/dev/ttyUSB0";
pub const DEFAULT_BAUD: u32 = 115_200;
pub const DEFAULT_LEDS: u8 = 65;
pub const DEFAULT_FPS: u32 = 30;

#[derive(Debug, Clone)]
pub struct DeviceConfig {
    pub port: String,
    pub baud: u32,
    pub leds: u8,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT.into(),
            baud: DEFAULT_BAUD,
            leds: DEFAULT_LEDS,
        }
    }
}

#[cfg(feature = "screen")]
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct LayoutConfig {
    pub led_count: u8,
    #[serde(default = "default_origin")]
    pub origin: String,
    #[serde(default = "default_direction")]
    pub direction: String,
    pub segments: Vec<Segment>,
}

#[cfg(feature = "screen")]
fn default_origin() -> String {
    "bottom_right".into()
}

#[cfg(feature = "screen")]
fn default_direction() -> String {
    "counter_clockwise".into()
}

#[cfg(feature = "screen")]
pub fn resolve_layout_path(path: &Path) -> PathBuf {
    if path.is_absolute() && path.exists() {
        return path.to_path_buf();
    }
    if path.exists() {
        return path.to_path_buf();
    }
    if let Ok(cwd) = std::env::current_dir() {
        let from_cwd = cwd.join(path);
        if from_cwd.exists() {
            return from_cwd;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let from_exe = dir.join(path);
            if from_exe.exists() {
                return from_exe;
            }
            let from_project = dir.join("config/layout.toml");
            if from_project.exists() {
                return from_project;
            }
        }
    }
    let from_config = dirs_config().join("layout.toml");
    if from_config.exists() {
        return from_config;
    }
    path.to_path_buf()
}

#[cfg(feature = "screen")]
pub fn config_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .unwrap_or_else(|_| PathBuf::from(".config"))
        })
        .join("hyper-sync")
}

#[cfg(feature = "screen")]
fn dirs_config() -> PathBuf {
    config_dir()
}

#[cfg(feature = "screen")]
pub fn portal_token_path() -> PathBuf {
    config_dir().join("restore-token")
}

#[cfg(feature = "screen")]
pub fn load_portal_token() -> Option<String> {
    let path = portal_token_path();
    let token = std::fs::read_to_string(&path).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

#[cfg(feature = "screen")]
pub fn save_portal_token(token: &str) -> anyhow::Result<()> {
    let path = portal_token_path();
    std::fs::create_dir_all(path.parent().unwrap())
        .with_context(|| format!("create {}", path.parent().unwrap().display()))?;
    std::fs::write(&path, token).with_context(|| format!("write {}", path.display()))
}

#[cfg(feature = "screen")]
pub fn clear_portal_token() -> anyhow::Result<()> {
    let path = portal_token_path();
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

#[cfg(feature = "screen")]
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Segment {
    pub name: String,
    pub count: u8,
    pub edge: String,
}

#[cfg(feature = "screen")]
#[derive(Debug, Clone)]
pub struct EdgeZone {
    pub cx: f32,
    pub cy: f32,
    pub edge: String,
}

#[cfg(feature = "screen")]
impl LayoutConfig {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("read layout {}", path.as_ref().display()))?;
        toml::from_str(&text).context("parse layout.toml")
    }

    /// Normalized center points per LED index (bottom-right origin, CCW).
    pub fn edge_zones(&self) -> Vec<EdgeZone> {
        let mut zones = Vec::with_capacity(usize::from(self.led_count));

        for seg in &self.segments {
            let n = usize::from(seg.count);
            for i in 0..n {
                let t = if n <= 1 {
                    0.5
                } else {
                    i as f32 / (n as f32 - 1.0)
                };
                let (cx, cy) = edge_point(&seg.edge, t);
                zones.push(EdgeZone {
                    cx,
                    cy,
                    edge: seg.edge.clone(),
                });
            }
        }

        zones.truncate(usize::from(self.led_count));
        zones
    }

    /// Normalized (x, y) sample points per LED index, origin bottom-right CCW.
    pub fn sample_points(&self) -> Vec<(f32, f32)> {
        let mut points = Vec::with_capacity(usize::from(self.led_count));
        let mut idx = 0u8;

        for seg in &self.segments {
            let n = usize::from(seg.count);
            for i in 0..n {
                let t = if n <= 1 {
                    0.5
                } else {
                    i as f32 / (n as f32 - 1.0)
                };
                points.push(edge_point(&seg.edge, t));
                idx += 1;
                let _ = idx;
            }
        }

        points.truncate(usize::from(self.led_count));
        points
    }
}

#[cfg(feature = "screen")]
fn edge_point(edge: &str, t: f32) -> (f32, f32) {
    match edge {
        "right" => (1.0, 1.0 - t),
        "top" => (1.0 - t, 0.0),
        "left" => (0.0, t),
        "bottom" => (t, 1.0),
        _ => (0.5, 0.5),
    }
}
