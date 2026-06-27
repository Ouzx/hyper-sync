use std::io::Write;
use std::thread;
use std::time::Duration;

use anyhow::Context;
use serialport::{DataBits, FlowControl, Parity, StopBits};

use crate::config::DeviceConfig;

pub struct SerialWriter {
    cfg: DeviceConfig,
    port: Option<Box<dyn serialport::SerialPort>>,
}

impl SerialWriter {
    pub fn new(cfg: DeviceConfig) -> Self {
        Self { cfg, port: None }
    }

    pub fn write_frame(&mut self, frame: &[u8]) -> anyhow::Result<()> {
        loop {
            if self.port.is_none() {
                self.reconnect()?;
            }

            match self.port.as_mut().unwrap().write_all(frame) {
                Ok(()) => {
                    self.port.as_mut().unwrap().flush()?;
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("serial write failed: {e}, reconnecting...");
                    self.port = None;
                    thread::sleep(Duration::from_secs(2));
                }
            }
        }
    }

    fn reconnect(&mut self) -> anyhow::Result<()> {
        let port = serialport::new(&self.cfg.port, self.cfg.baud)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(StopBits::One)
            .flow_control(FlowControl::None)
            .timeout(Duration::from_millis(100))
            .open()
            .with_context(|| format!("open serial port {}", self.cfg.port))?;
        self.port = Some(port);
        Ok(())
    }
}
