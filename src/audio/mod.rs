pub mod dynamics;
pub mod envelope;
pub mod modulate;
pub mod monitor;
pub mod spectrum;
pub mod state;

pub use modulate::modulate_rgb;
pub use monitor::AudioMonitor;
pub use state::{AudioSnapshot, SPECTRUM_BANDS};

use crate::config::{EffectMode, RuntimeConfig, SoundMode};

pub fn audio_needed(cfg: &RuntimeConfig) -> bool {
    cfg.audio.sound_mode != SoundMode::Off || cfg.effect.mode == EffectMode::SoundViz
}

pub fn maybe_modulate(
    rgb: &mut [u8],
    n: usize,
    cfg: &RuntimeConfig,
    snap: &AudioSnapshot,
) {
    if cfg.effect.mode == EffectMode::SoundViz || cfg.audio.sound_mode == SoundMode::Off {
        return;
    }
    modulate_rgb(
        rgb,
        n,
        cfg.effect.brightness,
        cfg.audio.sound_mode,
        cfg.audio.reactivity,
        cfg.audio.sensitivity,
        snap,
    );
}
