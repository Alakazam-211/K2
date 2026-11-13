//! What this server lets a compute node take: the controller-side pause
//! (`k2 compute node pause|drain|resume`) and pinned-only
//! (`k2 compute node pinned-only`). The routes are listed and dispatched
//! in `compute_routes`; these are their handlers.

use k2_core::compute::{self as compute, codes, store};

use crate::compute_manage::Manager;
use crate::compute_routes::{admin_or_manager, json_body, need_admin, no, node_or_404, s, Who, R};
use crate::compute_ws;

pub(crate) fn node_pause(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<serde_json::Value> {
    admin_or_manager(who, mgr)?;
    let v = json_body(body)?;
    let n = node_or_404(s(&v, "node"))?;
    let action = s(&v, "action").unwrap_or("pause");
    let pause = match action {
        "pause" => Some("paused"),
        "drain" => Some("draining"),
        "resume" => None,
        _ => return Err(no(codes::BAD_REQUEST, 400, "action is pause, drain or resume")),
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_controller_pause(&conn, &n.id, pause, &who.label(), compute::now()).map_err(|e| no("internal", 500, e))?;
        store::record_event(&conn, &who.label(), "controller_pause", Some(&n.id), None, None, serde_json::json!({"action": action}));
    }
    if let Some(m) = mgr {
        crate::compute_manage::audit(m, &format!("node-{action}"), &n.name, &[]);
    }
    if pause.is_none() {
        // Resuming a retiring node keeps it (the machine was switched back on).
        let _ = compute::access::unmark_retiring_at(&compute::access::retiring_path(), &n.id);
        // …and puts a node set aside for infra failures back at once.
        crate::compute_place::clear_set_aside(&n.id);
    }
    compute_ws::kick(&n.id);
    Ok(serde_json::json!({"ok": true, "node": n.name, "controllerPause": pause,
        "note": "this only stops new assignments from this server; the machine's owner pauses the node itself on that machine"}))
}

/// `k2 compute node pinned-only <node> on|off`: whether automatic
/// placement may choose the node (`k2_core::compute::pinned_only`). The
/// owner or an Admin only: an agent under "Allow managing compute nodes"
/// is refused like any other agent, because this decides which jobs a
/// node set up for one purpose can be given. A node that is added but not
/// yet confirmed can be set, so it is closed before it ever connects.
pub(crate) fn node_pinned_only(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_admin(who)?;
    let v = json_body(body)?;
    let n = node_or_404(s(&v, "node"))?;
    let on = v.get("on").and_then(|x| x.as_bool()).ok_or_else(|| no(codes::BAD_REQUEST, 400, "on is true or false"))?;
    let changed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let changed = compute::pinned_only::set(&conn, &n.id, on, &who.label(), compute::now()).map_err(|e| no("internal", 500, e))?;
        if changed {
            store::record_event(&conn, &who.label(), "pinned_only", Some(&n.id), None, None, serde_json::json!({"on": on}));
        }
        changed
    };
    let note = if on {
        "automatic placement no longer chooses this node; a job runs here only when it names the node. Jobs already queued or running here are not moved"
    } else {
        "automatic placement may choose this node again"
    };
    Ok(serde_json::json!({"ok": true, "node": n.name, "pinnedOnly": on, "changed": changed, "note": note}))
}
