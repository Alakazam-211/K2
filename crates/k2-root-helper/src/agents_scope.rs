//! `agents` namespace, protocol 19 (`namespaces.agentscope`; Agent
//! Isolation M2 B4, `k2_root_proto::agent_scope`): one root-owned systemd
//! scope per agent session, in K2's agent slices, outside the daemon's
//! unit.
//!
//! - `agents scope <ws> <id> <pid> <start> <open|fenced> [mem=N] [cpu=N]`
//!   (daemon): the daemon's unit comes from the caller's main process
//!   (never argv); `pid` must be held by a pidfd, started at `start`, the
//!   daemon user's, waiting in the exec helper, and in the daemon's unit
//!   (not in another workspace's old-layout cgroup). The unit need not be
//!   delegated. The kind's slice must exist and be root's. Then root asks
//!   systemd (`StartTransientUnit`, mode `fail`) for
//!   `k2-agent-<ws>:<id>.scope` with `PIDs=[pid]`, `Delegate=false`,
//!   `CollectMode=inactive-or-failed`, a 10 s stop timeout, a fixed
//!   `Description` and the caps as numbers. After the move the scope's own
//!   `cgroup.procs` (root's, read through a handle) must list exactly that
//!   pid, still held, alive and started at `<start>`; otherwise the scope
//!   is stopped (`--no-block`), the mismatch audited, and the call
//!   refused.
//! - `agents adopt <ws> <id> <open|fenced>` (daemon): every process of the
//!   daemon's user left in a **root-owned** `<unit>/agents/a-<ws>` (a
//!   session M1b placed that lived through an update) into one new scope.
//!   The folder and its `cgroup.procs` are read through one handle; a
//!   folder the server user owns is never adopted (anyone of that user
//!   could have written into it). After the move every pid the new
//!   scope lists must be one the helper holds, alive, with the start time
//!   read before the move (or a child such a process forked since); else
//!   the whole scope is stopped, which ends that agent's surviving
//!   sessions too.
//! - `agents stop <ws> <id>` / `agents stop-all` (daemon): `systemctl stop
//!   --no-block` for names the classifier accepts, found under the two
//!   slices only, and an answer at once: the helper's lock is never held
//!   while a scope ends (systemd's own 10 s stop timeout escalates to a
//!   kill).
//! - `agents layout` (daemon): report only, with the P1 probe run as the
//!   daemon's user.
//! - `agents setup` / `agents teardown` (daemon): the slice and anchor
//!   units, the bus policy and the daemon unit's `Delegate=no` drop-in
//!   (`agents_scope_policy`).

use std::ffi::OsStr;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use k2_root_proto::agent_cgroup::{argv1, pid_ok, start_of_stat, AgentWs, AGENTS_LEAF, DAEMON_LEAF, EXEC_HELPER_ARG, PREFIX};
use k2_root_proto::agent_scope::{
    classify_probe, may_scope_from, parse_scope_name, probe_scope_name, scope_cgroup, scope_name, slice_cgroup, start_transient_argv, unit_files,
    Caps, Kind, Level, Probe, ScopeId, ScopeVerb, ADOPT_MAX_PIDS, ADOPTED_DESCRIPTION, OPEN_SLICE, SLEEP_PATHS, TOP_SLICE,
};
use k2_root_proto::apps::ENV;
use k2_root_proto::creds::{parse_unit_path, BUSCTL};
use k2_root_proto::paths::{RUNUSER, SYSTEMCTL, SYSTEMD_RUN, SYSTEMD_SYSTEM_DIR};
use serde_json::json;

use crate::agents::{cgroup_line, daemon_unit, is_cgroup2, owner_of, pids_in, CGROUP_FS};
use crate::caller::Caller;
use crate::ctx::{Ctx, Fail, HResult, ProcHandle};

type J = serde_json::Value;

/// How long the helper waits for systemd to move the pids after the start
/// job is queued (polled; the first look is immediate).
const MOVE_WAIT_TRIES: u32 = 25;
const MOVE_WAIT_STEP: Duration = Duration::from_millis(20);
/// How often `agents teardown` looks whether a stopped scope's cgroup is
/// gone, and how long it looks again after a kill.
const STOP_WAIT_STEP: Duration = Duration::from_millis(50);
const AFTER_KILL_WAIT: Duration = Duration::from_millis(1000);
/// ptyd's process name (= k2-core `agents_leaf`'s check): never adopted.
const PTYD_COMM: &str = "k2-ptyd";
/// Most bytes read from one cgroup's `cgroup.procs`.
const PROCS_CAP: u64 = 1 << 20;
/// How many parents up an adopted scope's unlisted pid may be from a
/// checked one (a child a survivor forked after the move).
const CHILD_HOPS: usize = 64;

pub fn run(ctx: &Ctx, v: &ScopeVerb, who: &Caller) -> HResult<J> {
    match v {
        ScopeVerb::Scope { ws, id, pid, start, kind, caps } => scope(ctx, who, ws, id, *pid, *start, *kind, caps),
        ScopeVerb::Adopt { ws, id, kind } => adopt(ctx, who, ws, id, *kind),
        ScopeVerb::Stop { ws, id } => stop(ctx, ws, id),
        ScopeVerb::StopAll => stop_all(ctx),
        ScopeVerb::Layout => layout(ctx, who),
        ScopeVerb::Setup => setup(ctx, who),
        ScopeVerb::Teardown => teardown(ctx, who),
    }
}

fn cg(abs: &str) -> String {
    format!("{CGROUP_FS}{abs}")
}

fn read_proc(ctx: &Ctx, abs: &str) -> HResult<Vec<u8>> {
    std::fs::read(ctx.p(abs)).map_err(|e| Fail::state(format!("agents scope refused: {abs} unreadable ({})", e.kind())))
}

fn cgroup_of(ctx: &Ctx, pid: u32) -> Option<String> {
    std::fs::read(ctx.p(&format!("/proc/{pid}/cgroup"))).ok().and_then(|b| cgroup_line(&String::from_utf8_lossy(&b)))
}

/// Field 22 of `/proc/<pid>/stat`; `None` when it can't be read (gone).
fn start_now(ctx: &Ctx, pid: u32) -> Option<u64> {
    std::fs::read(ctx.p(&format!("/proc/{pid}/stat"))).ok().and_then(|b| start_of_stat(&String::from_utf8_lossy(&b)))
}

fn is_dir(p: &std::path::Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// The top slice and `kind`'s slice exist, are cgroup v2 and root's.
fn require_slice(ctx: &Ctx, kind: Kind) -> HResult<()> {
    for l in [Level::Top, Level::Agents(kind)] {
        let d = ctx.p(&cg(&l.cgroup()));
        if !is_dir(&d) {
            return Err(Fail::state("agents scope refused: the agent slices are not set up (agents setup)"));
        }
        if ctx.real_owners && (!is_cgroup2(&d) || owner_of(&d)? != 0) {
            return Err(Fail::state("agents scope refused: an agent slice is not root's cgroup"));
        }
    }
    Ok(())
}

/// The exec helper's process, held, with every check `agents place` makes
/// (start time, user, argv[1]) and [`may_scope_from`] for where it is.
fn held_exec(ctx: &Ctx, unit: &str, ws: &AgentWs, pid: u32, start: u64, uid: u32) -> HResult<Box<dyn ProcHandle>> {
    let held = ctx.procs.hold(pid as i32).ok_or_else(|| Fail::state("agents scope refused: no such process"))?;
    let stat = String::from_utf8_lossy(&read_proc(ctx, &format!("/proc/{pid}/stat"))?).into_owned();
    if start_of_stat(&stat) != Some(start) {
        return Err(Fail::state("agents scope refused: the process is not the one the daemon started (start time differs)"));
    }
    if ctx.procs.ruid_of(pid as i32) != Some(uid) {
        return Err(Fail::state("agents scope refused: the process is not the daemon user's"));
    }
    if argv1(&read_proc(ctx, &format!("/proc/{pid}/cmdline"))?) != Some(EXEC_HELPER_ARG) {
        return Err(Fail::state("agents scope refused: the process is not waiting in the exec helper"));
    }
    let cur = cgroup_of(ctx, pid).ok_or_else(|| Fail::state("agents scope refused: the process has no cgroup v2 path"))?;
    if !may_scope_from(unit, ws, &cur) {
        return Err(Fail::state("agents scope refused: the process is outside the daemon's unit or in another workspace's cgroup"));
    }
    Ok(held)
}

/// `StartTransientUnit` as root over the system bus.
fn start_scope(ctx: &Ctx, name: &str, kind: Kind, pids: &[u32], caps: &Caps, adopted: bool) -> HResult<()> {
    let args = start_transient_argv(name, kind, pids, caps, adopted);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    ctx.run_ok(BUSCTL, &refs, None)
        .map(|_| ())
        .map_err(|f| Fail::state(format!("agents scope refused: systemd did not start the scope ({})", f.msg)))
}

/// Which of `pids` sit in `want` (polled until all do or the wait ends).
fn wait_moved(ctx: &Ctx, pids: &[u32], want: &str) -> Vec<u32> {
    let mut moved = Vec::new();
    for i in 0..MOVE_WAIT_TRIES {
        moved = pids.iter().copied().filter(|p| cgroup_of(ctx, *p).as_deref() == Some(want)).collect();
        if moved.len() == pids.len() {
            break;
        }
        if i + 1 < MOVE_WAIT_TRIES {
            std::thread::sleep(MOVE_WAIT_STEP);
        }
    }
    moved
}

/// `systemctl stop --no-block`: every stop outside `teardown`, the failure
/// paths too, so nothing waits for a scope to end under the helper's lock.
fn stop_no_block(ctx: &Ctx, name: &str) -> HResult<()> {
    ctx.run_ok(SYSTEMCTL, &["stop", "--no-block", name], None).map(|_| ())
}

/// One pid the new scope lists that isn't the process the helper checked.
struct Mismatch {
    pid: u32,
    start_before: Option<u64>,
    start_now: Option<u64>,
}

/// The loud line for a scope stopped because a pid in it is not the
/// process checked before the move (review N1): every pid with both start
/// times, the workspace and the scope, on the helper's audit sink.
fn audit_mismatch(ctx: &Ctx, verb: &str, ws: &AgentWs, name: &str, found: &[Mismatch]) {
    let pids: Vec<J> = found.iter().map(|m| json!({ "pid": m.pid, "startBefore": m.start_before, "startNow": m.start_now })).collect();
    let line = json!({
        "ns": "agents",
        "verb": format!("agents {verb}"),
        "event": "scope_member_mismatch",
        "result": "scope_stopped",
        "ws": ws.as_str(),
        "scope": name,
        "pids": pids,
    });
    ctx.audit.line(&line.to_string());
}

/// The new scope's members, read through a handle (no link followed) on
/// its root-owned cgroup; `None` when it can't be read or isn't root's.
fn scope_members(ctx: &Ctx, kind: Kind, ws: &AgentWs, id: &ScopeId) -> Option<Vec<u32>> {
    let c = read_cgroup(ctx, &cg(&scope_cgroup(kind, ws, id))).ok()??;
    if !c.root_owned(ctx) {
        return None;
    }
    c.pids.iter().map(|p| pid_ok(p)).collect()
}

#[allow(clippy::too_many_arguments)]
fn scope(ctx: &Ctx, who: &Caller, ws: &AgentWs, id: &ScopeId, pid: u32, start: u64, kind: Kind, caps: &Caps) -> HResult<J> {
    let (_, unit, uid) = daemon_unit(ctx, who)?;
    let held = held_exec(ctx, &unit, ws, pid, start, uid)?;
    require_slice(ctx, kind)?;
    let name = scope_name(ws, id);
    let want = scope_cgroup(kind, ws, id);
    start_scope(ctx, &name, kind, &[pid], caps, false)?;
    let placed = wait_moved(ctx, &[pid], &want) == [pid];
    if !placed {
        let _ = stop_no_block(ctx, &name);
        return Err(Fail::state("agents scope refused: the process is not in its scope after the start (scope stopped)"));
    }
    // systemd got a number (review R3, N1): the scope must hold exactly the
    // process checked, still alive and started at `start`.
    let members = scope_members(ctx, kind, ws, id);
    let mut found: Vec<Mismatch> = Vec::new();
    match &members {
        None => found.push(Mismatch { pid, start_before: Some(start), start_now: None }),
        Some(list) => {
            for p in list {
                let now = start_now(ctx, *p);
                if *p != pid || !held.alive() || now != Some(start) {
                    found.push(Mismatch { pid: *p, start_before: (*p == pid).then_some(start), start_now: now });
                }
            }
            if !list.contains(&pid) {
                found.push(Mismatch { pid, start_before: Some(start), start_now: start_now(ctx, pid) });
            }
        }
    }
    if !found.is_empty() {
        let _ = stop_no_block(ctx, &name);
        audit_mismatch(ctx, "scope", ws, &name, &found);
        return Err(Fail::state(if members.is_none() {
            "agents scope refused: the scope's member list can't be read as root's (scope stopped)"
        } else if !held.alive() {
            "agents scope refused: the process exited during the move (scope stopped)"
        } else if found.iter().all(|m| m.pid == pid) {
            "agents scope refused: the process's start time changed during the move (scope stopped)"
        } else {
            "agents scope refused: the scope holds a process other than the one checked (scope stopped)"
        }));
    }
    Ok(json!({ "ok": true, "cgroup": want, "pid": pid, "scope": name, "kind": kind.as_str() }))
}

/// A cgroup as read through one handle: who owns the folder and its
/// `cgroup.procs` (each `fstat`ed on the open handle), and the pids listed.
struct CgroupRead {
    owner: u32,
    procs_owner: u32,
    pids: Vec<String>,
}

impl CgroupRead {
    fn root_owned(&self, ctx: &Ctx) -> bool {
        self.owner == ctx.root_uid && self.procs_owner == ctx.root_uid
    }
}

/// Open `abs` (every component without following a link, `O_DIRECTORY`),
/// `fstat` it, open `cgroup.procs` beneath that handle (no link), `fstat`
/// it and read it. `Ok(None)` when the folder is not there.
fn read_cgroup(ctx: &Ctx, abs: &str) -> HResult<Option<CgroupRead>> {
    let d = match ctx.dir(abs) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Fail::io_err("agents adopt refused: open the old-layout cgroup", e)),
    };
    let st = d.stat_self().map_err(|e| Fail::io_err("stat the old-layout cgroup", e))?;
    let (f, pst) = d
        .open_regular(OsStr::new("cgroup.procs"))
        .map_err(|e| Fail::io_err("open the old cgroup's cgroup.procs", e))?
        .ok_or_else(|| Fail::state("agents adopt refused: the old-layout cgroup has no cgroup.procs"))?;
    let mut text = String::new();
    f.take(PROCS_CAP).read_to_string(&mut text).map_err(|e| Fail::io_err("read the old cgroup's cgroup.procs", e))?;
    Ok(Some(CgroupRead { owner: st.st_uid, procs_owner: pst.st_uid, pids: text.split_whitespace().map(str::to_string).collect() }))
}

/// Pids that are never adopted: the caller (the daemon's main process),
/// whatever sits in the unit's root or its `daemon/` leaf, and ptyd.
fn never_adopted(ctx: &Ctx, who: &Caller, unit: &str, pid: u32) -> bool {
    if i64::from(pid) == i64::from(who.pid) {
        return true;
    }
    let in_unit = [cg(unit), cg(&format!("{unit}/{DAEMON_LEAF}"))].iter().any(|d| pids_in(&ctx.p(d)).iter().any(|p| *p == pid.to_string()));
    let comm = std::fs::read_to_string(ctx.p(&format!("/proc/{pid}/comm"))).unwrap_or_default();
    in_unit || comm.trim() == PTYD_COMM
}

fn adopt(ctx: &Ctx, who: &Caller, ws: &AgentWs, id: &ScopeId, kind: Kind) -> HResult<J> {
    let (_, unit, uid) = daemon_unit(ctx, who)?;
    require_slice(ctx, kind)?;
    let old_rel = format!("{unit}/{AGENTS_LEAF}/{}", ws.cgroup_name());
    let old = read_cgroup(ctx, &cg(&old_rel))?.ok_or_else(|| Fail::state("agents adopt refused: no old-layout cgroup for this workspace"))?;
    // Review R1: only a cgroup root made (M1b's helper-placed ones). One
    // the server user owns may hold anything that user put there.
    if !old.root_owned(ctx) {
        return Err(Fail::state(if old.owner == uid || old.procs_owner == uid {
            "agents adopt refused: old layout, not adopted: the cgroup is the server user's"
        } else {
            "agents adopt refused: old layout, not adopted: the cgroup is not root's"
        }));
    }
    if old.pids.is_empty() {
        return Err(Fail::state("agents adopt refused: the old-layout cgroup is empty"));
    }
    if old.pids.len() > ADOPT_MAX_PIDS {
        return Err(Fail::state("agents adopt refused: more processes than one adopt moves"));
    }
    let mut listed = Vec::with_capacity(old.pids.len());
    for p in &old.pids {
        let n = pid_ok(p).ok_or_else(|| Fail::state("agents adopt refused: the old cgroup lists something that is not a pid"))?;
        if never_adopted(ctx, who, &unit, n) {
            return Err(Fail::state("agents adopt refused: the old cgroup lists the daemon's own process or ptyd; nothing moved"));
        }
        listed.push(n);
    }
    // Held, the daemon user's, and each one's start time before the move.
    let mut held: Vec<(u32, Box<dyn ProcHandle>, u64)> = Vec::new();
    for n in listed {
        // One that exited since the list was read is skipped.
        let Some(h) = ctx.procs.hold(n as i32) else { continue };
        let Some(start) = start_now(ctx, n) else { continue };
        match ctx.procs.ruid_of(n as i32) {
            None => continue,
            Some(u) if u == uid => held.push((n, h, start)),
            Some(_) => return Err(Fail::state("agents adopt refused: a process there is not the daemon user's")),
        }
    }
    if held.is_empty() {
        return Err(Fail::state("agents adopt refused: every process there has exited"));
    }
    let pids: Vec<u32> = held.iter().map(|(p, _, _)| *p).collect();
    let name = scope_name(ws, id);
    let want = scope_cgroup(kind, ws, id);
    start_scope(ctx, &name, kind, &pids, &Caps::default(), true)?;
    let alive: Vec<u32> = held.iter().filter(|(_, h, _)| h.alive()).map(|(p, _, _)| *p).collect();
    let moved = wait_moved(ctx, &alive, &want);
    if moved.is_empty() {
        let _ = stop_no_block(ctx, &name);
        return Err(Fail::state("agents adopt refused: no process is in the new scope after the start (scope stopped)"));
    }
    // Review R3, N1: check the scope, not our list. Every pid it holds must
    // be one the helper held, still alive, with the start time read before
    // the move; or a child one of those forked since (its parent chain
    // reaches a checked pid inside the scope). A number that died before
    // the move and came back as another process is caught here too.
    let (found, children) = adopt_mismatches(ctx, &held, scope_members(ctx, kind, ws, id));
    if !found.is_empty() {
        let _ = stop_no_block(ctx, &name);
        audit_mismatch(ctx, "adopt", ws, &name, &found);
        let list: Vec<String> = found.iter().map(|m| m.pid.to_string()).collect();
        return Err(Fail::state(format!(
            "agents adopt refused: \"mismatched\":[{}] (not the processes checked before the move); the scope is stopped, \
             so this agent's surviving sessions in it end too",
            list.join(",")
        )));
    }
    // Only a scope that stands keeps a child accepted by its parent chain:
    // one audit line each.
    audit_children(ctx, ws, &name, &children);
    let not_moved: Vec<u32> = alive.iter().copied().filter(|p| !moved.contains(p)).collect();
    let left = pids_in(&ctx.p(&cg(&old_rel))).len();
    Ok(json!({
        "ok": true,
        "scope": name,
        "cgroup": want,
        "kind": kind.as_str(),
        "moved": moved.len(),
        "pids": moved,
        "notMoved": not_moved,
        "mismatched": [],
        "left": left,
    }))
}

/// The adopted scope's members that aren't the processes checked before
/// the move (see `adopt`). Unreadable members are one mismatch (pid 0).
/// A pid the adopted scope lists that the helper didn't hold, accepted as
/// a child of a checked process: its start time now and the parent chain
/// (`via`, from its parent up to the checked one).
struct ChildAccepted {
    pid: u32,
    start_now: Option<u64>,
    via: Vec<u32>,
}

/// The parent chain of `pid` up to a checked process, every step inside the
/// scope's member list (`list`) and never through a held pid (one that
/// failed its own check is in `held` but not in `checked`). `None` when the
/// chain leaves the scope, meets a held pid that failed, or runs past
/// [`CHILD_HOPS`].
fn chain_to_checked(ctx: &Ctx, pid: u32, list: &[u32], checked: &[u32], held: &[u32]) -> Option<Vec<u32>> {
    let mut via = Vec::new();
    let mut cur = pid as i32;
    for _ in 0..CHILD_HOPS {
        let pp = ctx.procs.ppid_of(cur)?;
        if pp <= 1 || !list.contains(&(pp as u32)) {
            return None;
        }
        let ppu = pp as u32;
        via.push(ppu);
        if checked.contains(&ppu) {
            return Some(via);
        }
        if held.contains(&ppu) {
            return None;
        }
        cur = pp;
    }
    None
}

fn audit_children(ctx: &Ctx, ws: &AgentWs, name: &str, children: &[ChildAccepted]) {
    for c in children {
        let line = json!({
            "ns": "agents",
            "verb": "agents adopt",
            "event": "scope_member_child_accepted",
            "ws": ws.as_str(),
            "scope": name,
            "pid": c.pid,
            "startNow": c.start_now,
            "via": c.via,
        });
        ctx.audit.line(&line.to_string());
    }
}

fn adopt_mismatches(ctx: &Ctx, held: &[(u32, Box<dyn ProcHandle>, u64)], members: Option<Vec<u32>>) -> (Vec<Mismatch>, Vec<ChildAccepted>) {
    let Some(list) = members else {
        return (vec![Mismatch { pid: 0, start_before: None, start_now: None }], vec![]);
    };
    let checked: Vec<u32> = held
        .iter()
        .filter(|(p, h, s)| list.contains(p) && h.alive() && start_now(ctx, *p) == Some(*s))
        .map(|(p, _, _)| *p)
        .collect();
    let held_pids: Vec<u32> = held.iter().map(|(p, _, _)| *p).collect();
    let mut found = Vec::new();
    let mut children = Vec::new();
    for p in &list {
        if checked.contains(p) {
            continue;
        }
        let before = held.iter().find(|(q, _, _)| q == p).map(|(_, _, s)| *s);
        // A held number that failed the check is never "a child".
        let via = if before.is_none() { chain_to_checked(ctx, *p, &list, &checked, &held_pids) } else { None };
        match via {
            Some(via) => children.push(ChildAccepted { pid: *p, start_now: start_now(ctx, *p), via }),
            None => found.push(Mismatch { pid: *p, start_before: before, start_now: start_now(ctx, *p) }),
        }
    }
    (found, children)
}

/// Every agent scope under both slices: (kind, ws, id, its cgroup dir).
fn live_scopes(ctx: &Ctx) -> Vec<(Kind, AgentWs, ScopeId, PathBuf)> {
    let mut v = Vec::new();
    for k in Kind::ALL {
        let Ok(rd) = std::fs::read_dir(ctx.p(&cg(&slice_cgroup(k)))) else { continue };
        let mut names: Vec<String> = rd.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
        names.sort();
        for n in names {
            let Some((ws, id)) = parse_scope_name(&n) else { continue };
            let d = ctx.p(&cg(&scope_cgroup(k, &ws, &id)));
            if is_dir(&d) {
                v.push((k, ws, id, d));
            }
        }
    }
    v
}

fn live_scope_dirs(ctx: &Ctx) -> Vec<(String, PathBuf)> {
    live_scopes(ctx).into_iter().map(|(_, ws, id, d)| (scope_name(&ws, &id), d)).collect()
}

/// `agents stop`: ask systemd to stop the scope and answer at once (review
/// R2: no wait under the helper's lock; the scope's 10 s stop timeout
/// escalates to SIGKILL inside systemd).
fn stop(ctx: &Ctx, ws: &AgentWs, id: &ScopeId) -> HResult<J> {
    let name = scope_name(ws, id);
    let dir_of = |k: Kind| ctx.p(&cg(&scope_cgroup(k, ws, id)));
    let Some(kind) = Kind::ALL.into_iter().find(|k| is_dir(&dir_of(*k))) else {
        return Ok(json!({ "ok": true, "gone": true, "scope": name }));
    };
    if let Err(f) = stop_no_block(ctx, &name) {
        // Ended on its own in between: as good as stopped.
        if is_dir(&dir_of(kind)) {
            return Err(f);
        }
        return Ok(json!({ "ok": true, "gone": true, "scope": name }));
    }
    Ok(json!({ "ok": true, "gone": false, "stopping": true, "scope": name, "kind": kind.as_str() }))
}

/// `agents stop-all`: `--no-block` for every scope the classifier accepts
/// under both slices, answered at once; anchors and slices keep running.
/// Refuses, after asking for them all, when a call failed on a scope that
/// is still there.
fn stop_all(ctx: &Ctx) -> HResult<J> {
    let scopes = live_scope_dirs(ctx);
    let failed = scopes.iter().filter(|(n, d)| stop_no_block(ctx, n).is_err() && is_dir(d)).count();
    if failed > 0 {
        return Err(Fail::state(format!("agents stop-all: systemd refused to stop {failed} of {} scopes", scopes.len())));
    }
    Ok(json!({ "ok": true, "stopping": scopes.len() }))
}

/// Teardown only (uninstall, where a wait is fine): `--no-block` stop for
/// each, wait up to `ctx.agent_stop_wait_ms` for the cgroups to go,
/// `systemctl kill --signal=KILL` any still there, then look again.
/// `(killed, still there)`.
fn stop_and_wait(ctx: &Ctx, scopes: &[(String, PathBuf)]) -> (Vec<String>, Vec<String>) {
    for (name, _) in scopes {
        // A refusal here still gets the wait and the kill below.
        let _ = stop_no_block(ctx, name);
    }
    let deadline = std::time::Instant::now() + Duration::from_millis(ctx.agent_stop_wait_ms);
    while scopes.iter().any(|(_, d)| is_dir(d)) && std::time::Instant::now() < deadline {
        std::thread::sleep(STOP_WAIT_STEP);
    }
    let mut killed = Vec::new();
    for (name, _) in scopes.iter().filter(|(_, d)| is_dir(d)) {
        let _ = ctx.run_ok(SYSTEMCTL, &["kill", "--signal=KILL", name], None);
        killed.push(name.clone());
    }
    if !killed.is_empty() {
        let deadline = std::time::Instant::now() + AFTER_KILL_WAIT;
        while scopes.iter().any(|(_, d)| is_dir(d)) && std::time::Instant::now() < deadline {
            std::thread::sleep(STOP_WAIT_STEP);
        }
    }
    let still: Vec<String> = scopes.iter().filter(|(_, d)| is_dir(d)).map(|(n, _)| n.clone()).collect();
    (killed, still)
}

fn busctl_json(ctx: &Ctx, args: &[&str]) -> Option<serde_json::Value> {
    let mut a: Vec<&str> = vec!["--json=short"];
    a.extend_from_slice(args);
    let out = ctx.run_ok(BUSCTL, &a, None).ok()?;
    serde_json::from_slice(&out.stdout).ok()
}

/// The unit's `Delegate` as systemd holds it; `None` when unreadable.
fn delegate_of(ctx: &Ctx, unit_name: &str) -> Option<bool> {
    let m = "org.freedesktop.systemd1";
    let loaded = busctl_json(ctx, &["call", m, "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager", "LoadUnit", "s", unit_name])?;
    let path = parse_unit_path(&loaded.to_string())?;
    let v = busctl_json(ctx, &["get-property", m, &path, "org.freedesktop.systemd1.Service", "Delegate"])?;
    (v["type"] == "b").then(|| v["data"].as_bool()).flatten()
}

/// P1: as the daemon's user, ask systemd on the system bus for a scope in
/// the open slice, named like an agent's. It must be refused. `LC_ALL=C`
/// so systemd's refusal reads in the words the classifier knows.
fn probe(ctx: &Ctx, user: &str) -> (Probe, String) {
    let mut b = [0u8; 8];
    if ctx.rand.fill(&mut b).is_err() {
        return (Probe::Unknown, "no randomness for the probe's name".into());
    }
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let Some(name) = probe_scope_name(&hex) else { return (Probe::Unknown, "probe name".into()) };
    let slice = format!("--slice={OPEN_SLICE}");
    let unit = format!("--unit={name}");
    let args: Vec<String> = [
        "-u",
        user,
        "--",
        ENV,
        "LC_ALL=C",
        SYSTEMD_RUN,
        "--system",
        "--scope",
        "--quiet",
        "--collect",
        "--no-ask-password",
        slice.as_str(),
        unit.as_str(),
        "--",
        "true",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    match ctx.exec.run(RUNUSER, &args, None) {
        Ok(out) => {
            let err: String = String::from_utf8_lossy(&out.stderr).trim().chars().take(200).collect();
            (classify_probe(out.status, &err), err)
        }
        Err(e) => (Probe::Unknown, format!("runuser: {}", e.kind())),
    }
}

fn layout(ctx: &Ctx, who: &Caller) -> HResult<J> {
    let (entry, unit, _) = daemon_unit(ctx, who)?;
    let mut ready = true;
    let mut slices = serde_json::Map::new();
    let mut anchors = serde_json::Map::new();
    for l in Level::ALL {
        let d = ctx.p(&cg(&l.cgroup()));
        let present = is_dir(&d);
        let owner = if present { owner_of(&d).ok() } else { None };
        let root_owned = owner == Some(ctx.root_uid) && (!ctx.real_owners || is_cgroup2(&d));
        ready &= present && root_owned;
        slices.insert(l.slice().into(), json!({ "present": present, "rootOwned": root_owned, "owner": owner }));
        let state = ctx
            .exec
            .run(SYSTEMCTL, &["is-active".to_string(), l.anchor().to_string()], None)
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());
        ready &= state == "active";
        anchors.insert(l.anchor().into(), json!({ "active": state == "active", "state": state }));
    }
    let unit_dir = ctx.p(&cg(&unit));
    let unit_owner = owner_of(&unit_dir).ok();
    let scopes: Vec<J> = live_scopes(ctx)
        .into_iter()
        .map(|(k, ws, id, d)| {
            let name = scope_name(&ws, &id);
            let desc = ctx
                .exec
                .run(SYSTEMCTL, &["show".to_string(), "-p".to_string(), "Description".to_string(), "--value".to_string(), name], None)
                .ok()
                .filter(|o| o.status == 0)
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
            json!({
                "ws": ws.as_str(),
                "id": id.as_str(),
                "kind": k.as_str(),
                "pids": pids_in(&d).len(),
                "adopted": desc.as_deref() == Some(ADOPTED_DESCRIPTION),
            })
        })
        .collect();
    // The pid range (review N1): with the old 32768 a number comes back
    // soon enough to matter.
    let pid_max: Option<u64> = std::fs::read_to_string(ctx.p("/proc/sys/kernel/pid_max")).ok().and_then(|t| t.trim().parse().ok());
    let mut survivors = Vec::new();
    if let Ok(rd) = std::fs::read_dir(unit_dir.join(AGENTS_LEAF)) {
        let mut names: Vec<String> = rd.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
        names.sort();
        for n in names {
            let Some(ws) = n.strip_prefix(PREFIX).and_then(AgentWs::parse) else { continue };
            let rel = format!("{unit}/{AGENTS_LEAF}/{n}");
            let Ok(Some(old)) = read_cgroup(ctx, &cg(&rel)) else { continue };
            if !old.pids.is_empty() {
                survivors.push(json!({
                    "ws": ws.as_str(),
                    "cgroup": rel,
                    "pids": old.pids.len(),
                    "owner": old.owner,
                    "adoptable": old.root_owned(ctx),
                }));
            }
        }
    }
    let (p1, p1_err) = probe(ctx, &entry.user);
    ready &= p1 == Probe::Refused;
    // Rows 1c, 2 and P4: the bus policy and the drop-in in place, and the
    // unit not delegated (off, or pending its full restart).
    let delegate = delegate_of(ctx, &entry.unit);
    let (policy_ready, policy) = crate::agents_scope_policy::report(ctx, &entry, delegate, unit_owner);
    ready &= policy_ready;
    let mut v = json!({
        "ok": p1 != Probe::Allowed,
        "ready": ready,
        "slices": slices,
        "anchors": anchors,
        "unit": {
            "name": entry.unit,
            "cgroup": unit,
            "delegate": delegate,
            "cgroupOwner": unit_owner,
            "cgroupRootOwned": unit_owner == Some(ctx.root_uid),
        },
        "scopes": scopes,
        "survivors": survivors,
        "pidMax": pid_max,
        "p1": p1.as_str(),
        "p1Detail": if p1 == Probe::Refused { String::new() } else { p1_err },
    });
    if let Some(m) = v.as_object_mut() {
        m.extend(policy);
    }
    Ok(v)
}

fn setup(ctx: &Ctx, who: &Caller) -> HResult<J> {
    let entry = crate::daemon_unit::caller_entry(ctx, who)?;
    let sleep = SLEEP_PATHS
        .iter()
        .copied()
        .find(|p| std::fs::metadata(ctx.p(p)).is_ok_and(|m| m.is_file()))
        .ok_or_else(|| Fail::state("agents setup refused: no sleep program for the anchors"))?;
    let files = unit_files(sleep).ok_or_else(|| Fail::state("agents setup refused: the anchors' sleep is not on the list"))?;
    let sysd = ctx.dir(SYSTEMD_SYSTEM_DIR).map_err(|e| Fail::io_err("open /etc/systemd/system", e))?;
    let mut changed = false;
    for (name, text) in &files {
        changed |= crate::apps::put(ctx, &sysd, name, text.as_bytes())?;
    }
    // Rows 1c, 2 and P4: the bus policy and the daemon unit's drop-in.
    let policy = crate::agents_scope_policy::install(ctx, &entry)?;
    if changed || policy.delegate_dropin {
        ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None)?;
    }
    let anchors: Vec<&str> = Level::ALL.iter().map(|l| l.anchor()).collect();
    let mut a = vec!["enable", "--now"];
    a.extend(&anchors);
    ctx.run_ok(SYSTEMCTL, &a, None)?;
    if policy.bus_policy {
        crate::agents_scope_policy::reload_bus(ctx).map_err(|f| Fail::state(format!("agents setup: the bus policy is written, but {}", f.msg)))?;
    }
    Ok(json!({
        "ok": true,
        "changed": changed || policy.bus_policy || policy.delegate_dropin,
        "busPolicyChanged": policy.bus_policy,
        "delegateDropinChanged": policy.delegate_dropin,
        "sleep": sleep,
        "slices": Level::ALL.iter().map(|l| l.slice()).collect::<Vec<_>>(),
        "anchors": anchors,
    }))
}

/// Uninstall: every scope stopped (waiting is fine here, unlike `stop`),
/// the anchors disabled, `k2agents.slice` stopped (best effort), the six
/// unit files, the daemon unit's drop-in and the bus policy removed, then
/// systemd and the bus reloaded. `ok` is false when a scope is still there
/// at the end or the bus did not reload.
fn teardown(ctx: &Ctx, who: &Caller) -> HResult<J> {
    // The caller passed the daemon check; its entry names the unit. The
    // policy file goes even without it.
    let entry = crate::daemon_unit::caller_entry(ctx, who).ok();
    let scopes = live_scope_dirs(ctx);
    let (killed, _) = stop_and_wait(ctx, &scopes);
    let present: Vec<&str> = Level::ALL
        .iter()
        .map(|l| l.anchor())
        .filter(|a| std::fs::symlink_metadata(ctx.p(&format!("{SYSTEMD_SYSTEM_DIR}/{a}"))).is_ok())
        .collect();
    if !present.is_empty() {
        let mut a = vec!["disable", "--now"];
        a.extend(&present);
        ctx.run_ok(SYSTEMCTL, &a, None)?;
    }
    let slice_stopped = is_dir(&ctx.p(&cg(&Level::Top.cgroup()))).then(|| ctx.run_ok(SYSTEMCTL, &["stop", TOP_SLICE], None).is_ok());
    let mut removed = 0;
    for l in Level::ALL {
        for name in [l.anchor(), l.slice()] {
            if crate::apps::unput(ctx, SYSTEMD_SYSTEM_DIR, None, name)? {
                removed += 1;
            }
        }
    }
    let policy = crate::agents_scope_policy::remove(ctx, entry.as_ref())?;
    if removed > 0 || policy.delegate_dropin {
        ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None)?;
    }
    let bus_reload = if policy.bus_policy { Some(crate::agents_scope_policy::reload_bus(ctx).map_err(|f| f.msg)) } else { None };
    let still: Vec<String> = scopes.iter().filter(|(_, d)| is_dir(d)).map(|(n, _)| n.clone()).collect();
    Ok(json!({
        "ok": still.is_empty() && !matches!(bus_reload, Some(Err(_))),
        "busPolicyRemoved": policy.bus_policy,
        "delegateDropinRemoved": policy.delegate_dropin,
        "busReloadError": bus_reload.and_then(Result::err),
        "stopped": scopes.len(),
        "killed": killed.len(),
        "stillThere": still,
        "sliceStopped": slice_stopped,
        "removed": removed,
    }))
}
