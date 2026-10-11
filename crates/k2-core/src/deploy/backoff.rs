//! Restart backoff and the crash-loop breaker (§6.3). Backoff lives in the
//! K2 supervisor, not systemd (`RestartSteps=` needs systemd 254).
//!
//! - After the watch window: restart at once, then 5 s → 30 s → 2 min →
//!   10 min → 30 min. The step resets after 10 healthy minutes.
//! - Crash-loop breaker: 6 restarts within an hour → `crashlooping`;
//!   restarts stop and the App's agent is told.
//!
//! Pure: the caller passes the clock (unix seconds), so tests use a fake.

use std::collections::VecDeque;

/// Seconds before restart N (0-based): at once, then the PRD's steps.
pub const SCHEDULE: &[i64] = &[0, 5, 30, 120, 600, 1800];
/// Healthy this long resets the step.
pub const RESET_AFTER_SECS: i64 = 600;
/// This many restarts inside [`BREAKER_WINDOW_SECS`] trips the breaker.
pub const BREAKER_RESTARTS: usize = 6;
pub const BREAKER_WINDOW_SECS: i64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Restart after this many seconds.
    Restart { after_secs: i64 },
    /// Stop restarting: `crashlooping`.
    Breaker,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Backoff {
    step: usize,
    restarts: VecDeque<i64>,
    healthy_since: Option<i64>,
}

impl Backoff {
    /// The instance died (or failed its health checks) at `now`.
    pub fn on_crash(&mut self, now: i64) -> Decision {
        self.healthy_since = None;
        while self.restarts.front().is_some_and(|t| now - *t >= BREAKER_WINDOW_SECS) {
            self.restarts.pop_front();
        }
        if self.restarts.len() >= BREAKER_RESTARTS {
            return Decision::Breaker;
        }
        self.restarts.push_back(now);
        let after = SCHEDULE[self.step.min(SCHEDULE.len() - 1)];
        self.step += 1;
        Decision::Restart { after_secs: after }
    }

    /// The instance is up and healthy at `now`; after
    /// [`RESET_AFTER_SECS`] of that the schedule starts over.
    pub fn on_healthy(&mut self, now: i64) {
        let since = *self.healthy_since.get_or_insert(now);
        if now - since >= RESET_AFTER_SECS {
            self.step = 0;
        }
    }

    /// Restarts counted inside the breaker window.
    pub fn recent_restarts(&self) -> usize {
        self.restarts.len()
    }

    /// The owner fixed it (`k2 app start` after `crashlooping`).
    pub fn reset(&mut self) {
        *self = Backoff::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_steps_and_resets_after_ten_healthy_minutes() {
        let mut b = Backoff::default();
        let mut now = 1_000_000;
        let delays: Vec<i64> = (0..4)
            .map(|_| {
                now += 1000; // spread out so the breaker window doesn't matter here
                match b.on_crash(now) {
                    Decision::Restart { after_secs } => after_secs,
                    Decision::Breaker => panic!("breaker too early"),
                }
            })
            .collect();
        assert_eq!(delays, vec![0, 5, 30, 120]);
        b.on_healthy(now + 10);
        b.on_healthy(now + 10 + RESET_AFTER_SECS - 1);
        assert_eq!(b.on_crash(now + 2000), Decision::Restart { after_secs: 600 }, "9m59s healthy is not a reset");
        b.on_healthy(now + 3000);
        b.on_healthy(now + 3000 + RESET_AFTER_SECS);
        assert_eq!(b.on_crash(now + 5000), Decision::Restart { after_secs: 0 }, "10 healthy minutes reset the step");
    }

    #[test]
    fn breaker_trips_at_six_restarts_in_an_hour_and_the_window_slides() {
        let mut b = Backoff::default();
        let t0 = 5_000_000;
        for i in 0..BREAKER_RESTARTS as i64 {
            assert!(matches!(b.on_crash(t0 + i * 60), Decision::Restart { .. }), "restart {i}");
        }
        assert_eq!(b.on_crash(t0 + 400), Decision::Breaker);
        assert_eq!(b.recent_restarts(), 6);
        // An hour after the first restart, it slides out of the window.
        assert!(matches!(b.on_crash(t0 + BREAKER_WINDOW_SECS), Decision::Restart { .. }));
        b.reset();
        assert_eq!(b.recent_restarts(), 0);
        assert_eq!(b.on_crash(t0), Decision::Restart { after_secs: 0 });
    }
}
