//! This server's own labels on compute nodes (0.46.18):
//! `k2 compute node labels [<node>]` and
//! `k2 compute node label <node> add|remove <label>…`, and the
//! `--label WORD` of `k2 compute run` (only nodes carrying every such
//! label for this server).
//!
//! The labels live in this server's database
//! (`k2_core::compute::server_labels`): never sent to the node, never seen
//! by another server that uses it. Reading them is every caller's that may
//! list nodes (an agent sees its granted nodes); changing them is the
//! owner's, an Admin's, or a local agent's under "Allow managing compute
//! nodes" (as `node pause`).

use std::collections::HashMap;

use k2_core::compute::{self as compute, codes, server_labels, store};

use crate::compute_manage::Manager;
use crate::compute_routes::{admin_or_manager, json_body, need_credential, no, node_or_404, passport_gate, s, Who, R};
use crate::compute_views::granted_node_ids;

/// `GET ?node=` (optional): each node's labels for this server.
pub const LIST: &str = "/cli/compute/nodes/labels";
/// `POST {node, add:[…], remove:[…]}`.
pub const SET: &str = "/cli/compute/nodes/labels/set";

pub const ROUTES: &[&str] = &[LIST, SET];
/// POST-only (the dispatcher's 405 guard, via `compute_routes::is_post_route`).
pub const POST_ROUTES: &[&str] = &[SET];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>, mgr: Option<&Manager>) -> R<serde_json::Value> {
    match path {
        LIST => list(who, params, mgr),
        SET => set(who, body, mgr),
        _ => Err(no("not_found", 404, "no such compute labels route")),
    }
}

fn list(who: &Who, params: &HashMap<String, String>, mgr: Option<&Manager>) -> R<serde_json::Value> {
    need_credential(who)?;
    // As `k2 compute nodes`: an agent sees its workspace's granted nodes
    // (and needs compute on); a manager and a person see every node.
    let gate = if mgr.is_some() { None } else { passport_gate(who)? };
    let (rows, labels) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (store::list_nodes(&conn, false), server_labels::all(&conn))
    };
    let granted = gate.as_deref().map(granted_node_ids);
    let want = params.get("node").map(|t| t.trim()).filter(|t| !t.is_empty());
    let mut nodes: Vec<serde_json::Value> = rows
        .iter()
        .filter(|n| n.state != "revoked")
        .filter(|n| granted.as_ref().is_none_or(|g| g.contains(&n.id)))
        .filter(|n| want.is_none_or(|t| n.name == t || n.id == t))
        .map(|n| serde_json::json!({"node": n.name, "id": n.id, "labels": labels.of(&n.id)}))
        .collect();
    nodes.sort_by(|a, b| a["node"].as_str().cmp(&b["node"].as_str()));
    if let (Some(t), true) = (want, nodes.is_empty()) {
        return Err(no(codes::NODE_NOT_FOUND, 404, format!("no compute node '{t}' you can see ({} compute nodes)", k2_core::brand::CLI_NAME)));
    }
    Ok(serde_json::json!({"nodes": nodes, "note": "these labels are this server's own: the node and other servers that use it never see them"}))
}

/// The labels in `v[key]`: a list of strings (absent: none).
fn words(v: &serde_json::Value, key: &str) -> R<Vec<String>> {
    match v.get(key) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .map(|x| x.as_str().map(str::to_string).ok_or_else(|| no(codes::BAD_REQUEST, 400, format!("{key} is a list of labels"))))
            .collect(),
        Some(_) => Err(no(codes::BAD_REQUEST, 400, format!("{key} is a list of labels"))),
    }
}

fn set(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<serde_json::Value> {
    admin_or_manager(who, mgr)?;
    let v = json_body(body)?;
    let n = node_or_404(s(&v, "node"))?;
    let (add, remove) = (words(&v, "add")?, words(&v, "remove")?);
    if add.is_empty() && remove.is_empty() {
        return Err(no(codes::BAD_REQUEST, 400, "say what to change: add or remove a label"));
    }
    let (now, changed) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let (now, changed) = server_labels::change(&conn, &n.id, &add, &remove, &who.label(), compute::now()).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
        if changed {
            store::record_event(&conn, &who.label(), "server_labels", Some(&n.id), None, None, serde_json::json!({"add": add, "remove": remove, "labels": now}));
        }
        (now, changed)
    };
    if let (Some(m), true) = (mgr, changed) {
        let joined = now.iter().cloned().collect::<Vec<_>>().join(",");
        crate::compute_manage::audit(m, "node-labels", &n.name, &[("labels", joined.as_str())]);
    }
    Ok(serde_json::json!({"ok": true, "node": n.name, "labels": now, "changed": changed,
        "note": "this server's own labels: only its jobs (--label WORD) see them; jobs already queued or running are not moved"}))
}

/// `serverLabels` in a `run` body (`--label WORD`): normalized, sorted,
/// each once. Only for auto-placement: a job that names a node is refused
/// (the node is the choice), and one that `--cache` pins to its cache's
/// node is refused when that node doesn't carry them all.
pub(crate) fn from_run(v: &serde_json::Value, auto: bool, cache_pin: Option<&store::NodeRow>) -> R<Vec<String>> {
    let raw = words(v, "serverLabels")?;
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = raw.iter().map(|l| server_labels::normalize(l)).collect::<Result<Vec<_>, _>>().map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    out.sort();
    out.dedup();
    if !auto {
        return Err(no(
            codes::BAD_REQUEST,
            400,
            "--label WORD is for auto-placement; this job names a node (or --same-node-as), which pins it there. Leave the node off, or drop the label",
        ));
    }
    if let Some(n) = cache_pin {
        let has = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            server_labels::all(&conn).has_all(&n.id, &out)
        };
        if !has {
            return Err(no(
                codes::BAD_REQUEST,
                400,
                format!("--cache pins this job to {}, which doesn't carry the label {} here; drop --cache or the label", n.name, out.join(", ")),
            ));
        }
    }
    Ok(out)
}

/// The longest `--for` version.
pub const FOR_MAX: usize = 32;

/// `forVersion` in a `run` body (`--for <version>`, 0.46.18): the release
/// the job's work is for, kept on the job (`JobCtl::for_version`). A
/// version: starts with a digit, then digits, letters and `. + -`, at most
/// [`FOR_MAX`] (`1.2.0`, `1.3.0-rc1`); a leading `v` is dropped.
pub(crate) fn for_from(v: &serde_json::Value) -> R<Option<String>> {
    let raw = match v.get("forVersion") {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(serde_json::Value::String(t)) => t.trim(),
        Some(_) => return Err(no(codes::BAD_REQUEST, 400, "--for is a version, like 1.2.0")),
    };
    let t = raw.strip_prefix('v').unwrap_or(raw);
    let ok = !t.is_empty()
        && t.len() <= FOR_MAX
        && t.starts_with(|c: char| c.is_ascii_digit())
        && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'));
    if !ok {
        return Err(no(codes::BAD_REQUEST, 400, format!("--for is a version, like 1.2.0 (got '{raw}')")));
    }
    Ok(Some(t.to_string()))
}

#[cfg(test)]
#[path = "compute_node_labels_tests.rs"]
mod tests;
