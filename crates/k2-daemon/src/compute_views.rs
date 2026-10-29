//! The compute views (prd-compute-easy-mode-0467 §9, Lane B): what
//! `nodes`, `status`, `info`, `queue`, `jobs`, `get` and `cancel` return.
//! Moved out of [`crate::compute_routes`] unchanged, so the submit and
//! placement work there and the record and view work here touch different
//! files. The route table, `Who` and the refusal helpers stay in
//! `compute_routes`.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use k2_core::compute::proto::frames::{Control, JobState};
use k2_core::compute::proto::pairing;
use k2_core::compute::proto::share as pshare;
use k2_core::compute::proto::tiers::Tier;
use k2_core::compute::share::{self, pending};
use k2_core::compute::{self as compute, codes, scheduler, store};

use crate::compute_manage::Manager;
use crate::compute_routes::{
    job_for, json_body, need_credential, no, no_workspace, node_or_404, passport_gate, resolve_node_for, s, sweep_retiring, ws_by_token_or_worktree,
    ws_handle, Who, R,
};
use crate::compute_ws;

// ── nodes ────────────────────────────────────────────────────────────

fn node_json(n: &store::NodeRow, show_sas: bool) -> serde_json::Value {
    let (online, route, live_offer) = compute_ws::live_view(&n.id);
    let offer = live_offer.or_else(|| n.last_offer.clone());
    let (queued, active) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        // Node-queued jobs (a shared node's queue) count as queued, not
        // running: their rows stay `assigned` until the node starts them.
        let (node_queued, running): (Vec<_>, Vec<_>) = store::jobs_in_states(&conn, &n.id, store::ACTIVE_STATES).into_iter().partition(|r| r.node_queued);
        (store::jobs_in_states(&conn, &n.id, &["queued"]).len() + node_queued.len(), running.len())
    };
    let mut v = serde_json::to_value(n).unwrap_or_default();
    v["online"] = online.into();
    v["liveRoute"] = route.into();
    v["queued"] = queued.into();
    v["running"] = active.into();
    if let Some(o) = offer {
        v["offer"] = serde_json::json!({
            "control": o.control.as_str(),
            "controlBy": o.control_by,
            "available": o.availability.ok,
            "unavailable": o.availability.reasons,
            "refusal": o.refusal,
            "caps": o.caps,
            "free": o.free,
            "parallelMax": o.parallel_max,
            "tools": o.tools,
            "slots": o.slots,
            "protocol": o.protocol,
            "features": o.features,
        });
    }
    if show_sas && n.state == "pending" {
        v["sas"] = n.sas.as_deref().map(pairing::display_sas).into();
    }
    v["retiring"] = compute::access::retiring_at(&compute::access::retiring_path()).contains_key(&n.id).into();
    if let Some(u) = crate::compute_place::set_aside_until(&n.id, compute::now()) {
        // Infra failures in a row: `--any` skips it until then.
        v["setAsideUntil"] = u.into();
    }
    // Node sharing (protocol 3): only for shared nodes and tenants, so a
    // solo node's JSON is unchanged.
    if let Some(s) = share_json(n) {
        v["share"] = s;
        v["role"] = n.role.clone().into();
    }
    v
}

/// What `nodes`/`info` show about a shared node (§4.2, §9): who shares it,
/// how many servers, the end date and this server's share.
fn share_json(n: &store::NodeRow) -> Option<serde_json::Value> {
    if !n.shared && !n.is_tenant() {
        return None;
    }
    let v = n.share.as_ref();
    let now = compute::now();
    let expires_at = v.and_then(|v| v.expires_in_secs).zip(n.share_at).map(|(left, at)| at + left);
    Some(serde_json::json!({
        "shared": n.shared,
        "role": n.role,
        "ownerLabel": n.owner_label,
        // The node says this server owns it now; only the owner's
        // take-over makes it so (MT36, `k2 compute node take-owner`).
        "ownerClaim": n.owner_claim(),
        "servers": v.map(|v| v.servers),
        "state": v.map(|v| v.state.as_str()),
        "expiresAt": expires_at,
        "expiresInSecs": expires_at.map(|e| (e - now).max(0)),
        "limits": v.map(|v| &v.limits),
        "running": v.map(|v| v.running),
        "queued": v.map(|v| v.queued),
        "cpuSecsToday": v.map(|v| v.cpu_secs_today),
        "line": v.map(share::share_line),
        // §14: the node's clock against this server's, only when worth
        // showing (order and ETAs never use it).
        "skewSecs": v.and_then(|v| v.skew_secs).filter(|s| s.abs() > pshare::SKEW_SHOW_SECS),
        // Job tiers (T7): does this server's Priority count there? null =
        // a node older than 0.46.5 doesn't say.
        "priority": v.and_then(|v| v.priority),
        "at": n.share_at,
    }))
}

/// `{ci, priority, normal}` counts, every tier present (0 when none).
fn tier_counts(m: &BTreeMap<String, i64>) -> serde_json::Value {
    serde_json::json!({
        "ci": m.get("ci").copied().unwrap_or(0),
        "priority": m.get("priority").copied().unwrap_or(0),
        "normal": m.get("normal").copied().unwrap_or(0),
        "batch": m.get("batch").copied().unwrap_or(0),
    })
}

/// The Priority holders on this server: `[{submitter, workspace, jobs}]`.
fn critical_path_json() -> serde_json::Value {
    let holders = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::priority_holders(&conn)
    };
    holders
        .into_iter()
        .map(|h| serde_json::json!({"submitter": h.submitter, "workspace": ws_handle(&h.workspace_id), "jobs": h.jobs}))
        .collect::<Vec<_>>()
        .into()
}

pub(crate) fn granted_node_ids(ws: &str) -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::grants(&conn, None, Some(ws)).into_iter().map(|g| g.node_id).collect()
}

pub(crate) fn nodes_list(who: &Who, params: &HashMap<String, String>, mgr: Option<&Manager>) -> R<serde_json::Value> {
    need_credential(who)?;
    // A manager sees every node (it adds, confirms and retires them).
    let gate = if mgr.is_some() { None } else { passport_gate(who)? };
    sweep_retiring();
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_nodes(&conn, false)
    };
    let (rows, ws): (Vec<_>, Option<String>) = match gate {
        Some(ws) => {
            let ids = granted_node_ids(&ws);
            (rows.into_iter().filter(|n| n.state == "active" && ids.contains(&n.id)).collect(), Some(ws))
        }
        // The owner may name a workspace (the CLI sends its checkout) to see
        // that workspace's grant on each node. One that doesn't resolve is
        // ignored, never a 404, so `nodes` works from anywhere.
        None => {
            let ws = params
                .get("workspace")
                .map(String::as_str)
                .filter(|s| !s.trim().is_empty())
                .and_then(ws_by_token_or_worktree)
                .map(|w| w.id);
            (rows, ws)
        }
    };
    let show_sas = who.is_owner();
    // `k2 compute status`: per node what the controller already knows.
    let detail = params.get("detail").is_some_and(|d| d == "1" || d == "true");
    let nodes: Vec<_> = rows
        .iter()
        .map(|n| {
            let mut v = node_json(n, show_sas);
            if let Some(ws) = &ws {
                let db = k2_core::db::shared();
                let conn = db.lock();
                v["grant"] = serde_json::to_value(store::grant(&conn, &n.id, ws)).unwrap_or_default();
            }
            // This server's jobs waiting there, by tier (counts only).
            v["queueTiers"] = tier_counts(&{
                let db = k2_core::db::shared();
                let conn = db.lock();
                store::queued_tiers(&conn, &n.id)
            });
            if detail {
                v["status"] = node_status(who, n);
            }
            v
        })
        .collect();
    let codes: Vec<serde_json::Value> = if who.is_owner() || mgr.is_some() {
        compute::enroll::pending().into_iter().map(|(name, exp)| serde_json::json!({"name": name, "expiresAt": exp})).collect()
    } else {
        Vec::new()
    };
    // `ticketSupported`: this server trusts a one-click ticket key, so a
    // machine on the same Connect account can join it without a code.
    let mut out = serde_json::json!({
        "nodes": nodes,
        "waitingCodes": codes,
        "controller": compute_ws::controller_label(),
        // This server's full controller key: what the person checks before
        // approving a node's move to this server (servers.owner, review F5).
        "controllerFp": compute::controller_key().ok().map(|k| k.fingerprint()),
        "ticketSupported": compute::ticket::supported(),
        // Who is on the critical path on this server (§3.5): a coordinator
        // sees it before choosing. Empty when nobody uses Priority.
        "criticalPath": critical_path_json(),
    });
    if detail {
        out["controllerMachine"] = controller_machine();
        out["notYet"] = "hand-run lock files (sew-ci, k2-release, runner) with holder and age, build-cache size against its cap, \
            and pinned long-runners need node reporting, which nodes don't send yet. Locks a node already refuses \
            on show as waiting: foreign_lock."
            .into();
    }
    Ok(out)
}

/// What `status` shows for one node, from what the controller already has:
/// the live offer (control, the node-wide refusal such as a foreign lock,
/// free CPU/memory under the caps, free disk), the last health frame (load,
/// disks), the cached policy (disk floor) and the job table.
fn node_status(who: &Who, n: &store::NodeRow) -> serde_json::Value {
    let now = compute::now();
    let (online, route, live) = compute_ws::live_view(&n.id);
    let offer = live.or_else(|| n.last_offer.clone());
    let decision = compute_ws::decision_for(&n.id);
    let (active, queued, health, cols) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (
            store::jobs_in_states(&conn, &n.id, store::ACTIVE_STATES),
            store::jobs_in_states(&conn, &n.id, &["queued"]),
            compute::admin::health(&conn, &n.id),
            compute::admin::admin_cols(&conn, &n.id),
        )
    };
    let viewer = who.passport();
    // A shared node's queued jobs stay `assigned` here (reason
    // `node_queued`): they wait in the node's own queue, so they're
    // queued, not running (0.46.6: `status` showed "run 6/3" on a node
    // running 2). The node's queue first, in its order, then this server's.
    let (node_queued, active): (Vec<_>, Vec<_>) = active.into_iter().partition(|r| r.node_queued);
    let running: Vec<serde_json::Value> = active.iter().map(|r| job_entry(viewer, r, &decision, now)).collect();
    let mut waiting_jobs: Vec<((u8, usize), serde_json::Value)> = node_queued
        .iter()
        .map(|r| ((0, r.node_pos.map(|p| p.max(0) as usize).unwrap_or(usize::MAX)), job_entry(viewer, r, &decision, now)))
        .chain(queued.iter().map(|r| ((1, decision.waiting.get(&r.id).map(|(p, _)| *p).unwrap_or(usize::MAX)), job_entry(viewer, r, &decision, now))))
        .collect();
    waiting_jobs.sort_by_key(|(p, _)| *p);
    let state = if n.state == "pending" {
        "pending".to_string()
    } else if !online {
        "offline".to_string()
    } else {
        match offer.as_ref().map(|o| o.control) {
            Some(c) if c != Control::Active => c.as_str().to_string(),
            _ => format!("online/{}", route.unwrap_or("?")),
        }
    };
    let gib = 1u64 << 30;
    let home_disk = health.as_ref().and_then(|(_, h)| h.disks.iter().find(|d| d.name == "node_home").or_else(|| h.disks.first()).cloned());
    let policy_floor = cols.policy.as_ref().and_then(|p| p.get("disk_floor_gb")).and_then(|g| g.as_u64()).map(|g| g * gib);
    let (floor, floor_from) = match (policy_floor, &home_disk) {
        (Some(f), _) => (Some(f), Some("policy")),
        // The node's own default: max(10% of its disk, 30 GB).
        (None, Some(d)) if d.total_bytes > 0 => (Some((d.total_bytes / 10 / gib).max(30) * gib), Some("default")),
        _ => (None, None),
    };
    let disk_free = offer.as_ref().map(|o| o.free.disk_free_bytes).or(home_disk.as_ref().map(|d| d.free_bytes));
    serde_json::json!({
        "state": state,
        "controllerPause": n.controller_pause,
        "waiting": offer.as_ref().and_then(|o| o.refusal.as_ref()).map(|r| serde_json::json!({"code": r.code, "message": r.message})),
        "unavailable": offer.as_ref().filter(|o| !o.availability.ok).map(|o| o.availability.reasons.clone()),
        "disk": {
            "freeBytes": disk_free,
            "floorBytes": floor,
            "floorFrom": floor_from,
            "belowFloor": match (disk_free, floor) { (Some(f), Some(fl)) => Some(f < fl), _ => None },
        },
        "free": offer.as_ref().map(|o| serde_json::json!({"cpuMillis": o.free.cpu_millis, "memBytes": o.free.mem_bytes})),
        "caps": offer.as_ref().map(|o| serde_json::json!({"cpuMillis": o.caps.cpu_millis, "memBytes": o.caps.mem_bytes, "hard": o.caps.hard})),
        "parallelMax": offer.as_ref().map(|o| o.parallel_max),
        "load": health.as_ref().and_then(|(_, h)| h.load_milli).map(|l| l.map(|x| x as f64 / 1000.0)),
        "memAvailableBytes": health.as_ref().and_then(|(_, h)| h.mem_available_bytes),
        "healthAt": health.as_ref().map(|(at, _)| *at),
        "running": running,
        "queued": waiting_jobs.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
    })
}

/// The controller machine's own row for `status`: free disk on the home
/// volume and, on macOS, how many Time Machine local snapshots hold space
/// (`tmutil listlocalsnapshots /`).
fn controller_machine() -> serde_json::Value {
    let home = k2_core::paths::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
    let hw = compute::advertise::probe(&home);
    let mut v = serde_json::json!({
        "name": compute_ws::controller_label(),
        "diskFreeBytes": hw.disk_free_bytes,
        "diskTotalBytes": hw.disk_total_bytes,
    });
    if cfg!(target_os = "macos") {
        match std::process::Command::new("tmutil").args(["listlocalsnapshots", "/"]).stdin(std::process::Stdio::null()).output() {
            Ok(o) if o.status.success() => {
                let n = String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.contains("com.apple.TimeMachine")).count();
                v["tmLocalSnapshots"] = n.into();
            }
            Ok(o) => v["tmLocalSnapshotsError"] = String::from_utf8_lossy(&o.stderr).trim().chars().take(200).collect::<String>().into(),
            Err(e) => v["tmLocalSnapshotsError"] = e.to_string().into(),
        }
    }
    v
}

pub(crate) fn node_visible(who: &Who, n: &store::NodeRow) -> R<()> {
    if let Some(ws) = passport_gate(who)? {
        if n.state != "active" || !granted_node_ids(&ws).contains(&n.id) {
            return Err(no(codes::NODE_NOT_FOUND, 404, format!("no compute node '{}' is granted to this workspace", n.name)));
        }
    }
    Ok(())
}

pub(crate) fn node_info(who: &Who, params: &HashMap<String, String>, mgr: Option<&Manager>) -> R<serde_json::Value> {
    need_credential(who)?;
    let n = resolve_node_for(who, params.get("node").map(String::as_str), who.passport().map(|(ws, _)| ws))?;
    if mgr.is_none() {
        node_visible(who, &n)?;
    }
    let mut v = node_json(&n, who.is_owner());
    let (grants, jobs) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let ws = who.passport().map(|(w, _)| w.to_string());
        // A manager sees every grant on the node; jobs stay its own.
        let grants = store::grants(&conn, Some(&n.id), if mgr.is_some() { None } else { ws.as_deref() });
        let jobs = store::list_jobs(&conn, &store::JobFilter { node_id: Some(&n.id), workspace_id: ws.as_deref(), limit: 10, ..Default::default() });
        (grants, jobs)
    };
    v["grants"] = serde_json::to_value(grants).unwrap_or_default();
    v["recentJobs"] = jobs.iter().map(job_summary).collect::<Vec<_>>().into();
    Ok(v)
}

// ── jobs ─────────────────────────────────────────────────────────────

pub(crate) fn job_summary(j: &store::JobRow) -> serde_json::Value {
    let node = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, &j.node_id).map(|n| n.name)
    };
    let mut v = serde_json::to_value(j).unwrap_or_default();
    v["short"] = compute::message::short(&j.id).into();
    v["workspace"] = ws_handle(&j.workspace_id).into();
    if j.node_queued {
        // In a shared node's queue: shown as queued, at the node's place.
        v["state"] = "queued".into();
        v["nodeQueued"] = true.into();
        v["class"] = j.class.clone().into();
        v["queue"] = node_queue_json(j, node.as_deref());
    }
    v["node"] = node.into();
    // Where it was placed and why (prd-compute-auto-placement-v1 §7).
    if let Some(p) = j.ctl.placement.as_ref() {
        v["placement"] = serde_json::to_value(p).unwrap_or_default();
    }
    // Secret-env (SE8): the env names the job gets from the vault. Names
    // only; absent for every other job, so their JSON is unchanged.
    let secrets = crate::compute_secrets::job_secret_names(j);
    if !secrets.is_empty() {
        v["secrets"] = secrets.into();
    }
    v
}

/// The queue line of a job waiting in a shared node's queue (§4.2):
/// `queued on k2n2-m1, position 2, ETA ~6m (fair share: …)`.
fn node_queue_json(j: &store::JobRow, node: Option<&str>) -> serde_json::Value {
    let node = node.unwrap_or("the node");
    // "2 of 5" (prd-compute-job-tiers-v2 L6, T26): the node's place; a node
    // older than 0.46.6 sends no total.
    let place = match (j.node_pos, j.node_of) {
        (Some(p), Some(m)) => format!(": {p} of {m}"),
        (Some(p), None) => format!(", position {p}"),
        _ => String::new(),
    };
    let eta = j.node_eta_secs.map(|e| format!(", ETA {}", share::human_eta(e.max(0) as u64))).unwrap_or_default();
    let words = j.node_blocked_by.as_deref().map(|b| share::wait_words(b, &node_wait_facts(j)));
    let passes = j.node_blocked_by.as_deref().is_some_and(pshare::blocked::lets_others_pass);
    let why = words.as_deref().map(|w| format!(" — {w}{}", if passes { "; jobs behind it may start first" } else { "" })).unwrap_or_default();
    let reason = match j.reason.as_deref() {
        Some(r) if pending::any(Some(r)) || r == "cancel_requested" => r.to_string(),
        _ => pshare::codes::NODE_QUEUED.to_string(),
    };
    let line = format!("queued on {node}{place}{eta}{why}");
    serde_json::json!({
        "onNode": true,
        "where": "node",
        "position": j.node_pos,
        "of": j.node_of,
        "etaSecs": j.node_eta_secs,
        "etaP90Secs": j.node_eta_p90_secs,
        "blockedBy": j.node_blocked_by,
        "waitReason": j.node_blocked_by,
        "waitWords": words,
        "holdsQueue": !passes,
        "reason": reason,
        "message": line,
        "line": line,
        "seenAt": j.node_seen_at,
    })
}

/// The numbers behind a node-queued job's wait (L6): its ask, the node's
/// free room from its live offer, this workspace's and this server's
/// running jobs there and their caps.
fn node_wait_facts(j: &store::JobRow) -> share::WaitFacts {
    let plan = j.plan();
    let offer = compute_ws::live_view(&j.node_id).2;
    let active = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::jobs_in_states(&conn, &j.node_id, store::ACTIVE_STATES)
    };
    let started: Vec<&store::JobRow> = active.iter().filter(|r| !r.node_queued).collect();
    share::WaitFacts {
        asks_cpu_millis: plan.as_ref().and_then(|p| p.limits.cpu_millis),
        asks_mem_bytes: plan.as_ref().and_then(|p| p.limits.mem_bytes),
        free_cpu_millis: offer.as_ref().map(|o| o.free.cpu_millis),
        free_mem_bytes: offer.as_ref().map(|o| o.free.mem_bytes),
        cap_cpu_millis: offer.as_ref().map(|o| o.caps.cpu_millis),
        cap_mem_bytes: offer.as_ref().map(|o| o.caps.mem_bytes),
        running: offer.as_ref().map(|o| o.running),
        parallel: offer.as_ref().map(|o| o.parallel_max),
        ws_running: Some(started.iter().filter(|r| r.workspace_id == j.workspace_id).count() as u32),
        ws_cap: plan.as_ref().and_then(|p| p.ws_max_running),
        server_running: Some(started.len() as u32),
        server_cap: offer.as_ref().and_then(|o| o.share.as_ref()).map(|v| v.limits.max_running),
        submitter_running: None,
        submitter_cap: None,
    }
}

/// One job as another job's queue view (or `status`) shows it. `viewer` =
/// the caller's passport: it sees ids and commands only for its own
/// workspace; other workspaces' jobs carry state and age only.
fn job_entry(viewer: Option<(&str, &str)>, r: &store::JobRow, decision: &scheduler::Decision, now: i64) -> serde_json::Value {
    let mut e = serde_json::json!({"state": if r.node_queued { "queued" } else { r.state.as_str() }, "ageSecs": (now - r.created_at).max(0), "tier": r.tier().as_str()});
    if viewer.is_none_or(|(ws, _)| ws == r.workspace_id) {
        let mut argv = r.argv.join(" ");
        if argv.chars().count() > 80 {
            argv = argv.chars().take(79).collect::<String>() + "…";
        }
        e["short"] = compute::message::short(&r.id).into();
        e["workspace"] = ws_handle(&r.workspace_id).into();
        e["mine"] = viewer.is_some_and(|(_, sid)| r.session_id.as_deref() == Some(sid)).into();
        e["argv"] = argv.into();
    } else {
        e["workspace"] = serde_json::Value::Null;
        e["mine"] = false.into();
    }
    if let Some(t) = r.assigned_at.or(r.started_at) {
        e["elapsedSecs"] = (now - t).max(0).into();
    }
    if let Some((p, w)) = decision.waiting.get(&r.id) {
        e["reason"] = w.code.clone().into();
        e["position"] = (*p).into();
    }
    priority_who(r, &mut e);
    e
}

/// A Priority job's one-line `--why` and its submitter (Rosson 10-28):
/// shown in the queue and `k2 compute status` to everyone on this server,
/// so builders see who holds the critical path and why.
fn priority_who(r: &store::JobRow, e: &mut serde_json::Value) {
    if r.tier() == Tier::Priority {
        if let Some(w) = &r.ctl.why {
            e["tierWhy"] = w.clone().into();
        }
        if let Some(s) = &r.submitter {
            e["submitter"] = s.clone().into();
        }
    }
}

/// The queue view of a queued job (prd-k2-compute-dx-v1 C21, without ETAs
/// in 0.46.1): its position of how many, why it waits, and what is ahead
/// of it — the node's active jobs plus the queued jobs in front (FIFO by
/// age, the scheduler's positions). A passport sees ids and commands only
/// for its own workspace; other workspaces' entries carry state and age.
pub(crate) fn queue_view(who: &Who, j: &store::JobRow, decision: &scheduler::Decision) -> Option<serde_json::Value> {
    let (pos, why) = decision.waiting.get(&j.id)?;
    let now = compute::now();
    let viewer = who.passport();
    let (active, queued, grant, node_offer) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (
            store::jobs_in_states(&conn, &j.node_id, store::ACTIVE_STATES),
            store::jobs_in_states(&conn, &j.node_id, &["queued"]),
            store::grant(&conn, &j.node_id, &j.workspace_id),
            store::node_by_id(&conn, &j.node_id).and_then(|n| n.last_offer),
        )
    };
    let offer = compute_ws::live_view(&j.node_id).2.or(node_offer);
    let entry = |r: &store::JobRow| job_entry(viewer, r, decision, now);
    let mut ahead: Vec<serde_json::Value> = active.iter().map(&entry).collect();
    let mut before: Vec<(usize, &store::JobRow)> = queued
        .iter()
        .filter_map(|q| decision.waiting.get(&q.id).map(|(p, _)| (*p, q)))
        .filter(|(p, _)| p < pos)
        .collect();
    before.sort_by_key(|(p, _)| *p);
    ahead.extend(before.iter().map(|(_, q)| entry(q)));
    let grant_parallel = grant.map(|g| g.max_parallel.max(1) as u32).unwrap_or(1);
    let parallel = offer.as_ref().map(|o| o.parallel_max.min(grant_parallel)).unwrap_or(grant_parallel);
    // "behind 1 CI" (prd §4.1): the queued jobs ahead of it, by tier.
    let ahead_ci = before.iter().filter(|(_, q)| q.tier() == Tier::Ci).count();
    let ahead_p = before.iter().filter(|(_, q)| q.tier() == Tier::Priority).count();
    // L6 (prd-compute-job-tiers-v2, T26): two kinds of place. On a shared
    // node a job here waits on THIS server (its share is full, a feature
    // or `--any` wait) before it joins the node's queue; on a solo node
    // this server orders the node's queue itself.
    let on_shared = offer.as_ref().is_some_and(share::is_shared);
    let node_name = node_name_of(&j.node_id);
    let ws_running = active.iter().filter(|r| r.workspace_id == j.workspace_id && !r.node_queued).count() as u32;
    let plan = j.plan();
    let facts = share::WaitFacts {
        asks_cpu_millis: plan.as_ref().and_then(|p| p.limits.cpu_millis),
        asks_mem_bytes: plan.as_ref().and_then(|p| p.limits.mem_bytes),
        free_cpu_millis: offer.as_ref().map(|o| o.free.cpu_millis),
        free_mem_bytes: offer.as_ref().map(|o| o.free.mem_bytes),
        cap_cpu_millis: offer.as_ref().map(|o| o.caps.cpu_millis),
        cap_mem_bytes: offer.as_ref().map(|o| o.caps.mem_bytes),
        running: Some(active.iter().filter(|r| !r.node_queued).count() as u32),
        parallel: offer.as_ref().map(|o| o.parallel_max),
        ws_running: Some(ws_running),
        ws_cap: Some(grant_parallel),
        ..Default::default()
    };
    // The scheduler's own message already carries its numbers; the grant
    // cap gets the PRD's words.
    let words = if why.code == scheduler::GRANT_PARALLEL_FULL { share::wait_words(&why.code, &facts) } else { why.message.clone() };
    let passes = why.code == scheduler::GRANT_PARALLEL_FULL || pshare::blocked::lets_others_pass(&why.code);
    let tail = if passes { "; jobs behind it may start first" } else { "" };
    let line = if on_shared {
        format!("waits on this server for {node_name} ({pos} of {}) — {words}{tail}", queued.len())
    } else {
        format!("queued on {node_name}: {pos} of {} — {words}{tail}", queued.len())
    };
    Some(serde_json::json!({
        "where": if on_shared { "server" } else { "node" },
        "position": pos,
        "of": queued.len(),
        "reason": why.code,
        "message": why.message,
        "waitReason": why.code,
        "waitWords": words,
        "holdsQueue": !passes,
        "line": line,
        "ahead": ahead,
        "parallel": parallel,
        "tier": j.tier().as_str(),
        "aheadTiers": {"ci": ahead_ci, "priority": ahead_p},
    }))
}

/// A node's name for a line (its id when it's gone).
fn node_name_of(node_id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::node_by_id(&conn, node_id).map(|n| n.name).unwrap_or_else(|| node_id.to_string())
}

/// How long `queue` waits for a node's `queue_view`.
const QUEUE_WAIT: Duration = Duration::from_secs(10);

/// `GET /cli/compute/queue?node=N` (`k2 compute queue <node>`, §7, §9):
/// a shared node's whole queue as this server may see it. Other servers'
/// entries carry only class, state, size and ETA (the node redacts them;
/// the owner server also sees their labels and totals). This server's own
/// entries get their short id and command, and a passport sees those only
/// for its own workspace. A solo node (unshared, or without a queue): this
/// server's own running and queued jobs there, as today.
pub(crate) fn queue(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    need_credential(who)?;
    let n = node_or_404(params.get("node").map(String::as_str))?;
    node_visible(who, &n)?;
    let viewer = who.passport();
    let now = compute::now();
    let (online, _, live) = compute_ws::live_view(&n.id);
    let own_entry = |r: &store::JobRow, e: &mut serde_json::Value| {
        if viewer.is_none_or(|(ws, _)| ws == r.workspace_id) {
            let mut argv = r.argv.join(" ");
            if argv.chars().count() > 80 {
                argv = argv.chars().take(79).collect::<String>() + "…";
            }
            e["short"] = compute::message::short(&r.id).into();
            e["workspace"] = ws_handle(&r.workspace_id).into();
            e["argv"] = argv.into();
            e["mySession"] = viewer.is_some_and(|(_, sid)| r.session_id.as_deref() == Some(sid)).into();
        } else {
            // Another workspace on this server: state and size only.
            e["otherWorkspace"] = true.into();
        }
        priority_who(r, e);
    };
    if online && live.as_ref().is_some_and(share::is_shared) {
        let view = compute_ws::queue_get(&n.id, QUEUE_WAIT).map_err(|(code, hint)| match code {
            c if c == codes::NODE_OFFLINE => no(codes::NODE_OFFLINE, 409, format!("{} is offline: {hint}", n.name)),
            _ => no("queue_timeout", 504, format!("{}: {hint}", n.name)),
        })?;
        let entries: Vec<serde_json::Value> = view
            .entries
            .iter()
            .map(|q| {
                let mut e = serde_json::json!({
                    "pos": q.pos,
                    "mine": q.mine,
                    "class": q.class.as_str(),
                    // A node older than 0.46.5 sends no tier: its class says.
                    "tier": q.tier.unwrap_or_else(|| Tier::from_class(q.class)).as_str(),
                    "state": q.state.as_str(),
                    "cpuMillis": q.cpu_millis,
                    "memBytes": q.mem_bytes,
                    "etaSecs": q.eta_secs,
                    "etaP90Secs": q.eta_p90_secs,
                    "blockedBy": q.blocked_by,
                    // The WHY column (L6): the code in words.
                    "why": q.blocked_by.as_deref().map(share::blocked_words),
                    "server": if q.mine { None } else { q.server_label.clone() },
                });
                if let Some(id) = q.job_id.as_deref().filter(|_| q.mine) {
                    let row = {
                        let db = k2_core::db::shared();
                        let conn = db.lock();
                        store::job_by_id(&conn, id)
                    };
                    match row.filter(|r| r.node_id == n.id) {
                        Some(r) => own_entry(&r, &mut e),
                        None => e["otherWorkspace"] = true.into(),
                    }
                }
                e
            })
            .collect();
        let totals: Vec<serde_json::Value> = view
            .totals
            .iter()
            .map(|t| serde_json::json!({"slot": t.slot, "label": t.label, "running": t.running, "queued": t.queued, "cpuSecsToday": t.cpu_secs_today}))
            .collect();
        let counts = queued_tier_counts(entries.iter().filter(|e| e["state"] == "queued").map(|e| e["tier"].as_str().unwrap_or("normal")));
        return Ok(serde_json::json!({"node": n.name, "shared": n.shared, "from": "node", "at": view.at, "entries": entries, "totals": totals, "tiers": counts}));
    }
    // This server's own view of the node.
    let decision = compute_ws::decision_for(&n.id);
    let (active, queued) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (store::jobs_in_states(&conn, &n.id, store::ACTIVE_STATES), store::jobs_in_states(&conn, &n.id, &["queued"]))
    };
    let mut entries = Vec::new();
    for r in &active {
        let mut e = serde_json::json!({"mine": true, "class": r.class, "tier": r.tier().as_str(), "state": r.state, "elapsedSecs": r.started_at.or(r.assigned_at).map(|t| (now - t).max(0))});
        own_entry(r, &mut e);
        entries.push(e);
    }
    let mut waiting: Vec<(usize, &store::JobRow)> = queued.iter().map(|r| (decision.waiting.get(&r.id).map(|(p, _)| *p).unwrap_or(usize::MAX), r)).collect();
    waiting.sort_by_key(|(p, _)| *p);
    for (_, r) in waiting {
        let mut e = serde_json::json!({"mine": true, "class": r.class, "tier": r.tier().as_str(), "state": "queued", "ageSecs": (now - r.created_at).max(0)});
        if let Some((_, w)) = decision.waiting.get(&r.id) {
            e["blockedBy"] = w.code.clone().into();
            e["why"] = w.message.clone().into();
        }
        own_entry(r, &mut e);
        entries.push(e);
    }
    for (i, e) in entries.iter_mut().enumerate() {
        e["pos"] = (i + 1).into();
    }
    let counts = queued_tier_counts(queued.iter().map(|r| r.tier().as_str()));
    Ok(serde_json::json!({"node": n.name, "shared": false, "from": "server", "online": online, "entries": entries, "tiers": counts}))
}

/// `{ci, priority, normal}` counts of queued entries' tiers.
fn queued_tier_counts<'a>(tiers_of: impl Iterator<Item = &'a str>) -> serde_json::Value {
    let mut m: BTreeMap<String, i64> = BTreeMap::new();
    for t in tiers_of {
        *m.entry(t.to_string()).or_default() += 1;
    }
    tier_counts(&m)
}

pub(crate) fn jobs_list(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    need_credential(who)?;
    let gate = passport_gate(who)?;
    let node = match params.get("node").map(String::as_str).filter(|s| !s.is_empty()) {
        Some(t) => Some(node_or_404(Some(t))?.id),
        None => None,
    };
    let ws = match gate {
        Some(own) => Some(own),
        None => match params.get("workspace").map(String::as_str).filter(|s| !s.is_empty()) {
            Some(t) => Some(ws_by_token_or_worktree(t).ok_or_else(|| no_workspace(t))?.id),
            None => None,
        },
    };
    let mine = params.get("mine").is_some_and(|v| v == "1" || v == "true");
    let session = if mine { who.passport().map(|(_, s)| s.to_string()) } else { None };
    let limit = params.get("limit").and_then(|l| l.parse().ok()).unwrap_or(30);
    let state = params.get("state").map(String::as_str).filter(|s| JobState::parse(s).is_some());
    let tier = match params.get("tier").map(String::as_str).filter(|t| !t.is_empty()) {
        None => None,
        Some(t) => Some(Tier::parse(t).ok_or_else(|| no(codes::BAD_REQUEST, 400, "--tier is ci, priority or normal"))?.as_str()),
    };
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_jobs(&conn, &store::JobFilter { node_id: node.as_deref(), workspace_id: ws.as_deref(), state, session_id: session.as_deref(), limit, tier })
    };
    // Queued rows carry the queue view (one scheduler pass per node).
    let mut decisions: HashMap<String, scheduler::Decision> = HashMap::new();
    let jobs: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let mut v = job_summary(r);
            if r.state == "queued" {
                let d = decisions.entry(r.node_id.clone()).or_insert_with(|| compute_ws::decision_for(&r.node_id));
                if let Some(q) = queue_view(who, r, d) {
                    v["queue"] = q;
                }
            }
            v
        })
        .collect();
    Ok(serde_json::json!({ "jobs": jobs }))
}

/// A job the node killed at its memory cap (`failed`/`mem_cap`, no exit
/// code): what to try next.
fn mem_cap_hint(j: &store::JobRow) -> Option<String> {
    if j.state != "failed" || j.reason.as_deref() != Some("mem_cap") {
        return None;
    }
    Some(match j.plan().and_then(|p| p.limits.mem_bytes) {
        Some(m) => {
            let g = m as f64 / (1u64 << 30) as f64;
            let shown = if (g - g.round()).abs() < 0.05 { format!("{}G", g.round() as u64) } else { format!("{g:.1}G") };
            if j.ctl.mem_defaulted {
                format!("stopped: it used more than the default {shown} memory reservation (no --mem was given). Pass --mem with what it needs (e.g. --mem 24G for a cargo build), or CARGO_BUILD_JOBS=2")
            } else {
                format!("stopped: it used more than its --mem {shown}. Try a bigger --mem or CARGO_BUILD_JOBS=2")
            }
        }
        None => "killed at the node's memory cap: pass a bigger --mem (k2 compute nodes shows each node's cap) or CARGO_BUILD_JOBS=2".into(),
    })
}

/// What a `get?wait=N` waiter watches: a change in any of these ends the wait.
type WaitPrint = (String, Option<String>, i64, Option<usize>);

/// A job's wait fingerprint; a split run's (compute B) also changes when
/// any of its shards' does, so its CLI can print one line per shard change.
fn wait_fingerprint(j: &store::JobRow) -> (WaitPrint, Vec<WaitPrint>) {
    let one = |j: &store::JobRow| -> WaitPrint {
        let position = if j.node_queued {
            j.node_pos.map(|p| p.max(0) as usize)
        } else {
            (j.state == "queued").then(|| compute_ws::decision_for(&j.node_id).waiting.get(&j.id).map(|(p, _)| *p)).flatten()
        };
        // A node-queued job's wait reason (L6) changes the print too, so a
        // waiter prints the new "why" at once.
        let reason = match (&j.reason, &j.node_blocked_by) {
            (Some(r), Some(b)) if j.node_queued => Some(format!("{r}:{b}")),
            (r, _) => r.clone(),
        };
        (j.state.clone(), reason, j.generation, position)
    };
    let kids = if j.ctl.shards.is_empty() {
        Vec::new()
    } else {
        crate::compute_shards::children(j).iter().map(|(k, _)| one(k)).collect()
    };
    (one(j), kids)
}

/// Re-checks of a waiting `get` happen at most this often: the change
/// counter moves on every log chunk, and a job that prints a lot must not
/// turn a waiter into a hot loop.
const WAIT_RECHECK: Duration = Duration::from_millis(500);

/// `get?wait=N` (N ≤ 25 s; the same parameter compute B uses): block until
/// the job's state, reason, attempt or queue position changes, it ends, or
/// N seconds pass. Counts as following the job, so an attached run
/// (`run --wait`) gets no duplicate `[compute]` message while it waits.
fn wait_for_change(j: store::JobRow, wait: u64) -> R<store::JobRow> {
    if wait == 0 || j.is_terminal() {
        return Ok(j);
    }
    compute_ws::touch_follower(&j.id);
    let deadline = Instant::now() + Duration::from_secs(wait);
    let start = wait_fingerprint(&j);
    let mut row = j;
    loop {
        let seen = compute_ws::change_counter();
        let checked = Instant::now();
        row = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::job_by_id(&conn, &row.id)
        }
        .ok_or_else(|| no(codes::JOB_NOT_FOUND, 404, "the job vanished"))?;
        let now = Instant::now();
        if row.is_terminal() || now >= deadline || wait_fingerprint(&row) != start {
            break;
        }
        compute_ws::wait_change(seen, deadline - now);
        let pause = WAIT_RECHECK.saturating_sub(checked.elapsed()).min(deadline.saturating_duration_since(Instant::now()));
        if !pause.is_zero() {
            std::thread::sleep(pause);
        }
    }
    compute_ws::touch_follower(&row.id);
    Ok(row)
}

pub(crate) fn job_get(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    let j = job_for(who, params.get("job").map(String::as_str))?;
    let wait = params.get("wait").and_then(|w| w.parse::<u64>().ok()).unwrap_or(0).min(25);
    let j = wait_for_change(j, wait)?;
    let mut v = job_summary(&j);
    // `commit` is srcCommit; `dirty` says whether --dirty changes rode along.
    v["dirty"] = j.dirty_sha256.is_some().into();
    if let Some(h) = mem_cap_hint(&j) {
        v["hint"] = h.into();
    }
    // The audit line: who asked for the cancel, and when.
    if j.state == "cancelled" || j.reason.as_deref() == Some("cancel_requested") {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some((by, at)) = store::last_job_event(&conn, &j.id, "job_cancel") {
            v["cancelledBy"] = serde_json::json!({"by": by, "at": at});
        }
    }
    if j.state == "queued" {
        if let Some(q) = queue_view(who, &j, &compute_ws::decision_for(&j.node_id)) {
            v["queue"] = q;
        }
    }
    v["nodeOnline"] = compute_ws::is_online(&j.node_id).into();
    let node = v["node"].as_str().map(str::to_string);
    v["result"] = if j.ctl.shards.is_empty() {
        compute::result::envelope(&j, node.as_deref())
    } else {
        compute::result::envelope_with_shards(&j, &crate::compute_shards::children(&j))
    };
    if let Some(s) = v.get("secrets").cloned() {
        v["result"]["secrets"] = s;
    }
    Ok(v)
}

/// The job has left the queue; a queued-only cancel (`ifState: queued`,
/// what the CLI sends without `--force`) refuses it.
pub(crate) const JOB_STARTED: &str = "job_started";

pub(crate) fn job_cancel(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    let v = json_body(body)?;
    let queued_only = match s(&v, "ifState") {
        None => false,
        Some("queued") => true,
        Some(_) => return Err(no(codes::BAD_REQUEST, 400, "ifState is \"queued\" (or leave it out to cancel in any state)")),
    };
    // Visibility (a passport sees only its workspace's jobs).
    let seen = job_for(who, s(&v, "job"))?;
    if !seen.ctl.shards.is_empty() {
        // A split run (compute B): cancel every shard that hasn't ended.
        // Without --force (slice A's `ifState: queued`), a run with a
        // started shard is refused as a whole, before anything is cancelled.
        let short = compute::message::short(&seen.id).to_string();
        if seen.is_terminal() {
            return Err(no("already_ended", 409, format!("job {short} already ended ({})", seen.state)));
        }
        let kids: Vec<store::JobRow> = crate::compute_shards::children(&seen).into_iter().map(|(k, _)| k).filter(|k| !k.is_terminal()).collect();
        let started = kids.iter().filter(|k| k.state != "queued").count();
        if queued_only && started > 0 {
            return Err(no(
                JOB_STARTED,
                409,
                format!("split run {short} has {started} shard(s) already started; k2 compute cancel {short} --force stops them"),
            ));
        }
        let mut n = 0;
        for k in &kids {
            if cancel_one(who, k, false).is_ok() {
                n += 1;
            }
        }
        return Ok(serde_json::json!({"ok": true, "state": seen.state, "cancel": format!("sent to {n} shard(s)")}));
    }
    cancel_one(who, &seen, queued_only)
}

/// Cancel one job: queued, at once (compare-and-set); on a node, the node
/// is told, unless `queued_only` (no --force), which refuses it.
pub(crate) fn cancel_one(who: &Who, seen: &store::JobRow, queued_only: bool) -> R<serde_json::Value> {
    let by = who.label();
    let short = compute::message::short(&seen.id).to_string();
    let node_name = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, &seen.node_id).map(|n| n.name).unwrap_or_else(|| seen.node_id.clone())
    };
    // Decide under the placement lock on a fresh read: `kick` can't assign
    // between the read and the write, and the queued path is a
    // compare-and-set, so `cancelled` never lands on a job that runs.
    enum Done {
        Queued,
        Sent(store::JobRow, bool),
        /// A shared node holds it in its queue: asked to cancel it only if
        /// it hasn't started (V57); it ends `cancelled` or `job_started`.
        Asked(bool),
    }
    let done = compute_ws::with_kick_lock(|| -> R<Done> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let j = store::job_by_id(&conn, &seen.id).ok_or_else(|| no(codes::JOB_NOT_FOUND, 404, format!("no job '{short}'")))?;
        if j.is_terminal() {
            return Err(no("already_ended", 409, format!("job {short} already ended ({})", j.state)));
        }
        // The one who asked for the job is cancelling it: its `cancelled`
        // end needs no completion message (they just did it).
        if by == j.requested_by && !j.ctl.cancelled_by_requester {
            let mut ctl = j.ctl.clone();
            ctl.cancelled_by_requester = true;
            store::set_job_ctl(&conn, &j.id, &ctl).map_err(|e| no("internal", 500, e))?;
        }
        if j.state == "queued" && store::cancel_if_queued(&conn, &j.id, compute::now()).map_err(|e| no("internal", 500, e))? {
            store::record_event(
                &conn,
                &by,
                "job_cancel",
                Some(&j.node_id),
                Some(&j.workspace_id),
                Some(&j.id),
                serde_json::json!({"state": "queued", "by": by, "force": !queued_only}),
            );
            return Ok(Done::Queued);
        }
        let j = store::job_by_id(&conn, &seen.id).ok_or_else(|| no(codes::JOB_NOT_FOUND, 404, format!("no job '{short}'")))?;
        if queued_only && j.node_queued && j.reason.as_deref() != Some("cancel_requested") {
            // V57: the node answers its real state; a started job is never
            // touched without --force.
            let sent = j.reason.as_deref() == Some(pending::CANCEL)
                || compute_ws::send_cancel_if_not_started(&j.node_id, &j.id, j.generation, k2_core::compute::proto::frames::CANCEL_IF_QUEUED);
            store::set_job_reason(&conn, &j.id, Some(pending::CANCEL), None);
            store::record_event(
                &conn,
                &by,
                "job_cancel",
                Some(&j.node_id),
                Some(&j.workspace_id),
                Some(&j.id),
                serde_json::json!({"state": "queued", "onNode": true, "sent": sent, "by": by, "force": false}),
            );
            return Ok(Done::Asked(sent));
        }
        if queued_only {
            let since = j.started_at.or(j.assigned_at).map(|t| compute::message::duration(compute::now() - t));
            return Err(no(
                JOB_STARTED,
                409,
                format!(
                    "job {short} {} on {node_name}{}; k2 compute cancel {short} --force stops it",
                    if j.state == "assigned" { "was just assigned" } else { "started" },
                    since.map(|d| format!(" {d} ago")).unwrap_or_default(),
                ),
            ));
        }
        let sent = compute_ws::send_cancel(&j.node_id, &j.id, j.generation);
        store::set_job_reason(&conn, &j.id, Some("cancel_requested"), None);
        store::record_event(
            &conn,
            &by,
            "job_cancel",
            Some(&j.node_id),
            Some(&j.workspace_id),
            Some(&j.id),
            serde_json::json!({"state": j.state, "sent": sent, "by": by, "force": true}),
        );
        Ok(Done::Sent(j, sent))
    })?;
    match done {
        Done::Queued => {
            compute_ws::finalize(&seen.id);
            compute_ws::notify_change();
            Ok(serde_json::json!({"ok": true, "state": "cancelled", "was": "queued", "node": node_name, "by": by}))
        }
        Done::Sent(j, sent) => {
            compute_ws::notify_change();
            let ran = j.started_at.or(j.assigned_at).map(|t| compute::now() - t);
            Ok(serde_json::json!({"ok": true, "state": j.state, "was": j.state, "node": node_name, "by": by,
                "ranSecs": ran, "cancel": if sent { "sent" } else { "delivered when the node reconnects" }}))
        }
        Done::Asked(sent) => {
            compute_ws::notify_change();
            Ok(serde_json::json!({"ok": true, "state": "cancel_requested", "was": "queued", "onNode": true, "node": node_name, "by": by,
                "cancel": if sent { "asked the node (only if it hasn't started)" } else { "asked when the node reconnects" },
                "note": format!("it ends cancelled, or keeps running if it just started (job_started); k2 compute get {short}")}))
        }
    }
}
