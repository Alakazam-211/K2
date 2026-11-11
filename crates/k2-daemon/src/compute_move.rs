//! `k2 compute move` and `--avoid` (0.46.8, Rosson 11-03): a QUEUED job
//! goes to auto-placement or to a named node without a cancel and resubmit.
//!
//! - `--auto` releases a job: it gets the auto filter a no-node submit
//!   would have (`JobCtl.any`). A pinned job keeps the OS and arch of the
//!   node it was named on unless `--os`/`--arch` say otherwise (`any`
//!   drops one); `--label k=v` adds a label; `--avoid N` adds nodes it
//!   must never go to. `--needs`, `--mem`, `--cpus`, the tier, secrets,
//!   `--cache`, `--exclusive` and the grant/share/CI rules all still come
//!   from the job (`compute_place::Want::of_job`), so the sweeper moves it
//!   exactly as it moves any auto job: on its next pass (every 5 s), to a
//!   node that can start it now. A job its new filter rules off its own
//!   node (`--avoid` names it, the node isn't granted, `--os` differs)
//!   moves at once instead, or the release is refused (`no_other_node`).
//!   An auto job's `queued_for_cache` record is kept (CE-6 holds).
//! - `--to NODE` moves it to that node and pins it there: the same checks
//!   as naming the node at submit (grant, node list, CI, `--exclusive`,
//!   the share's limits, features, size).
//! - Only queued jobs, never a started one (`job_started`). A job in a
//!   shared node's own queue may be released (the sweeper asks the node
//!   for it back, V46); `--to` and a release that must leave at once are
//!   refused there (`in_node_queue`).
//! - Who: the job's own workspace (an agent, a K2 shell tab), or the owner
//!   and Admin logins for any job: the same as `retier`.
//!   `--all-mine` / `--workspace-all` pick jobs like `cancel` does.
//!
//! `--avoid NODE` on `run`/`test`: stored in the auto filter (node ids), so
//! submit, the sweeper's moves and retries on another node never pick it.
//! With a named node it is refused (`avoid_with_pinned_node`, exit 2); when
//! the avoid list alone leaves no node, `avoid_leaves_no_node` (exit 3). A
//! name that isn't one of the caller's nodes is a note, not an error
//! (0.46.12, [`avoid_from`]).

use std::collections::BTreeMap;

use k2_core::compute::proto::share as pshare;
use k2_core::compute::share;
use k2_core::compute::{self as compute, codes, store};

use crate::compute_place::{self as place, Tier, Want};
use crate::compute_routes::{b, json_body, job_for, need_credential, no, no_fit_hint, passport_gate, resolve_node_for, s, ws_by_id, ws_by_token_or_worktree, no_workspace, Who, Ws, R};
use crate::compute_views::tag_param;
use crate::compute_ws;

/// `POST {job | filter, auto | to, avoid?, os?, arch?, labels?}`.
pub const MOVE: &str = "/cli/compute/jobs/move";

/// `--avoid` with a named node (or `--same-node-as`): exit 2.
pub const AVOID_WITH_PINNED: &str = "avoid_with_pinned_node";
/// Every node that fits is on the avoid list (exit 3).
pub const AVOID_LEAVES_NONE: &str = "avoid_leaves_no_node";
/// `move --auto` on a job that is already auto with that filter (exit 3).
pub const ALREADY_AUTO: &str = "already_auto";
/// `move --to N` where it already is, pinned (exit 3).
pub const ALREADY_ON_NODE: &str = "already_on_node";
/// `move --to N`: that node can't take this job (exit 3).
pub const NODE_CANT_TAKE: &str = "node_cant_take";
/// The job waits in a shared node's own queue and the ask needs it back at
/// once (exit 3).
pub const IN_NODE_QUEUE: &str = "in_node_queue";
/// The release rules its node out and no other node can take it now
/// (exit 3); nothing changed.
pub const NO_OTHER_NODE: &str = "no_other_node";

/// Most `--avoid` nodes one job may name.
const MAX_AVOID: usize = 32;

// ── --avoid ──────────────────────────────────────────────────────────

/// `avoid: ["k2n11", …]` → node ids (full name or a unique prefix of a node
/// the caller may see, as when naming a node; `node_ambiguous`, exit 2),
/// and one note per name that isn't one of the caller's nodes (0.46.12).
///
/// A node not granted to the workspace can never be picked, so avoiding it
/// is a no-op, not an error: a caller can keep a standing "never those
/// nodes" list as flags. A name that matches no node at all gets the very
/// same note, so the answer never says whether an ungranted node exists.
/// The owner and Admins see every node; for them only a name that matches
/// none is a note.
pub(crate) fn avoid_from(who: &Who, v: &serde_json::Value, ws_id: &str) -> R<(Vec<String>, Vec<String>)> {
    let Some(raw) = v.get("avoid").filter(|x| !x.is_null()) else { return Ok((Vec::new(), Vec::new())) };
    let list = raw.as_array().ok_or_else(|| no(codes::BAD_REQUEST, 400, "avoid is a list of node names (--avoid k2n11, repeatable)"))?;
    if list.len() > MAX_AVOID {
        return Err(no(codes::BAD_REQUEST, 400, format!("--avoid names at most {MAX_AVOID} nodes")));
    }
    let granted: Option<Vec<String>> = (!who.is_admin()).then(|| crate::compute_views::granted_node_ids(ws_id));
    let mut ids: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    for x in list {
        let t = x.as_str().ok_or_else(|| no(codes::BAD_REQUEST, 400, "avoid is a list of node names (--avoid k2n11, repeatable)"))?;
        let mine = match resolve_node_for(who, Some(t), Some(ws_id)) {
            Ok(n) if granted.as_ref().is_none_or(|g| g.contains(&n.id)) => Some(n),
            Ok(_) => None,
            Err(e) if e.code == codes::NODE_NOT_FOUND => None,
            Err(e) => return Err(e),
        };
        match mine {
            Some(n) if !ids.contains(&n.id) => ids.push(n.id),
            Some(_) => {}
            None => {
                let note = avoid_not_yours(t.trim());
                if !notes.contains(&note) {
                    notes.push(note);
                }
            }
        }
    }
    Ok((ids, notes))
}

/// The note for an `--avoid` name that isn't one of the caller's nodes
/// (granted elsewhere, or no node at all: the same words either way).
pub(crate) fn avoid_not_yours(name: &str) -> String {
    format!("--avoid {name}: not one of your nodes; nothing to avoid")
}

/// `--only NODE` (repeatable, 0.46.9): auto-placement picks only among
/// these nodes, so a team keeps private work on its own machines without
/// listing every other node to avoid. Node ids (a rename keeps it).
pub(crate) fn only_from(who: &Who, v: &serde_json::Value, ws_id: &str) -> R<Vec<String>> {
    let Some(raw) = v.get("only").filter(|x| !x.is_null()) else { return Ok(Vec::new()) };
    let list = raw.as_array().ok_or_else(|| no(codes::BAD_REQUEST, 400, "only is a list of node names (--only k2n8, repeatable)"))?;
    if list.len() > MAX_AVOID {
        return Err(no(codes::BAD_REQUEST, 400, format!("--only names at most {MAX_AVOID} nodes")));
    }
    let mut ids: Vec<String> = Vec::new();
    for x in list {
        let t = x.as_str().ok_or_else(|| no(codes::BAD_REQUEST, 400, "only is a list of node names (--only k2n8, repeatable)"))?;
        let n = resolve_node_for(who, Some(t), Some(ws_id)).map_err(|mut e| {
            if e.code == codes::NODE_NOT_FOUND {
                e.hint = format!("--only {t}: {}", e.hint);
            }
            e
        })?;
        if !ids.contains(&n.id) {
            ids.push(n.id);
        }
    }
    Ok(ids)
}

/// `--avoid` with a node named: refused (exit 2).
pub(crate) fn avoid_with_pinned(named: &str) -> crate::compute_routes::Refusal {
    no(
        AVOID_WITH_PINNED,
        400,
        format!(
            "--avoid is for auto-placement, and this job names a node ({named}), which pins it there. Leave the node off so K2 picks one and skips the --avoid nodes, or drop --avoid"
        ),
    )
}

fn names_of(ids: &[String]) -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ids.iter().map(|id| store::node_by_id(&conn, id).map(|n| n.name).unwrap_or_else(|| format!("{id} (removed)"))).collect()
}

/// Nothing fits with `want`'s avoid list: `avoid_leaves_no_node` when the
/// list is why (without it a node would fit), else `Ok` (the caller's own
/// no-fit refusal says why).
pub(crate) fn avoid_leaves_none(ws: &Ws, want: &Want, now: i64) -> R<()> {
    let mut open = want.clone();
    open.filter.avoid.clear();
    let without = place::candidates(&open, &[], now);
    if without.is_empty() {
        return Ok(());
    }
    let mut fit: Vec<String> = without.iter().map(|c| c.node.name.clone()).collect();
    fit.sort();
    Err(no(
        AVOID_LEAVES_NONE,
        409,
        format!(
            "every node granted to workspace {} that can take this job is on your --avoid list ({}); nothing was queued. Avoid fewer nodes, or wait and run it later",
            ws.handle,
            fit.join(", ")
        ),
    )
    .with("avoid", names_of(&want.filter.avoid)))
}

// ── the submit hint ──────────────────────────────────────────────────

/// A job pinned to a node that can't start it now, while another granted
/// node that fits could: one advisory line (never a refusal).
pub(crate) fn pinned_hint(job: &store::JobRow, want: &Want) -> Option<String> {
    if compute::result::shown_state(job) != "queued" {
        return None;
    }
    let now = compute::now();
    let mut w = want.clone();
    w.auto = true;
    w.filter.avoid.clear();
    let report = place::candidates_report(&w, std::slice::from_ref(&job.node_id), now);
    let free = report.cands.iter().filter(|c| c.tier(&w) == Some(Tier::Now)).min_by(|a, b| a.band(&w).cmp(&b.band(&w)).then_with(|| a.node.name.cmp(&b.node.name)))?;
    let (here, queued) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (store::node_by_id(&conn, &job.node_id).map(|n| n.name).unwrap_or_default(), store::jobs_in_states(&conn, &job.node_id, &["queued"]).len())
    };
    let short = compute::message::short(&job.id);
    Some(format!(
        "hint: {here} can't start this job now ({queued} queued there) but {} could. Leave the node off next time (K2 picks one), or release this one: k2 compute move {short} --auto",
        free.node.name
    ))
}

/// No `--cpus`: what the job reserves on the node (its CPUs ÷ its job
/// slots, as k2-node does), said once at submit. `None` on a node that
/// runs one job at a time (the whole node is the slot; `--cpus` lets
/// nothing else in) or with no offer to read.
pub(crate) fn cpus_hint(node: &str, offer: Option<&k2_core::compute::proto::frames::Offer>) -> Option<String> {
    let o = offer.filter(|o| o.caps.cpu_millis > 0 && o.parallel_max > 1)?;
    let all = o.caps.cpu_millis.div_ceil(1000);
    let each = (o.caps.cpu_millis / u64::from(o.parallel_max)).div_ceil(1000).max(1);
    Some(format!(
        "no --cpus: this job reserves {each} of the {all} CPUs on {node} ({all} CPUs ÷ {} job slots) while it runs, used or not; a job waits for unreserved CPUs even when they sit idle. Pass --cpus N with what it really uses so more jobs fit there",
        o.parallel_max
    ))
}

// ── the route ────────────────────────────────────────────────────────

/// What the caller asked for.
struct Ask {
    /// `--to NODE`; `None` = `--auto`.
    to: Option<String>,
    avoid: Vec<String>,
    /// `--avoid` names that aren't the caller's nodes (one note each).
    avoid_notes: Vec<String>,
    /// `Some(None)`: `--os any` (drop it).
    os: Option<Option<String>>,
    arch: Option<Option<String>>,
    labels: BTreeMap<String, String>,
}

fn any_or(v: &serde_json::Value, k: &str) -> Option<Option<String>> {
    s(v, k).map(|x| (!x.eq_ignore_ascii_case("any")).then(|| x.to_string()))
}

/// `POST /cli/compute/jobs/move`.
pub(crate) fn handle(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_credential(who)?;
    let v = json_body(body)?;
    if who.passport().is_none() && !who.is_admin() {
        return Err(no(codes::OWNER_ONLY, 403, "only the job's own workspace, the owner or an Admin moves a job"));
    }
    let to = s(&v, "to").map(str::to_string);
    let auto = b(&v, "auto") == Some(true);
    if to.is_some() == auto {
        return Err(no(codes::BAD_REQUEST, 400, "say where: k2 compute move <job> --auto (K2 picks a node) or --to <node> (pin it there)"));
    }
    let labels: BTreeMap<String, String> = v
        .get("labels")
        .and_then(|l| l.as_object())
        .map(|m| m.iter().filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string()))).collect())
        .unwrap_or_default();
    let has_avoid = v.get("avoid").and_then(|a| a.as_array()).is_some_and(|a| !a.is_empty());
    if to.is_some() && (has_avoid || s(&v, "os").is_some() || s(&v, "arch").is_some() || !labels.is_empty()) {
        return Err(no(codes::BAD_REQUEST, 400, "--avoid, --os, --arch and --label go with --auto; --to pins the job to that one node"));
    }
    let mut ask = Ask { to, avoid: Vec::new(), avoid_notes: Vec::new(), os: any_or(&v, "os"), arch: any_or(&v, "arch"), labels };
    if let Some(f) = v.get("filter") {
        return move_matching(who, &v, f, ask);
    }
    let j = job_for(who, s(&v, "job"))?;
    (ask.avoid, ask.avoid_notes) = avoid_from(who, &v, &j.workspace_id)?;
    let mut out = move_one(who, &j, &ask)?;
    out["warnings"] = ask.avoid_notes.clone().into();
    Ok(out)
}

/// `--all-mine` / `--workspace-all` (as `cancel`): every matching QUEUED
/// job, each through [`move_one`]; refusals are listed, not fatal.
fn move_matching(who: &Who, v: &serde_json::Value, f: &serde_json::Value, mut ask: Ask) -> R<serde_json::Value> {
    let gate = passport_gate(who)?;
    let fs = |k: &str| f.get(k).and_then(|x| x.as_str()).map(str::trim).filter(|x| !x.is_empty());
    let scope = fs("scope").unwrap_or("session");
    let (ws, session) = match (scope, gate) {
        ("session", Some(ws)) => (ws, who.passport().map(|(_, s)| s.to_string())),
        ("workspace", Some(ws)) => (ws, None),
        ("session", None) => {
            return Err(no(
                codes::BAD_REQUEST,
                400,
                "--all-mine means this agent session's jobs, and this caller has no session (the owner token): pass --workspace-all from the workspace's folder",
            ))
        }
        ("workspace", None) => {
            let t = fs("workspace").ok_or_else(|| no("workspace_not_found", 404, "pass --workspace <workspace>"))?;
            (ws_by_token_or_worktree(t).ok_or_else(|| no_workspace(t))?.id, None)
        }
        _ => return Err(no(codes::BAD_REQUEST, 400, "filter.scope is \"session\" (--all-mine) or \"workspace\" (--workspace-all)")),
    };
    let tag = tag_param(fs("tag"))?;
    let node = match fs("node") {
        Some(t) => Some(resolve_node_for(who, Some(t), Some(&ws))?.id),
        None => None,
    };
    (ask.avoid, ask.avoid_notes) = avoid_from(who, v, &ws)?;
    let rows: Vec<store::JobRow> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut rows = store::list_jobs(
            &conn,
            &store::JobFilter { node_id: node.as_deref(), workspace_id: Some(&ws), state: Some("queued"), session_id: session.as_deref(), limit: 500, tier: None, tag },
        );
        rows.retain(|r| r.ctl.shards.is_empty());
        rows.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
        rows
    };
    let mut moved = Vec::new();
    let mut skipped = Vec::new();
    for r in &rows {
        let short = compute::message::short(&r.id).to_string();
        match move_one(who, r, &ask) {
            Ok(out) => moved.push(out),
            Err(e) => skipped.push(serde_json::json!({"job": short, "code": e.code, "hint": e.hint})),
        }
    }
    Ok(serde_json::json!({
        "ok": true,
        "matched": rows.len(),
        "count": moved.len(),
        "moved": moved,
        "skipped": skipped,
        "scope": scope,
        "warnings": ask.avoid_notes,
    }))
}

fn node_name(id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::node_by_id(&conn, id).map(|n| n.name).unwrap_or_else(|| id.to_string())
}

/// One queued job: checks shared by `--auto` and `--to`, then either.
fn move_one(who: &Who, j: &store::JobRow, ask: &Ask) -> R<serde_json::Value> {
    let short = compute::message::short(&j.id).to_string();
    if !j.ctl.shards.is_empty() {
        return Err(no(codes::BAD_REQUEST, 400, format!("{short} is a split run: move its shards (k2 compute get {short} lists them)")));
    }
    let on_node = j.node_queued && j.state == "assigned";
    if j.state != "queued" && !on_node {
        return Err(no(
            pshare::codes::JOB_STARTED,
            409,
            format!("job {short} is {}: only a queued job moves (a started job is never stopped or moved)", j.state),
        ));
    }
    let ws = ws_by_id(&j.workspace_id).ok_or_else(|| no("workspace_not_found", 404, "the job's workspace is gone"))?;
    match ask.to.as_deref() {
        Some(to) => move_to(who, j, &ws, to, on_node),
        None => release(who, j, &ws, ask, on_node),
    }
}

/// The OS and arch of the node a pinned job waits on (its offer, else the
/// labels it enrolled with).
fn node_kind(node_id: &str) -> (Option<String>, Option<String>) {
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, node_id)
    };
    let Some(n) = row else { return (None, None) };
    let offer = compute_ws::live_view(&n.id).2.or_else(|| n.last_offer.clone());
    let get = |k: &str| offer.as_ref().and_then(|o| o.labels.get(k).cloned()).or_else(|| n.labels.get(k).cloned());
    (get("os"), get("arch"))
}

fn filter_json(f: &store::AnyFilter) -> serde_json::Value {
    serde_json::json!({"os": f.os, "arch": f.arch, "labels": f.labels, "avoid": names_of(&f.avoid)})
}

/// `--auto`: write the auto filter; the sweeper moves it (or, when the
/// filter rules out its own node, move it now).
fn release(who: &Who, j: &store::JobRow, ws: &Ws, ask: &Ask, on_node: bool) -> R<serde_json::Value> {
    let short = compute::message::short(&j.id).to_string();
    let plan = j.plan().ok_or_else(|| no("internal", 500, "the job's stored plan can't be read"))?;
    let was_auto = j.ctl.any.is_some();
    let mut f = match j.ctl.any.clone() {
        Some(f) => f,
        None => {
            let (os, arch) = node_kind(&j.node_id);
            store::AnyFilter { os, arch, ..Default::default() }
        }
    };
    if let Some(os) = &ask.os {
        f.os = os.clone();
    }
    if let Some(arch) = &ask.arch {
        f.arch = arch.clone();
    }
    f.labels.extend(ask.labels.clone());
    for id in &ask.avoid {
        if !f.avoid.contains(id) {
            f.avoid.push(id.clone());
        }
    }
    if was_auto && j.ctl.any.as_ref() == Some(&f) {
        return Err(no(
            ALREADY_AUTO,
            409,
            format!("job {short} is already auto-placed: it moves when another node can start it first. --avoid <node> keeps it off a node"),
        ));
    }
    let mut ctl = j.ctl.clone();
    ctl.any = Some(f.clone());
    let mut moved_job = j.clone();
    moved_job.ctl = ctl.clone();
    let w = Want::of_job(&moved_job, &plan);
    let now = compute::now();
    let report = place::candidates_report(&w, &[], now);
    if report.cands.is_empty() {
        if !f.avoid.is_empty() {
            avoid_leaves_none(ws, &w, now)?;
        }
        return Err(no(codes::NO_NODE_FITS, 404, format!("{}; job {short} stays where it is, pinned", no_fit_hint(ws, &w, &report))));
    }
    let here = node_name(&j.node_id);
    // Its own node is no longer a candidate (avoided, not granted, off the
    // list, another --os): it must leave now, or nothing changes.
    if !report.cands.iter().any(|c| c.node.id == j.node_id) {
        if on_node {
            return Err(no(
                IN_NODE_QUEUE,
                409,
                format!(
                    "job {short} waits in {here}'s own queue, and this ask rules {here} out; K2 can't take it back from there at once. Cancel it (k2 compute cancel {short}) and run it again without a node, or release it without ruling {here} out"
                ),
            ));
        }
        return match place::try_move(&moved_job, &[], true, "released") {
            Some(to) => {
                audit_release(who, j, &f, was_auto, Some(&to));
                Ok(serde_json::json!({
                    "ok": true, "job": short, "mode": "auto", "from": here, "node": node_name(&to), "moved": true, "filter": filter_json(&f),
                    "note": format!("{here} doesn't fit the new ask, so it moved now"),
                }))
            }
            None => Err(no(
                NO_OTHER_NODE,
                409,
                format!(
                    "job {short} would have to leave {here} at once, and no other node that fits can take it now (offline or unavailable); nothing changed. Try again later, or ask for less"
                ),
            )),
        };
    }
    if !was_auto {
        // The record says it was released here (a fresh auto record; a
        // pinned job never held a cache wait, so CE-6 doesn't apply).
        let skipped: BTreeMap<String, u32> = report.skipped.iter().map(|(k, n)| (k.to_string(), *n)).collect();
        ctl.placement = Some(store::Placement {
            mode: "auto".into(),
            policy: compute::placement_policy().as_wire().into(),
            node: here.clone(),
            code: "released_auto".into(),
            why: format!("released to auto-placement by {} (k2 compute move --auto) · it moves when another node can start it first", who.label()),
            fits: place::fits_words(report.cands.len() as u32, &skipped, &w),
            starts_now: None,
            eta_secs: None,
            considered: report.cands.len() as u32,
            skipped,
            at: now,
        });
    }
    let written = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_waiting_job_ctl(&conn, &j.id, j.generation, &ctl).map_err(|e| no("internal", 500, e))?
    };
    if !written {
        return Err(no(pshare::codes::JOB_STARTED, 409, format!("job {short} started (or ended) just now; it stays where it is")));
    }
    audit_release(who, j, &f, was_auto, None);
    compute_ws::notify_change();
    Ok(serde_json::json!({
        "ok": true, "job": short, "mode": "auto", "from": here, "node": here, "moved": false, "filter": filter_json(&f),
        "note": if on_node {
            format!("it waits in {here}'s own queue; K2 asks for it back when another node can start it now")
        } else {
            format!("it waits on {here} and moves on the next pass (every 5 s) to a node that can start it now")
        },
    }))
}

fn audit_release(who: &Who, j: &store::JobRow, f: &store::AnyFilter, was_auto: bool, moved_to: Option<&str>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::record_event(
        &conn,
        &who.label(),
        "job_release",
        Some(&j.node_id),
        Some(&j.workspace_id),
        Some(&j.id),
        serde_json::json!({"was": if was_auto { "auto" } else { "pinned" }, "filter": f, "movedTo": moved_to, "by": who.label()}),
    );
}

/// `--to NODE`: rebuild the plan for that node and pin it there.
fn move_to(who: &Who, j: &store::JobRow, ws: &Ws, to: &str, on_node: bool) -> R<serde_json::Value> {
    let short = compute::message::short(&j.id).to_string();
    let n = resolve_node_for(who, Some(to), Some(&ws.id))?;
    if n.state != "active" {
        return Err(no(codes::NODE_NOT_FOUND, 404, format!("'{}' is waiting for its code to be confirmed", n.name)));
    }
    crate::compute_allow::check_node(ws, &n, "--to")?;
    let here = node_name(&j.node_id);
    if n.id == j.node_id && j.ctl.any.is_none() {
        return Err(no(ALREADY_ON_NODE, 409, format!("job {short} is already pinned to {}", n.name)));
    }
    if on_node {
        return Err(no(
            IN_NODE_QUEUE,
            409,
            format!(
                "job {short} waits in {here}'s own queue; K2 can't move it to another node at once. Release it instead (k2 compute move {short} --auto: K2 asks for it back when another node can start it), or cancel it and run it again on {}",
                n.name
            ),
        ));
    }
    let plan = j.plan().ok_or_else(|| no("internal", 500, "the job's stored plan can't be read"))?;
    let mut w = Want::of_job(j, &plan);
    w.auto = false;
    w.admin = who.is_admin();
    w.filter = store::AnyFilter::default();
    let now = compute::now();
    // Only `n` is considered, so the skipped count names its one reason.
    let others: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_nodes(&conn, false).into_iter().map(|x| x.id).filter(|id| *id != n.id).collect()
    };
    let report = place::candidates_report(&w, &others, now);
    let Some(c) = report.cands.into_iter().next() else {
        let granted = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::grant(&conn, &n.id, &ws.id).is_some()
        };
        if !granted && !who.is_admin() {
            return Err(no(
                codes::NOT_GRANTED,
                403,
                format!("'{}' isn't granted to workspace {}: ask your owner (k2 compute grant {} --workspace {})", n.name, ws.handle, n.name, ws.handle),
            ));
        }
        let why: Vec<String> = report.skipped.iter().map(|(k, c)| place::skip_words(k, *c, &w)).collect();
        return Err(no(
            NODE_CANT_TAKE,
            409,
            format!("'{}' can't take job {short}: {}; it stays on {here} (k2 compute nodes says more)", n.name, if why.is_empty() { "it isn't available".to_string() } else { why.join(", ") }),
        ));
    };
    let mut ctl = j.ctl.clone();
    ctl.any = None;
    let mut rec = place::pinned_record(&c.node.name, now);
    rec.code = "moved_to".into();
    rec.why = format!("moved here by {} (k2 compute move --to)", who.label());
    ctl.placement = Some(rec);
    let same = c.node.id == j.node_id;
    let ok = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let ok = if same {
            store::set_waiting_job_ctl(&conn, &j.id, j.generation, &ctl).map_err(|e| no("internal", 500, e))?
        } else {
            let mut new_plan = place::plan_for_node(&plan, &j.ctl, &c.node, c.grant.as_ref(), &c.offer);
            if share::queue_on(&c.offer) {
                place::set_queue_fields(&mut new_plan, j);
            }
            store::move_queued_job(&conn, &j.id, &new_plan, &ctl, "moved_to").map_err(|e| no("internal", 500, e))?
        };
        if ok {
            store::record_event(
                &conn,
                &who.label(),
                "job_move",
                Some(&c.node.id),
                Some(&j.workspace_id),
                Some(&j.id),
                serde_json::json!({"from": j.node_id, "to": c.node.id, "why": "asked", "by": who.label(), "pinned": true}),
            );
        }
        ok
    };
    if !ok {
        return Err(no(pshare::codes::JOB_STARTED, 409, format!("job {short} started (or ended) just now; it stays on {here}")));
    }
    if !same {
        place::note_placed(&c.node.id, now);
    }
    compute_ws::kick(&c.node.id);
    compute_ws::notify_change();
    Ok(serde_json::json!({
        "ok": true, "job": short, "mode": "pinned", "from": here, "node": c.node.name, "moved": !same,
        "note": if same { format!("pinned where it waits ({}): it no longer moves", c.node.name) } else { format!("pinned to {}: it waits there and never moves on its own", c.node.name) },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_or_reads_any_as_drop() {
        let v = serde_json::json!({"os": "any", "arch": "arm64"});
        assert_eq!(any_or(&v, "os"), Some(None));
        assert_eq!(any_or(&v, "arch"), Some(Some("arm64".to_string())));
        assert_eq!(any_or(&v, "labels"), None);
    }

    #[test]
    fn avoid_with_a_named_node_is_a_usage_refusal() {
        let r = avoid_with_pinned("k2n12-vin");
        assert_eq!((r.code, r.status), (AVOID_WITH_PINNED, 400));
        assert!(r.hint.contains("k2n12-vin") && r.hint.contains("Leave the node off"), "{}", r.hint);
    }
}
