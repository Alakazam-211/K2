//! k2-node rollouts, controller side (prd-compute-update-continuity-v1
//! §5.10, rev 3; migration 0270).
//!
//! - **Start**: `k2 compute node update --all [--batch N] [--now]` (an
//!   operator), or automatically once this server runs a newer version than
//!   its last rollout (`compute.nodes.follow`, on unless set `off`), 5 min
//!   after boot. Every node this server owns (never a tenant node) gets a
//!   place: `pending`, or `skipped` with a code (`node_needs_manual_install`
//!   for k2-node older than 0.46.4, whose updater can't run a new build;
//!   `up_to_date`).
//! - **Send**: all at once by default (Rosson Q3), or `--batch N` in flight
//!   at a time; canary-ring nodes first, then by name. An offline node stays
//!   `pending` and goes when it attaches. The command adds no args (C16).
//! - **Settle**: each node's own `update_state` (`healthy`, `rolled_back`,
//!   `refused`, `failed`), or reattaching on the new version. Per-node
//!   limit 10 min, 35 min while it reports `staged` (a pre-0.46.9 node's
//!   drain; C12). The first `rolled_back` pauses the rollout and alerts the
//!   owner (resume / stop by hand).
//! - Runs from the 60 s node-admin watch (headless, no client needed), on
//!   every attach and on every update state: it survives this server's own
//!   restarts (the rows are the truth).

use std::time::Duration;

use k2_core::compute::proto::admin::UpdatePhase;
use k2_core::compute::rollout::{self as ro, Rollout};
use k2_core::compute::{self as compute, codes, store};
use serde_json::{json, Value};

use crate::compute_routes::{json_body, no, s, Who, R};

/// A node's limit once offered (C12).
pub const NODE_LIMIT_SECS: i64 = 600;
/// …while it reports `staged` (an older node draining first).
pub const NODE_LIMIT_STAGED_SECS: i64 = 35 * 60;
/// Auto-follow waits this long after the daemon starts.
pub const FOLLOW_AFTER_SECS: i64 = 300;
/// The setting that turns auto-follow off (`off`).
pub const FOLLOW_SETTING: &str = "compute.nodes.follow";
/// The first k2-node whose updater works remotely (the 0.46.4 fix).
pub const UPDATER_FIX: &str = "0.46.4";

pub const NEEDS_MANUAL_INSTALL: &str = "node_needs_manual_install";
pub const UP_TO_DATE: &str = "up_to_date";
pub const ROLLOUT_PAUSED: &str = "rollout_paused";

static BOOTED_AT: std::sync::OnceLock<i64> = std::sync::OnceLock::new();

fn booted_at() -> i64 {
    *BOOTED_AT.get_or_init(compute::now)
}

/// Why a node takes no part, or `None` when it does (at start; whether it
/// is online and admin-ready is checked when it's sent).
fn skip_reason(n: &store::NodeRow, version: &str) -> Option<(&'static str, String)> {
    let v = n.node_version.as_deref();
    if crate::compute_admin::newer_than(UPDATER_FIX, v) {
        return Some((
            NEEDS_MANUAL_INSTALL,
            format!(
                "k2-node {} can't take a remote update (its updater was fixed in {UPDATER_FIX}): install {version} at the machine once (sudo k2-node setup with the new k2-node)",
                v.unwrap_or("?")
            ),
        ));
    }
    if !crate::compute_admin::newer_than(version, v) {
        return Some((UP_TO_DATE, format!("already on {}", v.unwrap_or("?"))));
    }
    None
}

/// Start a rollout of this server's k2-node build. A running or paused one
/// to the same version is returned as it is.
pub(crate) fn start(by: &str, trigger: &str, batch: i64, now_flag: bool) -> R<Value> {
    let version = crate::compute_admin::offered_version();
    let now = compute::now();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(r) = ro::open(&conn).into_iter().find(|r| r.to_version == version) {
            drop(conn);
            return Ok(json!({"ok": true, "rollout": rollout_json(&r), "note": "a rollout to this version is already open"}));
        }
    }
    let id = k2_core::compute::proto::crypto::random_id().map_err(|e| no("random", 500, e))?;
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_nodes(&conn, false)
    };
    let mut places: Vec<(String, &str, Option<String>, Option<String>)> = Vec::new();
    for n in rows.iter().filter(|n| n.state == "active" && !n.is_tenant()) {
        match skip_reason(n, &version) {
            Some((code, detail)) => places.push((n.id.clone(), ro::N_SKIPPED, Some(code.to_string()), Some(detail))),
            None => places.push((n.id.clone(), ro::N_PENDING, None, None)),
        }
    }
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let r = ro::create(&conn, &id, &version, trigger, batch, now_flag, by, now, &places).map_err(|e| no("rollout_failed", 500, e))?;
        store::record_event(&conn, by, "rollout_started", None, None, None, json!({"rollout": id, "version": version, "trigger": trigger, "nodes": places.len()}));
        r
    };
    tick(now);
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ro::get(&conn, &r.id).unwrap_or(r)
    };
    Ok(json!({"ok": true, "rollout": rollout_json(&r), "note": "running jobs keep running on nodes from k2-node 0.46.9 on; older nodes drain or stop theirs once"}))
}

fn rollout_json(r: &Rollout) -> Value {
    let (nodes, names) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let ns = ro::nodes(&conn, &r.id);
        let names: std::collections::HashMap<String, String> = store::list_nodes(&conn, true).into_iter().map(|n| (n.id, n.name)).collect();
        (ns, names)
    };
    let list: Vec<Value> = nodes
        .iter()
        .map(|n| {
            json!({"node": names.get(&n.node_id).cloned().unwrap_or_else(|| n.node_id.clone()), "nodeId": n.node_id, "state": n.state,
                   "code": n.code, "detail": n.detail, "at": n.at, "offeredAt": n.offered_at})
        })
        .collect();
    let settled = nodes.iter().filter(|n| ro::node_done(&n.state)).count();
    let mut v = serde_json::to_value(r).unwrap_or_default();
    v["nodes"] = list.into();
    v["position"] = json!({"settled": settled, "total": nodes.len()});
    v
}

/// `GET /cli/compute/nodes/rollout`: the newest rollout.
pub(crate) fn show(who: &Who) -> R<Value> {
    crate::compute_admin::operator_check(who)?;
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ro::latest(&conn)
    };
    Ok(match r {
        Some(r) => json!({"rollout": rollout_json(&r), "follow": follow_on()}),
        None => json!({"rollout": null, "follow": follow_on()}),
    })
}

/// `POST …/rollout/pause|resume|stop` (`{"id"?}`; the newest open one).
pub(crate) fn set(who: &Who, path: &str, body: &[u8]) -> R<Value> {
    crate::compute_admin::operator_check(who)?;
    let v = json_body(body)?;
    let now = compute::now();
    let want = match path {
        crate::compute_admin::NODE_ROLLOUT_PAUSE => ro::PAUSED,
        crate::compute_admin::NODE_ROLLOUT_RESUME => ro::RUNNING,
        _ => ro::STOPPED,
    };
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let r = match s(&v, "id") {
            Some(id) => ro::get(&conn, id),
            None => ro::open(&conn).into_iter().last(),
        };
        let r = r.ok_or_else(|| no("rollout_not_found", 404, "no open rollout (k2 compute node update --all starts one)"))?;
        if !matches!(r.state.as_str(), ro::RUNNING | ro::PAUSED) {
            return Err(no(codes::BAD_REQUEST, 409, format!("rollout {} is {}", r.id, r.state)));
        }
        ro::set_state(&conn, &r.id, want, Some(&format!("{want} by {}", who.label())), now);
        store::record_event(&conn, &who.label(), &format!("rollout_{want}"), None, None, None, json!({"rollout": r.id}));
        ro::get(&conn, &r.id).unwrap_or(r)
    };
    if want == ro::RUNNING {
        tick(now);
    }
    Ok(json!({"ok": true, "rollout": rollout_json(&r)}))
}

fn follow_on() -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::compute::admin::setting(&conn, FOLLOW_SETTING).as_deref() != Some("off")
}

/// §5.10 auto-follow: this server runs a newer version than any rollout it
/// started, it owns nodes, it's been up 5 min, and follow isn't off.
pub(crate) fn maybe_follow(now: i64) {
    if now - booted_at() < FOLLOW_AFTER_SECS || !follow_on() {
        return;
    }
    let version = crate::compute_admin::offered_version();
    let (seen, any_behind) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let seen = ro::latest_to(&conn, &version).is_some();
        let behind = store::list_nodes(&conn, false)
            .iter()
            .any(|n| n.state == "active" && !n.is_tenant() && skip_reason(n, &version).is_none());
        (seen, behind)
    };
    if seen || !any_behind {
        return;
    }
    match start("k2 (follows this server's update)", "auto", 0, false) {
        Ok(_) => k2_core::log_debug!("[compute] rollout of k2-node {version} started (auto-follow)"),
        Err(e) => k2_core::log_debug!("[compute] auto-follow rollout: {}", e.hint),
    }
}

/// Something changed (an attach): look again soon, off the caller's thread.
pub(crate) fn kick() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(2));
        if k2_core::db::try_shared().is_some() {
            tick(compute::now());
        }
    });
}

/// A node's own update state for `version`: settle its place.
pub(crate) fn on_update_state(node_id: &str, phase: UpdatePhase, version: &str) {
    let now = compute::now();
    let paused = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some((r, n)) = ro::node_in_open(&conn, node_id) else { return };
        if r.to_version != version {
            return;
        }
        let state = match phase {
            UpdatePhase::Healthy => ro::N_HEALTHY,
            UpdatePhase::RolledBack => ro::N_ROLLED_BACK,
            UpdatePhase::Refused => ro::N_REFUSED,
            UpdatePhase::Failed => ro::N_FAILED,
            UpdatePhase::Staged | UpdatePhase::Swapped => return,
        };
        if !ro::set_node(&conn, &r.id, &n.node_id, state, None, None, now) {
            return;
        }
        if state == ro::N_ROLLED_BACK && r.state == ro::RUNNING {
            ro::set_state(&conn, &r.id, ro::PAUSED, Some("paused: a node rolled back"), now);
            store::record_event(&conn, "k2", "rollout_paused", Some(node_id), None, None, json!({"rollout": r.id, "version": version}));
            Some(r.id)
        } else {
            None
        }
    };
    if let Some(id) = paused {
        let name = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::node_by_id(&conn, node_id).map(|n| n.name).unwrap_or_else(|| node_id.to_string())
        };
        crate::compute_admin::send_alert(
            node_id,
            ROLLOUT_PAUSED,
            &format!("k2-node {version} rolled back on {name}; the rollout ({id}) paused. Look at k2 compute node health {name}, then k2 compute node rollout resume or stop"),
        );
    }
    tick(now);
}

/// One pass over open rollouts: settle, time out, send, finish.
pub(crate) fn tick(now: i64) {
    let open = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ro::open(&conn)
    };
    for r in open.into_iter().filter(|r| r.state == ro::RUNNING) {
        tick_one(&r, now);
    }
}

fn tick_one(r: &Rollout, now: i64) {
    let (places, rows) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (ro::nodes(&conn, &r.id), store::list_nodes(&conn, true))
    };
    let row = |id: &str| rows.iter().find(|n| n.id == id).cloned();
    let mut in_flight = 0i64;
    for p in places.iter().filter(|p| p.state == ro::N_OFFERED) {
        let Some(n) = row(&p.node_id) else { continue };
        // Back on the new build (the healthy frame may come on a later attach).
        if n.node_version.as_deref() == Some(r.to_version.as_str()) {
            let db = k2_core::db::shared();
            let conn = db.lock();
            ro::set_node(&conn, &r.id, &n.id, ro::N_HEALTHY, None, Some("attached on the new version"), now);
            continue;
        }
        let staged = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::compute::admin::admin_cols(&conn, &n.id).update.as_ref().and_then(|u| u.get("state").and_then(Value::as_str).map(str::to_string)).as_deref() == Some("staged")
        };
        let limit = if staged { NODE_LIMIT_STAGED_SECS } else { NODE_LIMIT_SECS };
        if now - p.offered_at.unwrap_or(p.at) >= limit {
            let db = k2_core::db::shared();
            let conn = db.lock();
            ro::set_node(&conn, &r.id, &n.id, ro::N_TIMED_OUT, Some("node_update_timeout"), Some(&format!("no healthy report within {} min", limit / 60)), now);
            continue;
        }
        in_flight += 1;
    }
    // Canary ring first, then by name.
    let mut pending: Vec<(bool, String, String)> = places
        .iter()
        .filter(|p| p.state == ro::N_PENDING)
        .filter_map(|p| row(&p.node_id))
        .map(|n| {
            let canary = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::compute::admin::admin_cols(&conn, &n.id).update_ring == "canary"
            };
            (!canary, n.name.clone(), n.id.clone())
        })
        .collect();
    pending.sort();
    for (_, _, id) in pending {
        if r.batch_size > 0 && in_flight >= r.batch_size {
            break;
        }
        let Some(n) = row(&id) else { continue };
        if !crate::compute_admin::online(&n.id) {
            continue; // its turn comes when it attaches
        }
        if let Some((code, detail)) = skip_reason(&n, &r.to_version) {
            let db = k2_core::db::shared();
            let conn = db.lock();
            ro::set_node(&conn, &r.id, &n.id, ro::N_SKIPPED, Some(code), Some(&detail), now);
            continue;
        }
        match crate::compute_admin::offer_update(&n, &r.by, r.now, &r.trigger) {
            Ok(_) => {
                let db = k2_core::db::shared();
                let conn = db.lock();
                ro::set_node(&conn, &r.id, &n.id, ro::N_OFFERED, None, None, now);
                store::record_event(&conn, &r.by, "update_offered", Some(&n.id), None, None, json!({"version": r.to_version, "rollout": r.id}));
                in_flight += 1;
            }
            Err(e) if e.code == codes::NODE_OFFLINE => {}
            Err(e) => {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let state = if e.code == k2_core::compute::admin::codes::NODE_LACKS_ADMIN { ro::N_SKIPPED } else { ro::N_FAILED };
                ro::set_node(&conn, &r.id, &n.id, state, Some(e.code), Some(&e.hint), now);
            }
        }
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let places = ro::nodes(&conn, &r.id);
    if places.iter().all(|p| ro::node_done(&p.state)) {
        let ok = places.iter().filter(|p| p.state == ro::N_HEALTHY).count();
        let note = format!("{ok} of {} node(s) updated", places.len());
        ro::set_state(&conn, &r.id, ro::DONE, Some(&note), now);
        store::record_event(&conn, "k2", "rollout_done", None, None, None, json!({"rollout": r.id, "note": note}));
    }
}

/// The node list's view of a node's update and rollout (§5.13).
pub(crate) fn node_view(node_id: &str) -> Value {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let update = k2_core::compute::admin::admin_cols(&conn, node_id).update;
    let rollout = ro::node_latest(&conn, node_id).map(|(r, n)| {
        json!({"id": r.id, "state": r.state, "toVersion": r.to_version, "node": n.state, "code": n.code, "detail": n.detail})
    });
    json!({"update": update, "rollout": rollout})
}
