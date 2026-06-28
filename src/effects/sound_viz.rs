use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use crate::audio::AudioSnapshot;
use crate::config::{EffectMode, RuntimeConfig};
use crate::daemon::DaemonStatus;
use crate::effects::solid::resolve_pixel_color;
use crate::layout::segment_bounds;
use crate::protocol::build_frame;
use crate::serial::SerialWriter;

pub fn run_controlled(
    writer: Arc<Mutex<SerialWriter>>,
    config: Arc<RwLock<RuntimeConfig>>,
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<DaemonStatus>>,
    audio: Arc<AudioSnapshot>,
) -> anyhow::Result<()> {
    let n = config.read().unwrap().device.leds as usize;
    let mut display = vec![0.0f32; crate::audio::SPECTRUM_BANDS];

    while !cancel.load(Ordering::Relaxed) {
        let cfg = config.read().unwrap().clone();
        if cfg.effect.mode != EffectMode::SoundViz {
            break;
        }

        let interval = Duration::from_micros(1_000_000 / u64::from(cfg.effect.fps.max(1)));
        let bands = audio.spectrum();
        for (d, &b) in display.iter_mut().zip(bands.iter()) {
            *d = (*d * 0.85 + b * 0.15).max(b * 0.7);
        }

        let accent = cfg.solid.color.as_str();
        let mut rgb = vec![0u8; n * 3];
        let bounds = segment_bounds(n);
        let zones = [bounds[0], bounds[1], bounds[2]];
        let bands_per_zone = [
            crate::audio::SPECTRUM_BANDS / 3,
            crate::audio::SPECTRUM_BANDS / 3,
            crate::audio::SPECTRUM_BANDS - 2 * (crate::audio::SPECTRUM_BANDS / 3),
        ];

        for (zi, (start_i, end_i)) in zones.iter().enumerate() {
            let band_start = bands_per_zone[..zi].iter().sum::<usize>();
            for i in *start_i..(*end_i).min(n) {
                let local = if end_i > start_i {
                    (i - start_i) as f32 / (end_i - start_i - 1).max(1) as f32
                } else {
                    0.0
                };
                let band_idx = band_start
                    + (local * (bands_per_zone[zi].saturating_sub(1)) as f32).round() as usize;
                let energy = display[band_idx.min(crate::audio::SPECTRUM_BANDS - 1)];
                let color = resolve_pixel_color(i, n, accent);
                let gain = energy * cfg.effect.brightness;
                let base = i * 3;
                rgb[base] = (f32::from(color[0]) * gain) as u8;
                rgb[base + 1] = (f32::from(color[1]) * gain) as u8;
                rgb[base + 2] = (f32::from(color[2]) * gain) as u8;
            }
        }

        let frame = build_frame(cfg.device.leds, &rgb)?;
        writer.lock().unwrap().write_frame(&frame)?;
        {
            let mut st = status.lock().unwrap();
            st.brightness = cfg.effect.brightness;
            st.fps = cfg.effect.fps;
            st.serial_ok = true;
            st.detail = "sound_viz".into();
            st.color = cfg.solid.color.clone();
            st.audio_level = audio.level();
        }
        thread::sleep(interval);
    }
    Ok(())
}
