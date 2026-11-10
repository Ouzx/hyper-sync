//! Ports of Hyperion.ng's bundled effects (github.com/hyperion-project/hyperion.ng, effects/*.py,
//! MIT — see LICENSES/hyperion.ng.txt). Each follows its script's maths and timing at speed 1.
//!
//! Hyperion draws most effects on an image and gives each LED the colour of its patch of that
//! image; here each LED samples the image at the middle of its edge zone (`uv`). Where a script
//! has a `color` argument the accent colour stands in for it, and a `rainbow` accent picks the
//! script's own default or its random/full-colour preset.

use std::f32::consts::{PI, TAU};

use crate::config::EffectMode;
use crate::effects::solid::{hsv_to_rgb, is_rainbow_color, parse_color, rgb_to_hsv};
use crate::effects::{hash, Ctx};
use crate::layout::{pos, segment_bounds, ASPECT};

type Rgba = [f32; 4];

pub fn render(mode: EffectMode, c: &mut Ctx) -> Vec<u8> {
    let n = c.n;
    let t = c.t;
    let per_led = |f: &dyn Fn(usize) -> [u8; 3]| (0..n).flat_map(f).collect::<Vec<u8>>();
    match mode {
        // rainbow-mood.py: the whole strip on one hue, once round the wheel per minute.
        EffectMode::RainbowMood => per_led(&|_| hsv_to_rgb((t / 60.0).fract(), 1.0, 1.0)),
        EffectMode::MoodBlobs => mood_blobs(c),
        EffectMode::Swirl => swirl(c),
        EffectMode::DoubleSwirl => double_swirl(c),
        EffectMode::Plasma => plasma(c),
        // waves.py with Seawaves.json: blue rings from beyond the top-right corner, a cycle a minute.
        EffectMode::SeaWaves => waves(c, (1.25, -0.25), &SEA, 60.0),
        // waves.py defaults ("Waves with Color"): rainbow rings from the top-left, 90 s a cycle.
        EffectMode::ColorWaves => waves(c, (-0.15, -0.25), &RAINBOW6, 90.0),
        EffectMode::Juggler => juggler(c),
        EffectMode::Collision => collision(c),
        EffectMode::LightClock => light_clock(c),
        EffectMode::Traces => traces(c),
        EffectMode::Flags => flags(c),
        EffectMode::KnightRider => knight_rider(c),
        // ledtest.py: every LED red, green, blue, white, off — half a second each.
        EffectMode::LedTest => {
            const SEQ: [[u8; 3]; 5] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255], [0, 0, 0]];
            per_led(&|_| SEQ[(t / 0.5) as usize % 5])
        }
        // ledtest-seq.py: LEDs light one by one (red, green, blue, ...), then two dark steps.
        EffectMode::LedTestSeq => {
            let step = (t / 0.5) as usize % (n + 2);
            per_led(&|i| if step < n && i <= step { [[255, 0, 0], [0, 255, 0], [0, 0, 255]][i % 3] } else { [0, 0, 0] })
        }
        // police.py ("Police Lights Single"): a red and a blue block of 11 LEDs circling, 1.5 s a lap.
        EffectMode::Police => police(c, 10.0, 1.5),
        // police.py ("Police Lights Solid"): half red, half blue, a lap a second.
        EffectMode::PoliceSolid => police(c, n as f32 / 2.0, 1.0),
        EffectMode::Random => random(c),
        EffectMode::Snake => snake(c),
        EffectMode::Sparks => sparks(c),
        EffectMode::Trails => trails(c),
        // x-mas.py: white and red LEDs alternating, swapping every 0.75 s.
        EffectMode::Xmas => {
            let phase = (t / 0.75) as usize % 2;
            per_led(&|i| if (i + phase).is_multiple_of(2) { [255, 255, 255] } else { [255, 0, 0] })
        }
        // gif.py with fire.gif / lights.gif (25 fps) and matrix.gif (30 fps).
        EffectMode::HyperionFire => gif(c, FIRE_GIF, 25.0),
        EffectMode::Lights => gif(c, LIGHTS_GIF, 25.0),
        EffectMode::Matrix => gif(c, MATRIX_GIF, 30.0),
        // fade.py presets.
        EffectMode::Breath => fade(c, &Fade { from: [50; 3], to: [255; 3], up: 3.0, hold_to: 0.05, down: 1.0, hold_from: 0.25, times: 0, stay: true }),
        EffectMode::NotifyBlue => fade(c, &Fade { from: [0, 0, 50], to: [0, 0, 255], up: 0.2, hold_to: 0.04, down: 0.1, hold_from: 0.15, times: 3, stay: false }),
        EffectMode::CinemaBrighten => fade(c, &CINEMA_UP),
        EffectMode::CinemaDim => fade(c, &Fade { up: 0.0, down: 5.0, ..CINEMA_UP }),
        _ => vec![0; n * 3],
    }
}

/// A fade.py preset (times in seconds).
struct Fade {
    from: [u8; 3],
    to: [u8; 3],
    up: f32,
    hold_to: f32,
    down: f32,
    hold_from: f32,
    /// Cycles to run; 0 = forever.
    times: u32,
    /// Keep the last colour afterwards (else go dark).
    stay: bool,
}

/// "Cinema brighten lights": dim amber up to bright amber over 5 s, then stay (for when the film
/// ends). "Cinema dim lights" is the same fade downward.
const CINEMA_UP: Fade = Fade { from: [136, 97, 7], to: [238, 173, 47], up: 5.0, hold_to: 0.0, down: 0.0, hold_from: 0.0, times: 1, stay: true };

/// fade.py: fade `from` → `to`, hold, fade back, hold; `times` cycles, then keep the last colour
/// (after the script's half-second pause) or go dark.
fn fade(c: &Ctx, f: &Fade) -> Vec<u8> {
    let cycle = f.up + f.hold_to + f.down + f.hold_from;
    let mix = |a: [u8; 3], b: [u8; 3], k: f32| -> [u8; 3] {
        std::array::from_fn(|i| (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * k.clamp(0.0, 1.0)).round() as u8)
    };
    let done = f.times > 0 && c.t >= cycle * f.times as f32;
    let last = if f.down > 0.0 || f.hold_from > 0.0 { f.from } else { f.to };
    let color = if done {
        if f.stay || c.t < cycle * f.times as f32 + 0.5 { last } else { [0, 0, 0] }
    } else {
        let p = c.t % cycle;
        if p < f.up {
            mix(f.from, f.to, p / f.up)
        } else if p < f.up + f.hold_to {
            f.to
        } else if p < f.up + f.hold_to + f.down {
            mix(f.to, f.from, (p - f.up - f.hold_to) / f.down)
        } else {
            f.from
        }
    };
    color.repeat(c.n)
}

/// Hyperion's GIF animations, every frame shrunk to 32×18 RGB with area averaging (each cell is the
/// mean of its patch, much as Hyperion averages an LED's zone). Made with
/// `ffmpeg -i fire.gif -vf scale=32:18:flags=area -fps_mode passthrough -f rawvideo -pix_fmt rgb24`.
const GIF_W: usize = 32;
const GIF_H: usize = 18;
static FIRE_GIF: &[u8] = include_bytes!("../../assets/hyperion/fire.rgb");
static LIGHTS_GIF: &[u8] = include_bytes!("../../assets/hyperion/lights.rgb");
static MATRIX_GIF: &[u8] = include_bytes!("../../assets/hyperion/matrix.rgb");

/// gif.py: plays the frames in a loop at a fixed rate, stretched over the whole screen; each LED
/// takes the cell at its spot on the edge.
fn gif(c: &Ctx, frames: &[u8], fps: f32) -> Vec<u8> {
    let size = GIF_W * GIF_H * 3;
    let frame = &frames[((c.t * fps) as usize % (frames.len() / size)) * size..][..size];
    (0..c.n)
        .flat_map(|i| {
            let (x, y) = pos(i, c.n);
            let col = ((x / ASPECT * GIF_W as f32) as usize).min(GIF_W - 1);
            let row = ((y * GIF_H as f32) as usize).min(GIF_H - 1);
            let p = (row * GIF_W + col) * 3;
            [frame[p], frame[p + 1], frame[p + 2]]
        })
        .collect()
}

/// The accent colour, or None for `rainbow`.
fn accent(c: &Ctx) -> Option<[u8; 3]> {
    (!is_rainbow_color(&c.cfg.solid.color)).then(|| parse_color(&c.cfg.solid.color).unwrap_or([255, 51, 0]))
}

/// Where LED `i` samples Hyperion's effect image: u, v in 0..1 (left→right, top→bottom), at the
/// middle of its edge zone rather than the very border.
fn uv(i: usize, n: usize) -> (f32, f32) {
    let (x, y) = pos(i, n);
    let inset = |w: f32| 0.04 + 0.92 * w;
    (inset(x / ASPECT), inset(y))
}

fn rand(seed: u32, a: u32, b: u32) -> f32 {
    hash(seed ^ a.wrapping_mul(0x2545_F491), b)
}

fn to_u8(c: [f32; 3]) -> [u8; 3] {
    c.map(|v| v.clamp(0.0, 255.0) as u8)
}

fn rgba(c: [u8; 3], a: f32) -> Rgba {
    [f32::from(c[0]), f32::from(c[1]), f32::from(c[2]), a]
}

/// Qt-style gradient: `stops` sorted by position (0..1), padded beyond the first and last.
fn gradient(stops: &[(f32, Rgba)], t: f32) -> Rgba {
    let first = stops[0];
    if t <= first.0 {
        return first.1;
    }
    for w in stops.windows(2) {
        let ((p0, a), (p1, b)) = (w[0], w[1]);
        if t <= p1 {
            let f = if p1 > p0 { (t - p0) / (p1 - p0) } else { 1.0 };
            return std::array::from_fn(|k| a[k] + (b[k] - a[k]) * f);
        }
    }
    stops[stops.len() - 1].1
}

/// swirl.py's buildGradient: colours evenly spaced (step = 255 / count), plus a stop at 0 with
/// the last colour so the circle closes.
fn closed_gradient(colors: &[Rgba]) -> Vec<(f32, Rgba)> {
    let step = (255 / colors.len()) as f32;
    let mut stops = vec![(0.0, colors[colors.len() - 1])];
    stops.extend(colors.iter().enumerate().map(|(k, &c)| ((k + 1) as f32 * step / 255.0, c)));
    stops
}

/// Position on a Qt conical gradient: 0..1 counter-clockwise from `start` degrees (0 = 3 o'clock).
fn cone(u: f32, v: f32, start: f32) -> f32 {
    let angle = (0.5 - v).atan2(u - 0.5).to_degrees();
    (angle - start).rem_euclid(360.0) / 360.0
}

/// Paint `top` over `under` using its alpha (0..255).
fn over(under: [f32; 3], top: Rgba) -> [f32; 3] {
    let a = top[3] / 255.0;
    std::array::from_fn(|k| top[k] * a + under[k] * (1.0 - a))
}

/// mood-blobs.py: five blobs drifting round (60 s per half turn of the pattern) over a colour field
/// that varies ±60° in hue around the strip and rotates a step every 0.2 s. A colour accent gives
/// the red/green/blue presets; rainbow gives "Full color mood blobs" (±30°, base hue wandering 1°
/// every 0.2 s from a random start).
fn mood_blobs(c: &Ctx) -> Vec<u8> {
    let (n, t) = (c.n as f32, c.t);
    let (base, sat, val, change) = match accent(c) {
        Some(rgb) => {
            let (h, s, v) = rgb_to_hsv(rgb);
            (h, s, v, 60.0 / 360.0)
        }
        None => ((rand(c.seed, 1, 0) + t / (0.2 * 360.0)).fract(), 1.0, 1.0, 30.0 / 360.0),
    };
    let blobs = 5.0;
    let phase = t * blobs * PI / 60.0;
    let shift = (t / 0.2).floor();
    (0..c.n)
        .flat_map(|i| {
            let fi = i as f32;
            let hue = (base + change * (TAU * (fi - shift) / n).sin()).rem_euclid(1.0);
            let amp = (-phase + TAU * blobs * fi / n).sin().max(0.0);
            hsv_to_rgb(hue, sat, val).map(|ch| (f32::from(ch) * amp) as u8)
        })
        .collect()
}

/// swirl.py: a conical gradient turning round the middle of the screen. Rainbow accent: "Rainbow
/// swirl" (a turn per 20 s; "Rainbow swirl fast" is speed 2.9). Colour accent: "Atomic swirl"'s
/// three arms (a turn per 25 s) in the accent instead of yellow.
fn swirl(c: &Ctx) -> Vec<u8> {
    let (stops, rotation) = match accent(c) {
        None => {
            let s = [
                (0, [255, 0, 0]),
                (25, [255, 230, 0]),
                (63, [255, 255, 0]),
                (100, [0, 255, 0]),
                (127, [0, 255, 200]),
                (159, [0, 255, 255]),
                (191, [0, 0, 255]),
                (224, [255, 0, 255]),
                (255, [255, 0, 127]),
            ];
            (s.map(|(p, c)| (p as f32 / 255.0, rgba(c, 255.0))).to_vec(), 20.0)
        }
        Some(arm) => {
            let (k, a) = (rgba([0, 0, 0], 255.0), rgba(arm, 255.0));
            (closed_gradient(&[k, k, a, k, k, a, k, k, a]), 25.0)
        }
    };
    let start = 360.0 * c.t / rotation;
    (0..c.n)
        .flat_map(|i| {
            let (u, v) = uv(i, c.n);
            let g = gradient(&stops, cone(u, v, start));
            to_u8([g[0], g[1], g[2]])
        })
        .collect()
}

/// swirl.py with double-swirl.json: a red→blue gradient turning one way (25 s a turn) under two
/// black shutters turning the other way.
fn double_swirl(c: &Ctx) -> Vec<u8> {
    let under = closed_gradient(&[rgba([255, 0, 0], 255.0), rgba([20, 0, 255], 255.0)]);
    let (clear, black) = (rgba([255, 0, 0], 0.0), rgba([0, 0, 0], 255.0));
    let shutters = closed_gradient(&[clear, clear, clear, black, rgba([0, 0, 0], 0.0), clear, clear, black]);
    let angle = 360.0 * c.t / 25.0;
    (0..c.n)
        .flat_map(|i| {
            let (u, v) = uv(i, c.n);
            let g = gradient(&under, cone(u, v, angle));
            to_u8(over([g[0], g[1], g[2]], gradient(&shutters, cone(u, v, -angle))))
        })
        .collect()
}

/// plasma.py: a fixed field of sines over a 64×64 image, cycled through the hue wheel. Hyperion
/// advanced it by its own CPU time × 100, which runs slower than the clock and varies with load;
/// here it moves 20 hue steps a second (a full cycle every 13 s).
fn plasma(c: &Ctx) -> Vec<u8> {
    let shift = c.t * 20.0;
    (0..c.n)
        .flat_map(|i| {
            let (u, v) = uv(i, c.n);
            let (x, y) = ((u * 63.0).round(), (v * 63.0).round());
            let sum = 128.0 + 128.0 * (x / 16.0).sin()
                + 128.0 + 128.0 * (y / 8.0).sin()
                + 128.0 + 128.0 * (x + y).sin() / 16.0
                + 128.0 + 128.0 * ((x * x + y * y).sqrt() / 8.0).sin();
            let value = sum.floor() / 4.0;
            hsv_to_rgb(((value + shift) % 256.0).floor() / 255.0, 1.0, 1.0)
        })
        .collect()
}

const SEA: [[u8; 3]; 6] = [[8, 0, 255], [0, 161, 255], [0, 222, 255], [0, 153, 255], [38, 0, 255], [0, 199, 255]];
const RAINBOW6: [[u8; 3]; 6] = [[255, 0, 0], [255, 255, 0], [0, 255, 0], [0, 255, 255], [0, 0, 255], [255, 0, 255]];

/// waves.py: a radial gradient from `center` (0..1 screen units, may lie off screen) whose colour
/// stops creep outward one step in 255 every `rotation`/255 seconds, wrapping back to the middle.
fn waves(c: &Ctx, center: (f32, f32), colors: &[[u8; 3]], rotation: f32) -> Vec<u8> {
    let far = |p: f32| if p < 0.5 { 1.0 - p } else { p };
    let radius = (far(center.0) * ASPECT).hypot(far(center.1)) * 1.3;
    let step = (255 / colors.len()) as f32;
    let moved = (c.t * 255.0 / rotation).floor();
    let mut stops: Vec<(f32, Rgba)> = colors
        .iter()
        .enumerate()
        .map(|(k, &col)| ((k as f32 * step + moved) % 256.0 / 255.0, rgba(col, 255.0)))
        .collect();
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    (0..c.n)
        .flat_map(|i| {
            let (u, v) = uv(i, c.n);
            let d = (u * ASPECT - center.0 * ASPECT).hypot(v - center.1) / radius;
            let g = gradient(&stops, d);
            to_u8([g[0], g[1], g[2]])
        })
        .collect()
}

/// juggler.py: eight dots on slow Lissajous paths over a 64×64 image, leaving fading trails.
/// Only the pixels the LEDs sample are kept (in `mem`, RGB per LED).
fn juggler(c: &mut Ctx) -> Vec<u8> {
    let n = c.n;
    let fade = (1.0 - 25.0 / 256.0f32).powf(c.dt * 60.0);
    c.mem.iter_mut().for_each(|v| *v *= fade);
    let ms = c.t * 1000.0;
    for k in 0..8 {
        let kf = k as f32;
        let ax = ms / 60000.0 * (kf + 5.0) * TAU + kf * 1.3;
        let ay = ms / 60000.0 * (kf + 3.0) * TAU + kf * 0.9 + PI;
        let (dx, dy) = (((ax.sin() + 1.0) * 0.5 * 63.0).floor(), ((ay.sin() + 1.0) * 0.5 * 63.0).floor());
        let rgb = hsv_to_rgb(((k * 32) & 255) as f32 / 255.0, 200.0 / 255.0, 1.0);
        for i in 0..n {
            let (u, v) = uv(i, n);
            let d = ((u * 63.0).round() - dx).hypot((v * 63.0).round() - dy);
            if d <= 3.0 {
                let intensity = (255.0 - (d / 3.0 * 200.0).floor()) / 255.0;
                for (m, ch) in c.mem[i * 3..i * 3 + 3].iter_mut().zip(rgb) {
                    *m = m.max(f32::from(ch) * intensity);
                }
            }
        }
    }
    c.mem[..n * 3].iter().map(|&v| v as u8).collect()
}

/// collision.py: two dots with 5-LED trails start at both ends with random hues, close in a LED
/// every 0.1 s, and burst where they meet (8 frames). The script then rotates its output by a
/// random number of *bytes*, not LEDs — which also shifts colours between channels; kept as is.
fn collision(c: &mut Ctx) -> Vec<u8> {
    const STEP: f32 = 0.1;
    const TRAIL: i64 = 5;
    const RADIUS: i64 = 8;
    let n = c.n as i64;
    // mem: [steps done, p1, p2, hue1, hue2, byte shift, burst frame (-1 = flying), pair number]
    let m = &mut c.mem;
    let new_pair = |m: &mut Vec<f32>, seed: u32| {
        let k = m[7] as u32 + 1;
        let shift = (rand(seed, 3, k) * n as f32) as i64 * if rand(seed, 4, k) < 0.5 { -1 } else { 1 };
        m[1..8].copy_from_slice(&[0.0, (n - 1) as f32, rand(seed, 1, k), rand(seed, 2, k), shift as f32, -1.0, k as f32]);
    };
    if m[7] == 0.0 {
        new_pair(m, c.seed);
        m[1] = 1.0; // the first move happens before the first frame
        m[2] = (n - 2) as f32;
    }
    let due = (c.t / STEP) as u32;
    while (m[0] as u32) < due {
        m[0] += 1.0;
        if m[6] >= 0.0 {
            // Bursting: frames 0..7, then the last one is held one more step.
            m[6] += 1.0;
            if m[6] > RADIUS as f32 {
                new_pair(m, c.seed);
                m[1] = 1.0;
                m[2] = (n - 2) as f32;
            }
            continue;
        }
        let (p1, p2) = (m[1] as i64, m[2] as i64);
        if (p1 + 1) - (p2 - 1) >= 0 {
            m[1] += 1.0;
            m[2] -= 1.0;
            m[6] = 0.0; // they met: the burst replaces this frame
        } else {
            m[1] += 1.0;
            m[2] -= 1.0;
        }
    }
    let (p1, p2, h1, h2, shift, burst) = (m[1] as i64, m[2] as i64, m[3], m[4], m[5] as i64, m[6]);
    let mut buf = vec![0u8; c.n * 3];
    let mut put = |pixel: i64, rgb: [u8; 3]| {
        let p = pixel.rem_euclid(n) as usize;
        buf[p * 3..p * 3 + 3].copy_from_slice(&rgb);
    };
    for (head, dir, hue) in [(p1, 1, h1), (p2, -1, h2)] {
        for k in 0..TRAIL {
            put(head - dir * k, hsv_to_rgb(hue, 1.0, (TRAIL - k) as f32 / TRAIL as f32));
        }
    }
    if burst >= 0.0 {
        let pair = m[7] as u32;
        for d in 0..=(burst as i64).min(RADIUS - 1) {
            for pixel in (p1 - d).max(0)..(p1 + d).min(n) {
                let hue = if rand(c.seed, pixel as u32 + 100 * d as u32, pair) < 0.5 { h1 } else { h2 };
                put(pixel, hsv_to_rgb(hue, 1.0, (RADIUS - d) as f32 / RADIUS as f32));
            }
        }
    }
    let len = buf.len() as i64;
    (0..len).map(|j| buf[(j - shift).rem_euclid(len) as usize]).collect()
}

/// light-clock.py: hour (blue), minute (green) and second (red) as wedges of a conical gradient
/// around the middle of the screen; each has a sharp edge at its hand and fades out behind it.
fn light_clock(c: &Ctx) -> Vec<u8> {
    let (h, m, s) = local_time();
    let angle = |a: i32| (if 449 - a >= 360 { 449 - a - 360 } else { 449 - a }) as f32;
    let (ah, am, asec) = (angle(30 * (h % 12)), angle(6 * m), angle(6 * s));
    let wedge = |rgb: [u8; 3], stops: &[(u8, f32)]| -> Vec<(f32, Rgba)> {
        stops.iter().map(|&(p, a)| (f32::from(p) / 255.0, if a > 0.0 { rgba(rgb, a) } else { rgba([0, 0, 0], 0.0) })).collect()
    };
    let hour = wedge([0, 0, 255], &[(0, 255.0), (90, 255.0), (150, 127.0), (191, 0.0)]);
    let minute = wedge([0, 255, 0], &[(0, 255.0), (35, 255.0), (50, 127.0), (90, 0.0)]);
    let second = wedge([255, 0, 0], &[(0, 255.0), (8, 255.0), (10, 0.0)]);
    // The script draws whichever of hour and minute is "behind" first.
    let mut layers = if ah - am < 90.0 && ah - am > 0.0 {
        vec![(&minute, am), (&hour, ah)]
    } else {
        vec![(&hour, ah), (&minute, am)]
    };
    layers.push((&second, asec));
    (0..c.n)
        .flat_map(|i| {
            let (u, v) = uv(i, c.n);
            to_u8(layers.iter().fold([0.0; 3], |px, (stops, a)| over(px, gradient(stops, cone(u, v, *a)))))
        })
        .collect()
}

/// traces.py: six runners, one per colour channel at two levels, stepping a LED every 9…4 ticks
/// of 4 ms and leaving their channel set to a random level; nothing fades, runners overwrite.
fn traces(c: &mut Ctx) -> Vec<u8> {
    const RUNNERS: [(usize, f32, f32); 6] = [(0, 9.0, 255.0), (1, 8.0, 255.0), (2, 7.0, 255.0), (0, 6.0, 100.0), (1, 5.0, 100.0), (2, 4.0, 100.0)];
    let n = c.n;
    // mem: n*3 LED channels, then (position, countdown) per runner, then ticks done.
    let base = n * 3;
    if c.mem.len() < base + 13 {
        c.mem.resize(base + 13, 0.0);
    }
    let due = (c.t / 0.004) as u64;
    let mut done = c.mem[base + 12] as u64;
    done = done.max(due.saturating_sub(4000)); // after a pause, don't replay minutes of ticks
    while done < due {
        for (r, &(ch, step, level)) in RUNNERS.iter().enumerate() {
            let (pos, count) = (base + r * 2, base + r * 2 + 1);
            if c.mem[count] == 0.0 {
                c.mem[count] = step;
                c.mem[pos] = ((c.mem[pos] as usize + 1) % n) as f32;
                let led = c.mem[pos] as usize;
                c.mem[led * 3 + ch] = (level * (0.2 + 0.8 * rand(c.seed, r as u32, done as u32))).floor();
            } else {
                c.mem[count] -= 1.0;
            }
        }
        done += 1;
    }
    c.mem[base + 12] = done as f32;
    c.mem[..base].iter().map(|&v| v as u8).collect()
}

/// flag.py with its default config: the German flag, then the Swedish one, two seconds each.
fn flags(c: &Ctx) -> Vec<u8> {
    let germany = ((c.t / 2.0) as u32).is_multiple_of(2);
    (0..c.n)
        .flat_map(|i| {
            let (u, v) = uv(i, c.n);
            if germany {
                if v < 0.33 { [0, 0, 0] } else if v < 0.66 { [221, 0, 0] } else { [255, 206, 0] }
            } else if (u - 0.35).abs() < 0.07 || (v - 0.5).abs() < 0.07 {
                [254, 204, 0]
            } else {
                [0, 106, 167]
            }
        })
        .collect()
}

/// knight-rider.py: a light bouncing along a 25-pixel row (25 px/s), dimming to 70% per pixel it
/// leaves behind. The row spans the screen width, so each side lights all at once at the ends.
/// Accent colour (red with a rainbow accent).
fn knight_rider(c: &Ctx) -> Vec<u8> {
    let color = accent(c).unwrap_or([255, 0, 0]);
    let phase = (c.t * 25.0).floor() % 48.0;
    (0..c.n)
        .flat_map(|i| {
            let col = ((pos(i, c.n).0 / ASPECT) * 25.0).floor().min(24.0);
            let since = |at: f32| (phase - at).rem_euclid(48.0);
            let steps = if col == 0.0 || col == 24.0 { since(col) } else { since(col).min(since(48.0 - col)) };
            color.map(|ch| (f32::from(ch) * 0.7f32.powf(steps)) as u8)
        })
        .collect()
}

/// police.py: `count`+1 red LEDs and a blue block opposite, circling a lap per `rotation` seconds
/// in jumps of 2 or 4 LEDs, as the script steps no faster than every 50 ms.
fn police(c: &Ctx, count: f32, rotation: f32) -> Vec<u8> {
    let n = c.n;
    let half = n as f32 / 2.0;
    let (mut inc, mut sleep) = (1usize, rotation / n as f32);
    while sleep < 0.05 {
        inc *= 2;
        sleep *= 2.0;
    }
    let shift = (c.t / sleep) as usize * inc;
    (0..n)
        .flat_map(|i| {
            let j = ((i as i64 - shift as i64).rem_euclid(n as i64)) as f32;
            if j <= count {
                [255, 0, 0]
            } else if j >= half - 1.0 && j < half + count {
                [0, 0, 255]
            } else {
                [0, 0, 0]
            }
        })
        .collect()
}

/// random.py: every 0.75 s each LED has a 1-in-10 chance of a new random colour (any hue, random
/// brightness). Brighter colours fade in over ~0.2 s and darker ones snap, like the script's fade.
fn random(c: &mut Ctx) -> Vec<u8> {
    let n = c.n;
    // mem: n*3 shown, n*3 target, last period + 1
    if c.mem.len() < n * 6 + 1 {
        c.mem.resize(n * 6 + 1, 0.0);
    }
    let period = (c.t / 0.75) as u32 + 1;
    if c.mem[n * 6] as u32 != period {
        c.mem[n * 6] = period as f32;
        for i in 0..n {
            if rand(c.seed, i as u32, period) < 0.1 {
                let rgb = hsv_to_rgb(rand(c.seed, i as u32 + 1000, period), 1.0, rand(c.seed, i as u32 + 2000, period));
                for (m, ch) in c.mem[n * 3 + i * 3..][..3].iter_mut().zip(rgb) {
                    *m = f32::from(ch);
                }
            }
        }
    }
    let a = 1.0 - (-c.dt / 0.07).exp();
    for k in 0..n * 3 {
        let (shown, target) = (c.mem[k], c.mem[n * 3 + k]);
        c.mem[k] = if target > shown { shown + (target - shown) * a } else { target };
    }
    c.mem[..n * 3].iter().map(|&v| v as u8).collect()
}

/// snake.py: a snake of 10% of the LEDs, fading from the accent colour (red with a rainbow
/// accent) to black in HSV, crawling a LED every 12/65 s toward LED 0.
fn snake(c: &Ctx) -> Vec<u8> {
    let n = c.n;
    let len = (n / 10).max(1);
    let (h, s, v) = rgb_to_hsv(accent(c).unwrap_or([255, 0, 0]));
    let (mut inc, mut sleep) = (1usize, 12.0 / n as f32);
    while sleep < 0.05 {
        inc *= 2;
        sleep *= 2.0;
    }
    let moved = (c.t / sleep) as usize * inc;
    let head = (n - len + n - moved % n) % n;
    (0..n)
        .flat_map(|i| {
            let k = (i + n - head) % n;
            if k < len {
                let f = if len == 1 { 0.0 } else { k as f32 / (len - 1) as f32 };
                hsv_to_rgb(h * (1.0 - f), s * (1.0 - f), v * (1.0 - f))
            } else {
                [0, 0, 0]
            }
        })
        .collect()
}

/// sparks.py: every 50 ms each LED has a 0.5% chance to flash for that one frame. White in
/// Hyperion; here the accent colour, or random hues (the script's random-color option) with rainbow.
fn sparks(c: &Ctx) -> Vec<u8> {
    let step = (c.t / 0.05) as u32;
    let color = accent(c);
    (0..c.n)
        .flat_map(|i| {
            if rand(c.seed, i as u32, step) < 0.005 {
                color.unwrap_or_else(|| hsv_to_rgb(rand(c.seed, i as u32 + 500, step), 1.0, 1.0))
            } else {
                [0, 0, 0]
            }
        })
        .collect()
}

/// trails.py: 8 drops (the script reads its trail count from a misspelt key, so always 8) falling
/// down random columns of an image with a column per top LED and a row per side LED. Only the
/// outer columns and the top row reach the LEDs, so on an edge strip they show up now and then.
/// Accent colour: "Trails" (2–7 long, a step every 30 ms) in that colour; rainbow: "Trails color"
/// (random hues, 2–6 long, every 50 ms).
fn trails(c: &mut Ctx) -> Vec<u8> {
    let n = c.n;
    let [(_, r1), (t0, t1), (l0, l1)] = segment_bounds(n);
    let (w, h) = ((t1 - t0) as i64, (r1.max(l1 - l0)) as i64);
    let color = accent(c);
    let (tick, max_len) = if color.is_some() { (0.03, 7) } else { (0.05, 6) };
    let base = color.map(rgb_to_hsv);
    let now = (c.t / tick) as i64;
    // mem per trail: x, y, step, length, hue, sat, birth tick, generation
    if c.mem.len() < 64 {
        c.mem.resize(64, 0.0);
    }
    let mut columns: Vec<Option<[u8; 3]>> = vec![None; (w * h) as usize];
    for k in 0..8 {
        let m = &mut c.mem[k * 8..k * 8 + 8];
        loop {
            if m[7] == 0.0 || {
                let ticks_per_row = (1.0 / m[2]).floor() as i64 + 1;
                let rows_moved = (now - m[6] as i64) / ticks_per_row;
                rows_moved >= h - m[1] as i64 + m[3] as i64
            } {
                // (Re)start: born when the previous one ended (or now, the first time).
                let gen = m[7] as u32 + 1;
                let born = if m[7] == 0.0 {
                    now
                } else {
                    m[6] as i64 + ((1.0 / m[2]).floor() as i64 + 1) * (h - m[1] as i64 + m[3] as i64)
                };
                let r = |s: u32| rand(c.seed, k as u32 * 16 + s, gen);
                let (hue, sat) = base.map_or((r(5), 1.0), |(hh, ss, _)| (hh, ss));
                m.copy_from_slice(&[
                    (r(1) * (w + 1) as f32).floor(),
                    (r(2) * (h + 1) as f32).floor(),
                    0.2 + 0.6 * r(3),
                    2.0 + (r(4) * (max_len - 1) as f32).floor(),
                    hue,
                    sat,
                    born as f32,
                    gen as f32,
                ]);
                if born > now {
                    break;
                }
                continue;
            }
            break;
        }
        let (x, y, step, len) = (m[0] as i64, m[1] as i64, m[2], m[3] as i64);
        if x >= w || columns[x as usize].is_some() {
            continue;
        }
        let head = y - 1 + (now - m[6] as i64) / ((1.0 / step).floor() as i64 + 1);
        let value = base.map_or(1.0, |(_, _, v)| v);
        for row in 0..h {
            let behind = head - row;
            if (0..len).contains(&behind) {
                let rgb = hsv_to_rgb(m[4], m[5], value * (len - behind) as f32 / len as f32);
                columns[(row * w + x) as usize].get_or_insert(rgb);
            }
        }
        // Mark the column as taken by this trail even where it's dark (the script's first match wins).
        for row in 0..h {
            columns[(row * w + x) as usize].get_or_insert([0, 0, 0]);
        }
    }
    (0..n)
        .flat_map(|i| {
            let (u, v) = uv(i, n);
            let col = ((u * w as f32) as i64).min(w - 1);
            let row = ((v * h as f32) as i64).min(h - 1);
            let at = |x: i64, y: i64| columns[(y * w + x) as usize].unwrap_or([0, 0, 0]);
            if i < r1 {
                at(w - 1, row)
            } else if i < t1 {
                at(col, 0)
            } else {
                at(0, row)
            }
        })
        .collect()
}

/// Local wall-clock time as (hours, minutes, seconds).
fn local_time() -> (i32, i32, i32) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs() as libc::time_t;
    // SAFETY: localtime_r only writes into the tm we own.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    (tm.tm_hour, tm.tm_min, tm.tm_sec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RuntimeConfig;

    fn frame_at(mode: EffectMode, color: &str, t: f32) -> Vec<u8> {
        let mut cfg = RuntimeConfig::default();
        cfg.solid.color = color.into();
        let audio = crate::audio::AudioSnapshot::new();
        let mut mem = vec![0.0; 65 * 3];
        let dt = 1.0 / 45.0;
        let mut out = Vec::new();
        for frame in 1..=((t / dt) as u32).max(1) {
            let mut c = Ctx { n: 65, t: frame as f32 * dt, dt, frame, cfg: &cfg, mem: &mut mem, seed: 7, audio: &audio };
            out = render(mode, &mut c);
        }
        out
    }
    fn led(f: &[u8], i: usize) -> [u8; 3] {
        [f[i * 3], f[i * 3 + 1], f[i * 3 + 2]]
    }

    #[test]
    fn knight_rider_lights_a_whole_side_at_the_ends() {
        // At t = 0 the light is at column 0: the whole left side (LEDs 48..65) is fully lit.
        let f = frame_at(EffectMode::KnightRider, "rainbow", 0.0);
        assert!((48..65).all(|i| led(&f, i) == [255, 0, 0]), "{:?}", led(&f, 50));
        assert!((0..17).all(|i| led(&f, i)[0] < 10));
    }

    #[test]
    fn led_test_sequence_lights_leds_in_order() {
        let f = frame_at(EffectMode::LedTestSeq, "rainbow", 2.6); // step 5: LEDs 0..=5 lit
        assert_eq!((led(&f, 0), led(&f, 4), led(&f, 5), led(&f, 6)), ([255, 0, 0], [0, 255, 0], [0, 0, 255], [0, 0, 0]));
    }

    #[test]
    fn hyperion_fire_is_brightest_low_on_the_sides() {
        let f = frame_at(EffectMode::HyperionFire, "rainbow", 0.0);
        let lum = |i: usize| led(&f, i).iter().map(|&c| u32::from(c)).sum::<u32>();
        // LED 0 is the bottom of the right side, LED 16 its top corner.
        assert!(lum(0) > 3 * lum(16), "bottom {} vs top {}", lum(0), lum(16));
    }

    #[test]
    fn cinema_fades_then_stays() {
        let led0 = |mode, t| led(&frame_at(mode, "rainbow", t), 0);
        assert_eq!(led0(EffectMode::CinemaBrighten, 0.02), [136, 97, 7]);
        assert_eq!(led0(EffectMode::CinemaBrighten, 9.0), [238, 173, 47]);
        assert_eq!(led0(EffectMode::CinemaDim, 0.02), [238, 173, 47]);
        assert_eq!(led0(EffectMode::CinemaDim, 9.0), [136, 97, 7]);
        // Notify blue: three blinks (~1.5 s), then dark.
        assert_eq!(led0(EffectMode::NotifyBlue, 0.42), [0, 0, 50]); // holding dim between blinks
        assert_eq!(led0(EffectMode::NotifyBlue, 3.0), [0, 0, 0]);
    }

    #[test]
    fn xmas_swaps_every_750_ms() {
        let (a, b) = (frame_at(EffectMode::Xmas, "rainbow", 0.3), frame_at(EffectMode::Xmas, "rainbow", 1.0));
        assert_eq!((led(&a, 0), led(&a, 1)), ([255, 255, 255], [255, 0, 0]));
        assert_eq!((led(&b, 0), led(&b, 1)), ([255, 0, 0], [255, 255, 255]));
    }

    #[test]
    fn police_single_is_two_blocks_of_eleven() {
        let f = frame_at(EffectMode::Police, "rainbow", 0.0);
        let count = |rgb: [u8; 3]| (0..65).filter(|&i| led(&f, i) == rgb).count();
        assert_eq!((count([255, 0, 0]), count([0, 0, 255])), (11, 11));
    }

    #[test]
    fn collision_bursts_where_the_dots_meet() {
        // 31 flying steps of 0.1 s, then the burst starts at step 32.
        let flying = frame_at(EffectMode::Collision, "rainbow", 1.0);
        let lit = |f: &[u8]| f.chunks(3).filter(|p| p.iter().any(|&c| c > 0)).count();
        assert_eq!(lit(&flying), 10, "two 5-LED trails");
        let burst = frame_at(EffectMode::Collision, "rainbow", 3.85);
        assert!(lit(&burst) > 10, "burst wider than the trails: {}", lit(&burst));
    }
}
