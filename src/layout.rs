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
