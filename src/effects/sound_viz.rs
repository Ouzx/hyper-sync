use crate::audio::SPECTRUM_BANDS;
use crate::effects::solid::resolve_pixel_color;
use crate::effects::Ctx;
use crate::layout::segment_bounds;

/// Bass on the right side, mids on top, highs on the left; each LED shows its band's energy.
pub fn render(c: &mut Ctx) -> Vec<u8> {
    let n = c.n;
    let bands = c.audio.spectrum();
    let per_zone = [
        SPECTRUM_BANDS / 3,
        SPECTRUM_BANDS / 3,
        SPECTRUM_BANDS - 2 * (SPECTRUM_BANDS / 3),
    ];
    let mut rgb = vec![0u8; n * 3];
    for (zi, (start, end)) in segment_bounds(n).into_iter().enumerate() {
        let band_start: usize = per_zone[..zi].iter().sum();
        for i in start..end.min(n) {
            let local = (i - start) as f32 / (end - start).saturating_sub(1).max(1) as f32;
            let band = band_start + (local * per_zone[zi].saturating_sub(1) as f32).round() as usize;
            let energy = bands[band.min(SPECTRUM_BANDS - 1)].clamp(0.0, 1.0);
            let color = resolve_pixel_color(i, n, &c.cfg.solid.color);
            for k in 0..3 {
                rgb[i * 3 + k] = (f32::from(color[k]) * energy) as u8;
            }
        }
    }
    rgb
}
