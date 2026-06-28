use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Context;
use pipewire as pw;
use pw::spa::param::format::{MediaSubtype, MediaType};
use pw::spa::param::format_utils;
use pw::spa::param::video::VideoInfoRaw;
use pw::spa::param::ParamType;

use crate::capture::sample;
use crate::capture::screen::{open_portal_session, wait_for_registry_target, PortalGuard};
use crate::capture::negotiate;
use crate::config::{self, EffectMode, LayoutConfig, RuntimeConfig};
use crate::daemon::DaemonStatus;
use crate::effects::solid::scale_rgb_buf;
use crate::protocol::build_frame;
use crate::serial::SerialWriter;

#[cfg(feature = "audio")]
use crate::audio::{maybe_modulate, AudioSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    Idle,
    Acquiring,
    Running,
    Error,
}

enum ScreenCmd {
    Acquire,
    Release,
    Reselect,
    Shutdown,
}

pub struct ScreenWorker {
    cmd_tx: Sender<ScreenCmd>,
    cancel: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    state: Arc<Mutex<WorkerState>>,
    handle: Option<JoinHandle<()>>,
}

impl ScreenWorker {
    pub fn new(
        config: Arc<RwLock<RuntimeConfig>>,
        status: Arc<Mutex<DaemonStatus>>,
        writer: Arc<Mutex<SerialWriter>>,
        #[cfg(feature = "audio")] audio: Arc<AudioSnapshot>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let running = Arc::new(AtomicBool::new(false));
        let pending_acquire = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(WorkerState::Idle));

        let worker_cancel = Arc::clone(&cancel);
        let worker_running = Arc::clone(&running);
        let worker_pending = Arc::clone(&pending_acquire);
        let worker_state = Arc::clone(&state);

        let handle = thread::spawn(move || {
            worker_loop(
                cmd_rx,
                worker_cancel,
                worker_running,
                worker_pending,
                worker_state,
                config,
                status,
                writer,
                #[cfg(feature = "audio")]
                audio,
            );
        });

        Self {
            cmd_tx,
            cancel,
            running,
            state,
            handle: Some(handle),
        }
    }

    pub fn acquire(&self) {
        let _ = self.cmd_tx.send(ScreenCmd::Acquire);
    }

    pub fn release(&self) {
        if !self.running.load(Ordering::Relaxed) {
            return;
        }
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(ScreenCmd::Release);
        while self.running.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn reselect(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(ScreenCmd::Reselect);
    }

    pub fn shutdown(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(ScreenCmd::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }

    pub fn state(&self) -> WorkerState {
        *self.state.lock().unwrap()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn wait_idle(&self) {
        while self.running.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

fn worker_loop(
    cmd_rx: Receiver<ScreenCmd>,
    cancel: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    pending_acquire: Arc<AtomicBool>,
    state: Arc<Mutex<WorkerState>>,
    config: Arc<RwLock<RuntimeConfig>>,
    status: Arc<Mutex<DaemonStatus>>,
    writer: Arc<Mutex<SerialWriter>>,
    #[cfg(feature = "audio")] audio: Arc<AudioSnapshot>,
) {
    let rt = Arc::new(
        tokio::runtime::Runtime::new().expect("screen worker tokio runtime"),
    );
    let last_portal_close = Cell::new(None::<Instant>);

    loop {
        let cmd = match cmd_rx.recv() {
            Ok(c) => c,
            Err(_) => break,
        };

        match cmd {
            ScreenCmd::Acquire => {
                if running.load(Ordering::Relaxed) {
                    pending_acquire.store(true, Ordering::SeqCst);
                    cancel.store(true, Ordering::SeqCst);
                    eprintln!("screen worker: acquire coalesced (busy)");
                    continue;
                }
                if *state.lock().unwrap() == WorkerState::Error {
                    *state.lock().unwrap() = WorkerState::Idle;
                }
                loop {
                    pending_acquire.store(false, Ordering::SeqCst);
                    cancel.store(false, Ordering::SeqCst);
                    running.store(true, Ordering::SeqCst);
                    eprintln!("screen worker: acquire");
                    let result = run_capture(
                        Arc::clone(&rt),
                        &last_portal_close,
                        Arc::clone(&cancel),
                        Arc::clone(&state),
                        Arc::clone(&config),
                        Arc::clone(&status),
                        Arc::clone(&writer),
                        false,
                        #[cfg(feature = "audio")]
                        Arc::clone(&audio),
                    );
                    portal_settle(&last_portal_close);
                    running.store(false, Ordering::SeqCst);
                    match result {
                        Ok(()) => {
                            *state.lock().unwrap() = WorkerState::Idle;
                        }
                        Err(e) if e.to_string().contains("portal open cancelled")
                            || e.to_string().contains("cancelled") =>
                        {
                            eprintln!("screen worker: acquire cancelled");
                            *state.lock().unwrap() = WorkerState::Idle;
                        }
                        Err(e) => {
                            eprintln!("screen worker error: {e:#}");
                            if e.to_string().contains("timed out") {
                                let _ = config::clear_portal_token();
                            }
                            status.lock().unwrap().last_error = Some(e.to_string());
                            *state.lock().unwrap() = WorkerState::Error;
                        }
                    }
                    if !pending_acquire.load(Ordering::Relaxed) {
                        break;
                    }
                    if !config.read().unwrap().effect.mode.is_screen() {
                        pending_acquire.store(false, Ordering::SeqCst);
                        break;
                    }
                    eprintln!("screen worker: coalesced acquire retry");
                    wait_portal_settle(&last_portal_close);
                }
            }
            ScreenCmd::Release => {
                pending_acquire.store(false, Ordering::SeqCst);
                eprintln!(
                    "screen worker: release (running={})",
                    running.load(Ordering::Relaxed)
                );
                cancel.store(true, Ordering::SeqCst);
                if !running.load(Ordering::Relaxed) {
                    clear_screen_status(&status);
                    *state.lock().unwrap() = WorkerState::Idle;
                }
            }
            ScreenCmd::Reselect => {
                pending_acquire.store(false, Ordering::SeqCst);
                cancel.store(true, Ordering::SeqCst);
                while running.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(10));
                }
                if let Err(e) = config::clear_portal_token() {
                    eprintln!("clear portal token: {e:#}");
                }
                clear_screen_status(&status);
                *state.lock().unwrap() = WorkerState::Idle;
                eprintln!("screen worker: reselect");

                if !config.read().unwrap().effect.mode.is_screen() {
                    continue;
                }

                cancel.store(false, Ordering::SeqCst);
                running.store(true, Ordering::SeqCst);
                let result = run_capture(
                    Arc::clone(&rt),
                    &last_portal_close,
                    Arc::clone(&cancel),
                    Arc::clone(&state),
                    Arc::clone(&config),
                    Arc::clone(&status),
                    Arc::clone(&writer),
                    true,
                    #[cfg(feature = "audio")]
                    Arc::clone(&audio),
                );
                portal_settle(&last_portal_close);
                running.store(false, Ordering::SeqCst);
                match result {
                    Ok(()) => {
                        *state.lock().unwrap() = WorkerState::Idle;
                    }
                    Err(e) if e.to_string().contains("portal open cancelled")
                        || e.to_string().contains("cancelled") =>
                    {
                        eprintln!("screen worker: acquire cancelled");
                        *state.lock().unwrap() = WorkerState::Idle;
                    }
                    Err(e) => {
                        eprintln!("screen worker error: {e:#}");
                        if e.to_string().contains("timed out") {
                            let _ = config::clear_portal_token();
                        }
                        status.lock().unwrap().last_error = Some(e.to_string());
                        *state.lock().unwrap() = WorkerState::Error;
                    }
                }
            }
            ScreenCmd::Shutdown => {
                pending_acquire.store(false, Ordering::SeqCst);
                cancel.store(true, Ordering::SeqCst);
                while running.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(10));
                }
                clear_screen_status(&status);
                break;
            }
        }
    }
}

fn clear_screen_status(status: &Arc<Mutex<DaemonStatus>>) {
    let mut st = status.lock().unwrap();
    st.width = 0;
    st.height = 0;
    st.detail.clear();
}

// ponytail: KDE portal needs ~1s between session.close and Screencast::new
const PORTAL_SETTLE: Duration = Duration::from_millis(1000);

fn wait_portal_settle(last_close: &Cell<Option<Instant>>) {
    let Some(t) = last_close.get() else {
        return;
    };
    let wait = PORTAL_SETTLE.saturating_sub(t.elapsed());
    if wait > Duration::ZERO {
        eprintln!("screen worker: portal settle {:?}…", wait);
        thread::sleep(wait);
    }
}

fn portal_settle(last_close: &Cell<Option<Instant>>) {
    last_close.set(Some(Instant::now()));
    thread::sleep(PORTAL_SETTLE);
}

struct CtrlFrameState {
    writer: Arc<Mutex<SerialWriter>>,
    config: Arc<RwLock<RuntimeConfig>>,
    zones: Rc<Vec<config::EdgeZone>>,
    format: std::cell::RefCell<Option<pw::spa::param::video::VideoFormat>>,
    width: Arc<AtomicU32>,
    height: Arc<AtomicU32>,
    last_frame: std::cell::RefCell<Instant>,
    got_frame: std::cell::RefCell<bool>,
    status: Arc<Mutex<DaemonStatus>>,
    cancel: Arc<AtomicBool>,
    mainloop_ptr: *mut pw::sys::pw_main_loop,
    quit_reason: std::cell::RefCell<&'static str>,
    #[cfg(feature = "audio")]
    audio: Arc<AudioSnapshot>,
}

fn run_capture(
    rt: Arc<tokio::runtime::Runtime>,
    last_portal_close: &Cell<Option<Instant>>,
    cancel: Arc<AtomicBool>,
    worker_state: Arc<Mutex<WorkerState>>,
    config: Arc<RwLock<RuntimeConfig>>,
    status: Arc<Mutex<DaemonStatus>>,
    writer: Arc<Mutex<SerialWriter>>,
    forget_portal: bool,
    #[cfg(feature = "audio")] audio: Arc<AudioSnapshot>,
) -> anyhow::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }

    *worker_state.lock().unwrap() = WorkerState::Acquiring;

    pw::init();

    let (layout_path, monitor) = {
        let cfg = config.read().unwrap();
        (
            config::resolve_layout_path(Path::new(&cfg.screen.layout)),
            cfg.screen.monitor,
        )
    };

    let zones = {
        let layout_path = layout_path
            .to_str()
            .context("layout path is not valid UTF-8")?;
        let layout = LayoutConfig::load(layout_path)?;
        let zones = layout.edge_zones();
        anyhow::ensure!(
            zones.len() == usize::from(layout.led_count),
            "layout zone count {} != led_count {}",
            zones.len(),
            layout.led_count
        );
        Rc::new(zones)
    };

    wait_portal_settle(last_portal_close);

    if cancel.load(Ordering::Relaxed) {
        eprintln!("screen worker: capture aborted (cancelled before portal)");
        return Ok(());
    }

    eprintln!("screen worker: opening portal…");
    let portal = match rt.block_on(open_portal_session(
        monitor,
        forget_portal,
        Some(Arc::clone(&cancel)),
    )) {
        Ok(p) => p,
        Err(e) if cancel.load(Ordering::Relaxed) || e.to_string().contains("cancelled") => {
            eprintln!("screen worker: portal open aborted ({e:#})");
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let (portal_stream, pw_fd, session, proxy) = portal;
    let _portal_guard = PortalGuard::new(Arc::clone(&rt), session, proxy);
    if cancel.load(Ordering::Relaxed) {
        eprintln!("screen worker: capture aborted (cancelled after portal)");
        return Ok(());
    }
    *worker_state.lock().unwrap() = WorkerState::Running;

    let (init_w, init_h) = portal_stream.size().unwrap_or((1920, 1080));
    let width = Arc::new(AtomicU32::new(init_w.max(1) as u32));
    let height = Arc::new(AtomicU32::new(init_h.max(1) as u32));
    let node_id = portal_stream.pipe_wire_node_id();

    let mainloop = pw::main_loop::MainLoopBox::new(None).context("pipewire mainloop")?;
    let mainloop_ptr = mainloop.as_raw_ptr();
    let context =
        pw::context::ContextBox::new(mainloop.loop_(), None).context("pipewire context")?;
    let core = context
        .connect_fd(pw_fd, None)
        .context("pipewire connect_fd")?;

    let target = match wait_for_registry_target(
        &core,
        &mainloop,
        node_id,
        Some(Arc::clone(&cancel)),
    ) {
        Ok(t) => t,
        Err(_) if cancel.load(Ordering::Relaxed) => return Ok(()),
        Err(e) => return Err(e),
    };

    let mut props = pw::properties::PropertiesBox::new();
    props.insert("media.type", "Video");
    props.insert("media.category", "Capture");
    props.insert("media.role", "Screen");
    props.insert("target.object", target.clone());

    let stream =
        pw::stream::StreamBox::new(&core, "hyper-sync", props).context("pipewire stream")?;

    let fps = config.read().unwrap().effect.fps;
    let min_interval = Duration::from_micros(1_000_000 / u64::from(fps.max(1)));
    let frame_state = Rc::new(CtrlFrameState {
        writer,
        config: Arc::clone(&config),
        zones,
        format: std::cell::RefCell::new(None),
        width: width.clone(),
        height: height.clone(),
        last_frame: std::cell::RefCell::new(Instant::now() - min_interval),
        got_frame: std::cell::RefCell::new(false),
        status: Arc::clone(&status),
        cancel: Arc::clone(&cancel),
        mainloop_ptr,
        quit_reason: std::cell::RefCell::new("unknown"),
        #[cfg(feature = "audio")]
        audio,
    });

    let cancel_watch = Arc::clone(&cancel);
    let _cancel_timer = mainloop.loop_().add_timer(move |_| {
        if cancel_watch.load(Ordering::Relaxed) {
            unsafe { pw::sys::pw_main_loop_quit(mainloop_ptr) };
        }
    });
    let _ = _cancel_timer.update_timer(Some(Duration::from_millis(50)), None);

    let frame_state_capture = Rc::clone(&frame_state);
    let _listener = stream
        .add_local_listener_with_user_data(frame_state)
        .state_changed(|_stream, _state, old, new| {
            if let pw::stream::StreamState::Error(msg) = new {
                eprintln!("pipewire stream: {old:?} -> Error({msg:?})");
            } else if old != new {
                eprintln!("pipewire stream: {old:?} -> {new:?}");
            }
        })
        .param_changed(|stream, state, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != ParamType::Format.as_raw() {
                return;
            }

            let Ok((media_type, media_subtype)) = format_utils::parse_format(param) else {
                return;
            };
            if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
                return;
            }

            let mut info = VideoInfoRaw::new();
            if info.parse(param).is_err() {
                return;
            }

            let fmt = info.format();
            let size = info.size();
            let stride = negotiate::stride_for(fmt, size.width);
            state.width.store(size.width, Ordering::Relaxed);
            state.height.store(size.height, Ordering::Relaxed);
            *state.format.borrow_mut() = Some(fmt);

            let buf_bytes = negotiate::buffer_params_bytes(stride, size.height);
            let buf_pod = pw::spa::pod::Pod::from_bytes(&buf_bytes).expect("buffer pod");
            let mut params = [buf_pod];
            let _ = stream.update_params(&mut params);
        })
        .process({
            let w = width.clone();
            let h = height.clone();
            move |stream, state| {
                if state.cancel.load(Ordering::Relaxed) {
                    *state.quit_reason.borrow_mut() = "cancel";
                    unsafe { pw::sys::pw_main_loop_quit(state.mainloop_ptr) };
                    return;
                }

                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };

                let cfg = state.config.read().unwrap();
                let mode = cfg.effect.mode;
                let min_interval =
                    Duration::from_micros(1_000_000 / u64::from(cfg.effect.fps.max(1)));
                let now = Instant::now();
                if now.duration_since(*state.last_frame.borrow()) < min_interval {
                    return;
                }

                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }

                let width = w.load(Ordering::Relaxed).max(1);
                let height = h.load(Ordering::Relaxed).max(1);
                let Some(format) = *state.format.borrow() else {
                    return;
                };

                let mut rgb = match mode {
                    EffectMode::ScreenCenter => {
                        sample::sample_center(datas, format, width, height, cfg.device.leds)
                    }
                    _ => sample::sample_edges(datas, format, width, height, &state.zones),
                };
                scale_rgb_buf(&mut rgb, cfg.effect.brightness);
                let leds = cfg.device.leds;
                let n = usize::from(leds);
                #[cfg(feature = "audio")]
                maybe_modulate(&mut rgb, n, &cfg, &state.audio);
                drop(cfg);

                if let Ok(packet) = build_frame(leds, &rgb) {
                    if state.writer.lock().unwrap().write_frame(&packet).is_ok() {
                        if !*state.got_frame.borrow() {
                            eprintln!("first frame sent to LEDs");
                            *state.got_frame.borrow_mut() = true;
                        }
                        let mut st = state.status.lock().unwrap();
                        st.brightness = state.config.read().unwrap().effect.brightness;
                        st.fps = state.config.read().unwrap().effect.fps;
                        st.width = width;
                        st.height = height;
                        st.serial_ok = true;
                        st.detail = format!("{width}x{height}");
                        #[cfg(feature = "audio")]
                        {
                            st.audio_level = state.audio.level();
                        }
                    }
                }
                *state.last_frame.borrow_mut() = now;
            }
        })
        .register()
        .context("pipewire listener")?;

    let format_bytes = negotiate::connect_format_bytes();
    let format_pod = pw::spa::pod::Pod::from_bytes(&format_bytes).expect("format pod");
    let mut connect_params = [format_pod];

    stream
        .connect(
            pw::spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut connect_params,
        )
        .context("pipewire stream connect")?;

    eprintln!(
        "hyper-sync screen: target.object={target} (portal node {node_id}) {}x{}",
        width.load(Ordering::Relaxed),
        height.load(Ordering::Relaxed),
    );

    eprintln!("screen worker: pipewire mainloop running");
    mainloop.run();
    let reason = *frame_state_capture.quit_reason.borrow();
    if cancel.load(Ordering::Relaxed) && reason == "unknown" {
        eprintln!("screen worker: capture ended (cancel)");
    } else {
        eprintln!("screen worker: capture ended ({reason})");
    }
    Ok(())
}
