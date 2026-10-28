//! The owner's firewall choices and the public-ports view's shared types
//! (0.46.6, `prd-custom-domains-path-a-0466-v1` §4–§6, vs-live PA-A1…A19).
//!
//! - **Where the choices live**: `~/.k2/firewall.json`, its own file (like
//!   `front443.json`), not a block in `app_settings`. Two reasons: the
//!   generic Settings write can't reach it (only `/cli/firewall/*`, owner
//!   and Admin, refuses an agent), and a 0.46.5 daemon neither reads it nor
//!   drops it when it saves its own settings (PA-A19: `AppSettings` drops
//!   unknown keys on every round-trip). A missing or unreadable file is the
//!   defaults: SSH from anywhere on 22, no extra ports, the App guard on.
//! - **The view** (`GET /cli/firewall/ports`) is computed by the daemon from
//!   its own state (`k2-daemon::firewall`), never by scanning. This module
//!   holds its row type ([`PortEntry`]), its [`hash`] and the ready-made
//!   rules ([`rules`]) for boxes nobody applies the list for.
//! - **SSH is always public** (the lockout guard): its row can be narrowed
//!   to a list of addresses, never removed, and an empty list is refused.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The contract's version (`version` in the view). Bumped only on a
/// breaking change; new fields are additive (PRD §4.1 rule 7).
pub const VIEW_VERSION: u32 = 1;
/// G1: open connections per visitor address at the App TLS door.
pub const DEFAULT_CONNS_PER_IP: u32 = 64;
/// G2: writes (`POST`/`PUT`/`PATCH`/`DELETE`) per visitor per App per minute.
pub const DEFAULT_WRITES_PER_MIN: u32 = 120;
/// Bounds the owner may set (a limit of 0 is "off", said with `off`).
pub const MIN_LIMIT: u32 = 1;
pub const MAX_CONNS_PER_IP: u32 = 16_384;
pub const MAX_WRITES_PER_MIN: u32 = 100_000;
/// Most owner-opened ports and most addresses on one rule.
pub const MAX_OPEN: usize = 64;
pub const MAX_ALLOW_FROM: usize = 32;
pub const MAX_LABEL: usize = 80;

/// TCP or UDP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Proto {
    Tcp,
    Udp,
}

impl Proto {
    pub fn as_str(self) -> &'static str {
        match self {
            Proto::Tcp => "tcp",
            Proto::Udp => "udp",
        }
    }
}

/// `22/tcp`, `6000/udp`, or a bare port (TCP).
pub fn parse_port_proto(raw: &str) -> Result<(u16, Proto), String> {
    let t = raw.trim().to_ascii_lowercase();
    let (p, proto) = match t.split_once('/') {
        Some((p, "tcp")) => (p.to_string(), Proto::Tcp),
        Some((p, "udp")) => (p.to_string(), Proto::Udp),
        Some((_, other)) => return Err(format!("unknown protocol '{other}': use tcp or udp (e.g. 6000/udp)")),
        None => (t.clone(), Proto::Tcp),
    };
    let port: u16 = p.parse().map_err(|_| format!("not a port: '{raw}' (1–65535, e.g. 6000/udp)"))?;
    if port == 0 {
        return Err("port 0 is not a port".into());
    }
    Ok((port, proto))
}

/// One source address rule: `any`, an address, or `addr/prefix`.
/// Returns the canonical text (`any`, `198.51.100.0/24`, `2001:db8::/48`;
/// a bare address gets /32 or /128). Host bits are cleared.
pub fn normalize_cidr(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.eq_ignore_ascii_case("any") {
        return Ok("any".into());
    }
    let (addr, bits) = match t.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (t, None),
    };
    let ip: IpAddr = addr.parse().map_err(|_| format!("not an address or CIDR: '{t}' (e.g. 198.51.100.0/24)"))?;
    let max = if ip.is_ipv4() { 32u8 } else { 128u8 };
    let bits: u8 = match bits {
        Some(b) => b.parse().map_err(|_| format!("bad prefix in '{t}'"))?,
        None => max,
    };
    if bits > max {
        return Err(format!("prefix /{bits} is too long for '{addr}'"));
    }
    let net = match ip {
        IpAddr::V4(a) => {
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - u32::from(bits)) };
            IpAddr::V4((u32::from(a) & mask).into())
        }
        IpAddr::V6(a) => {
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - u32::from(bits)) };
            IpAddr::V6((u128::from(a) & mask).into())
        }
    };
    Ok(format!("{net}/{bits}"))
}

/// A list of source rules, normalized, deduplicated, sorted. `any` alone
/// (or an empty input) is "anyone"; `any` beside addresses is refused.
pub fn normalize_allow_from(raw: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for r in raw {
        for part in r.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let n = normalize_cidr(part)?;
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    if out.iter().any(|s| s == "any") {
        if out.len() > 1 {
            return Err("'any' can't be combined with addresses".into());
        }
        return Ok(Vec::new());
    }
    if out.len() > MAX_ALLOW_FROM {
        return Err(format!("at most {MAX_ALLOW_FROM} addresses per rule"));
    }
    out.sort();
    Ok(out)
}

/// The SSH row: always public; the owner may move the recorded port and
/// narrow who reaches it. K2 doesn't manage sshd (PA-A15): the port is a
/// record, and a wrong one makes the collector hold (the safe outcome).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SshRule {
    pub port: u16,
    /// Empty = anyone.
    pub allow_from: Vec<String>,
}

impl Default for SshRule {
    fn default() -> Self {
        SshRule { port: 22, allow_from: Vec::new() }
    }
}

/// A port the owner publishes for something they run themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenPort {
    pub port: u16,
    pub proto: Proto,
    pub label: String,
    #[serde(default)]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub added_at: i64,
    #[serde(default)]
    pub added_by: String,
}

/// One App's guard override (absent fields take the box's).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppGuard {
    pub enabled: Option<bool>,
    pub writes_per_min: Option<u32>,
}

/// The minimal E3 guard (PRD §6.2): G1 is box-wide (it runs before K2
/// knows which App a connection is for); G2 is per App.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GuardConfig {
    /// G1 on/off for the box.
    pub enabled: bool,
    pub conns_per_ip: u32,
    /// G2's default for every App.
    pub writes_per_min: u32,
    /// G2 per App (by App id).
    pub apps: BTreeMap<String, AppGuard>,
}

impl Default for GuardConfig {
    fn default() -> Self {
        GuardConfig {
            enabled: true,
            conns_per_ip: DEFAULT_CONNS_PER_IP,
            writes_per_min: DEFAULT_WRITES_PER_MIN,
            apps: BTreeMap::new(),
        }
    }
}

impl GuardConfig {
    /// G2 for `app_id`: `None` = off, else the per-minute limit.
    pub fn writes_for(&self, app_id: &str) -> Option<u32> {
        let a = self.apps.get(app_id);
        let on = a.and_then(|a| a.enabled).unwrap_or(self.enabled);
        on.then(|| a.and_then(|a| a.writes_per_min).unwrap_or(self.writes_per_min).max(MIN_LIMIT))
    }

    /// G1 for the box: `None` = off.
    pub fn conns(&self) -> Option<u32> {
        self.enabled.then_some(self.conns_per_ip.max(MIN_LIMIT))
    }
}

/// K2 Web's note that it applied the view at the box's provider
/// (`POST /cli/firewall/upstream`, KW-Q3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamAck {
    pub hash: String,
    pub provider: String,
    pub applied_at: i64,
    pub recorded_at: i64,
    pub by: String,
}

/// `~/.k2/firewall.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FirewallConfig {
    pub ssh: SshRule,
    pub open: Vec<OpenPort>,
    pub guard: GuardConfig,
    /// `k2` (K2 Web applies the list at the provider) or `owner` (nobody
    /// does: the owner uses `k2 firewall rules`). `None` = derived: `k2`
    /// once K2 Web has acked an apply, else `owner` (KW-Q2).
    pub managed_by: Option<String>,
    pub upstream: Option<UpstreamAck>,
    pub changed_at: Option<i64>,
}

impl Default for FirewallConfig {
    fn default() -> Self {
        FirewallConfig {
            ssh: SshRule::default(),
            open: Vec::new(),
            guard: GuardConfig::default(),
            managed_by: None,
            upstream: None,
            changed_at: None,
        }
    }
}

impl FirewallConfig {
    /// Who applies the list upstream: `k2` or `owner`.
    pub fn managed_by(&self) -> &str {
        match self.managed_by.as_deref() {
            Some("k2") => "k2",
            Some(_) => "owner",
            None if self.upstream.is_some() => "k2",
            None => "owner",
        }
    }
}

pub fn path() -> PathBuf {
    crate::paths::k2_home().join("firewall.json")
}

/// The saved choices; a missing file is the defaults. An unreadable or
/// malformed file is an error (the view must never silently drop the
/// owner's ports or SSH narrowing).
pub fn load_checked() -> Result<FirewallConfig, String> {
    let p = path();
    match std::fs::read_to_string(&p) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FirewallConfig::default()),
        Err(e) => Err(format!("read {}: {e}", p.display())),
    }
}

/// [`load_checked`], defaults on error (hot paths: the guard).
pub fn load() -> FirewallConfig {
    load_checked().unwrap_or_default()
}

pub fn save(c: &FirewallConfig) -> Result<(), String> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("mkdir {}: {e}", d.display()))?;
    }
    let text = serde_json::to_string_pretty(c).map_err(|e| e.to_string())?;
    crate::fs_atomic::atomic_write_private(&p, text.as_bytes()).map_err(|e| format!("write {}: {e}", p.display()))
}

// ── the view's rows ──────────────────────────────────────────────────

/// One row of the view (`public` or `knownNotPublic`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortEntry {
    pub port: u16,
    pub proto: Proto,
    /// `ssh`, `front443`, `skin-direct`, `hostmail`, `sew-voice`, `daemon`, `owner`.
    pub source: String,
    /// `["any"]` or CIDRs (public rows only; empty on `knownNotPublic`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_from: Vec<String>,
    pub why: String,
}

impl PortEntry {
    pub fn public(port: u16, proto: Proto, source: &str, allow: &[String], why: impl Into<String>) -> Self {
        let allow_from = if allow.is_empty() { vec!["any".to_string()] } else { allow.to_vec() };
        PortEntry { port, proto, source: source.into(), allow_from, why: why.into() }
    }
    pub fn closed(port: u16, proto: Proto, source: &str, why: impl Into<String>) -> Self {
        PortEntry { port, proto, source: source.into(), allow_from: Vec::new(), why: why.into() }
    }
}

/// Sort and merge rows: one row per (port, proto). The first source kept
/// (callers add in priority order); a later source on the same port adds
/// its reason (`why` joined with "; ") and widens `allowFrom` to the union
/// (`any` wins).
pub fn merge(rows: Vec<PortEntry>) -> Vec<PortEntry> {
    let mut out: Vec<PortEntry> = Vec::new();
    for r in rows {
        if let Some(cur) = out.iter_mut().find(|c| c.port == r.port && c.proto == r.proto) {
            if !cur.why.contains(&r.why) {
                cur.why = format!("{}; {}", cur.why, r.why);
            }
            if cur.allow_from.iter().any(|a| a == "any") || r.allow_from.iter().any(|a| a == "any") {
                if !cur.allow_from.is_empty() || !r.allow_from.is_empty() {
                    cur.allow_from = vec!["any".into()];
                }
            } else {
                for a in r.allow_from {
                    if !cur.allow_from.contains(&a) {
                        cur.allow_from.push(a);
                    }
                }
                cur.allow_from.sort();
            }
        } else {
            out.push(r);
        }
    }
    out.sort_by(|a, b| (a.port, a.proto).cmp(&(b.port, b.proto)));
    out
}

/// `sha256:<hex>` over the `public` rows only (port, proto, source,
/// allowFrom; never `why`, never `knownNotPublic`), sorted, so it changes
/// iff the allow-list changes (PRD §4.1).
pub fn hash(public: &[PortEntry]) -> String {
    let mut rows: Vec<(u16, &str, &str, Vec<String>)> = public
        .iter()
        .map(|r| {
            let mut a = r.allow_from.clone();
            a.sort();
            (r.port, r.proto.as_str(), r.source.as_str(), a)
        })
        .collect();
    rows.sort();
    let canon = serde_json::to_string(&rows).expect("plain tuples");
    let mut h = Sha256::new();
    h.update(canon.as_bytes());
    format!("sha256:{}", hex_lower(&h.finalize()))
}

fn hex_lower(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ── ready-made rules (PRD §5.3) ──────────────────────────────────────

/// `nft` or `ufw`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulesFormat {
    Nft,
    Ufw,
}

impl RulesFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "nft" | "nftables" => Some(RulesFormat::Nft),
            "ufw" => Some(RulesFormat::Ufw),
            _ => None,
        }
    }
}

fn is_v6(cidr: &str) -> bool {
    cidr.contains(':')
}

/// Shell-safe comment text (letters, digits, spaces and a few marks).
fn comment(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || " .,:()+-/_".contains(c) { c } else { ' ' })
        .take(120)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Default-deny inbound rules from the `public` rows, SSH first so the
/// rules never lock the owner out. K2 never applies them (no host firewall
/// in 0.46.6, PRD §5.2): the owner reviews and runs them.
pub fn rules(public: &[PortEntry], fmt: RulesFormat, label: &str, view_hash: &str) -> String {
    let mut rows: Vec<&PortEntry> = public.iter().collect();
    rows.sort_by_key(|r| (r.source != "ssh", r.port, r.proto));
    let mut out = String::new();
    match fmt {
        RulesFormat::Nft => {
            out.push_str(&format!("# K2 firewall rules for {} ({view_hash})\n", comment(label)));
            out.push_str("# Review before applying; K2 does not apply these. SSH comes first.\n");
            out.push_str("# Apply: sudo nft -f <this file>   Remove: sudo nft delete table inet k2_filter\n");
            out.push_str("table inet k2_filter {\n  chain input {\n    type filter hook input priority 0; policy drop;\n");
            out.push_str("    ct state established,related accept\n    ct state invalid drop\n    iif \"lo\" accept\n");
            out.push_str("    ip protocol icmp accept\n    ip6 nexthdr ipv6-icmp accept\n");
            for r in rows {
                let p = r.proto.as_str();
                let why = comment(&r.why);
                if r.allow_from.iter().any(|a| a == "any") || r.allow_from.is_empty() {
                    out.push_str(&format!("    {p} dport {} accept comment \"{why}\"\n", r.port));
                } else {
                    let v4: Vec<&str> = r.allow_from.iter().filter(|a| !is_v6(a)).map(String::as_str).collect();
                    let v6: Vec<&str> = r.allow_from.iter().filter(|a| is_v6(a)).map(String::as_str).collect();
                    if !v4.is_empty() {
                        out.push_str(&format!("    ip saddr {{ {} }} {p} dport {} accept comment \"{why}\"\n", v4.join(", "), r.port));
                    }
                    if !v6.is_empty() {
                        out.push_str(&format!("    ip6 saddr {{ {} }} {p} dport {} accept comment \"{why}\"\n", v6.join(", "), r.port));
                    }
                }
            }
            out.push_str("  }\n}\n");
        }
        RulesFormat::Ufw => {
            out.push_str(&format!("# K2 firewall rules for {} ({view_hash})\n", comment(label)));
            out.push_str("# Review before running; K2 does not apply these. SSH comes first.\n");
            for r in rows {
                let p = r.proto.as_str();
                let why = comment(&r.why);
                if r.allow_from.iter().any(|a| a == "any") || r.allow_from.is_empty() {
                    out.push_str(&format!("sudo ufw allow {}/{p} comment '{why}'\n", r.port));
                } else {
                    for a in &r.allow_from {
                        out.push_str(&format!("sudo ufw allow from {a} to any port {} proto {p} comment '{why}'\n", r.port));
                    }
                }
            }
            out.push_str("sudo ufw default deny incoming\nsudo ufw default allow outgoing\nsudo ufw enable\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_and_protocols_parse() {
        assert_eq!(parse_port_proto("22/tcp").unwrap(), (22, Proto::Tcp));
        assert_eq!(parse_port_proto("6000/UDP").unwrap(), (6000, Proto::Udp));
        assert_eq!(parse_port_proto("8080").unwrap(), (8080, Proto::Tcp));
        for bad in ["0", "65536/tcp", "22/sctp", "x/tcp", ""] {
            assert!(parse_port_proto(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn cidrs_normalize_and_any_stands_alone() {
        assert_eq!(normalize_cidr("198.51.100.7/24").unwrap(), "198.51.100.0/24");
        assert_eq!(normalize_cidr("203.0.113.9").unwrap(), "203.0.113.9/32");
        assert_eq!(normalize_cidr("2001:db8::1/48").unwrap(), "2001:db8::/48");
        assert_eq!(normalize_cidr("ANY").unwrap(), "any");
        assert!(normalize_cidr("10.0.0.0/33").is_err());
        assert!(normalize_cidr("nope").is_err());
        assert_eq!(normalize_allow_from(&["any".into()]).unwrap(), Vec::<String>::new());
        assert!(normalize_allow_from(&["any".into(), "10.0.0.1".into()]).is_err());
        assert_eq!(
            normalize_allow_from(&["10.0.0.9/8, 10.1.2.3/8".into(), "192.0.2.1".into()]).unwrap(),
            vec!["10.0.0.0/8".to_string(), "192.0.2.1/32".to_string()]
        );
    }

    #[test]
    fn the_hash_follows_the_allow_list_only() {
        let a = vec![
            PortEntry::public(22, Proto::Tcp, "ssh", &[], "SSH"),
            PortEntry::public(443, Proto::Tcp, "front443", &[], "front"),
        ];
        let mut b = a.clone();
        b.reverse();
        b[0].why = "different words".into();
        assert_eq!(hash(&a), hash(&b), "order and reasons don't move the hash");
        let mut c = a.clone();
        c.push(PortEntry::public(6000, Proto::Udp, "owner", &[], "game"));
        assert_ne!(hash(&a), hash(&c));
        let mut d = a.clone();
        d[0].allow_from = vec!["198.51.100.0/24".into()];
        assert_ne!(hash(&a), hash(&d));
        assert!(hash(&a).starts_with("sha256:") && hash(&a).len() == 7 + 64);
    }

    #[test]
    fn merge_keeps_one_row_per_port() {
        let rows = merge(vec![
            PortEntry::public(443, Proto::Tcp, "front443", &[], "the :443 front"),
            PortEntry::public(443, Proto::Tcp, "hostmail", &[], "mail web"),
            PortEntry::public(80, Proto::Tcp, "hostmail", &[], "ACME"),
        ]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].port, 80);
        assert_eq!(rows[1].source, "front443");
        assert!(rows[1].why.contains("mail web"), "{rows:?}");
    }

    #[test]
    fn guard_defaults_and_overrides() {
        let mut g = GuardConfig::default();
        assert_eq!(g.conns(), Some(64));
        assert_eq!(g.writes_for("a"), Some(120));
        g.apps.insert("a".into(), AppGuard { enabled: Some(false), writes_per_min: None });
        g.apps.insert("b".into(), AppGuard { enabled: None, writes_per_min: Some(10) });
        assert_eq!(g.writes_for("a"), None);
        assert_eq!(g.writes_for("b"), Some(10));
        g.enabled = false;
        assert_eq!(g.conns(), None);
        assert_eq!(g.writes_for("b"), None, "box off, App unset → off");
    }

    #[test]
    fn a_missing_file_is_the_defaults_and_a_broken_one_is_an_error() {
        let _h = crate::test_env::TempHome::new();
        assert_eq!(load_checked().unwrap(), FirewallConfig::default());
        let mut c = FirewallConfig::default();
        c.ssh.allow_from = vec!["198.51.100.0/24".into()];
        save(&c).unwrap();
        assert_eq!(load_checked().unwrap(), c);
        // An old file without new fields still reads (PA-A19).
        std::fs::write(path(), r#"{"ssh":{"port":2222}}"#).unwrap();
        let old = load_checked().unwrap();
        assert_eq!(old.ssh.port, 2222);
        assert_eq!(old.guard, GuardConfig::default());
        std::fs::write(path(), "{not json").unwrap();
        assert!(load_checked().is_err());
    }

    #[test]
    fn managed_by_follows_the_ack_unless_set() {
        let mut c = FirewallConfig::default();
        assert_eq!(c.managed_by(), "owner");
        c.upstream = Some(UpstreamAck { hash: "sha256:x".into(), provider: "hetzner".into(), applied_at: 1, recorded_at: 2, by: "owner".into() });
        assert_eq!(c.managed_by(), "k2");
        c.managed_by = Some("owner".into());
        assert_eq!(c.managed_by(), "owner");
    }

    #[test]
    fn rules_put_ssh_first_and_narrow_by_family() {
        let public = vec![
            PortEntry::public(443, Proto::Tcp, "front443", &[], "front"),
            PortEntry::public(22, Proto::Tcp, "ssh", &["198.51.100.0/24".into(), "2001:db8::/48".into()], "SSH"),
            PortEntry::public(7882, Proto::Udp, "sew-voice", &[], "media"),
        ];
        let nft = rules(&public, RulesFormat::Nft, "box", "sha256:abc");
        let ssh4 = nft.find("ip saddr { 198.51.100.0/24 } tcp dport 22").expect(&nft);
        let ssh6 = nft.find("ip6 saddr { 2001:db8::/48 } tcp dport 22").expect(&nft);
        let https = nft.find("tcp dport 443 accept").expect(&nft);
        assert!(ssh4 < https && ssh6 < https, "{nft}");
        assert!(nft.contains("udp dport 7882 accept"), "{nft}");
        assert!(nft.contains("policy drop"), "{nft}");
        let ufw = rules(&public, RulesFormat::Ufw, "box", "sha256:abc");
        let first_allow = ufw.find("sudo ufw allow").unwrap();
        assert!(ufw[first_allow..].starts_with("sudo ufw allow from 198.51.100.0/24 to any port 22 proto tcp"), "{ufw}");
        assert!(ufw.find("default deny").unwrap() > ufw.find("port 22").unwrap(), "{ufw}");
        assert!(ufw.contains("sudo ufw allow 7882/udp"), "{ufw}");
    }
}
