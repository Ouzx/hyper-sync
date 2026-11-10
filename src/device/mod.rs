//! Devices: what puts a frame of colors on the LEDs.
//!
//! Effects and screen sync render one frame at a time: R,G,B bytes per LED, in strip order (LED 0
//! first, see config/layout.toml), with brightness, gamma, sound and flash already applied. A device
//! only has to get that frame onto its LEDs, in its own protocol, over its own link.
//!
//! Adding a device:
//! 1. Write `src/device/<name>.rs` with a type that implements [`Device`]. `skydimo.rs` is the
//!    example; a serial device can reuse [`crate::serial::SerialWriter`], which reconnects on replug.
//! 2. Add a variant to [`Driver`] and its line to [`open`].
//! 3. Select it in config.toml: `[device] driver = "<name>"`, with `port` and `leds` as it needs.
//!
//! Limits today: at most 255 LEDs (`leds` is a u8), at most `config::MAX_FPS` frames a second, and
//! the effects draw on the U-shaped strip of `layout.rs` (right, top, left).

mod skydimo;

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::config::DeviceSection;

pub trait Device: Send {
    /// Show one frame: R,G,B per LED in strip order. Called once per frame (up to `MAX_FPS` a
    /// second), so don't wait on the hardware. Return an error when the frame didn't get through;
    /// connect lazily and reconnect on a later call.
    fn show(&mut self, rgb: &[u8]) -> anyhow::Result<()>;
}

/// The device, shared by the effect thread and screen sync.
pub type SharedDevice = Arc<Mutex<Box<dyn Device>>>;

/// `[device] driver` in config.toml.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Driver {
    /// Skydimo controller on USB serial (`port`).
    #[default]
    Skydimo,
    /// No hardware: frames only show in the TUI preview. For trying hyper-sync, working on
    /// effects without a strip, or recording demos.
    None,
}

pub fn open(cfg: &DeviceSection) -> Box<dyn Device> {
    match cfg.driver {
        Driver::Skydimo => Box::new(skydimo::Skydimo::new(&cfg.port)),
        Driver::None => Box::new(NoDevice),
    }
}

struct NoDevice;

impl Device for NoDevice {
    fn show(&mut self, _rgb: &[u8]) -> anyhow::Result<()> {
        Ok(())
    }
}
