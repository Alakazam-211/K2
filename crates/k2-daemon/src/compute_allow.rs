//! `k2 compute allow-nodes` (0.46.8): a workspace's compute node list
//! ([`k2_core::compute::allow`]). With a list, every job from the
//! workspace goes only to the nodes on it, whoever submits it (an agent,
//! a K2 terminal, the owner) and however (`--sha`, `--dirty`, `--with`,
//! `--shards`, `--same-node-as`, prewarm, retry):
//!
//! - a named node off the list: `node_not_allowed` (403, exit 3), naming
//!   the nodes on it ([`check_node`], from `compute_routes::pick_node`);
//! - auto-placement (`--any`, no node named) and every move: nodes off the
//!   list are not candidates (`compute_place::candidates_report`, skip
//!   `not_allowed`);
//! - a queued job on a node off the list (the list narrowed after it was
//!   placed) never starts there: it ends `failed/node_not_allowed`
//!   ([`fail_queued_off_list`], at the change and on every kick).
//!
//! Who may change it ([`set`]): the owner or an Admin login, any way, any
//! workspace (`--workspace`). An agent passport (or a K2 shell tab, which
//! follows the agent rules) only for its own workspace and only to narrow
//! it (a first list counts: no list means every node); adding a node or
//! clearing the list is `allow_nodes_needs_person`, which the permission
//! cards teach as `compute.allow-nodes` / `compute.allow-nodes-clear`.
//! A Member login can read it, not change it.

use std::collections::HashMap;

use k2_core::compute::allow::{self, Change};
use k2_core::compute::proto::frames::JobState;
use k2_core::compute::{self as compute, store};

use crate::compute_routes::{audit, b, json_body, need_credential, no, owner_only, resolve_node_for, s, target_ws, Who, Ws, R};

/// `GET` the list (`k2 compute allow-nodes`).
pub const SHOW: &str = "/cli/compute/allow-nodes";
/// `POST` set or clear it (`k2 compute allow-nodes <nodes> | --clear`).
pub const SET: &str = "/cli/compute/allow-nodes/set";

/// A job (or a list request) names a node off the workspace's list (exit 3).
pub const NODE_NOT_ALLOWED: &str = "node_not_allowed";
/// An agent's change that adds a node or clears the list (exit 3).
pub const NEEDS_PERSON: &str = "allow_nodes_needs_person";
/// Skip code in a placement record: on the grant, off the list.
pub const SKIP: &str = "not_allowed";

/// The workspace's list as names (a removed node shows as its id with
/// `(removed)`), or `None` when it has none. A read error is a list with
/// nothing on it.
pub(crate) fn names_of(ws_id: &str) -> Option<Vec<String>> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match allow::permit(&conn, ws_id) {
        allow::Permit::Any => None,
        allow::Permit::Only(ids) => Some(ids.iter().map(|id| name_of(&conn, id)).collect()),
    }
}

fn name_of(conn: &rusqlite::Connection, id: &str) -> String {
    store::node_by_id(conn, id).map(|n| n.name).unwrap_or_else(|| format!("{id} (removed)"))
}

fn list_words(names: &[String]) -> String {
    if names.is_empty() {
        "none (the stored list can't be read; the owner sets it again)".to_string()
    } else {
        names.join(", ")
    }
}

/// `node_not_allowed` for a job of `ws` aimed at `node` (`what`: how it got
/// there, "named", "--same-node-as …"). Carries `node`, `nodes` (what a
/// person would add: the permission card's value) and `allowed`.
pub(crate) fn not_allowed(ws: &Ws, node_name: &str, names: &[String], what: &str) -> crate::compute_routes::Refusal {
    no(
        NODE_NOT_ALLOWED,
        403,
        format!(
            "{what} '{node_name}' is not on workspace {}'s compute node list, so its jobs (and the source they sync) never go there. \
             Its list: {}. Name one of those, or no node (K2 picks one on the list). Only a person adds a node: \
             k2 compute allow-nodes --add {node_name} --workspace {} (the owner or an Admin, from a terminal outside K2)",
            ws.handle,
            list_words(names),
            ws.handle
        ),
    )
    .with("node", node_name)
    .with("nodes", node_name)
    .with("allowed", names.to_vec())
    .with("workspace", ws.handle.clone())
}

/// Refuse a job of `ws` on `node` when the node is off the list.
pub(crate) fn check_node(ws: &Ws, node: &store::NodeRow, what: &str) -> R<()> {
    let ok = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        allow::allows(&conn, &ws.id, &node.id)
    };
    if ok {
        return Ok(());
    }
    Err(not_allowed(ws, &node.name, &names_of(&ws.id).unwrap_or_default(), what))
}

/// The detail a job failed for being off the list carries.
fn off_list_detail(node: &str, ws: &str) -> String {
    format!(
        "'{node}' is no longer on workspace {ws}'s compute node list, so this job never started there. \
         Run it again: K2 places it on a node on the list (k2 compute allow-nodes)"
    )
}

/// Fail every still-QUEUED job that sits on a node off its workspace's
/// list: `node` = one node's queue (a kick), `ws` = one workspace's jobs
/// (its list just narrowed). Compare-and-set from `queued`, so a cancel or
/// an assign that won keeps its state. Returns the failed ids; the caller
/// runs `compute_ws::finalize` for each (outside the DB lock) and
/// `notify_change`.
pub(crate) fn fail_queued_off_list(conn: &rusqlite::Connection, node: Option<&str>, ws: Option<&str>) -> Vec<String> {
    let jobs = store::list_jobs(conn, &store::JobFilter { node_id: node, workspace_id: ws, state: Some("queued"), limit: 500, ..Default::default() });
    let mut permits: HashMap<String, allow::Permit> = HashMap::new();
    let mut failed = Vec::new();
    let now = compute::now();
    for j in jobs.into_iter().filter(|j| j.state == "queued" && !j.node_id.is_empty()) {
        let permit = permits.entry(j.workspace_id.clone()).or_insert_with(|| allow::permit(conn, &j.workspace_id));
        if permit.allows(&j.node_id) {
            continue;
        }
        let node_name = name_of(conn, &j.node_id);
        let handle = crate::compute_routes::ws_handle(&j.workspace_id);
        let detail = off_list_detail(&node_name, &handle);
        if store::set_job_state_if(conn, &j.id, "queued", JobState::Failed, Some(NODE_NOT_ALLOWED), Some(&detail), now) == Ok(true) {
            store::record_event(
                conn,
                "controller",
                "fail",
                Some(&j.node_id),
                Some(&j.workspace_id),
                Some(&j.id),
                serde_json::json!({"reason": NODE_NOT_ALLOWED, "detail": detail}),
            );
            failed.push(j.id);
        }
    }
    failed
}

/// [`fail_queued_off_list`] with its own lock, then the requester's
/// `[compute]` message for each and one change notice.
pub(crate) fn sweep_off_list(node: Option<&str>, ws: Option<&str>) -> Vec<String> {
    let failed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        fail_queued_off_list(&conn, node, ws)
    };
    for id in &failed {
        crate::compute_ws::finalize(id);
    }
    if !failed.is_empty() {
        crate::compute_ws::notify_change();
    }
    failed
}

/// Jobs of `ws` already handed to a shared node's own queue (sent, not
/// started) on a node off its list: ask the node to give each back (the
/// withdraw cancel, only if it hasn't started). Back here, an auto job
/// moves to a node on the list or the kick fails it. Returns how many were
/// asked.
fn withdraw_node_queued_off_list(ws: &str) -> usize {
    let jobs: Vec<store::JobRow> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let permit = allow::permit(&conn, ws);
        store::list_jobs(&conn, &store::JobFilter { workspace_id: Some(ws), state: Some("queued"), limit: 500, ..Default::default() })
            .into_iter()
            .filter(|j| j.state == "assigned" && j.node_queued && !permit.allows(&j.node_id))
            .collect()
    };
    jobs.iter()
        .filter(|j| crate::compute_ws::send_cancel_if_not_started(&j.node_id, &j.id, j.generation, k2_core::compute::proto::frames::CANCEL_WITHDRAW))
        .count()
}

/// One node on the list, for the views.
fn node_json(conn: &rusqlite::Connection, ws_id: &str, id: &str) -> serde_json::Value {
    match store::node_by_id(conn, id) {
        Some(n) => serde_json::json!({
            "id": n.id,
            "name": n.name,
            "state": n.state,
            "granted": store::grant(conn, &n.id, ws_id).is_some(),
        }),
        None => serde_json::json!({ "id": id, "name": null, "state": "removed", "granted": false }),
    }
}

/// The list of `ws` for a response (`k2 compute allow-nodes`, `nodes`).
pub(crate) fn view(ws: &Ws) -> R<serde_json::Value> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let entry = allow::get(&conn, &ws.id).map_err(|e| no("internal", 500, format!("the node list of {} can't be read: {e}", ws.handle)))?;
    Ok(match entry {
        None => serde_json::json!({
            "workspace": ws.handle,
            "list": false,
            "nodes": [],
            "note": format!("no node list: {}'s jobs may go to any node granted to it", ws.handle),
        }),
        Some(e) => serde_json::json!({
            "workspace": ws.handle,
            "list": true,
            "nodes": e.nodes.iter().map(|id| node_json(&conn, &ws.id, id)).collect::<Vec<_>>(),
            "updatedBy": e.updated_by,
            "updatedAt": e.updated_at,
            "note": format!("{}'s jobs go only to these nodes (named, --any, moves and retries alike)", ws.handle),
        }),
    })
}

/// `GET /cli/compute/allow-nodes[?workspace=WS]`: anyone who may see the
/// workspace's compute (a passport: its own; others name one).
pub(crate) fn show(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    need_credential(who)?;
    let ws = target_ws(who, params.get("workspace").map(String::as_str))?;
    view(&ws)
}

/// The node names a set request carries (`nodes`: a list or a comma
/// string; `add` / `remove`: the same, applied to the current list).
fn names_in(v: &serde_json::Value, key: &str) -> R<Vec<String>> {
    match v.get(key) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::String(s)) => Ok(s.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect()),
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .map(|x| x.as_str().map(|s| s.trim().to_string()).ok_or_else(|| no(compute::codes::BAD_REQUEST, 400, format!("{key} is a list of node names"))))
            .filter(|r| !matches!(r, Ok(s) if s.is_empty()))
            .collect(),
        Some(_) => Err(no(compute::codes::BAD_REQUEST, 400, format!("{key} is a list of node names"))),
    }
}

/// `POST /cli/compute/allow-nodes/set {workspace?, nodes | add | remove | clear}`.
pub(crate) fn set(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_credential(who)?;
    // A person: the owner token or an Owner/Admin login. A Member login
    // reads only. A passport (agent or K2 shell tab) narrows only.
    let person = who.is_admin();
    if !person && who.passport().is_none() {
        return Err(owner_only());
    }
    let v = json_body(body)?;
    let clear = b(&v, "clear") == Some(true);
    let (nodes, add, remove) = (names_in(&v, "nodes")?, names_in(&v, "add")?, names_in(&v, "remove")?);
    let asks = [!nodes.is_empty(), !add.is_empty(), !remove.is_empty(), clear].iter().filter(|x| **x).count();
    if asks != 1 {
        return Err(no(
            compute::codes::BAD_REQUEST,
            400,
            "say one of: the whole list (k2 compute allow-nodes k2n9,k2-arch), --add NODE, --remove NODE, or --clear",
        ));
    }
    let ws = target_ws(who, s(&v, "workspace"))?;
    let resolve = |names: &[String]| -> R<Vec<String>> {
        let mut ids = Vec::new();
        for name in names {
            ids.push(resolve_node_for(who, Some(name.as_str()), Some(&ws.id))?.id);
        }
        Ok(allow::normalize(&ids))
    };
    let old = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        allow::get(&conn, &ws.id).map_err(|e| no("internal", 500, format!("the node list of {} can't be read: {e}", ws.handle)))?
    };
    let old_ids = old.as_ref().map(|e| e.nodes.clone());
    let new_ids: Option<Vec<String>> = if clear {
        None
    } else if !nodes.is_empty() {
        Some(resolve(&nodes)?)
    } else if !add.is_empty() {
        // Adding to no list leaves no list (every node is allowed already).
        old_ids.as_ref().map(|o| -> R<Vec<String>> { Ok(allow::normalize(&[o.clone(), resolve(&add)?].concat())) }).transpose()?
    } else {
        let Some(o) = old_ids.as_ref() else {
            return Err(no(
                compute::codes::BAD_REQUEST,
                400,
                format!("workspace {} has no node list to remove from; set one: k2 compute allow-nodes <node>[,<node>…]", ws.handle),
            ));
        };
        let gone = resolve(&remove)?;
        let left: Vec<String> = o.iter().filter(|id| !gone.contains(id)).cloned().collect();
        if left.is_empty() {
            return Err(no(
                compute::codes::BAD_REQUEST,
                400,
                "that would leave the list empty; a list keeps at least one node (--clear removes the list: every granted node again)",
            ));
        }
        Some(left)
    };
    let change = allow::change(old_ids.as_deref(), new_ids.as_deref());
    if change == Change::Widen && !person {
        let (added, words) = match new_ids.as_deref() {
            Some(n) => {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let names: Vec<String> = allow::added(old_ids.as_deref(), n).iter().map(|id| name_of(&conn, id)).collect();
                (names.clone(), format!("adding {} to", names.join(", ")))
            }
            None => (Vec::new(), "clearing".to_string()),
        };
        let mut r = no(
            NEEDS_PERSON,
            403,
            format!(
                "{words} workspace {}'s compute node list lets its code reach more machines, so only a person does it: the owner \
                 or an Admin, from a terminal outside K2 (k2 compute allow-nodes … --workspace {}). An agent may only narrow its list",
                ws.handle, ws.handle
            ),
        )
        .with("workspace", ws.handle.clone());
        // The card values: `nodes` (what to add) or `clear` (one of them
        // only, so each refusal names one card).
        if !added.is_empty() {
            r = r.with("nodes", added.join(","));
        }
        if clear {
            r = r.with("clear", true);
        }
        return Err(r);
    }
    let (from, to) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if change != Change::Same {
            allow::set(&conn, &ws.id, new_ids.as_deref(), &who.label(), compute::now()).map_err(|e| no("internal", 500, e))?;
        }
        let names = |ids: Option<&[String]>| ids.map(|i| i.iter().map(|id| name_of(&conn, id)).collect::<Vec<_>>());
        (names(old_ids.as_deref()), names(new_ids.as_deref()))
    };
    let (mut failed, mut withdrawn) = (Vec::new(), 0);
    if change != Change::Same {
        audit(who, "allow_nodes", None, Some(&ws.id), None, serde_json::json!({"from": from, "to": to}));
        // Jobs already queued on a node that just left the list never
        // start there.
        failed = sweep_off_list(None, Some(&ws.id));
        withdrawn = withdraw_node_queued_off_list(&ws.id);
    }
    let mut out = view(&ws)?;
    out["change"] = match change {
        Change::Same => "same",
        Change::Narrow => "narrow",
        Change::Widen => "widen",
    }
    .into();
    out["failedQueued"] = failed.into();
    out["withdrawn"] = withdrawn.into();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_in_takes_a_list_or_a_comma_string() {
        let v = serde_json::json!({"nodes": "k2n9, k2-arch,,", "add": ["a", " "], "remove": 3});
        assert_eq!(names_in(&v, "nodes").expect("nodes"), vec!["k2n9", "k2-arch"]);
        assert_eq!(names_in(&v, "add").expect("add"), vec!["a"]);
        assert_eq!(names_in(&v, "missing").expect("none"), Vec::<String>::new());
        assert_eq!(names_in(&v, "remove").expect_err("a number").code, compute::codes::BAD_REQUEST);
    }

    #[test]
    fn not_allowed_carries_the_card_values_and_names_the_list() {
        let ws = Ws { id: "w".into(), path: "/w".into(), handle: "k2".into() };
        let r = not_allowed(&ws, "k2n2-m1", &["k2-arch".to_string(), "k2n9".to_string()], "node");
        assert_eq!((r.code, r.status), (NODE_NOT_ALLOWED, 403));
        assert!(r.hint.contains("Its list: k2-arch, k2n9.") && r.hint.contains("'k2n2-m1'"), "{}", r.hint);
        let body: serde_json::Value = serde_json::from_str(&crate::compute_routes::refusal_response(&r).body).expect("json");
        assert_eq!(body["error"]["code"], NODE_NOT_ALLOWED);
        assert_eq!(body["error"]["nodes"], "k2n2-m1");
        assert_eq!(body["error"]["allowed"], serde_json::json!(["k2-arch", "k2n9"]));
    }
}
