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
//!   `CollectMode=inactive-or-failed` and the caps as numbers, and checks
//!   `/proc/<pid>/cgroup` names that scope while the held process lives;
//!   otherwise the scope is stopped and the call refused.
//! - `agents adopt <ws> <id> <open|fenced>` (daemon): every process of the
//!   daemon's user left in `<unit>/agents/a-<ws>` (a session that lived
//!   through an update) into one new scope.
//! - `agents stop <ws> <id>` / `agents stop-all` (daemon): `systemctl stop`
//!   for names the classifier accepts, found under the two slices only.
//! - `agents layout` (daemon): report only, with the P1 probe run as the
//!   daemon's user.
//! - `agents setup` (open) / `agents teardown` (daemon): the slice and
//!   anchor units.

use std::path::PathBuf;
use std::time::Duration;

use k2_root_proto::agent_cgroup::{argv1, pid_ok, start_of_stat, AgentWs, AGENTS_LEAF, EXEC_HELPER_ARG, PREFIX};
use k2_root_proto::agent_scope::{
    classify_probe, may_scope_from, parse_scope_name, probe_scope_name, scope_cgroup, scope_name, slice_cgroup, start_transient_argv, unit_files,
    Caps, Kind, Level, Probe, ScopeId, ScopeVerb, ADOPT_MAX_PIDS, OPEN_SLICE, SLEEP_PATHS,
};
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

pub fn run(ctx: &Ctx, v: &ScopeVerb, who: &Caller) -> HResult<J> {
    match v {
        ScopeVerb::Scope { ws, id, pid, start, kind, caps } => scope(ctx, who, ws, id, *pid, *start, *kind, caps),
        ScopeVerb::Adopt { ws, id, kind } => adopt(ctx, who, ws, id, *kind),
        ScopeVerb::Stop { ws, id } => stop(ctx, ws, id),
        ScopeVerb::StopAll => stop_all(ctx).map(|n| json!({ "ok": true, "stopped": n })),
        ScopeVerb::Layout => layout(ctx, who),
        ScopeVerb::Setup => setup(ctx),
        ScopeVerb::Teardown => teardown(ctx),
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
fn start_scope(ctx: &Ctx, name: &str, kind: Kind, pids: &[u32], caps: &Caps) -> HResult<()> {
    let args = start_transient_argv(name, kind, pids, caps);
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

fn stop_unit(ctx: &Ctx, name: &str) -> HResult<()> {
    ctx.run_ok(SYSTEMCTL, &["stop", name], None).map(|_| ())
}

#[allow(clippy::too_many_arguments)]
fn scope(ctx: &Ctx, who: &Caller, ws: &AgentWs, id: &ScopeId, pid: u32, start: u64, kind: Kind, caps: &Caps) -> HResult<J> {
    let (_, unit, uid) = daemon_unit(ctx, who)?;
    let held = held_exec(ctx, &unit, ws, pid, start, uid)?;
    require_slice(ctx, kind)?;
    let name = scope_name(ws, id);
    let want = scope_cgroup(kind, ws, id);
    start_scope(ctx, &name, kind, &[pid], caps)?;
    let placed = wait_moved(ctx, &[pid], &want) == [pid];
    if !placed || !held.alive() {
        let _ = stop_unit(ctx, &name);
        return Err(Fail::state(if placed {
            "agents scope refused: the process exited during the move (scope stopped)"
        } else {
            "agents scope refused: the process is not in its scope after the start (scope stopped)"
        }));
    }
    Ok(json!({ "ok": true, "cgroup": want, "pid": pid, "scope": name, "kind": kind.as_str() }))
}

fn adopt(ctx: &Ctx, who: &Caller, ws: &AgentWs, id: &ScopeId, kind: Kind) -> HResult<J> {
    let (_, unit, uid) = daemon_unit(ctx, who)?;
    require_slice(ctx, kind)?;
    let old_rel = format!("{unit}/{AGENTS_LEAF}/{}", ws.cgroup_name());
    let old = ctx.p(&cg(&old_rel));
    if !is_dir(&old) {
        return Err(Fail::state("agents adopt refused: no old-layout cgroup for this workspace"));
    }
    let listed = pids_in(&old);
    if listed.is_empty() {
        return Err(Fail::state("agents adopt refused: the old-layout cgroup is empty"));
    }
    if listed.len() > ADOPT_MAX_PIDS {
        return Err(Fail::state("agents adopt refused: more processes than one adopt moves"));
    }
    let mut held: Vec<(u32, Box<dyn ProcHandle>)> = Vec::new();
    for p in &listed {
        let n = pid_ok(p).ok_or_else(|| Fail::state("agents adopt refused: the old cgroup lists something that is not a pid"))?;
        // One that exited since the list was read is skipped.
        let Some(h) = ctx.procs.hold(n as i32) else { continue };
        match ctx.procs.ruid_of(n as i32) {
            None => continue,
            Some(u) if u == uid => held.push((n, h)),
            Some(_) => return Err(Fail::state("agents adopt refused: a process there is not the daemon user's")),
        }
    }
    if held.is_empty() {
        return Err(Fail::state("agents adopt refused: every process there has exited"));
    }
    let pids: Vec<u32> = held.iter().map(|(p, _)| *p).collect();
    let name = scope_name(ws, id);
    let want = scope_cgroup(kind, ws, id);
    start_scope(ctx, &name, kind, &pids, &Caps::default())?;
    let alive: Vec<u32> = held.iter().filter(|(_, h)| h.alive()).map(|(p, _)| *p).collect();
    let moved = wait_moved(ctx, &alive, &want);
    if moved.is_empty() {
        let _ = stop_unit(ctx, &name);
        return Err(Fail::state("agents adopt refused: no process is in the new scope after the start (scope stopped)"));
    }
    let not_moved: Vec<u32> = alive.iter().copied().filter(|p| !moved.contains(p)).collect();
    let left = pids_in(&old).len();
    Ok(json!({
        "ok": true,
        "scope": name,
        "cgroup": want,
        "kind": kind.as_str(),
        "moved": moved.len(),
        "pids": moved,
        "notMoved": not_moved,
        "left": left,
    }))
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

fn stop(ctx: &Ctx, ws: &AgentWs, id: &ScopeId) -> HResult<J> {
    let name = scope_name(ws, id);
    let here = |k: Kind| is_dir(&ctx.p(&cg(&scope_cgroup(k, ws, id))));
    let Some(kind) = Kind::ALL.into_iter().find(|k| here(*k)) else {
        return Ok(json!({ "ok": true, "gone": true, "scope": name }));
    };
    if let Err(f) = stop_unit(ctx, &name) {
        // Ended on its own in between: as good as stopped.
        if here(kind) {
            return Err(f);
        }
    }
    Ok(json!({ "ok": true, "gone": false, "scope": name, "kind": kind.as_str() }))
}

/// Stop every scope the classifier accepts under both slices; anchors and
/// slices keep running. Refuses (after trying them all) when any didn't
/// stop.
fn stop_all(ctx: &Ctx) -> HResult<usize> {
    let scopes = live_scopes(ctx);
    let mut failed = 0;
    for (_, ws, id, dir) in &scopes {
        if stop_unit(ctx, &scope_name(ws, id)).is_err() && is_dir(dir) {
            failed += 1;
        }
    }
    if failed > 0 {
        return Err(Fail::state(format!("agents stop-all: {failed} of {} scopes did not stop", scopes.len())));
    }
    Ok(scopes.len())
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
/// the open slice, named like an agent's. It must be refused.
fn probe(ctx: &Ctx, user: &str) -> (Probe, String) {
    let mut b = [0u8; 8];
    if ctx.rand.fill(&mut b).is_err() {
        return (Probe::Unknown, "no randomness for the probe's name".into());
    }
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let Some(name) = probe_scope_name(&hex) else { return (Probe::Unknown, "probe name".into()) };
    let slice = format!("--slice={OPEN_SLICE}");
    let unit = format!("--unit={name}");
    let args: Vec<String> =
        ["-u", user, "--", SYSTEMD_RUN, "--system", "--scope", "--quiet", "--collect", "--no-ask-password", slice.as_str(), unit.as_str(), "--", "true"]
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
        let root_owned = owner == Some(0) && (!ctx.real_owners || is_cgroup2(&d));
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
        .map(|(k, ws, id, d)| json!({ "ws": ws.as_str(), "id": id.as_str(), "kind": k.as_str(), "pids": pids_in(&d).len() }))
        .collect();
    let mut survivors = Vec::new();
    if let Ok(rd) = std::fs::read_dir(unit_dir.join(AGENTS_LEAF)) {
        let mut names: Vec<String> = rd.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
        names.sort();
        for n in names {
            let Some(ws) = n.strip_prefix(PREFIX).and_then(AgentWs::parse) else { continue };
            let d = unit_dir.join(AGENTS_LEAF).join(&n);
            let count = if is_dir(&d) { pids_in(&d).len() } else { 0 };
            if count > 0 {
                survivors.push(json!({ "ws": ws.as_str(), "cgroup": format!("{unit}/{AGENTS_LEAF}/{n}"), "pids": count }));
            }
        }
    }
    let (p1, p1_err) = probe(ctx, &entry.user);
    ready &= p1 == Probe::Refused;
    Ok(json!({
        "ok": p1 != Probe::Allowed,
        "ready": ready,
        "slices": slices,
        "anchors": anchors,
        "unit": {
            "name": entry.unit,
            "cgroup": unit,
            "delegate": delegate_of(ctx, &entry.unit),
            "cgroupOwner": unit_owner,
            "cgroupRootOwned": unit_owner == Some(0),
        },
        "scopes": scopes,
        "survivors": survivors,
        "p1": p1.as_str(),
        "p1Detail": if p1 == Probe::Refused { String::new() } else { p1_err },
    }))
}

fn setup(ctx: &Ctx) -> HResult<J> {
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
    if changed {
        ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None)?;
    }
    let anchors: Vec<&str> = Level::ALL.iter().map(|l| l.anchor()).collect();
    let mut a = vec!["enable", "--now"];
    a.extend(&anchors);
    ctx.run_ok(SYSTEMCTL, &a, None)?;
    Ok(json!({
        "ok": true,
        "changed": changed,
        "sleep": sleep,
        "slices": Level::ALL.iter().map(|l| l.slice()).collect::<Vec<_>>(),
        "anchors": anchors,
    }))
}

fn teardown(ctx: &Ctx) -> HResult<J> {
    let stopped = stop_all(ctx)?;
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
    let mut removed = 0;
    for l in Level::ALL {
        for name in [l.anchor(), l.slice()] {
            if crate::apps::unput(ctx, SYSTEMD_SYSTEM_DIR, None, name)? {
                removed += 1;
            }
        }
    }
    if removed > 0 {
        ctx.run_ok(SYSTEMCTL, &["daemon-reload"], None)?;
    }
    Ok(json!({ "ok": true, "stopped": stopped, "removed": removed }))
}
