//! Per-IP login throttle — 5 login POSTs per 300s (PRD
//! `prd-daemon-login-ip-throttle-v1` T1–T5).
//!
//! In-memory, process-local, restart-resets. Username 3/15 lockout stays
//! persisted in `connect_users` / skin `login_lockouts`. The dispatcher
//! skips this map on Connect `Ingress::Loopback`; LAN and tunnel (incl.
//! dash-attested) are capped. Skin logins use [`skin_ip_key`] so a
//! Connect spray cannot 429 skin guests.
//! Fail-closed: a poisoned lock or a full map that cannot record a new IP
//! is treated as over-limit (429), never as "let it through".

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

/// T1 — max login POSTs per IP inside the window.
pub const LIMIT: usize = 5;
/// T1 — window length in seconds (`Retry-After` matches).
pub const WINDOW_SECS: i64 = 300;
/// T5 — bound the map so a scanner cannot grow it without bound.
const CAPACITY: usize = 4096;
/// Skin door prefix — Connect and skin guesses must not share a bucket.
pub const SKIN_KEY_PREFIX: &str = "skin:";

/// Whether this attempt may proceed to argon2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Limited,
}

struct Store {
    /// IP → timestamps (unix seconds) of login POSTs in the live window.
    hits: HashMap<String, VecDeque<i64>>,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(Store {
            hits: HashMap::new(),
        })
    })
}

/// Record this login POST for `ip` at `now` (unix seconds) and return
/// whether it is still under the cap. The 5th attempt is allowed; the
/// 6th is [`Verdict::Limited`]. Fail-closed on lock poison or a full map
/// that cannot admit a new IP.
pub fn check_and_record(ip: &str, now: i64) -> Verdict {
    let key = if ip.is_empty() { "-" } else { ip };
    let Ok(mut g) = store().lock() else {
        return Verdict::Limited;
    };
    prune_ip(&mut g, key, now);
    if g.hits.len() >= CAPACITY && !g.hits.contains_key(key) {
        return Verdict::Limited;
    }
    g.hits.entry(key.to_string()).or_default().push_back(now);
    let count = g.hits.get(key).map(|q| q.len()).unwrap_or(0);
    if count > LIMIT {
        Verdict::Limited
    } else {
        Verdict::Allow
    }
}

/// Skin login map key: `"skin:" + ip`. Empty aliases `-` like Connect.
pub fn skin_ip_key(ip: &str) -> String {
    let ip = if ip.is_empty() { "-" } else { ip };
    format!("{SKIN_KEY_PREFIX}{ip}")
}

fn prune_ip(g: &mut Store, ip: &str, now: i64) {
    if let Some(q) = g.hits.get_mut(ip) {
        while q
            .front()
            .is_some_and(|t| now.saturating_sub(*t) >= WINDOW_SECS)
        {
            q.pop_front();
        }
        if q.is_empty() {
            g.hits.remove(ip);
        }
    }
}

/// Drop all throttle state. Integration tests share one process-wide map
/// and must not leak hits across cases.
#[allow(dead_code)]
pub fn reset() {
    if let Ok(mut g) = store().lock() {
        g.hits.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn isolated(f: impl FnOnce()) {
        static LOCK: Mutex<()> = Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        f();
        reset();
    }

    #[test]
    fn fifth_allow_sixth_limited() {
        isolated(|| {
            let now = 1_700_000_000;
            for i in 0..LIMIT {
                assert_eq!(
                    check_and_record("198.51.100.1", now + i as i64),
                    Verdict::Allow,
                    "attempt {} must pass",
                    i + 1
                );
            }
            assert_eq!(
                check_and_record("198.51.100.1", now + LIMIT as i64),
                Verdict::Limited,
                "6th must 429"
            );
        });
    }

    #[test]
    fn window_expiry_allows_again() {
        isolated(|| {
            let now = 1_700_000_000;
            for _ in 0..LIMIT {
                assert_eq!(check_and_record("198.51.100.2", now), Verdict::Allow);
            }
            assert_eq!(check_and_record("198.51.100.2", now), Verdict::Limited);
            assert_eq!(
                check_and_record("198.51.100.2", now + WINDOW_SECS),
                Verdict::Allow,
                "after the window the oldest hits expire"
            );
        });
    }

    #[test]
    fn dash_key_is_still_capped() {
        isolated(|| {
            let now = 1_700_000_000;
            for _ in 0..LIMIT {
                assert_eq!(check_and_record("-", now), Verdict::Allow);
            }
            assert_eq!(check_and_record("-", now), Verdict::Limited);
            assert_eq!(
                check_and_record("", now),
                Verdict::Limited,
                "empty aliases -"
            );
        });
    }

    #[test]
    fn ips_are_independent() {
        isolated(|| {
            let now = 1_700_000_000;
            for _ in 0..LIMIT {
                assert_eq!(check_and_record("198.51.100.3", now), Verdict::Allow);
            }
            assert_eq!(check_and_record("198.51.100.4", now), Verdict::Allow);
        });
    }

    #[test]
    fn skin_prefix_does_not_share_connect_bucket() {
        isolated(|| {
            let now = 1_700_000_000;
            for _ in 0..LIMIT {
                assert_eq!(check_and_record("198.51.100.5", now), Verdict::Allow);
            }
            assert_eq!(check_and_record("198.51.100.5", now), Verdict::Limited);
            assert_eq!(
                check_and_record(&skin_ip_key("198.51.100.5"), now),
                Verdict::Allow,
                "skin guests must not share the Connect IP bucket"
            );
            for _ in 0..(LIMIT - 1) {
                assert_eq!(
                    check_and_record(&skin_ip_key("198.51.100.5"), now),
                    Verdict::Allow
                );
            }
            assert_eq!(
                check_and_record(&skin_ip_key("198.51.100.5"), now),
                Verdict::Limited,
                "skin 6th from the same IP must still 429 on its own bucket"
            );
        });
    }
}
