#[cfg(feature = "screen")]
mod capture;
mod config;
#[cfg(feature = "daemon")]
mod daemon;
mod effects;
mod protocol;
mod serial;
#[cfg(feature = "tui")]
mod tui;
#[cfg(feature = "tray")]
mod tray;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use config::DeviceConfig;

#[cfg(feature = "screen")]
use anyhow::Context;

#[cfg(feature = "daemon")]
use daemon::{daemon_running, ipc_request, patch_config, write_default_config_if_missing, IpcRequest, IpcResponse};

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
    /// Solid color on all LEDs
    Solid {
        #[arg(long, default_value = config::DEFAULT_PORT)]
        port: String,
        #[arg(long, default_value_t = config::DEFAULT_LEDS)]
        leds: u8,
        #[arg(long, default_value = "ff3300")]
        color: String,
        #[arg(long, default_value_t = 0.8)]
        brightness: f32,
        #[arg(long, default_value_t = config::DEFAULT_FPS)]
        fps: u32,
    },
    /// Turn all LEDs off
    Off {
        #[arg(long, default_value = config::DEFAULT_PORT)]
        port: String,
        #[arg(long, default_value_t = config::DEFAULT_LEDS)]
        leds: u8,
    },
    /// Candle flicker effect
    Candle {
        #[arg(long, default_value = config::DEFAULT_PORT)]
        port: String,
        #[arg(long, default_value_t = config::DEFAULT_LEDS)]
        leds: u8,
        #[arg(long, default_value_t = 0.9)]
        warmth: f32,
        #[arg(long, default_value_t = 1.0)]
        speed: f32,
        #[arg(long, default_value_t = config::DEFAULT_FPS)]
        fps: u32,
    },
    /// Screen edge sync via PipeWire portal
    Screen {
        #[arg(long, default_value = config::DEFAULT_PORT)]
        port: String,
        #[arg(long, default_value_t = config::DEFAULT_LEDS)]
        leds: u8,
        #[arg(long, default_value_t = 0.8)]
        brightness: f32,
        #[arg(long, default_value_t = config::DEFAULT_FPS)]
        fps: u32,
        #[arg(long, default_value = "0")]
        monitor: u32,
        #[arg(long, default_value = "config/layout.toml")]
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
    Set {
        #[arg(long)]
        mode: Option<String>,
        #[arg(long)]
        brightness: Option<f32>,
        #[arg(long)]
        color: Option<String>,
        #[arg(long)]
        fps: Option<u32>,
        #[arg(long)]
        speed: Option<f32>,
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
        Commands::Solid {
            port,
            leds,
            color,
            brightness,
            fps,
        } => {
            #[cfg(feature = "daemon")]
            if try_ipc_legacy(|| {
                patch_config(IpcRequest::Patch {
                    mode: Some("solid".into()),
                    brightness: Some(brightness),
                    color: Some(color.clone()),
                    fps: Some(fps),
                    speed: None,
                })
            })? {
                return Ok(());
            }
            let rgb = effects::solid::parse_color(&color)?;
            effects::solid::run(
                DeviceConfig {
                    port,
                    baud: config::DEFAULT_BAUD,
                    leds,
                },
                rgb,
                brightness,
                fps,
            )
        }
        Commands::Off { port, leds } => {
            #[cfg(feature = "daemon")]
            if try_ipc_legacy(|| patch_config(IpcRequest::Stop))? {
                return Ok(());
            }
            effects::solid::run_off(DeviceConfig {
                port,
                baud: config::DEFAULT_BAUD,
                leds,
            })
        }
        Commands::Candle {
            port,
            leds,
            warmth,
            speed,
            fps,
        } => {
            #[cfg(feature = "daemon")]
            if try_ipc_legacy(|| {
                patch_config(IpcRequest::Patch {
                    mode: Some("candle".into()),
                    brightness: None,
                    color: None,
                    fps: Some(fps),
                    speed: Some(speed),
                })
            })? {
                return Ok(());
            }
            effects::candle::run(
                DeviceConfig {
                    port,
                    baud: config::DEFAULT_BAUD,
                    leds,
                },
                warmth,
                speed,
                fps,
            )
        }
        Commands::Screen {
            port,
            leds,
            brightness,
            fps,
            monitor,
            layout,
            forget_portal,
        } => {
            #[cfg(feature = "daemon")]
            if try_ipc_legacy(|| {
                patch_config(IpcRequest::Patch {
                    mode: Some("screen".into()),
                    brightness: Some(brightness),
                    color: None,
                    fps: Some(fps),
                    speed: None,
                })
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
                    DeviceConfig {
                        port,
                        baud: config::DEFAULT_BAUD,
                        leds,
                    },
                    layout_path,
                    fps,
                    monitor,
                    brightness,
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
                    "effect={} brightness={:.2} fps={} serial_ok={} detail={}",
                    st.effect, st.brightness, st.fps, st.serial_ok, st.detail
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
        CtlAction::Quit => {
            ipc_request(&IpcRequest::Quit)?;
            Ok(())
        }
        CtlAction::Set {
            mode,
            brightness,
            color,
            fps,
            speed,
        } => {
            ipc_request(&IpcRequest::Patch {
                mode,
                brightness,
                color,
                fps,
                speed,
            })?;
            Ok(())
        }
    }
}
