/// Skydimo serial frame: `Ada` + 0x00 0x00 + led_count + RGB payload.
pub fn build_frame(led_count: u8, rgb: &[u8]) -> anyhow::Result<Vec<u8>> {
    let expected = usize::from(led_count) * 3;
    anyhow::ensure!(
        rgb.len() == expected,
        "rgb payload length {} != {} (led_count * 3)",
        rgb.len(),
        expected
    );

    let mut frame = Vec::with_capacity(6 + rgb.len());
    frame.extend_from_slice(b"Ada");
    frame.push(0x00);
    frame.push(0x00);
    frame.push(led_count);
    frame.extend_from_slice(rgb);
    Ok(frame)
}

pub fn black_frame(led_count: u8) -> Vec<u8> {
    let len = usize::from(led_count) * 3;
    build_frame(led_count, &vec![0u8; len]).expect("black frame always valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_for_65_leds() {
        let frame = build_frame(65, &vec![0u8; 195]).unwrap();
        assert_eq!(&frame[..6], &[0x41, 0x64, 0x61, 0x00, 0x00, 0x41]);
        assert_eq!(frame.len(), 201);
    }

    #[cfg(feature = "screen")]
    #[test]
    fn layout_toml_parses() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/layout.toml");
        let layout = crate::config::LayoutConfig::load(&path).unwrap();
        assert_eq!(layout.led_count, 65);
        assert_eq!(layout.origin, "bottom_right");
        assert_eq!(layout.segments.len(), 3);
        assert_eq!(layout.sample_points().len(), 65);
    }
}
