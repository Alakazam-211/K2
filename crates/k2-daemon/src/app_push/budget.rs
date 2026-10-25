//! Sliding one-hour windows for App push (prd-custom-app-push-v1 CP8,
//! CP33). In memory: a restart starts every window afresh (the limits
//! protect the box and the guests, they are not a ledger).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One family of windows (registers per pass, sends per App, …).
pub struct Limiter {
    window: Duration,
    map: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl Limiter {
    pub fn new(window: Duration) -> Self {
        Limiter { window, map: Mutex::new(HashMap::new()) }
    }

    fn trim(q: &mut VecDeque<Instant>, window: Duration, now: Instant) {
        while q.front().is_some_and(|t| now.saturating_duration_since(*t) >= window) {
            q.pop_front();
        }
    }

    /// Take `n` from `key`'s window of `max`, all or nothing. `Err` = the
    /// milliseconds until enough of the window frees.
    pub fn take_n_at(&self, key: &str, n: usize, max: usize, now: Instant) -> Result<(), u64> {
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if m.len() > 50_000 {
            let w = self.window;
            m.retain(|_, q| {
                Self::trim(q, w, now);
                !q.is_empty()
            });
        }
        let q = m.entry(key.to_string()).or_default();
        Self::trim(q, self.window, now);
        if n > max {
            return Err(self.window.as_millis() as u64);
        }
        if q.len() + n > max {
            // The (len + n - max)-th oldest hit must leave the window.
            let need = q.len() + n - max;
            let at = q.get(need - 1).copied().unwrap_or(now);
            let wait = self.window.saturating_sub(now.saturating_duration_since(at));
            return Err(wait.as_millis().max(1) as u64);
        }
        for _ in 0..n {
            q.push_back(now);
        }
        Ok(())
    }

    pub fn take_at(&self, key: &str, max: usize, now: Instant) -> Result<(), u64> {
        self.take_n_at(key, 1, max, now)
    }

    pub fn take(&self, key: &str, max: usize) -> Result<(), u64> {
        self.take_at(key, max, Instant::now())
    }

    pub fn take_n(&self, key: &str, n: usize, max: usize) -> Result<(), u64> {
        self.take_n_at(key, n, max, Instant::now())
    }

    /// Forget every window (tests).
    pub fn clear(&self) {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_refuses_past_max_and_frees_as_hits_age() {
        let l = Limiter::new(Duration::from_secs(3600));
        let t0 = Instant::now();
        for _ in 0..3 {
            l.take_at("k", 3, t0).expect("under max");
        }
        let wait = l.take_at("k", 3, t0 + Duration::from_secs(600)).expect_err("over max");
        assert_eq!(wait, 3_000_000, "the oldest hit leaves 50 minutes later");
        l.take_at("other", 3, t0).expect("per key");
        l.take_at("k", 3, t0 + Duration::from_secs(3600)).expect("the first hit aged out");
        assert!(l.take_n_at("n", 4, 3, t0).is_err(), "more than max at once never fits");
        l.take_n_at("n", 2, 3, t0).expect("two");
        assert!(l.take_n_at("n", 2, 3, t0).is_err(), "all or nothing");
        l.take_n_at("n", 1, 3, t0).expect("the one left");
    }
}
