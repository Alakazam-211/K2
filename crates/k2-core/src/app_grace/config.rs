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
//! In 0.46.0 the walls that enforce extra ports and the private database
//! aren't on yet (0.46.1); the values are stored and approved now so the
//! wall starts from the owner's decisions. Egress is not here: a guarded
//! App's host list and mode are `deploy::egress` (S15, enforced by its
//! proxy), and `/cli/app/config`'s egress fields go there.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// The fields without a column of their own (`apps.config_json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub resources: Resources,
    /// Extra ports beyond the one front port (`$PORT`). Any entry is an
    /// open-path feature.
    pub ports: Vec<PortSpec>,
    pub recycle: Recycle,
    /// `normal` | `high` (high → VM once stage V ships).
    pub risk: String,
    pub db: Db,
    /// The open path's in-wall port (0.46.1, K5): the port an old-style
    /// `k2 publish run --cmd --port N` service listens on inside its
    /// relaxed container, behind K2's host port. `None` = `$PORT`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_port: Option<u16>,
    /// Public labels this App took over from its own workspace's legacy
    /// `--cmd` services (`k2 app label take`, 0.46.3). The legacy service
    /// keeps running, unrouted; `k2 app label return` moves a label back.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<TakenLabel>,
}

/// A label an App serves for one of its workspace's legacy services.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakenLabel {
    pub label: String,
    /// The legacy service's `published_services.id`.
    pub service_id: String,
    /// The legacy service's port (where `return` points the label).
    pub legacy_port: u16,
    pub taken_at: i64,
    pub taken_by: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            resources: Resources::default(),
            ports: Vec::new(),
            recycle: Recycle::default(),
            risk: "normal".into(),
            db: Db::default(),
            app_port: None,
            labels: Vec::new(),
        }
    }
}

impl AppConfig {
    /// The App's extra TCP ports (UDP waits for 0.46.2, Rosson Q3).
    pub fn tcp_ports(&self) -> Vec<u16> {
        self.ports.iter().filter(|p| p.proto == "tcp").map(|p| p.port).collect()
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
    /// The disk ceilings column (`apps.io_max_json`, 0170): `None` = K2's
    /// default ceiling. The daemon compares ceilings (it owns the wall's
    /// numbers); here it is stored and applied.
    pub io_max_json: Option<String>,
}

/// One change. Serialized into held requests and applied on approve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", content = "value", rename_all = "camelCase")]
pub enum Op {
    Memory(Option<i64>),
    CpuWeight(Option<u32>),
    Tasks(Option<u32>),
    Disk(Option<i64>),
    PortAdd(PortSpec),
    PortRm(PortSpec),
    RecycleEvery(Option<i64>),
    RecycleAt(Option<u8>),
    Isolation(String),
    Path(String),
    Risk(String),
    DbPrivate(bool),
    DbRaw(bool),
    /// The disk ceilings (§6.2, runtime lane D): the new column value
    /// (`None` = K2's default). The daemon classifies it (tightening
    /// narrows; raising a ceiling is the owner's) and holds only a raise.
    Io(Option<String>),
}

/// How a change sits against the App's approved set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Class {
    /// Takes something away (or changes nothing): never waits.
    Narrow,
    /// A budget raise: humans and an agent with "…and manage them end to
    /// end" (switch 2) make it; switch 1 proposes it.
    Budget,
    /// An open-path feature: it loosens the wall, so the person confirms
    /// it from every caller (prd-agent-app-permissions-v1 §2.2; the old
    /// "…and publish open Apps" standing approval is retired).
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
        // The daemon compares ceilings before an Io op gets here; one
        // that reaches the classifier is treated as a raise.
        Op::Io(col) => {
            if *col == s.io_max_json {
                Class::Narrow
            } else {
                Class::Owner
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
        Op::PortAdd(p) => format!("open port {}/{}", p.port, p.proto),
        Op::PortRm(p) => format!("close port {}/{}", p.port, p.proto),
        Op::RecycleEvery(v) => format!("recycle every {v:?} s"),
        Op::RecycleAt(v) => format!("recycle at {v:?}% of memory"),
        Op::Isolation(i) => format!("isolation {} → {i}", s.isolation),
        Op::Path(p) => format!("path {} → {p}", s.path),
        Op::Risk(r) => format!("risk {} → {r}", s.config.risk),
        Op::DbPrivate(on) => format!("private database {}", if *on { "on" } else { "off" }),
        Op::DbRaw(on) => format!("raw private-database connection {}", if *on { "on" } else { "off" }),
        Op::Io(col) => format!(
            "disk ceilings {} → {}",
            s.io_max_json.as_deref().unwrap_or("K2's default"),
            col.as_deref().unwrap_or("K2's default")
        ),
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
        Op::Io(col) => s.io_max_json = col.clone(),
    }
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

/// Write the state back (memory, isolation, path, disk ceilings and the
/// JSON in one statement).
pub fn store(conn: &Connection, app_id: &str, s: &State, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE apps SET memory_limit_bytes = ?2, isolation = ?3, path = ?4, config_json = ?5, io_max_json = ?6, updated_at = ?7 \
         WHERE app_id = ?1",
        params![app_id, s.memory_limit_bytes, s.isolation, s.path, serde_json::to_string(&s.config).unwrap_or_default(), s.io_max_json, now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> State {
        State {
            memory_limit_bytes: Some(512 << 20),
            isolation: "container".into(),
            path: "guarded".into(),
            config: AppConfig::default(),
            io_max_json: None,
        }
    }

    #[test]
    fn narrowing_never_waits_and_the_per_event_list_always_does() {
        let s = st();
        assert_eq!(classify(&Op::Memory(Some(256 << 20)), &s), Class::Narrow);
        assert_eq!(classify(&Op::Memory(None), &s), Class::Budget, "removing the limit raises it");
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
    fn ports_apply_idempotently_and_ops_round_trip() {
        let mut s = st();
        apply(&Op::Path("open".into()), &mut s);
        assert_eq!(classify(&Op::PortAdd(parse_port("9000").unwrap()), &s), Class::Open);
        apply(&Op::PortAdd(parse_port("9000").unwrap()), &mut s);
        apply(&Op::PortAdd(parse_port("9000").unwrap()), &mut s);
        assert_eq!(s.config.ports, vec![parse_port("9000").unwrap()]);
        assert_eq!(classify(&Op::PortAdd(parse_port("9000").unwrap()), &s), Class::Narrow, "already open");
        // Disk ceilings: the same column narrows; any other is the owner's
        // (the daemon only lets a raise reach the classifier).
        assert_eq!(classify(&Op::Io(None), &s), Class::Narrow);
        assert_eq!(classify(&Op::Io(Some("{\"wbps\":1}".into())), &s), Class::Owner);
        apply(&Op::Io(Some("{\"wbps\":1}".into())), &mut s);
        assert_eq!(s.io_max_json.as_deref(), Some("{\"wbps\":1}"));
        let ops = vec![Op::Memory(None), Op::PortAdd(parse_port("9000").unwrap()), Op::Io(None)];
        let j = serde_json::to_string(&ops).unwrap();
        let back: Vec<Op> = serde_json::from_str(&j).unwrap();
        assert_eq!(back, ops, "ops round-trip through a held request");
    }

    #[test]
    fn ports_parse_strictly() {
        assert_eq!(parse_port("8080").unwrap(), PortSpec { port: 8080, proto: "tcp".into() });
        assert!(parse_port("0").is_err() && parse_port("80/sctp").is_err());
    }
}
