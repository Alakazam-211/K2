//! S17 / AR18: agent sessions get their own memory budget inside the
//! daemon's unit (`prd-k2-app-runtime-v1` §6.1).
//!
//! Agent sessions are the daemon's own fork children, so a non-root daemon
//! can't put them in a sibling system slice. Box prep's `apps cores-slice`
//! drop-in sets `Delegate=yes` on the daemon unit; with it the daemon:
//! 1. moves every process in its unit's cgroup (itself, and what a hand-off
//!    left there: frpc, published services) into `<unit>/daemon/` (cgroup
//!    v2's no-internal-process rule);
//! 2. enables `+memory +pids` (and `+cpu` where it can) for its children;
//! 3. makes `<unit>/agents/` with `memory.high` = the agents' budget, and
//!    moves each terminal session's child into it right after the spawn.
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
//! path, so the tests use a temp dir.

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

/// `MemTotal` from `/proc/meminfo` text, in bytes.
pub fn mem_total(meminfo: &str) -> Option<u64> {
    let l = meminfo.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = l.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

fn write(p: &Path, s: &str) -> Result<(), String> {
    std::fs::write(p, s).map_err(|e| format!("write {}: {e}", p.display()))
}

/// Set up the leaves under `fs_root` (normally `/sys/fs/cgroup`) for the
/// process cgroup `self_path`, owned by `uid`. Fails (and changes nothing
/// it can't undo) unless the unit's cgroup is a system unit's, delegated
/// to `uid`.
pub fn setup_at(fs_root: &Path, self_path: &str, uid: u32, memory_high: &str) -> Result<Leaves, String> {
    if uid == 0 {
        return Err("the daemon runs as root (no delegated unit to split)".into());
    }
    let rel = unit_base(self_path).ok_or("the daemon is in the root cgroup")?;
    if rel.contains("/user@") {
        return Err("a user-scope daemon (its unit isn't delegated by box prep)".into());
    }
    let base = fs_root.join(rel.trim_start_matches('/'));
    let sc = base.join("cgroup.subtree_control");
    if !sc.exists() {
        return Err(format!("{} has no subtree control (cgroup v2 with Delegate=yes)", base.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let owner = std::fs::metadata(&base).map_err(|e| format!("{}: {e}", base.display()))?.uid();
        if owner != uid {
            return Err("the daemon's unit isn't delegated to it (box prep: k2 app box prepare --cores-mem-min, then restart the daemon)".into());
        }
    }
    let daemon = base.join("daemon");
    let agents = base.join("agents");
    for d in [&daemon, &agents] {
        std::fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
    }
    // Everything still in the unit's own cgroup moves into daemon/ (a
    // process can't stay in a cgroup that hands controllers down).
    let procs = std::fs::read_to_string(base.join("cgroup.procs")).unwrap_or_default();
    for pid in procs.split_whitespace() {
        // A process that exited in between is gone already: fine.
        let _ = write(&daemon.join("cgroup.procs"), pid);
    }
    write(&sc, "+memory +pids")?;
    let _ = write(&sc, "+cpu");
    write(&agents.join("memory.high"), memory_high)?;
    Ok(Leaves { base, memory_high: memory_high.to_string() })
}

/// Move `pid` (a terminal session's child) into the agents' leaf.
pub fn join_at(l: &Leaves, pid: u32) -> Result<(), String> {
    write(&l.base.join("agents").join("cgroup.procs"), &pid.to_string())
}

// ── the process-wide state ───────────────────────────────────────────

static STATE: OnceLock<Result<Leaves, String>> = OnceLock::new();

/// Boot (the daemon, once): set up the leaves when this daemon runs in a
/// delegated system unit. Never fails the boot.
pub fn init() {
    let _ = STATE.get_or_init(|| {
        if !cfg!(target_os = "linux") {
            return Err("Linux only".into());
        }
        let text = std::fs::read_to_string("/proc/self/cgroup").map_err(|e| format!("/proc/self/cgroup: {e}"))?;
        let path = parse_self_cgroup(&text).ok_or("no cgroup v2 path")?;
        let total = std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| mem_total(&m));
        let high = budget(std::env::var("K2_AGENTS_MEMORY_HIGH").ok().as_deref(), total)?;
        #[cfg(unix)]
        let uid = unsafe { libc::geteuid() };
        #[cfg(not(unix))]
        let uid = 0;
        let r = setup_at(Path::new("/sys/fs/cgroup"), &path, uid, &high);
        match &r {
            Ok(l) => crate::log_debug!("[agents] agent sessions get their own leaf {} (memory.high {})", l.base.join("agents").display(), l.memory_high),
            Err(e) => crate::log_debug!("[agents] no agents leaf: {e}"),
        }
        r
    });
}

/// After a terminal session spawns: its child joins the agents' leaf.
pub fn join(pid: u32) {
    if let Some(Ok(l)) = STATE.get() {
        if let Err(e) = join_at(l, pid) {
            crate::log_debug!("[agents] session {pid} not moved into the agents leaf: {e}");
        }
    }
}

/// For `k2 app doctor`: the leaf, or why there is none.
pub fn status_json() -> serde_json::Value {
    match STATE.get() {
        Some(Ok(l)) => serde_json::json!({ "on": true, "leaf": l.base.join("agents"), "memoryHigh": l.memory_high }),
        Some(Err(e)) => serde_json::json!({ "on": false, "why": e }),
        None => serde_json::json!({ "on": false, "why": "not set up (the daemon sets it up at boot)" }),
    }
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

    // Server-only feature (Linux boxes): systemd cgroup delegation, checked
    // by uid; not shipped on personal Windows machines.
    #[cfg(unix)]
    #[test]
    fn a_delegated_unit_is_split_into_daemon_and_agents() {
        let d = std::env::temp_dir().join(format!("k2-agents-leaf-{}", uuid::Uuid::new_v4().simple()));
        let unit = d.join("k2cores.slice/k2-daemon.service");
        std::fs::create_dir_all(&unit).unwrap();
        std::fs::write(unit.join("cgroup.subtree_control"), "").unwrap();
        std::fs::write(unit.join("cgroup.procs"), "100\n200\n").unwrap();
        #[cfg(unix)]
        let uid = unsafe { libc::geteuid() };
        #[cfg(not(unix))]
        let uid = 1000;
        if uid == 0 {
            let _ = std::fs::remove_dir_all(&d);
            return;
        }
        let l = setup_at(&d, "/k2cores.slice/k2-daemon.service", uid, "6442450944").unwrap();
        assert_eq!(l.base, unit);
        // The temp dir isn't a cgroup fs: each write replaces the file, so
        // the last pid and the last controller line are what remain.
        assert_eq!(std::fs::read_to_string(unit.join("daemon/cgroup.procs")).unwrap(), "200");
        assert_eq!(std::fs::read_to_string(unit.join("cgroup.subtree_control")).unwrap(), "+cpu");
        assert_eq!(std::fs::read_to_string(unit.join("agents/memory.high")).unwrap(), "6442450944");
        join_at(&l, 4242).unwrap();
        assert_eq!(std::fs::read_to_string(unit.join("agents/cgroup.procs")).unwrap(), "4242");
        // Refusals: root, the root cgroup, a user-scope unit, no subtree
        // control, a cgroup owned by someone else.
        assert!(setup_at(&d, "/k2cores.slice/k2-daemon.service", 0, "max").is_err());
        assert!(setup_at(&d, "/", uid, "max").is_err());
        assert!(setup_at(&d, "/user.slice/user-1000.slice/user@1000.service/app.slice/k2.service", uid, "max").unwrap_err().contains("user-scope"));
        assert!(setup_at(&d, "/system.slice/none.service", uid, "max").unwrap_err().contains("subtree control"));
        assert!(setup_at(&d, "/k2cores.slice/k2-daemon.service", uid + 1, "max").unwrap_err().contains("isn't delegated"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
