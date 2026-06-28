mod ipc;
mod state;
mod supervisor;

pub use ipc::{cleanup_stale_socket, daemon_running, ipc_request, patch_config, write_default_config_if_missing, IpcRequest, IpcResponse};
pub use state::DaemonStatus;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::config::RuntimeConfig;

/// Ignore config-file watcher events briefly after we save (avoids stale read + double reload).
static CONFIG_SAVE_GEN: AtomicU64 = AtomicU64::new(0);

pub(crate) fn bump_config_save_gen() {
    CONFIG_SAVE_GEN.fetch_add(1, Ordering::SeqCst);
}

pub(crate) fn config_save_gen() -> u64 {
    CONFIG_SAVE_GEN.load(Ordering::SeqCst)
}

/// Config changed — restart effect only if `effect_key` changed.
pub(crate) enum ReloadMsg {
    Apply,
    /// Restart current effect even when mode unchanged (portal reconnect, etc.).
    Force,
    /// Clear portal token and re-acquire screen capture.
    ReselectScreen,
}

pub fn run(with_tray: bool) -> anyhow::Result<()> {
    eprintln!("hyper-sync daemon starting…");

    cleanup_stale_socket();
    if daemon_running() {
        anyhow::bail!("hyper-sync daemon already running (use `hyper-sync ctl quit` first)");
    }

    let (config, config_path) = RuntimeConfig::load_or_create_default()?;
    let config = Arc::new(RwLock::new(config));
    let config_path = Arc::new(config_path);
    let status = Arc::new(Mutex::new(DaemonStatus::from_config(&config.read().unwrap())));
    let shutdown = Arc::new(AtomicBool::new(false));

    let shutdown_signal = Arc::clone(&shutdown);
    ctrlc::set_handler(move || {
        shutdown_signal.store(true, Ordering::Relaxed);
    })?;

    let (reload_tx, reload_rx) = mpsc::channel::<ReloadMsg>();

    // IPC first — TUI/tray must connect even while an effect is starting or stopping.
    let ipc_cfg = Arc::clone(&config);
    let ipc_path = Arc::clone(&config_path);
    let ipc_status = Arc::clone(&status);
    let ipc_shutdown = Arc::clone(&shutdown);
    let ipc_reload = reload_tx.clone();
    thread::spawn(move || {
        if let Err(e) = ipc::run_server(ipc_cfg, ipc_path, ipc_status, ipc_reload, ipc_shutdown) {
            eprintln!("ipc server error: {e}");
        }
    });
    for _ in 0..50 {
        if crate::config::ipc_socket_path().exists() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    eprintln!("hyper-sync daemon ipc ready");

    let mut supervisor = supervisor::Supervisor::new(Arc::clone(&config), Arc::clone(&status));
    supervisor.reload();

    let cfg_path = Arc::clone(&config_path);
    let cfg_arc = Arc::clone(&config);
    let reload_tx_watch = reload_tx.clone();
    thread::spawn(move || watch_config(&cfg_path, cfg_arc, reload_tx_watch));

    #[cfg(feature = "tray")]
    if with_tray {
        let tray_cfg = Arc::clone(&config);
        thread::spawn(move || {
            if let Err(e) = crate::tray::run(tray_cfg) {
                eprintln!("tray error: {e}");
            }
        });
    }

    eprintln!(
        "hyper-sync daemon running (mode: {})",
        config.read().unwrap().effect.mode.as_str()
    );

    loop {
        let mut last_reload = None;
        while let Ok(msg) = reload_rx.try_recv() {
            last_reload = Some(msg);
        }
        if let Some(msg) = last_reload {
            match msg {
                ReloadMsg::Apply => supervisor.apply_config(),
                ReloadMsg::Force => supervisor.reload(),
                ReloadMsg::ReselectScreen => supervisor.reselect_screen(),
            }
        }
        supervisor.tick();
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    supervisor.stop_effect();
    supervisor.shutdown();
    let _ = std::fs::remove_file(crate::config::ipc_socket_path());
    eprintln!("hyper-sync daemon stopped");
    Ok(())
}

fn watch_config(path: &PathBuf, config: Arc<RwLock<RuntimeConfig>>, reload_tx: mpsc::Sender<ReloadMsg>) {
    let (tx, rx): (mpsc::Sender<notify::Result<notify::Event>>, Receiver<_>) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(tx, notify::Config::default()).expect("notify watcher");
    let _ = watcher.watch(path.as_path(), RecursiveMode::NonRecursive);
    let mut last_seen_save_gen = config_save_gen();
    while let Ok(Ok(event)) = rx.recv() {
        if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
            continue;
        }
        let seen_gen = config_save_gen();
        if seen_gen != last_seen_save_gen {
            last_seen_save_gen = seen_gen;
            thread::sleep(Duration::from_millis(150));
        }
        if let Ok(cfg) = RuntimeConfig::load(path) {
            let (mem_key, mem_mode) = {
                let current = config.read().unwrap();
                (current.effect_key(), current.effect.mode)
            };
            if cfg.effect_key() == mem_key && cfg.effect.mode == mem_mode {
                continue;
            }
            eprintln!(
                "config file changed externally: mode {} -> {}",
                mem_mode.as_str(),
                cfg.effect.mode.as_str()
            );
            let new_mode = cfg.effect.mode;
            *config.write().unwrap() = cfg;
            let msg = if mem_mode != new_mode {
                if mem_mode.is_screen() && new_mode.is_screen() {
                    ReloadMsg::Apply
                } else {
                    ReloadMsg::Force
                }
            } else {
                ReloadMsg::Apply
            };
            let _ = reload_tx.send(msg);
        }
    }
}
