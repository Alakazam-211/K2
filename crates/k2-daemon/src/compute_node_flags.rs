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
/// node set up for one purpose can be given. An agent asks a person on a
/// card instead (`compute.node.pinned-only`, 0.46.18): the refusal ends
/// with its Ask with: line, and Allow runs [`node_pinned_only_card`]. A
/// node that is added but not yet confirmed can be set, so it is closed
/// before it ever connects.
pub(crate) fn node_pinned_only(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    pinned_only_as(who, body, None)
}

/// The `compute.node.pinned-only` card's Allow (0.46.18): the same change
/// as the route, as the person who tapped (the owner or an Admin), with
/// the agent that asked recorded beside it: the compute event's detail
/// carries `askedBy` and `via: permission-card`, and an Access Audit line
/// ([`PINNED_ONLY_AUDIT`]) says the same.
pub(crate) fn node_pinned_only_card(who: &Who, body: &[u8], asked_by: &str) -> R<serde_json::Value> {
    pinned_only_as(who, body, Some(asked_by))
}

/// The card's Access Audit event.
pub(crate) const PINNED_ONLY_AUDIT: &str = "compute-node-pinned-only";

fn pinned_only_as(who: &Who, body: &[u8], asked_by: Option<&str>) -> R<serde_json::Value> {
    need_admin(who)?;
    let v = json_body(body)?;
    let n = node_or_404(s(&v, "node"))?;
    let on = v.get("on").and_then(|x| x.as_bool()).ok_or_else(|| no(codes::BAD_REQUEST, 400, "on is true or false"))?;
    let changed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let changed = compute::pinned_only::set(&conn, &n.id, on, &who.label(), compute::now()).map_err(|e| no("internal", 500, e))?;
        if changed {
            let mut detail = serde_json::json!({"on": on});
            if let Some(a) = asked_by {
                detail["askedBy"] = serde_json::json!(a);
                detail["via"] = serde_json::json!("permission-card");
            }
            store::record_event(&conn, &who.label(), "pinned_only", Some(&n.id), None, None, detail);
        }
        changed
    };
    if let Some(a) = asked_by {
        k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
            PINNED_ONLY_AUDIT,
            &who.label(),
            format!("node={} on={on} changed={changed} askedBy={a} via=permission-card", n.name),
            "-",
            "-",
            "cli",
        ));
    }
    let note = if on {
        "automatic placement no longer chooses this node; a job runs here only when it names the node. Jobs already queued or running here are not moved"
    } else {
        "automatic placement may choose this node again"
    };
    Ok(serde_json::json!({"ok": true, "node": n.name, "pinnedOnly": on, "changed": changed, "note": note}))
}
