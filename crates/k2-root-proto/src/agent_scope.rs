//! Agent Isolation M2 B4 (protocol 19, `namespaces.agentscope`): one
//! root-owned systemd scope per agent session, outside the daemon's unit.
//!
//! The layout (design rev 3, with systemd's dash rule applied: a dash in a
//! slice name is a level, so the top slice has none):
//!
//! ```text
//! /sys/fs/cgroup/k2agents.slice/                       root, never delegated
//!   k2agents-anchor.service                            keeps the slice alive
//!   k2agents-open.slice/
//!     k2agents-open-anchor.service
//!     k2-agent-<ws>:<id>.scope                         one per session
//!   k2agents-fenced.slice/
//!     k2agents-fenced-anchor.service
//!     k2-agent-<ws>:<id>.scope
//! ```
//!
//! `<ws>` is an [`AgentWs`] (`[A-Za-z0-9_-]{1,64}`) and `<id>` a
//! [`ScopeId`] the daemon makes (lowercase hex, 16-32). Neither can hold a
//! `:`, so [`parse_scope_name`] splits exactly once on it and never matches
//! a name by a prefix such as `k2-agent-<ws>` alone (`abc` is not
//! `abc-def`).
//!
//! Everything here is pure: names, the classifier, the caps a scope may
//! carry (numbers only, never a property string), the unit files `agents
//! setup` writes, the `busctl` argv of `StartTransientUnit`, and the P1
//! probe's verdict. The helper side is `k2-root-helper/src/agents_scope.rs`.

pub use crate::agent_cgroup::AgentWs;
use crate::agent_cgroup::{pid_ok, start_ok, AGENTS_LEAF, DAEMON_LEAF};

/// The top slice (no dash: `k2-agents.slice` would sit under `k2.slice`).
pub const TOP_SLICE: &str = "k2agents.slice";
pub const OPEN_SLICE: &str = "k2agents-open.slice";
pub const FENCED_SLICE: &str = "k2agents-fenced.slice";
pub const TOP_ANCHOR: &str = "k2agents-anchor.service";
pub const OPEN_ANCHOR: &str = "k2agents-open-anchor.service";
pub const FENCED_ANCHOR: &str = "k2agents-fenced-anchor.service";
/// `k2-agent-<ws>:<id>.scope`.
pub const SCOPE_PREFIX: &str = "k2-agent-";
pub const SCOPE_SUFFIX: &str = ".scope";
/// The P1 probe's workspace part (`k2-agent-probe:<16 hex>.scope`): it is
/// shaped like an agent's scope on purpose, the name a K2-user process
/// would ask for.
pub const PROBE_WS: &str = "probe";
/// Where `agents setup` may find `sleep` for the anchors, first found wins
/// (a closed list: never a path from argv or the environment).
pub const SLEEP_PATHS: &[&str] = &["/usr/bin/sleep", "/bin/sleep"];
/// Every scope's `TimeoutStopUSec` (10 s): how long systemd waits after
/// SIGTERM before SIGKILL when a scope is stopped.
pub const STOP_TIMEOUT_USEC: u64 = 10_000_000;
/// A scope's `Description` at birth: fixed text, never the caller's. An
/// adopted scope says so, so `agents layout` can show the one way a
/// process reached a scope without `agents scope`'s checks on it.
pub const SCOPE_DESCRIPTION: &str = "K2 agent scope";
pub const ADOPTED_DESCRIPTION: &str = "K2 agent scope (adopted)";
/// Most pids one `agents adopt` moves (one old-layout cgroup).
pub const ADOPT_MAX_PIDS: usize = 4096;
/// `mem=` bounds, MB.
pub const MEM_MB: (u32, u32) = (16, 1_048_576);
/// `cpu=` bounds, percent of one CPU (25600 = 256 CPUs).
pub const CPU_PCT: (u32, u32) = (1, 25_600);

/// Which agent slice a scope is born in (fixed for its life).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Open,
    Fenced,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::Open, Kind::Fenced];
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Kind::Open),
            "fenced" => Some(Kind::Fenced),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Open => "open",
            Kind::Fenced => "fenced",
        }
    }
    pub fn slice(self) -> &'static str {
        match self {
            Kind::Open => OPEN_SLICE,
            Kind::Fenced => FENCED_SLICE,
        }
    }
    pub fn anchor(self) -> &'static str {
        match self {
            Kind::Open => OPEN_ANCHOR,
            Kind::Fenced => FENCED_ANCHOR,
        }
    }
}

/// The daemon's id for one scope: lowercase hex, 16-32 characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeId(String);

impl ScopeId {
    pub fn parse(s: &str) -> Option<Self> {
        let ok = (16..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        ok.then(|| Self(s.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `k2-agent-<ws>:<id>.scope`.
pub fn scope_name(ws: &AgentWs, id: &ScopeId) -> String {
    format!("{SCOPE_PREFIX}{}:{}{SCOPE_SUFFIX}", ws.as_str(), id.as_str())
}

/// The classifier: a unit or cgroup name → its workspace and scope id, or
/// `None` for anything that isn't exactly `k2-agent-<ws>:<id>.scope`.
pub fn parse_scope_name(name: &str) -> Option<(AgentWs, ScopeId)> {
    let mid = name.strip_prefix(SCOPE_PREFIX)?.strip_suffix(SCOPE_SUFFIX)?;
    let (ws, id) = mid.split_once(':')?;
    Some((AgentWs::parse(ws)?, ScopeId::parse(id)?))
}

/// `/k2agents.slice/<kind's slice>` (relative to the cgroup root).
pub fn slice_cgroup(kind: Kind) -> String {
    format!("/{TOP_SLICE}/{}", kind.slice())
}

/// `/k2agents.slice/<kind's slice>/k2-agent-<ws>:<id>.scope`.
pub fn scope_cgroup(kind: Kind, ws: &AgentWs, id: &ScopeId) -> String {
    format!("{}/{}", slice_cgroup(kind), scope_name(ws, id))
}

/// `k2-agent-probe:<hex>.scope` for the P1 probe; `None` unless `hex` is a
/// [`ScopeId`].
pub fn probe_scope_name(hex: &str) -> Option<String> {
    let id = ScopeId::parse(hex)?;
    Some(scope_name(&AgentWs::parse(PROBE_WS)?, &id))
}

/// Where `agents scope` takes a process from: the daemon's unit itself or
/// any cgroup below it, except another workspace's old-layout cgroup
/// (`<unit>/agents/a-<other>` and below). Never outside the unit, so never
/// from a scope.
pub fn may_scope_from(unit: &str, ws: &AgentWs, current: &str) -> bool {
    let cur = current.trim_end_matches('/');
    if cur.contains("/../") || cur.contains("/./") || cur.ends_with("/..") || cur.ends_with("/.") {
        return false;
    }
    if cur == unit {
        return true;
    }
    let Some(rest) = cur.strip_prefix(unit).and_then(|r| r.strip_prefix('/')) else { return false };
    let mut parts = rest.split('/');
    match (parts.next(), parts.next()) {
        (Some(AGENTS_LEAF), None) => true,
        (Some(AGENTS_LEAF), Some(name)) => name == ws.cgroup_name(),
        (Some(DAEMON_LEAF), _) => true,
        // Any other child of the unit is allowed too: where the process
        // sits is not what proves it is the daemon's new session. The
        // held pidfd, the start time, the daemon user's ruid and the exec
        // helper's argv carry that; this check only keeps out another
        // workspace's old cgroup and everything outside the unit.
        (Some(p), _) => !p.is_empty(),
        _ => false,
    }
}

/// A scope's caps: numbers only, mapped to properties by the helper.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Caps {
    pub mem_mb: Option<u32>,
    pub cpu_pct: Option<u32>,
}

fn digits_in(s: &str, (lo, hi): (u32, u32)) -> Option<u32> {
    if s.is_empty() || s.len() > 7 || s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = s.parse().ok()?;
    (lo..=hi).contains(&n).then_some(n)
}

impl Caps {
    /// `[mem=<MB>] [cpu=<percent>]`, in that order, each at most once:
    /// strict digits, no leading zero, no unit, inside [`MEM_MB`] /
    /// [`CPU_PCT`].
    pub fn parse(tokens: &[&str]) -> Option<Self> {
        let mut c = Caps::default();
        let mut it = tokens.iter().peekable();
        if let Some(v) = it.peek().and_then(|t| t.strip_prefix("mem=")) {
            c.mem_mb = Some(digits_in(v, MEM_MB)?);
            it.next();
        }
        if let Some(v) = it.peek().and_then(|t| t.strip_prefix("cpu=")) {
            c.cpu_pct = Some(digits_in(v, CPU_PCT)?);
            it.next();
        }
        it.next().is_none().then_some(c)
    }
    pub fn tokens(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(m) = self.mem_mb {
            v.push(format!("mem={m}"));
        }
        if let Some(c) = self.cpu_pct {
            v.push(format!("cpu={c}"));
        }
        v
    }
    /// `MemoryMax` in bytes.
    pub fn memory_max(&self) -> Option<u64> {
        self.mem_mb.map(|m| u64::from(m) * 1024 * 1024)
    }
    /// `CPUQuotaPerSecUSec`: percent × 10 000 µs (100% = one CPU).
    pub fn cpu_quota_usec(&self) -> Option<u64> {
        self.cpu_pct.map(|p| u64::from(p) * 10_000)
    }
}

/// The `busctl` argv (after the program) that asks systemd, as root, for
/// one transient scope: `StartTransientUnit(name, "fail", [Slice, PIDs,
/// Delegate=false, CollectMode=inactive-or-failed, TimeoutStopUSec=10 s,
/// Description (fixed: [`SCOPE_DESCRIPTION`], or [`ADOPTED_DESCRIPTION`] when
/// `adopted`), caps…], [])`. Every value is a validated part, a fixed
/// string or a number; nothing from the caller passes as a property.
pub fn start_transient_argv(name: &str, kind: Kind, pids: &[u32], caps: &Caps, adopted: bool) -> Vec<String> {
    let mut props: Vec<Vec<String>> = vec![
        vec!["Slice".into(), "s".into(), kind.slice().into()],
        {
            let mut p = vec!["PIDs".into(), "au".into(), pids.len().to_string()];
            p.extend(pids.iter().map(u32::to_string));
            p
        },
        vec!["Delegate".into(), "b".into(), "false".into()],
        vec!["CollectMode".into(), "s".into(), "inactive-or-failed".into()],
        vec!["TimeoutStopUSec".into(), "t".into(), STOP_TIMEOUT_USEC.to_string()],
        vec!["Description".into(), "s".into(), (if adopted { ADOPTED_DESCRIPTION } else { SCOPE_DESCRIPTION }).into()],
    ];
    if let Some(b) = caps.memory_max() {
        props.push(vec!["MemoryMax".into(), "t".into(), b.to_string()]);
    }
    if let Some(u) = caps.cpu_quota_usec() {
        props.push(vec!["CPUQuotaPerSecUSec".into(), "t".into(), u.to_string()]);
    }
    let mut a: Vec<String> = [
        "call",
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "StartTransientUnit",
        "ssa(sv)a(sa(sv))",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.push(name.into());
    a.push("fail".into());
    a.push(props.len().to_string());
    for p in props {
        a.extend(p);
    }
    a.push("0".into());
    a
}

/// What the P1 probe (a K2-user `systemd-run --system --scope` into the
/// open slice) showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// systemd (polkit) said no: what must happen.
    Refused,
    /// It ran: a K2-user process can make a unit in an agent slice.
    Allowed,
    /// It failed for another reason; proves nothing either way.
    Unknown,
}

impl Probe {
    pub fn as_str(self) -> &'static str {
        match self {
            Probe::Refused => "refused",
            Probe::Allowed => "ALLOWED",
            Probe::Unknown => "unknown",
        }
    }
}

/// The probe's exit status and stderr → its verdict.
pub fn classify_probe(status: i32, stderr: &str) -> Probe {
    if status == 0 {
        return Probe::Allowed;
    }
    let e = stderr.to_ascii_lowercase();
    // systemd's own words for a polkit "no" (AccessDenied,
    // InteractiveAuthorizationRequired); anything else proves nothing.
    let refused = ["access denied", "interactive authentication required", "not authorized"];
    if refused.iter().any(|w| e.contains(w)) {
        Probe::Refused
    } else {
        Probe::Unknown
    }
}

// ── unit files (`agents setup`) ─────────────────────────────────────────

/// One of the three slices (top, open, fenced).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Top,
    Agents(Kind),
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Top, Level::Agents(Kind::Open), Level::Agents(Kind::Fenced)];
    pub fn slice(self) -> &'static str {
        match self {
            Level::Top => TOP_SLICE,
            Level::Agents(k) => k.slice(),
        }
    }
    pub fn anchor(self) -> &'static str {
        match self {
            Level::Top => TOP_ANCHOR,
            Level::Agents(k) => k.anchor(),
        }
    }
    /// `/k2agents.slice` or `/k2agents.slice/<child>`.
    pub fn cgroup(self) -> String {
        match self {
            Level::Top => format!("/{TOP_SLICE}"),
            Level::Agents(k) => slice_cgroup(k),
        }
    }
    fn what(self) -> &'static str {
        match self {
            Level::Top => "K2 agents",
            Level::Agents(Kind::Open) => "K2 agents (open)",
            Level::Agents(Kind::Fenced) => "K2 agents (fenced)",
        }
    }
}

/// The slice unit file for `level`.
pub fn slice_unit(level: Level) -> String {
    format!(
        "# {what}: every agent session's scope sits in K2's agent slices, root's alone.\n\
         # Written by k2-root-helper `agents setup`.\n\
         [Unit]\n\
         Description={what}\n\
         Before=slices.target\n",
        what = level.what()
    )
}

/// The anchor for `level`: one idle process that keeps the slice's cgroup
/// (and so its id) alive while no agent runs. `sleep` is one of
/// [`SLEEP_PATHS`]; `None` for anything else.
pub fn anchor_unit(level: Level, sleep: &str) -> Option<String> {
    SLEEP_PATHS.contains(&sleep).then(|| {
        format!(
            "# Keeps {slice}'s cgroup alive while no agent runs.\n\
             # Written by k2-root-helper `agents setup`.\n\
             [Unit]\n\
             Description={what} anchor\n\
             \n\
             [Service]\n\
             Type=exec\n\
             Slice={slice}\n\
             User=nobody\n\
             ExecStart={sleep} infinity\n\
             Restart=always\n\
             RestartSec=5\n\
             NoNewPrivileges=yes\n\
             PrivateNetwork=yes\n\
             CapabilityBoundingSet=\n\
             TasksMax=4\n\
             \n\
             [Install]\n\
             WantedBy=multi-user.target\n",
            slice = level.slice(),
            what = level.what(),
        )
    })
}

/// Every file `agents setup` writes under `/etc/systemd/system`, in order.
pub fn unit_files(sleep: &str) -> Option<Vec<(&'static str, String)>> {
    let mut v = Vec::new();
    for l in Level::ALL {
        v.push((l.slice(), slice_unit(l)));
    }
    for l in Level::ALL {
        v.push((l.anchor(), anchor_unit(l, sleep)?));
    }
    Some(v)
}

// ── verbs (`agents …`, protocol 19) ─────────────────────────────────────

/// The `agents` verbs protocol 19 adds (`agents place` and `agents check`
/// are protocol 18's, in `crate::verbs::Verb`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeVerb {
    /// `agents scope <ws> <id> <pid> <start> <open|fenced> [mem=N] [cpu=N]`.
    Scope { ws: AgentWs, id: ScopeId, pid: u32, start: u64, kind: Kind, caps: Caps },
    /// `agents adopt <ws> <id> <open|fenced>`: every process left in the
    /// old layout's `<unit>/agents/a-<ws>` into one new scope.
    Adopt { ws: AgentWs, id: ScopeId, kind: Kind },
    /// `agents stop <ws> <id>`.
    Stop { ws: AgentWs, id: ScopeId },
    /// `agents stop-all`: every agent scope; slices and anchors stay.
    StopAll,
    /// `agents layout`: report only.
    Layout,
    /// `agents setup`: the slices and anchors.
    Setup,
    /// `agents teardown`: every scope stopped, anchors and files removed.
    Teardown,
}

impl ScopeVerb {
    /// The argv after `agents`.
    pub fn parse(rest: &[&str]) -> Option<Self> {
        Some(match rest {
            ["scope", ws, id, pid, start, kind, caps @ ..] => ScopeVerb::Scope {
                ws: AgentWs::parse(ws)?,
                id: ScopeId::parse(id)?,
                pid: pid_ok(pid)?,
                start: start_ok(start)?,
                kind: Kind::parse(kind)?,
                caps: Caps::parse(caps)?,
            },
            ["adopt", ws, id, kind] => ScopeVerb::Adopt { ws: AgentWs::parse(ws)?, id: ScopeId::parse(id)?, kind: Kind::parse(kind)? },
            ["stop", ws, id] => ScopeVerb::Stop { ws: AgentWs::parse(ws)?, id: ScopeId::parse(id)? },
            ["stop-all"] => ScopeVerb::StopAll,
            ["layout"] => ScopeVerb::Layout,
            ["setup"] => ScopeVerb::Setup,
            ["teardown"] => ScopeVerb::Teardown,
            _ => return None,
        })
    }

    /// The argv after `agents` (`parse(tail())` round-trips).
    pub fn tail(&self) -> Vec<String> {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        match self {
            ScopeVerb::Scope { ws, id, pid, start, kind, caps } => {
                let mut v = s(&["scope", ws.as_str(), id.as_str(), &pid.to_string(), &start.to_string(), kind.as_str()]);
                v.extend(caps.tokens());
                v
            }
            ScopeVerb::Adopt { ws, id, kind } => s(&["adopt", ws.as_str(), id.as_str(), kind.as_str()]),
            ScopeVerb::Stop { ws, id } => s(&["stop", ws.as_str(), id.as_str()]),
            ScopeVerb::StopAll => s(&["stop-all"]),
            ScopeVerb::Layout => s(&["layout"]),
            ScopeVerb::Setup => s(&["setup"]),
            ScopeVerb::Teardown => s(&["teardown"]),
        }
    }

    /// All the daemon's alone. `setup` loosens nothing, but it switches the
    /// box to scope placement at the next daemon start, which is the
    /// owner's choice: the daemon runs it only on an owner action (review).
    /// The rest move, stop or report on agents' processes.
    pub fn class(&self) -> crate::verbs::Class {
        crate::verbs::Class::Daemon
    }

    /// `layout` changes nothing (its probe is refused or reported), so it
    /// takes no lock.
    pub fn reads_only(&self) -> bool {
        matches!(self, ScopeVerb::Layout)
    }

    /// One of each (tests, docs).
    pub fn catalog() -> Vec<Self> {
        let ws = AgentWs::parse("ws-1").expect("ws");
        let id = ScopeId::parse("0123456789abcdef").expect("id");
        vec![
            ScopeVerb::Scope { ws: ws.clone(), id: id.clone(), pid: 4242, start: 123456, kind: Kind::Open, caps: Caps::default() },
            ScopeVerb::Scope {
                ws: ws.clone(),
                id: id.clone(),
                pid: 4242,
                start: 123456,
                kind: Kind::Fenced,
                caps: Caps { mem_mb: Some(512), cpu_pct: Some(150) },
            },
            ScopeVerb::Scope { ws: ws.clone(), id: id.clone(), pid: 4242, start: 1, kind: Kind::Open, caps: Caps { mem_mb: None, cpu_pct: Some(50) } },
            ScopeVerb::Adopt { ws: ws.clone(), id: id.clone(), kind: Kind::Fenced },
            ScopeVerb::Stop { ws, id },
            ScopeVerb::StopAll,
            ScopeVerb::Layout,
            ScopeVerb::Setup,
            ScopeVerb::Teardown,
        ]
    }
}

#[cfg(test)]
#[path = "agent_scope_tests.rs"]
mod tests;
