//! Retry policy, full-jitter backoff and idempotency keys (S25 §8.1, §8.2).

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::{Rng, RngCore};
use sew_proto::error::Code;
use sew_proto::http::IdempotencyKey;

use crate::error::{ApiFailure, TransportError};

/// When and how often a call is retried.
///
/// Defaults: 4 attempts (1 try + 3 retries), 60 s total from the first send, 10 s longest
/// `Retry-After` honored, backoff base 250 ms, cap 5 s.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RetryPolicy {
    /// Attempts including the first.
    pub max_attempts: u32,
    /// Time budget measured from the first send.
    pub max_elapsed: Duration,
    /// A longer server wait is returned as an error at once.
    pub max_retry_after: Duration,
    /// Backoff base.
    pub base: Duration,
    /// Backoff cap.
    pub cap: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 4,
            max_elapsed: Duration::from_secs(60),
            max_retry_after: Duration::from_secs(10),
            base: Duration::from_millis(250),
            cap: Duration::from_secs(5),
        }
    }
}

impl RetryPolicy {
    /// One attempt, no retries.
    pub fn none() -> Self {
        RetryPolicy {
            max_attempts: 1,
            ..RetryPolicy::default()
        }
    }

    /// Sets `max_attempts` (at least 1).
    pub fn with_max_attempts(mut self, n: u32) -> Self {
        self.max_attempts = n.max(1);
        self
    }

    /// Sets `max_elapsed`.
    pub fn with_max_elapsed(mut self, d: Duration) -> Self {
        self.max_elapsed = d;
        self
    }

    /// Sets `max_retry_after`.
    pub fn with_max_retry_after(mut self, d: Duration) -> Self {
        self.max_retry_after = d;
        self
    }

    /// Sets the backoff base and cap.
    pub fn with_backoff(mut self, base: Duration, cap: Duration) -> Self {
        self.base = base;
        self.cap = cap;
        self
    }

    /// The backoff this policy uses.
    pub fn backoff(&self) -> Backoff {
        Backoff {
            base: self.base,
            cap: self.cap,
        }
    }
}

/// Full-jitter exponential backoff: retry `n` (0-based) waits `uniform[0, min(cap, base × 2^n))`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    /// Base delay.
    pub base: Duration,
    /// Largest delay.
    pub cap: Duration,
}

impl Backoff {
    /// The gateway's backoff (C7): base 1 s, cap 60 s.
    pub const GATEWAY: Backoff = Backoff {
        base: Duration::from_secs(1),
        cap: Duration::from_secs(60),
    };

    /// The HTTP default: base 250 ms, cap 5 s.
    pub const HTTP: Backoff = Backoff {
        base: Duration::from_millis(250),
        cap: Duration::from_secs(5),
    };

    /// The exclusive upper bound for retry `n`: `min(cap, base × 2^n)`.
    pub fn ceiling(&self, n: u32) -> Duration {
        let factor = 1u128 << n.min(64);
        let ns = self.base.as_nanos().saturating_mul(factor);
        let ns = ns.min(self.cap.as_nanos());
        Duration::from_nanos(u64::try_from(ns).unwrap_or(u64::MAX))
    }

    /// The delay for retry `n` given a uniform sample `unit` in `[0, 1)`.
    pub fn delay(&self, n: u32, unit: f64) -> Duration {
        let unit = if unit.is_finite() {
            unit.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let d = self.ceiling(n).mul_f64(unit);
        // `unit` < 1 keeps the delay below the ceiling; guard float rounding at the edge.
        let ceiling = self.ceiling(n);
        if d >= ceiling && !ceiling.is_zero() {
            ceiling - Duration::from_nanos(1)
        } else {
            d
        }
    }

    /// A random delay for retry `n`.
    pub fn sample<R: Rng + ?Sized>(&self, n: u32, rng: &mut R) -> Duration {
        self.delay(n, rng.r#gen::<f64>())
    }
}

/// A fresh idempotency key: `sc_` + 16 random bytes, base64url without padding (25 chars).
pub fn new_idempotency_key() -> IdempotencyKey {
    idempotency_key_from(&mut rand::thread_rng())
}

/// [`new_idempotency_key`] from a given RNG.
pub fn idempotency_key_from<R: RngCore + ?Sized>(rng: &mut R) -> IdempotencyKey {
    let mut bytes = [0u8; 16];
    rng.fill_bytes(&mut bytes);
    let s = format!("sc_{}", URL_SAFE_NO_PAD.encode(bytes));
    match s.parse() {
        Ok(k) => k,
        // `sc_` + 22 base64url chars is always 25 chars of [A-Za-z0-9_-].
        Err(e) => unreachable_key(e),
    }
}

#[cold]
fn unreachable_key(e: sew_proto::http::IdempotencyKeyError) -> IdempotencyKey {
    panic!("sew-client: generated an invalid idempotency key: {e}")
}

/// The 429 wait: `details.retry_after_ms`, else `Retry-After` seconds, else 1 s (S25 §8.3).
pub fn rate_limit_wait(f: &ApiFailure) -> Duration {
    f.retry_after.unwrap_or(Duration::from_secs(1))
}

/// What happened on one attempt.
#[derive(Debug)]
pub(crate) enum Outcome<'a> {
    Transport(&'a TransportError),
    Api(&'a ApiFailure),
    Other,
}

/// What to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    Stop,
    /// Wait exactly this long (server-given), then retry.
    After(Duration),
    /// Wait a backoff sample, then retry.
    Backoff,
}

/// The retry table (S25 §8.1). `safe` = a GET or a POST carrying an idempotency key;
/// `auth_route` = `/api/v1/auth/*` (retried only when the request never left).
pub(crate) fn decide(
    policy: &RetryPolicy,
    safe: bool,
    auth_route: bool,
    outcome: Outcome<'_>,
) -> Decision {
    match outcome {
        Outcome::Transport(t) if !t.sent => Decision::Backoff,
        _ if auth_route => Decision::Stop,
        Outcome::Transport(_) if safe => Decision::Backoff,
        Outcome::Transport(_) => Decision::Stop,
        Outcome::Api(f) => match f.code {
            Code::RateLimited => {
                let wait = rate_limit_wait(f);
                if wait <= policy.max_retry_after {
                    Decision::After(
                        wait + Duration::from_millis(rand::thread_rng().gen_range(0..=100)),
                    )
                } else {
                    Decision::Stop
                }
            }
            Code::Unavailable if safe => match f.retry_after {
                Some(w) if w <= policy.max_retry_after => Decision::After(w),
                Some(_) => Decision::Stop,
                None => Decision::Backoff,
            },
            Code::IdempotencyConflict if f.reason() == Some("in_progress") => {
                Decision::After(f.retry_after.unwrap_or(Duration::from_secs(1)))
            }
            _ => Decision::Stop,
        },
        Outcome::Other => Decision::Stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceiling_caps() {
        let b = Backoff::GATEWAY;
        assert_eq!(b.ceiling(0), Duration::from_secs(1));
        assert_eq!(b.ceiling(3), Duration::from_secs(8));
        assert_eq!(b.ceiling(6), Duration::from_secs(60));
        assert_eq!(b.ceiling(200), Duration::from_secs(60));
        assert!(b.delay(0, 0.999_999_999_9) < Duration::from_secs(1));
        assert_eq!(b.delay(2, 0.5), Duration::from_secs(2));
    }
}
