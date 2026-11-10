use std::process::{Command, Stdio};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, RgbaImage};
use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{Icon, ToolTip, Tray};

use crate::config::{runtime_config_path, EffectMode, RuntimeConfig};
use crate::daemon::{ipc_request, IpcRequest, Patch};
use crate::effects::solid::{hsv_to_rgb, parse_color};
use crate::effects::CATALOG;

static ICON_PNG: &[u8] = include_bytes!("../../assets/hyper-sync.png");

pub fn run(config: Arc<RwLock<RuntimeConfig>>) -> anyhow::Result<()> {
    let icon = image::load_from_memory(ICON_PNG)?.to_rgba8();
    let mode = config.read().unwrap().effect.mode;
    let tray = HyperTray {
        icons_on: pixmaps(&icon, false),
        icons_off: pixmaps(&icon, true),
        last_mode: if mode == EffectMode::Off { "screen".into() } else { mode.as_str().into() },
        config: Arc::clone(&config),
    };
    let handle = tray.spawn()?;
    // The menu, icon and tooltip are built from the live config; rebuild them when it changes
    // (from the TUI, ctl or a hand edit), and keep the effect a click on the icon brings back.
    let mut shown = String::new();
    loop {
        thread::sleep(Duration::from_millis(700));
        let cfg = config.read().unwrap().clone();
        let serial_ok = ipc_request(&IpcRequest::Status).ok().and_then(|r| r.status).is_none_or(|s| s.serial_ok);
        let favorites = std::fs::read_to_string(crate::config::favorites_path()).unwrap_or_default();
        let now = format!("{:?}{:?}{:?}{:?}{serial_ok}{favorites}", cfg.effect, cfg.solid, cfg.audio, cfg.candle);
        #[cfg(feature = "screen")]
        let now = format!("{now}{:?}", cfg.screen.gamma + cfg.screen.smoothing);
        if now != shown {
            shown = now;
            handle.update(|t| {
                if cfg.effect.mode != EffectMode::Off {
                    t.last_mode = cfg.effect.mode.as_str().into();
                }
            });
        }
    }
}

struct HyperTray {
    icons_on: Vec<Icon>,
    icons_off: Vec<Icon>,
    /// What a click on the icon turns back on.
    last_mode: String,
    config: Arc<RwLock<RuntimeConfig>>,
}

impl Tray for HyperTray {
    fn id(&self) -> String {
        "hyper-sync".into()
    }

    fn title(&self) -> String {
        "hyper-sync".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        if self.config.read().unwrap().effect.mode == EffectMode::Off {
            self.icons_off.clone()
        } else {
            self.icons_on.clone()
        }
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "hyper-sync".into(),
            description: format!(
                "{}\nClick: lights on/off · Scroll: brightness · Middle-click: control panel",
                summary(&self.config.read().unwrap())
            ),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        toggle(self);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        spawn_tui_in_terminal();
    }

    /// Mouse wheel over the icon: brightness up/down in 5% steps.
    fn scroll(&mut self, delta: i32, _orientation: ksni::Orientation) {
        let b = self.config.read().unwrap().effect.brightness;
        let step = if delta > 0 { 0.05 } else { -0.05 }; // positive = wheel up
        patch(Patch { brightness: Some(((b + step) * 20.0).round() / 20.0), ..Default::default() });
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let cfg = self.config.read().unwrap().clone();
        let mode = cfg.effect.mode.as_str();
        let label_of = |id: &str| CATALOG.iter().find(|e| e.0 == id).map_or(id.to_string(), |e| e.1.to_string());
        let pct = |v: f32| format!("{:.0}%", v * 100.0);

        // Off plus the TUI's favorites.
        let favorites: Vec<String> = std::iter::once("off".to_string())
            .chain(
                std::fs::read_to_string(crate::config::favorites_path())
                    .unwrap_or_default()
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| l != "off" && CATALOG.iter().any(|e| e.0 == l)),
            )
            .collect();
        let favorite_names: Vec<String> = favorites.iter().map(|f| label_of(f)).collect();

        // Every effect, one submenu per group, the current one marked.
        let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
        for (id, _, group, _) in CATALOG.iter().skip(1) {
            match groups.last_mut() {
                Some((g, ids)) if g == group => ids.push(id),
                _ => groups.push((group, vec![id])),
            }
        }
        let effect_menu = groups
            .into_iter()
            .map(|(group, ids)| {
                let names: Vec<String> = ids.iter().map(|id| label_of(id)).collect();
                let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
                let current = ids.iter().any(|i| i == mode);
                let title = if current { format!("{} — {}", capitalize(group), label_of(mode)) } else { capitalize(group) };
                submenu(&title, vec![radio(&names, ids.iter().position(|i| i == mode), move |k| Patch { mode: Some(ids[k].clone()), ..Default::default() })])
            })
            .collect();

        let colors: [(&str, &str); 8] = [
            ("Warm orange", "ff3300"),
            ("Red", "ff0000"),
            ("White", "ffffff"),
            ("Blue", "0099ff"),
            ("Green", "00ff88"),
            ("Magenta", "ff00ff"),
            ("Yellow", "ffff00"),
            ("Rainbow", "rainbow"),
        ];
        let color_now = colors.iter().find(|c| c.1 == cfg.solid.color).map_or(format!("#{}", cfg.solid.color), |c| c.0.into());
        let color_menu = MenuItem::RadioGroup(RadioGroup {
            selected: colors.iter().position(|c| c.1 == cfg.solid.color).unwrap_or(usize::MAX),
            select: Box::new(move |_, k| patch(Patch { color: Some(colors[k].1.into()), ..Default::default() })),
            options: colors.iter().map(|(name, hex)| RadioItem { label: (*name).into(), icon_data: swatch(hex), ..Default::default() }).collect(),
        });

        // Preset values: (label, value); the one nearest the current setting is marked.
        fn presets(values: &[f32], show: impl Fn(f32) -> String) -> (Vec<String>, Vec<f32>) {
            (values.iter().map(|&v| show(v)).collect(), values.to_vec())
        }
        let nearest = |values: &[f32], v: f32| values.iter().position(|&x| (x - v).abs() < 0.026);
        let slider = |title: &str, now: String, (names, values): (Vec<String>, Vec<f32>), current: f32, set: fn(f32) -> Patch| {
            let sel = nearest(&values, current);
            submenu(&format!("{title}: {now}"), vec![radio(&names, sel, move |k| set(values[k]))])
        };
        let ms = |v: f32| if v < 0.01 { "off".to_string() } else { format!("{:.0} ms", 600.0 * v * v) };
        let sound_ms = |v: f32| format!("{:.0} ms", 50.0 + 550.0 * v);
        let sound_modes = ["off", "level", "balance"];

        let mut items = vec![
            disabled(&summary(&cfg)),
            MenuItem::Checkmark(CheckmarkItem {
                label: "Lights on".into(),
                checked: cfg.effect.mode != EffectMode::Off,
                activate: Box::new(toggle),
                ..Default::default()
            }),
            MenuItem::Separator,
            submenu(
                &format!("Favorites{}", if favorites.len() == 1 { " (f in the control panel adds)" } else { "" }),
                vec![radio(&favorite_names, favorites.iter().position(|f| f == mode), move |k| Patch { mode: Some(favorites[k].clone()), ..Default::default() })],
            ),
            submenu(&format!("Effect: {}", label_of(mode)), effect_menu),
            submenu(&format!("Color: {color_now}"), vec![color_menu]),
            MenuItem::Separator,
            slider("Brightness", pct(cfg.effect.brightness), presets(&[0.1, 0.25, 0.4, 0.5, 0.75, 1.0], pct), cfg.effect.brightness, |v| Patch { brightness: Some(v), ..Default::default() }),
            slider("Speed", format!("{:.1}×", cfg.effect.speed), presets(&[0.5, 0.7, 1.0, 1.5, 2.0, 3.0], |v| format!("{v:.1}×")), cfg.effect.speed, |v| Patch { speed: Some(v), ..Default::default() }),
            slider("Power", pct(cfg.effect.power), presets(&[0.0, 0.25, 0.5, 0.75, 1.0], pct), cfg.effect.power, |v| Patch { power: Some(v), ..Default::default() }),
            slider("Frame rate", format!("{} fps", cfg.effect.fps), presets(&[24.0, 30.0, 45.0, 55.0], |v| format!("{v:.0} fps")), cfg.effect.fps as f32, |v| Patch { fps: Some(v as u32), ..Default::default() }),
            submenu(
                &format!("Sound: {}", cfg.audio.sound_mode.as_str()),
                vec![
                    radio(&["Off".into(), "Level — whole strip".into(), "Balance — left/right channel".into()], sound_modes.iter().position(|m| *m == cfg.audio.sound_mode.as_str()), move |k| Patch { sound_mode: Some(sound_modes[k].into()), ..Default::default() }),
                    MenuItem::Separator,
                    slider("Reaction", pct(cfg.audio.reactivity), presets(&[0.25, 0.5, 0.75, 1.0], pct), cfg.audio.reactivity, |v| Patch { reactivity: Some(v), ..Default::default() }),
                    slider("Sensitivity", pct(cfg.audio.sensitivity), presets(&[0.25, 0.5, 0.75, 1.0], pct), cfg.audio.sensitivity, |v| Patch { sensitivity: Some(v), ..Default::default() }),
                    slider("Smoothing", sound_ms(cfg.audio.smoothing), presets(&[0.0, 0.15, 0.3, 0.5, 0.8], sound_ms), cfg.audio.smoothing, |v| Patch { audio_smoothing: Some(v), ..Default::default() }),
                ],
            ),
        ];
        #[cfg(feature = "screen")]
        items.push(submenu(
            "Screen sync",
            vec![
                slider("Gamma", format!("{:.1}", cfg.screen.gamma), presets(&[1.0, 1.5, 1.8, 2.2, 2.6], |v| format!("{v:.1}")), cfg.screen.gamma, |v| Patch { gamma: Some(v), ..Default::default() }),
                slider("Smoothing", ms(cfg.screen.smoothing), presets(&[0.0, 0.2, 0.4, 0.6], ms), cfg.screen.smoothing, |v| Patch { smoothing: Some(v), ..Default::default() }),
                action("Choose another screen…", |_| {
                    let _ = ipc_request(&IpcRequest::ReselectScreen);
                }),
            ],
        ));
        items.extend([
            MenuItem::Separator,
            action("Open control panel", |_| spawn_tui_in_terminal()),
            action("Edit settings file…", |_| {
                let _ = Command::new("xdg-open").arg(runtime_config_path()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
            }),
            action("Restart effect", |_| {
                let _ = ipc_request(&IpcRequest::Restart);
            }),
            action("Quit hyper-sync", |_| {
                let _ = ipc_request(&IpcRequest::Quit);
            }),
        ]);
        items
    }
}

/// "Candle · 40%", plus the sound mode when it's on.
fn summary(cfg: &RuntimeConfig) -> String {
    let name = CATALOG.iter().find(|e| e.0 == cfg.effect.mode.as_str()).map_or("?", |e| e.1);
    if cfg.effect.mode == EffectMode::Off {
        return "Lights off".into();
    }
    let mut s = format!("{name} · {:.0}%", cfg.effect.brightness * 100.0);
    if cfg.audio.sound_mode.as_str() != "off" {
        s += &format!(" · sound {}", cfg.audio.sound_mode.as_str());
    }
    s
}

/// Off ↔ the last effect that was on.
fn toggle(t: &mut HyperTray) {
    let off = t.config.read().unwrap().effect.mode == EffectMode::Off;
    let mode = if off { t.last_mode.clone() } else { "off".into() };
    patch(Patch { mode: Some(mode), ..Default::default() });
}

/// The daemon answers at once (same process), so this is quick enough for a menu callback, and the
/// menu rebuilt right after already shows the change.
fn patch(p: Patch) {
    let _ = ipc_request(&IpcRequest::Patch(p));
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}

fn submenu(label: &str, submenu: Vec<MenuItem<HyperTray>>) -> MenuItem<HyperTray> {
    MenuItem::SubMenu(SubMenu { label: label.replace('_', "__"), submenu, ..Default::default() })
}

fn disabled(label: &str) -> MenuItem<HyperTray> {
    MenuItem::Standard(StandardItem { label: label.replace('_', "__"), enabled: false, ..Default::default() })
}

fn action(label: &str, f: impl Fn(&mut HyperTray) + Send + 'static) -> MenuItem<HyperTray> {
    MenuItem::Standard(StandardItem { label: label.into(), activate: Box::new(f), ..Default::default() })
}

fn radio(names: &[String], selected: Option<usize>, set: impl Fn(usize) -> Patch + Send + 'static) -> MenuItem<HyperTray> {
    MenuItem::RadioGroup(RadioGroup {
        selected: selected.unwrap_or(usize::MAX),
        select: Box::new(move |_, k| patch(set(k))),
        options: names.iter().map(|n| RadioItem { label: n.replace('_', "__"), ..Default::default() }).collect(),
    })
}

/// A small PNG colour swatch for the colour menu (a hue strip for rainbow).
fn swatch(hex: &str) -> Vec<u8> {
    let rgb = parse_color(hex).ok();
    let img = RgbaImage::from_fn(16, 16, |x, y| {
        if x == 0 || y == 0 || x == 15 || y == 15 {
            return image::Rgba([90, 90, 90, 255]);
        }
        let [r, g, b] = rgb.unwrap_or_else(|| hsv_to_rgb(x as f32 / 16.0, 1.0, 1.0));
        image::Rgba([r, g, b, 255])
    });
    let mut png = Vec::new();
    let _ = PngEncoder::new(&mut png).write_image(img.as_raw(), 16, 16, ExtendedColorType::Rgba8);
    png
}

/// Tray pixmaps at the sizes panels ask for, in the ARGB byte order the tray protocol uses
/// (the old code sent RGBA, which scrambled the colours). `off`: greyed and dimmed.
fn pixmaps(icon: &RgbaImage, off: bool) -> Vec<Icon> {
    [22u32, 32, 44, 64]
        .into_iter()
        .map(|size| {
            let small = image::imageops::resize(icon, size, size, FilterType::Lanczos3);
            let data = small
                .pixels()
                .flat_map(|p| {
                    let [r, g, b, a] = p.0;
                    if off {
                        let grey = ((0.3 * f32::from(r) + 0.59 * f32::from(g) + 0.11 * f32::from(b)) * 0.6) as u8;
                        [a, grey, grey, grey]
                    } else {
                        [a, r, g, b]
                    }
                })
                .collect();
            Icon { width: size as i32, height: size as i32, data }
        })
        .collect()
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
