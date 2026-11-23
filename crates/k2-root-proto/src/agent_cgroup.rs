//! Agent Isolation M1b (protocol 19, `namespaces.agentcg`): root-owned
//! per-agent cgroups, placed by the root helper.
//!
//! With M1 alone, `<unit>/agents/a-<workspace id>/` belongs to the K2 user
//! (the daemon makes it inside its delegated unit), so an *unfenced* agent,
//! being that user, can write its pid into another agent's `cgroup.procs`
//! and pose as it. With the helper, every agent cgroup is made by root and
//! its files stay root's: the K2 user can neither write into one nor rename
//! one (cgroup v2 refuses a rename), only remove an empty one (its parent,
//! `agents/`, is the user's) — and a cgroup the K2 user made again under
//! that name is not root's, so the daemon doesn't take it for the agent.
//!
//! `agents place <workspace id> <pid>` (daemon class): the helper finds the
//! daemon's unit from the caller (its main process, already proven), makes
//! `agents/a-<id>` root-owned if it isn't (a K2-user-owned one is emptied
//! into `agents/` and made again: chowning one would leave a writable fd
//! someone opened earlier usable), and moves `pid` into it. Only a process
//! of the daemon's user, still in the daemon's own unit, outside every other
//! agent's cgroup, and waiting in the exec helper (`argv[1] ==
//! --agent-exec`): never a path or a cgroup from the caller.

/// `agents/` under the daemon's unit (= k2-core `agents_leaf::AGENTS_LEAF`).
pub const AGENTS_LEAF: &str = "agents";
/// The daemon's own leaf (= k2-core `agents_leaf::DAEMON_LEAF`).
pub const DAEMON_LEAF: &str = "daemon";
/// Per-agent cgroup names (= k2-core `agent_cgroup::PREFIX`).
pub const PREFIX: &str = "a-";
/// The exec helper's argv[1] (= k2-core `agent_fence::ARG`).
pub const EXEC_HELPER_ARG: &str = "--agent-exec";

/// A workspace id that names an agent cgroup: `[A-Za-z0-9_-]{1,64}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWs(String);

impl AgentWs {
    pub fn parse(s: &str) -> Option<Self> {
        let ok = !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        ok.then(|| Self(s.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// `a-<id>`.
    pub fn cgroup_name(&self) -> String {
        format!("{PREFIX}{}", self.0)
    }
}

/// A pid the helper may be asked to place: 2..=4194304 (`pid_max`'s cap).
pub fn pid_ok(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 7 || !s.bytes().all(|b| b.is_ascii_digit()) || s.starts_with('0') {
        return None;
    }
    let n: u32 = s.parse().ok()?;
    (2..=4_194_304).contains(&n).then_some(n)
}

/// A start time (clock ticks since boot, `/proc/<pid>/stat` field 22):
/// digits only, no leading zero, at most 20.
pub fn start_ok(s: &str) -> Option<u64> {
    if s.is_empty() || s.len() > 20 || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0')) {
        return None;
    }
    s.parse().ok()
}

/// `stat` (the text of `/proc/<pid>/stat`) → the start time (field 22).
pub fn start_of_stat(stat: &str) -> Option<u64> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()
}

/// The daemon's unit cgroup from its own `/proc/<pid>/cgroup` path: the
/// path minus a trailing `/daemon` (DelegateSubgroup) leaf. `None` for the
/// root cgroup, a user-scope daemon, or anything that isn't a `.service`.
pub fn unit_of(daemon_cgroup: &str) -> Option<String> {
    let p = daemon_cgroup.trim_end_matches('/');
    let base = p.strip_suffix(&format!("/{DAEMON_LEAF}")).unwrap_or(p);
    let last = base.rsplit('/').next()?;
    (base.starts_with('/') && !base.contains("/user@") && last.ends_with(".service") && !base.contains("/../") && !base.contains("/./"))
        .then(|| base.to_string())
}

/// Where a process may be placed from: the daemon's unit (its `daemon/`
/// leaf or below, the shared `agents/` leaf) or already this agent's own
/// cgroup. Never another agent's, never outside the unit.
pub fn may_place_from(unit: &str, ws: &AgentWs, current: &str) -> bool {
    let cur = current.trim_end_matches('/');
    let Some(rest) = cur.strip_prefix(unit).and_then(|r| r.strip_prefix('/')) else { return false };
    let mut parts = rest.split('/');
    match (parts.next(), parts.next()) {
        (Some(DAEMON_LEAF), _) => true,
        (Some(AGENTS_LEAF), None) => true,
        (Some(AGENTS_LEAF), Some(name)) => name == ws.cgroup_name(),
        _ => false,
    }
}

/// The `argv[1]` of a `/proc/<pid>/cmdline` (NUL-separated).
pub fn argv1(cmdline: &[u8]) -> Option<&str> {
    std::str::from_utf8(cmdline.split(|b| *b == 0).nth(1)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT: &str = "/k2cores.slice/k2-daemon.service";

    /// Review B5: the start time the daemon passes, and the one the helper
    /// reads back from `/proc/<pid>/stat` (a comm with spaces and a `)`).
    #[test]
    fn start_times_parse_strictly() {
        assert_eq!(start_ok("123456"), Some(123456));
        assert_eq!(start_ok("0"), Some(0));
        for bad in ["", "01", "-1", "12a", "1 2", &"9".repeat(21)] {
            assert_eq!(start_ok(bad), None, "{bad:?}");
        }
        let stat = "4242 (k2-daemon) x) S 1 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 12345678 300";
        assert_eq!(start_of_stat(stat), Some(987654));
        assert_eq!(start_of_stat("4242 (cut"), None);
    }

    #[test]
    fn ids_and_pids() {
        assert_eq!(AgentWs::parse("3f1c-uuid_9").unwrap().cgroup_name(), "a-3f1c-uuid_9");
        for bad in ["", "../x", "a/b", "a b", "x.y", "a\0", &"z".repeat(65)] {
            assert!(AgentWs::parse(bad).is_none(), "{bad:?}");
        }
        assert_eq!(pid_ok("4242"), Some(4242));
        for bad in ["", "0", "1", "01", "-5", "+5", "9999999", "12a", "4194305"] {
            assert_eq!(pid_ok(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_unit_comes_from_the_daemons_own_cgroup() {
        assert_eq!(unit_of(&format!("{UNIT}/daemon")).as_deref(), Some(UNIT));
        assert_eq!(unit_of(UNIT).as_deref(), Some(UNIT));
        assert_eq!(unit_of("/system.slice/run-u5.service/daemon").as_deref(), Some("/system.slice/run-u5.service"));
        for bad in ["/", "", "/user.slice/user-1000.slice/user@1000.service/app.slice/k2.service", "/system.slice/x.scope", "relative.service"] {
            assert_eq!(unit_of(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_process_is_placed_only_from_the_daemons_unit_and_never_from_another_agent() {
        let ws = AgentWs::parse("A").unwrap();
        for ok in [format!("{UNIT}/daemon"), format!("{UNIT}/daemon/x"), format!("{UNIT}/agents"), format!("{UNIT}/agents/a-A")] {
            assert!(may_place_from(UNIT, &ws, &ok), "{ok}");
        }
        for bad in [
            format!("{UNIT}/agents/a-B"),
            format!("{UNIT}/agents/a-B/a-A"),
            UNIT.to_string(),
            "/user.slice/user-1000.slice/session-2.scope".to_string(),
            format!("{UNIT}x/daemon"),
            "/system.slice/other.service/daemon".to_string(),
        ] {
            assert!(!may_place_from(UNIT, &ws, &bad), "{bad}");
        }
        assert_eq!(argv1(b"/usr/bin/k2-daemon\0--agent-exec\0--\0/bin/sh\0"), Some(EXEC_HELPER_ARG));
        assert_eq!(argv1(b"/bin/sh\0-c\0x\0"), Some("-c"));
        assert_eq!(argv1(b"/bin/sh"), None);
    }
}
