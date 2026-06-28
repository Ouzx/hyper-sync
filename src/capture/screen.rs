use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::Context;
use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
use ashpd::desktop::PersistMode;
use ashpd::WindowIdentifier;
use pipewire as pw;
use pw::core::PW_ID_CORE;
use pw::spa::param::format::{MediaSubtype, MediaType};
use pw::spa::param::format_utils;
use pw::spa::param::video::VideoInfoRaw;
use pw::spa::param::ParamType;
use pw::spa::utils::dict::DictRef;
use pw::types::ObjectType;

use crate::capture::negotiate;
use crate::capture::sample;
use crate::config::{self, DeviceConfig, EdgeZone, LayoutConfig, RuntimeConfig};
use crate::daemon::DaemonStatus;
use crate::effects::solid::scale_rgb_buf;
use crate::protocol::build_frame;
use crate::serial::SerialWriter;

struct FrameState {
    writer: Rc<RefCell<SerialWriter>>,
    device: Rc<RefCell<DeviceConfig>>,
    zones: Rc<Vec<EdgeZone>>,
    format: RefCell<Option<pw::spa::param::video::VideoFormat>>,
    width: Arc<AtomicU32>,
    height: Arc<AtomicU32>,
    last_frame: RefCell<Instant>,
    min_interval: Duration,
    brightness: f32,
    got_frame: RefCell<bool>,
}

struct CtrlFrameState {
    writer: Arc<Mutex<SerialWriter>>,
    config: Arc<RwLock<RuntimeConfig>>,
    zones: Rc<Vec<EdgeZone>>,
    format: RefCell<Option<pw::spa::param::video::VideoFormat>>,
    width: Arc<AtomicU32>,
    height: Arc<AtomicU32>,
    last_frame: RefCell<Instant>,
    got_frame: RefCell<bool>,
    status: Arc<Mutex<DaemonStatus>>,
    cancel: Arc<AtomicBool>,
    mainloop_ptr: *mut pw::sys::pw_main_loop,
}

#[derive(Clone)]
struct NodeInfo {
    id: u32,
    label: String,
    target: Option<String>,
}

#[derive(Default)]
struct RegistryScan {
    portal_node_id: u32,
    nodes: Vec<NodeInfo>,
}

impl RegistryScan {
    fn note_node(&mut self, id: u32, props: &DictRef) {
        let label = props
            .get("node.name")
            .or_else(|| props.get("media.class"))
            .unwrap_or("?")
            .to_string();
        self.nodes.push(NodeInfo {
            id,
            label,
            target: target_from_props(props),
        });
    }

    fn portal_target(&self) -> Option<String> {
        self.nodes
            .iter()
            .find(|n| n.id == self.portal_node_id)
            .and_then(|n| n.target.clone())
    }

    fn matched_portal_id(&self) -> bool {
        self.nodes.iter().any(|n| n.id == self.portal_node_id)
    }
}

fn target_from_props(props: &DictRef) -> Option<String> {
    props
        .get("object.serial")
        .or_else(|| props.get("node.name"))
        .map(str::to_string)
}

fn wait_for_registry_target(
    core: &pw::core::Core,
    mainloop: &pw::main_loop::MainLoop,
    portal_node_id: u32,
    cancel: Option<Arc<AtomicBool>>,
) -> anyhow::Result<String> {
    let registry = core.get_registry().context("pipewire registry")?;
    let scan = Rc::new(RefCell::new(RegistryScan {
        portal_node_id,
        ..Default::default()
    }));
    let mainloop_ptr = mainloop.as_raw_ptr();

    let scan_reg = Rc::clone(&scan);
    let _reg_listener = registry
        .add_listener_local()
        .global(move |global| {
            if global.type_ != ObjectType::Node {
                return;
            }
            let Some(props) = global.props.as_ref() else {
                return;
            };
            let mut s = scan_reg.borrow_mut();
            s.note_node(global.id, props);
            if s.matched_portal_id() {
                unsafe { pw::sys::pw_main_loop_quit(mainloop_ptr) };
            }
        })
        .register();

    let pending = core.sync(0).context("pipewire sync")?;
    let scan_sync = Rc::clone(&scan);
    let _core_listener = core
        .add_listener_local()
        .done(move |id, seq| {
            if id == PW_ID_CORE && seq == pending && scan_sync.borrow().matched_portal_id() {
                unsafe { pw::sys::pw_main_loop_quit(mainloop_ptr) };
            }
        })
        .register();

    let _timer = mainloop.loop_().add_timer(move |_| {
        unsafe { pw::sys::pw_main_loop_quit(mainloop_ptr) };
    });
    let _ = _timer.update_timer(Some(Duration::from_secs(10)), None);

    if let Some(cancel_watch) = cancel.clone() {
        let _poll = mainloop.loop_().add_timer(move |_| {
            if cancel_watch.load(Ordering::Relaxed) {
                unsafe { pw::sys::pw_main_loop_quit(mainloop_ptr) };
            }
        });
        let _ = _poll.update_timer(Some(Duration::from_millis(50)), None);
    }

    mainloop.run();

    if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
        anyhow::bail!("screen capture cancelled");
    }

    let scan = scan.borrow();
    eprintln!(
        "pipewire registry: {} node(s), portal id {}{}",
        scan.nodes.len(),
        portal_node_id,
        if scan.matched_portal_id() {
            " matched"
        } else {
            " not matched"
        }
    );
    for node in &scan.nodes {
        eprintln!("  node {} ({})", node.id, node.label);
    }

    scan.portal_target().with_context(|| {
        format!(
            "portal screencast node {portal_node_id} not seen within 10s ({} other nodes)",
            scan.nodes.len()
        )
    })
}

pub fn run(
    device: DeviceConfig,
    layout_path: &str,
    fps: u32,
    monitor: u32,
    brightness: f32,
    forget_portal: bool,
) -> anyhow::Result<()> {
    pw::init();

    let layout = LayoutConfig::load(layout_path)?;
    let zones = layout.edge_zones();
    anyhow::ensure!(
        zones.len() == usize::from(layout.led_count),
        "layout zone count {} != led_count {}",
        zones.len(),
        layout.led_count
    );

    let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
    let (portal_stream, pw_fd) = rt.block_on(open_portal(monitor, forget_portal))?;

    let (init_w, init_h) = portal_stream.size().unwrap_or((1920, 1080));
    let width = Arc::new(AtomicU32::new(init_w.max(1) as u32));
    let height = Arc::new(AtomicU32::new(init_h.max(1) as u32));
    let node_id = portal_stream.pipe_wire_node_id();

    let mainloop = pw::main_loop::MainLoopBox::new(None).context("pipewire mainloop")?;
    let context = pw::context::ContextBox::new(mainloop.loop_(), None).context("pipewire context")?;
    let core = context
        .connect_fd(pw_fd, None)
        .context("pipewire connect_fd")?;

    let target = wait_for_registry_target(&core, &mainloop, node_id, None)?;

    let mut props = pw::properties::PropertiesBox::new();
    props.insert("media.type", "Video");
    props.insert("media.category", "Capture");
    props.insert("media.role", "Screen");
    props.insert("target.object", target.clone());

    let stream = pw::stream::StreamBox::new(&core, "hyper-sync", props).context("pipewire stream")?;

    let min_interval = Duration::from_micros(1_000_000 / u64::from(fps.max(1)));
    let state = Rc::new(FrameState {
        writer: Rc::new(RefCell::new(SerialWriter::new(device.clone()))),
        device: Rc::new(RefCell::new(device)),
        zones: Rc::new(zones),
        format: RefCell::new(None),
        width: width.clone(),
        height: height.clone(),
        last_frame: RefCell::new(Instant::now() - min_interval),
        min_interval,
        brightness,
        got_frame: RefCell::new(false),
    });

    let _listener = stream
        .add_local_listener_with_user_data(Rc::clone(&state))
        .state_changed(|_stream, _state, old, new| {
            if let pw::stream::StreamState::Error(msg) = new {
                eprintln!("pipewire stream: {old:?} -> Error({msg:?})");
            } else {
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
                eprintln!("failed to parse video format");
                return;
            }

            let fmt = info.format();
            let size = info.size();
            let stride = negotiate::stride_for(fmt, size.width);
            state.width.store(size.width, Ordering::Relaxed);
            state.height.store(size.height, Ordering::Relaxed);
            *state.format.borrow_mut() = Some(fmt);

            eprintln!(
                "negotiated format {:?} {}x{} stride {}",
                fmt, size.width, size.height, stride
            );

            let buf_bytes = negotiate::buffer_params_bytes(stride, size.height);
            let buf_pod = pw::spa::pod::Pod::from_bytes(&buf_bytes).expect("buffer pod");
            let mut params = [buf_pod];
            if let Err(e) = stream.update_params(&mut params) {
                eprintln!("update_params failed: {e}");
            }
        })
        .process({
            let w = width.clone();
            let h = height.clone();
            move |stream, state| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };

                let now = Instant::now();
                if now.duration_since(*state.last_frame.borrow()) < state.min_interval {
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

                let mut rgb = sample::sample_edges(datas, format, width, height, &state.zones);
                scale_rgb_buf(&mut rgb, state.brightness);
                let leds = state.device.borrow().leds;
                if let Ok(packet) = build_frame(leds, &rgb) {
                    if state.writer.borrow_mut().write_frame(&packet).is_ok() {
                        if !*state.got_frame.borrow() {
                            eprintln!("first frame sent to LEDs");
                            *state.got_frame.borrow_mut() = true;
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
        "hyper-sync screen: target.object={target} (portal node {node_id}) {}x{} @ {fps}fps",
        width.load(Ordering::Relaxed),
        height.load(Ordering::Relaxed),
    );

    mainloop.run();
    Ok(())
}

pub fn run_controlled(
    writer: Arc<Mutex<SerialWriter>>,
    config: Arc<RwLock<RuntimeConfig>>,
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<DaemonStatus>>,
    _preview: Arc<Mutex<Vec<u8>>>,
) -> anyhow::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }

    pw::init();

    let (layout_path, monitor, forget_portal) = {
        let cfg = config.read().unwrap();
        (
            config::resolve_layout_path(Path::new(&cfg.screen.layout)),
            cfg.screen.monitor,
            cfg.screen.forget_portal,
        )
    };
    if forget_portal {
        config::clear_portal_token()?;
        config.write().unwrap().screen.forget_portal = false;
    }
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

    let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    let (portal_stream, pw_fd) = rt.block_on(open_portal(monitor, false))?;
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }

    let (init_w, init_h) = portal_stream.size().unwrap_or((1920, 1080));
    let width = Arc::new(AtomicU32::new(init_w.max(1) as u32));
    let height = Arc::new(AtomicU32::new(init_h.max(1) as u32));
    let node_id = portal_stream.pipe_wire_node_id();

    let mainloop = pw::main_loop::MainLoopBox::new(None).context("pipewire mainloop")?;
    let mainloop_ptr = mainloop.as_raw_ptr();
    let context = pw::context::ContextBox::new(mainloop.loop_(), None).context("pipewire context")?;
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

    let stream = pw::stream::StreamBox::new(&core, "hyper-sync", props).context("pipewire stream")?;

    let fps = config.read().unwrap().effect.fps;
    let min_interval = Duration::from_micros(1_000_000 / u64::from(fps.max(1)));
    let state = Rc::new(CtrlFrameState {
        writer,
        config: Arc::clone(&config),
        zones: Rc::new(zones),
        format: RefCell::new(None),
        width: width.clone(),
        height: height.clone(),
        last_frame: RefCell::new(Instant::now() - min_interval),
        got_frame: RefCell::new(false),
        status,
        cancel: Arc::clone(&cancel),
        mainloop_ptr,
    });

    let cancel_watch = Arc::clone(&cancel);
    let _cancel_timer = mainloop.loop_().add_timer(move |_| {
        if cancel_watch.load(Ordering::Relaxed) {
            unsafe { pw::sys::pw_main_loop_quit(mainloop_ptr) };
        }
    });
    let _ = _cancel_timer.update_timer(Some(Duration::from_millis(50)), None);

    let _listener = stream
        .add_local_listener_with_user_data(Rc::clone(&state))
        .state_changed(|_stream, _state, old, new| {
            if let pw::stream::StreamState::Error(msg) = new {
                eprintln!("pipewire stream: {old:?} -> Error({msg:?})");
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
                if state.cancel.load(Ordering::Relaxed)
                    || state.config.read().unwrap().effect.mode
                        != crate::config::EffectMode::Screen
                {
                    unsafe { pw::sys::pw_main_loop_quit(state.mainloop_ptr) };
                    return;
                }

                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };

                let cfg = state.config.read().unwrap();
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

                let mut rgb = sample::sample_edges(datas, format, width, height, &state.zones);
                scale_rgb_buf(&mut rgb, cfg.effect.brightness);
                let leds = cfg.device.leds;
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

    mainloop.run();
    Ok(())
}

async fn open_portal(
    monitor: u32,
    forget_portal: bool,
) -> anyhow::Result<(ashpd::desktop::screencast::Stream, std::os::fd::OwnedFd)> {
    if forget_portal {
        config::clear_portal_token()?;
        eprintln!("cleared saved portal permission");
    }

    let saved_token = config::load_portal_token();
    if saved_token.is_some() {
        eprintln!("restoring screencast session…");
    } else {
        eprintln!("requesting screen capture permission…");
    }

    let proxy = Screencast::new()
        .await
        .context("create ScreenCast proxy")?;
    let session = proxy
        .create_session()
        .await
        .context("create screencast session")?;

    proxy
        .select_sources(
            &session,
            CursorMode::Embedded,
            SourceType::Monitor.into(),
            false,
            saved_token.as_deref(),
            PersistMode::ExplicitlyRevoked,
        )
        .await
        .context("select_sources")?
        .response()
        .context("select_sources response")?;

    let request = proxy
        .start(&session, &WindowIdentifier::default())
        .await
        .context("start screencast")?;
    let response = request.response().context("start response")?;

    if let Some(token) = response.restore_token() {
        config::save_portal_token(token)?;
        if saved_token.is_some() {
            eprintln!("screencast session restored");
        } else {
            eprintln!("screencast permission saved");
        }
    }

    let streams = response.streams();
    let stream = streams
        .get(monitor as usize)
        .or_else(|| streams.first())
        .context("no screencast stream returned")?
        .to_owned();

    let fd = proxy
        .open_pipe_wire_remote(&session)
        .await
        .context("open_pipe_wire_remote")?;

    Ok((stream, fd))
}
