//! Agent Isolation M2, B4 (protocol 19, `namespaces.agentscope`): each
//! agent session, job run and published service in its own root-owned
//! systemd scope, `k2-agent-<ws>:<id>.scope`, under `k2agents.slice/
//! k2agents-<open|fenced>.slice/`, made by the root helper with
//! `StartTransientUnit` (`k2_root_proto::agent_scope`). The scopes are
//! outside the daemon's unit, so no process of the K2 user can write a pid
//! into one (the common ancestor is the root cgroup), and the slice of a
//! fenced session is fixed at its birth.
//!
//! This module is the daemon side: whether the helper places scopes on this
//! box ([`scope_placement`]), the plan one session gets ([`ScopePlan`]),
//! the classifier for a peer's cgroup ([`classify_scope`]), and the calls
//! ([`place`], [`stop`], [`stop_for_pid`]). With no scope placer the M1b
//! layout (`<unit>/agents/a-<ws>`) applies as before.

use std::sync::OnceLock;

// The names, the same as `k2_root_proto::agent_scope` (k2-core doesn't
// depend on it at run time; the tests below check every one against it).
pub const TOP_SLICE: &str = "k2agents.slice";
const SCOPE_PREFIX: &str = "k2-agent-";
const SCOPE_SUFFIX: &str = ".scope";
const MEM_MB: (u32, u32) = (16, 1_048_576);
const CPU_PCT: (u32, u32) = (1, 25_600);

/// Which agent slice a scope is born in (fixed for its life).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Open,
    Fenced,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Open => "open",
            Kind::Fenced => "fenced",
        }
    }
    pub fn slice(self) -> &'static str {
        match self {
            Kind::Open => "k2agents-open.slice",
            Kind::Fenced => "k2agents-fenced.slice",
        }
    }
}

fn ws_ok(s: &str) -> bool {
    crate::agent_cgroup::valid_ws(s)
}

fn id_ok(s: &str) -> bool {
    (16..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `k2-agent-<ws>:<id>.scope`, or `None` when either can't name one.
fn scope_name(ws: &str, id: &str) -> Option<String> {
    (ws_ok(ws) && id_ok(id)).then(|| format!("{SCOPE_PREFIX}{ws}:{id}{SCOPE_SUFFIX}"))
}

/// The classifier on a unit name: exactly `k2-agent-<ws>:<id>.scope`.
fn parse_scope_name(name: &str) -> Option<(&str, &str)> {
    let mid = name.strip_prefix(SCOPE_PREFIX)?.strip_suffix(SCOPE_SUFFIX)?;
    let (ws, id) = mid.split_once(':')?;
    (ws_ok(ws) && id_ok(id)).then_some((ws, id))
}

/// What one isolated process gets in scope mode: a fresh scope id, its
/// slice (fenced or open, fixed at birth), and a dev server's caps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopePlan {
    /// Lowercase hex, 16 characters, new for every spawn.
    pub id: String,
    pub fenced: bool,
    /// A dev server's caps, as bounded integers (the helper maps them to
    /// `MemoryMax` and `CPUQuotaPerSecUSec`; never a property string).
    pub mem_mb: Option<u32>,
    pub cpu_pct: Option<u32>,
}

impl ScopePlan {
    pub fn new(fenced: bool) -> Result<Self, String> {
        let mut b = [0u8; 8];
        getrandom::getrandom(&mut b).map_err(|e| format!("no random scope id: {e}"))?;
        Ok(ScopePlan { id: b.iter().map(|x| format!("{x:02x}")).collect(), fenced, mem_mb: None, cpu_pct: None })
    }
    pub fn kind(&self) -> Kind {
        if self.fenced {
            Kind::Fenced
        } else {
            Kind::Open
        }
    }
    /// A dev server's caps (`publish_dev`: MB and CPUs), clamped into the
    /// helper's bounds. A CPU count that isn't a finite number is refused.
    pub fn with_caps(mut self, mem_mb: i64, cpus: f64) -> Result<Self, String> {
        let (mlo, mhi) = MEM_MB;
        let (clo, chi) = CPU_PCT;
        let pct = (cpus * 100.0).round();
        if !pct.is_finite() {
            return Err(format!("a dev server's CPU cap of {cpus} isn't a number of CPUs"));
        }
        self.mem_mb = Some(mem_mb.clamp(mlo as i64, mhi as i64) as u32);
        self.cpu_pct = Some((pct as i64).clamp(clo as i64, chi as i64) as u32);
        Ok(self)
    }
    /// `k2-agent-<ws>:<id>.scope`.
    pub fn unit(&self, ws: &str) -> Option<String> {
        scope_name(ws, &self.id)
    }
    /// `/k2agents.slice/k2agents-<kind>.slice/k2-agent-<ws>:<id>.scope`.
    pub fn cgroup(&self, ws: &str) -> Option<String> {
        Some(format!("/{TOP_SLICE}/{}/{}", self.kind().slice(), self.unit(ws)?))
    }
    /// The helper's argv for this plan.
    pub fn argv(&self, ws: &str, pid: u32, start: u64) -> Vec<String> {
        let mut v = vec!["agents".into(), "scope".into(), ws.into(), self.id.clone(), pid.to_string(), start.to_string(), self.kind().as_str().into()];
        if let Some(m) = self.mem_mb {
            v.push(format!("mem={m}"));
        }
        if let Some(c) = self.cpu_pct {
            v.push(format!("cpu={c}"));
        }
        v
    }
}

/// A peer's cgroup path that is an agent scope (or below one): its
/// workspace, scope id, kind, and the scope's own cgroup. `None` for any
/// other path: the classifier never matches on a prefix.
pub fn classify_scope(path: &str) -> Option<(String, String, Kind, String)> {
    let mut parts = path.trim_end_matches('/').strip_prefix('/')?.split('/');
    if parts.next()? != TOP_SLICE {
        return None;
    }
    let slice = parts.next()?;
    let kind = [Kind::Open, Kind::Fenced].into_iter().find(|k| k.slice() == slice)?;
    let unit = parts.next()?;
    let (ws, id) = parse_scope_name(unit)?;
    Some((ws.to_string(), id.to_string(), kind, format!("/{TOP_SLICE}/{slice}/{unit}")))
}

/// Calls the root helper (`agents …`, daemon class) and returns its JSON.
pub type Caller = Box<dyn Fn(&[String]) -> Result<serde_json::Value, String> + Send + Sync>;

static CALLER: OnceLock<Caller> = OnceLock::new();

/// Boot: the root helper places agent scopes on this box (protocol 19,
/// `namespaces.agentscope`, and `agents layout` ready).
pub fn set_caller(c: Caller) {
    let _ = CALLER.set(c);
}

/// The helper places every isolated process in its own scope here.
pub fn scope_placement() -> bool {
    CALLER.get().is_some()
}

/// What `/boot-status` and the doctor say about scopes on this box (R4):
/// why scope placement is off, the P1 answer, and the agents whose
/// sessions are still in the old layout (not adopted: they keep running
/// where they are and are placed in a scope at their next start).
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxState {
    /// `None` while scope placement is on.
    pub why_off: Option<String>,
    /// `agents layout`'s P1 answer (`refused`, `ALLOWED`, `unknown`).
    pub p1: Option<String>,
    /// Workspace ids with sessions in the old layout, and why each wasn't
    /// adopted.
    pub old_layout: Vec<(String, String)>,
    /// `kernel.pid_max` (`agents layout`): the doctor warns at the old 32768.
    pub pid_max: Option<u64>,
}

static BOX_STATE: std::sync::Mutex<Option<BoxState>> = std::sync::Mutex::new(None);

/// Boot: record the box's scope state for `/boot-status` and the doctor.
pub fn set_box_state(st: BoxState) {
    *BOX_STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(st);
}

pub fn box_state() -> Option<BoxState> {
    BOX_STATE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The text the helper sets as an adopted scope's `Description`.
pub const ADOPTED_DESCRIPTION: &str = "K2 agent scope (adopted)";

/// Whether the scope `k2-agent-<ws>:<id>.scope` was made by `agents adopt`
/// (its `Description`, read from systemd; `false` on any doubt).
pub fn is_adopted(ws: &str, id: &str) -> bool {
    let Some(unit) = scope_name(ws, id) else { return false };
    crate::bg_proc::command("systemctl")
        .args(["show", "-p", "Description", "--value", &unit])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == ADOPTED_DESCRIPTION)
}

/// While scope placement is on, a bound session is recognised only from a
/// scope, except for an agent whose sessions are still listed in the old
/// layout (survivors of an update, until they end).
pub fn old_layout_allowed(ws: &str) -> bool {
    box_state().is_some_and(|b| b.old_layout.iter().any(|(w, _)| w == ws))
}

/// Place `pid` (an exec helper waiting, started at `start`) in its scope.
/// `Ok(the scope's cgroup)`.
pub fn place(ws: &str, plan: &ScopePlan, pid: u32, start: u64) -> Result<String, String> {
    let call = CALLER.get().ok_or("no root helper places agent scopes on this box")?;
    let v = call(&plan.argv(ws, pid, start))?;
    let want = plan.cgroup(ws).ok_or_else(|| format!("workspace id {ws:?} or scope id {:?} can't name a scope", plan.id))?;
    match v["cgroup"].as_str() {
        Some(cg) if cg == want => Ok(want),
        other => Err(format!("agents scope: the helper answered {other:?}, not {want}")),
    }
}

/// End one scope (C4): idle stop, `publish stop`, a job's end. Already
/// gone is fine.
pub fn stop(ws: &str, id: &str) -> Result<(), String> {
    let call = CALLER.get().ok_or("no root helper places agent scopes on this box")?;
    if scope_name(ws, id).is_none() {
        return Err(format!("{ws:?}/{id:?} doesn't name an agent scope"));
    }
    call(&["agents".into(), "stop".into(), ws.into(), id.into()]).map(|_| ())
}

/// The scope `pid` runs in, if it is an agent scope: read before its
/// process is ended, so [`stop`] can end what it left behind.
pub fn scope_of_pid(pid: u32) -> Option<(String, String)> {
    let path = crate::agent_cgroup::cgroup_of_pid(pid).ok()?;
    classify_scope(&path).map(|(ws, id, _, _)| (ws, id))
}

/// After a service or job process is ended: stop its scope too, so a child
/// that left its process group doesn't stay (a root-owned scope's
/// `cgroup.kill` is root's). Nothing outside scope mode.
pub fn stop_for(scope: Option<(String, String)>) {
    if let Some((ws, id)) = scope.filter(|_| scope_placement()) {
        if let Err(e) = stop(&ws, &id) {
            crate::log_debug!("[agents] stop scope of agent {ws} ({id}): {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WS: &str = "3f1c2d4e-0000-4000-8000-00000000abcd";

    /// The copies above match the root helper's own names and rules.
    #[test]
    fn the_names_match_the_root_helpers() {
        use k2_root_proto::agent_scope as rp;
        assert_eq!(TOP_SLICE, rp::TOP_SLICE);
        assert_eq!((SCOPE_PREFIX, SCOPE_SUFFIX, MEM_MB, CPU_PCT), (rp::SCOPE_PREFIX, rp::SCOPE_SUFFIX, rp::MEM_MB, rp::CPU_PCT));
        for (k, r) in [(Kind::Open, rp::Kind::Open), (Kind::Fenced, rp::Kind::Fenced)] {
            assert_eq!((k.as_str(), k.slice()), (r.as_str(), r.slice()));
        }
        let ws = rp::AgentWs::parse(WS).unwrap();
        let id = rp::ScopeId::parse("0123456789abcdef").unwrap();
        assert_eq!(scope_name(WS, "0123456789abcdef"), Some(rp::scope_name(&ws, &id)));
        let p = ScopePlan { id: "0123456789abcdef".into(), fenced: true, mem_mb: Some(512), cpu_pct: Some(150) };
        assert_eq!(p.cgroup(WS), Some(rp::scope_cgroup(rp::Kind::Fenced, &ws, &id)));
        let argv = p.argv(WS, 4242, 99);
        let tail: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
        assert!(rp::ScopeVerb::parse(&tail).is_some(), "the helper parses the daemon's argv: {argv:?}");
        for name in [format!("k2-agent-{WS}:0123456789abcdef.scope"), "k2-agent-a:b.scope".into(), format!("k2-agent-{WS}-0123456789abcdef.scope")] {
            assert_eq!(parse_scope_name(&name).is_some(), rp::parse_scope_name(&name).is_some(), "{name}");
        }
    }

    #[test]
    fn a_plan_names_its_scope_and_slice() {
        let p = ScopePlan { id: "0123456789abcdef".into(), fenced: true, mem_mb: None, cpu_pct: None };
        assert_eq!(p.unit(WS).as_deref(), Some(format!("k2-agent-{WS}:0123456789abcdef.scope").as_str()));
        assert_eq!(p.cgroup(WS).as_deref(), Some(format!("/k2agents.slice/k2agents-fenced.slice/k2-agent-{WS}:0123456789abcdef.scope").as_str()));
        assert_eq!(p.argv(WS, 4242, 99), ["agents", "scope", WS, "0123456789abcdef", "4242", "99", "fenced"]);
        let fresh = ScopePlan::new(false).unwrap();
        assert_eq!(fresh.id.len(), 16, "{}", fresh.id);
        assert_ne!(fresh.id, ScopePlan::new(false).unwrap().id);
        assert!(p.unit("a/b").is_none());
    }

    #[test]
    fn dev_caps_are_bounded_integers() {
        let p = ScopePlan::new(false).unwrap().with_caps(512, 1.5).unwrap();
        assert_eq!((p.mem_mb, p.cpu_pct), (Some(512), Some(150)));
        assert!(p.argv(WS, 2, 3).ends_with(&["mem=512".to_string(), "cpu=150".to_string()]));
        let p = ScopePlan::new(false).unwrap().with_caps(1, 0.001).unwrap();
        assert_eq!((p.mem_mb, p.cpu_pct), (Some(16), Some(1)), "clamped up into the helper's bounds");
        let p = ScopePlan::new(false).unwrap().with_caps(i64::MAX, 1e9).unwrap();
        assert_eq!((p.mem_mb, p.cpu_pct), (Some(1_048_576), Some(25_600)));
        for bad in [f64::NAN, f64::INFINITY] {
            assert!(ScopePlan::new(false).unwrap().with_caps(512, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_classifier_reads_only_an_exact_scope() {
        let id = "0123456789abcdef";
        let base = format!("/k2agents.slice/k2agents-open.slice/k2-agent-{WS}:{id}.scope");
        assert_eq!(classify_scope(&base), Some((WS.into(), id.into(), Kind::Open, base.clone())));
        assert_eq!(classify_scope(&format!("{base}/sub")).map(|c| c.3), Some(base.clone()), "below a scope: that scope");
        for other in [
            "/k2agents.slice".to_string(),
            "/k2agents.slice/k2agents-open.slice".to_string(),
            "/k2agents.slice/k2agents-open.slice/k2agents-open-anchor.service".to_string(),
            format!("/k2agents.slice/k2agents-other.slice/k2-agent-{WS}:{id}.scope"),
            format!("/system.slice/k2-agent-{WS}:{id}.scope"),
            format!("/k2agents.slice/k2agents-open.slice/k2-agent-{WS}-{id}.scope"),
            format!("/k2agents.slice/k2agents-open.slice/k2-agent-{WS}:{id}.scope.x"),
            format!("/k2agents.slice/k2agents-open.slice/k2-agent-{WS}:XYZ.scope"),
            "/k2cores.slice/k2-daemon.service/agents/a-x".to_string(),
        ] {
            assert_eq!(classify_scope(&other), None, "{other}");
        }
        // One workspace id that starts another's: each reads as itself.
        let short = format!("/k2agents.slice/k2agents-fenced.slice/k2-agent-abc:{id}.scope");
        let long = format!("/k2agents.slice/k2agents-fenced.slice/k2-agent-abc-def:{id}.scope");
        assert_eq!(classify_scope(&short).map(|c| c.0), Some("abc".into()));
        assert_eq!(classify_scope(&long).map(|c| c.0), Some("abc-def".into()));
    }
}
