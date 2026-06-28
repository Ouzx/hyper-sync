use pipewire::spa::buffer::Data;
use pipewire::spa::param::video::VideoFormat;

use crate::config::EdgeZone;

pub fn sample_edges(
    datas: &mut [Data],
    format: VideoFormat,
    width: u32,
    height: u32,
    zones: &[EdgeZone],
) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(zones.len() * 3);
    let depth = ((width.min(height) as f32) * 0.03).clamp(16.0, 96.0) as i32;

    for zone in zones {
        let [r, g, b] = if format == VideoFormat::NV12 || format == VideoFormat::NV21 {
            avg_nv12(datas, width, height, depth, zone)
        } else {
            let Some((frame, stride)) = raw_plane(datas, format, width) else {
                rgb.extend_from_slice(&[0, 0, 0]);
                continue;
            };
            avg_raw(frame, stride, format, width, height, depth, zone)
        };
        rgb.extend_from_slice(&[r, g, b]);
    }

    rgb
}

fn raw_plane(datas: &mut [Data], format: VideoFormat, width: u32) -> Option<(&[u8], usize)> {
    let bpp = super::negotiate::bytes_per_pixel(format);
    let data = datas.first_mut()?;
    let stride = data
        .chunk()
        .stride()
        .unsigned_abs()
        .max(width * bpp) as usize;
    let offset = data.chunk().offset() as usize;
    let size = data.chunk().size() as usize;
    let frame = data.data()?;
    let end = (offset + size).min(frame.len());
    if end <= offset {
        return None;
    }
    Some((&frame[offset..end], stride))
}

fn avg_raw(
    frame: &[u8],
    stride: usize,
    format: VideoFormat,
    width: u32,
    height: u32,
    depth: i32,
    zone: &EdgeZone,
) -> [u8; 3] {
    let bpp = super::negotiate::bytes_per_pixel(format) as usize;
    let (x0, y0, x1, y1) = zone_rect(width, height, depth, zone);
    let mut r = 0u64;
    let mut g = 0u64;
    let mut b = 0u64;
    let mut n = 0u64;

    for y in y0..y1 {
        for x in x0..x1 {
            let offset = y as usize * stride + x as usize * bpp;
            if offset + bpp > frame.len() {
                continue;
            }
            let px = read_pixel(frame, offset, format);
            r += u64::from(px[0]);
            g += u64::from(px[1]);
            b += u64::from(px[2]);
            n += 1;
        }
    }

    if n == 0 {
        return [0, 0, 0];
    }
    [(r / n) as u8, (g / n) as u8, (b / n) as u8]
}

fn avg_nv12(
    datas: &mut [Data],
    width: u32,
    height: u32,
    depth: i32,
    zone: &EdgeZone,
) -> [u8; 3] {
    if datas.len() < 2 {
        return [0, 0, 0];
    }
    let (y_plane, uv_planes) = datas.split_at_mut(1);
    let y_stride = y_plane[0]
        .chunk()
        .stride()
        .unsigned_abs()
        .max(width) as usize;
    let uv_stride = uv_planes[0]
        .chunk()
        .stride()
        .unsigned_abs()
        .max(width) as usize;
    let Some(y) = y_plane[0].data() else {
        return [0, 0, 0];
    };
    let Some(uv) = uv_planes[0].data() else {
        return [0, 0, 0];
    };

    let (x0, y0, x1, y1) = zone_rect(width, height, depth, zone);
    let mut r = 0u64;
    let mut g = 0u64;
    let mut b = 0u64;
    let mut n = 0u64;

    for row in y0..y1 {
        for col in x0..x1 {
            let yo = row as usize * y_stride + col as usize;
            if yo >= y.len() {
                continue;
            }
            let uv_x = (col as usize / 2) * 2;
            let uv_y = row as usize / 2;
            let uvo = uv_y * uv_stride + uv_x;
            if uvo + 1 >= uv.len() {
                continue;
            }
            let px = yuv_to_rgb(y[yo], uv[uvo], uv[uvo + 1]);
            r += u64::from(px[0]);
            g += u64::from(px[1]);
            b += u64::from(px[2]);
            n += 1;
        }
    }

    if n == 0 {
        return [0, 0, 0];
    }
    [(r / n) as u8, (g / n) as u8, (b / n) as u8]
}

/// Average the center region and replicate to every LED (movie-style ambient).
pub fn sample_center(
    datas: &mut [Data],
    format: VideoFormat,
    width: u32,
    height: u32,
    leds: u8,
) -> Vec<u8> {
    let [r, g, b] = avg_center(datas, format, width, height);
    vec![r, g, b].repeat(usize::from(leds))
}

fn avg_center(
    datas: &mut [Data],
    format: VideoFormat,
    width: u32,
    height: u32,
) -> [u8; 3] {
    let w = width as i32;
    let h = height as i32;
    let mx = (w as f32 * 0.275) as i32;
    let my = (h as f32 * 0.275) as i32;
    let x0 = mx;
    let y0 = my;
    let x1 = w - mx;
    let y1 = h - my;
    if format == VideoFormat::NV12 || format == VideoFormat::NV21 {
        avg_nv12_center(datas, width, height, x0, y0, x1, y1)
    } else {
        let Some((frame, stride)) = raw_plane(datas, format, width) else {
            return [0, 0, 0];
        };
        avg_raw_rect(frame, stride, format, x0, y0, x1, y1)
    }
}

fn avg_raw_rect(
    frame: &[u8],
    stride: usize,
    format: VideoFormat,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
) -> [u8; 3] {
    let bpp = super::negotiate::bytes_per_pixel(format) as usize;
    let mut r = 0u64;
    let mut g = 0u64;
    let mut b = 0u64;
    let mut n = 0u64;
    for y in y0..y1 {
        for x in x0..x1 {
            let offset = y as usize * stride + x as usize * bpp;
            if offset + bpp > frame.len() {
                continue;
            }
            let px = read_pixel(frame, offset, format);
            r += u64::from(px[0]);
            g += u64::from(px[1]);
            b += u64::from(px[2]);
            n += 1;
        }
    }
    if n == 0 {
        return [0, 0, 0];
    }
    [(r / n) as u8, (g / n) as u8, (b / n) as u8]
}

fn avg_nv12_center(
    datas: &mut [Data],
    width: u32,
    _height: u32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
) -> [u8; 3] {
    if datas.len() < 2 {
        return [0, 0, 0];
    }
    let (y_plane, uv_planes) = datas.split_at_mut(1);
    let y_stride = y_plane[0]
        .chunk()
        .stride()
        .unsigned_abs()
        .max(width) as usize;
    let uv_stride = uv_planes[0]
        .chunk()
        .stride()
        .unsigned_abs()
        .max(width) as usize;
    let Some(y) = y_plane[0].data() else {
        return [0, 0, 0];
    };
    let Some(uv) = uv_planes[0].data() else {
        return [0, 0, 0];
    };
    let mut r = 0u64;
    let mut g = 0u64;
    let mut b = 0u64;
    let mut n = 0u64;
    for row in y0..y1 {
        for col in x0..x1 {
            let yo = row as usize * y_stride + col as usize;
            if yo >= y.len() {
                continue;
            }
            let uv_x = (col as usize / 2) * 2;
            let uv_y = row as usize / 2;
            let uvo = uv_y * uv_stride + uv_x;
            if uvo + 1 >= uv.len() {
                continue;
            }
            let px = yuv_to_rgb(y[yo], uv[uvo], uv[uvo + 1]);
            r += u64::from(px[0]);
            g += u64::from(px[1]);
            b += u64::from(px[2]);
            n += 1;
        }
    }
    if n == 0 {
        return [0, 0, 0];
    }
    [(r / n) as u8, (g / n) as u8, (b / n) as u8]
}

fn zone_rect(width: u32, height: u32, depth: i32, zone: &EdgeZone) -> (i32, i32, i32, i32) {
    let w = width as i32;
    let h = height as i32;
    let cx = (zone.cx * (w - 1) as f32).round() as i32;
    let cy = (zone.cy * (h - 1) as f32).round() as i32;
    let half = (depth / 2).max(4);

    match zone.edge.as_str() {
        "right" => (w - depth, (cy - half).max(0), w, (cy + half).min(h)),
        "left" => (0, (cy - half).max(0), depth, (cy + half).min(h)),
        "top" => ((cx - half).max(0), 0, (cx + half).min(w), depth),
        "bottom" => ((cx - half).max(0), h - depth, (cx + half).min(w), h),
        _ => ((cx - half).max(0), (cy - half).max(0), (cx + half).min(w), (cy + half).min(h)),
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
