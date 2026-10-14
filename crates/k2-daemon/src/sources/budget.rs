//! Per-origin budgets (PRD §5.3, GV13, GV59): one token bucket per origin
//! shared by every source and target on it (1 request per 2 s, burst 4),
//! and parks from `Retry-After` / spent `X-RateLimit-*` windows (capped at
//! 1 h). The global in-flight cap lives in the scheduler.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use k2_core::sources::{Reason, ORIGIN_BURST, ORIGIN_RATE_PER_SEC, PARK_MAX_SECS};

#[derive(Debug, Clone)]
struct Bucket {
    tokens: f64,
    last: Instant,
    parked: Option<Park>,
}

/// An origin that asked K2 to wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Park {
    pub until: Instant,
    /// Wall seconds, for display.
    pub until_at: i64,
    pub reason: Reason,
}

/// What a fetch for one origin may do now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Take {
    Go,
    /// No token yet; one arrives at this instant.
    Wait(Instant),
    /// The origin is parked.
    Parked(Park),
}

#[derive(Debug, Default)]
pub struct Budget {
    buckets: HashMap<String, Bucket>,
}

impl Budget {
    pub fn new() -> Budget {
        Budget::default()
    }

    fn bucket(&mut self, origin: &str, now: Instant) -> &mut Bucket {
        self.buckets
            .entry(origin.to_string())
            .or_insert_with(|| Bucket { tokens: f64::from(ORIGIN_BURST), last: now, parked: None })
    }

    /// Take a token for `origin` if one is there.
    pub fn try_take(&mut self, origin: &str, now: Instant) -> Take {
        let b = self.bucket(origin, now);
        if let Some(p) = &b.parked {
            if now < p.until {
                return Take::Parked(p.clone());
            }
            b.parked = None;
        }
        let elapsed = now.saturating_duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * ORIGIN_RATE_PER_SEC).min(f64::from(ORIGIN_BURST));
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Take::Go
        } else {
            let wait = (1.0 - b.tokens) / ORIGIN_RATE_PER_SEC;
            Take::Wait(now + Duration::from_secs_f64(wait))
        }
    }

    /// Park `origin` for `secs` (capped at `PARK_MAX_SECS`). A longer park
    /// already in place stays.
    pub fn park(&mut self, origin: &str, secs: u32, reason: Reason, now: Instant, now_wall: i64) {
        let secs = secs.clamp(1, PARK_MAX_SECS);
        let until = now + Duration::from_secs(u64::from(secs));
        let b = self.bucket(origin, now);
        if b.parked.as_ref().is_some_and(|p| p.until >= until) {
            return;
        }
        b.parked = Some(Park { until, until_at: now_wall + i64::from(secs), reason });
    }

    /// The park on `origin`, if it still holds.
    pub fn parked(&self, origin: &str, now: Instant) -> Option<Park> {
        self.buckets.get(origin).and_then(|b| b.parked.clone()).filter(|p| now < p.until)
    }

    /// Every origin parked now, for `GET /cli/sources/status`.
    pub fn parks(&self, now: Instant) -> Vec<(String, Park)> {
        let mut v: Vec<(String, Park)> = self
            .buckets
            .iter()
            .filter_map(|(o, b)| b.parked.clone().filter(|p| now < p.until).map(|p| (o.clone(), p)))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Lift a park (a person pressed Refresh after fixing something).
    pub fn unpark(&mut self, origin: &str) {
        if let Some(b) = self.buckets.get_mut(origin) {
            b.parked = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_four_then_one_every_two_seconds_per_origin() {
        let mut b = Budget::new();
        let t0 = Instant::now();
        for _ in 0..4 {
            assert_eq!(b.try_take("https://a:443", t0), Take::Go);
        }
        match b.try_take("https://a:443", t0) {
            Take::Wait(at) => assert_eq!(at.duration_since(t0).as_secs_f64().round(), 2.0),
            other => panic!("{other:?}"),
        }
        assert_eq!(b.try_take("https://b:443", t0), Take::Go, "another origin has its own bucket");
        assert!(matches!(b.try_take("https://a:443", t0 + Duration::from_millis(1900)), Take::Wait(_)));
        assert_eq!(b.try_take("https://a:443", t0 + Duration::from_secs(2)), Take::Go);
        assert!(matches!(b.try_take("https://a:443", t0 + Duration::from_secs(2)), Take::Wait(_)));
        // A long idle refills to the burst only.
        let later = t0 + Duration::from_secs(600);
        for _ in 0..4 {
            assert_eq!(b.try_take("https://a:443", later), Take::Go);
        }
        assert!(matches!(b.try_take("https://a:443", later), Take::Wait(_)));
    }

    #[test]
    fn parks_hold_cap_at_an_hour_and_lift() {
        let mut b = Budget::new();
        let t0 = Instant::now();
        b.park("https://api.github.com:443", 99_999, Reason::RateLimited, t0, 1000);
        match b.try_take("https://api.github.com:443", t0 + Duration::from_secs(10)) {
            Take::Parked(p) => {
                assert_eq!(p.until_at, 1000 + 3600);
                assert_eq!(p.reason, Reason::RateLimited);
            }
            other => panic!("{other:?}"),
        }
        b.park("https://api.github.com:443", 5, Reason::RateLimited, t0, 1000);
        assert_eq!(b.parked("https://api.github.com:443", t0).unwrap().until_at, 4600, "a shorter park doesn't shorten");
        assert_eq!(b.parks(t0).len(), 1);
        assert_eq!(b.try_take("https://api.github.com:443", t0 + Duration::from_secs(3601)), Take::Go);
        b.park("https://x:443", 60, Reason::RateLimited, t0, 0);
        b.unpark("https://x:443");
        assert_eq!(b.try_take("https://x:443", t0), Take::Go);
    }
}
