//! Audio meter/envelope tuning from the user's sensitivity (input drive) and smoothing (fall-back speed).

pub struct Dynamics {
    pub drive: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub block_samples: usize,
    pub shape_pow: f32,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

pub fn from_settings(sensitivity: f32, smoothing: f32) -> Dynamics {
    let (s, m) = (sensitivity.clamp(0.0, 1.0), smoothing.clamp(0.0, 1.0));
    Dynamics {
        drive: lerp(3.5, 7.5, s),
        attack_ms: lerp(4.0, 40.0, m),
        release_ms: lerp(50.0, 600.0, m),
        block_samples: lerp(256.0, 96.0, s).round() as usize,
        shape_pow: lerp(0.92, 0.78, s),
    }
}

pub fn meter_gain(peak: f32, drive: f32) -> f32 {
    let d = peak.max(0.0) * drive;
    (d / (1.0 + d * 0.95)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::{from_settings, meter_gain};

    #[test]
    fn midpoint_near_tuned_defaults() {
        let d = from_settings(0.5, 0.15);
        assert!((d.drive - 5.5).abs() < 0.5);
        assert!(meter_gain(0.15, d.drive) < 0.65);
    }
}
