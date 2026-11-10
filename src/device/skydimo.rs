//! Skydimo controllers (model SK0127, CH340 USB serial): 115200 8N1, the only rate they answer.
//! A frame is Adalight without the checksum: `Ada`, 0x00 0x00, the LED count (not count − 1), then
//! R,G,B per LED. 201 bytes for 65 LEDs, ~17.4 ms on the wire.

use anyhow::Context;

use super::Device;
use crate::serial::SerialWriter;

const BAUD: u32 = 115_200;

pub struct Skydimo(SerialWriter);

impl Skydimo {
    pub fn new(port: &str) -> Self {
        Self(SerialWriter::new(port, BAUD))
    }
}

impl Device for Skydimo {
    fn show(&mut self, rgb: &[u8]) -> anyhow::Result<()> {
        self.0.write_frame(&frame(rgb)?)
    }
}

fn frame(rgb: &[u8]) -> anyhow::Result<Vec<u8>> {
    let count = u8::try_from(rgb.len() / 3).context("a Skydimo frame holds at most 255 LEDs")?;
    let mut frame = Vec::with_capacity(6 + rgb.len());
    frame.extend_from_slice(b"Ada\0\0");
    frame.push(count);
    frame.extend_from_slice(rgb);
    Ok(frame)
}

#[cfg(test)]
mod tests {
    #[test]
    fn header_for_65_leds() {
        let frame = super::frame(&[0u8; 195]).unwrap();
        assert_eq!(&frame[..6], &[0x41, 0x64, 0x61, 0x00, 0x00, 0x41]);
        assert_eq!(frame.len(), 201);
    }
}
