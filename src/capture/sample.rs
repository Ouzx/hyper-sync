use pipewire::spa::buffer::Data;
use pipewire::spa::param::video::VideoFormat;

use super::negotiate::bytes_per_pixel;
use crate::config::EdgeZone;

type Rect = (i32, i32, i32, i32);

pub fn sample_edges(
    datas: &mut [Data],
    format: VideoFormat,
    width: u32,
    height: u32,
    zones: &[EdgeZone],
) -> Vec<u8> {
    let depth = ((width.min(height) as f32) * 0.03).clamp(16.0, 96.0) as i32;
    zones
        .iter()
        .flat_map(|z| avg_rect(datas, format, width, zone_rect(width, height, depth, z), 2))
        .collect()
}

/// Average the center region and replicate to every LED (movie-style ambient).
pub fn sample_center(
    datas: &mut [Data],
    format: VideoFormat,
    width: u32,
    height: u32,
    leds: u8,
) -> Vec<u8> {
    let (w, h) = (width as i32, height as i32);
    let (mx, my) = ((w as f32 * 0.275) as i32, (h as f32 * 0.275) as i32);
    // A color average doesn't need every pixel of a 1152×648 area — every 4th each way is plenty.
    avg_rect(datas, format, width, (mx, my, w - mx, h - my), 4).repeat(usize::from(leds))
}

fn stride(data: &Data, min: u32) -> usize {
    data.chunk().stride().unsigned_abs().max(min) as usize
}

fn avg_rect(
    datas: &mut [Data],
    format: VideoFormat,
    width: u32,
    (x0, y0, x1, y1): Rect,
    step: usize,
) -> [u8; 3] {
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    let mut add = |px: [u8; 3]| {
        for (s, c) in sum.iter_mut().zip(px) {
            *s += u64::from(c);
        }
        n += 1;
    };

    if format == VideoFormat::NV12 || format == VideoFormat::NV21 {
        if datas.len() < 2 {
            return [0, 0, 0];
        }
        let (y_plane, uv_plane) = datas.split_at_mut(1);
        let (y_stride, uv_stride) = (stride(&y_plane[0], width), stride(&uv_plane[0], width));
        let (Some(y), Some(uv)) = (y_plane[0].data(), uv_plane[0].data()) else {
            return [0, 0, 0];
        };
        for row in (y0..y1).step_by(step) {
            for col in (x0..x1).step_by(step) {
                let (row, col) = (row as usize, col as usize);
                let yo = row * y_stride + col;
                let uvo = row / 2 * uv_stride + col / 2 * 2;
                if yo < y.len() && uvo + 1 < uv.len() {
                    add(yuv_to_rgb(y[yo], uv[uvo], uv[uvo + 1]));
                }
            }
        }
    } else {
        let Some((frame, stride)) = raw_plane(datas, format, width) else {
            return [0, 0, 0];
        };
        let bpp = bytes_per_pixel(format) as usize;
        for row in (y0..y1).step_by(step) {
            for col in (x0..x1).step_by(step) {
                let offset = row as usize * stride + col as usize * bpp;
                if offset + bpp <= frame.len() {
                    add(read_pixel(frame, offset, format));
                }
            }
        }
    }

    if n == 0 {
        return [0, 0, 0];
    }
    sum.map(|s| (s / n) as u8)
}

fn raw_plane(datas: &mut [Data], format: VideoFormat, width: u32) -> Option<(&[u8], usize)> {
    let data = datas.first_mut()?;
    let stride = stride(data, width * bytes_per_pixel(format));
    let offset = data.chunk().offset() as usize;
    let size = data.chunk().size() as usize;
    let frame = data.data()?;
    let end = (offset + size).min(frame.len());
    if end <= offset {
        return None;
    }
    Some((&frame[offset..end], stride))
}

fn zone_rect(width: u32, height: u32, depth: i32, zone: &EdgeZone) -> Rect {
    let (w, h) = (width as i32, height as i32);
    let cx = (zone.cx * (w - 1) as f32).round() as i32;
    let cy = (zone.cy * (h - 1) as f32).round() as i32;
    // Cover the whole stretch of edge up to the neighbouring LEDs, not just a small square.
    let along = |len: i32| ((zone.span * len as f32).ceil() as i32).max(depth / 2).max(4);

    match zone.edge.as_str() {
        "right" => (w - depth, (cy - along(h)).max(0), w, (cy + along(h)).min(h)),
        "left" => (0, (cy - along(h)).max(0), depth, (cy + along(h)).min(h)),
        "top" => ((cx - along(w)).max(0), 0, (cx + along(w)).min(w), depth),
        "bottom" => ((cx - along(w)).max(0), h - depth, (cx + along(w)).min(w), h),
        _ => {
            let half = (depth / 2).max(4);
            ((cx - half).max(0), (cy - half).max(0), (cx + half).min(w), (cy + half).min(h))
        }
    }
}

fn read_pixel(frame: &[u8], offset: usize, format: VideoFormat) -> [u8; 3] {
    if format == VideoFormat::RGB {
        [frame[offset], frame[offset + 1], frame[offset + 2]]
    } else if format == VideoFormat::BGR {
        [frame[offset + 2], frame[offset + 1], frame[offset]]
    } else if format == VideoFormat::RGBx || format == VideoFormat::RGBA {
        [frame[offset], frame[offset + 1], frame[offset + 2]]
    } else if format == VideoFormat::BGRx || format == VideoFormat::BGRA {
        [frame[offset + 2], frame[offset + 1], frame[offset]]
    } else if format == VideoFormat::xRGB {
        [frame[offset + 1], frame[offset + 2], frame[offset + 3]]
    } else if format == VideoFormat::xBGR {
        [frame[offset + 3], frame[offset + 2], frame[offset + 1]]
    } else if format == VideoFormat::ARGB {
        [frame[offset + 1], frame[offset + 2], frame[offset + 3]]
    } else if format == VideoFormat::ABGR {
        [frame[offset + 3], frame[offset + 2], frame[offset + 1]]
    } else {
        [frame[offset + 2], frame[offset + 1], frame[offset]]
    }
}

fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let y = f32::from(y);
    let u = f32::from(u) - 128.0;
    let v = f32::from(v) - 128.0;
    let r = (y + 1.402 * v).clamp(0.0, 255.0) as u8;
    let g = (y - 0.344136 * u - 0.714136 * v).clamp(0.0, 255.0) as u8;
    let b = (y + 1.772 * u).clamp(0.0, 255.0) as u8;
    [r, g, b]
}
