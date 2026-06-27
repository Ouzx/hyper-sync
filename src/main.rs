#[cfg(feature = "screen")]
mod capture;
mod config;
mod effects;
mod protocol;
mod serial;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use config::DeviceConfig;

#[cfg(feature = "screen")]
use anyhow::Context;

#[derive(Parser)]
#[command(name = "hyper-sync", about = "Low-latency Skydimo LED sync")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Solid {
            port,
            leds,
            color,
            brightness,
            fps,
        } => {
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
        Commands::Off { port, leds } => effects::solid::run_off(DeviceConfig {
            port,
            baud: config::DEFAULT_BAUD,
            leds,
        }),
        Commands::Candle {
            port,
            leds,
            warmth,
            speed,
            fps,
        } => effects::candle::run(
            DeviceConfig {
                port,
                baud: config::DEFAULT_BAUD,
                leds,
            },
            warmth,
            speed,
            fps,
        ),
        Commands::Screen {
            port,
            leds,
            brightness,
            fps,
            monitor,
            layout,
            forget_portal,
        } => {
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
