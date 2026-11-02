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
//!   box's public addresses (path a, `direct`). A name resolving to
//!   Cloudflare's proxy is called out ("DNS-only, grey cloud").
//! - **Path B** (0.46.8, `connect`; prd-custom-domains-path-b-0467-v1
//!   PB-3/PB-5): the name is a CNAME (a chain) to one of this box's k2.dev
//!   names (the App's own `<app>.<box>.k2.dev`), or an ALIAS / flattened
//!   apex whose addresses are all K2's relays. Then the box also needs K2
//!   Connect Pro, a free place in the 20-name cap, and the control plane's
//!   own verification (`relay_claims`): claim + verify → `issuing` (the
//!   tunnel's custom proxy carries the name; TLS-ALPN-01 through the
//!   relay) → `live`. For such a name the control plane decides routing:
//!   the box's daily DNS check only informs the owner, and the control
//!   plane's `released` moves it to `failing` (vs-live PB-A15). Which path
//!   a name takes is decided by DNS alone (D4); the certificate is the same.
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

/// The Path B pointer for a box behind a router (PRD §7), when Path B is
/// open to it.
pub const NAT_WORDING: &str = "This server is on a home or office connection. Pointing your domain here would send \
visitors to your network directly and show its address, so K2 uses K2 Connect: your domain points at K2's relays \
(set the records below) and your address stays hidden.";
/// What a Mac or Windows computer offers (PRD §7, vs-live PB-A16): Path B
/// only (front443 is Linux-only).
pub const DESKTOP_WORDING: &str = "On this computer your own domain goes through K2 Connect: it points at K2's \
relays (set the records below). Nothing listens on this computer's port 443.";

/// Path B's facts for [`advice_for`] (PB-4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathB {
    /// Behind K2 Connect with E2E (`Err`: what's missing).
    pub ready: Result<(), String>,
    /// On Pro (`Err`: can't tell right now).
    pub pro: Result<bool, String>,
    /// Names holding a place in the box's cap.
    pub used: usize,
}

impl PathB {
    /// Path B is open to this box now.
    pub fn open(&self) -> bool {
        self.ready.is_ok() && self.pro == Ok(true)
    }
    /// Why not, in plain words (`None` when open).
    pub fn why_not(&self) -> Option<String> {
        if let Err(w) = &self.ready {
            return Some(w.clone());
        }
        match &self.pro {
            Ok(true) => None,
            Ok(false) => Some(super::relay_claims::not_pro_words()),
            Err(w) => Some(w.clone()),
        }
    }
    fn json(&self) -> J {
        let cap = k2_core::tunnel::custom_names::MAX_PER_BOX;
        json!({
            "available": self.open(),
            "connect": self.ready.is_ok(),
            "pro": self.pro.as_ref().ok(),
            "why": self.why_not(),
            "used": self.used,
            "cap": cap,
            "meter": format!("{} of {cap} names through K2's relays", self.used),
            "upgrade": (self.pro == Ok(false)).then_some(super::relay_claims::UPGRADE_URL),
            "proxy": super::relay_claims::proxy_json(),
        })
    }
}

/// Pure: which path this box offers and what K2 says (PRD §7; Path B
/// PB-4, PB-A16). `holder`: the :443 holder's JSON when the front is off
/// (`front443::port443_holder`). `path` is the one K2 recommends (`a` or
/// `b`); `records` says how the records are shown (`shown`, `advanced`,
/// `none`); `pathB` carries Path B's availability and the 20-name meter.
pub fn advice_for(linux: bool, nat: Option<bool>, front_enabled: bool, holder: Option<&J>, b: &PathB) -> J {
    let pb = b.json();
    let open = b.open();
    if !linux {
        if open {
            return json!({"path": "b", "pathA": false, "records": "shown", "message": DESKTOP_WORDING, "pathB": pb});
        }
        let why = b.why_not().unwrap_or_default();
        return json!({
            "path": "b",
            "pathA": false,
            "records": "none",
            "message": format!("Your own domain on this computer goes through K2 Connect (Path B). {why}"),
            "pathB": pb,
        });
    }
    if nat == Some(true) {
        if open {
            return json!({"path": "b", "pathA": true, "behindNat": true, "records": "shown", "message": NAT_WORDING, "pathB": pb});
        }
        let why = b.why_not().unwrap_or_default();
        return json!({
            "path": "b",
            "pathA": true,
            "behindNat": true,
            "records": "advanced",
            "message": format!(
                "This server is on a home or office connection. Pointing your domain here sends visitors to your network \
                 directly and shows its address. K2 Connect keeps it hidden (Path B): {why}"
            ),
            "advanced": "Advanced: I've forwarded port 443 to this server: use the records below.",
            "pathB": pb,
        });
    }
    if !front_enabled {
        let foreign = holder.filter(|h| h["holder"] == "foreign");
        if open {
            let lead = match foreign {
                Some(_) => "Another web server holds port 443 on this server.",
                None => "K2's :443 front is off on this server.",
            };
            let mut v = json!({
                "path": "b",
                "pathA": true,
                "behindNat": nat,
                "records": "shown",
                "message": format!(
                    "{lead} Through K2 Connect (Pro) your domain points at K2's relays instead: no sudo, nothing on port \
                     443 (set the records below). Path A (the owner runs k2 front enable) also works."
                ),
                "pathB": pb,
            });
            if let Some(h) = foreign {
                v["port443"] = h.clone();
            }
            return v;
        }
        if let Some(h) = foreign {
            return json!({
                "path": "a",
                "pathA": true,
                "behindNat": nat,
                "records": "shown",
                "port443": h,
                "message": h["message"],
                "pathB": pb,
            });
        }
        return json!({
            "path": "a",
            "pathA": true,
            "behindNat": nat,
            "records": "shown",
            "message": "Turn on the :443 front first: the owner runs k2 front enable (an agent's request is held for the owner).",
            "pathB": pb,
        });
    }
    json!({"path": "a", "pathA": true, "behindNat": nat, "records": "shown", "message": J::Null, "pathB": pb})
}

/// Path B's facts for this box now (the plan read is cached, PB-A5).
pub fn path_b_now() -> PathB {
    let used = k2_core::db::try_shared()
        .map(|db| {
            let conn = db.lock();
            domains::relay_claims(&conn).unwrap_or(0)
        })
        .unwrap_or(0);
    let ready = super::relay_claims::ready();
    let pro = if ready.is_ok() { super::relay_claims::pro() } else { Ok(false) };
    PathB { ready, pro, used }
}

/// [`advice_for`] for this box, cached 30 s (the :443 check may run
/// `systemctl`; the plan read may reach the control plane).
pub fn setup_advice() -> J {
    // Tests: Linux wording, no :443 probe, no cache.
    #[cfg(test)]
    return advice_for(true, behind_nat(), k2_core::front443::load().enabled, None, &path_b_now());
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
        let v = advice_for(cfg!(target_os = "linux"), behind_nat(), front_enabled, holder.as_ref(), &path_b_now());
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

/// This box's k2.dev name (`<sub>.k2.dev`), or `None` off K2 Connect.
fn box_k2_name(sub: &str) -> Option<String> {
    let sub = sub.trim().to_ascii_lowercase();
    (!sub.is_empty()).then(|| format!("{sub}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST))
}

/// `name` is this box's k2.dev name or a name under it.
fn is_box_k2_name(name: &str, box_name: &str) -> bool {
    let n = name.trim().trim_end_matches('.').to_ascii_lowercase();
    n == box_name || n.ends_with(&format!(".{box_name}"))
}

/// The k2.dev name a Path B record points at: the App's own
/// `<app>.<box>.k2.dev` (K2 Web guidance, 11-01). `None` off K2 Connect.
pub fn path_b_target(app: &str, sub: &str) -> Option<String> {
    box_k2_name(sub).map(|b| format!("{}.{b}", app.trim().to_ascii_lowercase()))
}

/// Most CNAME hops followed.
const MAX_CNAME_HOPS: usize = 8;

/// Check `d` against DNS (pure given `lk` and the box's addresses), with
/// K2's relay addresses as last resolved (`relay_addrs`).
pub fn evaluate(d: &AppDomain, lk: &dyn Lookup, box_ips: &[IpAddr], sub: &str) -> Outcome {
    evaluate_with(d, lk, box_ips, sub, &crate::relay_addrs::addrs())
}

/// [`evaluate`] with the relay addresses given (tests).
///
/// Path B (`connect`, PB-5): a CNAME chain (at most 8 hops) that reaches
/// this box's k2.dev name or a name under it; or, with no CNAME (an ALIAS,
/// ANAME or flattened apex), A/AAAA records that are all K2's relays or
/// this box's k2.dev name's own addresses. A chain ending at another k2.dev
/// name, or answers mixing the relays with anything else, is named.
pub fn evaluate_with(d: &AppDomain, lk: &dyn Lookup, box_ips: &[IpAddr], sub: &str, relay_ips: &[IpAddr]) -> Outcome {
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
    let box_name = box_k2_name(sub);
    let target = path_b_target(&d.app, sub);
    // Path B by CNAME: follow the chain.
    let mut chain: Vec<String> = Vec::new();
    let mut cur = d.hostname.clone();
    while chain.len() < MAX_CNAME_HOPS {
        let Some(c) = lk.cname(&cur) else { break };
        let c = c.trim().trim_end_matches('.').to_ascii_lowercase();
        chain.push(c.clone());
        if box_name.as_deref().is_some_and(|b| is_box_k2_name(&c, b)) {
            let ok = txt_ok;
            return Outcome {
                txt_ok,
                here: true,
                ok,
                path: Some(domains::PATH_CONNECT),
                reason: (!ok).then(|| problems.join("; ")),
                seen: chain,
            };
        }
        if c == k2_core::tunnel::config::SUBDOMAIN_HOST || c.ends_with(&format!(".{}", k2_core::tunnel::config::SUBDOMAIN_HOST)) {
            problems.push(format!(
                "{} points at {c}, which isn't this server's K2 name: point it at {}",
                d.hostname,
                target.as_deref().unwrap_or("this server's <app>.<box>.k2.dev")
            ));
            return Outcome { txt_ok, here: false, ok: false, path: None, reason: Some(problems.join("; ")), seen: chain };
        }
        cur = c;
    }
    let mut here = false;
    let mut seen: Vec<String> = Vec::new();
    // Path B by ALIAS / flattening: every address is a relay (or this box's
    // own k2.dev name's address, which is the relay it is on).
    let relays: Vec<IpAddr> = {
        let mut v: Vec<IpAddr> = relay_ips.to_vec();
        if let Some(b) = box_name.as_deref() {
            if let Ok(a) = lk.addrs(b) {
                v.extend(a);
            }
        }
        v
    };
    let answer = lk.addrs(&d.hostname);
    match &answer {
        Ok(addrs)
            if !addrs.is_empty()
                && box_name.is_some()
                && addrs.iter().all(|a| relays.contains(a))
                && !addrs.iter().all(|a| box_ips.contains(a)) =>
        {
            let ok = txt_ok;
            return Outcome {
                txt_ok,
                here: true,
                ok,
                path: Some(domains::PATH_CONNECT),
                reason: (!ok).then(|| problems.join("; ")),
                seen: addrs.iter().map(IpAddr::to_string).collect(),
            };
        }
        Ok(addrs)
            if box_name.is_some()
                && addrs.iter().any(|a| relays.contains(a) && !box_ips.contains(a))
                && addrs.iter().any(|a| !relays.contains(a)) =>
        {
            let other: Vec<String> = addrs.iter().filter(|a| !relays.contains(a)).map(IpAddr::to_string).collect();
            problems.push(format!(
                "part of your DNS points elsewhere ({}): every record for {} must point at {}",
                other.join(", "),
                d.hostname,
                target.as_deref().unwrap_or("K2")
            ));
            return Outcome {
                txt_ok,
                here: false,
                ok: false,
                path: None,
                reason: Some(problems.join("; ")),
                seen: addrs.iter().map(IpAddr::to_string).collect(),
            };
        }
        _ => {}
    }
    match answer {
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
                    // A Path B name is judged against its k2.dev name.
                    let want = match target.as_deref().filter(|_| d.is_connect()) {
                        Some(t) => t.to_string(),
                        None => format!("this box ({})", box_ips.iter().map(IpAddr::to_string).collect::<Vec<_>>().join(", ")),
                    };
                    problems.push(format!("DNS now points at {}, not {want}", elsewhere.join(", ")));
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
    /// Serving changed (live, failing): [`serving_changed`].
    Reroute,
}

/// A custom name started or stopped being served (live, failing): rebuild
/// front443 and the door, and bring the rest that follows the App's hosts up
/// to date now: its "open in app" link files (they answer only on the App's
/// live hosts, so a name that turned live after `k2 app links set` got none
/// until the next deploy) and the origins K2 tells Sew.
pub fn serving_changed(app_id: &str) {
    crate::front443::rebuild_now();
    hosts_changed(app_id);
}

/// [`serving_changed`] after the caller has rebuilt the door.
pub(crate) fn hosts_changed(app_id: &str) {
    let service = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::deploy::store::get_app(&conn, app_id).ok().flatten().map(|a| a.service_id)
    };
    if let Some(service) = service {
        crate::app_pairing::refresh_links(&service);
    }
    k2_core::tool_hook::kick_reconcile();
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
    // PB-A15: a name K2 Connect routes here is the control plane's to
    // fail; the box's own failed check only informs the owner (and never
    // starts the 7-day run). Its `released` moves it (`relay_claims::sync`).
    if d.via_relay() && matches!(d.state.as_str(), STATE_LIVE | STATE_ISSUING) {
        let _ = domains::record_app_check(
            &conn,
            host,
            &CheckRecord { checked_at: at, verified_at: None, path: o.path.map(str::to_string), failing_since: None },
        );
        let _ = domains::set_app_state(
            &conn,
            host,
            &d.state,
            Some(&format!("this server's own DNS check failed (K2 Connect still routes it; it decides): {reason}")),
        );
        return Next::Nothing;
    }
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
    let mut d = get(host)?;
    if matches!(d.state.as_str(), STATE_WAITING_DNS | STATE_FAILING) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = domains::set_app_state(&conn, host, STATE_VERIFYING, d.last_error.as_deref());
    }
    let ips = box_ips();
    let sub = crate::apps::origins::box_subdomain();
    let mut o = evaluate(&d, lk, &ips, &sub);
    // Path B: K2 Connect picks the TXT value, so the claim comes before any
    // TXT check: when DNS already shows Path B, or when Path B is what K2
    // shows for a name not yet on either path. A name DNS shows on this box
    // keeps its own proof.
    if wants_claim(&d, &o) {
        match super::relay_claims::with_api(|api| super::relay_claims::ensure_claim(&d, api)) {
            Ok(()) => {
                d = get(host)?;
                o = evaluate(&d, lk, &ips, &sub);
            }
            // No claim: only that is said. What DNS or the TXT would need
            // comes with K2 Connect's answer (the box's token proves nothing).
            Err(words) => {
                o.ok = false;
                o.reason = Some(words);
            }
        }
    }
    // Path B (PB-3): DNS and the TXT pass on the box; the control plane
    // decides whether its relays route the name here.
    if o.ok && o.path == Some(domains::PATH_CONNECT) {
        let step = super::relay_claims::with_api(|api| super::relay_claims::step(&d, api));
        let words = match step {
            super::relay_claims::Step::Verified => None,
            super::relay_claims::Step::Waiting(w) => Some(format!("through K2 Connect: {w}")),
            super::relay_claims::Step::Refused { words, .. } => Some(words),
            super::relay_claims::Step::Unreachable(w) => Some(format!("K2 Connect didn't answer ({w}); K2 tries again")),
        };
        if let Some(w) = words {
            o.ok = false;
            o.reason = Some(w);
        }
    }
    let d = get(host)?;
    match apply(&d, &o, now()) {
        Next::Issue => issue(host),
        Next::Reroute => serving_changed(&d.app),
        Next::Nothing => {}
    }
    Some((get(host)?, o))
}

/// Does this check need a claim first? No claim held, not `external`, and
/// DNS shows Path B or (DNS not on this box) Path B is what K2 shows.
fn wants_claim(d: &AppDomain, o: &Outcome) -> bool {
    if d.holds_relay_claim() {
        return false;
    }
    if o.path == Some(domains::PATH_CONNECT) {
        return true;
    }
    if o.path == Some(PATH_DIRECT) || d.state == STATE_EXTERNAL {
        return false;
    }
    let advice = setup_advice();
    advice["records"] != "none" && shown_path(d, &advice) == domains::PATH_CONNECT
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
    // Path B: the CA reaches the name through the relay, so the tunnel's
    // custom proxy must carry it first (frpc restarts to take it).
    if get(host).is_some_and(|d| d.via_relay()) {
        crate::apps::custom_names::reload();
        let g = k2_core::tunnel::custom_names::generation();
        if !k2_core::tunnel::custom_names::wait_running(g, Duration::from_secs(45)) {
            log_debug!("[apps/custom-domain] {host}: the tunnel hasn't restarted with it yet; ordering anyway");
        }
        // frps's answer on the custom proxy: while it refuses (another box's
        // live proxy still holds the name) the CA can't reach this box, so
        // no order is spent; the next tick tries again.
        use k2_core::tunnel::custom_names::{wait_proxy_answer, ProxyState};
        if let ProxyState::Waiting(why) = wait_proxy_answer(Duration::from_secs(20)) {
            let words = format!("{}: {why}", super::relay_claims::WAITING_TO_TAKE_OVER);
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = domains::set_app_state(&conn, host, STATE_ISSUING, Some(&words));
            return Err(words);
        }
    }
    match crate::domains::acme::issue_attached(host) {
        Ok(_) => {
            let app = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let d = domains::app_domain(&conn, host).ok().flatten().filter(|d| d.state == STATE_ISSUING);
                if d.is_some() {
                    let _ = domains::set_app_state(&conn, host, STATE_LIVE, None);
                }
                d.map(|d| d.app)
            };
            match app {
                Some(app) => serving_changed(&app),
                None => crate::front443::rebuild_now(),
            }
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
    // PB-A15: the control plane's list, at most hourly: its `released`
    // wins over a passing box check.
    if rows.iter().any(AppDomain::holds_relay_claim) && relay_sync_due(at) {
        match super::relay_claims::with_api(super::relay_claims::sync) {
            Ok(released) if !released.is_empty() => {
                log_debug!("[apps/custom-domain] K2 Connect released {}: no longer served", released.join(", "));
                crate::front443::rebuild_now();
            }
            Ok(_) => {}
            Err(e) => log_debug!("[apps/custom-domain] K2 Connect's list of this server's names: {e}"),
        }
    }
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

/// How often the box reads the control plane's list of its Path B names.
const RELAY_SYNC_EVERY: i64 = 3600;

/// Is the control plane's list due at `at`? (Marks it read.)
fn relay_sync_due(at: i64) -> bool {
    static LAST: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
    let last = LAST.load(std::sync::atomic::Ordering::Relaxed);
    if last != 0 && at - last < RELAY_SYNC_EVERY && !cfg!(test) {
        return false;
    }
    LAST.store(at, std::sync::atomic::Ordering::Relaxed);
    true
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

/// The DNS records for path (a): the A/AAAA at this box and the TXT proof
/// (DNS-only). None on a Mac or Windows computer (front443 is Linux-only).
pub fn records_direct(d: &AppDomain) -> Vec<J> {
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

/// The DNS records for Path B (PB-5; Rosson 2026-11-01): a CNAME to the
/// App's own `<app>.<box>.k2.dev`, or at an apex (which can't hold a
/// CNAME) an ALIAS / ANAME / flattened CNAME to it; and the TXT proof K2
/// Connect gave, once it has accepted a claim (no TXT line before that:
/// `relay_claims::txt_pending_words`).
/// Every line DNS-only. No `A` records at the relays: their addresses are
/// not promised. Empty off K2 Connect.
pub fn records_connect(d: &AppDomain, sub: &str) -> Vec<J> {
    let Some(target) = path_b_target(&d.app, sub) else { return Vec::new() };
    let apex = d.hostname.eq_ignore_ascii_case(&d.apex);
    let first = if apex {
        json!({
            "type": "ALIAS",
            "name": d.hostname,
            "value": target,
            "note": "an apex can't hold a CNAME: use ALIAS, ANAME or CNAME flattening at your DNS host (Cloudflare, \
                     DNSimple, Route 53 alias…), DNS-only (grey cloud). If your DNS host has none, use www and a redirect there",
        })
    } else {
        json!({ "type": "CNAME", "name": d.hostname, "value": target, "note": "DNS-only (no proxy / grey cloud)" })
    };
    if !d.holds_relay_claim() {
        return vec![first];
    }
    let (txt_name, txt_value) = d.txt();
    vec![first, json!({ "type": "TXT", "name": txt_name, "value": txt_value })]
}

/// The path whose records K2 shows for `d`: what DNS already shows, else
/// the one this box recommends (`advice["path"]`, Path B only when open).
pub fn shown_path(d: &AppDomain, advice: &J) -> &'static str {
    if d.is_connect() {
        return domains::PATH_CONNECT;
    }
    if d.path.as_deref() == Some(PATH_DIRECT) {
        return PATH_DIRECT;
    }
    if advice["path"] == "b" && advice.pointer("/pathB/available") == Some(&J::Bool(true)) {
        return domains::PATH_CONNECT;
    }
    if advice["pathA"] == false {
        return domains::PATH_CONNECT;
    }
    PATH_DIRECT
}

/// The DNS records the person sets for `d` (the path [`shown_path`] picks).
pub fn records_to_set(d: &AppDomain) -> Vec<J> {
    records_for(d, &setup_advice())
}

/// [`records_to_set`] for a given piece of advice.
pub fn records_for(d: &AppDomain, advice: &J) -> Vec<J> {
    // Nothing this box can serve yet (a desktop without Path B).
    if advice["records"] == "none" {
        return Vec::new();
    }
    if shown_path(d, advice) == domains::PATH_CONNECT {
        return records_connect(d, &crate::apps::origins::box_subdomain());
    }
    records_direct(d)
}

#[cfg(test)]
#[path = "custom_domain_flow_tests.rs"]
mod tests;
