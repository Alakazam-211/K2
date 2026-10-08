//! Host binding (VS4). A secret's `allowed_hosts` are normalised
//! **origins**: scheme (https unless `http://` is spelled out), lowercase
//! A-label host with no trailing dot, explicit port.
//!
//! - Exact host by default.
//! - `*.example.com` matches exactly one more label, and only when
//!   `example.com` is at least a registrable domain per the embedded Public
//!   Suffix List (ICANN + private sections) plus K2's own shared suffixes
//!   (`k2.dev`, `app.k2.dev`). `*.supabase.co`, `*.vercel.app`, `*.k2.dev`
//!   are refused: each tenant there is a different owner.
//! - IP literals: exact only, canonicalised (decimal/hex/octal IPv4 forms,
//!   IPv4-mapped IPv6).
//! - Userinfo is refused.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::error::{Result, VaultError};

const PSL: &str = include_str!("../data/public_suffix_list.dat");

/// K2's own shared suffixes: every customer box lives under these, so a
/// wildcard there would reach every customer (Rosson, VS4).
const K2_SHARED_SUFFIXES: &[&str] = &["k2.dev", "app.k2.dev"];

struct Psl {
    rules: HashSet<String>,
    wildcards: HashSet<String>,
    exceptions: HashSet<String>,
}

fn psl() -> &'static Psl {
    static P: OnceLock<Psl> = OnceLock::new();
    P.get_or_init(|| {
        let mut p = Psl { rules: HashSet::new(), wildcards: HashSet::new(), exceptions: HashSet::new() };
        for line in PSL.lines() {
            let rule = line.split_whitespace().next().unwrap_or("");
            if rule.is_empty() || rule.starts_with("//") {
                continue;
            }
            let (set, body) = if let Some(r) = rule.strip_prefix('!') {
                (&mut p.exceptions, r)
            } else if let Some(r) = rule.strip_prefix("*.") {
                (&mut p.wildcards, r)
            } else {
                (&mut p.rules, rule)
            };
            let ascii = idna::domain_to_ascii(body).unwrap_or_else(|_| body.to_lowercase());
            set.insert(ascii);
        }
        for s in K2_SHARED_SUFFIXES {
            p.rules.insert((*s).to_string());
        }
        p
    })
}

/// Number of labels in the public suffix of `domain` (ASCII, lowercase).
pub fn public_suffix_labels(domain: &str) -> usize {
    let p = psl();
    let labels: Vec<&str> = domain.split('.').collect();
    let n = labels.len();
    for i in 0..n {
        if p.exceptions.contains(&labels[i..].join(".")) {
            return n - i - 1;
        }
    }
    let mut best = 1usize; // the implicit "*" rule
    for i in 0..n {
        let cand = labels[i..].join(".");
        if p.rules.contains(&cand) {
            best = best.max(n - i);
        }
        if i + 1 < n && p.wildcards.contains(&labels[i + 1..].join(".")) {
            best = best.max(n - i);
        }
    }
    best
}

/// True when `domain` is a registrable domain or below one (it has at
/// least one label more than its public suffix).
pub fn is_registrable_or_below(domain: &str) -> bool {
    domain.split('.').count() > public_suffix_labels(domain)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostPat {
    Exact(String),
    /// `*.<parent>`: one more label under `parent`.
    Wildcard(String),
    Ip(IpAddr),
}

/// A normalised origin pattern.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OriginPattern {
    pub https: bool,
    pub host: HostPat,
    pub port: u16,
}

/// A concrete origin (what a URL points at).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    pub https: bool,
    pub host: HostKind,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HostKind {
    Domain(String),
    Ip(IpAddr),
}

fn bad(h: impl Into<String>) -> VaultError {
    VaultError::bad_request("bad_host", h)
}

/// IPv4-mapped IPv6 → IPv4 (so `[::ffff:127.0.0.1]` is 127.0.0.1).
pub fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    }
}

fn normalise_domain(d: &str) -> Result<String> {
    let d = d.strip_suffix('.').unwrap_or(d);
    if d.is_empty() {
        return Err(bad("empty host"));
    }
    let ascii = idna::domain_to_ascii_strict(d).map_err(|_| bad(format!("not a valid host name: {d}")))?;
    if ascii.len() > 253 {
        return Err(bad("host name too long"));
    }
    for label in ascii.split('.') {
        if label.is_empty()
            || label.len() > 63
            || !label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || label.starts_with('-')
            || label.ends_with('-')
        {
            return Err(bad(format!("not a valid host name: {d}")));
        }
    }
    Ok(ascii)
}

impl Origin {
    /// Parse the origin of a URL. Refuses userinfo and non-http(s).
    pub fn from_url(u: &url::Url) -> Result<Origin> {
        let https = match u.scheme() {
            "https" => true,
            "http" => false,
            s => return Err(bad(format!("scheme {s} isn't allowed (http or https)"))),
        };
        if !u.username().is_empty() || u.password().is_some() {
            return Err(bad("URLs with a user name or password aren't allowed"));
        }
        let host = match u.host() {
            Some(url::Host::Domain(d)) => HostKind::Domain(normalise_domain(d)?),
            Some(url::Host::Ipv4(v4)) => HostKind::Ip(IpAddr::V4(v4)),
            Some(url::Host::Ipv6(v6)) => HostKind::Ip(canonical_ip(IpAddr::V6(v6))),
            None => return Err(bad("URL has no host")),
        };
        let port = u.port_or_known_default().ok_or_else(|| bad("no port"))?;
        Ok(Origin { https, host, port })
    }

    pub fn parse_url(s: &str) -> Result<(url::Url, Origin)> {
        let u = url::Url::parse(s.trim()).map_err(|_| bad("not a URL"))?;
        let o = Origin::from_url(&u)?;
        Ok((u, o))
    }

    pub fn host_str(&self) -> String {
        match &self.host {
            HostKind::Domain(d) => d.clone(),
            HostKind::Ip(IpAddr::V6(v6)) => format!("[{v6}]"),
            HostKind::Ip(ip) => ip.to_string(),
        }
    }

    pub fn render(&self) -> String {
        format!("{}://{}:{}", if self.https { "https" } else { "http" }, self.host_str(), self.port)
    }
}

impl OriginPattern {
    /// Parse and normalise. `allow_wildcard` is false for agents and
    /// Thread-card proposals (VS4: wildcards only from people).
    pub fn parse(s: &str, allow_wildcard: bool) -> Result<OriginPattern> {
        let s = s.trim();
        if s.is_empty() {
            return Err(bad("empty host"));
        }
        let with_scheme = if s.contains("://") { s.to_string() } else { format!("https://{s}") };
        let (scheme, rest) = with_scheme.split_once("://").expect("has ://");
        let https = match scheme.to_ascii_lowercase().as_str() {
            "https" => true,
            "http" => false,
            other => return Err(bad(format!("scheme {other} isn't allowed (http or https)"))),
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if authority.contains('@') {
            return Err(bad("hosts can't include a user name or password"));
        }
        if rest.len() > authority.len() && !matches!(&rest[authority.len()..], "/" | "") {
            return Err(bad("a host is just scheme://host[:port], no path"));
        }
        if let Some(parent) = authority.strip_prefix("*.") {
            if !allow_wildcard {
                return Err(VaultError::bad_request(
                    "wildcard_not_allowed",
                    "wildcards can only be added by a person in Settings → Vault; use the exact host",
                ));
            }
            let (host_part, port) = split_port(parent, https)?;
            let parent = normalise_domain(host_part)?;
            if !is_registrable_or_below(&parent) {
                return Err(VaultError::bad_request(
                    "wildcard_public_suffix",
                    format!(
                        "*.{parent} would match other people's sites (each one is a different owner); enter the exact host"
                    ),
                ));
            }
            return Ok(OriginPattern { https, host: HostPat::Wildcard(parent), port });
        }
        if authority.contains('*') {
            return Err(bad("a wildcard is only allowed as the first label (*.example.com)"));
        }
        // Let the URL parser canonicalise IP forms and IDNA.
        let u = url::Url::parse(&format!("{}://{authority}/", if https { "https" } else { "http" }))
            .map_err(|_| bad(format!("not a valid host: {authority}")))?;
        let o = Origin::from_url(&u)?;
        if o.port == 0 {
            return Err(bad("bad port"));
        }
        let host = match o.host {
            HostKind::Domain(d) => HostPat::Exact(d),
            HostKind::Ip(ip) => HostPat::Ip(ip),
        };
        Ok(OriginPattern { https, host, port: o.port })
    }

    pub fn render(&self) -> String {
        let host = match &self.host {
            HostPat::Exact(d) => d.clone(),
            HostPat::Wildcard(p) => format!("*.{p}"),
            HostPat::Ip(IpAddr::V6(v6)) => format!("[{v6}]"),
            HostPat::Ip(ip) => ip.to_string(),
        };
        format!("{}://{}:{}", if self.https { "https" } else { "http" }, host, self.port)
    }

    pub fn is_wildcard(&self) -> bool {
        matches!(self.host, HostPat::Wildcard(_))
    }

    /// Does this pattern name an exact IP (the only way to reach a
    /// private or loopback address, VS5)?
    pub fn is_exact_ip(&self) -> bool {
        matches!(self.host, HostPat::Ip(_))
    }

    pub fn matches(&self, o: &Origin) -> bool {
        if self.https != o.https || self.port != o.port {
            return false;
        }
        match (&self.host, &o.host) {
            (HostPat::Exact(p), HostKind::Domain(d)) => p == d,
            (HostPat::Wildcard(parent), HostKind::Domain(d)) => match d.strip_suffix(parent.as_str()) {
                Some(head) => {
                    let head = match head.strip_suffix('.') {
                        Some(h) => h,
                        None => return false,
                    };
                    !head.is_empty() && !head.contains('.')
                }
                None => false,
            },
            (HostPat::Ip(p), HostKind::Ip(ip)) => canonical_ip(*p) == canonical_ip(*ip),
            _ => false,
        }
    }
}

fn split_port(s: &str, https: bool) -> Result<(&str, u16)> {
    match s.rsplit_once(':') {
        Some((h, p)) => {
            let port: u16 = p.parse().map_err(|_| bad("bad port"))?;
            if port == 0 {
                return Err(bad("bad port"));
            }
            Ok((h, port))
        }
        None => Ok((s, if https { 443 } else { 80 })),
    }
}

/// Normalise a list (dedup, sorted, stored form).
pub fn normalise_list(hosts: &[String], allow_wildcard: bool) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for h in hosts {
        let r = OriginPattern::parse(h, allow_wildcard)?.render();
        if !out.contains(&r) {
            out.push(r);
        }
    }
    out.sort();
    Ok(out)
}

/// Parse stored (already normalised) patterns.
pub fn parse_stored(hosts: &[String]) -> Vec<OriginPattern> {
    hosts.iter().filter_map(|h| OriginPattern::parse(h, true).ok()).collect()
}

/// Is `new` a widening of `old` (a host in `new` not covered by `old`)?
pub fn widens(old: &[String], new: &[String]) -> bool {
    new.iter().any(|h| !old.contains(h))
}

/// Human display (VS4): the A-label, plus the U-label only when it's a
/// single script. Returns `(a_label, u_label, mixed_script_warning)`.
pub fn display_host(a_label: &str) -> (String, Option<String>, bool) {
    if !a_label.split('.').any(|l| l.starts_with("xn--")) {
        return (a_label.to_string(), None, false);
    }
    let (u, res) = idna::domain_to_unicode(a_label);
    if res.is_err() {
        return (a_label.to_string(), None, true);
    }
    let mut scripts: HashSet<u8> = HashSet::new();
    for c in u.chars().filter(|c| !c.is_ascii_digit() && *c != '.' && *c != '-') {
        scripts.insert(script_class(c));
    }
    // ASCII letters count as Latin; mixing Latin with anything else is the
    // homograph case.
    if scripts.len() > 1 {
        (a_label.to_string(), None, true)
    } else {
        (a_label.to_string(), Some(u), false)
    }
}

fn script_class(c: char) -> u8 {
    match c as u32 {
        0x0041..=0x024F | 0x1E00..=0x1EFF => 1, // Latin
        0x0370..=0x03FF | 0x1F00..=0x1FFF => 2, // Greek
        0x0400..=0x052F => 3,                   // Cyrillic
        0x0590..=0x05FF => 4,                   // Hebrew
        0x0600..=0x06FF | 0x0750..=0x077F => 5, // Arabic
        0x3040..=0x30FF => 6,                   // Kana
        0x4E00..=0x9FFF => 7,                   // Han
        0xAC00..=0xD7AF => 8,                   // Hangul
        0x0E00..=0x0E7F => 9,                   // Thai
        0x0900..=0x097F => 10,                  // Devanagari
        _ => 99,
    }
}

/// Addresses no fetch/proxy may reach unless the pattern names that exact
/// IP (VS5): loopback, private, link-local and metadata, CGNAT/tailnets,
/// unspecified, multicast, broadcast/reserved, ULA, NAT64-mapped private.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match canonical_ip(ip) {
        IpAddr::V4(v4) => blocked_v4(v4),
        IpAddr::V6(v6) => blocked_v6(v6),
    }
}

fn blocked_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || o[0] == 0
        || (o[0] == 100 && (o[1] & 0xC0) == 64) // 100.64.0.0/10
        || o[0] >= 240 // reserved
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24
        || (o[0] == 198 && (o[1] & 0xFE) == 18) // 198.18.0.0/15 benchmarking
}

fn blocked_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    if (s[0] & 0xFE00) == 0xFC00 || (s[0] & 0xFFC0) == 0xFE80 {
        return true; // ULA fc00::/7, link-local fe80::/10
    }
    // NAT64 64:ff9b::/96 → judge the embedded IPv4.
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6].iter().all(|&x| x == 0) {
        let v4 = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
        return blocked_v4(v4);
    }
    // IPv4-compatible (deprecated) ::a.b.c.d
    if s[..6].iter().all(|&x| x == 0) {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<OriginPattern> {
        OriginPattern::parse(s, true)
    }

    fn o(s: &str) -> Origin {
        Origin::parse_url(s).expect("url").1
    }

    #[test]
    fn normalises_exact_hosts() {
        let cases = [
            ("api.stripe.com", "https://api.stripe.com:443"),
            ("HTTPS://API.Stripe.COM/", "https://api.stripe.com:443"),
            ("api.stripe.com.", "https://api.stripe.com:443"),
            ("https://api.stripe.com:443", "https://api.stripe.com:443"),
            ("http://10.0.0.5:8080", "http://10.0.0.5:8080"),
            ("http://2130706433", "http://127.0.0.1:80"),
            ("http://0x7f.1", "http://127.0.0.1:80"),
            ("http://[::ffff:127.0.0.1]:81", "http://127.0.0.1:81"),
            ("bücher.example", "https://xn--bcher-kva.example:443"),
        ];
        for (input, want) in cases {
            assert_eq!(p(input).unwrap_or_else(|e| panic!("{input}: {e}")).render(), want, "{input}");
        }
    }

    #[test]
    fn refuses_bad_hosts() {
        for s in ["", "ftp://x.com", "https://user:pw@api.stripe.com", "https://a.com/path", "a*.b.com", "https://x.com:0", "https://x.com:99999"] {
            assert!(p(s).is_err(), "{s} should be refused");
        }
    }

    #[test]
    fn psl_table() {
        let refused = ["*.supabase.co", "*.vercel.app", "*.github.io", "*.k2.dev", "*.app.k2.dev", "*.com", "*.co.uk", "*.herokuapp.com"];
        for s in refused {
            let e = p(s).expect_err(s);
            assert_eq!(e.code, "wildcard_public_suffix", "{s}");
        }
        for s in ["*.stripe.com", "*.example.co.uk", "*.myproject.supabase.co", "*.box1.k2.dev"] {
            p(s).unwrap_or_else(|e| panic!("{s}: {e}"));
        }
        assert_eq!(public_suffix_labels("foo.bar.co.uk"), 2);
        assert_eq!(public_suffix_labels("abc.supabase.co"), 2);
        assert!(!is_registrable_or_below("supabase.co"));
        assert!(is_registrable_or_below("abc.supabase.co"));
    }

    #[test]
    fn wildcards_only_from_people() {
        let e = OriginPattern::parse("*.stripe.com", false).expect_err("agents can't add wildcards");
        assert_eq!(e.code, "wildcard_not_allowed");
    }

    #[test]
    fn matching() {
        let exact = p("api.stripe.com").expect("ok");
        assert!(exact.matches(&o("https://api.stripe.com/v1/charges")));
        assert!(exact.matches(&o("https://API.STRIPE.COM.:443/x")));
        assert!(!exact.matches(&o("http://api.stripe.com/")), "scheme must match");
        assert!(!exact.matches(&o("https://api.stripe.com:8443/")), "port must match");
        assert!(!exact.matches(&o("https://evil.api.stripe.com/")));
        assert!(!exact.matches(&o("https://api.stripe.com.evil.example/")));
        let wc = p("*.stripe.com").expect("ok");
        assert!(wc.matches(&o("https://files.stripe.com/")));
        assert!(!wc.matches(&o("https://a.b.stripe.com/")), "one label only");
        assert!(!wc.matches(&o("https://stripe.com/")));
        assert!(!wc.matches(&o("https://notstripe.com/")));
        let ip = p("http://127.0.0.1:8080").expect("ok");
        assert!(ip.matches(&o("http://2130706433:8080/")));
        assert!(ip.matches(&o("http://[::ffff:127.0.0.1]:8080/")));
    }

    #[test]
    fn idn_homograph_is_not_the_ascii_host() {
        // Cyrillic "а" (U+0430) in place of the Latin a.
        let homograph = p("\u{0430}pi.stripe.com").expect("valid IDN");
        assert_ne!(homograph.render(), "https://api.stripe.com:443");
        let real = p("api.stripe.com").expect("ok");
        assert!(!real.matches(&o("https://\u{0430}pi.stripe.com/")));
        let host = match &homograph.host {
            HostPat::Exact(d) => d.clone(),
            other => panic!("unexpected {other:?}"),
        };
        let (_a, u, mixed) = display_host(&host);
        assert!(mixed, "mixed Latin/Cyrillic must be flagged");
        assert!(u.is_none());
        let (_a, u, mixed) = display_host("xn--bcher-kva.example");
        assert!(!mixed);
        assert_eq!(u.as_deref(), Some("bücher.example"));
    }

    #[test]
    fn blocked_ranges() {
        for s in [
            "127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "0.1.2.3",
            "100.64.0.1", "224.0.0.1", "255.255.255.255", "::1", "::", "fc00::1", "fd12::1", "fe80::1",
            "ff02::1", "64:ff9b::a9fe:a9fe", "::ffff:10.0.0.1",
        ] {
            assert!(is_blocked_ip(s.parse().expect("ip")), "{s} should be blocked");
        }
        for s in ["1.1.1.1", "93.184.216.34", "2606:4700::1111", "64:ff9b::808:808"] {
            assert!(!is_blocked_ip(s.parse().expect("ip")), "{s} should be allowed");
        }
    }

    #[test]
    fn widening() {
        let old = vec!["https://api.stripe.com:443".to_string()];
        assert!(!widens(&old, &[]));
        assert!(!widens(&old, &old));
        assert!(widens(&old, &["https://evil.example:443".to_string()]));
    }
}
