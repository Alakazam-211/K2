//! N1 (prd-app-caps-0464-v1 §6, AC29–AC32, 0.46.4): a per-caller token
//! bucket on `GET /cli/thread/activity` (a HEAD that reaches the
//! handler counts the same; the main port answers HEAD 405 first).
//!
//! The catch-up GET takes the global Thread-activity tracker lock twice
//! (`thread_activity::current_turn[_guest]`), and every turn's start,
//! delivery and end needs that lock. One caller polling in a loop could
//! starve them (the ear security review, N1). So each caller gets
//! [`RATE_PER_SEC`] calls a second sustained, bursts of [`BURST`], checked
//! after the caller is known and **before** any address resolution or
//! tracker work, so a refused call costs a hash-map lookup.
//!
//! Keys ([`key_for`]): an App guest login `guest:<principal>` (one person
//! across devices and Apps), any other `k2skn_` pass `pass:<id>`, a Connect
//! login `user:<name>`, an agent passport `agent:<workspace id>` (a cell
//! without a stamped principal `cell:<session>`). The owner token is never
//! limited. Refusal: 429 `{"error":"rate_limited","retryAfterMs":N,
//! "retryAfterSecs":⌈N/1000⌉}` (the daemon's `retryAfterMs` convention plus
//! the field the server SDK reads; the SDK doesn't retry on its own).
//!
//! In memory only (lost on restart): keys idle over [`IDLE`] are pruned on
//! access, at most [`MAX_KEYS`] keys (the least recently used go first).
//! Live state is the socket's job (`thread.stream` / `?ear=1`); this GET is
//! the catch-up after a reconnect (AC32).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::cli_response::CliResponse;

/// Sustained calls a second per caller (Q7).
pub const RATE_PER_SEC: f64 = 2.0;
/// Calls a caller may make at once before the rate applies (Q7).
pub const BURST: f64 = 10.0;
/// Keys untouched this long are dropped.
pub const IDLE: Duration = Duration::from_secs(60);
/// At most this many keys; past it the least recently used is dropped.
pub const MAX_KEYS: usize = 10_000;

/// Who is calling, for the bucket key. `None` = the owner (never limited).
pub fn key_for(params: &std::collections::HashMap<String, String>, pass: Option<&k2_core::skin::SkinPass>) -> Option<String> {
    if let Some(pass) = pass {
        let principal = pass.principal_id.as_deref().map(str::trim).filter(|p| !p.is_empty());
        return Some(match principal {
            Some(p) if pass.session => format!("guest:{p}"),
            _ => format!("pass:{}", pass.id),
        });
    }
    if let Some(p) = crate::caller_workspace::principal_from_params(params) {
        return Some(format!("agent:{}", p.workspace_uuid));
    }
    if let Some(cell) = params.get("cell_session_id").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return Some(format!("cell:{cell}"));
    }
    match crate::read_routes::request_login() {
        Some(login) if login != "owner" => Some(format!("user:{login}")),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// The buckets (pure: the clock is passed in).
#[derive(Debug, Default)]
pub struct Limiter {
    buckets: HashMap<String, Bucket>,
    last_prune: Option<Instant>,
}

impl Limiter {
    /// Take one call for `key` at `now`. `Err(ms)` = refused; try again
    /// after that many milliseconds (at least 1).
    pub fn take(&mut self, key: &str, now: Instant) -> Result<(), u64> {
        self.prune(now);
        if !self.buckets.contains_key(key) && self.buckets.len() >= MAX_KEYS {
            if let Some(oldest) = self.buckets.iter().min_by_key(|(_, b)| b.last).map(|(k, _)| k.clone()) {
                self.buckets.remove(&oldest);
            }
        }
        let b = self.buckets.entry(key.to_string()).or_insert(Bucket { tokens: BURST, last: now });
        let elapsed = now.saturating_duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * RATE_PER_SEC).min(BURST);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            return Ok(());
        }
        let wait_ms = ((1.0 - b.tokens) / RATE_PER_SEC * 1000.0).ceil() as u64;
        Err(wait_ms.max(1))
    }

    /// Drop keys idle over [`IDLE`] (at most once a second).
    fn prune(&mut self, now: Instant) {
        if self.last_prune.is_some_and(|t| now.saturating_duration_since(t) < Duration::from_secs(1)) {
            return;
        }
        self.last_prune = Some(now);
        self.buckets.retain(|_, b| now.saturating_duration_since(b.last) <= IDLE);
    }

    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }
}

fn limiter() -> &'static Mutex<Limiter> {
    static L: OnceLock<Mutex<Limiter>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(Limiter::default()))
}

/// Count one catch-up call for `key` (the process-wide limiter).
pub fn take(key: &str) -> Result<(), u64> {
    limiter().lock().unwrap_or_else(|p| p.into_inner()).take(key, Instant::now())
}

/// The 429 (AC30).
pub fn refused(retry_after_ms: u64) -> CliResponse {
    CliResponse {
        status: "429 Too Many Requests",
        content_type: "application/json",
        body: serde_json::json!({
            "error": "rate_limited",
            "retryAfterMs": retry_after_ms,
            "retryAfterSecs": retry_after_ms.div_ceil(1000),
            "message": "too many thread.activity catch-ups: use thread.stream (?ear=1) for live state and this GET only after a reconnect",
        })
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_of_ten_then_two_a_second() {
        let mut l = Limiter::default();
        let t0 = Instant::now();
        for i in 0..10 {
            assert_eq!(l.take("guest:p1", t0), Ok(()), "call {i} of the burst");
        }
        let wait = l.take("guest:p1", t0).expect_err("the 11th call in one instant is refused");
        assert_eq!(wait, 500, "one token refills in 500 ms at 2/s");
        // A second caller is unaffected.
        assert_eq!(l.take("guest:p2", t0), Ok(()));
        // After the wait, one more call; then refused again.
        let t1 = t0 + Duration::from_millis(500);
        assert_eq!(l.take("guest:p1", t1), Ok(()));
        assert!(l.take("guest:p1", t1).is_err());
        // Sustained: two a second.
        let t2 = t1 + Duration::from_secs(1);
        assert_eq!(l.take("guest:p1", t2), Ok(()));
        assert_eq!(l.take("guest:p1", t2), Ok(()));
        assert!(l.take("guest:p1", t2).is_err());
        // A long rest refills to the burst, never past it.
        let t3 = t2 + Duration::from_secs(30);
        for _ in 0..10 {
            assert_eq!(l.take("guest:p1", t3), Ok(()));
        }
        assert!(l.take("guest:p1", t3).is_err());
    }

    #[test]
    fn idle_keys_are_pruned_and_the_map_is_bounded() {
        let mut l = Limiter::default();
        let t0 = Instant::now();
        l.take("a", t0).unwrap();
        l.take("b", t0 + Duration::from_secs(30)).unwrap();
        assert_eq!(l.len(), 2);
        l.take("c", t0 + Duration::from_secs(62)).unwrap();
        assert_eq!(l.len(), 2, "a was idle over 60 s");
        assert!(!l.buckets.contains_key("a"));
        // Bounded: past MAX_KEYS the least recently used goes.
        let mut l = Limiter::default();
        for i in 0..MAX_KEYS {
            l.take(&format!("k{i}"), t0 + Duration::from_micros(i as u64)).unwrap();
        }
        assert_eq!(l.len(), MAX_KEYS);
        l.take("new", t0 + Duration::from_millis(500)).unwrap();
        assert_eq!(l.len(), MAX_KEYS);
        assert!(!l.buckets.contains_key("k0"), "the oldest went first");
        assert!(l.buckets.contains_key("new"));
    }

    #[test]
    fn the_refusal_carries_both_retry_fields() {
        let r = refused(1_200);
        assert_eq!(r.status, "429 Too Many Requests");
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!((v["error"].as_str(), v["retryAfterMs"].as_u64(), v["retryAfterSecs"].as_u64()), (Some("rate_limited"), Some(1200), Some(2)));
        let v: serde_json::Value = serde_json::from_str(&refused(1).body).unwrap();
        assert_eq!(v["retryAfterSecs"], 1);
    }
}
