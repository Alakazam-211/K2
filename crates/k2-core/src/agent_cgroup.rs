//! Agent Isolation M1: every agent's terminal sessions run in the agent's
//! own cgroup, `<unit>/agents/a-<workspace-id>/`, a child of the agents'
//! leaf ([`crate::agents_leaf`]), and the daemon recognises a caller on a
//! session socket by the cgroup its process is in
//! (`plan-agent-isolation-build-v1` §1, §3; owner decisions 2026-11-21).
//!
//! **What it gives.** Every terminal, `k2 job` run and published service of
//! an agent is labelled by agent: its processes start in the agent's cgroup
//! (`systemd-cgls`, `ps -o cgroup`, `k2 agent whois <pid>`). A fenced agent
//! can't write to its cgroup files, but until the Unix-socket block lands it
//! can still reach systemd's buses and other programs of the same user (kernel
//! rows 4, P2, P7), which can move it or run commands for it outside its
//! fence. A copied terminal or job passport alone no
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
//! for all (plan M1b). So the claim is "labelled by agent", with what a
//! fenced agent can still reach said beside it, never "cannot be forged".
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
     and a fenced agent can't write to its cgroup files. A copied terminal or job passport alone no longer works (App \
     exec and microVM cell passports aren't bound and work as before). An unfenced agent (same user) can still move itself \
     into another agent's cgroup, or read the owner token on disk. A fenced agent can't read the owner token, but \
     until the Unix-socket block lands, a fenced agent can still reach programs of the same user over Unix sockets (systemd's buses, the owner's tmux), which can run commands outside its fence or move its processes. Fencing an agent does not protect it from an unfenced one";

/// The claim with the root helper placing sessions (M1b).
pub const CLAIM_ROOT: &str = "labelled by agent: each agent's terminals, job runs and published services run in its own \
     root-owned cgroup, placed by the root helper, and an agent can't write itself into another agent's cgroup (until the \
     bus policy lands it can still ask systemd to put it there, kernel row 1c). A copied \
     terminal or job passport alone no longer works (App exec and microVM cell passports aren't bound and work as before). \
     An unfenced agent is still the same user as every other agent: it can make another agent's session run its commands \
     (through that agent's files, the owner token on disk, or ptrace where the kernel allows it), and it can move any \
     agent's processes out of their cgroup, which drops that label. A fenced agent can't do these directly, but \
     until the Unix-socket block lands, a fenced agent can still reach programs of the same user over Unix sockets (systemd's buses, the owner's tmux), which can run commands outside its fence or move its processes. Fencing an agent does not \
     protect it from an unfenced one";

/// The claim with the root helper's scopes (B4).
pub const CLAIM_SCOPE: &str = "labelled by agent: each agent's terminals, job runs and published services run in their own \
     root-owned scope outside the server's own service, placed by the root helper, and no process of the server user can write \
     a process into another agent's scope (checked when the server starts). Sessions from before an update may still be in \
     the old layout until they end; they get their own scope at their next start. A copied terminal or job passport alone no longer works (App exec and microVM cell \
     passports aren't bound and work as before). An unfenced agent is still the same user as every other agent: it can make \
     another agent's session run its commands (through that agent's files, the owner token on disk, or ptrace where the kernel \
     allows it), and until the bus policy lands it can ask systemd to move an agent's processes out of their scope (kernel \
     row 2), which drops that label. A fenced agent can't do these directly, but \
     until the Unix-socket block lands, a fenced agent can still reach programs of the same user over Unix sockets (systemd's buses, the owner's tmux), which can run commands outside its fence or move its processes. Fencing an agent does not protect it from an unfenced one";

/// The claim that holds on this box now.
pub fn claim() -> &'static str {
    if crate::agent_scope::scope_placement() {
        return CLAIM_SCOPE;
    }
    if root_placement() {
        CLAIM_ROOT
    } else {
        CLAIM
    }
}

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
    /// In `agents/a-<ws>` (or below it). `root_owned`: the cgroup is root's
    /// (the root helper made it; M1b), not the K2 user's.
    Agent { ws: String, cgroup: String, root_owned: bool },
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
            PeerAgent::Agent { ws, cgroup, root_owned: true } => format!("agent {ws} (cgroup {cgroup})"),
            PeerAgent::Agent { ws, cgroup, root_owned: false } => format!("agent {ws} (cgroup {cgroup}, the server user's, not root's)"),
            PeerAgent::Leaf { cgroup } => format!("no agent cgroup (the shared agents' leaf {cgroup})"),
            PeerAgent::Outside { cgroup } => format!("no agent cgroup ({cgroup})"),
            PeerAgent::Unknown { why } => format!("an unknown cgroup ({why})"),
        }
    }
}

/// Classify a cgroup v2 path (`/proc/<pid>/cgroup`'s `0::` value) against
/// the agents' leaf (`agents_rel`, e.g. `/k2cores.slice/k2-daemon.service/agents`).
pub fn classify(agents_rel: &str, path: &str) -> PeerAgent {
    // B4: a root-made scope under `k2agents.slice` names its agent exactly.
    if let Some((ws, _, _, cgroup)) = crate::agent_scope::classify_scope(path) {
        return PeerAgent::Agent { ws, cgroup, root_owned: false };
    }
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
                Some(ws) => PeerAgent::Agent { ws: ws.to_string(), cgroup: format!("{leaf}/{first}"), root_owned: false },
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

/// [`classify`] plus whether the agent cgroup is root's and holds `pid`
/// (M1b review B3): the directory is opened once (never through a link),
/// its owner read from that handle (`fstat`), and `pid` looked for in the
/// `cgroup.procs` opened through it. A cgroup swapped in under the same name
/// after `/proc/<pid>/cgroup` was read is a different handle: either not
/// root's, or not holding `pid`.
pub fn with_owner(peer: PeerAgent, pid: u32) -> PeerAgent {
    match peer {
        PeerAgent::Agent { ws, cgroup, .. } => {
            let root_owned = held_by(&Path::new(CGROUP_FS).join(cgroup.trim_start_matches('/')), pid, 0);
            PeerAgent::Agent { ws, cgroup, root_owned }
        }
        other => other,
    }
}

/// The cgroup directory `dir` is `uid`'s (and so is its `cgroup.procs`)
/// and lists `pid` among its own processes, all read through one handle
/// opened without following a link. `false` on any doubt.
#[cfg(unix)]
pub fn held_by(dir: &Path, pid: u32, uid: u32) -> bool {
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return false };
    // SAFETY: a NUL-terminated path; the fd is owned right after.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        return false;
    }
    // SAFETY: a fresh fd this function owns.
    let dirfd = unsafe { OwnedFd::from_raw_fd(fd) };
    let owner = |fd: libc::c_int| -> Option<libc::uid_t> {
        // SAFETY: fstat into a zeroed struct on an open fd.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        (unsafe { libc::fstat(fd, &mut st) } == 0).then_some(st.st_uid)
    };
    if owner(dirfd.as_raw_fd()) != Some(uid) {
        return false;
    }
    // SAFETY: openat relative to the handle above; the fd is owned right after.
    let pfd = unsafe { libc::openat(dirfd.as_raw_fd(), c"cgroup.procs".as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if pfd < 0 {
        return false;
    }
    // SAFETY: a fresh fd this function owns.
    let mut procs = unsafe { std::fs::File::from_raw_fd(pfd) };
    if owner(procs.as_raw_fd()) != Some(uid) {
        return false;
    }
    let mut s = String::new();
    if procs.read_to_string(&mut s).is_err() {
        return false;
    }
    s.split_whitespace().any(|p| p.parse::<u32>().ok() == Some(pid))
}

#[cfg(not(unix))]
pub fn held_by(_dir: &Path, _pid: u32, _uid: u32) -> bool {
    false
}

/// How a session was bound: its agent, and whether the root helper placed
/// it (then only a root-owned cgroup of that name is the agent: one the K2
/// user made, after removing the empty root one, is not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bound<'a> {
    pub ws: &'a str,
    pub root_placed: bool,
    /// B4: placed in its own scope; then only a scope of its agent counts.
    pub scoped: bool,
}

/// `bound`: the agent the passport's session is bound to, `None` for a
/// session placed without identity (accepted as before). `peer`: who the
/// connecting process is, by its cgroup (with [`with_owner`]).
pub fn decide(bound: Option<Bound>, peer: &PeerAgent) -> Decision {
    decide_in(bound, peer, crate::agent_scope::scope_placement(), crate::agent_scope::old_layout_allowed)
}

/// [`decide`] with the box's scope state passed in (pure, for tests).
pub fn decide_in(bound: Option<Bound>, peer: &PeerAgent, scopes_on: bool, old_layout_ok: impl Fn(&str) -> bool) -> Decision {
    let Some(b) = bound else { return Decision::Allow };
    // B4: with scopes on, a bound session is recognised from a scope only,
    // or from the old layout while its agent has survivors listed there.
    // A scoped session counts only from a scope (N3); the old-layout
    // exception is for the survivors' own (not scoped) sessions.
    if let PeerAgent::Agent { ws, cgroup, .. } = peer {
        let in_scope = crate::agent_scope::classify_scope(cgroup).is_some();
        if !in_scope && (b.scoped || (scopes_on && !old_layout_ok(ws))) {
            return Decision::Refuse {
                detail: format!("the passport of agent {} was presented from {}, outside an agent scope", b.ws, peer.describe()),
            };
        }
    }
    match peer {
        PeerAgent::Agent { ws, root_owned, .. } if ws == b.ws && (*root_owned || !b.root_placed) => Decision::Allow,
        other => Decision::Refuse {
            detail: format!("the passport of agent {} was presented from {}", b.ws, other.describe()),
        },
    }
}

// ── who places sessions: the root helper (M1b) or the daemon itself ───

/// Puts `pid` (an exec helper waiting to run agent `ws`, started at `start`,
/// clock ticks since boot) into the agent's root-owned cgroup: the daemon
/// registers the root helper's `agents place` at boot when the helper
/// serves it. `Ok(cgroup path)`.
pub type Placer = Box<dyn Fn(&str, u32, u64) -> Result<String, String> + Send + Sync>;

static PLACER: OnceLock<Placer> = OnceLock::new();
static NO_PLACER_WHY: OnceLock<String> = OnceLock::new();

/// Boot: the root helper places agent sessions (root-owned cgroups).
pub fn set_placer(p: Placer) {
    let _ = PLACER.set(p);
}

/// Boot: why the root helper doesn't (shown in `/boot-status`).
pub fn set_no_placer(why: String) {
    let _ = NO_PLACER_WHY.set(why);
}

/// The root helper places sessions here (identity on).
pub fn root_placement() -> bool {
    PLACER.get().is_some()
}

/// `pid`'s start time in clock ticks since boot (`/proc/<pid>/stat` field
/// 22). `None` without procfs.
pub fn proc_start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()
}

/// Place `pid` (started at `start`) for `ws` through the root helper.
pub fn place(ws: &str, pid: u32, start: u64) -> Result<String, String> {
    match PLACER.get() {
        Some(p) => p(ws, pid, start),
        None => Err("no root helper places agent sessions on this box".into()),
    }
}

/// M1b, right after a spawn: a session the root helper places is moved
/// (its exec helper, `pid`, waits for it) into the agent's root-owned
/// cgroup. On a refusal the process is killed (it never runs unplaced; the
/// helper would refuse on its own once its wait ends). Nothing for a
/// session the daemon placed. `start`: the process's start time as its
/// parent read it (ptyd); `None` when the daemon is the parent (it reads it
/// now: an unreaped child's pid can't be reused). The root helper places
/// the process only if that start time still matches (review B5).
pub fn place_spawned(iso: &SessionIsolation, pid: u32, start: Option<u64>) -> std::io::Result<()> {
    if !(iso.identity && iso.root_placed) {
        return Ok(());
    }
    let start = match start.or_else(|| proc_start_time(pid)) {
        Some(s) => Ok(s),
        None => Err(format!("no start time for pid {pid}")),
    };
    let placed = start.and_then(|s| match iso.scope.as_ref().filter(|_| crate::agent_scope::scope_placement()) {
        Some(plan) => crate::agent_scope::place(&iso.ws, plan, pid, s),
        None => place(&iso.ws, pid, s),
    });
    match placed {
        Ok(cg) => {
            crate::log_debug!("[agents] pid {pid} placed in {cg} by the root helper (agent {})", iso.ws);
            Ok(())
        }
        Err(e) => {
            #[cfg(unix)]
            // SAFETY: the process this spawn just started, still waiting in
            // the exec helper.
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
            crate::log_debug!("[agents] pid {pid} not started: the root helper didn't place it (agent {}): {e}", iso.ws);
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{} agent isolation: the root helper didn't place the session: {e}", crate::brand::PRODUCT_NAME)))
        }
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
    // B4 (helper mode): the scopes live under `k2agents.slice`, outside the
    // daemon's unit, which then needs no delegation and no agents' leaf.
    if crate::agent_scope::scope_placement() {
        let agents_rel = format!("/{}", crate::agent_scope::TOP_SLICE);
        return Ok(Avail { agents_dir: Path::new(CGROUP_FS).join(crate::agent_scope::TOP_SLICE), agents_rel });
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
    /// The root helper places it (a root-owned cgroup, M1b); `false`: the
    /// daemon does (a K2-user-owned one: traceable, but an unfenced agent
    /// can move into it).
    pub root_placed: bool,
    /// The owner turned this agent's folder fence on.
    pub fence: bool,
    /// The agent's workspace folder (the fence opens it).
    pub workspace: Option<PathBuf>,
    /// B4: the root helper places this process in its own scope
    /// (`k2-agent-<ws>:<id>.scope`); `None` for the M1b and M1 layouts.
    pub scope: Option<crate::agent_scope::ScopePlan>,
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
    let scoped = identity && crate::agent_scope::scope_placement();
    // A fresh scope id per spawn; with no randomness the spawn fails at
    // `wrap` rather than run unplaced.
    let scope = if scoped { crate::agent_scope::ScopePlan::new(fence_on).ok() } else { None };
    (identity || fence_on).then(|| SessionIsolation {
        ws: ws.trim().to_string(),
        identity,
        root_placed: identity && (root_placement() || scoped),
        fence: fence_on,
        workspace: workspace.map(Path::to_path_buf),
        scope,
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
    let mut spec = ExecSpec { v: 1, cgroup: None, wait_cgroup: None, fence: None, unset: Vec::new(), set: Default::default() };
    if iso.identity && crate::agent_scope::scope_placement() {
        // B4: the helper makes the scope around the waiting exec helper.
        let plan = iso.scope.as_ref().ok_or("agent isolation: no scope id for this session")?;
        let cg = plan.cgroup(&iso.ws).ok_or_else(|| format!("workspace id {:?} can't name a scope", iso.ws))?;
        spec.wait_cgroup = Some(format!("{CGROUP_FS}{cg}"));
        spec.set.insert(ENV_SESSION_MARK.into(), plan.unit(&iso.ws).unwrap_or_default());
    } else if iso.identity {
        let a = availability()?;
        let name = cgroup_name(&iso.ws).ok_or_else(|| format!("workspace id {:?} can't name a cgroup", iso.ws))?;
        if iso.root_placed {
            // The root helper makes the cgroup and moves the helper in once
            // the spawn reports its pid ([`place`]); the helper waits for it.
            spec.wait_cgroup = Some(a.agents_dir.join(&name).to_string_lossy().into_owned());
        } else {
            spec.cgroup = Some(ensure_dir(&a, &iso.ws)?.to_string_lossy().into_owned());
        }
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
            Ok(p) => (with_owner(classify(&a.agents_rel, &p), pid), Some(pid), Pinned::Pidfd),
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
        Ok(p) => (with_owner(classify(&a.agents_rel, &p), pid), Some(pid), Pinned::Pid),
        Err(e) => (PeerAgent::Unknown { why: e }, Some(pid), Pinned::Pid),
    }
}

/// Which agent owns `pid`: its cgroup, the agent, and the chain up to
/// the session leader K2 started (owner-only route, `k2 agent whois`).
pub fn whois(pid: u32) -> serde_json::Value {
    let a = availability();
    let cg = cgroup_of_pid(pid);
    let peer = match (&a, &cg) {
        (Ok(a), Ok(p)) => with_owner(classify(&a.agents_rel, p), pid),
        (_, Err(e)) => PeerAgent::Unknown { why: e.clone() },
        (Err(_), Ok(p)) => PeerAgent::Outside { cgroup: p.clone() },
    };
    // B4: an adopted scope (survivors of an update) is the one place a
    // process came in other than at its own spawn; whois says so.
    let scope = cg.as_ref().ok().and_then(|p| crate::agent_scope::classify_scope(p));
    let adopted = scope.as_ref().map(|(ws, id, _, _)| crate::agent_scope::is_adopted(ws, id));
    serde_json::json!({
        "pid": pid,
        "cgroup": cg.as_ref().ok(),
        "workspaceId": peer.ws(),
        "who": peer.describe(),
        "scope": scope.as_ref().map(|s| s.3.clone()),
        "adopted": adopted,
        "rootOwned": matches!(peer, PeerAgent::Agent { root_owned: true, .. }),
        "identity": status_json()["identity"],
        "claim": claim(),
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
        Ok(a) if crate::agent_scope::scope_placement() => serde_json::json!({
            "identity": "on",
            "placement": "root helper (a root-owned scope per session, job run and service)",
            "agentsCgroup": a.agents_rel,
            "claim": claim(),
        }),
        Ok(a) if root_placement() => serde_json::json!({
            "identity": "on",
            "placement": "root helper (root-owned agent cgroups)",
            "agentsCgroup": a.agents_rel,
            "claim": claim(),
        }),
        // M1 without the helper: traceable, never claimed as identity.
        Ok(a) => serde_json::json!({
            "identity": "unavailable",
            "why": NO_PLACER_WHY.get().cloned().unwrap_or_else(|| "the root helper doesn't place agent sessions here".into()),
            "traceable": true,
            "placement": "the daemon (agent cgroups the server user owns)",
            "agentsCgroup": a.agents_rel,
            "claim": claim(),
        }),
        Err(why) => serde_json::json!({
            "identity": if switch_off() { "off" } else { "unavailable" },
            "why": why,
            "passports": "accepted as before (no cgroup check)",
        }),
    };
    v["switch"] = serde_json::json!(format!("{ENV_SWITCH}={}", if switch_off() { "0 (emergency off)" } else { "unset (on)" }));
    // B4 (R4): why scopes are off, the P1 answer, and sessions still in the
    // old layout ("old layout, not adopted"), not only in the debug log.
    if let Some(b) = crate::agent_scope::box_state() {
        v["scopes"] = serde_json::json!({
            "on": crate::agent_scope::scope_placement(),
            "whyOff": b.why_off,
            "p1": b.p1,
            "pidMax": b.pid_max,
            "warning": b.pid_max.filter(|m| *m <= 32_768).map(|m| format!("kernel.pid_max is {m}: process numbers come round again quickly on this box (a session whose number is reused during placement is stopped, never mislabelled); 4194304 is the usual value")),
            "oldLayout": b.old_layout.iter().map(|(ws, why)| serde_json::json!({ "agent": ws, "state": "old layout, not adopted", "why": why })).collect::<Vec<_>>(),
        });
    }
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

    /// Review B5: a live process has a start time, and the same one twice.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_start_time_names_one_process() {
        let mine = proc_start_time(std::process::id()).expect("this process's start time");
        assert_eq!(proc_start_time(std::process::id()), Some(mine), "stable for the same process");
        assert_eq!(proc_start_time(0), None, "no such process");
    }

    /// Review B3: ownership and membership come from one handle; a link, a
    /// missing or unlisted pid, or another owner is a no.
    #[cfg(unix)]
    #[test]
    fn held_by_reads_owner_and_procs_through_one_handle() {
        let root = std::env::temp_dir().join(format!("k2-heldby-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cg = root.join("a-ws1");
        std::fs::create_dir_all(&cg).unwrap();
        std::fs::write(cg.join("cgroup.procs"), "101\n4242\n").unwrap();
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        assert!(held_by(&cg, 4242, me), "listed, owned by me");
        assert!(!held_by(&cg, 4243, me), "not listed");
        assert!(!held_by(&cg, 4242, me + 1), "another owner");
        std::os::unix::fs::symlink(&cg, root.join("a-link")).unwrap();
        assert!(!held_by(&root.join("a-link"), 4242, me), "never through a link");
        std::fs::create_dir_all(root.join("a-empty")).unwrap();
        assert!(!held_by(&root.join("a-empty"), 4242, me), "no cgroup.procs");
        std::fs::create_dir_all(root.join("a-linkedprocs")).unwrap();
        std::os::unix::fs::symlink(cg.join("cgroup.procs"), root.join("a-linkedprocs/cgroup.procs")).unwrap();
        assert!(!held_by(&root.join("a-linkedprocs"), 4242, me), "a linked cgroup.procs is refused");
        assert!(!held_by(&root.join("missing"), 4242, me));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// On a real kernel (review N2): a cgroup owned as expected that does
    /// not hold the pid is refused. This test's own cgroup holds it; its
    /// parent (owned by whoever owns it, root on a stock unit) does not,
    /// because cgroup v2 keeps processes in leaves. An owner check alone
    /// (the old path stat) would take the parent.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_real_cgroup_that_does_not_hold_the_pid_is_refused() {
        use std::os::unix::fs::MetadataExt;
        let line = std::fs::read_to_string("/proc/self/cgroup").expect("read /proc/self/cgroup");
        let rel = line.lines().find_map(|l| l.strip_prefix("0::")).expect("a cgroup v2 line in /proc/self/cgroup").trim().to_string();
        let own = Path::new(CGROUP_FS).join(rel.trim_start_matches('/'));
        assert!(own.join("cgroup.procs").is_file(), "cgroup v2 mounted at {CGROUP_FS} with this process's cgroup {}", own.display());
        let pid = std::process::id();
        let own_uid = std::fs::metadata(&own).expect("stat own cgroup").uid();
        assert!(held_by(&own, pid, own_uid), "its own cgroup ({}, uid {own_uid}) holds it", own.display());
        let parent = own.parent().expect("a parent cgroup");
        assert!(parent.join("cgroup.procs").is_file() && parent != own, "a parent cgroup: {}", parent.display());
        let parent_uid = std::fs::metadata(parent).expect("stat parent cgroup").uid();
        eprintln!("parent cgroup {} owned by uid {parent_uid}", parent.display());
        assert!(!held_by(parent, pid, parent_uid), "the parent ({}, uid {parent_uid}) does not hold it", parent.display());
    }

    #[test]
    fn a_cgroup_path_names_its_agent() {
        assert_eq!(classify(LEAF, &format!("{LEAF}/a-ws1")), PeerAgent::Agent { ws: "ws1".into(), cgroup: format!("{LEAF}/a-ws1"), root_owned: false });
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
        let user = |ws| Some(Bound { ws, root_placed: false, scoped: false });
        assert_eq!(decide(user("A"), &a), Decision::Allow);
        let Decision::Refuse { detail } = decide(user("B"), &a) else { panic!("B's passport from A's cgroup must be refused") };
        assert!(detail.contains("agent B") && detail.contains("agent A"), "{detail}");
        for peer in [
            classify(LEAF, LEAF),
            classify(LEAF, "/user.slice/user-1000.slice/session-3.scope"),
            PeerAgent::Unknown { why: "gone".into() },
        ] {
            assert!(matches!(decide(user("A"), &peer), Decision::Refuse { .. }), "{peer:?}");
        }
        assert_eq!(decide(None, &b), Decision::Allow, "a session placed without identity: as before");
        assert_eq!(decide(None, &PeerAgent::Unknown { why: "x".into() }), Decision::Allow);
    }

    /// M1b: a session the root helper placed counts only from a root-owned
    /// cgroup of its name; one the K2 user made again (after removing the
    /// empty root one) is not the agent. A self-placed session (no helper
    /// then) keeps M1's rule.
    #[test]
    fn a_root_placed_session_counts_only_from_a_root_owned_cgroup() {
        let root_a = PeerAgent::Agent { ws: "A".into(), cgroup: format!("{LEAF}/a-A"), root_owned: true };
        let user_a = PeerAgent::Agent { ws: "A".into(), cgroup: format!("{LEAF}/a-A"), root_owned: false };
        let placed = Some(Bound { ws: "A", root_placed: true, scoped: false });
        assert_eq!(decide(placed, &root_a), Decision::Allow);
        let Decision::Refuse { detail } = decide(placed, &user_a) else { panic!("a K2-user cgroup named a-A is not agent A's") };
        assert!(detail.contains("not root's"), "{detail}");
        let selfp = Some(Bound { ws: "A", root_placed: false, scoped: false });
        assert_eq!(decide(selfp, &user_a), Decision::Allow, "placed before the helper: M1's rule");
        assert_eq!(decide(selfp, &root_a), Decision::Allow);
        assert!(matches!(decide(Some(Bound { ws: "B", root_placed: true, scoped: false }), &root_a), Decision::Refuse { .. }));
    }

    /// B4 review test 4: with scopes on, a session's passport counts from
    /// an agent scope; from an old-layout cgroup only while that agent has
    /// survivors listed there.
    #[test]
    fn with_scopes_on_a_passport_counts_from_a_scope_or_a_listed_survivor() {
        let scope = PeerAgent::Agent { ws: "A".into(), cgroup: "/k2agents.slice/k2agents-open.slice/k2-agent-A:0123456789abcdef.scope".into(), root_owned: true };
        let old = PeerAgent::Agent { ws: "A".into(), cgroup: format!("{LEAF}/a-A"), root_owned: true };
        let placed = Some(Bound { ws: "A", root_placed: true, scoped: false });
        let none = |_: &str| false;
        assert_eq!(decide_in(placed, &scope, true, none), Decision::Allow);
        let Decision::Refuse { detail } = decide_in(placed, &old, true, none) else { panic!("an old-layout cgroup with no survivors listed is refused") };
        assert!(detail.contains("outside an agent scope"), "{detail}");
        assert_eq!(decide_in(placed, &old, true, |w: &str| w == "A"), Decision::Allow, "a listed survivor");
        assert!(matches!(decide_in(placed, &old, true, |w: &str| w == "B"), Decision::Refuse { .. }), "another agent's survivors don't count");
        assert_eq!(decide_in(placed, &old, false, none), Decision::Allow, "scopes off: M1b's rule");
        let other = PeerAgent::Agent { ws: "B".into(), cgroup: "/k2agents.slice/k2agents-open.slice/k2-agent-B:0123456789abcdef.scope".into(), root_owned: true };
        assert!(matches!(decide_in(placed, &other, true, none), Decision::Refuse { .. }), "another agent's scope");
    }

    /// B4 fix review N3 (test 2): a scoped session's passport counts only
    /// from a scope, even from a listed agent's root-owned old cgroup; a
    /// survivor's own (M1b-placed, not scoped) passport still counts there.
    #[test]
    fn a_scoped_session_counts_only_from_a_scope() {
        let scope = PeerAgent::Agent { ws: "A".into(), cgroup: "/k2agents.slice/k2agents-fenced.slice/k2-agent-A:0123456789abcdef.scope".into(), root_owned: true };
        let old = PeerAgent::Agent { ws: "A".into(), cgroup: format!("{LEAF}/a-A"), root_owned: true };
        let scoped = Some(Bound { ws: "A", root_placed: true, scoped: true });
        let survivor = Some(Bound { ws: "A", root_placed: true, scoped: false });
        let listed = |w: &str| w == "A";
        assert_eq!(decide_in(scoped, &scope, true, listed), Decision::Allow);
        let Decision::Refuse { detail } = decide_in(scoped, &old, true, listed) else { panic!("a scoped passport from the old layout is refused") };
        assert!(detail.contains("outside an agent scope"), "{detail}");
        assert!(matches!(decide_in(scoped, &old, false, listed), Decision::Refuse { .. }), "even with scopes off now");
        assert_eq!(decide_in(survivor, &old, true, listed), Decision::Allow, "the survivor's own passport");
    }
}
