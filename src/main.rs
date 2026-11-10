#[cfg(feature = "screen")]
mod capture;
mod config;
#[cfg(feature = "daemon")]
mod daemon;
mod device;
mod effects;
mod install;
mod layout;
mod serial;
mod audio;
#[cfg(feature = "tui")]
mod tui;
#[cfg(feature = "tray")]
mod tray;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use config::DeviceSection;

#[cfg(feature = "screen")]
use anyhow::Context;

#[cfg(feature = "daemon")]
use daemon::{daemon_running, ipc_request, patch_config, write_default_config_if_missing, IpcRequest, IpcResponse, Patch};

#[derive(Parser)]
#[command(name = "hyper-sync", about = "Low-latency Skydimo LED sync")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Background daemon (tray icon by default)
    #[cfg(feature = "daemon")]
    Daemon {
        #[arg(long, help = "Do not show system tray icon")]
        no_tray: bool,
    },
    /// Interactive control panel (daemon keeps running on quit)
    #[cfg(feature = "tui")]
    Tui,
    /// Control the running daemon
    #[cfg(feature = "daemon")]
    Ctl {
        #[command(subcommand)]
        action: CtlAction,
    },
    /// Quit the running daemon (alias for `ctl quit`)
    #[cfg(feature = "daemon")]
    Quit,
    /// Install autostart desktop entry or systemd user service
    Install {
        #[command(subcommand)]
        target: install::InstallTarget,
    },
    /// Remove autostart desktop entry or systemd user service
    Uninstall {
        #[command(subcommand)]
        target: install::InstallTarget,
    },
    // Direct modes: with the daemon running they change its settings (only the options you pass);
    // without it they drive the LEDs themselves, on the device from config.toml.
    /// Solid color on all LEDs
    Solid {
        /// Serial port [default: config.toml's]
        #[arg(long)]
        port: Option<String>,
        /// LED count [default: config.toml's]
        #[arg(long)]
        leds: Option<u8>,
        /// [default: ff3300]
        #[arg(long)]
        color: Option<String>,
        /// [default: 0.8]
        #[arg(long)]
        brightness: Option<f32>,
        /// [default: 30]
        #[arg(long)]
        fps: Option<u32>,
    },
    /// Turn all LEDs off
    Off {
        /// Serial port [default: config.toml's]
        #[arg(long)]
        port: Option<String>,
        /// LED count [default: config.toml's]
        #[arg(long)]
        leds: Option<u8>,
    },
    /// Candle flicker effect
    Candle {
        /// Serial port [default: config.toml's]
        #[arg(long)]
        port: Option<String>,
        /// LED count [default: config.toml's]
        #[arg(long)]
        leds: Option<u8>,
        /// 0 = accent color, 1 = candle amber [default: 0.9]
        #[arg(long)]
        warmth: Option<f32>,
        /// [default: 1.0]
        #[arg(long)]
        speed: Option<f32>,
        /// [default: 30]
        #[arg(long)]
        fps: Option<u32>,
    },
    /// Screen edge sync via PipeWire portal
    Screen {
        /// Serial port [default: config.toml's]
        #[arg(long)]
        port: Option<String>,
        /// LED count [default: config.toml's]
        #[arg(long)]
        leds: Option<u8>,
        /// [default: 0.8]
        #[arg(long)]
        brightness: Option<f32>,
        /// [default: 30]
        #[arg(long)]
        fps: Option<u32>,
        /// Without the daemon only (it uses config.toml's)
        #[arg(long, default_value = "0")]
        monitor: u32,
        /// Without the daemon only; relative paths look in ~/.config/hyper-sync first
        #[arg(long, default_value = "layout.toml")]
        layout: PathBuf,
        #[arg(long, help = "Clear saved portal permission and re-prompt")]
        forget_portal: bool,
    },
}

#[cfg(feature = "daemon")]
#[derive(Subcommand)]
enum CtlAction {
    Status {
        #[arg(long)]
        json: bool,
    },
    Stop,
    Restart,
    Quit,
    /// Clear saved screen-capture permission (re-prompt next time screen sync starts)
    ReselectScreen,
    /// Blink a color over the current effect, e.g. `ctl flash 00ff00 --count 3`
    Flash {
        color: String,
        #[arg(long, default_value_t = 3)]
        count: u32,
    },
    Set {
        #[arg(long)]
        mode: Option<String>,
        #[arg(long)]
        brightness: Option<f32>,
        #[arg(long)]
        color: Option<String>,
        /// Frames per second, 1 to 55 (the serial line's limit)
        #[arg(long)]
        fps: Option<u32>,
        #[arg(long)]
        speed: Option<f32>,
        /// Candle: 0 = accent color, 1 = candle amber
        #[arg(long)]
        warmth: Option<f32>,
        /// Screen sync gamma, 1.0 (off) to 3.0
        #[arg(long)]
        gamma: Option<f32>,
        /// Effect strength: 0 = steady, 0.5 = as designed, 1 = strongest
        #[arg(long)]
        power: Option<f32>,
        /// Screen sync smoothing, 0 (off) to 1 (~0.6 s fade)
        #[arg(long)]
        smoothing: Option<f32>,
        /// Sound reaction smoothing, 0 (snappy) to 1 (smooth)
        #[arg(long)]
        audio_smoothing: Option<f32>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        #[cfg(feature = "daemon")]
        Commands::Daemon { no_tray } => {
            write_default_config_if_missing()?;
            daemon::run(!no_tray)
        }
        #[cfg(feature = "tui")]
        Commands::Tui => tui::run(),
        #[cfg(feature = "daemon")]
        Commands::Quit => run_ctl(CtlAction::Quit),
        #[cfg(feature = "daemon")]
        Commands::Ctl { action } => run_ctl(action),
        Commands::Install { target } => install::run_install(target),
        Commands::Uninstall { target } => install::run_uninstall(target),
        Commands::Solid { port, leds, color, brightness, fps } => {
            #[cfg(feature = "daemon")]
            if try_ipc_legacy(|| {
                patch_config(IpcRequest::Patch(Patch {
                    mode: Some("solid".into()),
                    brightness,
                    color: color.clone(),
                    fps,
                    ..Default::default()
                }))
            })? {
                return Ok(());
            }
            let rgb = effects::solid::parse_color(color.as_deref().unwrap_or("ff3300"))?;
            effects::solid::run(
                &device(port, leds),
                rgb,
                brightness.unwrap_or(0.8),
                fps.unwrap_or(config::DEFAULT_FPS),
            )
        }
        Commands::Off { port, leds } => {
            #[cfg(feature = "daemon")]
            if try_ipc_legacy(|| patch_config(IpcRequest::Stop))? {
                return Ok(());
            }
            effects::solid::run_off(&device(port, leds))
        }
        Commands::Candle { port, leds, warmth, speed, fps } => {
            #[cfg(feature = "daemon")]
            if try_ipc_legacy(|| {
                patch_config(IpcRequest::Patch(Patch {
                    mode: Some("candle".into()),
                    warmth,
                    speed,
                    fps,
                    ..Default::default()
                }))
            })? {
                return Ok(());
            }
            effects::candle::run(
                device(port, leds),
                warmth.unwrap_or(0.9),
                speed.unwrap_or(1.0),
                fps.unwrap_or(config::DEFAULT_FPS),
            )
        }
        Commands::Screen { port, leds, brightness, fps, monitor, layout, forget_portal } => {
            #[cfg(feature = "daemon")]
            if try_ipc_legacy(|| {
                let resp = patch_config(IpcRequest::Patch(Patch {
                    mode: Some("screen".into()),
                    brightness,
                    fps,
                    ..Default::default()
                }))?;
                if forget_portal {
                    return ipc_request(&IpcRequest::ReselectScreen);
                }
                Ok(resp)
            })? {
                return Ok(());
            }
            #[cfg(feature = "screen")]
            {
                let layout_path = config::resolve_layout_path(layout.as_path());
                let layout_path = layout_path
                    .to_str()
                    .context("layout path is not valid UTF-8")?;
                eprintln!("using layout {layout_path}");
                capture::screen::run(
                    device(port, leds),
                    layout_path,
                    fps.unwrap_or(config::DEFAULT_FPS),
                    monitor,
                    brightness.unwrap_or(0.8),
                    forget_portal,
                )
            }
            #[cfg(not(feature = "screen"))]
            {
                let _ = (port, leds, brightness, fps, monitor, layout, forget_portal);
                anyhow::bail!(
                    "screen mode requires building with --features screen (needs pipewire-devel + gcc)"
                )
            }
        }
    }
}

/// The device from config.toml, with `--port` / `--leds` on top.
fn device(port: Option<String>, leds: Option<u8>) -> DeviceSection {
    let mut device = config::RuntimeConfig::load(&config::runtime_config_path()).unwrap_or_default().device;
    if let Some(port) = port {
        device.port = port;
    }
    if let Some(leds) = leds {
        device.leds = leds;
    }
    device
}

#[cfg(feature = "daemon")]
fn try_ipc_legacy(f: impl FnOnce() -> anyhow::Result<IpcResponse>) -> anyhow::Result<bool> {
    if !daemon_running() {
        return Ok(false);
    }
    f()?;
    Ok(true)
}

#[cfg(feature = "daemon")]
fn run_ctl(action: CtlAction) -> anyhow::Result<()> {
    match action {
        CtlAction::Status { json } => {
            let resp = ipc_request(&IpcRequest::Status)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&resp)?);
            } else if let Some(st) = resp.status {
                println!(
                    "effect={} brightness={:.2} fps={} actual_fps={:.1} serial_ok={} detail={}",
                    st.effect, st.brightness, st.fps, st.actual_fps, st.serial_ok, st.detail
                );
                if let Some(err) = st.last_error {
                    println!("last_error={err}");
                }
            }
            Ok(())
        }
        CtlAction::Stop => {
            ipc_request(&IpcRequest::Stop)?;
            Ok(())
        }
        CtlAction::Restart => {
            ipc_request(&IpcRequest::Restart)?;
            Ok(())
        }
        CtlAction::ReselectScreen => {
            ipc_request(&IpcRequest::ReselectScreen)?;
            Ok(())
        }
        CtlAction::Quit => {
            ipc_request(&IpcRequest::Quit)?;
            Ok(())
        }
        CtlAction::Flash { color, count } => {
            ipc_request(&IpcRequest::Flash { color, count })?;
            Ok(())
        }
        CtlAction::Set {
            mode,
            brightness,
            color,
            fps,
            speed,
            warmth,
            gamma,
            power,
            smoothing,
            audio_smoothing,
        } => {
            ipc_request(&IpcRequest::Patch(Patch {
                mode,
                brightness,
                color,
                fps,
                speed,
                warmth,
                gamma,
                power,
                smoothing,
                audio_smoothing,
                ..Default::default()
            }))?;
            Ok(())
        }
    }
}
