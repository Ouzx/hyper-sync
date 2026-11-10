use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::config::{runtime_config_path, EffectMode, RuntimeConfig, SoundMode};
#[cfg(feature = "screen")]
use crate::config::ensure_layout_file;
use super::ReloadMsg;
use crate::daemon::state::DaemonStatus;
use crate::config::ipc_socket_path;
use crate::effects::solid::{is_rainbow_color, parse_color};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum IpcRequest {
    Status,
    Stop,
    Restart,
    Quit,
    /// Clear saved portal token; re-prompt on next screen capture start.
    ReselectScreen,
    /// Blink a color over whatever is running (notifications, build results).
    Flash { color: String, count: u32 },
    Patch(Patch),
}

/// Settings to change; anything left `None` stays as it is.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Patch {
    pub mode: Option<String>,
    pub brightness: Option<f32>,
    pub color: Option<String>,
    pub fps: Option<u32>,
    pub speed: Option<f32>,
    pub sound_mode: Option<String>,
    pub reactivity: Option<f32>,
    pub sensitivity: Option<f32>,
    pub warmth: Option<f32>,
    pub gamma: Option<f32>,
    pub power: Option<f32>,
    /// Screen sync smoothing.
    pub smoothing: Option<f32>,
    pub audio_smoothing: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IpcResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<DaemonStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn cleanup_stale_socket() {
    let path = ipc_socket_path();
    if path.exists() && !daemon_running() {
        let _ = std::fs::remove_file(&path);
    }
}

pub fn daemon_running() -> bool {
    ipc_request(&IpcRequest::Status).map(|r| r.ok).unwrap_or(false)
}

pub fn ipc_request(req: &IpcRequest) -> anyhow::Result<IpcResponse> {
    let mut stream = UnixStream::connect(ipc_socket_path())
        .context("connect to hyper-sync daemon (is it running?)")?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    let line = serde_json::to_string(req)?;
    writeln!(stream, "{line}")?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    reader.read_line(&mut buf)?;
    let resp: IpcResponse = serde_json::from_str(buf.trim()).context("parse daemon response")?;
    anyhow::ensure!(resp.ok, "daemon: {}", resp.error.as_deref().unwrap_or("request failed"));
    Ok(resp)
}

pub fn patch_config(patch: IpcRequest) -> anyhow::Result<IpcResponse> {
    ipc_request(&patch)
}

pub fn run_server(
    config: Arc<RwLock<RuntimeConfig>>,
    config_path: Arc<PathBuf>,
    status: Arc<Mutex<DaemonStatus>>,
    reload_tx: Sender<ReloadMsg>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<()> {
    let socket_path = ipc_socket_path();
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("bind {}", socket_path.display()))?;
    eprintln!("hyper-sync ipc listening on {}", socket_path.display());

    // Blocking accept: requests are answered at once (polling every 50 ms made a held TUI key
    // queue up faster than it drained). On shutdown the process exits, ending this thread.
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let config = Arc::clone(&config);
                let config_path = Arc::clone(&config_path);
                let status = Arc::clone(&status);
                let reload_tx = reload_tx.clone();
                let shutdown = Arc::clone(&shutdown);
                thread::spawn(move || {
                    let reply = stream.try_clone();
                    if let Err(e) = handle_client(stream, config, config_path, status, reload_tx, shutdown) {
                        let resp = IpcResponse { ok: false, status: None, error: Some(format!("{e:#}")) };
                        if let (Ok(mut s), Ok(line)) = (reply, serde_json::to_string(&resp)) {
                            let _ = writeln!(s, "{line}");
                        }
                    }
                });
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn handle_client(
    stream: UnixStream,
    config: Arc<RwLock<RuntimeConfig>>,
    config_path: Arc<PathBuf>,
    status: Arc<Mutex<DaemonStatus>>,
    reload_tx: Sender<ReloadMsg>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let req: IpcRequest = serde_json::from_str(line.trim()).context("parse ipc request")?;
    let resp = match req {
        IpcRequest::Status => IpcResponse {
            ok: true,
            status: Some(synced_status(&config, &status)),
            error: None,
        },
        IpcRequest::Stop => {
            {
                let mut cfg = config.write().unwrap();
                cfg.effect.mode = crate::config::EffectMode::Off;
            }
            save_and_notify(&config, &config_path, &reload_tx, ReloadMsg::Force)?;
            IpcResponse {
                ok: true,
                status: Some(status.lock().unwrap().clone()),
                error: None,
            }
        }
        IpcRequest::Restart => {
            reload_tx.send(ReloadMsg::Force)?;
            IpcResponse {
                ok: true,
                status: Some(status.lock().unwrap().clone()),
                error: None,
            }
        }
        IpcRequest::ReselectScreen => {
            reload_tx.send(ReloadMsg::ReselectScreen)?;
            IpcResponse {
                ok: true,
                status: Some(status.lock().unwrap().clone()),
                error: None,
            }
        }
        IpcRequest::Flash { color, count } => {
            crate::effects::flash(parse_color(&color)?, count);
            IpcResponse { ok: true, status: None, error: None }
        }
        IpcRequest::Quit => {
            shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
            IpcResponse {
                ok: true,
                status: Some(status.lock().unwrap().clone()),
                error: None,
            }
        }
        IpcRequest::Patch(Patch {
            mode,
            brightness,
            color,
            fps,
            speed,
            sound_mode,
            reactivity,
            sensitivity,
            warmth,
            gamma,
            power,
            smoothing,
            audio_smoothing,
        }) => {
            if let Some(c) = &color {
                let c = c.trim_start_matches('#');
                anyhow::ensure!(
                    is_rainbow_color(c) || parse_color(c).is_ok(),
                    "invalid color {c:?}: use 6 hex digits (ff3300) or \"rainbow\""
                );
            }
            let mode_change = mode.is_some();
            let old_mode = config.read().unwrap().effect.mode;
            {
                let mut cfg = config.write().unwrap();
                if let Some(m) = mode {
                    if m == "rainbow" {
                        cfg.effect.mode = EffectMode::Solid;
                        cfg.solid.color = "rainbow".into();
                    } else {
                        cfg.effect.mode = parse_mode(&m)?;
                    }
                }
                if let Some(b) = brightness {
                    cfg.effect.brightness = b.clamp(0.0, 1.0);
                }
                if let Some(c) = color {
                    cfg.solid.color = c.trim_start_matches('#').to_string();
                }
                if let Some(f) = fps {
                    cfg.effect.fps = f.clamp(1, crate::config::MAX_FPS);
                }
                if let Some(s) = speed {
                    cfg.effect.speed = s.clamp(0.1, 5.0);
                }
                if let Some(sm) = sound_mode {
                    cfg.audio.sound_mode = parse_sound_mode(&sm)?;
                }
                if let Some(r) = reactivity {
                    cfg.audio.reactivity = r.clamp(0.0, 1.0);
                }
                if let Some(s) = sensitivity {
                    cfg.audio.sensitivity = s.clamp(0.0, 1.0);
                }
                if let Some(w) = warmth {
                    cfg.candle.warmth = w.clamp(0.0, 1.0);
                }
                if let Some(p) = power {
                    cfg.effect.power = p.clamp(0.0, 1.0);
                }
                if let Some(s) = audio_smoothing {
                    cfg.audio.smoothing = s.clamp(0.0, 1.0);
                }
                #[cfg(feature = "screen")]
                {
                    if let Some(g) = gamma {
                        cfg.screen.gamma = g.clamp(1.0, 3.0);
                    }
                    if let Some(s) = smoothing {
                        cfg.screen.smoothing = s.clamp(0.0, 1.0);
                    }
                }
                #[cfg(not(feature = "screen"))]
                let _ = (gamma, smoothing);
            }
            let new_mode = config.read().unwrap().effect.mode;
            if mode_change && old_mode != new_mode {
                eprintln!("ipc: mode {} -> {}", old_mode.as_str(), new_mode.as_str());
                if old_mode.is_screen() && new_mode.is_screen() {
                    save_config(&config, &config_path)?;
                } else {
                    save_and_notify(&config, &config_path, &reload_tx, ReloadMsg::Force)?;
                }
            } else {
                save_config(&config, &config_path)?;
            }
            IpcResponse {
                ok: true,
                status: Some(synced_status(&config, &status)),
                error: None,
            }
        }
    };
    let mut stream = reader.into_inner();
    writeln!(stream, "{}", serde_json::to_string(&resp)?)?;
    Ok(())
}

/// Status with the settings fields taken from the live config.
fn synced_status(config: &Arc<RwLock<RuntimeConfig>>, status: &Arc<Mutex<DaemonStatus>>) -> DaemonStatus {
    let cfg = config.read().unwrap();
    let mut st = status.lock().unwrap();
    st.effect = cfg.effect.mode.as_str().to_string();
    st.brightness = cfg.effect.brightness;
    st.fps = cfg.effect.fps;
    st.speed = cfg.effect.speed;
    st.color = cfg.solid.color.clone();
    st.sound_mode = cfg.audio.sound_mode.as_str().to_string();
    st.reactivity = cfg.audio.reactivity;
    st.sensitivity = cfg.audio.sensitivity;
    st.warmth = cfg.candle.warmth;
    st.power = cfg.effect.power;
    st.audio_smoothing = cfg.audio.smoothing;
    #[cfg(feature = "screen")]
    {
        st.gamma = cfg.screen.gamma;
        st.screen_smoothing = cfg.screen.smoothing;
    }
    st.clone()
}

fn save_config(config: &Arc<RwLock<RuntimeConfig>>, path: &Path) -> anyhow::Result<()> {
    config.read().unwrap().save(path)
}

fn save_and_notify(
    config: &Arc<RwLock<RuntimeConfig>>,
    path: &Path,
    reload_tx: &Sender<ReloadMsg>,
    msg: ReloadMsg,
) -> anyhow::Result<()> {
    config.read().unwrap().save(path)?;
    reload_tx.send(msg)?;
    Ok(())
}

fn parse_mode(s: &str) -> anyhow::Result<EffectMode> {
    // Same names (and aliases for removed effects) as config.toml.
    serde_json::from_value(serde_json::Value::String(s.into())).with_context(|| format!("unknown mode {s}"))
}

fn parse_sound_mode(s: &str) -> anyhow::Result<SoundMode> {
    Ok(match s {
        "off" => SoundMode::Off,
        "level" => SoundMode::Level,
        "balance" => SoundMode::Balance,
        other => anyhow::bail!("unknown sound_mode {other}"),
    })
}

pub fn write_default_config_if_missing() -> anyhow::Result<PathBuf> {
    let path = runtime_config_path();
    if !path.exists() {
        RuntimeConfig::default().save(&path)?;
    }
    #[cfg(feature = "screen")]
    ensure_layout_file()?;
    Ok(path)
}
