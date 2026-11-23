//! `agents` namespace (protocol 18, `namespaces.agentcg`; Agent Isolation
//! M1b, `k2_root_proto::agent_cgroup`): root-owned per-agent cgroups.
//!
//! `agents place <workspace id> <pid>` (daemon class):
//! 1. The daemon's unit is the caller's own: its main process (proven by the
//!    caller walk) → `/proc/<main>/cgroup` → minus the `daemon/` leaf. A
//!    user-scope daemon, the root cgroup or a non-`.service` refuses. No path
//!    or cgroup ever comes from argv.
//! 2. `<unit>/agents` must be a cgroup v2 directory the daemon's user owns
//!    (the delegated unit; the daemon made it).
//! 3. `pid` must be a process of the daemon's user, waiting in the exec
//!    helper (`argv[1] == --agent-exec`: it has run nothing of the agent
//!    yet), in the daemon's unit and in no other agent's cgroup. It is held
//!    by a pidfd from before the checks to after the move.
//! 4. `agents/a-<id>` is made by root if missing. One the daemon's user owns
//!    (made before the helper could, or by an agent that removed an empty
//!    root-owned one) is emptied into `agents/` and made again by root:
//!    chowning it would keep a writable fd someone opened earlier usable
//!    (cgroup v2 checks the opener at write time against the common
//!    ancestor, `agents/`, which is the user's).
//! 5. `pid` moves into it. The answer names the cgroup.
//!
//! Never takes more than one pid, never moves anything but that pid and
//! (step 4) the processes squatting in a user-owned `a-<id>`.

use std::path::{Path, PathBuf};

use k2_root_proto::agent_cgroup::{argv1, may_place_from, start_of_stat, unit_of, AgentWs, AGENTS_LEAF, EXEC_HELPER_ARG};

use crate::caller::Caller;
use crate::ctx::{Ctx, Fail, HResult};

pub(crate) const CGROUP_FS: &str = "/sys/fs/cgroup";
#[cfg(target_os = "linux")]
const CGROUP2_SUPER_MAGIC: i64 = 0x6367_7270;

pub(crate) fn cgroup_line(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix("0::")).map(|p| p.trim().to_string()).filter(|p| p.starts_with('/'))
}

fn read(ctx: &Ctx, abs: &str) -> HResult<Vec<u8>> {
    std::fs::read(ctx.p(abs)).map_err(|e| Fail::state(format!("agents place refused: {abs} unreadable ({})", e.kind())))
}

pub(crate) fn owner_of(p: &Path) -> HResult<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(p).map(|m| m.uid()).map_err(|e| Fail::io_err("stat agent cgroup", e))
}

#[cfg(target_os = "linux")]
pub(crate) fn is_cgroup2(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return false };
    // SAFETY: statfs into a zeroed struct for a NUL-terminated path.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statfs(c.as_ptr(), &mut st) } == 0) && st.f_type as i64 == CGROUP2_SUPER_MAGIC
}
#[cfg(not(target_os = "linux"))]
pub(crate) fn is_cgroup2(_p: &Path) -> bool {
    false
}

pub(crate) fn pids_in(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("cgroup.procs")).unwrap_or_default().split_whitespace().map(str::to_string).collect()
}

fn write_pid(dir: &Path, pid: &str) -> std::io::Result<()> {
    std::fs::write(dir.join("cgroup.procs"), pid)
}

/// Make `dir` a root-owned cgroup: as it is when root's, else (a K2-user
/// one) emptied into `leaf` and made again. `Ok(true)` when (re)made.
fn ensure_root_owned(ctx: &Ctx, leaf: &Path, dir: &Path) -> HResult<bool> {
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => {
            if !ctx.real_owners || owner_of(dir)? == 0 {
                return Ok(false);
            }
            // A K2-user-owned a-<id>: move what's in it out, then remake.
            for _ in 0..8 {
                let pids = pids_in(dir);
                if pids.is_empty() {
                    break;
                }
                for p in pids {
                    match write_pid(leaf, &p) {
                        Ok(()) => {}
                        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {}
                        Err(e) => return Err(Fail::io_err("empty a user-owned agent cgroup", e)),
                    }
                }
            }
            std::fs::remove_dir(dir).map_err(|e| Fail::io_err("remove a user-owned agent cgroup", e))?;
        }
        Ok(_) => return Err(Fail::state("agents place refused: the agent cgroup is not a directory")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Fail::io_err("stat agent cgroup", e)),
    }
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        // Made in between by the same placement for another session.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if ctx.real_owners && owner_of(dir)? != 0 {
                return Err(Fail::state("agents place refused: the agent cgroup was made again by someone else"));
            }
        }
        Err(e) => return Err(Fail::io_err("create agent cgroup", e)),
    }
    Ok(true)
}

/// The caller's system unit: its `daemon.toml` entry, the daemon user's
/// uid, and the unit's cgroup from the caller's own main process (minus
/// the `daemon/` leaf). Shared with protocol 19's scopes, which need no
/// delegation.
pub(crate) fn daemon_unit(ctx: &Ctx, who: &Caller) -> HResult<(crate::caller::DaemonEntry, String, u32)> {
    let entry = crate::daemon_unit::caller_entry(ctx, who)?;
    if entry.scope != "system" {
        return Err(Fail::state("agents place refused: a user-scope daemon has no delegated unit"));
    }
    let daemon_uid = ctx.procs.uid_of_user(&entry.user).ok_or_else(|| Fail::state("agents place refused: the daemon user is unknown"))?;
    let main_cg = cgroup_line(&String::from_utf8_lossy(&read(ctx, &format!("/proc/{}/cgroup", who.pid))?))
        .ok_or_else(|| Fail::state("agents place refused: the daemon has no cgroup v2 path"))?;
    let unit = unit_of(&main_cg).ok_or_else(|| Fail::state("agents place refused: the daemon is not in a system unit"))?;
    Ok((entry, unit, daemon_uid))
}

/// Steps 1 and 2, shared by `place` and `check`: the caller's unit and its
/// agents' leaf, and the daemon user's uid.
fn unit_and_leaf(ctx: &Ctx, who: &Caller) -> HResult<(String, PathBuf, u32)> {
    // 1. The daemon's unit, from its own main process.
    let (_, unit, daemon_uid) = daemon_unit(ctx, who)?;
    // 2. The agents' leaf, the daemon's own.
    let leaf: PathBuf = ctx.p(&format!("{CGROUP_FS}{unit}/{AGENTS_LEAF}"));
    if !leaf.is_dir() || !leaf.join("cgroup.procs").exists() {
        return Err(Fail::state("agents place refused: the daemon's agents leaf is missing (agents_leaf off)"));
    }
    if ctx.real_owners {
        if !is_cgroup2(&leaf) {
            return Err(Fail::state("agents place refused: the agents leaf is not on cgroup v2"));
        }
        if owner_of(&leaf)? != daemon_uid {
            return Err(Fail::state("agents place refused: the agents leaf is not the daemon's (no Delegate=yes)"));
        }
    }
    Ok((unit, leaf, daemon_uid))
}

/// `agents check`: `place`'s caller and unit checks, nothing moved.
pub fn check(ctx: &Ctx, who: &Caller) -> HResult<serde_json::Value> {
    let (unit, _leaf, _) = unit_and_leaf(ctx, who)?;
    Ok(serde_json::json!({ "ok": true, "unit": unit, "agents": format!("{unit}/{AGENTS_LEAF}") }))
}

/// `agents place <ws> <pid> <start>`.
pub fn place(ctx: &Ctx, who: &Caller, ws: &AgentWs, pid: u32, start: u64) -> HResult<serde_json::Value> {
    let (unit, leaf, daemon_uid) = unit_and_leaf(ctx, who)?;
    // 3. The process: held first, checked, moved, still the same after.
    let held = ctx.procs.hold(pid as i32).ok_or_else(|| Fail::state("agents place refused: no such process"))?;
    // The one the daemon started (review B5): its start time, as its parent
    // read it at spawn, must match; a pid reused since is another process.
    let stat = String::from_utf8_lossy(&read(ctx, &format!("/proc/{pid}/stat"))?).into_owned();
    if start_of_stat(&stat) != Some(start) {
        return Err(Fail::state("agents place refused: the process is not the one the daemon started (start time differs)"));
    }
    if ctx.procs.ruid_of(pid as i32) != Some(daemon_uid) {
        return Err(Fail::state("agents place refused: the process is not the daemon user's"));
    }
    let cmdline = read(ctx, &format!("/proc/{pid}/cmdline"))?;
    if argv1(&cmdline) != Some(EXEC_HELPER_ARG) {
        return Err(Fail::state("agents place refused: the process is not waiting in the exec helper"));
    }
    let cur = cgroup_line(&String::from_utf8_lossy(&read(ctx, &format!("/proc/{pid}/cgroup"))?))
        .ok_or_else(|| Fail::state("agents place refused: the process has no cgroup v2 path"))?;
    if !may_place_from(&unit, ws, &cur) {
        return Err(Fail::state("agents place refused: the process is outside the daemon's unit or in another agent's cgroup"));
    }
    // 4. The agent's cgroup, root's.
    let dir = leaf.join(ws.cgroup_name());
    let remade = ensure_root_owned(ctx, &leaf, &dir)?;
    // 5. The move.
    write_pid(&dir, &pid.to_string()).map_err(|e| Fail::io_err("place the process", e))?;
    if !held.alive() {
        return Err(Fail::state("agents place refused: the process exited during the move"));
    }
    Ok(serde_json::json!({
        "ok": true,
        "cgroup": format!("{unit}/{AGENTS_LEAF}/{}", ws.cgroup_name()),
        "pid": pid,
        "remade": remade,
    }))
}
