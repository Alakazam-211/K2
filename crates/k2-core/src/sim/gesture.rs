//! Flick velocity for people's drags on the stream (0.46.7 Mobile gestures).
//!
//! Neither lane takes a velocity: Android's VelocityTracker and UIKit both
//! read it from the timing of the last few moves before the finger lifts.
//! Moves that crossed the network (or queued behind a slow input) arrive
//! bunched, so the device measures the wrong speed and a flick stops dead
//! or shoots off. The viewer measures the release velocity on its own clock
//! and sends it with `touch` up; the daemon then replays a short tail of
//! moves at that speed, spaced on the device's clock, and lifts at its end.

use std::time::Duration;

use super::lane::Point;

/// Below this (points per second) a release is a stop, not a flick.
pub const MIN_FLING_PT_S: f32 = 60.0;
/// A faster release is clamped (a phone's own fling cap is about this).
pub const MAX_FLING_PT_S: f32 = 8000.0;
/// Moves in the tail, and the gap between them on the device.
pub const TAIL_STEPS: u32 = 4;
pub const TAIL_STEP: Duration = Duration::from_millis(8);

/// The tail of moves for a release at `at` with velocity `v` (points per
/// second) on a `screen` (width, height in points, current orientation).
/// Empty for a slow release. Every point stays on the screen; when the
/// edge cuts the tail short, the remaining moves hold at the edge (the
/// device then reads a slower flick, never a wrong direction).
pub fn fling_tail(at: Point, v: (f32, f32), screen: (f32, f32)) -> Vec<Point> {
    let (vx, vy) = v;
    if !vx.is_finite() || !vy.is_finite() {
        return Vec::new();
    }
    let speed = (vx * vx + vy * vy).sqrt();
    if speed < MIN_FLING_PT_S {
        return Vec::new();
    }
    let k = if speed > MAX_FLING_PT_S { MAX_FLING_PT_S / speed } else { 1.0 };
    let dt = TAIL_STEP.as_secs_f32();
    let (sx, sy) = (vx * k * dt, vy * k * dt);
    let max_x = (screen.0 - 1.0).max(0.0);
    let max_y = (screen.1 - 1.0).max(0.0);
    (1..=TAIL_STEPS)
        .map(|i| {
            let f = i as f32;
            Point { x: (at.x + sx * f).clamp(0.0, max_x), y: (at.y + sy * f).clamp(0.0, max_y) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (f32, f32) = (400.0, 800.0);

    #[test]
    fn a_slow_release_has_no_tail() {
        assert!(fling_tail(Point { x: 200.0, y: 400.0 }, (10.0, 40.0), SCREEN).is_empty());
        assert!(fling_tail(Point { x: 200.0, y: 400.0 }, (f32::NAN, 4000.0), SCREEN).is_empty());
    }

    #[test]
    fn a_flick_continues_at_its_speed_on_the_device_clock() {
        // 2000 pt/s up the screen: 16 pt per 8 ms step.
        let tail = fling_tail(Point { x: 200.0, y: 600.0 }, (0.0, -2000.0), SCREEN);
        assert_eq!(tail.len(), TAIL_STEPS as usize);
        let ys: Vec<f32> = tail.iter().map(|p| (p.y * 100.0).round() / 100.0).collect();
        assert_eq!(ys, vec![584.0, 568.0, 552.0, 536.0]);
        assert!(tail.iter().all(|p| p.x == 200.0));
    }

    #[test]
    fn the_tail_stays_on_screen_and_the_speed_is_capped() {
        let tail = fling_tail(Point { x: 390.0, y: 20.0 }, (3000.0, -3000.0), SCREEN);
        assert!(tail.iter().all(|p| p.x >= 0.0 && p.x <= 399.0 && p.y >= 0.0 && p.y <= 799.0), "{tail:?}");
        assert_eq!(tail.last().map(|p| (p.x, p.y)), Some((399.0, 0.0)));
        // 80 000 pt/s is clamped to MAX_FLING_PT_S: 64 pt per step.
        let fast = fling_tail(Point { x: 200.0, y: 700.0 }, (0.0, -80_000.0), SCREEN);
        assert!((fast[0].y - (700.0 - MAX_FLING_PT_S * TAIL_STEP.as_secs_f32())).abs() < 1e-3, "{fast:?}");
    }
}
