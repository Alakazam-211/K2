//! Per-IP login throttle — 5 login POSTs per 300s (PRD
//! `prd-daemon-login-ip-throttle-v1` T1–T5, hardened by LM1/LM2 of
//! `prd-lan-mode-toggle-tls-docs-v1`).
//!
//! In-memory, process-local, restart-resets. Username 3/15 lockout stays
//! persisted in `connect_users` / skin `login_lockouts` and is NOT this
//! module's job — it applies on every path whether or not a per-IP bucket
//! exists. WHICH key an attempt counts against is decided by the
//! dispatcher (`login_throttle_key` / `skin_login_throttle_key`), which
//! only trusts a forwarded client IP from a trusted proxy and never from
//! the public tunnel.
//!
//! ## Bounded, never wholesale-deny (LM1)
//!
//! Connect and skin keys live in two SEPARATE tables with their own
//! capacity, so a flood on one door cannot starve the other. A table that
//! is full when a NEW key arrives first sweeps every expired entry; if it
//! is still full, it evicts the least-recently-seen key. A full table never
//! refuses an unrelated new client. Each key keeps at most `limit + 1`
//! timestamps (enough to decide "over the limit inside the window"), so a
//! hammered key cannot grow without bound either.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

/// T1 — max login POSTs per IP inside the window.
pub const LIMIT: usize = 5;
/// T1 — window length in seconds (`Retry-After` matches).
pub const WINDOW_SECS: i64 = 300;
/// LM2 — limit for the one shared bucket that unattested tunnel Connect
/// logins (`connectLoginIngress=any`) count against. The tunnel splice
/// carries no client address, so every such login would otherwise share
/// the 5/300s bucket of the splice peer `127.0.0.1`.
pub const TUNNEL_SHARED_LIMIT: usize = 50;
/// LM2 — the shared tunnel bucket key (never a valid IP, so it cannot
/// collide with a client key).
pub const TUNNEL_SHARED_KEY: &str = "tunnel:-";
/// T5 — per-table bound. Reaching it evicts; it never denies.
pub const CAPACITY: usize = 4096;
/// Skin door prefix — Connect and skin guesses must not share a bucket
/// (and live in separate tables).
pub const SKIN_KEY_PREFIX: &str = "skin:";

/// Whether this attempt may proceed to argon2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Limited,
}

/// One door's keys → timestamps (unix seconds, oldest first) of login
/// POSTs still inside the window.
#[derive(Default)]
struct Table {
    hits: HashMap<String, VecDeque<i64>>,
}

impl Table {
    fn check_and_record(&mut self, key: &str, limit: usize, now: i64) -> Verdict {
        self.prune_key(key, now);
        if !self.hits.contains_key(key) && self.hits.len() >= CAPACITY {
            self.sweep_expired(now);
            if self.hits.len() >= CAPACITY {
                self.evict_least_recent();
            }
        }
        let q = self.hits.entry(key.to_string()).or_default();
        q.push_back(now);
        // Only the newest `limit + 1` hits decide the verdict: the key is
        // over the limit iff the (limit+1)-th newest hit is in the window.
        while q.len() > limit.saturating_add(1) {
            q.pop_front();
        }
        if q.len() > limit {
            Verdict::Limited
        } else {
            Verdict::Allow
        }
    }

    fn prune_key(&mut self, key: &str, now: i64) {
        if let Some(q) = self.hits.get_mut(key) {
            prune_queue(q, now);
            if q.is_empty() {
                self.hits.remove(key);
            }
        }
    }

    /// Drop every hit older than the window, and every key left empty.
    fn sweep_expired(&mut self, now: i64) {
        self.hits.retain(|_, q| {
            prune_queue(q, now);
            !q.is_empty()
        });
    }

    /// Remove the key whose most recent hit is the oldest.
    fn evict_least_recent(&mut self) {
        let victim = self
            .hits
            .iter()
            .min_by_key(|(_, q)| q.back().copied().unwrap_or(i64::MIN))
            .map(|(k, _)| k.clone());
        if let Some(k) = victim {
            self.hits.remove(&k);
        }
    }
}

fn prune_queue(q: &mut VecDeque<i64>, now: i64) {
    while q
        .front()
        .is_some_and(|t| now.saturating_sub(*t) >= WINDOW_SECS)
    {
        q.pop_front();
    }
}

#[derive(Default)]
struct Store {
    connect: Table,
    skin: Table,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Store::default()))
}

/// Lock the store. A poisoned lock (a panic elsewhere while it was held)
/// is recovered rather than treated as "limited forever": the state is
/// plain counters, and failing closed here would 429 every login until a
/// restart — the very outage LM1 removes.
fn locked() -> std::sync::MutexGuard<'static, Store> {
    store().lock().unwrap_or_else(|p| p.into_inner())
}

/// Record this login POST for `key` at `now` (unix seconds) and return
/// whether it is still under [`LIMIT`]. The 5th attempt is allowed; the
/// 6th is [`Verdict::Limited`]. Keys starting with [`SKIN_KEY_PREFIX`]
/// count in the skin table, everything else in the Connect table.
pub fn check_and_record(key: &str, now: i64) -> Verdict {
    check_and_record_with_limit(key, LIMIT, now)
}

/// [`check_and_record`] with an explicit per-window limit (the shared
/// tunnel bucket uses [`TUNNEL_SHARED_LIMIT`]).
pub fn check_and_record_with_limit(key: &str, limit: usize, now: i64) -> Verdict {
    let key = if key.is_empty() { "-" } else { key };
    let mut g = locked();
    let table = if key.starts_with(SKIN_KEY_PREFIX) {
        &mut g.skin
    } else {
        &mut g.connect
    };
    table.check_and_record(key, limit, now)
}

/// Skin login map key: `"skin:" + ip`. Empty aliases `-` like Connect.
pub fn skin_ip_key(ip: &str) -> String {
    let ip = if ip.is_empty() { "-" } else { ip };
    format!("{SKIN_KEY_PREFIX}{ip}")
}

/// `(connect keys, skin keys)` currently tracked. Tests use it to prove a
/// path created no bucket and that the tables stay bounded.
#[allow(dead_code)]
pub fn tracked_keys() -> (usize, usize) {
    let g = locked();
    (g.connect.hits.len(), g.skin.hits.len())
}

/// Drop all throttle state. Integration tests share one process-wide map
/// and must not leak hits across cases.
#[allow(dead_code)]
pub fn reset() {
    let mut g = locked();
    g.connect.hits.clear();
    g.skin.hits.clear();
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
    fn continued_hammering_keeps_the_key_limited_and_bounded() {
        isolated(|| {
            let now = 1_700_000_000;
            for _ in 0..LIMIT {
                assert_eq!(check_and_record("198.51.100.9", now), Verdict::Allow);
            }
            for i in 0..1_000 {
                assert_eq!(
                    check_and_record("198.51.100.9", now + (i % 200)),
                    Verdict::Limited,
                    "hammer {i} must stay limited inside the window"
                );
            }
            let g = locked();
            let q = g.connect.hits.get("198.51.100.9").expect("key tracked");
            assert_eq!(q.len(), LIMIT + 1, "per-key history is capped at limit+1");
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

    #[test]
    fn shared_tunnel_bucket_uses_its_own_limit() {
        isolated(|| {
            let now = 1_700_000_000;
            for i in 0..TUNNEL_SHARED_LIMIT {
                assert_eq!(
                    check_and_record_with_limit(TUNNEL_SHARED_KEY, TUNNEL_SHARED_LIMIT, now),
                    Verdict::Allow,
                    "shared tunnel attempt {} must pass",
                    i + 1
                );
            }
            assert_eq!(
                check_and_record_with_limit(TUNNEL_SHARED_KEY, TUNNEL_SHARED_LIMIT, now),
                Verdict::Limited,
                "attempt {} on the shared tunnel bucket must 429",
                TUNNEL_SHARED_LIMIT + 1
            );
        });
    }

    /// LM1 — inserting far more distinct keys than the capacity never
    /// locks out a new legitimate client, and the table stays bounded.
    #[test]
    fn flood_past_capacity_never_denies_a_new_client() {
        isolated(|| {
            let now = 1_700_000_000;
            for i in 0..(CAPACITY + 1_000) {
                let ip = format!("10.{}.{}.{}", (i >> 16) & 0xff, (i >> 8) & 0xff, i & 0xff);
                assert_eq!(
                    check_and_record(&skin_ip_key(&ip), now),
                    Verdict::Allow,
                    "flood key {i} is a first attempt and must pass"
                );
                let (_, skin) = tracked_keys();
                assert!(skin <= CAPACITY, "skin table grew past capacity: {skin}");
            }
            assert_eq!(
                check_and_record(&skin_ip_key("203.0.113.77"), now),
                Verdict::Allow,
                "a fresh skin client after the flood must not be 429'd"
            );
            assert_eq!(
                check_and_record("203.0.113.78", now),
                Verdict::Allow,
                "a fresh Connect client after a skin flood must not be 429'd"
            );
            let (connect, skin) = tracked_keys();
            assert_eq!(skin, CAPACITY, "skin table is full but bounded");
            assert_eq!(connect, 1, "the skin flood left the Connect table alone");
        });
    }

    /// LM1(b) — a full Connect table does not touch the skin table.
    #[test]
    fn connect_flood_does_not_consume_skin_budget() {
        isolated(|| {
            let now = 1_700_000_000;
            for i in 0..(CAPACITY + 10) {
                let ip = format!("10.9.{}.{}", (i >> 8) & 0xff, i & 0xff);
                check_and_record(&ip, now);
            }
            let (connect, skin) = tracked_keys();
            assert_eq!(connect, CAPACITY);
            assert_eq!(skin, 0);
            assert_eq!(check_and_record(&skin_ip_key("203.0.113.5"), now), Verdict::Allow);
        });
    }

    /// LM1(a) — expired entries are swept when a new key needs room, so a
    /// limited client that keeps hammering is not the one evicted while
    /// stale keys remain.
    #[test]
    fn expired_entries_are_swept_before_anything_live_is_evicted() {
        isolated(|| {
            let t0 = 1_700_000_000;
            for i in 0..(CAPACITY - 1) {
                let ip = format!("10.8.{}.{}", (i >> 8) & 0xff, i & 0xff);
                check_and_record(&ip, t0);
            }
            let later = t0 + WINDOW_SECS;
            for _ in 0..LIMIT {
                assert_eq!(check_and_record("198.51.100.66", later), Verdict::Allow);
            }
            assert_eq!(tracked_keys().0, CAPACITY, "table is full");
            assert_eq!(
                check_and_record("203.0.113.9", later),
                Verdict::Allow,
                "new client admitted"
            );
            assert_eq!(
                tracked_keys().0,
                2,
                "every expired key was swept; only the live ones remain"
            );
            assert_eq!(
                check_and_record("198.51.100.66", later),
                Verdict::Limited,
                "the live limited key survived the sweep"
            );
        });
    }

    /// When nothing has expired, the least-recently-seen key is evicted.
    #[test]
    fn full_table_with_nothing_expired_evicts_least_recent() {
        isolated(|| {
            let t0 = 1_700_000_000;
            check_and_record("192.0.2.1", t0);
            for i in 1..CAPACITY {
                let ip = format!("10.7.{}.{}", (i >> 8) & 0xff, i & 0xff);
                check_and_record(&ip, t0 + 1);
            }
            assert_eq!(tracked_keys().0, CAPACITY);
            assert_eq!(check_and_record("203.0.113.10", t0 + 2), Verdict::Allow);
            assert_eq!(tracked_keys().0, CAPACITY);
            assert!(
                !locked().connect.hits.contains_key("192.0.2.1"),
                "the oldest key is the one evicted"
            );
        });
    }
}
