use std::io::Write;
use std::thread;
use std::time::Duration;

use anyhow::Context;
use serialport::{DataBits, FlowControl, Parity, StopBits};

/// A serial port that reopens itself after a write fails (USB unplugged and plugged back).
pub struct SerialWriter {
    path: String,
    baud: u32,
    port: Option<Box<dyn serialport::SerialPort>>,
}

impl SerialWriter {
    pub fn new(path: &str, baud: u32) -> Self {
        Self { path: path.into(), baud, port: None }
    }

    pub fn write_frame(&mut self, frame: &[u8]) -> anyhow::Result<()> {
        loop {
            if self.port.is_none() {
                self.reconnect()?;
            }

            let port = self.port.as_mut().unwrap();
            // No flush: waiting for the wire (tcdrain) woke the thread ~8 times a frame. Callers
            // pace frames at no more than MAX_FPS, so the tty buffer never fills.
            match port.write_all(frame) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    eprintln!("serial write failed: {e}, reconnecting...");
                    self.port = None;
                    thread::sleep(Duration::from_secs(2));
                }
            }
        }
    }

    fn reconnect(&mut self) -> anyhow::Result<()> {
        let port = serialport::new(&self.path, self.baud)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(StopBits::One)
            .flow_control(FlowControl::None)
            .timeout(Duration::from_millis(100))
            .open()
            .with_context(|| format!("open serial port {}", self.path))?;
        self.port = Some(port);
        Ok(())
    }
}
