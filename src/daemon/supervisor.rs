use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::{self, EffectMode, RuntimeConfig};
use crate::daemon::DaemonStatus;
use crate::protocol::black_frame;
use crate::serial::SerialWriter;

#[cfg(feature = "screen")]
use crate::capture::worker::{ScreenWorker, WorkerState};

#[cfg(feature = "audio")]
use crate::audio::AudioSnapshot;

pub struct Supervisor {
    config: Arc<RwLock<RuntimeConfig>>,
    status: Arc<Mutex<DaemonStatus>>,
    cancel: Arc<AtomicBool>,
    writer: Arc<Mutex<SerialWriter>>,
    handle: Option<JoinHandle<()>>,
    last_effect_key: String,
    last_mode: EffectMode,
    effect_retry: Option<Instant>,
    #[cfg(feature = "audio")]
    audio: Arc<AudioSnapshot>,
    #[cfg(feature = "screen")]
    screen: ScreenWorker,
    #[cfg(feature = "screen")]
    last_screen_retry: Option<Instant>,
    #[cfg(feature = "screen")]
    screen_retry_count: u32,
}

impl Supervisor {
    pub fn new(
        config: Arc<RwLock<RuntimeConfig>>,
        status: Arc<Mutex<DaemonStatus>>,
        #[cfg(feature = "audio")] audio: Arc<AudioSnapshot>,
    ) -> Self {
        let cfg = config.read().unwrap().clone();
        let writer = Arc::new(Mutex::new(SerialWriter::new(cfg.device_config())));
        #[cfg(feature = "screen")]
        let screen = ScreenWorker::new(
            Arc::clone(&config),
            Arc::clone(&status),
            Arc::clone(&writer),
            #[cfg(feature = "audio")]
            Arc::clone(&audio),
        );
        Self {
            config,
            status,
            cancel: Arc::new(AtomicBool::new(false)),
            writer,
            handle: None,
            last_effect_key: String::new(),
            last_mode: cfg.effect.mode,
            effect_retry: None,
            #[cfg(feature = "audio")]
            audio,
            #[cfg(feature = "screen")]
            screen,
            #[cfg(feature = "screen")]
            last_screen_retry: None,
            #[cfg(feature = "screen")]
            screen_retry_count: 0,
        }
    }

    pub fn reload(&mut self) {
        self.stop_effect();
        thread::sleep(Duration::from_millis(50));
        #[cfg(feature = "screen")]
        {
            self.screen.wait_idle();
            self.screen_retry_count = 0;
        }
        self.start_effect();
    }

    pub fn apply_config(&mut self) {
        let cfg = self.config.read().unwrap();
        if self.last_mode.is_screen()
            && cfg.effect.mode.is_screen()
            && cfg.effect_key() == self.last_effect_key
        {
            self.last_mode = cfg.effect.mode;
            return;
        }
        drop(cfg);
        if self.needs_restart() {
            self.reload();
        }
    }

    pub fn reselect_screen(&mut self) {
        #[cfg(feature = "screen")]
        {
            self.screen_retry_count = 0;
            self.last_screen_retry = None;
            let _ = config::clear_portal_token();
            if !self.config.read().unwrap().effect.mode.is_screen() {
                return;
            }
            if self.screen.is_running() {
                self.screen.release();
            }
            self.screen.reselect();
        }
    }

    pub fn stop_effect(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        #[cfg(feature = "screen")]
        if self.screen.is_running() {
            eprintln!("supervisor: stopping screen capture");
            self.screen.release();
        }
        if let Some(h) = self.handle.take() {
            let deadline = Instant::now() + Duration::from_millis(300);
            while !h.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
    }

    pub fn shutdown(&mut self) {
        self.stop_effect();
        #[cfg(feature = "screen")]
        self.screen.shutdown();
    }

    fn needs_restart(&self) -> bool {
        let cfg = self.config.read().unwrap();
        mode_needs_restart(self.last_mode, cfg.effect.mode)
            || cfg.effect_key() != self.last_effect_key
    }

    #[cfg(feature = "screen")]
    fn screen_error_needs_retry(&self, cfg: &RuntimeConfig) -> bool {
        cfg.effect.mode.is_screen()
            && self.last_mode == cfg.effect.mode
            && self.last_effect_key == cfg.effect_key()
            && self.screen.state() == WorkerState::Error
            && !self.screen.is_running()
    }

    fn start_effect(&mut self) {
        let cfg = self.config.read().unwrap().clone();
        self.last_effect_key = cfg.effect_key();
        self.last_mode = cfg.effect.mode;
        {
            let mut st = self.status.lock().unwrap();
            st.effect = cfg.effect.mode.as_str().to_string();
            st.brightness = cfg.effect.brightness;
            st.fps = cfg.effect.fps;
            st.actual_fps = 0.0;
            st.speed = cfg.effect.speed;
            st.color = cfg.solid.color.clone();
            st.sound_mode = cfg.audio.sound_mode.as_str().to_string();
            st.last_error = None;
        }

        if cfg.effect.mode.is_screen() {
            #[cfg(feature = "screen")]
            {
                self.cancel = Arc::new(AtomicBool::new(false));
                self.handle = None;
                eprintln!("supervisor: starting screen capture");
                self.screen.acquire();
                return;
            }
            #[cfg(not(feature = "screen"))]
            {
                self.status.lock().unwrap().last_error =
                    Some("screen mode not compiled in".into());
                return;
            }
        }

        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&self.cancel);
        let config = Arc::clone(&self.config);
        let status = Arc::clone(&self.status);
        let writer = Arc::clone(&self.writer);
        #[cfg(feature = "audio")]
        let audio = Arc::clone(&self.audio);

        let mode = cfg.effect.mode;
        let status_err = Arc::clone(&status);
        let writer_err = Arc::clone(&writer);
        self.handle = Some(thread::spawn(move || {
            let result = match mode {
                EffectMode::Off => run_off(&writer, &config, &cancel),
                EffectMode::Screen | EffectMode::ScreenCenter => unreachable!("screen uses worker"),
                _ => crate::effects::run(
                    mode,
                    writer,
                    config,
                    cancel,
                    status,
                    #[cfg(feature = "audio")]
                    audio,
                ),
            };
            if let Err(e) = result {
                let open = writer_err.lock().unwrap().is_open();
                let mut st = status_err.lock().unwrap();
                st.last_error = Some(format!("{e:#}"));
                st.serial_ok = open;
            }
        }));
    }

    pub fn tick(&mut self) {
        #[cfg(feature = "screen")]
        {
            if self.screen.state() == WorkerState::Running {
                self.screen_retry_count = 0;
                self.last_screen_retry = None;
            }

            let cfg = self.config.read().unwrap();
            if self.screen_error_needs_retry(&cfg) {
                // PipeWire restarts can take a while — keep trying for ~1 min.
                const MAX_RETRIES: u32 = 12;
                const RETRY_BACKOFF: Duration = Duration::from_secs(5);
                if self.screen_retry_count >= MAX_RETRIES {
                    return;
                }
                if let Some(t) = self.last_screen_retry {
                    if t.elapsed() < RETRY_BACKOFF {
                        return;
                    }
                }
                eprintln!(
                    "supervisor: retrying failed screen capture ({}/{MAX_RETRIES})",
                    self.screen_retry_count + 1
                );
                self.last_screen_retry = Some(Instant::now());
                self.screen_retry_count += 1;
                drop(cfg);
                if !self.screen.is_running() {
                    self.screen.acquire();
                }
                return;
            }
        }
        if self.needs_restart() {
            eprintln!(
                "supervisor: reload (mode {} -> {}, key {} -> {})",
                self.last_mode.as_str(),
                self.config.read().unwrap().effect.mode.as_str(),
                self.last_effect_key,
                self.config.read().unwrap().effect_key(),
            );
            self.reload();
        } else if self.handle.as_ref().is_some_and(|h| h.is_finished())
            && self.effect_retry.is_none_or(|t| t.elapsed() >= EFFECT_RETRY)
        {
            // Effect thread died on its own (USB unplugged, bad color in config.toml) — keep retrying.
            // Log once per failure streak, not every retry.
            if self.effect_retry.is_none_or(|t| t.elapsed() > EFFECT_RETRY * 3) {
                let err = self.status.lock().unwrap().last_error.clone().unwrap_or_default();
                eprintln!("supervisor: effect stopped ({err}), retrying every {EFFECT_RETRY:?}");
            }
            self.effect_retry = Some(Instant::now());
            self.reload();
        }
    }
}

const EFFECT_RETRY: Duration = Duration::from_secs(2);

fn mode_needs_restart(old: EffectMode, new: EffectMode) -> bool {
    if old == new {
        return false;
    }
    if old.is_screen() && new.is_screen() {
        return false;
    }
    true
}

fn run_off(
    writer: &Arc<Mutex<SerialWriter>>,
    config: &Arc<RwLock<RuntimeConfig>>,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let leds = config.read().unwrap().device.leds;
    let frame = black_frame(leds);
    let interval = Duration::from_millis(200);
    while !cancel.load(Ordering::Relaxed) {
        writer.lock().unwrap().write_frame(&frame)?;
        thread::sleep(interval);
    }
    Ok(())
}
