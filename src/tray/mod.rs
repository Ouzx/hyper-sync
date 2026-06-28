use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use image::imageops::FilterType;
use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem, SubMenu};
use ksni::{Icon, ToolTip, Tray};

use crate::config::RuntimeConfig;
use crate::daemon::{ipc_request, IpcRequest};

pub fn run(_config: std::sync::Arc<std::sync::RwLock<RuntimeConfig>>) -> anyhow::Result<()> {
    let icons = load_icons()?;
    let tray = HyperTray {
        icons,
        last_mode: Mutex::new("screen".into()),
    };
    let _handle = tray.spawn()?;
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

struct HyperTray {
    icons: Vec<Icon>,
    last_mode: Mutex<String>,
}

impl Tray for HyperTray {
    fn id(&self) -> String {
        "hyper-sync".into()
    }

    fn title(&self) -> String {
        "hyper-sync".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.icons.clone()
    }

    fn tool_tip(&self) -> ToolTip {
        let detail = ipc_request(&IpcRequest::Status)
            .ok()
            .and_then(|r| r.status)
            .map(|s| format!("{} · {:.0}%", s.effect, s.brightness * 100.0))
            .unwrap_or_else(|| "hyper-sync".into());
        ToolTip {
            title: "hyper-sync".into(),
            description: format!("{detail}\nClick: toggle off · Middle-click: reselect screen"),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            ipc_item("Restart", IpcRequest::Restart),
            ipc_item("Stop", IpcRequest::Stop),
            ipc_item("Reselect screen capture…", IpcRequest::ReselectScreen),
            MenuItem::SubMenu(SubMenu {
                label: "Effect".into(),
                submenu: vec![
                    patch_item("Off", "off"),
                    patch_item("Screen Sync", "screen"),
                    patch_item("Screen Sync Center", "screen_center"),
                    MenuItem::SubMenu(SubMenu {
                        label: "Static".into(),
                        submenu: vec![patch_item("Solid", "solid")],
                        ..Default::default()
                    }),
                    MenuItem::SubMenu(SubMenu {
                        label: "Ambient".into(),
                        submenu: vec![
                            patch_item("Candle", "candle"),
                            patch_item("Pulse", "pulse"),
                            patch_item("Aurora", "aurora"),
                            patch_item("Fire", "fire"),
                        ],
                        ..Default::default()
                    }),
                    MenuItem::SubMenu(SubMenu {
                        label: "Motion".into(),
                        submenu: vec![
                            patch_item("Chase", "chase"),
                            patch_item("Wave", "wave"),
                            patch_item("Scanner", "scanner"),
                            patch_item("Sparkle", "sparkle"),
                            patch_item("Heartbeat", "heartbeat"),
                            patch_item("Segment", "segment"),
                            patch_item("Strobe", "strobe"),
                            patch_item("Wipe", "wipe"),
                            patch_item("Sound Viz", "sound_viz"),
                        ],
                        ..Default::default()
                    }),
                ],
                ..Default::default()
            }),
            MenuItem::SubMenu(SubMenu {
                label: "Sound".into(),
                submenu: vec![
                    sound_mode_item("Off", "off"),
                    sound_mode_item("Level", "level"),
                    sound_mode_item("Balance", "balance"),
                ],
                ..Default::default()
            }),
            MenuItem::SubMenu(SubMenu {
                label: "Brightness".into(),
                submenu: vec![
                    brightness_item("25%", 0.25),
                    brightness_item("50%", 0.5),
                    brightness_item("75%", 0.75),
                    brightness_item("100%", 1.0),
                ],
                ..Default::default()
            }),
            MenuItem::SubMenu(SubMenu {
                label: "Color".into(),
                submenu: vec![
                    color_item("Warm orange", "ff3300"),
                    color_item("Red", "ff0000"),
                    color_item("White", "ffffff"),
                    color_item("Rainbow", "rainbow"),
                ],
                ..Default::default()
            }),
            MenuItem::SubMenu(SubMenu {
                label: "Speed".into(),
                submenu: vec![
                    speed_item("Slow", 0.5),
                    speed_item("Normal", 1.0),
                    speed_item("Fast", 2.0),
                ],
                ..Default::default()
            }),
            MenuItem::Standard(StandardItem {
                label: "Open TUI".into(),
                activate: Box::new(|_| spawn_tui_in_terminal()),
                ..Default::default()
            }),
            ipc_item("Quit daemon", IpcRequest::Quit),
        ]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let current = ipc_request(&IpcRequest::Status)
            .ok()
            .and_then(|r| r.status)
            .map(|s| s.effect)
            .unwrap_or_else(|| "off".into());
        if current == "off" {
            let last = self.last_mode.lock().unwrap().clone();
            ipc_async(IpcRequest::Patch {
                mode: Some(last),
                brightness: None,
                color: None,
                fps: None,
                speed: None,
                sound_mode: None,
                reactivity: None,
                sensitivity: None,
            });
        } else {
            *self.last_mode.lock().unwrap() = current;
            ipc_async(IpcRequest::Patch {
                mode: Some("off".into()),
                brightness: None,
                color: None,
                fps: None,
                speed: None,
                sound_mode: None,
                reactivity: None,
                sensitivity: None,
            });
        }
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        ipc_async(IpcRequest::ReselectScreen);
    }
}

// ponytail: ksni menu callbacks must not block the D-Bus thread
fn ipc_async(req: IpcRequest) {
    thread::spawn(move || {
        let _ = ipc_request(&req);
    });
}

fn ipc_item(_label: &str, req: IpcRequest) -> MenuItem<HyperTray> {
    MenuItem::Standard(StandardItem {
        label: _label.into(),
        activate: Box::new(move |_| ipc_async(req.clone())),
        ..Default::default()
    })
}

fn patch_item(label: &str, mode: &str) -> MenuItem<HyperTray> {
    let mode = mode.to_string();
    MenuItem::Standard(StandardItem {
        label: label.into(),
        activate: Box::new(move |_| {
            ipc_async(IpcRequest::Patch {
                mode: Some(mode.clone()),
                brightness: None,
                color: None,
                fps: None,
                speed: None,
                sound_mode: None,
                reactivity: None,
                sensitivity: None,
            });
        }),
        ..Default::default()
    })
}

fn sound_mode_item(label: &str, mode: &str) -> MenuItem<HyperTray> {
    let mode = mode.to_string();
    MenuItem::Standard(StandardItem {
        label: label.into(),
        activate: Box::new(move |_| {
            ipc_async(IpcRequest::Patch {
                mode: None,
                brightness: None,
                color: None,
                fps: None,
                speed: None,
                sound_mode: Some(mode.clone()),
                reactivity: None,
                sensitivity: None,
            });
        }),
        ..Default::default()
    })
}

fn brightness_item(label: &str, value: f32) -> MenuItem<HyperTray> {
    MenuItem::Standard(StandardItem {
        label: label.into(),
        activate: Box::new(move |_| {
            ipc_async(IpcRequest::Patch {
                mode: None,
                brightness: Some(value),
                color: None,
                fps: None,
                speed: None,
                sound_mode: None,
                reactivity: None,
                sensitivity: None,
            });
        }),
        ..Default::default()
    })
}

fn color_item(label: &str, color: &str) -> MenuItem<HyperTray> {
    let color = color.to_string();
    MenuItem::Standard(StandardItem {
        label: label.into(),
        activate: Box::new(move |_| {
            ipc_async(IpcRequest::Patch {
                mode: Some("solid".into()),
                brightness: None,
                color: Some(color.clone()),
                fps: None,
                speed: None,
                sound_mode: None,
                reactivity: None,
                sensitivity: None,
            });
        }),
        ..Default::default()
    })
}

fn speed_item(label: &str, speed: f32) -> MenuItem<HyperTray> {
    MenuItem::Standard(StandardItem {
        label: label.into(),
        activate: Box::new(move |_| {
            ipc_async(IpcRequest::Patch {
                mode: None,
                brightness: None,
                color: None,
                fps: None,
                speed: Some(speed),
                sound_mode: None,
                reactivity: None,
                sensitivity: None,
            });
        }),
        ..Default::default()
    })
}

/// Open the TUI in a new terminal window (never attach to the daemon's stdio).
fn spawn_tui_in_terminal() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let exe = exe.to_string_lossy().into_owned();

    let try_spawn = |cmd: &mut Command| -> bool {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    };

    if let Ok(term) = std::env::var("TERMINAL") {
        let mut cmd = Command::new(&term);
        cmd.args([&exe, "tui"]);
        if try_spawn(&mut cmd) {
            return;
        }
        let mut cmd = Command::new(&term);
        cmd.args(["-e", &exe, "tui"]);
        if try_spawn(&mut cmd) {
            return;
        }
    }

    let attempts: &[(&str, &[&str])] = &[
        ("konsole", &["--new-tab", "-e"]),
        ("kgx", &["-e"]),
        ("gnome-terminal", &["--"]),
        ("xfce4-terminal", &["-e"]),
        ("alacritty", &["-e"]),
        ("foot", &["-e"]),
        ("wezterm", &["start", "--"]),
        ("xterm", &["-e"]),
    ];

    for (bin, prefix) in attempts {
        let mut args: Vec<&str> = prefix.to_vec();
        args.push(&exe);
        args.push("tui");
        let mut cmd = Command::new(bin);
        cmd.args(&args);
        if try_spawn(&mut cmd) {
            return;
        }
    }
}

fn load_icons() -> anyhow::Result<Vec<Icon>> {
    let path = icon_path();
    let img = image::open(&path).map_err(|e| anyhow::anyhow!("load icon {}: {e}", path.display()))?;
    Ok([22_u32, 44]
        .into_iter()
        .map(|size| {
            let rgba = img
                .resize_exact(size, size, FilterType::Triangle)
                .to_rgba8();
            Icon {
                width: size as i32,
                height: size as i32,
                data: rgba.into_raw(),
            }
        })
        .collect())
}

fn icon_path() -> PathBuf {
    let installed = dirs_icon_path();
    if installed.exists() {
        return installed;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let from_exe = dir.join("assets/hyper-hdr.png");
            if from_exe.exists() {
                return from_exe;
            }
        }
    }
    if PathBuf::from("assets/hyper-hdr.png").exists() {
        return PathBuf::from("assets/hyper-hdr.png");
    }
    installed
}

fn dirs_icon_path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".local/share"))
                .unwrap_or_else(|_| PathBuf::from(".local/share"))
        });
    base.join("icons/hyper-sync/hyper-hdr.png")
}
