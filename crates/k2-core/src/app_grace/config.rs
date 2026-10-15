//! The App config document (prd-k2-app-runtime-v1 §4.4) and the S13
//! classifier: which changes narrow (apply now), which widen, and who may
//! make a widening at once (§5.6, §5.11.1).
//!
//! The DB is the truth: `apps.memory_limit_bytes`, `apps.isolation`,
//! `apps.path`, and `apps.config_json` (0168) for the rest. One request is
//! a list of [`Op`]s; each op is classified on its own, so a call that
//! narrows one thing and widens another applies the narrowing now and
//! holds only the widening (§5.6 "narrowing never waits").
//!
//! In 0.46.0 the walls that enforce egress, ports and the private database
//! aren't on yet (0.46.1); the values are stored and approved now so the
//! wall starts from the owner's decisions.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// The fields without a column of their own (`apps.config_json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub resources: Resources,
    pub egress: Egress,
    /// Extra ports beyond the one front port (`$PORT`). Any entry is an
    /// open-path feature.
    pub ports: Vec<PortSpec>,
    pub recycle: Recycle,
    /// `normal` | `high` (high → VM once stage V ships).
    pub risk: String,
    pub db: Db,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            resources: Resources::default(),
            egress: Egress::default(),
            ports: Vec::new(),
            recycle: Recycle::default(),
            risk: "normal".into(),
            db: Db::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Resources {
    pub cpu_weight: Option<u32>,
    pub tasks: Option<u32>,
    pub disk_bytes: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Egress {
    /// `allowlist` | `broad`.
    pub mode: String,
    /// `host:port`.
    pub hosts: Vec<String>,
}

impl Default for Egress {
    fn default() -> Self {
        Self { mode: "allowlist".into(), hosts: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortSpec {
    pub port: u16,
    /// `tcp` | `udp`.
    pub proto: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Recycle {
    pub every_secs: Option<i64>,
    pub at_pct: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Db {
    pub private: bool,
    pub raw: bool,
}

/// Everything a config call can read or change about one App.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub memory_limit_bytes: Option<i64>,
    /// `container` | `vm` | `process` | `none`.
    pub isolation: String,
    /// `guarded` | `open`.
    pub path: String,
    pub config: AppConfig,
}

/// One change. Serialized into held requests and applied on approve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", content = "value", rename_all = "camelCase")]
pub enum Op {
    Memory(Option<i64>),
    CpuWeight(Option<u32>),
    Tasks(Option<u32>),
    Disk(Option<i64>),
    EgressAdd(String),
    EgressRm(String),
    EgressMode(String),
    PortAdd(PortSpec),
    PortRm(PortSpec),
    RecycleEvery(Option<i64>),
    RecycleAt(Option<u8>),
    Isolation(String),
    Path(String),
    Risk(String),
    DbPrivate(bool),
    DbRaw(bool),
}

/// How a change sits against the App's approved set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Class {
    /// Takes something away (or changes nothing): never waits.
    Narrow,
    /// A budget change "Allow this agent to manage Apps" covers.
    Budget,
    /// An open-path feature: "…and publish open Apps" covers it.
    Open,
    /// A widening only the owner makes (agents propose).
    Owner,
    /// Always per event: held for a fresh owner confirm from every
    /// caller, the owner token included (§5.6, K25, AR14).
    PerEvent,
}

fn iso_rank(i: &str) -> u8 {
    match i {
        "vm" => 3,
        "container" => 2,
        "process" => 1,
        _ => 0,
    }
}

fn cmp_opt<T: PartialOrd>(new: &Option<T>, old: &Option<T>) -> Class {
    // None = no limit: raising to none is a budget change, lowering from
    // none to a number narrows.
    match (new, old) {
        (Some(n), Some(o)) if n <= o => Class::Narrow,
        (Some(_), None) => Class::Narrow,
        (None, None) => Class::Narrow,
        _ => Class::Budget,
    }
}

/// Classify one op against the current state.
pub fn classify(op: &Op, s: &State) -> Class {
    let c = &s.config;
    match op {
        Op::Memory(v) => cmp_opt(v, &s.memory_limit_bytes),
        Op::CpuWeight(v) => cmp_opt(v, &c.resources.cpu_weight),
        Op::Tasks(v) => cmp_opt(v, &c.resources.tasks),
        Op::Disk(v) => cmp_opt(v, &c.resources.disk_bytes),
        Op::RecycleEvery(_) | Op::RecycleAt(_) => Class::Narrow,
        Op::EgressAdd(h) => {
            if c.egress.hosts.contains(h) || (s.path == "open" && c.egress.mode == "broad") {
                // Already allowed (broad egress on the open path audits
                // first-seen hosts instead).
                Class::Narrow
            } else {
                Class::PerEvent
            }
        }
        Op::EgressRm(_) => Class::Narrow,
        Op::EgressMode(m) => {
            if m == "broad" && c.egress.mode != "broad" {
                Class::Open
            } else {
                Class::Narrow
            }
        }
        Op::PortAdd(p) => {
            if c.ports.contains(p) {
                Class::Narrow
            } else {
                Class::Open
            }
        }
        Op::PortRm(_) => Class::Narrow,
        Op::Isolation(i) => {
            let (new, old) = (iso_rank(i), iso_rank(&s.isolation));
            if new >= old {
                Class::Narrow
            } else if i == "process" {
                Class::PerEvent
            } else {
                Class::Owner
            }
        }
        Op::Path(p) => {
            if p == "open" && s.path != "open" {
                Class::Open
            } else {
                Class::Narrow
            }
        }
        Op::Risk(r) => {
            if r == "normal" && c.risk == "high" {
                Class::Owner
            } else {
                Class::Narrow
            }
        }
        Op::DbPrivate(on) => {
            if *on && !c.db.private {
                Class::Owner
            } else {
                Class::Narrow
            }
        }
        Op::DbRaw(on) => {
            if *on && !c.db.raw {
                Class::PerEvent
            } else {
                Class::Narrow
            }
        }
    }
}

/// A plain-words line for one op (owner approvals, CLI output).
pub fn describe(op: &Op, s: &State) -> String {
    let mib = |v: &Option<i64>| v.map(|b| format!("{} MiB", b >> 20)).unwrap_or_else(|| "no limit".into());
    match op {
        Op::Memory(v) => format!("memory limit {} → {}", mib(&s.memory_limit_bytes), mib(v)),
        Op::CpuWeight(v) => format!("CPU weight {:?} → {:?}", s.config.resources.cpu_weight, v),
        Op::Tasks(v) => format!("task limit {:?} → {:?}", s.config.resources.tasks, v),
        Op::Disk(v) => format!("disk budget {} → {}", mib(&s.config.resources.disk_bytes), mib(v)),
        Op::EgressAdd(h) => format!("allow outbound calls to {h}"),
        Op::EgressRm(h) => format!("stop outbound calls to {h}"),
        Op::EgressMode(m) => format!("egress mode {} → {m}", s.config.egress.mode),
        Op::PortAdd(p) => format!("open port {}/{}", p.port, p.proto),
        Op::PortRm(p) => format!("close port {}/{}", p.port, p.proto),
        Op::RecycleEvery(v) => format!("recycle every {v:?} s"),
        Op::RecycleAt(v) => format!("recycle at {v:?}% of memory"),
        Op::Isolation(i) => format!("isolation {} → {i}", s.isolation),
        Op::Path(p) => format!("path {} → {p}", s.path),
        Op::Risk(r) => format!("risk {} → {r}", s.config.risk),
        Op::DbPrivate(on) => format!("private database {}", if *on { "on" } else { "off" }),
        Op::DbRaw(on) => format!("raw private-database connection {}", if *on { "on" } else { "off" }),
    }
}

/// Apply one op to the state (no I/O).
pub fn apply(op: &Op, s: &mut State) {
    let c = &mut s.config;
    match op {
        Op::Memory(v) => s.memory_limit_bytes = *v,
        Op::CpuWeight(v) => c.resources.cpu_weight = *v,
        Op::Tasks(v) => c.resources.tasks = *v,
        Op::Disk(v) => c.resources.disk_bytes = *v,
        Op::EgressAdd(h) => {
            if !c.egress.hosts.contains(h) {
                c.egress.hosts.push(h.clone());
            }
        }
        Op::EgressRm(h) => c.egress.hosts.retain(|x| x != h),
        Op::EgressMode(m) => c.egress.mode = m.clone(),
        Op::PortAdd(p) => {
            if !c.ports.contains(p) {
                c.ports.push(p.clone());
            }
        }
        Op::PortRm(p) => c.ports.retain(|x| x != p),
        Op::RecycleEvery(v) => c.recycle.every_secs = *v,
        Op::RecycleAt(v) => c.recycle.at_pct = *v,
        Op::Isolation(i) => s.isolation = i.clone(),
        Op::Path(p) => s.path = p.clone(),
        Op::Risk(r) => c.risk = r.clone(),
        Op::DbPrivate(on) => c.db.private = *on,
        Op::DbRaw(on) => c.db.raw = *on,
    }
}

/// Validate a host for the egress allowlist: `host:port`, a DNS name or
/// IP, port 1–65535. Lower-cased.
pub fn parse_egress_host(raw: &str) -> Result<String, String> {
    let t = raw.trim().to_ascii_lowercase();
    let (host, port) = t.rsplit_once(':').ok_or_else(|| format!("egress host {raw:?}: use host:port (api.stripe.com:443)"))?;
    let port: u16 = port.parse().ok().filter(|p| *p > 0).ok_or_else(|| format!("egress host {raw:?}: bad port"))?;
    let ok = !host.is_empty()
        && host.len() <= 253
        && !host.contains('*')
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    if !ok {
        return Err(format!("egress host {raw:?}: a plain host name (no wildcards or schemes)"));
    }
    Ok(format!("{host}:{port}"))
}

/// `3000/tcp`, `5000/udp`, `8080` (tcp).
pub fn parse_port(raw: &str) -> Result<PortSpec, String> {
    let t = raw.trim().to_ascii_lowercase();
    let (p, proto) = t.split_once('/').unwrap_or((t.as_str(), "tcp"));
    let port: u16 = p.parse().ok().filter(|p| *p > 0).ok_or_else(|| format!("port {raw:?}: 1–65535"))?;
    if proto != "tcp" && proto != "udp" {
        return Err(format!("port {raw:?}: tcp or udp"));
    }
    Ok(PortSpec { port, proto: proto.to_string() })
}

/// Read the stored config (defaults when NULL or unreadable).
pub fn load(conn: &Connection, app_id: &str) -> rusqlite::Result<AppConfig> {
    let raw: Option<Option<String>> =
        conn.query_row("SELECT config_json FROM apps WHERE app_id = ?1", params![app_id], |r| r.get(0)).optional()?;
    Ok(raw.flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
}

/// Write the state back (memory, isolation, path and the JSON in one
/// statement).
pub fn store(conn: &Connection, app_id: &str, s: &State, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE apps SET memory_limit_bytes = ?2, isolation = ?3, path = ?4, config_json = ?5, updated_at = ?6 WHERE app_id = ?1",
        params![app_id, s.memory_limit_bytes, s.isolation, s.path, serde_json::to_string(&s.config).unwrap_or_default(), now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> State {
        State { memory_limit_bytes: Some(512 << 20), isolation: "container".into(), path: "guarded".into(), config: AppConfig::default() }
    }

    #[test]
    fn narrowing_never_waits_and_the_per_event_list_always_does() {
        let s = st();
        assert_eq!(classify(&Op::Memory(Some(256 << 20)), &s), Class::Narrow);
        assert_eq!(classify(&Op::Memory(None), &s), Class::Budget, "removing the limit raises it");
        assert_eq!(classify(&Op::EgressAdd("api.stripe.com:443".into()), &s), Class::PerEvent);
        assert_eq!(classify(&Op::EgressRm("api.stripe.com:443".into()), &s), Class::Narrow);
        assert_eq!(classify(&Op::EgressMode("broad".into()), &s), Class::Open);
        assert_eq!(classify(&Op::PortAdd(parse_port("5000/udp").unwrap()), &s), Class::Open);
        assert_eq!(classify(&Op::Path("open".into()), &s), Class::Open);
        assert_eq!(classify(&Op::Isolation("vm".into()), &s), Class::Narrow);
        assert_eq!(classify(&Op::Isolation("process".into()), &s), Class::PerEvent);
        assert_eq!(classify(&Op::DbRaw(true), &s), Class::PerEvent);
        assert_eq!(classify(&Op::DbPrivate(true), &s), Class::Owner);
        let mut vm = st();
        vm.isolation = "vm".into();
        assert_eq!(classify(&Op::Isolation("container".into()), &vm), Class::Owner);
        let mut high = st();
        high.config.risk = "high".into();
        assert_eq!(classify(&Op::Risk("normal".into()), &high), Class::Owner);
        assert_eq!(classify(&Op::Risk("high".into()), &s), Class::Narrow);
    }

    #[test]
    fn broad_open_apps_only_audit_new_hosts_and_apply_is_idempotent() {
        let mut s = st();
        apply(&Op::Path("open".into()), &mut s);
        apply(&Op::EgressMode("broad".into()), &mut s);
        assert_eq!(classify(&Op::EgressAdd("x.example.com:443".into()), &s), Class::Narrow);
        apply(&Op::EgressAdd("a.example.com:443".into()), &mut s);
        apply(&Op::EgressAdd("a.example.com:443".into()), &mut s);
        assert_eq!(s.config.egress.hosts, vec!["a.example.com:443"]);
        let ops = vec![Op::Memory(None), Op::PortAdd(parse_port("9000").unwrap())];
        let j = serde_json::to_string(&ops).unwrap();
        let back: Vec<Op> = serde_json::from_str(&j).unwrap();
        assert_eq!(back, ops, "ops round-trip through a held request");
    }

    #[test]
    fn hosts_and_ports_parse_strictly() {
        assert_eq!(parse_egress_host("API.Stripe.com:443").unwrap(), "api.stripe.com:443");
        assert!(parse_egress_host("api.stripe.com").is_err());
        assert!(parse_egress_host("*.stripe.com:443").is_err());
        assert!(parse_egress_host("https://x:443").is_err());
        assert_eq!(parse_port("8080").unwrap(), PortSpec { port: 8080, proto: "tcp".into() });
        assert!(parse_port("0").is_err() && parse_port("80/sctp").is_err());
    }
}
