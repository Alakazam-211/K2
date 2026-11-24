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
//! An agent asks only for nodes that exist and are granted to its own
//! workspace (`allow_nodes_not_granted` otherwise, before any card), and
//! never for another workspace's list (`forbidden`, naming who can)
//! unless it manages that workspace ([`manages`], 0.46.18): then the
//! change (`--add`, `--remove` or `--clear`) is refused
//! `allow_nodes_for_needs_person`, which the cards teach as
//! `compute.allow-nodes-for target=<ws> add=|remove=|clear=true`. Only
//! nodes granted to that workspace can be added there.
//! A Member login can read it, not change it.
//!
//! **New nodes join (0.46.18).** A list follows new nodes unless a person
//! fixed it (`--fixed on|off`, the owner or an Admin only:
//! `allow_nodes_fixed_needs_person` for anyone else, not requestable): a
//! node confirmed on this server joins every following list
//! ([`follow_new_node`], from `node confirm` and a seat node's enrolment),
//! one `allow_nodes` event per list (`follow: true`).

use std::collections::HashMap;

use k2_core::compute::allow::{self, Change};
use k2_core::compute::proto::frames::JobState;
use k2_core::compute::{self as compute, store};
use k2_core::brand::PRODUCT_NAME;
use k2_core::brand::{CLI_SLOT, PRODUCT_SLOT};

use crate::compute_routes::{audit, b, json_body, need_credential, no, owner_only, resolve_node_for, s, target_ws, Refusal, Who, Ws, R};
use k2_core::brand::CLI_NAME;

/// [`no`] for a hint that names the command or the product: `template`
/// holds [`k2_core::brand::CLI_SLOT`] and [`k2_core::brand::PRODUCT_SLOT`]
/// where they go. The hint is the template with this server's names, and
/// the template rides beside it as `hintTemplate`, so a CLI of another
/// brand prints its own names there (white-label C5). An older CLI reads
/// the hint as before.
fn named_no(code: &'static str, status: u16, template: impl Into<String>) -> Refusal {
    let template = template.into();
    let hint = k2_core::brand::fill_names(&template);
    if hint == template {
        return no(code, status, hint);
    }
    no(code, status, hint).with("hintTemplate", template)
}

/// `GET` the list (`k2 compute allow-nodes`).
pub const SHOW: &str = "/cli/compute/allow-nodes";
/// `POST` set or clear it (`k2 compute allow-nodes <nodes> | --clear`).
pub const SET: &str = "/cli/compute/allow-nodes/set";

/// A job (or a list request) names a node off the workspace's list (exit 3).
pub const NODE_NOT_ALLOWED: &str = "node_not_allowed";
/// An agent's change that adds a node or clears the list (exit 3).
pub const NEEDS_PERSON: &str = "allow_nodes_needs_person";
/// An agent's change that adds a node not granted to its workspace (exit
/// 3): a list entry alone never lets a job in, so there is nothing to ask
/// for until a person grants the node.
pub const NOT_GRANTED: &str = "allow_nodes_not_granted";
/// Skip code in a placement record: on the grant, off the list.
pub const SKIP: &str = "not_allowed";
/// A managing agent's change to ANOTHER workspace's list (exit 3): a
/// person decides, on the `compute.allow-nodes-for` card (0.46.18).
pub const FOR_NEEDS_PERSON: &str = "allow_nodes_for_needs_person";
/// The card for another workspace's list, asked by an agent that doesn't
/// manage it (409 at the ask).
pub const NOT_MANAGER: &str = "allow_nodes_not_manager";

/// Why an agent may ask for another workspace's node list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Manages {
    /// It hired that workspace (the hire ledger, `k2 agent hire`).
    Hired,
    /// "Let this agent manage compute nodes" is on for it (compute_manage).
    ComputeManager,
}

impl Manages {
    /// "it hired k2-engine" / "it may manage this server's compute nodes".
    pub(crate) fn words(self, target: &str) -> String {
        match self {
            Manages::Hired => format!("it hired {target}"),
            Manages::ComputeManager => "it may manage this server's compute nodes".to_string(),
        }
    }
}

/// May the agent of `asker` ask for `target`'s node list? Never for its
/// own (that is `compute.allow-nodes`).
pub(crate) fn manages(asker: &str, target: &str) -> Option<Manages> {
    if asker.is_empty() || asker == target {
        return None;
    }
    let hired = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::hire::ledger::hired_by_agent(&conn, asker, target)
    };
    if hired {
        return Some(Manages::Hired);
    }
    crate::compute_manage::workspace_allows(asker).then_some(Manages::ComputeManager)
}

/// `allow_nodes_not_granted` for `ws`: these nodes have no grant there.
fn not_granted(ws: &Ws, ungranted: &[String]) -> crate::compute_routes::Refusal {
    let one = ungranted.len() == 1;
    named_no(
        NOT_GRANTED,
        403,
        format!(
            "{} {} not granted to workspace {}, so a node list entry would never let its jobs in. A person grants {} first \
             (Settings → Cores → Compute, or {CLI_SLOT} compute grant <node> --workspace {} from a terminal outside {PRODUCT_SLOT}); then ask to add {}",
            ungranted.join(", "),
            if one { "is" } else { "are" },
            ws.handle,
            if one { "it" } else { "them" },
            ws.handle,
            if one { "it" } else { "them" },
        ),
    )
    .with("workspace", ws.handle.clone())
    .with("notGranted", ungranted.join(","))
}
/// Anyone but a person setting `--fixed` (exit 3, not requestable).
pub const FIXED_NEEDS_PERSON: &str = "allow_nodes_fixed_needs_person";

/// A node was just added to this server (`node confirm`, a seat node):
/// it joins every node list that follows new nodes, each change one
/// `allow_nodes` event (`follow: true`). Returns the handles of the
/// workspaces whose list it joined.
pub(crate) fn follow_new_node(conn: &rusqlite::Connection, node_id: &str, by: &str) -> Vec<String> {
    let joined = match allow::follow_new_node(conn, node_id, &format!("new-node:{by}"), compute::now()) {
        Ok(j) => j,
        Err(e) => {
            store::record_event(conn, by, "allow_nodes_follow_failed", Some(node_id), None, None, serde_json::json!({"why": e}));
            return Vec::new();
        }
    };
    let name = name_of(conn, node_id);
    joined
        .iter()
        .map(|ws| {
            store::record_event(conn, by, "allow_nodes", Some(node_id), Some(ws), None, serde_json::json!({"follow": true, "added": [name]}));
            crate::compute_routes::ws_handle(ws)
        })
        .collect()
}

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
    named_no(
        NODE_NOT_ALLOWED,
        403,
        format!(
            "{what} '{node_name}' is not on workspace {}'s compute node list, so its jobs (and the source they sync) never go there. \
             Its list: {}. Name one of those, or no node ({PRODUCT_SLOT} picks one on the list). Only a person adds a node: \
             {CLI_SLOT} compute allow-nodes --add {node_name} --workspace {} (the owner or an Admin, from a terminal outside {PRODUCT_SLOT}), \
             or they approve the card {}'s agent asks with: {CLI_SLOT} permission request compute.allow-nodes nodes={node_name}",
            ws.handle,
            list_words(names),
            ws.handle,
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
         Run it again: {PRODUCT_NAME} places it on a node on the list ({CLI_NAME} compute allow-nodes)"
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

/// Is any queued job [`fail_queued_off_list`] would fail there? Reads only.
fn any_queued_off_list(conn: &rusqlite::Connection, node: Option<&str>, ws: Option<&str>) -> bool {
    let jobs = store::list_jobs(conn, &store::JobFilter { node_id: node, workspace_id: ws, state: Some("queued"), limit: 500, ..Default::default() });
    let mut permits: HashMap<String, allow::Permit> = HashMap::new();
    jobs.iter()
        .filter(|j| j.state == "queued" && !j.node_id.is_empty())
        .any(|j| !permits.entry(j.workspace_id.clone()).or_insert_with(|| allow::permit(conn, &j.workspace_id)).allows(&j.node_id))
}

/// [`fail_queued_off_list`] with its own lock, then the requester's
/// `[compute]` message for each and one change notice.
pub(crate) fn sweep_off_list(node: Option<&str>, ws: Option<&str>) -> Vec<String> {
    // Every kick (every offer of every node) runs this: nearly always
    // nothing is off a list, so check that on a pooled read and take the
    // main lock only to fail what is (0.46.17: the whole queue was read
    // and checked under the lock each time). The failing pass re-checks.
    if !k2_core::db::read(|conn| any_queued_off_list(conn, node, ws)) {
        return Vec::new();
    }
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
            // 0.46.18: a node newly added to this server joins it unless fixed.
            "fixed": allow::is_fixed(&conn, &ws.id),
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
    let fixed = fixed_in(&v)?;
    let asks = [!nodes.is_empty(), !add.is_empty(), !remove.is_empty(), clear, fixed.is_some()].iter().filter(|x| **x).count();
    if asks != 1 {
        return Err(named_no(
            compute::codes::BAD_REQUEST,
            400,
            format!("say one of: the whole list ({CLI_SLOT} compute allow-nodes linux-1,arch-1), --add NODE, --remove NODE, --clear, or --fixed on|off"),
        ));
    }
    if let Some(on) = fixed {
        return set_fixed(who, s(&v, "workspace"), on);
    }
    let asked = Asked { add: &add, remove: &remove, clear };
    let ws = target_ws(who, s(&v, "workspace")).map_err(|r| other_workspace(who, s(&v, "workspace"), r, &asked))?;
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
            return Err(named_no(
                compute::codes::BAD_REQUEST,
                400,
                format!("workspace {} has no node list to remove from; set one: {CLI_SLOT} compute allow-nodes <node>[,<node>…]", ws.handle),
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
                let ids = allow::added(old_ids.as_deref(), n);
                // A node this workspace has no grant on: no card can help
                // (a list entry never lets a job in), so say so first.
                let ungranted: Vec<String> = ids.iter().filter(|id| store::grant(&conn, id, &ws.id).is_none()).map(|id| name_of(&conn, id)).collect();
                if !ungranted.is_empty() {
                    return Err(not_granted(&ws, &ungranted));
                }
                let names: Vec<String> = ids.iter().map(|id| name_of(&conn, id)).collect();
                (names.clone(), format!("adding {} to", names.join(", ")))
            }
            None => (Vec::new(), "clearing".to_string()),
        };
        let hint = if clear {
            format!(
                "clearing workspace {}'s compute node list opens every node granted to it at once (more than adding the ones it needs), \
                 so only a person does it: the owner or an Admin, from a terminal outside {PRODUCT_SLOT} ({CLI_SLOT} compute allow-nodes --clear --workspace {}). \
                 To reach one more node, ask for just that node: {CLI_SLOT} compute allow-nodes --add <node>. An agent may only narrow its list",
                ws.handle, ws.handle
            )
        } else {
            format!(
                "{words} workspace {}'s compute node list lets its jobs (and the source they sync) reach more machines, so a person decides: \
                 the owner or an Admin, from a terminal outside {PRODUCT_SLOT} ({CLI_SLOT} compute allow-nodes --add {} --workspace {}), or on the card \
                 this agent asks for ({CLI_SLOT} permission request compute.allow-nodes nodes={}). An agent may only narrow its list",
                ws.handle,
                added.join(","),
                ws.handle,
                added.join(",")
            )
        };
        let mut r = named_no(NEEDS_PERSON, 403, hint).with("workspace", ws.handle.clone());
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
        let mut detail = serde_json::json!({"from": from, "to": to});
        // 0.46.18: a person's Allow on a managing agent's card says whose
        // card it was (the permission card's own audit line has the rest).
        if let Some(asker) = s(&v, "askedBy").filter(|_| person) {
            detail["askedBy"] = asker.into();
            detail["via"] = "permission-card".into();
        }
        audit(who, "allow_nodes", None, Some(&ws.id), None, detail);
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

/// `fixed`: `true` / `false`, or `on` / `off` (`None` when absent).
fn fixed_in(v: &serde_json::Value) -> R<Option<bool>> {
    match v.get("fixed") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Bool(x)) => Ok(Some(*x)),
        Some(serde_json::Value::String(t)) if matches!(t.trim(), "on" | "true") => Ok(Some(true)),
        Some(serde_json::Value::String(t)) if matches!(t.trim(), "off" | "false") => Ok(Some(false)),
        Some(_) => Err(named_no(compute::codes::BAD_REQUEST, 400, format!("fixed is on or off ({CLI_SLOT} compute allow-nodes --fixed on|off)"))),
    }
}

/// `--fixed on|off`: whether a node newly added to this server joins the
/// list (0.46.18). A person's only (the owner or an Admin), and only on a
/// list that exists.
fn set_fixed(who: &Who, requested: Option<&str>, on: bool) -> R<serde_json::Value> {
    if !who.is_admin() {
        let words = if on { "fixing" } else { "un-fixing" };
        return Err(named_no(
            FIXED_NEEDS_PERSON,
            403,
            format!(
                "{words} a compute node list decides whether new machines may receive its workspace's code, so only a person does it: \
                 the owner or an Admin, from a terminal outside {PRODUCT_SLOT} ({CLI_SLOT} compute allow-nodes --fixed on|off --workspace <ws>)"
            ),
        ));
    }
    let ws = target_ws(who, requested)?;
    let was = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let has = allow::get(&conn, &ws.id).map_err(|e| no("internal", 500, format!("the node list of {} can't be read: {e}", ws.handle)))?;
        if has.is_none() {
            return Err(named_no(
                compute::codes::BAD_REQUEST,
                400,
                format!(
                    "workspace {} has no node list to fix (its jobs may go to any node granted to it); set one first: \
                     {CLI_SLOT} compute allow-nodes <node>[,<node>…] --workspace {}",
                    ws.handle, ws.handle
                ),
            ));
        }
        let was = allow::is_fixed(&conn, &ws.id);
        if was != on {
            allow::set_fixed(&conn, &ws.id, on, &who.label(), compute::now()).map_err(|e| no("internal", 500, e))?;
        }
        was
    };
    if was != on {
        audit(who, "allow_nodes_fixed", None, Some(&ws.id), None, serde_json::json!({"fixed": on}));
    }
    let mut out = view(&ws)?;
    out["change"] = if was == on { "same" } else { "fixed" }.into();
    Ok(out)
}

/// What a set request asks for besides a whole list (`nodes`).
struct Asked<'a> {
    add: &'a [String],
    remove: &'a [String],
    clear: bool,
}

/// An agent naming another workspace's list (`target_ws`'s `forbidden`).
/// An agent that manages it ([`manages`]) is refused
/// `allow_nodes_for_needs_person` with the card's values (`workspace`, and
/// `add`, `remove` or `clear`), after the checks the card makes too (each
/// node exists; one to add is granted to that workspace). Any other agent
/// can't ask for that list: that workspace's own agent asks, or a person
/// changes it. Any other refusal passes through unchanged.
fn other_workspace(who: &Who, requested: Option<&str>, r: crate::compute_routes::Refusal, asked: &Asked<'_>) -> crate::compute_routes::Refusal {
    let (Some((own, _)), Some(req), "forbidden") = (who.passport(), requested.map(str::trim).filter(|t| !t.is_empty()), r.code) else {
        return r;
    };
    let target = crate::compute_routes::ws_by_token(req);
    if let Some(t) = target.as_ref() {
        if let Some(how) = manages(own, &t.id) {
            return for_needs_person(who, own, t, how, asked).unwrap_or_else(|r| r);
        }
    }
    let handle = target.map(|w| w.handle).unwrap_or_else(|| req.to_string());
    named_no(
        "forbidden",
        403,
        format!(
            "an agent changes only its own workspace's compute node list, never {handle}'s. {handle}'s own agent asks for a node on a card \
             ({CLI_SLOT} permission request compute.allow-nodes nodes=<node>, from its terminal), an agent that manages {handle} (it hired it) \
             asks on {CLI_SLOT} permission request compute.allow-nodes-for target={handle} add=<node>, or a person runs \
             {CLI_SLOT} compute allow-nodes --add <node> --workspace {handle} from a terminal outside {PRODUCT_SLOT}"
        ),
    )
    .with("workspace", handle)
}

/// The managing agent's refusal: the card's values, or why there is no
/// card to ask (an unknown or ungranted node). `Err` is the refusal too.
fn for_needs_person(
    who: &Who,
    own: &str,
    t: &Ws,
    how: Manages,
    asked: &Asked<'_>,
) -> R<crate::compute_routes::Refusal> {
    let me = crate::compute_routes::ws_handle(own);
    let resolve = |names: &[String]| -> R<Vec<store::NodeRow>> {
        let mut rows: Vec<store::NodeRow> = Vec::new();
        for name in names {
            let n = resolve_node_for(who, Some(name.as_str()), Some(&t.id))?;
            if !rows.iter().any(|r| r.id == n.id) {
                rows.push(n);
            }
        }
        Ok(rows)
    };
    let why = format!(
        "{me} manages {} ({}), but another workspace's compute node list decides which machines receive that workspace's code, so a person decides: \
         the owner or an Admin, on the card {me} asks for, or from a terminal outside {PRODUCT_SLOT}",
        t.handle,
        how.words(&t.handle)
    );
    let ask = |what: &str| format!("{CLI_SLOT} permission request compute.allow-nodes-for target={} {what}", t.handle);
    let base = |hint: String| named_no(FOR_NEEDS_PERSON, 403, hint).with("workspace", t.handle.clone());
    if !asked.add.is_empty() {
        let rows = resolve(asked.add)?;
        let ungranted: Vec<String> = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            rows.iter().filter(|n| store::grant(&conn, &n.id, &t.id).is_none()).map(|n| n.name.clone()).collect()
        };
        if !ungranted.is_empty() {
            return Err(not_granted(t, &ungranted));
        }
        let names = rows.iter().map(|n| n.name.clone()).collect::<Vec<_>>().join(",");
        return Ok(base(format!(
            "{why} ({CLI_SLOT} compute allow-nodes --add {names} --workspace {}). The card: {}",
            t.handle,
            ask(&format!("add={names}"))
        ))
        .with("add", names));
    }
    if !asked.remove.is_empty() {
        let names = resolve(asked.remove)?.iter().map(|n| n.name.clone()).collect::<Vec<_>>().join(",");
        return Ok(base(format!(
            "{why} ({CLI_SLOT} compute allow-nodes --remove {names} --workspace {}). The card: {}",
            t.handle,
            ask(&format!("remove={names}"))
        ))
        .with("remove", names));
    }
    if asked.clear {
        return Ok(base(format!(
            "{why} ({CLI_SLOT} compute allow-nodes --clear --workspace {}): clearing opens every node granted to {} at once. The card: {}",
            t.handle,
            t.handle,
            ask("clear=true")
        ))
        .with("clear", "true"));
    }
    // A whole list at once (`nodes`): the card takes one change.
    Ok(base(format!(
        "{why}. The card takes one change at a time: ask for the nodes to add ({}) or to take off ({}); a whole new list is a person's \
         ({CLI_SLOT} compute allow-nodes <node>[,<node>…] --workspace {})",
        ask("add=<node>[,<node>…]"),
        ask("remove=<node>[,<node>…]"),
        t.handle
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_in_takes_a_list_or_a_comma_string() {
        let v = serde_json::json!({"nodes": "rig9, arch-box,,", "add": ["a", " "], "remove": 3});
        assert_eq!(names_in(&v, "nodes").expect("nodes"), vec!["rig9", "arch-box"]);
        assert_eq!(names_in(&v, "add").expect("add"), vec!["a"]);
        assert_eq!(names_in(&v, "missing").expect("none"), Vec::<String>::new());
        assert_eq!(names_in(&v, "remove").expect_err("a number").code, compute::codes::BAD_REQUEST);
    }

    #[test]
    fn not_allowed_carries_the_card_values_and_names_the_list() {
        let ws = Ws { id: "w".into(), path: "/w".into(), handle: "k2".into() };
        let r = not_allowed(&ws, "rig2-mac", &["arch-box".to_string(), "rig9".to_string()], "node");
        assert_eq!((r.code, r.status), (NODE_NOT_ALLOWED, 403));
        assert!(r.hint.contains("Its list: arch-box, rig9.") && r.hint.contains("'rig2-mac'"), "{}", r.hint);
        let body: serde_json::Value = serde_json::from_str(&crate::compute_routes::refusal_response(&r).body).expect("json");
        assert_eq!(body["error"]["code"], NODE_NOT_ALLOWED);
        assert_eq!(body["error"]["nodes"], "rig2-mac");
        assert_eq!(body["error"]["allowed"], serde_json::json!(["arch-box", "rig9"]));
        // The hint names this server's command; its template has the slots
        // a CLI of another brand fills with its own (white-label C5).
        let t = body["error"]["hintTemplate"].as_str().expect("hintTemplate");
        assert!(t.contains("{cli} compute allow-nodes --add rig2-mac") && t.contains("{cli} permission request compute.allow-nodes"), "{t}");
        assert!(t.contains("outside {product}") && !t.contains(&format!("{CLI_NAME} compute")) && !t.contains(&format!("{CLI_NAME} permission")), "{t}");
        assert_eq!(body["error"]["hint"].as_str(), Some(k2_core::brand::fill_names(t).as_str()));
        assert_eq!(r.hint, k2_core::brand::fill_names(t));
    }
}
