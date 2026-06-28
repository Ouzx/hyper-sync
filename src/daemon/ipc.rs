use std::io::{BufRead, BufReader, Write};
use std::io::ErrorKind;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::config::{runtime_config_path, EffectMode, RuntimeConfig, SoundMode};
use super::ReloadMsg;
use crate::daemon::state::DaemonStatus;
use crate::config::ipc_socket_path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum IpcRequest {
    Status,
    Stop,
    Restart,
    Quit,
    /// Clear saved portal token; re-prompt on next screen capture start.
    ReselectScreen,
    Set { config: RuntimeConfig },
    Patch {
        mode: Option<String>,
        brightness: Option<f32>,
        color: Option<String>,
        fps: Option<u32>,
        speed: Option<f32>,
        sound_mode: Option<String>,
        reactivity: Option<f32>,
        sensitivity: Option<f32>,
    },
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
    serde_json::from_str(buf.trim()).context("parse daemon response")
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
    listener
        .set_nonblocking(true)
        .context("ipc nonblocking")?;
    eprintln!("hyper-sync ipc listening on {}", socket_path.display());

    loop {
        if shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let config = Arc::clone(&config);
                let config_path = Arc::clone(&config_path);
                let status = Arc::clone(&status);
                let reload_tx = reload_tx.clone();
                let shutdown = Arc::clone(&shutdown);
                thread::spawn(move || {
                    let _ = handle_client(stream, config, config_path, status, reload_tx, shutdown);
                });
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
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
            status: Some({
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
                st.clone()
            }),
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
        IpcRequest::Quit => {
            shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
            IpcResponse {
                ok: true,
                status: Some(status.lock().unwrap().clone()),
                error: None,
            }
        }
        IpcRequest::Set { config: new_cfg } => {
            *config.write().unwrap() = new_cfg;
            save_and_notify(&config, &config_path, &reload_tx, ReloadMsg::Force)?;
            IpcResponse {
                ok: true,
                status: Some(status.lock().unwrap().clone()),
                error: None,
            }
        }
        IpcRequest::Patch {
            mode,
            brightness,
            color,
            fps,
            speed,
            sound_mode,
            reactivity,
            sensitivity,
        } => {
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
                    cfg.effect.fps = f.max(1);
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
            {
                let cfg = config.read().unwrap();
                let mut st = status.lock().unwrap();
                st.brightness = cfg.effect.brightness;
                st.fps = cfg.effect.fps;
                st.speed = cfg.effect.speed;
                st.color = cfg.solid.color.clone();
                st.effect = cfg.effect.mode.as_str().to_string();
                st.sound_mode = cfg.audio.sound_mode.as_str().to_string();
                st.reactivity = cfg.audio.reactivity;
                st.sensitivity = cfg.audio.sensitivity;
            }
            IpcResponse {
                ok: true,
                status: Some(status.lock().unwrap().clone()),
                error: None,
            }
        }
    };
    let mut stream = reader.into_inner();
    writeln!(stream, "{}", serde_json::to_string(&resp)?)?;
    Ok(())
}

fn save_config(config: &Arc<RwLock<RuntimeConfig>>, path: &Path) -> anyhow::Result<()> {
    config.read().unwrap().save(path)?;
    super::bump_config_save_gen();
    Ok(())
}

fn save_and_notify(
    config: &Arc<RwLock<RuntimeConfig>>,
    path: &Path,
    reload_tx: &Sender<ReloadMsg>,
    msg: ReloadMsg,
) -> anyhow::Result<()> {
    config.read().unwrap().save(path)?;
    super::bump_config_save_gen();
    reload_tx.send(msg)?;
    Ok(())
}

fn parse_mode(s: &str) -> anyhow::Result<crate::config::EffectMode> {
    use crate::config::EffectMode;
    Ok(match s {
        "off" => EffectMode::Off,
        "solid" => EffectMode::Solid,
        "candle" => EffectMode::Candle,
        "chase" => EffectMode::Chase,
        "wave" => EffectMode::Wave,
        "scanner" => EffectMode::Scanner,
        "sparkle" => EffectMode::Sparkle,
        "pulse" => EffectMode::Pulse,
        "aurora" => EffectMode::Aurora,
        "fire" => EffectMode::Fire,
        "heartbeat" => EffectMode::Heartbeat,
        "segment" => EffectMode::Segment,
        "strobe" => EffectMode::Strobe,
        "wipe" => EffectMode::Wipe,
        "sound_viz" => EffectMode::SoundViz,
        "screen" => EffectMode::Screen,
        "screen_center" => EffectMode::ScreenCenter,
        other => anyhow::bail!("unknown mode {other}"),
    })
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
    Ok(path)
}
