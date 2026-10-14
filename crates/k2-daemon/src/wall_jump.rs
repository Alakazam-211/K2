//! Sleep/wake detection by wall-vs-monotonic drift (contract §3.1, GV38).
//!
//! On suspend the monotonic clock (and every task) pauses with the
//! machine, so on wake the wall clock has moved much further than the
//! monotonic one. Owner: B1, who moves `heartbeat_monitor`'s copy of this
//! check onto [`Detector`] and uses it in the sources scheduler.

use std::time::Instant;

/// A detected jump: the wall clock moved `wall_delta` seconds while the
/// monotonic clock moved `mono_delta`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Jump {
    /// Wall seconds at the previous observation.
    pub from: i64,
    /// Wall seconds now.
    pub to: i64,
    pub wall_delta: i64,
    pub mono_delta: i64,
}

#[derive(Debug, Clone)]
pub struct Detector {
    threshold_secs: i64,
    last_mono: Instant,
    last_wall: i64,
}

impl Detector {
    pub fn new(threshold_secs: i64) -> Detector {
        Detector::starting_at(threshold_secs, Instant::now(), chrono::Utc::now().timestamp())
    }

    /// A detector whose first observation is `(mono, wall)` (tests).
    pub fn starting_at(threshold_secs: i64, mono: Instant, wall: i64) -> Detector {
        Detector { threshold_secs, last_mono: mono, last_wall: wall }
    }

    /// Observe the clocks now.
    pub fn observe(&mut self) -> Option<Jump> {
        self.observe_at(Instant::now(), chrono::Utc::now().timestamp())
    }

    /// Observe `(mono, wall)`. A jump is the wall clock gaining more than
    /// the threshold over the monotonic clock. A backwards wall step is not
    /// a jump.
    pub fn observe_at(&mut self, mono: Instant, wall: i64) -> Option<Jump> {
        let mono_delta = mono.saturating_duration_since(self.last_mono).as_secs() as i64;
        let wall_delta = wall - self.last_wall;
        let from = self.last_wall;
        self.last_mono = mono;
        self.last_wall = wall;
        (wall_delta - mono_delta > self.threshold_secs).then_some(Jump { from, to: wall, wall_delta, mono_delta })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_long_sleep_is_a_jump_and_steady_time_is_not() {
        let t0 = Instant::now();
        let mut d = Detector::starting_at(120, t0, 1_000);
        assert_eq!(d.observe_at(t0 + Duration::from_secs(5), 1_005), None);
        let j = d.observe_at(t0 + Duration::from_secs(10), 1_005 + 6 * 3600).expect("6 h sleep");
        assert_eq!((j.from, j.to), (1_005, 1_005 + 6 * 3600));
        assert_eq!(d.observe_at(t0 + Duration::from_secs(15), 1_005 + 6 * 3600 + 5), None);
        // Backwards wall step: not a jump.
        assert_eq!(d.observe_at(t0 + Duration::from_secs(20), 900), None);
    }
}
