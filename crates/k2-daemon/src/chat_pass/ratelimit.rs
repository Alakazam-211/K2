//! Chat-pass mint rate limits (S28 §9.5): in-memory sliding windows,
//! bounded like `login_throttle` (4,096 keys, oldest evicted).
//!
//! | Bucket | Limit |
//! |---|---|
//! | per `sid` | 10 mints per 60 s |
//! | per daemon | 600 mints per 60 s |
//!
//! A normal BFF mints once per login and once per renew (under once an
//! hour per user); the limit only stops a loop. The owner handoff mint
//! has its own daemon-wide bucket (10 per 60 s, S32 §11.1).

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};

pub const WINDOW_SECS: i64 = 60;
pub const PER_SID: usize = 10;
pub const PER_DAEMON: usize = 600;
pub const HANDOFF_PER_DAEMON: usize = 10;
pub const MAX_KEYS: usize = 4096;

/// One set of sliding windows.
pub struct Limiter {
    per_key: usize,
    global: usize,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    keys: HashMap<String, VecDeque<i64>>,
    /// Insertion order of keys, for eviction.
    order: VecDeque<String>,
    all: VecDeque<i64>,
}

fn prune(q: &mut VecDeque<i64>, now: i64) {
    while q.front().is_some_and(|t| now - *t >= WINDOW_SECS) {
        q.pop_front();
    }
}

impl Limiter {
    pub fn new(per_key: usize, global: usize) -> Limiter {
        Limiter { per_key, global, inner: Mutex::new(Inner::default()) }
    }

    /// Record one mint for `key` at `now` unless a bucket is full. `Err`
    /// carries the milliseconds until a slot frees.
    pub fn check_and_record(&self, key: &str, now: i64) -> Result<(), i64> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let g = &mut *g;
        prune(&mut g.all, now);
        if g.all.len() >= self.global {
            let oldest = *g.all.front().expect("non-empty");
            return Err(((oldest + WINDOW_SECS - now).max(1)) * 1000);
        }
        let q = g.keys.entry(key.to_string()).or_default();
        prune(q, now);
        if q.len() >= self.per_key {
            let oldest = *q.front().expect("non-empty");
            return Err(((oldest + WINDOW_SECS - now).max(1)) * 1000);
        }
        let fresh = q.is_empty();
        q.push_back(now);
        g.all.push_back(now);
        if fresh {
            g.order.retain(|k| k != key);
            g.order.push_back(key.to_string());
            while g.keys.len() > MAX_KEYS {
                match g.order.pop_front() {
                    Some(old) => {
                        g.keys.remove(&old);
                    }
                    None => break,
                }
            }
        }
        Ok(())
    }

    pub fn tracked_keys(&self) -> usize {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).keys.len()
    }

    /// Forget everything (tests).
    pub fn reset(&self) {
        *self.inner.lock().unwrap_or_else(|p| p.into_inner()) = Inner::default();
    }
}

/// The daemon's pass-mint limiter.
pub static MINTS: LazyLock<Limiter> = LazyLock::new(|| Limiter::new(PER_SID, PER_DAEMON));
/// The daemon's owner-handoff mint limiter (one key).
pub static HANDOFFS: LazyLock<Limiter> = LazyLock::new(|| Limiter::new(HANDOFF_PER_DAEMON, HANDOFF_PER_DAEMON));
