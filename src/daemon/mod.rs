mod ipc;
mod lock;
mod state;
mod supervisor;

pub use ipc::{cleanup_stale_socket, daemon_running, ipc_request, patch_config, write_default_config_if_missing, IpcRequest, IpcResponse};
pub use state::{DaemonStatus, FpsMeter};

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::config::RuntimeConfig;

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

    // Flock first — beats the IPC race when launching twice from the KDE menu.
    let Some(_lock) = lock::try_acquire()? else {
        if daemon_running() {
            eprintln!("hyper-sync daemon already running");
            // Soft exit so desktop launches don't pop an error dialog.
            return Ok(());
        }
        anyhow::bail!(
            "hyper-sync daemon lock is held but IPC is not responding \
             (zombie?). Kill the old process, then retry."
        );
    };

    cleanup_stale_socket();
    if daemon_running() {
        eprintln!("hyper-sync daemon already running");
        return Ok(());
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

    #[cfg(feature = "audio")]
    let audio_snapshot = Arc::new(crate::audio::AudioSnapshot::new());
    #[cfg(feature = "audio")]
    let mut audio_monitor = crate::audio::AudioMonitor::new(
        Arc::clone(&audio_snapshot),
        Arc::clone(&status),
        Arc::clone(&config),
    );

    let mut supervisor = supervisor::Supervisor::new(
        Arc::clone(&config),
        Arc::clone(&status),
        #[cfg(feature = "audio")]
        Arc::clone(&audio_snapshot),
    );
    supervisor.reload();

    #[cfg(feature = "audio")]
    if crate::audio::audio_needed(&config.read().unwrap()) {
        audio_monitor.start();
    }

    let cfg_path = Arc::clone(&config_path);
    let cfg_arc = Arc::clone(&config);
    let reload_tx_watch = reload_tx.clone();
    let watch_shutdown = Arc::clone(&shutdown);
    thread::spawn(move || watch_config(&cfg_path, cfg_arc, reload_tx_watch, watch_shutdown));
    thread::spawn(watch_screen_lock);

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
        // Wake as soon as a change arrives (mode switches feel instant), else tick every 50 ms.
        let mut last_reload = reload_rx.recv_timeout(Duration::from_millis(50)).ok();
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
        #[cfg(feature = "audio")]
        {
            let cfg = config.read().unwrap();
            if crate::audio::audio_needed(&cfg) {
                if !audio_monitor.is_running() {
                    audio_monitor.start();
                }
            } else if audio_monitor.is_running() {
                audio_monitor.stop();
            }
            status.lock().unwrap().audio_level = audio_snapshot.level();
            drop(cfg);
        }
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
    }

    supervisor.stop_effect();
    supervisor.shutdown();
    #[cfg(feature = "audio")]
    audio_monitor.stop();
    let _ = std::fs::remove_file(crate::config::ipc_socket_path());
    eprintln!("hyper-sync daemon stopped");
    Ok(())
}

/// LEDs go dark while the screen is locked (KDE/GNOME emit ScreenSaver.ActiveChanged).
fn watch_screen_lock() {
    use crate::effects::LOCKED;
    const BUS: [&str; 5] =
        ["--session", "--dest", "org.freedesktop.ScreenSaver", "--object-path", "/ScreenSaver"];
    loop {
        if let Ok(out) = Command::new("gdbus")
            .arg("call")
            .args(BUS)
            .args(["--method", "org.freedesktop.ScreenSaver.GetActive"])
            .output()
        {
            LOCKED.store(String::from_utf8_lossy(&out.stdout).contains("true"), Ordering::Relaxed);
        }
        let mut monitor = Command::new("gdbus");
        // Any object path: KDE registers the interface on both /ScreenSaver and /org/freedesktop/ScreenSaver.
        monitor.arg("monitor").args(&BUS[..3]).stdout(Stdio::piped()).stderr(Stdio::null());
        // Die with the daemon instead of lingering as an orphan.
        unsafe {
            std::os::unix::process::CommandExt::pre_exec(&mut monitor, || {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let Ok(mut child) = monitor.spawn() else {
            return; // no gdbus: lock-off unavailable, everything else works
        };
        for line in BufReader::new(child.stdout.take().unwrap()).lines().map_while(Result::ok) {
            if line.contains("ScreenSaver.ActiveChanged") {
                let locked = line.contains("true");
                eprintln!("screen {}", if locked { "locked: LEDs off" } else { "unlocked: LEDs on" });
                LOCKED.store(locked, Ordering::Relaxed);
            }
        }
        let _ = child.wait();
        thread::sleep(Duration::from_secs(5)); // session bus restarted — resubscribe
    }
}

fn watch_config(
    path: &PathBuf,
    config: Arc<RwLock<RuntimeConfig>>,
    reload_tx: mpsc::Sender<ReloadMsg>,
    shutdown: Arc<AtomicBool>,
) {
    let (tx, rx): (mpsc::Sender<notify::Result<notify::Event>>, Receiver<_>) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(tx, notify::Config::default()).expect("notify watcher");
    // Watch the directory: editors like Kate save by renaming a new file over the old one,
    // which silently ends a watch on the file itself.
    let _ = watcher.watch(path.parent().unwrap_or(path), RecursiveMode::NonRecursive);
    while !shutdown.load(Ordering::Relaxed) {
        let event = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Ok(event)) => event,
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
        };
        if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_))
            || !event.paths.iter().any(|p| p == path)
        {
            continue;
        }
        // Let the writer finish, and fold the burst of events one save produces into one.
        thread::sleep(Duration::from_millis(150));
        while rx.try_recv().is_ok() {}
        if let Ok(cfg) = RuntimeConfig::load(path) {
            // TUI/tray/ctl saves land here too — only apply what differs from the daemon's state.
            if cfg.is_synced() {
                continue;
            }
            let (mem_key, mem_mode) = {
                let current = config.read().unwrap();
                (current.effect_key(), current.effect.mode)
            };
            eprintln!(
                "config file changed externally: mode {} -> {}",
                mem_mode.as_str(),
                cfg.effect.mode.as_str()
            );
            let new_mode = cfg.effect.mode;
            let restart = cfg.effect_key() != mem_key || new_mode != mem_mode;
            // Effects read brightness/color/fps/speed/audio every frame — no restart needed for those.
            cfg.mark_synced();
            *config.write().unwrap() = cfg;
            if !restart {
                continue;
            }
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
