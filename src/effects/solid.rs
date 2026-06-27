use crate::config::DeviceConfig;
use crate::protocol::build_frame;
use crate::serial::SerialWriter;

pub fn parse_color(hex: &str) -> anyhow::Result<[u8; 3]> {
    let s = hex.trim_start_matches('#');
    anyhow::ensure!(s.len() == 6, "color must be 6 hex digits, got {s}");
    Ok([
        u8::from_str_radix(&s[0..2], 16)?,
        u8::from_str_radix(&s[2..4], 16)?,
        u8::from_str_radix(&s[4..6], 16)?,
    ])
}

pub fn scale_rgb(rgb: [u8; 3], brightness: f32) -> [u8; 3] {
    let b = brightness.clamp(0.0, 1.0);
    [
        (f32::from(rgb[0]) * b) as u8,
        (f32::from(rgb[1]) * b) as u8,
        (f32::from(rgb[2]) * b) as u8,
    ]
}

pub fn scale_rgb_buf(rgb: &mut [u8], brightness: f32) {
    let b = brightness.clamp(0.0, 1.0);
    for c in rgb.iter_mut() {
        *c = (f32::from(*c) * b) as u8;
    }
}

pub fn run(
    cfg: DeviceConfig,
    color: [u8; 3],
    brightness: f32,
    fps: u32,
) -> anyhow::Result<()> {
    let scaled = scale_rgb(color, brightness);
    let n = usize::from(cfg.leds);
    let rgb: Vec<u8> = scaled
        .iter()
        .cycle()
        .take(n * 3)
        .copied()
        .collect();

    let frame = build_frame(cfg.leds, &rgb)?;
    let mut writer = SerialWriter::new(cfg);
    let interval = std::time::Duration::from_micros(1_000_000 / u64::from(fps.max(1)));

    loop {
        writer.write_frame(&frame)?;
        std::thread::sleep(interval);
    }
}

pub fn run_off(cfg: DeviceConfig) -> anyhow::Result<()> {
    let frame = crate::protocol::black_frame(cfg.leds);
    let mut writer = SerialWriter::new(cfg);
    writer.write_frame(&frame)?;
    Ok(())
}
