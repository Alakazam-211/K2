//! The custom App name's life (0.46.6, `prd-app-custom-domains-v1` §6/§7,
//! path (a)): the TXT proof and DNS check, the state machine, the first
//! certificate, and the re-checks.
//!
//! ```text
//! waiting_dns ──check ok──▶ issuing ──cert──▶ live
//!      ▲  (TXT + DNS at this box)                │ daily re-check fails
//!      └──── check fails ────┐                   ▼
//!                           failing ◀── 7 days of failed re-checks
//! ```
//!
//! - **The proof** (§6): `_k2-domain.<host> TXT "k2-verify=<token>"`
//!   (per name, per box), AND every A/AAAA of the name is one of this
//!   box's public addresses. A name resolving to Cloudflare's proxy is
//!   called out ("DNS-only, grey cloud"). A CNAME to `<sub>.k2.dev` is
//!   path (b), not served in this release (B5 + relay).
//! - K2 plants the TXT itself when k2.dev hosts the zone (`dns_write`).
//! - **No order before the proof** (`acme::app_name_may_issue`). The first
//!   certificate is ordered when the check passes (DNS-01 on a K2-hosted
//!   zone, else TLS-ALPN-01 through the App TLS door); a stored usable
//!   certificate (an upload, or a cert still valid from before) goes live
//!   at once. Renewal is the renewer's (`renew.rs`, `issuing`/`live` only).
//! - **Re-checks**: `waiting_dns` every 10 minutes, `failing` hourly,
//!   `issuing`/`live` daily. A failed re-check of a served name keeps it
//!   serving and says so; after 7 days in a row it goes `failing`: its
//!   route is removed (front443 / door) and it is not renewed.
//! - **External** (`external`): served by the box's own web server, not by
//!   K2's front. Grandfathered at the upgrade (migration 0242, a name bound
//!   before 0.46.6) or opted in by name (`k2 app domain add … --external`,
//!   `k2 app domain external … on`). It is an origin and is never checked
//!   by the loop, ordered or routed; `k2 app domain external … off` hands
//!   it back to `waiting_dns`, and `k2 app domain check` moves it into this
//!   flow only when the proof passes (a failed check leaves it `external`).

use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value as J};

use k2_core::domains::{
    self, AppDomain, CheckRecord, PATH_DIRECT, STATE_EXTERNAL, STATE_FAILING, STATE_ISSUING, STATE_LIVE, STATE_VERIFYING,
    STATE_WAITING_DNS,
};
use k2_core::log_debug;

/// A served name keeps serving this long after its re-checks start failing.
pub const FAILING_AFTER_SECS: i64 = 7 * 24 * 3600;
/// Path (a) has no relay to protect: the box caps at 100 custom names.
pub const MAX_NAMES: usize = 100;
const WAITING_EVERY: i64 = 10 * 60;
const FAILING_EVERY: i64 = 3600;
const SERVED_EVERY: i64 = 24 * 3600;
const TICK: Duration = Duration::from_secs(5 * 60);

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ── DNS ──────────────────────────────────────────────────────────────

/// The DNS reads a check makes (a trait so tests never touch the network).
pub trait Lookup {
    /// TXT values at `name` (chunks joined).
    fn txt(&self, name: &str) -> Result<Vec<String>, String>;
    /// A + AAAA of `host` (CNAMEs followed).
    fn addrs(&self, host: &str) -> Result<Vec<IpAddr>, String>;
    /// The CNAME target of `host` itself, if it is one.
    fn cname(&self, host: &str) -> Option<String>;
}

/// Public DNS (Google, then Cloudflare), uncached.
pub struct LiveLookup;

impl Lookup for LiveLookup {
    fn txt(&self, name: &str) -> Result<Vec<String>, String> {
        use crate::mail::dns_verify::{DnsError, DnsResolver, SystemResolver};
        let r = SystemResolver::google_nocache().or_else(|_| SystemResolver::cloudflare_nocache())?;
        match r.txt(name) {
            Ok(v) => Ok(v.into_iter().map(|c| c.concat()).collect()),
            Err(DnsError::NotFound) => Ok(Vec::new()),
            Err(e) => Err(format!("{e:?}")),
        }
    }
    fn addrs(&self, host: &str) -> Result<Vec<IpAddr>, String> {
        use crate::mail::dns_verify::{DnsError, DnsResolver, SystemResolver};
        let r = SystemResolver::google_nocache().or_else(|_| SystemResolver::cloudflare_nocache())?;
        let mut out: Vec<IpAddr> = Vec::new();
        match r.a(host) {
            Ok(v) => out.extend(v.into_iter().map(IpAddr::V4)),
            Err(DnsError::NotFound) => {}
            Err(e) => return Err(format!("{e:?}")),
        }
        if let Ok(v) = r.aaaa(host) {
            out.extend(v.into_iter().map(IpAddr::V6));
        }
        Ok(out)
    }
    fn cname(&self, host: &str) -> Option<String> {
        use hickory_resolver::config::{ResolverConfig, ResolverOpts};
        use hickory_resolver::proto::rr::RecordType;
        let mut opts = ResolverOpts::default();
        opts.cache_size = 0;
        let resolver = hickory_resolver::Resolver::new(ResolverConfig::google(), opts).ok()?;
        let q = format!("{}.", host.trim_end_matches('.'));
        let l = resolver.lookup(q, RecordType::CNAME).ok()?;
        let target = l.iter().next()?.to_string();
        Some(target.trim_end_matches('.').to_ascii_lowercase())
    }
}

#[cfg(test)]
thread_local! {
    static TEST_BOX_IPS: std::cell::RefCell<Option<Vec<IpAddr>>> = const { std::cell::RefCell::new(None) };
}

/// Test seam: this box's public addresses.
#[cfg(test)]
pub(crate) fn set_test_box_ips(ips: Option<Vec<IpAddr>>) {
    TEST_BOX_IPS.with(|c| *c.borrow_mut() = ips);
}

/// This box's public addresses: the egress IPv4 (checked at most hourly)
/// plus every public interface address.
pub fn box_ips() -> Vec<IpAddr> {
    #[cfg(test)]
    if let Some(v) = TEST_BOX_IPS.with(|c| c.borrow().clone()) {
        return v;
    }
    let egress = egress_ipv4();
    let local = crate::mail::cert_names::box_addrs_local(egress.as_deref());
    let mut out: Vec<IpAddr> = local.v4.into_iter().map(IpAddr::V4).collect();
    out.extend(local.v6.into_iter().map(IpAddr::V6));
    out
}

/// The IPv4 this box reaches the internet from (cached for an hour;
/// `None` under air-gap or when the lookup failed).
pub fn egress_ipv4() -> Option<String> {
    static CACHE: OnceLock<Mutex<Option<(i64, Option<String>)>>> = OnceLock::new();
    let c = CACHE.get_or_init(|| Mutex::new(None));
    let egress = {
        let mut g = c.lock().unwrap_or_else(|p| p.into_inner());
        match g.as_ref() {
            Some((at, ip)) if now() - at < 3600 => ip.clone(),
            _ => {
                let ip = if k2_core::airgap::enabled() {
                    None
                } else {
                    use crate::mail::preflight::PreflightEnv;
                    crate::mail::preflight::RealPreflightEnv.public_ip()
                };
                *g = Some((now(), ip.clone()));
                ip
            }
        }
    };
    egress
}

/// Pure (PA-A11): is the box behind a router? The egress IPv4 not on any
/// interface = NAT (`true`); on one = `false`; unknown egress = `None`.
pub fn nat_verdict(egress: Option<&str>, local_v4: &[std::net::Ipv4Addr]) -> Option<bool> {
    let e: std::net::Ipv4Addr = egress?.trim().parse().ok()?;
    Some(!local_v4.contains(&e))
}

/// This box: behind a home or office router? (`None` = can't tell:
/// air-gap, or the egress lookup failed; then no Path B wording.)
pub fn behind_nat() -> Option<bool> {
    // Tests never touch the network: unset is "can't tell".
    #[cfg(test)]
    return TEST_NAT.with(|c| *c.borrow()).unwrap_or(None);
    #[cfg(not(test))]
    {
        let (v4, _) = crate::mail::cert_names::local_interface_addrs();
        nat_verdict(egress_ipv4().as_deref(), &v4)
    }
}

#[cfg(test)]
thread_local! {
    static TEST_NAT: std::cell::RefCell<Option<Option<bool>>> = const { std::cell::RefCell::new(None) };
}

/// Test seam: the NAT verdict.
#[cfg(test)]
pub(crate) fn set_test_nat(v: Option<Option<bool>>) {
    TEST_NAT.with(|c| *c.borrow_mut() = v);
}

/// The Path B pointer for a box behind a router (PRD §7).
pub const NAT_WORDING: &str = "This server is on a home or office connection. Pointing your domain here sends visitors \
to your network directly and shows its address. Use K2 Connect instead (Pro, coming in 0.46.7): your domain points at \
K2's relays and your address stays hidden.";
/// The Path B pointer for a Mac or Windows computer (PRD §7).
pub const DESKTOP_WORDING: &str = "Your own domain on this computer comes with K2 Connect in 0.46.7.";

/// Pure: which path this box offers and what K2 says (PRD §7). `holder`:
/// the :443 holder's JSON when the front is off (`front443::port443_holder`).
pub fn advice_for(linux: bool, nat: Option<bool>, front_enabled: bool, holder: Option<&J>) -> J {
    if !linux {
        return json!({"path": "b", "pathA": false, "records": "none", "message": DESKTOP_WORDING});
    }
    if nat == Some(true) {
        return json!({
            "path": "b",
            "pathA": true,
            "behindNat": true,
            "records": "advanced",
            "message": NAT_WORDING,
            "advanced": "Advanced: I've forwarded port 443 to this server: use the records below.",
        });
    }
    if !front_enabled {
        if let Some(h) = holder.filter(|h| h["holder"] == "foreign") {
            return json!({
                "path": "a",
                "pathA": true,
                "behindNat": nat,
                "records": "shown",
                "port443": h,
                "message": h["message"],
            });
        }
        return json!({
            "path": "a",
            "pathA": true,
            "behindNat": nat,
            "records": "shown",
            "message": "Turn on the :443 front first: the owner runs k2 front enable (an agent's request is held for the owner).",
        });
    }
    json!({"path": "a", "pathA": true, "behindNat": nat, "records": "shown", "message": J::Null})
}

/// [`advice_for`] for this box, cached 30 s (the :443 check may run
/// `systemctl`).
pub fn setup_advice() -> J {
    // Tests: Linux wording, no :443 probe, no cache.
    #[cfg(test)]
    return advice_for(true, behind_nat(), k2_core::front443::load().enabled, None);
    #[cfg(not(test))]
    {
        static CACHE: OnceLock<Mutex<Option<(i64, J)>>> = OnceLock::new();
        let c = CACHE.get_or_init(|| Mutex::new(None));
        let mut g = c.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, v)) = g.as_ref() {
            if now() - at < 30 {
                return v.clone();
            }
        }
        let front_enabled = k2_core::front443::load().enabled;
        let holder = (cfg!(target_os = "linux") && !front_enabled).then(crate::front443::port443_holder);
        let v = advice_for(cfg!(target_os = "linux"), behind_nat(), front_enabled, holder.as_ref());
        *g = Some((now(), v.clone()));
        v
    }
}

/// Cloudflare's proxy ranges (a proxied, "orange cloud" record hides the
/// real name from this box and breaks TLS-ALPN-01).
pub fn is_cloudflare_proxy(ip: &IpAddr) -> bool {
    const V4: &[(&str, u8)] = &[
        ("173.245.48.0", 20),
        ("103.21.244.0", 22),
        ("103.22.200.0", 22),
        ("103.31.4.0", 22),
        ("141.101.64.0", 18),
        ("108.162.192.0", 18),
        ("190.93.240.0", 20),
        ("188.114.96.0", 20),
        ("197.234.240.0", 22),
        ("198.41.128.0", 17),
        ("162.158.0.0", 15),
        ("104.16.0.0", 13),
        ("104.24.0.0", 14),
        ("172.64.0.0", 13),
        ("131.0.72.0", 22),
    ];
    const V6: &[(&str, u8)] = &[
        ("2400:cb00::", 32),
        ("2606:4700::", 32),
        ("2803:f800::", 32),
        ("2405:b500::", 32),
        ("2405:8100::", 32),
        ("2a06:98c0::", 29),
        ("2c0f:f248::", 32),
    ];
    match ip {
        IpAddr::V4(a) => V4.iter().any(|(net, bits)| {
            let n: std::net::Ipv4Addr = net.parse().unwrap();
            let mask = u32::MAX << (32 - u32::from(*bits));
            (u32::from(*a) & mask) == (u32::from(n) & mask)
        }),
        IpAddr::V6(a) => V6.iter().any(|(net, bits)| {
            let n: std::net::Ipv6Addr = net.parse().unwrap();
            let mask = u128::MAX << (128 - u32::from(*bits));
            (u128::from(*a) & mask) == (u128::from(n) & mask)
        }),
    }
}

/// What one check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub txt_ok: bool,
    /// Every A/AAAA is this box's (and there is at least one).
    pub here: bool,
    pub ok: bool,
    /// What DNS showed (`direct` when it points here).
    pub path: Option<&'static str>,
    /// Plain words for what is missing (`None` when `ok`).
    pub reason: Option<String>,
    pub seen: Vec<String>,
}

/// Check `d` against DNS (pure given `lk` and the box's addresses).
pub fn evaluate(d: &AppDomain, lk: &dyn Lookup, box_ips: &[IpAddr], sub: &str) -> Outcome {
    let mut problems: Vec<String> = Vec::new();
    let (txt_name, txt_value) = d.txt();
    let txt_ok = match lk.txt(&txt_name) {
        Ok(v) => v.iter().any(|s| s.trim().trim_matches('"') == txt_value),
        Err(e) => {
            problems.push(format!("the TXT lookup failed ({e})"));
            false
        }
    };
    if !txt_ok && problems.is_empty() {
        problems.push(format!("TXT {txt_name} \"{txt_value}\" is not visible yet"));
    }
    let connect = (!sub.is_empty()).then(|| format!("{sub}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST));
    if let (Some(c), Some(k)) = (lk.cname(&d.hostname), connect.as_ref()) {
        if c == *k {
            problems.push(format!(
                "{} is a CNAME to {k} (through K2 Connect, path b): not served in this release; point an A record at this box",
                d.hostname
            ));
            return Outcome {
                txt_ok,
                here: false,
                ok: false,
                path: Some(domains::PATH_CONNECT),
                reason: Some(problems.join("; ")),
                seen: vec![c],
            };
        }
    }
    let mut here = false;
    let mut seen: Vec<String> = Vec::new();
    match lk.addrs(&d.hostname) {
        Err(e) => problems.push(format!("the A/AAAA lookup failed ({e})")),
        Ok(addrs) if addrs.is_empty() => problems.push(format!("{} has no A/AAAA record yet", d.hostname)),
        Ok(addrs) => {
            seen = addrs.iter().map(IpAddr::to_string).collect();
            if box_ips.is_empty() {
                problems.push("K2 can't tell this box's public address right now".to_string());
            } else {
                let elsewhere: Vec<String> = addrs.iter().filter(|a| !box_ips.contains(a)).map(IpAddr::to_string).collect();
                if addrs.iter().any(is_cloudflare_proxy) {
                    problems.push(format!(
                        "{} is proxied by Cloudflare (orange cloud): set the record DNS-only (grey cloud)",
                        d.hostname
                    ));
                } else if !elsewhere.is_empty() {
                    problems.push(format!(
                        "DNS now points at {}, not this box ({})",
                        elsewhere.join(", "),
                        box_ips.iter().map(IpAddr::to_string).collect::<Vec<_>>().join(", ")
                    ));
                } else {
                    here = true;
                }
            }
        }
    }
    let ok = txt_ok && here;
    Outcome {
        txt_ok,
        here,
        ok,
        path: here.then_some(PATH_DIRECT),
        reason: (!ok).then(|| problems.join("; ")),
        seen,
    }
}

// ── state machine ────────────────────────────────────────────────────

/// What to do after a check moved a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Nothing,
    /// Proven and no usable certificate: order one.
    Issue,
    /// Serving changed (live, failing): rebuild front443 and the door.
    Reroute,
}

/// A usable certificate for `host` in the store: covers it, not expired.
pub fn cert_usable(host: &str) -> bool {
    crate::domains::status::from_pem_file(host)
        .is_some_and(|p| p.state == "issued" && p.names.iter().any(|n| n.eq_ignore_ascii_case(host)))
}

/// Apply `o` to `d` at `at` (the whole transition table above).
pub fn apply(d: &AppDomain, o: &Outcome, at: i64) -> Next {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let host = d.hostname.as_str();
    if o.ok {
        let _ = domains::record_app_check(
            &conn,
            host,
            &CheckRecord { checked_at: at, verified_at: Some(at), path: o.path.map(str::to_string), failing_since: None },
        );
        return match d.state.as_str() {
            STATE_LIVE | STATE_ISSUING => {
                let _ = domains::set_app_state(&conn, host, &d.state, None);
                Next::Nothing
            }
            _ => {
                if cert_usable(host) {
                    let _ = domains::set_app_state(&conn, host, STATE_LIVE, None);
                    Next::Reroute
                } else {
                    let _ = domains::set_app_state(&conn, host, STATE_ISSUING, None);
                    Next::Issue
                }
            }
        };
    }
    let reason = o.reason.clone().unwrap_or_else(|| "the check failed".into());
    match d.state.as_str() {
        // The owner's explicit check of a grandfathered name: it stays
        // served by its own proxy (and an origin) until the proof passes.
        STATE_EXTERNAL => {
            let _ = domains::record_app_check(
                &conn,
                host,
                &CheckRecord { checked_at: at, verified_at: None, path: o.path.map(str::to_string), failing_since: None },
            );
            let _ = domains::set_app_state(
                &conn,
                host,
                STATE_EXTERNAL,
                Some(&format!("still served by your own web server; K2 can serve it once: {reason}")),
            );
            Next::Nothing
        }
        STATE_LIVE | STATE_ISSUING => {
            let since = d.failing_since.unwrap_or(at);
            let _ = domains::record_app_check(
                &conn,
                host,
                &CheckRecord { checked_at: at, verified_at: None, path: o.path.map(str::to_string), failing_since: Some(since) },
            );
            if at - since >= FAILING_AFTER_SECS {
                let _ = domains::set_app_state(&conn, host, STATE_FAILING, Some(&format!("for 7 days: {reason}")));
                Next::Reroute
            } else {
                let _ = domains::set_app_state(
                    &conn,
                    host,
                    &d.state,
                    Some(&format!("re-check failing since {since} (still served; it stops after 7 days): {reason}")),
                );
                Next::Nothing
            }
        }
        STATE_FAILING => {
            let _ = domains::record_app_check(
                &conn,
                host,
                &CheckRecord { checked_at: at, verified_at: None, path: o.path.map(str::to_string), failing_since: d.failing_since.or(Some(at)) },
            );
            let _ = domains::set_app_state(&conn, host, STATE_FAILING, Some(&reason));
            Next::Nothing
        }
        _ => {
            let _ = domains::record_app_check(
                &conn,
                host,
                &CheckRecord { checked_at: at, verified_at: None, path: o.path.map(str::to_string), failing_since: None },
            );
            let _ = domains::set_app_state(&conn, host, STATE_WAITING_DNS, Some(&reason));
            Next::Nothing
        }
    }
}

fn get(host: &str) -> Option<AppDomain> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    domains::app_domain(&conn, host).ok().flatten()
}

/// Check `host` now with `lk` and act on the result. Returns the name's
/// row after the check, and what the check saw.
pub fn check_with(host: &str, lk: &dyn Lookup, issue: &dyn Fn(&str)) -> Option<(AppDomain, Outcome)> {
    let d = get(host)?;
    if matches!(d.state.as_str(), STATE_WAITING_DNS | STATE_FAILING) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = domains::set_app_state(&conn, host, STATE_VERIFYING, d.last_error.as_deref());
    }
    let o = evaluate(&d, lk, &box_ips(), &crate::apps::origins::box_subdomain());
    match apply(&d, &o, now()) {
        Next::Issue => issue(host),
        Next::Reroute => crate::front443::rebuild_now(),
        Next::Nothing => {}
    }
    Some((get(host)?, o))
}

/// [`check_with`] against public DNS, issuing on a background thread.
/// Tests answer from [`set_test_lookup`] and never touch the network (no
/// lookup installed: no check).
pub fn check_now(host: &str) -> Option<(AppDomain, Outcome)> {
    if k2_core::airgap::enabled() {
        return None;
    }
    #[cfg(test)]
    {
        return TEST_LOOKUP.with(|l| l.borrow().as_ref().and_then(|lk| check_with(host, lk.as_ref(), &|h| issue_in_background(h))));
    }
    #[cfg(not(test))]
    check_with(host, &LiveLookup, &|h| issue_in_background(h))
}

#[cfg(test)]
thread_local! {
    static TEST_LOOKUP: std::cell::RefCell<Option<Box<dyn Lookup>>> = const { std::cell::RefCell::new(None) };
    static TEST_ISSUES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Test seam: the DNS [`check_now`] reads on this thread (`None` clears).
#[cfg(test)]
pub(crate) fn set_test_lookup(lk: Option<Box<dyn Lookup>>) {
    TEST_LOOKUP.with(|l| *l.borrow_mut() = lk);
}

/// Test seam: the names [`issue_in_background`] was asked to issue on this
/// thread (taken).
#[cfg(test)]
pub(crate) fn take_test_issues() -> Vec<String> {
    TEST_ISSUES.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

#[cfg(not(test))]
fn in_flight() -> &'static Mutex<std::collections::HashSet<String>> {
    static F: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// Order `host`'s certificate now (blocking): route it to the door
/// (`issuing`) first, so a TLS-ALPN-01 hello reaches it; `live` on
/// success, else it stays `issuing` with the error in plain words (the
/// next tick retries after the renewer's backoff).
pub fn issue_blocking(host: &str) -> Result<(), String> {
    crate::front443::rebuild_now();
    match crate::domains::acme::issue_attached(host) {
        Ok(_) => {
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                if domains::app_domain(&conn, host).ok().flatten().is_some_and(|d| d.state == STATE_ISSUING) {
                    let _ = domains::set_app_state(&conn, host, STATE_LIVE, None);
                }
            }
            crate::front443::rebuild_now();
            log_debug!("[apps/custom-domain] {host}: certificate issued, live");
            Ok(())
        }
        Err(e) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = domains::set_app_state(&conn, host, STATE_ISSUING, Some(&format!("certificate not issued yet: {e}")));
            log_debug!("[apps/custom-domain] {host}: issue failed: {e}");
            Err(e)
        }
    }
}

/// [`issue_blocking`] on its own thread, once per name at a time.
pub fn issue_in_background(host: &str) {
    // Tests never order from another thread (no scoped DB there).
    #[cfg(test)]
    TEST_ISSUES.with(|c| c.borrow_mut().push(host.to_string()));
    #[cfg(not(test))]
    spawn_issue(host);
}

#[cfg(not(test))]
fn spawn_issue(host: &str) {
    let h = host.to_string();
    if !in_flight().lock().unwrap_or_else(|p| p.into_inner()).insert(h.clone()) {
        return;
    }
    let _ = std::thread::Builder::new().name("k2-custom-domain-issue".into()).spawn(move || {
        let _ = issue_blocking(&h);
        in_flight().lock().unwrap_or_else(|p| p.into_inner()).remove(&h);
    });
}

/// Is a check due for `d` at `at`?
pub fn due(d: &AppDomain, at: i64) -> bool {
    if d.state == STATE_EXTERNAL {
        // Grandfathered: only the owner's explicit check moves it.
        return false;
    }
    let every = match d.state.as_str() {
        STATE_LIVE | STATE_ISSUING => SERVED_EVERY,
        STATE_FAILING => FAILING_EVERY,
        _ => WAITING_EVERY,
    };
    d.checked_at.is_none_or(|c| at - c >= every)
}

/// One pass: re-check what is due; retry the first certificate of an
/// `issuing` name once the renewer's backoff for it has passed.
pub fn tick(lk: &dyn Lookup, issue: &dyn Fn(&str)) -> usize {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        domains::app_domains(&conn).unwrap_or_default()
    };
    let at = now();
    let mut checked = 0;
    for d in rows.into_iter().filter(|d| d.apex_attached).take(MAX_NAMES) {
        if due(&d, at) {
            checked += 1;
            let _ = check_with(&d.hostname, lk, issue);
            continue;
        }
        if d.state == STATE_ISSUING && !cert_usable(&d.hostname) {
            let backoff = crate::domains::renew::load().names.get(&d.hostname).and_then(|r| r.next_attempt);
            if backoff.is_none_or(|t| t <= at) {
                issue(&d.hostname);
            }
        }
    }
    checked
}

/// Boot: the re-check loop on one detached thread (first pass a minute
/// in, then every 5 minutes; nothing under air-gap).
pub fn spawn_loop() {
    let _ = std::thread::Builder::new().name("k2-custom-domains".into()).spawn(|| {
        std::thread::sleep(Duration::from_secs(60));
        loop {
            if !k2_core::airgap::enabled() && k2_core::db::try_shared().is_some() {
                tick(&LiveLookup, &|h| issue_in_background(h));
            }
            std::thread::sleep(TICK);
        }
    });
}

// ── K2 plants the TXT on a K2-hosted zone (§6) ───────────────────────

/// The TXT record's name relative to its apex (`_k2-domain.app`).
pub fn txt_relative_name(hostname: &str, apex: &str) -> String {
    let full = domains::verify_txt_name(hostname);
    full.strip_suffix(&format!(".{apex}")).map(str::to_string).unwrap_or(full)
}

/// Plant `_k2-domain.<host> TXT "k2-verify=<token>"` in the k2.dev zone
/// (the apex is attached with `dns_write`). Earlier copies are removed.
pub fn plant_txt(d: &AppDomain, binding: &k2_core::domains::DomainBinding) -> Result<(), String> {
    let zone = binding.zone_id.as_deref().filter(|z| !z.is_empty()).ok_or("the zone has no k2.dev id")?;
    let _ = remove_txt(d, binding);
    let body = json!({
        "type": "TXT",
        "name": txt_relative_name(&d.hostname, &binding.apex),
        "content": domains::verify_txt_value(&d.verify_token),
        "ttl": 300,
    })
    .to_string();
    let r = crate::dns::proxy::proxy_request("POST", &format!("/api/dns/zones/{zone}/records"), Some("k2-custom-domain"), Some(&body))?;
    if r.status != 200 && r.status != 201 {
        return Err(format!("k2.dev refused the TXT record (HTTP {}): {}", r.status, r.body));
    }
    Ok(())
}

/// Remove this name's `_k2-domain` TXT records from the k2.dev zone.
pub fn remove_txt(d: &AppDomain, binding: &k2_core::domains::DomainBinding) -> Result<usize, String> {
    let zone = binding.zone_id.as_deref().filter(|z| !z.is_empty()).ok_or("the zone has no k2.dev id")?;
    let rel = txt_relative_name(&d.hostname, &binding.apex).to_ascii_lowercase();
    let full = domains::verify_txt_name(&d.hostname);
    let r = crate::dns::proxy::proxy_request("GET", &format!("/api/dns/zones/{zone}"), Some("k2-custom-domain"), None)?;
    let v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
    let recs = v.get("records").or_else(|| v.pointer("/zone/records")).and_then(J::as_array).cloned().unwrap_or_default();
    let mut n = 0;
    for rec in recs {
        let ty = rec.get("type").and_then(J::as_str).unwrap_or("");
        let name = rec.get("name").and_then(J::as_str).unwrap_or("").trim_end_matches('.').to_ascii_lowercase();
        if ty.eq_ignore_ascii_case("TXT") && (name == rel || name == full) {
            if let Some(id) = rec.get("id").and_then(J::as_str) {
                let _ = crate::dns::proxy::proxy_request("DELETE", &format!("/api/dns/records/{id}"), Some("k2-custom-domain"), None);
                n += 1;
            }
        }
    }
    Ok(n)
}

/// The DNS records the person sets for path (a): the A/AAAA at this box
/// and the TXT proof (DNS-only).
pub fn records_to_set(d: &AppDomain) -> Vec<J> {
    // A Mac or Windows computer gets no Path A records (front443 is
    // Linux-only; PRD §7).
    if !cfg!(target_os = "linux") && !cfg!(test) {
        return Vec::new();
    }
    let (txt_name, txt_value) = d.txt();
    let mut out: Vec<J> = Vec::new();
    for ip in box_ips() {
        let ty = if ip.is_ipv4() { "A" } else { "AAAA" };
        out.push(json!({ "type": ty, "name": d.hostname, "value": ip.to_string(), "note": "DNS-only (no proxy / grey cloud)" }));
    }
    out.push(json!({ "type": "TXT", "name": txt_name, "value": txt_value }));
    out
}

#[cfg(test)]
#[path = "custom_domain_flow_tests.rs"]
mod tests;
