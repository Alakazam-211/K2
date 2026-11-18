//! S17 / AR18: agent sessions get their own memory budget inside the
//! daemon's unit (`prd-k2-app-runtime-v1` §6.1).
//!
//! Agent sessions are the daemon's (or k2-ptyd's) children, so a non-root
//! daemon can't put them in a sibling system slice. Box prep's `apps
//! cores-slice` drop-in sets `Delegate=yes` on the daemon unit; with it the
//! daemon:
//! 1. moves every process left in its unit's own cgroup (itself, unless
//!    systemd already started it in `daemon/`; what a hand-off or an
//!    earlier run left there: frpc, ptyd, published services) into
//!    `<unit>/daemon/` (cgroup v2's no-internal-process rule);
//! 2. enables `+memory +pids` (and `+cpu` where it can) for its children;
//! 3. makes `<unit>/agents/` with `memory.high` = the agents' budget, and
//!    moves each terminal session's child into it right after the spawn;
//! 4. sweeps `daemon/` for agent processes (k2-ptyd's sessions and their
//!    children, a session's children forked before its move) at boot,
//!    after each spawn and every minute, into `agents/`.
//!
//! **The restart guard (0.46.16).** The unit runs `KillMode=process`, so
//! ptyd and its sessions outlive a stop and the split cgroup (controllers
//! on) stays. systemd's next start must place the main process somewhere:
//! in the unit's own cgroup the kernel refuses it (`status=219/CGROUP`).
//! Box prep therefore writes `k2-delegate-subgroup.conf` beside
//! `Delegate=yes`: `DelegateSubgroup=daemon` on systemd 254+ (the main
//! process starts in `daemon/`), or on older systemd an `ExecStopPost=+`
//! that turns the unit cgroup's controllers off after every stop, a failed
//! start included ([`CLEAR_MARK`]).
//! Without either ([`Guard::None`], a box prepared before 0.46.16) the
//! daemon never splits its unit, and turns the controllers off again if an
//! older daemon left them on, so the unit can always restart.
//!
//! `memory.high` throttles (reclaim pressure) rather than kills: a runaway
//! agent slows down instead of starving the daemon, Postgres and Stalwart,
//! which keep their protected floor (`k2cores.slice` `MemoryMin`). The
//! budget is 75% of the box's memory by default; the owner sets another in
//! the daemon's environment: `K2_AGENTS_MEMORY_HIGH=6G|60%|max`.
//!
//! Only on Linux, only in a **system** unit whose cgroup systemd handed to
//! the daemon (`Delegate=yes` chowns it to the unit's user). Anywhere else
//! (macOS, a container, no box prep, a user-scope daemon, root) it does
//! nothing and says why in [`status_json`]. Everything runs against a root
//! path through [`Cg`] and [`Procs`], so the tests use a temp dir that
//! keeps the kernel's rules.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The leaves of a delegated daemon unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaves {
    /// The unit's cgroup (parent of `daemon/` and `agents/`).
    pub base: PathBuf,
    /// The agents' budget as written to `memory.high` (`max` or bytes).
    pub memory_high: String,
}

/// The cgroup v2 path of a process from `/proc/<pid>/cgroup` text.
pub fn parse_self_cgroup(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix("0::")).map(|p| p.trim().to_string()).filter(|p| p.starts_with('/'))
}

/// The unit cgroup for a process path: a daemon already moved into
/// `daemon/` (a hand-off successor starts there) is under the same unit.
pub fn unit_base(self_path: &str) -> Option<String> {
    let p = self_path.trim_end_matches('/');
    let base = p.strip_suffix("/daemon").or_else(|| p.strip_suffix("/agents")).unwrap_or(p);
    (!base.is_empty() && base != "/").then(|| base.to_string())
}

/// `K2_AGENTS_MEMORY_HIGH`: `max`, a size (`6G`, `512M`, bytes) or a share
/// of `total` (`60%`). Default: 75% of `total`.
pub fn budget(raw: Option<&str>, total_bytes: Option<u64>) -> Result<String, String> {
    let pct = |p: u64| -> Result<String, String> {
        let t = total_bytes.ok_or("the box's memory is unknown")?;
        Ok((t / 100 * p).to_string())
    };
    let Some(r) = raw.map(str::trim).filter(|r| !r.is_empty()) else { return pct(75) };
    if r.eq_ignore_ascii_case("max") {
        return Ok("max".into());
    }
    if let Some(p) = r.strip_suffix('%') {
        let p: u64 = p.trim().parse().map_err(|_| format!("K2_AGENTS_MEMORY_HIGH {r:?}: a percentage 10-100"))?;
        if !(10..=100).contains(&p) {
            return Err(format!("K2_AGENTS_MEMORY_HIGH {r:?}: a percentage 10-100"));
        }
        return pct(p);
    }
    let (num, mult) = match r.char_indices().last() {
        Some((i, 'G' | 'g')) => (&r[..i], 1u64 << 30),
        Some((i, 'M' | 'm')) => (&r[..i], 1u64 << 20),
        _ => (r, 1u64),
    };
    let n: u64 = num.trim().parse().map_err(|_| format!("K2_AGENTS_MEMORY_HIGH {r:?}: max, 6G, 512M, bytes or 60%"))?;
    let b = n.checked_mul(mult).ok_or("K2_AGENTS_MEMORY_HIGH: too large")?;
    if b < 256 << 20 {
        return Err(format!("K2_AGENTS_MEMORY_HIGH {r:?}: at least 256M"));
    }
    Ok(b.to_string())
}

/// The protected floor for K2's cores (`k2 app box prepare --cores-mem-min`,
/// MiB) on a box with `total_bytes` of memory: about 1/16 of it rounded to
/// whole GiB, at least 2 GiB, at most 8 GiB (2 GiB at 32 GB or less, 4 GiB
/// at 64 GB, 8 GiB at 128 GB and up). The root helper refuses a floor above
/// half the box, so a box under ~4 GiB gets a quarter. The stock
/// provisioner's `k2_cores_mem_min_mib` is the same rule (its test pins the
/// same table). `None` for an unknown or empty box.
pub fn cores_floor_mib(total_bytes: u64) -> Option<u64> {
    let ram_mib = total_bytes >> 20;
    if ram_mib == 0 {
        return None;
    }
    let gib = ((ram_mib + 8192) / 16384).clamp(2, 8);
    let n = gib * 1024;
    Some(if n > ram_mib / 2 { ram_mib / 4 } else { n })
}

/// `MemTotal` from `/proc/meminfo` text, in bytes.
pub fn mem_total(meminfo: &str) -> Option<u64> {
    let l = meminfo.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = l.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

// ── the restart guard ────────────────────────────────────────────────

/// The leaf systemd starts the daemon in (`DelegateSubgroup=daemon`) and the
/// daemon's own processes live in.
pub const DAEMON_LEAF: &str = "daemon";
/// The agents' leaf.
pub const AGENTS_LEAF: &str = "agents";
/// `$0` of the older-systemd `ExecStopPost=` (= `k2_root_proto::apps::
/// CLEAR_MARK`; a k2-daemon test keeps the two equal).
pub const CLEAR_MARK: &str = "k2-agents-leaf-clear";
/// Box prep's drop-in that carries the guard (= `k2_root_proto::apps::
/// DAEMON_SUBGROUP_DROPIN_FILE`).
pub const GUARD_DROPIN: &str = "k2-delegate-subgroup.conf";

/// What lets the daemon's unit start again once its cgroup is split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    /// `DelegateSubgroup=daemon` (systemd 254+): the main process starts in
    /// `daemon/`, a leaf.
    Subgroup,
    /// The `ExecStopPost=+` that turns the unit cgroup's controllers off
    /// after each stop (systemd before 254).
    StopClear,
    /// Neither: a split unit would crash-loop at its next start.
    None,
}

impl Guard {
    pub fn as_str(self) -> &'static str {
        match self {
            Guard::Subgroup => "DelegateSubgroup",
            Guard::StopClear => "ExecStopPost",
            Guard::None => "none",
        }
    }
}

/// The parts of `systemctl show <unit> -p DelegateSubgroup -p ExecStopPost
/// -p Slice -p ControlGroup` the leaf needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitShow {
    pub guard: Guard,
    /// The unit's configured `Slice=` (after the last daemon-reload).
    pub slice: Option<String>,
    /// Where the unit runs now.
    pub control_group: Option<String>,
}

pub const SHOW_PROPS: &[&str] = &["DelegateSubgroup", "ExecStopPost", "Slice", "ControlGroup"];

pub fn parse_show(text: &str) -> UnitShow {
    let mut sub = false;
    let mut post = false;
    let (mut slice, mut cg) = (None, None);
    for l in text.lines() {
        if let Some(v) = l.strip_prefix("DelegateSubgroup=") {
            sub |= v.trim() == DAEMON_LEAF;
        } else if let Some(v) = l.strip_prefix("ExecStopPost=") {
            post |= v.contains(CLEAR_MARK);
        } else if let Some(v) = l.strip_prefix("Slice=") {
            slice = Some(v.trim().to_string()).filter(|v| !v.is_empty());
        } else if let Some(v) = l.strip_prefix("ControlGroup=") {
            cg = Some(v.trim().to_string()).filter(|v| !v.is_empty());
        }
    }
    let guard = if sub { Guard::Subgroup } else if post { Guard::StopClear } else { Guard::None };
    UnitShow { guard, slice, control_group: cg }
}

/// The guard as box prep's drop-in states it (when `systemctl show` can't
/// be asked).
pub fn guard_from_dropin(text: &str) -> Guard {
    let lines = || text.lines().map(str::trim).filter(|l| !l.starts_with('#'));
    if lines().any(|l| l == "DelegateSubgroup=daemon") {
        Guard::Subgroup
    } else if lines().any(|l| l.starts_with("ExecStopPost=") && l.contains(CLEAR_MARK)) {
        Guard::StopClear
    } else {
        Guard::None
    }
}

/// The slice a unit cgroup path sits in (`/k2cores.slice/k2-daemon.service`
/// → `k2cores.slice`; the root slice is `-.slice`).
pub fn slice_of(unit_cgroup: &str) -> Option<String> {
    let p = unit_cgroup.trim_end_matches('/');
    let parent = &p[..p.rfind('/')?];
    match parent.rsplit('/').next() {
        Some("") => Some("-.slice".into()),
        Some(s) if s.ends_with(".slice") => Some(s.to_string()),
        _ => None,
    }
}

// ── the cgroup tree and the process table, behind traits ─────────────

/// The cgroup v2 file system (the real one, or a temp dir in the tests).
pub trait Cg {
    fn read(&self, p: &Path) -> io::Result<String>;
    fn write(&self, p: &Path, s: &str) -> io::Result<()>;
    fn mkdir(&self, p: &Path) -> io::Result<()>;
    fn owner(&self, p: &Path) -> io::Result<u32>;
    fn exists(&self, p: &Path) -> bool;
}

/// The real cgroup file system.
pub struct RealCg;

impl Cg for RealCg {
    fn read(&self, p: &Path) -> io::Result<String> {
        std::fs::read_to_string(p)
    }
    fn write(&self, p: &Path, s: &str) -> io::Result<()> {
        std::fs::write(p, s)
    }
    fn mkdir(&self, p: &Path) -> io::Result<()> {
        match std::fs::create_dir(p) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            r => r,
        }
    }
    #[cfg(unix)]
    fn owner(&self, p: &Path) -> io::Result<u32> {
        use std::os::unix::fs::MetadataExt;
        Ok(std::fs::metadata(p)?.uid())
    }
    #[cfg(not(unix))]
    fn owner(&self, _p: &Path) -> io::Result<u32> {
        Err(io::Error::other("no uids here"))
    }
    fn exists(&self, p: &Path) -> bool {
        p.exists()
    }
}

/// The process table: parent pid and `comm`.
pub trait Procs {
    fn ppid(&self, pid: u32) -> Option<u32>;
    fn comm(&self, pid: u32) -> Option<String>;
}

/// `/proc`.
pub struct RealProcs;

impl Procs for RealProcs {
    fn ppid(&self, pid: u32) -> Option<u32> {
        parse_stat_ppid(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
    }
    fn comm(&self, pid: u32) -> Option<String> {
        std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim().to_string())
    }
}

/// The parent pid from `/proc/<pid>/stat` (`comm` may hold spaces and
/// parentheses: the fields after the last `)` are `state ppid …`).
pub fn parse_stat_ppid(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// k2-ptyd: every child it has is an agent session.
fn is_ptyd(procs: &dyn Procs, pid: u32) -> bool {
    procs.comm(pid).is_some_and(|c| c == "k2-ptyd")
}

fn pids(cg: &dyn Cg, dir: &Path) -> Vec<u32> {
    cg.read(&dir.join("cgroup.procs")).unwrap_or_default().split_whitespace().filter_map(|p| p.parse().ok()).collect()
}

fn controllers(cg: &dyn Cg, dir: &Path) -> Vec<String> {
    cg.read(&dir.join("cgroup.subtree_control")).unwrap_or_default().split_whitespace().map(str::to_string).collect()
}

fn write(cg: &dyn Cg, p: &Path, s: &str) -> Result<(), String> {
    cg.write(p, s).map_err(|e| format!("write {}: {e}", p.display()))
}

/// The processes in `daemon/` that belong in `agents/`: a k2-ptyd's
/// descendants, and descendants of a process already in `agents/` (forked
/// before their parent moved). Never ptyd itself, never the daemon's other
/// children (frpc, published services).
pub fn misplaced(daemon_pids: &[u32], agent_pids: &HashSet<u32>, procs: &dyn Procs) -> Vec<u32> {
    daemon_pids
        .iter()
        .copied()
        .filter(|&pid| {
            if is_ptyd(procs, pid) {
                return false;
            }
            let mut cur = procs.ppid(pid);
            for _ in 0..64 {
                match cur {
                    None | Some(0) | Some(1) => return false,
                    Some(a) if agent_pids.contains(&a) || is_ptyd(procs, a) => return true,
                    Some(a) => cur = procs.ppid(a),
                }
            }
            false
        })
        .collect()
}

/// What a sweep did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Swept {
    pub moved: Vec<u32>,
    pub failed: Vec<(u32, String)>,
}

/// Move the agent processes found in `daemon/` into `agents/`.
pub fn sweep_with(cg: &dyn Cg, procs: &dyn Procs, l: &Leaves) -> Swept {
    let agents = l.base.join(AGENTS_LEAF);
    let in_agents: HashSet<u32> = pids(cg, &agents).into_iter().collect();
    let mut out = Swept::default();
    for pid in misplaced(&pids(cg, &l.base.join(DAEMON_LEAF)), &in_agents, procs) {
        match cg.write(&agents.join("cgroup.procs"), &pid.to_string()) {
            Ok(()) => out.moved.push(pid),
            // Exited in between: gone already, fine.
            Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {}
            Err(e) => out.failed.push((pid, e.to_string())),
        }
    }
    out
}

/// Turn off every controller in the unit cgroup's `cgroup.subtree_control`
/// (allowed with processes in the children). `true` = some were on.
pub fn disarm_with(cg: &dyn Cg, base: &Path) -> Result<bool, String> {
    let on = controllers(cg, base);
    if on.is_empty() {
        return Ok(false);
    }
    let off: Vec<String> = on.iter().map(|c| format!("-{c}")).collect();
    write(cg, &base.join("cgroup.subtree_control"), &off.join(" "))?;
    Ok(true)
}

/// The refusal when box prep predates the restart guard.
pub const UNGUARDED: &str = "the daemon's unit has Delegate=yes but no restart guard (box prep before 0.46.16): \
     the daemon keeps the unit whole so it can restart. Re-run k2 app box prepare --cores-mem-min <MiB> (owner), then restart the daemon";

/// Set up the leaves under `fs_root` (normally `/sys/fs/cgroup`) for the
/// process cgroup `self_path`, owned by `uid`, given the unit's restart
/// `guard`. Fails (and changes nothing it can't undo) unless the unit's
/// cgroup is a system unit's, delegated to `uid`, with a guard.
pub fn setup_with(
    cg: &dyn Cg,
    procs: &dyn Procs,
    fs_root: &Path,
    self_path: &str,
    uid: u32,
    memory_high: &str,
    guard: Guard,
) -> Result<Leaves, String> {
    if uid == 0 {
        return Err("the daemon runs as root (no delegated unit to split)".into());
    }
    let rel = unit_base(self_path).ok_or("the daemon is in the root cgroup")?;
    if rel.contains("/user@") {
        return Err("a user-scope daemon (its unit isn't delegated by box prep)".into());
    }
    let base = fs_root.join(rel.trim_start_matches('/'));
    if !cg.exists(&base.join("cgroup.subtree_control")) {
        return Err(format!("{} has no subtree control (cgroup v2 with Delegate=yes)", base.display()));
    }
    let owner = cg.owner(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    if owner != uid {
        return Err("the daemon's unit isn't delegated to it (box prep: k2 app box prepare --cores-mem-min, then restart the daemon)".into());
    }
    if guard == Guard::None {
        // An older daemon may have split the unit already: undo the part
        // that stops the next start (the processes may stay where they are).
        match disarm_with(cg, &base) {
            Ok(true) => crate::log_debug!("[agents] no restart guard: turned the unit cgroup's controllers off so the unit can start again"),
            Ok(false) => {}
            Err(e) => return Err(format!("{UNGUARDED} (and turning its controllers off failed: {e})")),
        }
        return Err(UNGUARDED.into());
    }
    let daemon = base.join(DAEMON_LEAF);
    let agents = base.join(AGENTS_LEAF);
    for d in [&daemon, &agents] {
        cg.mkdir(d).map_err(|e| format!("create {}: {e}", d.display()))?;
    }
    // Whatever is still in the unit's own cgroup moves into daemon/ (a
    // process can't stay in a cgroup that hands controllers down): the
    // daemon itself on a StopClear start, what ran before the split. A
    // daemon systemd started in daemon/ (DelegateSubgroup) is never moved.
    let in_daemon_leaf = self_path.trim_end_matches('/').ends_with(&format!("/{DAEMON_LEAF}"));
    for pid in pids(cg, &base) {
        match cg.write(&daemon.join("cgroup.procs"), &pid.to_string()) {
            Ok(()) => {}
            // A process that exited in between is gone already: fine.
            Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {}
            Err(e) => return Err(format!("move {pid} into {}: {e}", daemon.display())),
        }
    }
    if !in_daemon_leaf {
        crate::log_debug!("[agents] the daemon moved itself into {}", daemon.display());
    }
    let sc = base.join("cgroup.subtree_control");
    write(cg, &sc, "+memory +pids")?;
    let _ = cg.write(&sc, "+cpu");
    write(cg, &agents.join("memory.high"), memory_high)?;
    let l = Leaves { base, memory_high: memory_high.to_string() };
    let s = sweep_with(cg, procs, &l);
    if !s.moved.is_empty() || !s.failed.is_empty() {
        crate::log_debug!("[agents] boot sweep: {} agent process(es) moved into agents/, {} not ({:?})", s.moved.len(), s.failed.len(), s.failed);
    }
    Ok(l)
}

/// [`setup_with`] on the real file system with the old signature's shape
/// (kept for callers that know their guard).
pub fn setup_at(fs_root: &Path, self_path: &str, uid: u32, memory_high: &str, guard: Guard) -> Result<Leaves, String> {
    setup_with(&RealCg, &RealProcs, fs_root, self_path, uid, memory_high, guard)
}

/// Move `pid` (a terminal session's child) into the agents' leaf.
pub fn join_with(cg: &dyn Cg, l: &Leaves, pid: u32) -> Result<(), String> {
    write(cg, &l.base.join(AGENTS_LEAF).join("cgroup.procs"), &pid.to_string())
}

/// The live picture of a unit cgroup, for `k2 app doctor`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Inspect {
    /// Processes in the unit's own cgroup.
    pub unit_procs: usize,
    pub daemon_procs: usize,
    pub agent_procs: usize,
    /// k2-ptyd's children (the live agent sessions) anywhere in the unit.
    pub sessions: usize,
    /// … of which in `agents/`.
    pub sessions_in_leaf: usize,
    /// Agent processes in `daemon/` ([`misplaced`]).
    pub misplaced: usize,
    /// Controllers on in the unit cgroup's `cgroup.subtree_control`.
    pub controllers_on: Vec<String>,
}

pub fn inspect_with(cg: &dyn Cg, procs: &dyn Procs, base: &Path) -> Inspect {
    let unit = pids(cg, base);
    let daemon = pids(cg, &base.join(DAEMON_LEAF));
    let agents = pids(cg, &base.join(AGENTS_LEAF));
    let in_agents: HashSet<u32> = agents.iter().copied().collect();
    let session = |p: &u32| procs.ppid(*p).is_some_and(|pp| is_ptyd(procs, pp));
    let sessions_in_leaf = agents.iter().filter(|p| session(p)).count();
    Inspect {
        unit_procs: unit.len(),
        daemon_procs: daemon.len(),
        agent_procs: agents.len(),
        sessions: unit.iter().chain(&daemon).filter(|p| session(p)).count() + sessions_in_leaf,
        sessions_in_leaf,
        misplaced: misplaced(&daemon, &in_agents, procs).len(),
        controllers_on: controllers(cg, base),
    }
}

/// A restart would fail: controllers on in the unit cgroup (processes
/// survive a stop under KillMode=process, so it stays) and no guard.
pub fn armed(i: &Inspect, guard: Guard) -> bool {
    !i.controllers_on.is_empty() && guard == Guard::None
}

// ── the process-wide state ───────────────────────────────────────────

struct State {
    /// The unit's cgroup (whatever the outcome), its name and its slice.
    base: Option<PathBuf>,
    unit: Option<String>,
    leaves: Result<Leaves, String>,
}

static STATE: OnceLock<State> = OnceLock::new();

/// `systemctl show` for the daemon's unit, else box prep's drop-in.
fn unit_show(unit: &str) -> UnitShow {
    let mut args: Vec<String> = vec!["show".into(), unit.into()];
    for p in SHOW_PROPS {
        args.push("-p".into());
        args.push((*p).into());
    }
    let shown = crate::bg_proc::command("systemctl")
        .args(&args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string());
    match shown {
        Some(t) => parse_show(&t),
        None => {
            let f = format!("/etc/systemd/system/{unit}.d/{GUARD_DROPIN}");
            let guard = std::fs::read_to_string(f).map(|t| guard_from_dropin(&t)).unwrap_or(Guard::None);
            UnitShow { guard, slice: None, control_group: None }
        }
    }
}

/// Boot (the daemon, once): set up the leaves when this daemon runs in a
/// delegated, guarded system unit. Never fails the boot.
pub fn init() {
    let st = STATE.get_or_init(|| {
        let mut st = State { base: None, unit: None, leaves: Err("Linux only".into()) };
        if !cfg!(target_os = "linux") {
            return st;
        }
        let mut base = None;
        let mut unit = None;
        let leaves = (|| {
            let text = std::fs::read_to_string("/proc/self/cgroup").map_err(|e| format!("/proc/self/cgroup: {e}"))?;
            let path = parse_self_cgroup(&text).ok_or("no cgroup v2 path")?;
            let rel = unit_base(&path);
            base = rel.as_ref().map(|r| Path::new("/sys/fs/cgroup").join(r.trim_start_matches('/')));
            unit = rel.as_ref().and_then(|r| r.rsplit('/').next()).filter(|u| u.ends_with(".service")).map(str::to_string);
            let total = std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| mem_total(&m));
            let high = budget(std::env::var("K2_AGENTS_MEMORY_HIGH").ok().as_deref(), total)?;
            #[cfg(unix)]
            let uid = unsafe { libc::geteuid() };
            #[cfg(not(unix))]
            let uid = 0;
            let guard = unit.as_deref().map(|u| unit_show(u).guard).unwrap_or(Guard::None);
            setup_at(Path::new("/sys/fs/cgroup"), &path, uid, &high, guard)
        })();
        st = State { base, unit, leaves };
        match &st.leaves {
            Ok(l) => crate::log_debug!("[agents] agent sessions get their own leaf {} (memory.high {})", l.base.join(AGENTS_LEAF).display(), l.memory_high),
            Err(e) => crate::log_debug!("[agents] no agents leaf: {e}"),
        }
        st
    });
    if st.leaves.is_ok() {
        // Catch what a spawn's move missed (a child forked before it).
        let _ = std::thread::Builder::new().name("agents-leaf-sweep".into()).spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
            sweep();
        });
    }
}

fn sweep() {
    if let Some(State { leaves: Ok(l), .. }) = STATE.get() {
        let s = sweep_with(&RealCg, &RealProcs, l);
        if !s.failed.is_empty() {
            crate::log_debug!("[agents] sweep: {} agent process(es) not moved into agents/: {:?}", s.failed.len(), s.failed);
        }
    }
}

/// After a terminal session spawns: its child joins the agents' leaf (and
/// anything it forked already follows).
pub fn join(pid: u32) {
    if let Some(State { leaves: Ok(l), .. }) = STATE.get() {
        if let Err(e) = join_with(&RealCg, l, pid) {
            crate::log_debug!("[agents] session {pid} not moved into the agents leaf: {e}");
        }
        sweep();
    }
}

/// The leaf, or why there is none (cheap: `k2 app resources` reads it).
pub fn status_json() -> serde_json::Value {
    match STATE.get().map(|s| &s.leaves) {
        Some(Ok(l)) => serde_json::json!({ "on": true, "leaf": l.base.join(AGENTS_LEAF), "memoryHigh": l.memory_high }),
        Some(Err(e)) => serde_json::json!({ "on": false, "why": e }),
        None => serde_json::json!({ "on": false, "why": "not set up (the daemon sets it up at boot)" }),
    }
}

/// `k2 app doctor`: [`status_json`] plus the live cgroup (after a sweep),
/// the unit's restart guard and its slice as configured vs as running.
pub fn doctor_json() -> serde_json::Value {
    let mut v = status_json();
    let Some(st) = STATE.get() else { return v };
    if st.leaves.is_ok() {
        sweep();
    }
    // A system unit only (a user-scope daemon's unit isn't box prep's).
    let Some(base) = st.base.as_ref().filter(|b| !b.to_string_lossy().contains("/user@")) else { return v };
    let show = st.unit.as_deref().map(unit_show);
    if RealCg.exists(&base.join("cgroup.subtree_control")) {
        let guard = show.as_ref().map(|s| s.guard).unwrap_or(Guard::None);
        v["live"] = doctor_fields(&inspect_with(&RealCg, &RealProcs, base), guard);
    }
    if let Some(s) = show {
        v["guard"] = serde_json::json!(s.guard.as_str());
        v["unitSlice"] = serde_json::json!(s.slice);
        v["runningSlice"] = serde_json::json!(s.control_group.as_deref().and_then(slice_of));
    }
    v
}

/// The doctor's fields for an [`Inspect`].
pub fn doctor_fields(i: &Inspect, guard: Guard) -> serde_json::Value {
    serde_json::json!({
        "unitProcs": i.unit_procs,
        "daemonProcs": i.daemon_procs,
        "agentProcs": i.agent_procs,
        "sessions": i.sessions,
        "sessionsInLeaf": i.sessions_in_leaf,
        "misplaced": i.misplaced,
        "controllersOn": i.controllers_on,
        "armed": armed(i, guard),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_budgets() {
        assert_eq!(parse_self_cgroup("0::/system.slice/k2-daemon.service\n").as_deref(), Some("/system.slice/k2-daemon.service"));
        assert_eq!(parse_self_cgroup("1:name=systemd:/x\n"), None);
        assert_eq!(unit_base("/system.slice/k2-daemon.service").as_deref(), Some("/system.slice/k2-daemon.service"));
        assert_eq!(unit_base("/k2cores.slice/k2-daemon.service/daemon").as_deref(), Some("/k2cores.slice/k2-daemon.service"), "a hand-off successor");
        assert_eq!(unit_base("/"), None);
        let total = Some(8u64 << 30);
        assert_eq!(budget(None, total).unwrap(), ((8u64 << 30) / 100 * 75).to_string(), "75% by default");
        assert_eq!(budget(Some("max"), None).unwrap(), "max");
        assert_eq!(budget(Some("6G"), None).unwrap(), (6u64 << 30).to_string());
        assert_eq!(budget(Some("512M"), None).unwrap(), (512u64 << 20).to_string());
        assert_eq!(budget(Some("60%"), total).unwrap(), ((8u64 << 30) / 100 * 60).to_string());
        for bad in ["5%", "101%", "lots", "1M", "-1G"] {
            assert!(budget(Some(bad), total).is_err(), "{bad}");
        }
        assert!(budget(None, None).is_err(), "75% of an unknown box");
        assert_eq!(mem_total("MemTotal:        8040532 kB\nMemFree: 1 kB\n"), Some(8040532 * 1024));
    }

    /// The same table as tests/cli/provision_apps.sh (MemTotal in KiB → MiB).
    #[test]
    fn the_cores_floor_follows_the_boxs_ram() {
        for (kib, want) in [
            (3_884_000u64, 948u64),
            (8_000_000, 2048),
            (16_300_000, 2048),
            (32_786_000, 2048),
            (33_554_432, 2048),
            (49_300_000, 3072),
            (65_842_476, 4096),
            (67_108_864, 4096),
            (98_600_000, 6144),
            (131_900_000, 8192),
            (134_217_728, 8192),
            (263_000_000, 8192),
        ] {
            assert_eq!(cores_floor_mib(kib * 1024), Some(want), "MemTotal {kib} kB");
        }
        assert_eq!(cores_floor_mib(0), None);
    }

    // ── a temp cgroup tree that keeps the kernel's rules ──────────────

    use std::cell::RefCell;
    use std::collections::HashMap;

    /// A cgroup v2 tree in a temp dir: moving a pid takes it out of every
    /// other `cgroup.procs`; a cgroup with controllers on in its
    /// `cgroup.subtree_control` takes no process (EBUSY, the no-internal-
    /// process rule: what systemd hits as 219/CGROUP), and controllers go
    /// on only in a cgroup without processes.
    struct FakeCg {
        root: PathBuf,
        uid: u32,
    }

    fn busy() -> io::Error {
        io::Error::from_raw_os_error(libc::EBUSY)
    }

    impl FakeCg {
        fn new(uid: u32) -> FakeCg {
            let root = std::env::temp_dir().join(format!("k2-agents-leaf-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&root).unwrap();
            FakeCg { root, uid }
        }
        fn all_dirs(&self, d: &Path, out: &mut Vec<PathBuf>) {
            out.push(d.to_path_buf());
            for e in std::fs::read_dir(d).unwrap().flatten() {
                if e.path().is_dir() {
                    self.all_dirs(&e.path(), out);
                }
            }
        }
        fn procs(&self, d: &Path) -> Vec<u32> {
            pids(self, d)
        }
        fn set(&self, d: &Path, list: &[u32]) {
            let t: Vec<String> = list.iter().map(|p| p.to_string()).collect();
            std::fs::write(d.join("cgroup.procs"), t.join("\n")).unwrap();
        }
        /// A process exits (or systemd's stop kills only the main one).
        fn exit(&self, pid: u32) {
            let mut v = Vec::new();
            self.all_dirs(&self.root, &mut v);
            for d in v {
                let l: Vec<u32> = self.procs(&d).into_iter().filter(|p| *p != pid).collect();
                if d.join("cgroup.procs").exists() {
                    self.set(&d, &l);
                }
            }
        }
    }

    impl Drop for FakeCg {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Cg for FakeCg {
        fn read(&self, p: &Path) -> io::Result<String> {
            std::fs::read_to_string(p)
        }
        fn write(&self, p: &Path, s: &str) -> io::Result<()> {
            let dir = p.parent().unwrap();
            match p.file_name().and_then(|n| n.to_str()) {
                Some("cgroup.procs") => {
                    if !dir.exists() {
                        return Err(io::Error::from_raw_os_error(libc::ENOENT));
                    }
                    if !controllers(self, dir).is_empty() {
                        return Err(busy());
                    }
                    let pid: u32 = s.trim().parse().map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
                    self.exit(pid);
                    let mut l = self.procs(dir);
                    l.push(pid);
                    self.set(dir, &l);
                    Ok(())
                }
                Some("cgroup.subtree_control") => {
                    let mut on: Vec<String> = controllers(self, dir);
                    for t in s.split_whitespace() {
                        if let Some(c) = t.strip_prefix('+') {
                            if !self.procs(dir).is_empty() {
                                return Err(busy());
                            }
                            if !on.iter().any(|x| x == c) {
                                on.push(c.to_string());
                            }
                        } else if let Some(c) = t.strip_prefix('-') {
                            on.retain(|x| x != c);
                        } else {
                            return Err(io::Error::from_raw_os_error(libc::EINVAL));
                        }
                    }
                    on.sort();
                    std::fs::write(p, on.join(" "))
                }
                _ => std::fs::write(p, s),
            }
        }
        fn mkdir(&self, p: &Path) -> io::Result<()> {
            std::fs::create_dir_all(p)?;
            for f in ["cgroup.procs", "cgroup.subtree_control"] {
                if !p.join(f).exists() {
                    std::fs::write(p.join(f), "")?;
                }
            }
            Ok(())
        }
        fn owner(&self, _p: &Path) -> io::Result<u32> {
            Ok(self.uid)
        }
        fn exists(&self, p: &Path) -> bool {
            p.exists()
        }
    }

    #[derive(Default)]
    struct FakeProcs(RefCell<HashMap<u32, (u32, String)>>);

    impl FakeProcs {
        fn add(&self, pid: u32, ppid: u32, comm: &str) {
            self.0.borrow_mut().insert(pid, (ppid, comm.to_string()));
        }
    }

    impl Procs for FakeProcs {
        fn ppid(&self, pid: u32) -> Option<u32> {
            self.0.borrow().get(&pid).map(|p| p.0)
        }
        fn comm(&self, pid: u32) -> Option<String> {
            self.0.borrow().get(&pid).map(|p| p.1.clone())
        }
    }

    const UID: u32 = 1001;
    const UNIT: &str = "/k2cores.slice/k2-daemon.service";

    /// systemd starting the unit's main process `pid` with `guard` (the
    /// unit cgroup made on the first start).
    fn systemd_start(cg: &FakeCg, guard: Guard, pid: u32) -> Result<(), String> {
        let base = cg.root.join(UNIT.trim_start_matches('/'));
        cg.mkdir(&base).unwrap();
        let fail = |e: io::Error| format!("status=219/CGROUP ({e})");
        match guard {
            Guard::Subgroup => {
                cg.mkdir(&base.join(DAEMON_LEAF)).unwrap();
                cg.write(&base.join(DAEMON_LEAF).join("cgroup.procs"), &pid.to_string()).map_err(fail)
            }
            Guard::StopClear | Guard::None => cg.write(&base.join("cgroup.procs"), &pid.to_string()).map_err(fail),
        }
    }

    /// systemd stopping it under KillMode=process (only the main process
    /// is killed), then the guard's ExecStopPost= in the `.control`
    /// subgroup (where systemd runs a delegated unit's later control
    /// processes): each controller off, then it exits. Also what follows a
    /// failed start (`main` = None).
    fn systemd_stop(cg: &FakeCg, guard: Guard, main: Option<u32>) {
        if let Some(m) = main {
            cg.exit(m);
        }
        if guard == Guard::StopClear {
            let base = cg.root.join(UNIT.trim_start_matches('/'));
            cg.mkdir(&base.join(".control")).unwrap();
            cg.write(&base.join(".control/cgroup.procs"), "9999").unwrap();
            for c in controllers(cg, &base) {
                cg.write(&base.join("cgroup.subtree_control"), &format!("-{c}")).unwrap();
            }
            cg.exit(9999);
        }
    }

    /// The daemon's own cgroup path after systemd started it.
    fn self_path(guard: Guard) -> String {
        if guard == Guard::Subgroup { format!("{UNIT}/daemon") } else { UNIT.to_string() }
    }

    /// A box that ran before the split: ptyd (300) with two sessions (301,
    /// 302; 303 is 301's child) and frpc (400), all left in the unit's own
    /// cgroup by KillMode=process.
    fn leftovers_before_the_split(cg: &FakeCg, procs: &FakeProcs) {
        let base = cg.root.join(UNIT.trim_start_matches('/'));
        cg.mkdir(&base).unwrap();
        procs.add(300, 1, "k2-ptyd");
        procs.add(301, 300, "claude");
        procs.add(302, 300, "codex");
        procs.add(303, 301, "node");
        procs.add(400, 1, "frpc");
        cg.set(&base, &[300, 301, 302, 303, 400]);
    }

    fn sorted(mut v: Vec<u32>) -> Vec<u32> {
        v.sort();
        v
    }

    /// The first start after box prep, then restarts with leftovers, per
    /// guard: every start succeeds, the sessions land in agents/, the
    /// daemon's own processes in daemon/, the unit cgroup stays empty.
    #[test]
    fn restarts_with_leftover_processes_start_again_per_guard() {
        for guard in [Guard::Subgroup, Guard::StopClear] {
            let cg = FakeCg::new(UID);
            let procs = FakeProcs::default();
            leftovers_before_the_split(&cg, &procs);
            let base = cg.root.join(UNIT.trim_start_matches('/'));
            let mut main = 100;
            procs.add(main, 1, "k2-daemon");
            systemd_start(&cg, guard, main).unwrap();
            let l = setup_with(&cg, &procs, &cg.root, &self_path(guard), UID, "6442450944", guard).unwrap();
            assert_eq!(l.base, base);
            assert!(cg.procs(&base).is_empty(), "{guard:?}: the unit cgroup hands controllers down");
            assert_eq!(sorted(cg.procs(&base.join("daemon"))), vec![100, 300, 400], "{guard:?}");
            assert_eq!(sorted(cg.procs(&base.join("agents"))), vec![301, 302, 303], "{guard:?}: ptyd's sessions and their children");
            assert_eq!(controllers(&cg, &base), vec!["cpu", "memory", "pids"]);
            assert_eq!(std::fs::read_to_string(base.join("agents/memory.high")).unwrap(), "6442450944");
            let i = inspect_with(&cg, &procs, &base);
            assert_eq!((i.sessions, i.sessions_in_leaf, i.misplaced), (2, 2, 0));
            assert!(!armed(&i, guard));

            // Three restarts: KillMode=process stops only the main process.
            for n in 0..3 {
                systemd_stop(&cg, guard, Some(main));
                main = 110 + n;
                procs.add(main, 1, "k2-daemon");
                // A session ptyd spawned while the daemon was down sits in
                // daemon/ (ptyd's cgroup) until the sweep.
                let late = 500 + n;
                procs.add(late, 300, "claude");
                cg.write(&base.join("daemon/cgroup.procs"), &late.to_string()).unwrap();
                systemd_start(&cg, guard, main).unwrap_or_else(|e| panic!("{guard:?} restart {n}: {e}"));
                setup_with(&cg, &procs, &cg.root, &self_path(guard), UID, "6442450944", guard).unwrap();
                assert!(cg.procs(&base).is_empty());
                assert_eq!(sorted(cg.procs(&base.join("daemon"))), sorted(vec![300, 400, main]), "{guard:?} restart {n}");
                assert!(cg.procs(&base.join("agents")).contains(&late), "{guard:?} restart {n}: the late session was swept");
                assert_eq!(controllers(&cg, &base), vec!["cpu", "memory", "pids"]);
            }
        }
    }

    /// The live bug (0.46.14): split with no guard, the next start fails
    /// with 219/CGROUP. A daemon of this release finding no guard (a
    /// hand-off successor in daemon/ on a box prepared before the guard)
    /// turns the controllers off and never splits, so the unit restarts.
    #[test]
    fn no_guard_is_never_armed_and_an_armed_unit_is_disarmed() {
        let cg = FakeCg::new(UID);
        let procs = FakeProcs::default();
        leftovers_before_the_split(&cg, &procs);
        let base = cg.root.join(UNIT.trim_start_matches('/'));
        // What a 0.46.14 daemon did: split and arm.
        procs.add(100, 1, "k2-daemon");
        systemd_start(&cg, Guard::None, 100).unwrap();
        for d in ["daemon", "agents"] {
            cg.mkdir(&base.join(d)).unwrap();
        }
        for p in [100, 300, 301, 302, 303, 400] {
            cg.write(&base.join("daemon/cgroup.procs"), &p.to_string()).unwrap();
        }
        cg.write(&base.join("cgroup.subtree_control"), "+memory +pids +cpu").unwrap();
        let i = inspect_with(&cg, &procs, &base);
        assert!(armed(&i, Guard::None), "{i:?}");
        assert_eq!((i.agent_procs, i.sessions, i.sessions_in_leaf, i.misplaced), (0, 2, 0, 3), "agents/ empty, sessions in daemon/");
        cg.exit(100);
        assert!(systemd_start(&cg, Guard::None, 101).unwrap_err().contains("219/CGROUP"), "the bug, reproduced");

        // A hand-off successor (in daemon/) of this release: no guard.
        procs.add(102, 1, "k2-daemon");
        cg.write(&base.join("daemon/cgroup.procs"), "102").unwrap();
        let e = setup_with(&cg, &procs, &cg.root, &format!("{UNIT}/daemon"), UID, "max", Guard::None).unwrap_err();
        assert_eq!(e, UNGUARDED);
        assert!(controllers(&cg, &base).is_empty(), "disarmed");
        assert!(!armed(&inspect_with(&cg, &procs, &base), Guard::None));
        cg.exit(102);
        systemd_start(&cg, Guard::None, 103).unwrap();
        // … and that daemon, unguarded still, leaves the unit whole.
        assert_eq!(setup_with(&cg, &procs, &cg.root, UNIT, UID, "max", Guard::None).unwrap_err(), UNGUARDED);
        assert_eq!(cg.procs(&base), vec![103]);
        cg.exit(103);
        systemd_start(&cg, Guard::None, 104).unwrap();
    }

    /// The repair on a box already crash-looping: once the guard is in
    /// (and systemd reloaded), DelegateSubgroup starts at the next attempt;
    /// the ExecStopPost= one after the next failed attempt's stop.
    #[test]
    fn the_guard_repairs_a_crash_looping_unit() {
        for guard in [Guard::Subgroup, Guard::StopClear] {
            let cg = FakeCg::new(UID);
            let procs = FakeProcs::default();
            leftovers_before_the_split(&cg, &procs);
            let base = cg.root.join(UNIT.trim_start_matches('/'));
            for d in ["daemon", "agents"] {
                cg.mkdir(&base.join(d)).unwrap();
            }
            for p in [300, 301, 302, 303, 400] {
                cg.write(&base.join("daemon/cgroup.procs"), &p.to_string()).unwrap();
            }
            cg.write(&base.join("cgroup.subtree_control"), "+cpu +memory +pids").unwrap();
            assert!(systemd_start(&cg, Guard::None, 100).is_err(), "crash-looping");
            let mut attempts = 0;
            let main = loop {
                attempts += 1;
                let pid = 100 + attempts;
                if systemd_start(&cg, guard, pid).is_ok() {
                    break pid;
                }
                systemd_stop(&cg, guard, None);
                assert!(attempts < 3, "{guard:?}");
            };
            assert_eq!(attempts, if guard == Guard::Subgroup { 1 } else { 2 }, "{guard:?}");
            procs.add(main, 1, "k2-daemon");
            setup_with(&cg, &procs, &cg.root, &self_path(guard), UID, "max", guard).unwrap();
            assert_eq!(sorted(cg.procs(&base.join("agents"))), vec![301, 302, 303], "{guard:?}: the stuck sessions moved too");
            systemd_stop(&cg, guard, Some(main));
            systemd_start(&cg, guard, main + 10).unwrap();
        }
    }

    /// DelegateSubgroup: the daemon starts in daemon/ and is never written
    /// anywhere; only strays in the unit cgroup move.
    #[test]
    fn a_daemon_started_in_its_leaf_is_not_moved() {
        let cg = FakeCg::new(UID);
        let procs = FakeProcs::default();
        let base = cg.root.join(UNIT.trim_start_matches('/'));
        procs.add(100, 1, "k2-daemon");
        systemd_start(&cg, Guard::Subgroup, 100).unwrap();
        setup_with(&cg, &procs, &cg.root, &format!("{UNIT}/daemon"), UID, "max", Guard::Subgroup).unwrap();
        assert_eq!(cg.procs(&base.join("daemon")), vec![100]);
        assert!(cg.procs(&base.join("agents")).is_empty());
        let i = inspect_with(&cg, &procs, &base);
        assert_eq!((i.sessions, i.misplaced, i.unit_procs), (0, 0, 0));
    }

    /// A ptyd-spawned session lands in agents/ (join after the spawn, and
    /// its children follow on the sweep); the daemon's other children don't.
    #[test]
    fn ptyd_sessions_join_the_agents_leaf() {
        let cg = FakeCg::new(UID);
        let procs = FakeProcs::default();
        let base = cg.root.join(UNIT.trim_start_matches('/'));
        procs.add(100, 1, "k2-daemon");
        systemd_start(&cg, Guard::Subgroup, 100).unwrap();
        let l = setup_with(&cg, &procs, &cg.root, &format!("{UNIT}/daemon"), UID, "max", Guard::Subgroup).unwrap();
        // ptyd launched by the daemon (double fork: its parent is 1) and a
        // published service (the daemon's child).
        procs.add(300, 1, "k2-ptyd");
        procs.add(450, 100, "python3");
        for p in [300, 450] {
            cg.write(&base.join("daemon/cgroup.procs"), &p.to_string()).unwrap();
        }
        // ptyd spawns a session; it forks before the daemon's move lands.
        procs.add(301, 300, "claude");
        procs.add(302, 301, "node");
        for p in [301, 302] {
            cg.write(&base.join("daemon/cgroup.procs"), &p.to_string()).unwrap();
        }
        join_with(&cg, &l, 301).unwrap();
        assert_eq!(sorted(cg.procs(&base.join("daemon"))), vec![100, 300, 302, 450]);
        let s = sweep_with(&cg, &procs, &l);
        assert_eq!((s.moved.clone(), s.failed.len()), (vec![302], 0));
        assert_eq!(sorted(cg.procs(&base.join("daemon"))), vec![100, 300, 450], "ptyd and the service stay");
        assert_eq!(sorted(cg.procs(&base.join("agents"))), vec![301, 302]);
        let i = inspect_with(&cg, &procs, &base);
        assert_eq!((i.sessions, i.sessions_in_leaf, i.misplaced), (1, 1, 0));
        assert_eq!(sweep_with(&cg, &procs, &l), Swept::default(), "nothing more to move");
    }

    #[test]
    fn refusals() {
        let cg = FakeCg::new(UID);
        let procs = FakeProcs::default();
        systemd_start(&cg, Guard::Subgroup, 100).unwrap();
        assert!(setup_with(&cg, &procs, &cg.root, UNIT, 0, "max", Guard::Subgroup).is_err(), "root");
        assert!(setup_with(&cg, &procs, &cg.root, "/", UID, "max", Guard::Subgroup).is_err());
        let user = "/user.slice/user-1000.slice/user@1000.service/app.slice/k2.service";
        assert!(setup_with(&cg, &procs, &cg.root, user, UID, "max", Guard::Subgroup).unwrap_err().contains("user-scope"));
        assert!(setup_with(&cg, &procs, &cg.root, "/system.slice/none.service", UID, "max", Guard::Subgroup).unwrap_err().contains("subtree control"));
        let other = FakeCg { root: cg.root.clone(), uid: UID + 1 };
        assert!(setup_with(&other, &procs, &cg.root, UNIT, UID, "max", Guard::Subgroup).unwrap_err().contains("isn't delegated"));
        std::mem::forget(other);
    }

    #[test]
    fn unit_show_dropins_and_slices() {
        let shown = "DelegateSubgroup=daemon\nExecStopPost=\nSlice=k2cores.slice\nControlGroup=/system.slice/k2-daemon.service\n";
        let u = parse_show(shown);
        assert_eq!(u.guard, Guard::Subgroup);
        assert_eq!(u.slice.as_deref(), Some("k2cores.slice"));
        assert_eq!(u.control_group.as_deref().and_then(slice_of).as_deref(), Some("system.slice"), "a restart keeps the old slice");
        let pre = "DelegateSubgroup=\nExecStopPost={ path=/bin/sh ; argv[]=/bin/sh -c f=... k2-agents-leaf-clear ; ignore_errors=yes ; start_time=[n/a] }\nSlice=k2cores.slice\nControlGroup=/k2cores.slice/k2-daemon.service\n";
        let u = parse_show(pre);
        assert_eq!(u.guard, Guard::StopClear);
        assert_eq!(u.control_group.as_deref().and_then(slice_of), u.slice);
        // systemd < 254 prints no DelegateSubgroup at all.
        assert_eq!(parse_show("ExecStopPost=\nSlice=system.slice\nControlGroup=\n").guard, Guard::None);
        assert_eq!(parse_show("DelegateSubgroup=agents\n").guard, Guard::None, "only the daemon leaf");
        assert_eq!(guard_from_dropin("# x\n[Service]\nDelegateSubgroup=daemon\n"), Guard::Subgroup);
        assert_eq!(guard_from_dropin("# DelegateSubgroup=daemon\n[Service]\n"), Guard::None);
        assert_eq!(guard_from_dropin("[Service]\nExecStopPost=+-/bin/sh -c 'x' k2-agents-leaf-clear\n"), Guard::StopClear);
        assert_eq!(guard_from_dropin("[Service]\nExecStartPre=+-/bin/sh -c 'x' k2-agents-leaf-clear\n"), Guard::None, "only after a stop");
        assert_eq!(slice_of("/a.slice/a-b.slice/x.service").as_deref(), Some("a-b.slice"));
        assert_eq!(slice_of("/x.service").as_deref(), Some("-.slice"));
        assert_eq!(slice_of("x"), None);
        assert_eq!(parse_stat_ppid("301 (tmux: server) S 300 301 301 0 -1"), Some(300));
        assert_eq!(parse_stat_ppid("5 (a) b) R 77 5"), Some(77));
        assert_eq!(parse_stat_ppid("garbage"), None);
        let f = doctor_fields(&Inspect { controllers_on: vec!["memory".into()], ..Inspect::default() }, Guard::None);
        assert_eq!(f["armed"], true);
        assert_eq!(doctor_fields(&Inspect { controllers_on: vec!["memory".into()], ..Inspect::default() }, Guard::Subgroup)["armed"], false);
    }
}
