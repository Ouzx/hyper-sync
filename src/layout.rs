#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedZone {
    Right,
    Top,
    Left,
}

/// Physical strip zones (17 right / 31 top / 17 left @ 65 LEDs).
pub fn segment_bounds(n: usize) -> [(usize, usize); 3] {
    let right = n * 17 / 65;
    let top = n * 31 / 65;
    let left = n.saturating_sub(right + top);
    [(0, right), (right, right + top), (right + top, right + top + left)]
}

/// How high an LED sits on the monitor: 0 at the bottom of either side, 1 along the top.
pub fn height(i: usize, n: usize) -> f32 {
    let [(_, r1), (_, t1), (l0, l1)] = segment_bounds(n);
    if i < r1 {
        i as f32 / r1.saturating_sub(1).max(1) as f32
    } else if i < t1 {
        1.0
    } else {
        (l1 - 1 - i.min(l1 - 1)) as f32 / (l1 - l0).saturating_sub(1).max(1) as f32
    }
}

pub fn led_zone(i: usize, n: usize) -> LedZone {
    let [(r0, r1), (t0, t1), _] = segment_bounds(n);
    if i >= r0 && i < r1 {
        LedZone::Right
    } else if i >= t0 && i < t1 {
        LedZone::Top
    } else {
        LedZone::Left
    }
}
