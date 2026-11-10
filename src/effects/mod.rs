pub mod animated;
pub mod candle;
pub mod solid;
pub mod sound_viz;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{EffectMode, RuntimeConfig};
use crate::daemon::{DaemonStatus, FpsMeter};
use crate::effects::solid::is_rainbow_color;
use crate::protocol::build_frame;
use crate::serial::SerialWriter;

#[cfg(feature = "audio")]
use crate::audio::{maybe_modulate, AudioSnapshot};

/// What a renderer gets each frame.
pub struct Ctx<'a> {
    pub n: usize,
    /// Effect time: seconds × speed, accumulated so a speed change never jumps the animation.
    pub t: f32,
    /// Effect time since the previous frame.
    pub dt: f32,
    pub frame: u32,
    pub cfg: &'a RuntimeConfig,
    /// Per-LED memory kept between frames, 3 floats per LED; zeroed when the effect starts.
    pub mem: &'a mut [f32],
    #[cfg(feature = "audio")]
    pub audio: &'a AudioSnapshot,
}

/// Run one effect until cancelled or the mode changes. Renderers return full-brightness RGB.
pub fn run(
    mode: EffectMode,
    writer: Arc<Mutex<SerialWriter>>,
    config: Arc<RwLock<RuntimeConfig>>,
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<DaemonStatus>>,
    #[cfg(feature = "audio")] audio: Arc<AudioSnapshot>,
) -> anyhow::Result<()> {
    let n = usize::from(config.read().unwrap().device.leds);
    let mut mem = vec![0.0f32; n * 3];
    let mut fps_meter = FpsMeter::new();
    let (mut t, mut frame) = (0.0f32, 0u32);
    let mut last = Instant::now();
    let mut next = last;

    while !cancel.load(Ordering::Relaxed) {
        let cfg = config.read().unwrap().clone();
        if cfg.effect.mode != mode {
            break;
        }
        let now = Instant::now();
        // With a sound mode on, music drives the clock: animations surge on beats.
        #[cfg(feature = "audio")]
        let surge = if cfg.audio.sound_mode == crate::config::SoundMode::Off {
            1.0
        } else {
            1.0 + 2.0 * cfg.audio.reactivity * audio.bass_level()
        };
        #[cfg(not(feature = "audio"))]
        let surge = 1.0;
        let dt = now.duration_since(last).as_secs_f32() * cfg.effect.speed.max(0.1) * surge;
        last = now;
        t += dt;
        frame = frame.wrapping_add(1);

        let mut rgb = render(
            mode,
            &mut Ctx {
                n,
                t,
                dt,
                frame,
                cfg: &cfg,
                mem: &mut mem,
                #[cfg(feature = "audio")]
                audio: &audio,
            },
        );
        finish(&mut rgb, &cfg, 1.0, #[cfg(feature = "audio")] &audio);
        let shown = to_hex(&rgb);
        writer.lock().unwrap().write_frame(&build_frame(cfg.device.leds, &rgb)?)?;

        let actual = fps_meter.tick();
        {
            let mut st = status.lock().unwrap();
            st.brightness = cfg.effect.brightness;
            st.fps = cfg.effect.fps;
            st.actual_fps = actual;
            st.speed = cfg.effect.speed;
            st.serial_ok = true;
            st.color = cfg.solid.color.clone();
            st.leds = shown;
            st.detail = match mode {
                EffectMode::Solid if is_rainbow_color(&cfg.solid.color) => "rainbow".into(),
                EffectMode::Solid => format!("#{}", cfg.solid.color),
                _ => mode.as_str().into(),
            };
            #[cfg(feature = "audio")]
            {
                st.audio_level = audio.level();
            }
        }

        // Sleep until the next frame is due — the ~17 ms serial write already used part of it.
        next += Duration::from_secs_f64(1.0 / f64::from(cfg.effect.fps.max(1)));
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    Ok(())
}

fn render(mode: EffectMode, c: &mut Ctx) -> Vec<u8> {
    match mode {
        EffectMode::Solid | EffectMode::Rainbow => solid::render(c),
        EffectMode::Candle => candle::render(c),
        EffectMode::SoundViz => sound_viz::render(c),
        _ => animated::render(mode, c),
    }
}

/// Screen locked: LEDs stay dark until unlock.
pub static LOCKED: AtomicBool = AtomicBool::new(false);
/// `ctl flash`: (color, start, end) blinked over whatever is running.
static FLASH: Mutex<Option<([u8; 3], Instant, Instant)>> = Mutex::new(None);
const FLASH_HALF: Duration = Duration::from_millis(150);

pub fn flash(color: [u8; 3], count: u32) {
    let now = Instant::now();
    *FLASH.lock().unwrap() = Some((color, now, now + FLASH_HALF * 2 * count.max(1)));
}

/// Last step for every frame (effects and screen sync): gamma, brightness, sound boost, lock/flash.
pub fn finish(
    rgb: &mut [u8],
    cfg: &RuntimeConfig,
    gamma: f32,
    #[cfg(feature = "audio")] audio: &AudioSnapshot,
) {
    let (g, b) = (gamma.max(0.1), cfg.effect.brightness.clamp(0.0, 1.0));
    for c in rgb.iter_mut() {
        *c = ((f32::from(*c) / 255.0).powf(g) * b * 255.0).round() as u8;
    }
    #[cfg(feature = "audio")]
    maybe_modulate(rgb, rgb.len() / 3, cfg, audio);

    if LOCKED.load(Ordering::Relaxed) {
        rgb.fill(0);
    } else if let Some((color, start, end)) = *FLASH.lock().unwrap() {
        let now = Instant::now();
        if now < end {
            let on = ((now - start).as_millis() / FLASH_HALF.as_millis()).is_multiple_of(2);
            for px in rgb.chunks_exact_mut(3) {
                for (out, c) in px.iter_mut().zip(color) {
                    *out = if on { (f32::from(c) * b) as u8 } else { 0 };
                }
            }
        }
    }
}

/// LED bytes as hex, for the status (the TUI's strip preview).
pub fn to_hex(rgb: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(rgb.len() * 2);
    for b in rgb {
        let _ = write!(s, "{b:02x}");
    }
    s
}

impl Ctx<'_> {
    /// How far an effect may swing from steady full color: `design` at the default power (0.5),
    /// nothing at 0, twice as much (max 1) at 1.
    pub fn depth(&self, design: f32) -> f32 {
        (design * self.cfg.effect.power.clamp(0.0, 1.0) * 2.0).min(1.0)
    }
}

/// Gain for a 0..1 animation `wave` that may dim the light by at most `depth`.
pub fn swing(wave: f32, depth: f32) -> f32 {
    1.0 - depth * (1.0 - wave.clamp(0.0, 1.0))
}

/// Deterministic hash of (a, b) to [0, 1).
pub fn hash(a: u32, b: u32) -> f32 {
    let mut x = a.wrapping_mul(0x9E37_79B9) ^ b.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// Smooth value noise in [0, 1): a new random target every unit of `t`, eased in between.
pub fn noise(seed: u32, t: f32) -> f32 {
    let k = t.floor();
    let f = t - k;
    let s = f * f * (3.0 - 2.0 * f);
    let k = k as i64 as u32;
    hash(seed, k) * (1.0 - s) + hash(seed, k.wrapping_add(1)) * s
}
