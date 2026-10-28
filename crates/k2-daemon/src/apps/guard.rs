//! The minimal E3 guard on custom App names (0.46.6,
//! `prd-custom-domains-path-a-0466-v1` §6.2; vs-live PA-A3, A9, A10, A17).
//!
//! Two limits, keyed by the visitor address a connection carries (the
//! PROXY v2 header front443 writes for the App TLS door, or the tunnel's
//! once its relay half sends one). A connection with no visitor address
//! (a local caller) is never counted.
//!
//! - **G1, connections per visitor** (box-wide, default 64): checked at the
//!   App TLS door right after the PROXY header, before the TLS handshake
//!   (PA-A3), so it runs before K2 knows which App the connection is for.
//!   Over the cap the connection is closed and `perIpRefused` counts it.
//!   This is not front443's `HELLO_PER_IP` (PA-A10): that budget covers
//!   connections still sending their ClientHello and is released once the
//!   front routes them; G1 covers the door's routed, open connections.
//! - **G2, writes per visitor per App** (default 120 a minute): every
//!   `POST`/`PUT`/`PATCH`/`DELETE` on the App path's HTTP/1.1 rewrite
//!   (`client_ip::forward_guarded`), a bucket of N that refills N a minute.
//!   Over it K2 answers `429` with `Retry-After` itself, the App never sees
//!   the request, and the connection closes. GETs are never counted. A raw
//!   splice (`host.clientIp: false`) has no parsed requests, so G2 doesn't
//!   apply there (PA-A9); G1 still does.
//!
//! IPv4 is keyed by address, IPv6 by its /64 (PA-A17). Both tables are
//! in memory (reset on restart, like the login throttle) and bounded: G1
//! holds only open connections; G2 evicts idle buckets first, then the
//! least recently used, never a wholesale deny. The limits come from the
//! owner's `~/.k2/firewall.json` (`guard`), cached here and reloaded on
//! every change through `/cli/firewall/guard` ([`reload`]).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use arc_swap::ArcSwap;
use serde_json::{json, Value};

use k2_core::firewall::GuardConfig;

/// Most G2 buckets kept at once.
pub const MAX_WRITE_KEYS: usize = 50_000;

/// The visitor key: IPv4 as is (v4-mapped v6 folded to v4), IPv6 by /64.
pub fn visitor_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & (u128::MAX << 64))),
        },
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// One limiter (the daemon has one, [`global`]; tests make their own).
pub struct Limiter {
    conns: Mutex<HashMap<IpAddr, u32>>,
    writes: Mutex<HashMap<(String, IpAddr), Bucket>>,
    max_write_keys: usize,
    pub per_ip_refused: AtomicU64,
    pub guard_limited: AtomicU64,
    limited_by_app: Mutex<HashMap<String, u64>>,
}

/// One open connection's place in its visitor's G1 count (released on
/// drop). Inert when the visitor is unknown or G1 is off.
pub struct ConnSlot {
    owner: Option<(Arc<Limiter>, IpAddr)>,
}

impl Drop for ConnSlot {
    fn drop(&mut self) {
        if let Some((l, k)) = self.owner.take() {
            let mut m = l.conns.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(n) = m.get_mut(&k) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    m.remove(&k);
                }
            }
        }
    }
}

impl Limiter {
    pub fn new(max_write_keys: usize) -> Arc<Limiter> {
        Arc::new(Limiter {
            conns: Mutex::new(HashMap::new()),
            writes: Mutex::new(HashMap::new()),
            max_write_keys,
            per_ip_refused: AtomicU64::new(0),
            guard_limited: AtomicU64::new(0),
            limited_by_app: Mutex::new(HashMap::new()),
        })
    }

    /// G1: take a slot for `ip` under `limit` (`None` = off). `None` back =
    /// refused (counted).
    pub fn try_conn(self: &Arc<Self>, ip: Option<IpAddr>, limit: Option<u32>) -> Option<ConnSlot> {
        let (Some(ip), Some(limit)) = (ip, limit) else {
            return Some(ConnSlot { owner: None });
        };
        let k = visitor_key(ip);
        let mut m = self.conns.lock().unwrap_or_else(|p| p.into_inner());
        let n = m.entry(k).or_insert(0);
        if *n >= limit {
            drop(m);
            self.per_ip_refused.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        *n += 1;
        Some(ConnSlot { owner: Some((Arc::clone(self), k)) })
    }

    /// Open connections counted for `ip`'s key.
    pub fn open_for(&self, ip: IpAddr) -> u32 {
        self.conns.lock().unwrap_or_else(|p| p.into_inner()).get(&visitor_key(ip)).copied().unwrap_or(0)
    }

    /// G2: one write by `ip` to `app` at `now` under `limit` per minute
    /// (`None` = off). `Err(seconds)` = over it (counted); retry after.
    pub fn check_write(&self, app: &str, ip: IpAddr, limit: Option<u32>, now: Instant) -> Result<(), u64> {
        let Some(limit) = limit else { return Ok(()) };
        let cap = f64::from(limit.max(1));
        let rate = cap / 60.0;
        let key = (app.to_string(), visitor_key(ip));
        let mut m = self.writes.lock().unwrap_or_else(|p| p.into_inner());
        if !m.contains_key(&key) && m.len() >= self.max_write_keys {
            evict(&mut m, self.max_write_keys, cap, rate, now);
        }
        let b = m.entry(key).or_insert(Bucket { tokens: cap, last: now });
        let dt = now.saturating_duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + dt * rate).min(cap);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            return Ok(());
        }
        let wait = ((1.0 - b.tokens) / rate).ceil().max(1.0) as u64;
        drop(m);
        self.guard_limited.fetch_add(1, Ordering::Relaxed);
        *self.limited_by_app.lock().unwrap_or_else(|p| p.into_inner()).entry(app.to_string()).or_insert(0) += 1;
        Err(wait)
    }

    pub fn write_keys(&self) -> usize {
        self.writes.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    pub fn counters(&self) -> Value {
        let by_app = self.limited_by_app.lock().unwrap_or_else(|p| p.into_inner()).clone();
        json!({
            "perIpRefused": self.per_ip_refused.load(Ordering::Relaxed),
            "guardLimited": self.guard_limited.load(Ordering::Relaxed),
            "guardLimitedByApp": by_app,
            "visitorsWithOpenConnections": self.conns.lock().unwrap_or_else(|p| p.into_inner()).len(),
            "writeBuckets": self.write_keys(),
        })
    }
}

/// Make room: drop buckets that have refilled (idle visitors) first, then
/// the least recently used half. Never refuses a new visitor.
fn evict(m: &mut HashMap<(String, IpAddr), Bucket>, max: usize, cap: f64, rate: f64, now: Instant) {
    m.retain(|_, b| b.tokens + now.saturating_duration_since(b.last).as_secs_f64() * rate < cap);
    if m.len() >= max {
        let mut lasts: Vec<Instant> = m.values().map(|b| b.last).collect();
        lasts.sort();
        let cut = lasts[lasts.len() / 2];
        m.retain(|_, b| b.last > cut);
    }
}

/// The daemon's limiter.
pub fn global() -> &'static Arc<Limiter> {
    static L: OnceLock<Arc<Limiter>> = OnceLock::new();
    L.get_or_init(|| Limiter::new(MAX_WRITE_KEYS))
}

fn cfg_cell() -> &'static ArcSwap<GuardConfig> {
    static C: OnceLock<ArcSwap<GuardConfig>> = OnceLock::new();
    C.get_or_init(|| ArcSwap::from_pointee(k2_core::firewall::load().guard))
}

/// The guard settings in force.
pub fn config() -> Arc<GuardConfig> {
    cfg_cell().load_full()
}

/// Re-read `~/.k2/firewall.json` (after an owner change).
pub fn reload() {
    cfg_cell().store(Arc::new(k2_core::firewall::load().guard));
}

/// G1 at the App TLS door: a slot for this visitor, or `None` (refused).
pub fn door_conn(ip: Option<IpAddr>) -> Option<ConnSlot> {
    global().try_conn(ip, config().conns())
}

/// G2's gate for one App (`client_ip::forward_guarded`).
pub fn write_gate(app_id: &str) -> crate::client_ip::WriteGate {
    let app = app_id.to_string();
    Arc::new(move |ip: IpAddr| global().check_write(&app, ip, config().writes_for(&app), Instant::now()).err())
}

/// Counters + settings for `k2 firewall` and `k2 front status`.
pub fn counters_json() -> Value {
    let c = config();
    let mut v = global().counters();
    v["connsPerIp"] = json!(c.conns());
    v["writesPerMin"] = json!(c.enabled.then_some(c.writes_per_min));
    v["enabled"] = json!(c.enabled);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// Test 7 (PRD §11.1): the 65th connection from one address is
    /// refused and counted; another address still gets in; two IPv6
    /// addresses in one /64 share a bucket; closing frees a slot.
    #[test]
    fn g1_caps_open_connections_per_visitor() {
        let l = Limiter::new(100);
        let a = ip("203.0.113.7");
        let mut held: Vec<ConnSlot> = (0..64).map(|_| l.try_conn(Some(a), Some(64)).expect("under the cap")).collect();
        assert!(l.try_conn(Some(a), Some(64)).is_none(), "65th refused");
        assert_eq!(l.per_ip_refused.load(Ordering::Relaxed), 1);
        assert!(l.try_conn(Some(ip("198.51.100.1")), Some(64)).is_some(), "another visitor unaffected");
        held.pop();
        assert!(l.try_conn(Some(a), Some(64)).is_some(), "a closed connection frees its slot");
        drop(held);
        let v6a = ip("2001:db8:1:2::1");
        let v6b = ip("2001:db8:1:2:ffff::9");
        let _one = l.try_conn(Some(v6a), Some(1)).unwrap();
        assert!(l.try_conn(Some(v6b), Some(1)).is_none(), "one /64 is one visitor");
        assert!(l.try_conn(Some(ip("2001:db8:1:3::1")), Some(1)).is_some(), "another /64 is another");
        // Unknown visitor or guard off: never counted, never refused.
        for _ in 0..200 {
            assert!(l.try_conn(None, Some(1)).is_some());
            assert!(l.try_conn(Some(a), None).is_some());
        }
        // The mapped form of an IPv4 address is that address.
        assert_eq!(visitor_key(ip("::ffff:203.0.113.7")), a);
    }

    /// Test 8 (the limiter half): the 121st write in a minute is over,
    /// with a Retry-After; another visitor and another App are unaffected;
    /// off is no limit; the bucket refills with time.
    #[test]
    fn g2_limits_writes_per_visitor_per_app() {
        let l = Limiter::new(100);
        let a = ip("203.0.113.7");
        let t0 = Instant::now();
        for i in 0..120 {
            assert_eq!(l.check_write("shop", a, Some(120), t0), Ok(()), "write {i}");
        }
        let retry = l.check_write("shop", a, Some(120), t0).expect_err("121st over");
        assert!((1..=60).contains(&retry), "{retry}");
        assert_eq!(l.guard_limited.load(Ordering::Relaxed), 1);
        assert_eq!(l.check_write("shop", ip("198.51.100.1"), Some(120), t0), Ok(()));
        assert_eq!(l.check_write("blog", a, Some(120), t0), Ok(()));
        assert_eq!(l.check_write("shop", a, None, t0), Ok(()), "guard off");
        // 2 a second come back.
        assert_eq!(l.check_write("shop", a, Some(120), t0 + Duration::from_secs(1)), Ok(()));
        assert_eq!(l.counters()["guardLimitedByApp"]["shop"], 1);
    }

    #[test]
    fn g2_table_is_bounded_and_never_denies_a_new_visitor() {
        let l = Limiter::new(10);
        let t0 = Instant::now();
        for i in 0..50u8 {
            let v = IpAddr::from([203, 0, 113, i]);
            assert_eq!(l.check_write("shop", v, Some(5), t0 + Duration::from_millis(u64::from(i))), Ok(()));
            assert!(l.write_keys() <= 10, "{}", l.write_keys());
        }
    }
}
