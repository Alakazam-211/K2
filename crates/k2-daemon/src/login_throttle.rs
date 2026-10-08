//! Per-IP login throttle — 5 login POSTs per 300s (PRD
//! `prd-daemon-login-ip-throttle-v1` T1–T5, hardened by LM1/LM2 of
//! `prd-lan-mode-toggle-tls-docs-v1`).
//!
//! In-memory, one [`LoginLimiter`] per daemon, restart-resets. Username 3/15 lockout stays
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
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

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

/// LM4 — failed logins per [`WINDOW_SECS`], per door, across every
/// ANONYMOUS client: a login whose client address the daemon cannot vouch
/// for (unattested tunnel, a local proxy that is not a configured trusted
/// proxy, a LAN peer with no address). Those are the only paths where an
/// attacker can rotate per-IP buckets, so they are the only failures that
/// count and the only logins the ceiling refuses. Logins with a vouched
/// client address (edge-attested Connect, a real LAN peer, a configured
/// trusted proxy) and local logins are never counted and never refused by
/// it — they keep their per-IP bucket and the per-username lockout. The
/// dispatcher decides which is which.
pub const GLOBAL_FAIL_LIMIT: usize = 60;

/// Which login door a global-ceiling call is about. Connect and app
/// (skin) logins have separate ceilings so a spray on one cannot lock
/// out the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    Connect,
    Skin,
}

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
    /// LM4 — timestamps of recent anonymous failures per door (oldest
    /// first), capped at [`GLOBAL_FAIL_LIMIT`] entries.
    connect_fails: VecDeque<i64>,
    skin_fails: VecDeque<i64>,
}

impl Store {
    fn fails(&mut self, door: Door) -> &mut VecDeque<i64> {
        match door {
            Door::Connect => &mut self.connect_fails,
            Door::Skin => &mut self.skin_fails,
        }
    }
}

/// The login limiter of ONE daemon. The running daemon holds exactly one
/// (in `DaemonState`, shared by the main and tunnel-ingress listeners);
/// every in-process test daemon gets its own, so tests in one process can
/// never trip each other.
///
/// In-memory and restart-resets by design.
#[derive(Default)]
pub struct LoginLimiter {
    store: Mutex<Store>,
    /// Seconds added to the wall clock by [`LoginLimiter::now`]. Always 0
    /// in the running daemon; tests move it forward to pass a window
    /// without sleeping.
    clock_offset: AtomicI64,
}

impl LoginLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lock the store. A poisoned lock (a panic elsewhere while it was
    /// held) is recovered rather than treated as "limited forever": the
    /// state is plain counters, and failing closed here would 429 every
    /// login until a restart — the very outage LM1 removes.
    fn locked(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The limiter's clock (unix seconds). The dispatcher passes this to
    /// every call below.
    pub fn now(&self) -> i64 {
        k2_core::edge_attest::now_unix().saturating_add(self.clock_offset.load(Ordering::Relaxed))
    }

    /// Move this limiter's clock forward by `secs` (tests only: pass a
    /// window without sleeping).
    #[allow(dead_code)]
    pub fn advance_clock_for_tests(&self, secs: i64) {
        self.clock_offset.fetch_add(secs, Ordering::Relaxed);
    }

    /// Record this login POST for `key` at `now` (unix seconds) and return
    /// whether it is still under [`LIMIT`]. The 5th attempt is allowed;
    /// the 6th is [`Verdict::Limited`]. Keys starting with
    /// [`SKIN_KEY_PREFIX`] count in the skin table, everything else in the
    /// Connect table.
    pub fn check_and_record(&self, key: &str, now: i64) -> Verdict {
        self.check_and_record_with_limit(key, LIMIT, now)
    }

    /// [`LoginLimiter::check_and_record`] with an explicit per-window
    /// limit (the shared tunnel bucket uses [`TUNNEL_SHARED_LIMIT`]).
    pub fn check_and_record_with_limit(&self, key: &str, limit: usize, now: i64) -> Verdict {
        let key = if key.is_empty() { "-" } else { key };
        let mut g = self.locked();
        let table = if key.starts_with(SKIN_KEY_PREFIX) {
            &mut g.skin
        } else {
            &mut g.connect
        };
        table.check_and_record(key, limit, now)
    }

    /// LM4 — true when `door` has seen [`GLOBAL_FAIL_LIMIT`] anonymous
    /// failed logins inside the window ending at `now`; the caller refuses
    /// an ANONYMOUS attempt (429) before argon2. Does not record anything.
    pub fn global_ceiling_reached(&self, door: Door, now: i64) -> bool {
        let mut g = self.locked();
        let q = g.fails(door);
        prune_queue(q, now);
        q.len() >= GLOBAL_FAIL_LIMIT
    }

    /// LM4 — record one anonymous failed login on `door` at `now`. Memory
    /// stays bounded: only the newest [`GLOBAL_FAIL_LIMIT`] failures are
    /// kept, which is all the ceiling needs.
    pub fn record_global_failure(&self, door: Door, now: i64) {
        let mut g = self.locked();
        let q = g.fails(door);
        prune_queue(q, now);
        q.push_back(now);
        while q.len() > GLOBAL_FAIL_LIMIT {
            q.pop_front();
        }
    }

    /// `(connect keys, skin keys)` currently tracked. Tests use it to
    /// prove a path created no bucket and that the tables stay bounded.
    #[allow(dead_code)]
    pub fn tracked_keys(&self) -> (usize, usize) {
        let g = self.locked();
        (g.connect.hits.len(), g.skin.hits.len())
    }

    /// Drop all throttle state (tests only; a test that wants a clean
    /// limiter mid-test).
    #[allow(dead_code)]
    pub fn reset(&self) {
        let mut g = self.locked();
        g.connect.hits.clear();
        g.skin.hits.clear();
        g.connect_fails.clear();
        g.skin_fails.clear();
    }
}

/// Skin login map key: `"skin:" + ip`. Empty aliases `-` like Connect.
pub fn skin_ip_key(ip: &str) -> String {
    let ip = if ip.is_empty() { "-" } else { ip };
    format!("{SKIN_KEY_PREFIX}{ip}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test owns a fresh limiter: nothing is shared between tests.
    fn isolated(f: impl FnOnce(&LoginLimiter)) {
        f(&LoginLimiter::new());
    }

    /// Two limiters (two daemons in one process) never share state.
    #[test]
    fn limiters_are_independent() {
        let a = LoginLimiter::new();
        let b = LoginLimiter::new();
        let now = 1_700_000_000;
        for _ in 0..GLOBAL_FAIL_LIMIT {
            a.record_global_failure(Door::Connect, now);
        }
        for _ in 0..LIMIT {
            a.check_and_record("198.51.100.200", now);
        }
        assert!(a.global_ceiling_reached(Door::Connect, now));
        assert_eq!(a.check_and_record("198.51.100.200", now), Verdict::Limited);
        assert!(!b.global_ceiling_reached(Door::Connect, now), "b saw none of a's failures");
        assert_eq!(b.check_and_record("198.51.100.200", now), Verdict::Allow);
        assert_eq!(b.tracked_keys(), (1, 0));
    }

    /// The test clock moves only its own limiter, forward from wall time.
    #[test]
    fn advance_clock_moves_now_forward() {
        let l = LoginLimiter::new();
        let other = LoginLimiter::new();
        let before = l.now();
        l.advance_clock_for_tests(WINDOW_SECS);
        let after = l.now();
        assert!(after >= before + WINDOW_SECS, "{before} -> {after}");
        assert!(other.now() < after, "another limiter keeps wall time");
    }

    /// LM4 — the per-door ceiling trips at exactly GLOBAL_FAIL_LIMIT
    /// failures, ages out with the window, keeps the doors apart, and
    /// keeps at most GLOBAL_FAIL_LIMIT timestamps.
    #[test]
    fn global_failure_ceiling_per_door() {
        isolated(|l| {
            let now = 1_700_000_000;
            for i in 0..(GLOBAL_FAIL_LIMIT - 1) {
                l.record_global_failure(Door::Connect, now + (i as i64 % 10));
            }
            assert!(
                !l.global_ceiling_reached(Door::Connect, now + 10),
                "one below the ceiling still admits"
            );
            l.record_global_failure(Door::Connect, now + 10);
            assert!(l.global_ceiling_reached(Door::Connect, now + 10), "ceiling reached");
            assert!(
                !l.global_ceiling_reached(Door::Skin, now + 10),
                "a Connect spray must not close the app door"
            );
            for _ in 0..500 {
                l.record_global_failure(Door::Connect, now + 20);
            }
            assert_eq!(l.locked().connect_fails.len(), GLOBAL_FAIL_LIMIT, "bounded");
            assert!(l.global_ceiling_reached(Door::Connect, now + 20 + WINDOW_SECS - 1));
            assert!(
                !l.global_ceiling_reached(Door::Connect, now + 20 + WINDOW_SECS),
                "failures age out with the window"
            );
        });
    }

    #[test]
    fn fifth_allow_sixth_limited() {
        isolated(|l| {
            let now = 1_700_000_000;
            for i in 0..LIMIT {
                assert_eq!(
                    l.check_and_record("198.51.100.1", now + i as i64),
                    Verdict::Allow,
                    "attempt {} must pass",
                    i + 1
                );
            }
            assert_eq!(
                l.check_and_record("198.51.100.1", now + LIMIT as i64),
                Verdict::Limited,
                "6th must 429"
            );
        });
    }

    #[test]
    fn window_expiry_allows_again() {
        isolated(|l| {
            let now = 1_700_000_000;
            for _ in 0..LIMIT {
                assert_eq!(l.check_and_record("198.51.100.2", now), Verdict::Allow);
            }
            assert_eq!(l.check_and_record("198.51.100.2", now), Verdict::Limited);
            assert_eq!(
                l.check_and_record("198.51.100.2", now + WINDOW_SECS),
                Verdict::Allow,
                "after the window the oldest hits expire"
            );
        });
    }

    #[test]
    fn continued_hammering_keeps_the_key_limited_and_bounded() {
        isolated(|l| {
            let now = 1_700_000_000;
            for _ in 0..LIMIT {
                assert_eq!(l.check_and_record("198.51.100.9", now), Verdict::Allow);
            }
            for i in 0..1_000 {
                assert_eq!(
                    l.check_and_record("198.51.100.9", now + (i % 200)),
                    Verdict::Limited,
                    "hammer {i} must stay limited inside the window"
                );
            }
            let g = l.locked();
            let q = g.connect.hits.get("198.51.100.9").expect("key tracked");
            assert_eq!(q.len(), LIMIT + 1, "per-key history is capped at limit+1");
        });
    }

    #[test]
    fn dash_key_is_still_capped() {
        isolated(|l| {
            let now = 1_700_000_000;
            for _ in 0..LIMIT {
                assert_eq!(l.check_and_record("-", now), Verdict::Allow);
            }
            assert_eq!(l.check_and_record("-", now), Verdict::Limited);
            assert_eq!(
                l.check_and_record("", now),
                Verdict::Limited,
                "empty aliases -"
            );
        });
    }

    #[test]
    fn ips_are_independent() {
        isolated(|l| {
            let now = 1_700_000_000;
            for _ in 0..LIMIT {
                assert_eq!(l.check_and_record("198.51.100.3", now), Verdict::Allow);
            }
            assert_eq!(l.check_and_record("198.51.100.4", now), Verdict::Allow);
        });
    }

    #[test]
    fn skin_prefix_does_not_share_connect_bucket() {
        isolated(|l| {
            let now = 1_700_000_000;
            for _ in 0..LIMIT {
                assert_eq!(l.check_and_record("198.51.100.5", now), Verdict::Allow);
            }
            assert_eq!(l.check_and_record("198.51.100.5", now), Verdict::Limited);
            assert_eq!(
                l.check_and_record(&skin_ip_key("198.51.100.5"), now),
                Verdict::Allow,
                "skin guests must not share the Connect IP bucket"
            );
            for _ in 0..(LIMIT - 1) {
                assert_eq!(
                    l.check_and_record(&skin_ip_key("198.51.100.5"), now),
                    Verdict::Allow
                );
            }
            assert_eq!(
                l.check_and_record(&skin_ip_key("198.51.100.5"), now),
                Verdict::Limited,
                "skin 6th from the same IP must still 429 on its own bucket"
            );
        });
    }

    #[test]
    fn shared_tunnel_bucket_uses_its_own_limit() {
        isolated(|l| {
            let now = 1_700_000_000;
            for i in 0..TUNNEL_SHARED_LIMIT {
                assert_eq!(
                    l.check_and_record_with_limit(TUNNEL_SHARED_KEY, TUNNEL_SHARED_LIMIT, now),
                    Verdict::Allow,
                    "shared tunnel attempt {} must pass",
                    i + 1
                );
            }
            assert_eq!(
                l.check_and_record_with_limit(TUNNEL_SHARED_KEY, TUNNEL_SHARED_LIMIT, now),
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
        isolated(|l| {
            let now = 1_700_000_000;
            for i in 0..(CAPACITY + 1_000) {
                let ip = format!("10.{}.{}.{}", (i >> 16) & 0xff, (i >> 8) & 0xff, i & 0xff);
                assert_eq!(
                    l.check_and_record(&skin_ip_key(&ip), now),
                    Verdict::Allow,
                    "flood key {i} is a first attempt and must pass"
                );
                let (_, skin) = l.tracked_keys();
                assert!(skin <= CAPACITY, "skin table grew past capacity: {skin}");
            }
            assert_eq!(
                l.check_and_record(&skin_ip_key("203.0.113.77"), now),
                Verdict::Allow,
                "a fresh skin client after the flood must not be 429'd"
            );
            assert_eq!(
                l.check_and_record("203.0.113.78", now),
                Verdict::Allow,
                "a fresh Connect client after a skin flood must not be 429'd"
            );
            let (connect, skin) = l.tracked_keys();
            assert_eq!(skin, CAPACITY, "skin table is full but bounded");
            assert_eq!(connect, 1, "the skin flood left the Connect table alone");
        });
    }

    /// LM1(b) — a full Connect table does not touch the skin table.
    #[test]
    fn connect_flood_does_not_consume_skin_budget() {
        isolated(|l| {
            let now = 1_700_000_000;
            for i in 0..(CAPACITY + 10) {
                let ip = format!("10.9.{}.{}", (i >> 8) & 0xff, i & 0xff);
                l.check_and_record(&ip, now);
            }
            let (connect, skin) = l.tracked_keys();
            assert_eq!(connect, CAPACITY);
            assert_eq!(skin, 0);
            assert_eq!(l.check_and_record(&skin_ip_key("203.0.113.5"), now), Verdict::Allow);
        });
    }

    /// LM1(a) — expired entries are swept when a new key needs room, so a
    /// limited client that keeps hammering is not the one evicted while
    /// stale keys remain.
    #[test]
    fn expired_entries_are_swept_before_anything_live_is_evicted() {
        isolated(|l| {
            let t0 = 1_700_000_000;
            for i in 0..(CAPACITY - 1) {
                let ip = format!("10.8.{}.{}", (i >> 8) & 0xff, i & 0xff);
                l.check_and_record(&ip, t0);
            }
            let later = t0 + WINDOW_SECS;
            for _ in 0..LIMIT {
                assert_eq!(l.check_and_record("198.51.100.66", later), Verdict::Allow);
            }
            assert_eq!(l.tracked_keys().0, CAPACITY, "table is full");
            assert_eq!(
                l.check_and_record("203.0.113.9", later),
                Verdict::Allow,
                "new client admitted"
            );
            assert_eq!(
                l.tracked_keys().0,
                2,
                "every expired key was swept; only the live ones remain"
            );
            assert_eq!(
                l.check_and_record("198.51.100.66", later),
                Verdict::Limited,
                "the live limited key survived the sweep"
            );
        });
    }

    /// When nothing has expired, the least-recently-seen key is evicted.
    #[test]
    fn full_table_with_nothing_expired_evicts_least_recent() {
        isolated(|l| {
            let t0 = 1_700_000_000;
            l.check_and_record("192.0.2.1", t0);
            for i in 1..CAPACITY {
                let ip = format!("10.7.{}.{}", (i >> 8) & 0xff, i & 0xff);
                l.check_and_record(&ip, t0 + 1);
            }
            assert_eq!(l.tracked_keys().0, CAPACITY);
            assert_eq!(l.check_and_record("203.0.113.10", t0 + 2), Verdict::Allow);
            assert_eq!(l.tracked_keys().0, CAPACITY);
            assert!(
                !l.locked().connect.hits.contains_key("192.0.2.1"),
                "the oldest key is the one evicted"
            );
        });
    }
}
