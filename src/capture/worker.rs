use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
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
use crate::capture::screen::{open_portal_session, wait_for_graphical_session, wait_for_registry_target, PortalGuard};
use crate::capture::negotiate;
use crate::config::{self, EffectMode, LayoutConfig, RuntimeConfig};
use crate::daemon::{DaemonStatus, FpsMeter};
use crate::effects::finish;
use crate::protocol::build_frame;
use crate::serial::SerialWriter;

use crate::audio::AudioSnapshot;

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
    /// Stops every capture asked for so far, running or still queued; a new request clears it.
    cancel: Arc<AtomicBool>,
    /// Captures asked for and not finished yet. Counted when asked (not when the worker gets to
    /// them), so a stop right after a start never misses the start.
    pending: Arc<AtomicUsize>,
    state: Arc<Mutex<WorkerState>>,
    handle: Option<JoinHandle<()>>,
}

impl ScreenWorker {
    pub fn new(
        config: Arc<RwLock<RuntimeConfig>>,
        status: Arc<Mutex<DaemonStatus>>,
        writer: Arc<Mutex<SerialWriter>>,
        audio: Arc<AudioSnapshot>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let pending = Arc::new(AtomicUsize::new(0));
        let state = Arc::new(Mutex::new(WorkerState::Idle));

        let worker_cancel = Arc::clone(&cancel);
        let worker_pending = Arc::clone(&pending);
        let worker_state = Arc::clone(&state);

        let handle = thread::spawn(move || {
            worker_loop(
                cmd_rx,
                worker_cancel,
                worker_pending,
                worker_state,
                config,
                status,
                writer,
                audio,
            );
        });

        Self {
            cmd_tx,
            cancel,
            pending,
            state,
            handle: Some(handle),
        }
    }

    pub fn acquire(&self) {
        self.request(ScreenCmd::Acquire);
    }

    /// Capture again with the saved permission cleared, so the portal asks which screen.
    pub fn reselect(&self) {
        self.request(ScreenCmd::Reselect);
    }

    fn request(&self, cmd: ScreenCmd) {
        self.cancel.store(false, Ordering::SeqCst);
        self.pending.fetch_add(1, Ordering::SeqCst);
        let _ = self.cmd_tx.send(cmd);
    }

    /// Stop every capture asked for so far and wait (up to 3 s) for them to end.
    pub fn release(&self) {
        if !self.is_running() {
            return;
        }
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(ScreenCmd::Release);
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.is_running() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        if self.is_running() {
            // Still counted as running, so the next stop cancels it again.
            eprintln!("screen worker: release timed out, capture still ending");
        }
    }

    pub fn shutdown(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(ScreenCmd::Shutdown);
        if let Some(h) = self.handle.take() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !h.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            if h.is_finished() {
                let _ = h.join();
            } else {
                eprintln!("screen worker: shutdown join timed out");
            }
        }
    }

    pub fn state(&self) -> WorkerState {
        *self.state.lock().unwrap()
    }

    /// A capture is running or queued.
    pub fn is_running(&self) -> bool {
        self.pending.load(Ordering::SeqCst) > 0
    }
}

#[allow(clippy::too_many_arguments)]
fn worker_loop(
    cmd_rx: Receiver<ScreenCmd>,
    cancel: Arc<AtomicBool>,
    pending: Arc<AtomicUsize>,
    state: Arc<Mutex<WorkerState>>,
    config: Arc<RwLock<RuntimeConfig>>,
    status: Arc<Mutex<DaemonStatus>>,
    writer: Arc<Mutex<SerialWriter>>,
    audio: Arc<AudioSnapshot>,
) {
    let rt = Arc::new(
        // Only runs a few portal calls; one worker (not one per core) keeps D-Bus moving between them.
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("screen worker tokio runtime"),
    );
    let last_portal_close = Cell::new(None::<Instant>);

    // Commands are read only between captures: a running capture ends through `cancel`.
    for cmd in cmd_rx {
        match cmd {
            ScreenCmd::Acquire => {}
            ScreenCmd::Release => {
                eprintln!("screen worker: release");
                clear_screen_status(&status);
                *state.lock().unwrap() = WorkerState::Idle;
                continue;
            }
            ScreenCmd::Reselect => {
                // Clear again now the old capture has ended (it may have saved a token meanwhile),
                // so the portal asks.
                if let Err(e) = config::clear_portal_token() {
                    eprintln!("clear portal token: {e:#}");
                }
                clear_screen_status(&status);
                *state.lock().unwrap() = WorkerState::Idle;
                eprintln!("screen worker: reselect");
                wait_portal_settle(&last_portal_close);
                thread::sleep(PORTAL_RESELECT_SETTLE);
            }
            ScreenCmd::Shutdown => {
                clear_screen_status(&status);
                break;
            }
        }
        // Skip a request that was stopped while it waited in the queue.
        if !cancel.load(Ordering::SeqCst) && config.read().unwrap().effect.mode.is_screen() {
            eprintln!("screen worker: acquire");
            let result = run_capture(
                Arc::clone(&rt),
                &last_portal_close,
                Arc::clone(&cancel),
                Arc::clone(&state),
                Arc::clone(&config),
                Arc::clone(&status),
                Arc::clone(&writer),
                Arc::clone(&audio),
            );
            // No sleep here: the next capture waits out the rest of PORTAL_SETTLE before opening.
            last_portal_close.set(Some(Instant::now()));
            let next = match result {
                Ok(()) => WorkerState::Idle,
                Err(e) if e.to_string().contains("cancelled") => {
                    eprintln!("screen worker: acquire cancelled");
                    WorkerState::Idle
                }
                Err(e) => {
                    eprintln!("screen worker error: {e:#}");
                    if e.to_string().contains("timed out") {
                        let _ = config::clear_portal_token();
                    }
                    status.lock().unwrap().last_error = Some(e.to_string());
                    WorkerState::Error
                }
            };
            *state.lock().unwrap() = next;
        }
        pending.fetch_sub(1, Ordering::SeqCst);
    }
}

fn clear_screen_status(status: &Arc<Mutex<DaemonStatus>>) {
    let mut st = status.lock().unwrap();
    st.width = 0;
    st.height = 0;
    st.detail.clear();
    st.actual_fps = 0.0;
}

// ponytail: KDE portal needs ~1s between session.close and Screencast::new
const PORTAL_SETTLE: Duration = Duration::from_millis(1000);
const PORTAL_RESELECT_SETTLE: Duration = Duration::from_millis(2000);

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

struct CtrlFrameState {
    writer: Arc<Mutex<SerialWriter>>,
    config: Arc<RwLock<RuntimeConfig>>,
    zones: Rc<Vec<config::EdgeZone>>,
    format: std::cell::RefCell<Option<pw::spa::param::video::VideoFormat>>,
    width: Arc<AtomicU32>,
    height: Arc<AtomicU32>,
    last_frame: std::cell::RefCell<Instant>,
    /// Smoothed LED colors carried between frames (screen smoothing).
    smooth: std::cell::RefCell<Vec<f32>>,
    got_frame: std::cell::RefCell<bool>,
    fps_meter: std::cell::RefCell<FpsMeter>,
    status: Arc<Mutex<DaemonStatus>>,
    cancel: Arc<AtomicBool>,
    mainloop_ptr: *mut pw::sys::pw_main_loop,
    quit_reason: std::cell::RefCell<&'static str>,
    audio: Arc<AudioSnapshot>,
}

#[allow(clippy::too_many_arguments)]
fn run_capture(
    rt: Arc<tokio::runtime::Runtime>,
    last_portal_close: &Cell<Option<Instant>>,
    cancel: Arc<AtomicBool>,
    worker_state: Arc<Mutex<WorkerState>>,
    config: Arc<RwLock<RuntimeConfig>>,
    status: Arc<Mutex<DaemonStatus>>,
    writer: Arc<Mutex<SerialWriter>>,
    audio: Arc<AudioSnapshot>,
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

    if !wait_for_graphical_session(Duration::from_secs(60)) {
        anyhow::bail!("graphical session not ready (WAYLAND_DISPLAY / DBUS)");
    }

    if cancel.load(Ordering::Relaxed) {
        eprintln!("screen worker: capture aborted (cancelled before portal)");
        return Ok(());
    }

    eprintln!("screen worker: opening portal…");
    let portal = match rt.block_on(open_portal_session(
        monitor,
        false,
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

    let min_interval = config::frame_interval(config.read().unwrap().effect.fps);
    let frame_state = Rc::new(CtrlFrameState {
        writer,
        config: Arc::clone(&config),
        zones,
        format: std::cell::RefCell::new(None),
        width: width.clone(),
        height: height.clone(),
        last_frame: std::cell::RefCell::new(Instant::now() - min_interval),
        smooth: std::cell::RefCell::new(Vec::new()),
        got_frame: std::cell::RefCell::new(false),
        fps_meter: std::cell::RefCell::new(FpsMeter::new()),
        status: Arc::clone(&status),
        cancel: Arc::clone(&cancel),
        mainloop_ptr,
        quit_reason: std::cell::RefCell::new("unknown"),
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
        .state_changed(|_stream, state, old, new| {
            if let pw::stream::StreamState::Error(msg) = new {
                eprintln!("pipewire stream: {old:?} -> Error({msg:?})");
                *state.quit_reason.borrow_mut() = "pipewire error";
                unsafe { pw::sys::pw_main_loop_quit(state.mainloop_ptr) };
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
                let min_interval = config::frame_interval(cfg.effect.fps);
                // Frames arrive at the monitor's refresh rate; take one per interval, keeping the
                // schedule (not the arrival time) so small lateness doesn't lower the frame rate.
                let now = Instant::now();
                let due = *state.last_frame.borrow() + min_interval;
                if now < due {
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
                // Smoothing: ease toward the new colors with a time constant of 0.6 s × smoothing².
                let tau = 0.6 * cfg.screen.smoothing.clamp(0.0, 1.0).powi(2);
                let mut prev = state.smooth.borrow_mut();
                if tau > 0.001 && prev.len() == rgb.len() {
                    let dt = now.duration_since(*state.last_frame.borrow()).as_secs_f32().min(0.5);
                    let a = 1.0 - (-dt / tau).exp();
                    for (p, c) in prev.iter_mut().zip(rgb.iter_mut()) {
                        *p += (f32::from(*c) - *p) * a;
                        *c = p.round() as u8;
                    }
                } else {
                    *prev = rgb.iter().map(|&c| f32::from(c)).collect();
                }
                drop(prev);
                finish(&mut rgb, &cfg, cfg.screen.gamma, &state.audio);
                // Copy what the status needs before letting go: taking `config` while holding
                // `status` (others lock config, then status) can deadlock when a writer queues.
                let (leds, brightness, fps) = (cfg.device.leds, cfg.effect.brightness, cfg.effect.fps);
                drop(cfg);

                if let Ok(packet) = build_frame(leds, &rgb) {
                    if state.writer.lock().unwrap().write_frame(&packet).is_ok() {
                        if !*state.got_frame.borrow() {
                            eprintln!("first frame sent to LEDs");
                            *state.got_frame.borrow_mut() = true;
                        }
                        let actual = state.fps_meter.borrow_mut().tick();
                        let mut st = state.status.lock().unwrap();
                        st.brightness = brightness;
                        st.fps = fps;
                        st.actual_fps = actual;
                        st.width = width;
                        st.height = height;
                        st.serial_ok = true;
                        st.detail = format!("{width}x{height}");
                        st.leds = crate::effects::to_hex(&rgb);
                        st.audio_level = state.audio.level();
                    } else {
                        state.status.lock().unwrap().serial_ok = false;
                    }
                }
                *state.last_frame.borrow_mut() = if now - due < min_interval { due } else { now };
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
    if reason == "pipewire error" {
        anyhow::bail!("pipewire stream error (restarted?)");
    }
    Ok(())
}
