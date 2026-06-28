/// Attack/release envelope follower (seconds).
pub struct Envelope {
    value: f32,
    attack: f32,
    release: f32,
}

impl Envelope {
    pub fn new(attack_ms: f32, release_ms: f32) -> Self {
        Self {
            value: 0.0,
            attack: attack_ms / 1000.0,
            release: release_ms / 1000.0,
        }
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn set_timing(&mut self, attack_ms: f32, release_ms: f32) {
        self.attack = (attack_ms / 1000.0).max(1e-4);
        self.release = (release_ms / 1000.0).max(1e-4);
    }

    pub fn tick(&mut self, target: f32, dt: f32) {
        let target = target.clamp(0.0, 1.0);
        let tau = if target > self.value {
            self.attack.max(1e-4)
        } else {
            self.release.max(1e-4)
        };
        let alpha = 1.0 - (-dt / tau).exp();
        self.value += (target - self.value) * alpha;
    }
}
