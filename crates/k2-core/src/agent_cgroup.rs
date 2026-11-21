//! Agent Isolation M1: every agent's terminal sessions run in the agent's
//! own cgroup, `<unit>/agents/a-<workspace-id>/`, a child of the agents'
//! leaf ([`crate::agents_leaf`]), and the daemon recognises a caller on a
//! session socket by the cgroup its process is in
//! (`plan-agent-isolation-build-v1` §1, §3; owner decisions 2026-11-21).
//!
//! **What it gives.** Every terminal, `k2 job` run and published service of
//! an agent is labelled by agent: its processes start in the agent's cgroup
//! (`systemd-cgls`, `ps -o cgroup`, `k2 agent whois <pid>`), and a fenced
//! agent can't change its label. A copied terminal or job passport alone no
//! longer works: the daemon accepts it on its session socket only from a
//! process in that agent's cgroup, and over loopback TCP only as that
//! socket's own forward (no peer process there otherwise). Passports that
//! are never bound (App exec, microVM cells, sessions from before 0.46.17)
//! work as before.
//!
//! **What it doesn't give (said everywhere it shows).** The cgroup files
//! belong to the K2 user, so an *unfenced* agent (the same user) can write
//! its pid into another agent's cgroup and pose as it, and it can read the
//! owner token on disk and call as the owner over TCP. Only the folder
//! fence (Landlock, [`crate::agent_fence`]) closes those for one agent;
//! root-owned per-agent cgroups placed by the root helper close the first
//! for all (plan M1b). So the claim is "labelled by agent; a fenced agent
//! can't change its label", never "cannot be forged".
//!
//! **Where it applies.** Linux, a system unit the daemon's box prep
//! delegated, with the restart guard (the agents' leaf is on). Anywhere
//! else (macOS, a user-unit install, an unguarded unit, a container) the
//! identity is *unavailable*: passports are accepted as before, and
//! `/boot-status` and `k2 app doctor` say so and why.
//!
//! **The emergency off-switch** (keep the old path until proven):
//! `K2_AGENT_CGROUP_IDENTITY=0` in the daemon's environment. Logged loudly
//! at boot, shown in `/boot-status` and the doctor, and listed in the
//! retirement ledger. Not a setting, not per agent.
//!
//! **Restart survival.** Nothing here lives only in memory: the cgroup's
//! name carries the workspace id, and which sessions are bound to their
//! cgroup is in the passport store (`hook-sessions.json`). k2-ptyd's
//! sessions outlive a daemon restart in their cgroups, and the next daemon
//! reads both back.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where cgroup v2 is mounted.
pub const CGROUP_FS: &str = "/sys/fs/cgroup";
/// The emergency off-switch (daemon environment).
pub const ENV_SWITCH: &str = "K2_AGENT_CGROUP_IDENTITY";
/// Per-agent cgroup names: `a-<workspace id>`.
pub const PREFIX: &str = "a-";
/// In an identity session's environment: its cgroup's name. The `k2` CLI
/// sends every passport request over the session socket when it is set.
pub const ENV_SESSION_MARK: &str = "K2_AGENT_CGROUP";

/// What the claim is, in the words every surface uses.
pub const CLAIM: &str = "labelled by agent: each agent's terminals, job runs and published services run in its own cgroup, \
     and a fenced agent can't change its label. A copied terminal or job passport alone no longer works (App exec and \
     microVM cell passports aren't bound and work as before). An unfenced agent (same user) can still move itself into \
     another agent's cgroup, or read the owner token on disk; the folder fence closes both for one agent";

/// `true` when the switch says off (`0`, `false`, `off`, `no`).
pub fn switch_off_from(v: Option<&str>) -> bool {
    matches!(v.map(|s| s.trim().to_ascii_lowercase()).as_deref(), Some("0" | "false" | "off" | "no"))
}

pub fn switch_off() -> bool {
    switch_off_from(std::env::var(ENV_SWITCH).ok().as_deref())
}

/// A workspace id that can name a cgroup (and a path component).
pub fn valid_ws(ws: &str) -> bool {
    !ws.is_empty() && ws.len() <= 64 && ws.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// `a-<ws>`, or `None` for an id that can't name one.
pub fn cgroup_name(ws: &str) -> Option<String> {
    valid_ws(ws.trim()).then(|| format!("{PREFIX}{}", ws.trim()))
}

/// Who a process is, by its cgroup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerAgent {
    /// In `agents/a-<ws>` (or below it).
    Agent { ws: String, cgroup: String },
    /// In the daemon unit's agents' leaf itself (a session from before
    /// this release, or one placed without identity).
    Leaf { cgroup: String },
    /// Anywhere else: the daemon's own leaf, a login shell, another unit.
    Outside { cgroup: String },
    /// Its cgroup couldn't be read (it exited, or no cgroup v2).
    Unknown { why: String },
}

impl PeerAgent {
    pub fn ws(&self) -> Option<&str> {
        match self {
            PeerAgent::Agent { ws, .. } => Some(ws),
            _ => None,
        }
    }
    /// One line for audit rows and refusals.
    pub fn describe(&self) -> String {
        match self {
            PeerAgent::Agent { ws, cgroup } => format!("agent {ws} (cgroup {cgroup})"),
            PeerAgent::Leaf { cgroup } => format!("no agent cgroup (the shared agents' leaf {cgroup})"),
            PeerAgent::Outside { cgroup } => format!("no agent cgroup ({cgroup})"),
            PeerAgent::Unknown { why } => format!("an unknown cgroup ({why})"),
        }
    }
}

/// Classify a cgroup v2 path (`/proc/<pid>/cgroup`'s `0::` value) against
/// the agents' leaf (`agents_rel`, e.g. `/k2cores.slice/k2-daemon.service/agents`).
pub fn classify(agents_rel: &str, path: &str) -> PeerAgent {
    let leaf = agents_rel.trim_end_matches('/');
    let p = path.trim_end_matches('/');
    let rest = if p == leaf {
        Some("")
    } else {
        p.strip_prefix(leaf).and_then(|r| r.strip_prefix('/'))
    };
    match rest {
        None => PeerAgent::Outside { cgroup: p.to_string() },
        Some("") => PeerAgent::Leaf { cgroup: p.to_string() },
        Some(r) => {
            let first = r.split('/').next().unwrap_or("");
            match first.strip_prefix(PREFIX).filter(|ws| valid_ws(ws)) {
                Some(ws) => PeerAgent::Agent { ws: ws.to_string(), cgroup: format!("{leaf}/{first}") },
                None => PeerAgent::Outside { cgroup: p.to_string() },
            }
        }
    }
}

/// The daemon's decision for a passport presented on a session socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Refuse { detail: String },
}

/// `bound`: the agent (workspace id) the passport's session is bound to,
/// `None` for a session placed without identity (accepted as before).
/// `peer`: who the connecting process is, by its cgroup.
pub fn decide(bound: Option<&str>, peer: &PeerAgent) -> Decision {
    let Some(want) = bound else { return Decision::Allow };
    match peer {
        PeerAgent::Agent { ws, .. } if ws == want => Decision::Allow,
        other => Decision::Refuse {
            detail: format!("the passport of agent {want} was presented from {}", other.describe()),
        },
    }
}

// ── the box: is identity available here? ─────────────────────────────

/// Where per-agent cgroups go on this box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Avail {
    /// `/sys/fs/cgroup/<unit>/agents`.
    pub agents_dir: PathBuf,
    /// The same, relative to the cgroup root (`/<unit>/agents`).
    pub agents_rel: String,
}

/// Identity on this box, or why not ("identity unavailable").
pub fn availability() -> Result<Avail, String> {
    if !cfg!(target_os = "linux") {
        return Err("Linux only (macOS has no cgroups; the Mac fence is Seatbelt, without a kernel identity)".into());
    }
    if switch_off() {
        return Err(format!("{ENV_SWITCH}=0 in the daemon's environment (the emergency off-switch)"));
    }
    let l = crate::agents_leaf::leaves()?;
    let agents_dir = l.base.join(crate::agents_leaf::AGENTS_LEAF);
    let agents_rel = agents_dir
        .strip_prefix(CGROUP_FS)
        .map(|r| format!("/{}", r.to_string_lossy()))
        .map_err(|_| format!("{} isn't under {CGROUP_FS}", agents_dir.display()))?;
    Ok(Avail { agents_dir, agents_rel })
}

/// The agent's cgroup directory (made if missing).
pub fn ensure_dir(a: &Avail, ws: &str) -> Result<PathBuf, String> {
    let name = cgroup_name(ws).ok_or_else(|| format!("workspace id {ws:?} can't name a cgroup"))?;
    let dir = a.agents_dir.join(name);
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("create {}: {e}", dir.display())),
    }
    Ok(dir)
}

/// The agent's private temp dir, `~/.k2/agents/<ws>/tmp` (0700).
pub fn tmp_dir(ws: &str) -> Result<PathBuf, String> {
    if !valid_ws(ws) {
        return Err(format!("workspace id {ws:?} can't name a folder"));
    }
    let dir = crate::paths::k2_home().join("agents").join(ws).join("tmp");
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// The executable the daemon starts isolated sessions through
/// (`<it> --agent-exec -- …`): the daemon's own binary, set at boot.
static EXEC_HELPER: OnceLock<PathBuf> = OnceLock::new();

pub fn set_exec_helper(p: PathBuf) {
    let _ = EXEC_HELPER.set(p);
}

pub fn exec_helper() -> Option<&'static Path> {
    EXEC_HELPER.get().map(PathBuf::as_path)
}

// ── one session's isolation ──────────────────────────────────────────

/// What one terminal session gets, decided by the daemon's spawn path
/// (identity: the box; fence: the agent's owner) and carried out by the
/// terminal backend at spawn ([`wrap`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIsolation {
    /// The agent (workspace id).
    pub ws: String,
    /// The session runs in `agents/a-<ws>` and its passport is bound to it.
    pub identity: bool,
    /// The owner turned this agent's folder fence on.
    pub fence: bool,
    /// The agent's workspace folder (the fence opens it).
    pub workspace: Option<PathBuf>,
}

/// The isolation for a session of agent `ws`: identity when the box has
/// it and the session is a plain host terminal (`passthrough`: a microVM
/// cell has its own boundary), the fence when `fence_on`. `None` when
/// neither applies (today's behaviour).
pub fn intent(ws: &str, passthrough: bool, fence_on: bool, workspace: Option<&Path>) -> Option<SessionIsolation> {
    if !passthrough || !valid_ws(ws.trim()) {
        return None;
    }
    let identity = availability().is_ok();
    (identity || fence_on).then(|| SessionIsolation {
        ws: ws.trim().to_string(),
        identity,
        fence: fence_on,
        workspace: workspace.map(Path::to_path_buf),
    })
}

/// The program a session runs once isolated: the exec helper in front of
/// it (`<helper> --agent-exec -- <program> <args…>`) and the helper's plan
/// for [`crate::agent_fence::ENV_SPEC`]. `env` is the session's final
/// environment (the harness home variables name its login slot).
/// Refuses (the spawn fails) rather than start a session without what it
/// was promised.
pub fn wrap(
    iso: &SessionIsolation,
    program: String,
    args: Vec<String>,
    env: &std::collections::HashMap<String, String>,
) -> Result<(String, Vec<String>, String), String> {
    use crate::agent_fence::{default_view, fence_env, landlock_abi, ExecSpec, ViewInputs, ARG};
    let helper = exec_helper().ok_or("the daemon has no exec helper for isolated sessions")?;
    let mut spec = ExecSpec { v: 1, cgroup: None, fence: None, unset: Vec::new(), set: Default::default() };
    if iso.identity {
        let a = availability()?;
        spec.cgroup = Some(ensure_dir(&a, &iso.ws)?.to_string_lossy().into_owned());
        // Tells the session's `k2` CLI that its passport is bound to this
        // cgroup, so it sends every passport request over the session socket
        // (the daemon refuses a bound passport on TCP).
        spec.set.insert(ENV_SESSION_MARK.into(), cgroup_name(&iso.ws).unwrap_or_default());
    }
    if iso.fence {
        landlock_abi().map_err(|e| format!("the folder fence needs Landlock: {e}"))?;
        let ws = iso.workspace.as_deref().ok_or("the folder fence needs the agent's workspace folder")?;
        let home = crate::paths::home_dir().ok_or("no home folder")?;
        let k2_home = crate::paths::k2_home();
        let tmp = tmp_dir(&iso.ws)?;
        let env: std::collections::BTreeMap<String, String> = env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let view = default_view(&ViewInputs { home: &home, k2_home: &k2_home, workspace: ws, tmp_dir: &tmp, env: &env })?;
        spec.fence = Some(view);
        fence_env(&mut spec, &tmp);
    }
    let json = serde_json::to_string(&spec).map_err(|e| e.to_string())?;
    let mut argv = vec![ARG.to_string(), "--".to_string(), program];
    argv.extend(args);
    Ok((helper.to_string_lossy().into_owned(), argv, json))
}

// ── a peer's cgroup ───────────────────────────────────────────────────

/// How the daemon pinned the peer process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pinned {
    /// `SO_PEERPIDFD` (Linux 6.5+), re-checked alive after reading its
    /// cgroup: race-free.
    Pidfd,
    /// The peer pid (`SO_PEERCRED`): a pid reused in between could be read
    /// instead (older kernels; `k2 app doctor` says so).
    Pid,
}

/// The cgroup v2 path of `pid`.
pub fn cgroup_of_pid(pid: u32) -> Result<String, String> {
    let t = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).map_err(|e| format!("/proc/{pid}/cgroup: {e}"))?;
    crate::agents_leaf::parse_self_cgroup(&t).ok_or_else(|| format!("/proc/{pid}/cgroup has no cgroup v2 path"))
}

/// Who is on the other end of a connected Unix socket, by cgroup.
#[cfg(target_os = "linux")]
pub fn peer_of_socket(fd: std::os::fd::RawFd, a: &Avail) -> (PeerAgent, Option<u32>, Pinned) {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    const SO_PEERPIDFD: libc::c_int = 77;
    let mut pidfd: libc::c_int = -1;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: sized out-param on a socket fd the caller owns.
    let rc = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, SO_PEERPIDFD, &mut pidfd as *mut _ as *mut libc::c_void, &mut len) };
    if rc == 0 && pidfd >= 0 {
        // SAFETY: the kernel handed us this fd.
        let pidfd = unsafe { OwnedFd::from_raw_fd(pidfd) };
        let pid = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", pidfd.as_raw_fd()))
            .ok()
            .and_then(|t| t.lines().find_map(|l| l.strip_prefix("Pid:")).and_then(|v| v.trim().parse::<i64>().ok()));
        let Some(pid) = pid.filter(|p| *p > 0).map(|p| p as u32) else {
            return (PeerAgent::Unknown { why: "the calling process has exited".into() }, None, Pinned::Pidfd);
        };
        let path = cgroup_of_pid(pid);
        // Still the same live process? Then the pid wasn't reused.
        // SAFETY: signal 0 to a pidfd we own only checks liveness.
        let alive = unsafe { libc::syscall(libc::SYS_pidfd_send_signal, pidfd.as_raw_fd(), 0, std::ptr::null::<u8>(), 0u32) } == 0;
        if !alive {
            return (PeerAgent::Unknown { why: "the calling process exited while it was checked".into() }, Some(pid), Pinned::Pidfd);
        }
        return match path {
            Ok(p) => (classify(&a.agents_rel, &p), Some(pid), Pinned::Pidfd),
            Err(e) => (PeerAgent::Unknown { why: e }, Some(pid), Pinned::Pidfd),
        };
    }
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: sized out-param.
    let rc = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) };
    if rc != 0 || cred.pid <= 0 {
        return (PeerAgent::Unknown { why: "no peer credentials on the socket".into() }, None, Pinned::Pid);
    }
    let pid = cred.pid as u32;
    match cgroup_of_pid(pid) {
        Ok(p) => (classify(&a.agents_rel, &p), Some(pid), Pinned::Pid),
        Err(e) => (PeerAgent::Unknown { why: e }, Some(pid), Pinned::Pid),
    }
}

/// Which agent owns `pid`: its cgroup, the agent, and the chain up to
/// the session leader K2 started (owner-only route, `k2 agent whois`).
pub fn whois(pid: u32) -> serde_json::Value {
    let a = availability();
    let cg = cgroup_of_pid(pid);
    let peer = match (&a, &cg) {
        (Ok(a), Ok(p)) => classify(&a.agents_rel, p),
        (_, Err(e)) => PeerAgent::Unknown { why: e.clone() },
        (Err(_), Ok(p)) => PeerAgent::Outside { cgroup: p.clone() },
    };
    serde_json::json!({
        "pid": pid,
        "cgroup": cg.as_ref().ok(),
        "workspaceId": peer.ws(),
        "who": peer.describe(),
        "identity": match &a { Ok(_) => "on".to_string(), Err(e) => format!("unavailable: {e}") },
        "claim": CLAIM,
    })
}

/// The per-agent cgroups now under the agents' leaf, with their process
/// counts (the doctor, and the boot scan that rebuilds nothing from memory).
pub fn list(a: &Avail) -> Vec<(String, usize)> {
    let Ok(rd) = std::fs::read_dir(&a.agents_dir) else { return Vec::new() };
    let mut out: Vec<(String, usize)> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let ws = name.strip_prefix(PREFIX).filter(|w| valid_ws(w))?.to_string();
            let n = std::fs::read_to_string(e.path().join("cgroup.procs")).map(|t| t.split_whitespace().count()).unwrap_or(0);
            Some((ws, n))
        })
        .collect();
    out.sort();
    out
}

/// At boot: report the agents' cgroups the last run left (k2-ptyd's
/// sessions keep theirs) and remove the empty ones (made again at the next
/// spawn). The name is the whole table: nothing to rebuild from memory.
pub fn boot_scan(a: &Avail) -> (usize, usize) {
    let mut live = 0;
    let mut removed = 0;
    for (ws, n) in list(a) {
        if n > 0 {
            live += 1;
        } else if std::fs::remove_dir(a.agents_dir.join(format!("{PREFIX}{ws}"))).is_ok() {
            removed += 1;
        }
    }
    (live, removed)
}

/// For `/boot-status` and `k2 app doctor`.
pub fn status_json() -> serde_json::Value {
    let landlock = crate::agent_fence::landlock_abi();
    let mut v = match availability() {
        Ok(a) => serde_json::json!({
            "identity": "on",
            "agentsCgroup": a.agents_rel,
            "claim": CLAIM,
        }),
        Err(why) => serde_json::json!({
            "identity": if switch_off() { "off" } else { "unavailable" },
            "why": why,
            "passports": "accepted as before (no cgroup check)",
        }),
    };
    v["switch"] = serde_json::json!(format!("{ENV_SWITCH}={}", if switch_off() { "0 (emergency off)" } else { "unset (on)" }));
    v["landlock"] = match landlock {
        Ok(n) => serde_json::json!({ "abi": n }),
        Err(e) => serde_json::json!({ "abi": null, "why": e }),
    };
    v
}

/// Boot (the daemon, after the agents' leaf): log the state loudly when
/// the switch is off, and scan the per-agent cgroups.
pub fn init() {
    if switch_off() {
        crate::log_debug!(
            "[agents] WARNING: agent cgroup identity is OFF ({ENV_SWITCH}=0, the emergency off-switch): passports are \
             accepted from any process, as before 0.46.17. Remove it from the daemon's environment once the box is fixed."
        );
        eprintln!("k2-daemon: WARNING: {ENV_SWITCH}=0: agent cgroup identity is off (emergency off-switch)");
        return;
    }
    match availability() {
        Ok(a) => {
            let (live, removed) = boot_scan(&a);
            crate::log_debug!("[agents] cgroup identity on under {}: {live} agent cgroup(s) with live sessions, {removed} empty one(s) removed", a.agents_rel);
        }
        Err(why) => crate::log_debug!("[agents] cgroup identity unavailable: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEAF: &str = "/k2cores.slice/k2-daemon.service/agents";

    #[test]
    fn names_and_the_switch() {
        assert_eq!(cgroup_name("3f1c-uuid_9").as_deref(), Some("a-3f1c-uuid_9"));
        for bad in ["", "../x", "a/b", "a b", "x.y", &"z".repeat(65)] {
            assert_eq!(cgroup_name(bad), None, "{bad}");
        }
        for off in ["0", "false", "OFF", " no "] {
            assert!(switch_off_from(Some(off)), "{off}");
        }
        for on in [None, Some(""), Some("1"), Some("on"), Some("yes")] {
            assert!(!switch_off_from(on), "{on:?}");
        }
    }

    #[test]
    fn a_cgroup_path_names_its_agent() {
        assert_eq!(classify(LEAF, &format!("{LEAF}/a-ws1")), PeerAgent::Agent { ws: "ws1".into(), cgroup: format!("{LEAF}/a-ws1") });
        assert_eq!(classify(LEAF, &format!("{LEAF}/a-ws1/sub/deeper")).ws(), Some("ws1"), "below its cgroup is still the agent");
        assert_eq!(classify(LEAF, LEAF), PeerAgent::Leaf { cgroup: LEAF.into() });
        assert!(matches!(classify(LEAF, "/k2cores.slice/k2-daemon.service/daemon"), PeerAgent::Outside { .. }));
        assert!(matches!(classify(LEAF, "/user.slice/user-1000.slice/session-3.scope"), PeerAgent::Outside { .. }));
        assert!(matches!(classify(LEAF, &format!("{LEAF}/b-ws1")), PeerAgent::Outside { .. }));
        assert!(matches!(classify(LEAF, &format!("{LEAF}x/a-ws1")), PeerAgent::Outside { .. }), "a sibling unit with the same prefix");
        assert!(matches!(classify(LEAF, &format!("{LEAF}/a-")), PeerAgent::Outside { .. }));
    }

    /// The guarantee in one table: a bound passport only from its own
    /// agent's cgroup; an unbound one as before.
    #[test]
    fn a_bound_passport_is_accepted_only_from_its_agents_cgroup() {
        let a = classify(LEAF, &format!("{LEAF}/a-A"));
        let b = classify(LEAF, &format!("{LEAF}/a-B"));
        assert_eq!(decide(Some("A"), &a), Decision::Allow);
        let Decision::Refuse { detail } = decide(Some("B"), &a) else { panic!("B's passport from A's cgroup must be refused") };
        assert!(detail.contains("agent B") && detail.contains("agent A"), "{detail}");
        for peer in [
            classify(LEAF, LEAF),
            classify(LEAF, "/user.slice/user-1000.slice/session-3.scope"),
            PeerAgent::Unknown { why: "gone".into() },
        ] {
            assert!(matches!(decide(Some("A"), &peer), Decision::Refuse { .. }), "{peer:?}");
        }
        assert_eq!(decide(None, &b), Decision::Allow, "a session placed without identity: as before");
        assert_eq!(decide(None, &PeerAgent::Unknown { why: "x".into() }), Decision::Allow);
    }
}
