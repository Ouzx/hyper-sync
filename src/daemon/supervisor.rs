use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::config::{EffectMode, RuntimeConfig};
use crate::daemon::DaemonStatus;
use crate::effects::solid;
use crate::protocol::black_frame;
use crate::serial::SerialWriter;

pub struct Supervisor {
    config: Arc<RwLock<RuntimeConfig>>,
    status: Arc<Mutex<DaemonStatus>>,
    cancel: Arc<AtomicBool>,
    writer: Arc<Mutex<SerialWriter>>,
    handle: Option<JoinHandle<()>>,
    last_effect_key: String,
    last_mode: EffectMode,
}

impl Supervisor {
    pub fn new(config: Arc<RwLock<RuntimeConfig>>, status: Arc<Mutex<DaemonStatus>>) -> Self {
        let cfg = config.read().unwrap().clone();
        Self {
            config,
            status,
            cancel: Arc::new(AtomicBool::new(false)),
            writer: Arc::new(Mutex::new(SerialWriter::new(cfg.device_config()))),
            handle: None,
            last_effect_key: String::new(),
            last_mode: cfg.effect.mode,
        }
    }

    pub fn reload(&mut self) {
        self.stop_effect();
        self.start_effect();
    }

    pub fn apply_config(&mut self) {
        if self.needs_restart() {
            self.reload();
        }
    }

    pub fn stop_effect(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            if h.is_finished() {
                let _ = h.join();
            }
            // ponytail: never block the daemon main loop waiting on pipewire — detach if still running
        }
        self.cancel.store(false, Ordering::SeqCst);
    }

    fn needs_restart(&self) -> bool {
        let cfg = self.config.read().unwrap();
        self.handle.is_none()
            || cfg.effect.mode != self.last_mode
            || cfg.effect_key() != self.last_effect_key
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
            st.color = cfg.solid.color.clone();
            st.last_error = None;
        }

        let cancel = Arc::clone(&self.cancel);
        let config = Arc::clone(&self.config);
        let status = Arc::clone(&self.status);
        let writer = Arc::clone(&self.writer);
        let preview = Arc::new(Mutex::new(Vec::new()));

        let mode = cfg.effect.mode;
        let status_err = Arc::clone(&status);
        self.handle = Some(thread::spawn(move || {
            let result = match mode {
                EffectMode::Off => run_off(&writer, &config, &cancel),
                EffectMode::Solid => {
                    solid::run_controlled(writer, config, cancel, status, preview)
                }
                EffectMode::Candle => crate::effects::candle::run_controlled(
                    writer, config, cancel, status, preview,
                ),
                EffectMode::Screen => {
                    #[cfg(feature = "screen")]
                    {
                        crate::capture::screen::run_controlled(
                            writer, config, cancel, status, preview,
                        )
                    }
                    #[cfg(not(feature = "screen"))]
                    {
                        Err(anyhow::anyhow!("screen mode not compiled in"))
                    }
                }
            };
            if let Err(e) = result {
                status_err.lock().unwrap().last_error = Some(e.to_string());
            }
        }));
    }

    pub fn tick(&mut self) {
        if self.needs_restart() {
            self.reload();
        }
    }
}

fn run_off(
    writer: &Arc<Mutex<SerialWriter>>,
    config: &Arc<RwLock<RuntimeConfig>>,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let cfg = config.read().unwrap();
    let frame = black_frame(cfg.device.leds);
    writer.lock().unwrap().write_frame(&frame)?;
    while !cancel.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}
