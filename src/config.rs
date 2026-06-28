use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: &str = "/dev/ttyUSB0";
pub const DEFAULT_BAUD: u32 = 115_200;
pub const DEFAULT_LEDS: u8 = 65;
pub const DEFAULT_FPS: u32 = 30;

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

pub fn runtime_config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn ipc_socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("hyper-sync.sock");
    }
    let cache = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".cache"))
                .unwrap_or_else(|_| PathBuf::from(".cache"))
        });
    cache.join("hyper-sync.sock")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffectMode {
    Off,
    Solid,
    Candle,
    Chase,
    Wave,
    Rainbow,
    Scanner,
    Sparkle,
    Pulse,
    Aurora,
    Fire,
    Heartbeat,
    Segment,
    Strobe,
    Wipe,
    Screen,
    #[serde(rename = "screen_center")]
    ScreenCenter,
}

impl EffectMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Solid => "solid",
            Self::Candle => "candle",
            Self::Chase => "chase",
            Self::Wave => "wave",
            Self::Rainbow => "rainbow",
            Self::Scanner => "scanner",
            Self::Sparkle => "sparkle",
            Self::Pulse => "pulse",
            Self::Aurora => "aurora",
            Self::Fire => "fire",
            Self::Heartbeat => "heartbeat",
            Self::Segment => "segment",
            Self::Strobe => "strobe",
            Self::Wipe => "wipe",
            Self::Screen => "screen",
            Self::ScreenCenter => "screen_center",
        }
    }

    pub fn is_screen(self) -> bool {
        matches!(self, Self::Screen | Self::ScreenCenter)
    }
}

impl Default for EffectMode {
    fn default() -> Self {
        Self::Screen
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub device: DeviceSection,
    pub effect: EffectSection,
    pub solid: SolidSection,
    pub candle: CandleSection,
    #[cfg(feature = "screen")]
    pub screen: ScreenSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSection {
    #[serde(default = "default_port")]
    pub port: String,
    #[serde(default = "default_leds")]
    pub leds: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectSection {
    #[serde(default)]
    pub mode: EffectMode,
    #[serde(default = "default_brightness")]
    pub brightness: f32,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_speed")]
    pub speed: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolidSection {
    #[serde(default = "default_color")]
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandleSection {
    #[serde(default = "default_warmth")]
    pub warmth: f32,
}

#[cfg(feature = "screen")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenSection {
    #[serde(default)]
    pub monitor: u32,
    #[serde(default = "default_layout")]
    pub layout: String,
    #[serde(default)]
    pub forget_portal: bool,
}

fn default_port() -> String {
    DEFAULT_PORT.into()
}
fn default_leds() -> u8 {
    DEFAULT_LEDS
}
fn default_brightness() -> f32 {
    0.8
}
fn default_fps() -> u32 {
    DEFAULT_FPS
}
fn default_color() -> String {
    "ff3300".into()
}
fn default_warmth() -> f32 {
    0.9
}
fn default_speed() -> f32 {
    1.0
}
#[cfg(feature = "screen")]
fn default_layout() -> String {
    "config/layout.toml".into()
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            device: DeviceSection {
                port: default_port(),
                leds: default_leds(),
            },
            effect: EffectSection {
                mode: EffectMode::Screen,
                brightness: default_brightness(),
                fps: default_fps(),
                speed: default_speed(),
            },
            solid: SolidSection {
                color: default_color(),
            },
            candle: CandleSection {
                warmth: default_warmth(),
            },
            #[cfg(feature = "screen")]
            screen: ScreenSection {
                monitor: 0,
                layout: default_layout(),
                forget_portal: false,
            },
        }
    }
}

impl RuntimeConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        toml::from_str(&text).context("parse config.toml")
    }

    pub fn load_or_create_default() -> anyhow::Result<(Self, PathBuf)> {
        let path = runtime_config_path();
        if path.exists() {
            return Ok((Self::load(&path)?, path));
        }
        let cfg = Self::default();
        cfg.save(&path)?;
        Ok((cfg, path))
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serialize config")?;
        std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
    }

    pub fn device_config(&self) -> DeviceConfig {
        DeviceConfig {
            port: self.device.port.clone(),
            baud: DEFAULT_BAUD,
            leds: self.device.leds,
        }
    }

    pub fn effect_key(&self) -> String {
        match self.effect.mode {
            EffectMode::Off => "off".into(),
            EffectMode::Solid => "solid".into(),
            EffectMode::Candle => "candle".into(),
            EffectMode::Chase => "chase".into(),
            EffectMode::Wave => "wave".into(),
            EffectMode::Rainbow => "rainbow".into(),
            EffectMode::Scanner => "scanner".into(),
            EffectMode::Sparkle => "sparkle".into(),
            EffectMode::Pulse => "pulse".into(),
            EffectMode::Aurora => "aurora".into(),
            EffectMode::Fire => "fire".into(),
            EffectMode::Heartbeat => "heartbeat".into(),
            EffectMode::Segment => "segment".into(),
            EffectMode::Strobe => "strobe".into(),
            EffectMode::Wipe => "wipe".into(),
            #[cfg(feature = "screen")]
            EffectMode::Screen => format!("screen:{}:{}", self.screen.monitor, self.screen.layout),
            #[cfg(feature = "screen")]
            EffectMode::ScreenCenter => {
                format!("screen_center:{}:{}", self.screen.monitor, self.screen.layout)
            }
            #[cfg(not(feature = "screen"))]
            EffectMode::Screen => "screen".into(),
            #[cfg(not(feature = "screen"))]
            EffectMode::ScreenCenter => "screen_center".into(),
        }
    }
}

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
    let from_config = config_dir().join("layout.toml");
    if from_config.exists() {
        return from_config;
    }
    path.to_path_buf()
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
