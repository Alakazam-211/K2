//! Per-agent send limiter for external agents. Every send to Grok Bot runs
//! a billed turn, so a stuck client or a double click must not fire a burst.
//!
//! Two limits per agent: a minimum gap between sends, and a cap per rolling
//! window. Time is passed in so tests drive it without sleeping.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// Minimum gap between two sends to the same agent.
pub const MIN_GAP_MS: i64 = 2_000;
/// Rolling window for [`MAX_PER_WINDOW`].
pub const WINDOW_MS: i64 = 10 * 60 * 1_000;
/// Most sends to one agent per [`WINDOW_MS`].
pub const MAX_PER_WINDOW: usize = 30;

#[derive(Debug, Default)]
pub struct SendLimiter {
    sent: Mutex<HashMap<String, VecDeque<i64>>>,
}

impl SendLimiter {
    /// Ok(()) and record the send, or Err(retry_after_ms).
    pub fn check_and_record(&self, agent_id: &str, now_ms: i64) -> Result<(), i64> {
        let mut map = self.sent.lock().unwrap_or_else(|p| p.into_inner());
        let q = map.entry(agent_id.to_string()).or_default();
        while q.front().is_some_and(|&t| now_ms - t >= WINDOW_MS) {
            q.pop_front();
        }
        if let Some(&last) = q.back() {
            let gap = now_ms - last;
            if gap < MIN_GAP_MS {
                return Err(MIN_GAP_MS - gap);
            }
        }
        if q.len() >= MAX_PER_WINDOW {
            let oldest = *q.front().expect("non-empty at cap");
            return Err(WINDOW_MS - (now_ms - oldest));
        }
        q.push_back(now_ms);
        Ok(())
    }

    /// Take back a send recorded at `at_ms` that never ran (refused before
    /// it reached the product), so it doesn't count against the limits.
    pub fn unrecord(&self, agent_id: &str, at_ms: i64) {
        let mut map = self.sent.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(q) = map.get_mut(agent_id) {
            if let Some(i) = q.iter().rposition(|&t| t == at_ms) {
                q.remove(i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_and_window_limits_hold_per_agent() {
        let l = SendLimiter::default();
        assert_eq!(l.check_and_record("a", 0), Ok(()));
        assert_eq!(l.check_and_record("a", 500), Err(1_500));
        // another agent is independent
        assert_eq!(l.check_and_record("b", 500), Ok(()));
        let mut t = 0;
        for _ in 1..MAX_PER_WINDOW {
            t += MIN_GAP_MS;
            assert_eq!(l.check_and_record("a", t), Ok(()));
        }
        t += MIN_GAP_MS;
        let err = l.check_and_record("a", t).expect_err("window cap");
        assert_eq!(err, WINDOW_MS - t);
        // after the window passes, sends are allowed again
        assert_eq!(l.check_and_record("a", WINDOW_MS + 1), Ok(()));
    }
}
