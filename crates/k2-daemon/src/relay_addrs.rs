//! K2's relay addresses, resolved, in memory (0.46.8, Path B PB-6; vs-live
//! PB-A6), and what the tunnel has seen of visitor addresses.
//!
//! **A relay is never a visitor.** frpc's PROXY v2 header names the address
//! the relay saw. If a relay ever passed its own address (a relay-to-relay
//! forward that lost the header, a misconfigured relay), every visitor would
//! share one bucket and the per-visitor limits would lock them all out
//! together. So an address that belongs to one of K2's relays is "no
//! client IP" (`client_ip::trusted_source`), counted as `relayAsClient`.
//! The set is every relay this box knows (`relay_list::guard_hosts`: what
//! the tunnel dials, what `tunnel.json` stores, what k2.dev served and
//! marked active, `relay.k2.dev`, the old baked relays), each name resolved
//! (A and AAAA) on a background thread every [`REFRESH`]: the hot path
//! (every tunnel connection) only reads memory.
//!
//! **Visitor IPs: known or not yet** (`k2 tunnel status`, Settings): the
//! relay half of E2 (the patched relay that passes the visitor address)
//! may land after 0.46.8. Until a tunnel connection has carried a real
//! visitor address, status says "not passed by the relay yet", and every
//! per-visitor limit treats those connections as "visitor unknown".

use std::collections::HashSet;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use arc_swap::ArcSwap;
use serde_json::{json, Value};

/// How often the relay names are resolved again.
pub const REFRESH: Duration = Duration::from_secs(10 * 60);
/// A visitor address seen within this window is "known".
pub const KNOWN_WINDOW_SECS: i64 = 24 * 3600;

fn set() -> &'static ArcSwap<HashSet<IpAddr>> {
    static S: OnceLock<ArcSwap<HashSet<IpAddr>>> = OnceLock::new();
    S.get_or_init(|| ArcSwap::from_pointee(HashSet::new()))
}

static RELAY_AS_CLIENT: AtomicU64 = AtomicU64::new(0);
static WITH_VISITOR: AtomicU64 = AtomicU64::new(0);
static WITHOUT_VISITOR: AtomicU64 = AtomicU64::new(0);
static LAST_VISITOR_AT: AtomicI64 = AtomicI64::new(0);

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Is `ip` one of K2's relays (as last resolved)?
pub fn is_relay(ip: &IpAddr) -> bool {
    set().load().contains(&crate::front443::proxy_v2::canonical(*ip))
}

/// The resolved relay addresses (sorted, for status).
pub fn addrs() -> Vec<IpAddr> {
    let mut v: Vec<IpAddr> = set().load().iter().copied().collect();
    v.sort();
    v
}

/// A PROXY header named a relay's own address (counted; it is no client).
pub fn note_relay_as_client() {
    RELAY_AS_CLIENT.fetch_add(1, Ordering::Relaxed);
}

/// One tunnel connection, with or without a visitor address.
pub fn note_tunnel_visitor(ip: Option<IpAddr>) {
    if ip.is_some() {
        WITH_VISITOR.fetch_add(1, Ordering::Relaxed);
        LAST_VISITOR_AT.store(now(), Ordering::Relaxed);
    } else {
        WITHOUT_VISITOR.fetch_add(1, Ordering::Relaxed);
    }
}

/// `known` (a visitor address in the last [`KNOWN_WINDOW_SECS`]) or
/// `not_passed` (the relay doesn't pass it yet).
pub fn visitor_ips_state() -> &'static str {
    let at = LAST_VISITOR_AT.load(Ordering::Relaxed);
    if at > 0 && now() - at < KNOWN_WINDOW_SECS {
        "known"
    } else {
        "not_passed"
    }
}

/// The status block (`GET /cli/tunnel/status` → `visitorIps`).
pub fn status_json() -> Value {
    let state = visitor_ips_state();
    let at = LAST_VISITOR_AT.load(Ordering::Relaxed);
    json!({
        "state": state,
        "words": if state == "known" {
            "Visitor IPs: known (the relay passes each visitor's address; per-visitor limits apply)"
        } else {
            "Visitor IPs: not passed by the relay yet (per-visitor limits wait for it; box-wide and per-App limits apply)"
        },
        "lastSeenAt": (at > 0).then_some(at),
        "withVisitor": WITH_VISITOR.load(Ordering::Relaxed),
        "withoutVisitor": WITHOUT_VISITOR.load(Ordering::Relaxed),
        "relayAsClient": RELAY_AS_CLIENT.load(Ordering::Relaxed),
        "relayAddrs": addrs().iter().map(IpAddr::to_string).collect::<Vec<_>>(),
        "perVisitorCap": crate::tunnel_tls_listener::visitor_cap_json(),
    })
}

/// Resolve `hosts` (IP literals as they are). Names that don't resolve are
/// skipped (the previous set is replaced only by a non-empty answer).
pub fn resolve(hosts: &[String]) -> HashSet<IpAddr> {
    let mut out = HashSet::new();
    for h in hosts {
        let h = h.trim();
        if h.is_empty() {
            continue;
        }
        if let Ok(ip) = h.parse::<IpAddr>() {
            out.insert(crate::front443::proxy_v2::canonical(ip));
            continue;
        }
        if let Ok(it) = (h, 443u16).to_socket_addrs() {
            out.extend(it.map(|a| crate::front443::proxy_v2::canonical(a.ip())));
        }
    }
    out
}

/// Every relay host this box knows (without the vault token).
fn relay_hosts() -> Vec<String> {
    use k2_core::tunnel::{config, relay_list};
    let cfg = config::load_without_token().unwrap_or_default();
    relay_list::guard_hosts(&cfg).into_iter().map(|r| r.host).collect()
}

/// Resolve the relays now and swap the set in (kept when nothing resolved).
pub fn refresh() -> usize {
    let next = resolve(&relay_hosts());
    if next.is_empty() {
        return set().load().len();
    }
    let n = next.len();
    set().store(Arc::new(next));
    n
}

/// Boot: resolve now, then every [`REFRESH`], on one detached thread (never
/// under air-gap: no tunnel there).
pub fn spawn_refresh() {
    let _ = std::thread::Builder::new().name("k2-relay-addrs".into()).spawn(|| loop {
        if !k2_core::airgap::enabled() {
            refresh();
        }
        std::thread::sleep(REFRESH);
    });
}

/// Test seam: the relay set.
#[cfg(test)]
pub(crate) fn set_for_test(ips: &[IpAddr]) {
    set().store(Arc::new(ips.iter().copied().collect()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_resolve_as_themselves_and_mapped_v4_folds() {
        let got = resolve(&["203.0.113.5".into(), "::ffff:198.51.100.9".into(), " ".into()]);
        assert!(got.contains(&"203.0.113.5".parse::<IpAddr>().unwrap()));
        assert!(got.contains(&"198.51.100.9".parse::<IpAddr>().unwrap()), "{got:?}");
        assert_eq!(got.len(), 2);
    }
}
