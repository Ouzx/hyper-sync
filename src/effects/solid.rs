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

pub fn is_rainbow_color(s: &str) -> bool {
    s.eq_ignore_ascii_case("rainbow")
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.fract().max(0.0);
    let i = (h * 6.0).floor() as i32;
    let f = h * 6.0 - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [
        (r * 255.0) as u8,
        (g * 255.0) as u8,
        (b * 255.0) as u8,
    ]
}

pub fn rgb_to_hsv(rgb: [u8; 3]) -> (f32, f32, f32) {
    let r = f32::from(rgb[0]) / 255.0;
    let g = f32::from(rgb[1]) / 255.0;
    let b = f32::from(rgb[2]) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if (max - r).abs() < 1e-6 {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if (max - g).abs() < 1e-6 {
        (((b - r) / d) + 2.0) / 6.0
    } else {
        (((r - g) / d) + 4.0) / 6.0
    };
    let s = if max < 1e-6 { 0.0 } else { d / max };
    (h, s, max)
}

#[cfg(feature = "tui")]
pub fn rgb_to_hex(rgb: [u8; 3]) -> String {
    format!("{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// Fixed hue per LED index along the strip (not animated).
pub fn rainbow_pixel(i: usize, n: usize) -> [u8; 3] {
    let hue = i as f32 / n.max(1) as f32;
    hsv_to_rgb(hue, 1.0, 1.0)
}

pub fn resolve_pixel_color(i: usize, n: usize, accent: &str) -> [u8; 3] {
    if is_rainbow_color(accent) {
        rainbow_pixel(i, n)
    } else {
        parse_color(accent).unwrap_or([255, 51, 0])
    }
}

pub fn scale_rgb(rgb: [u8; 3], brightness: f32) -> [u8; 3] {
    let b = brightness.clamp(0.0, 1.0);
    [
        (f32::from(rgb[0]) * b) as u8,
        (f32::from(rgb[1]) * b) as u8,
        (f32::from(rgb[2]) * b) as u8,
    ]
}

#[cfg(feature = "screen")]
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
    let interval = crate::config::frame_interval(fps);

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

pub fn render(c: &mut crate::effects::Ctx) -> Vec<u8> {
    (0..c.n)
        .flat_map(|i| resolve_pixel_color(i, c.n, &c.cfg.solid.color))
        .collect()
}
