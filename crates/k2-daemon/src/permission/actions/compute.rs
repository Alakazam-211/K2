//! E2 — `compute.admin-approve request=`: the node root step (PRD rev 3.1
//! §5.2, §5.3; Appendix A2, A4). Owner, with the `node-admin` confirm
//! Settings takes (`compute_admin.rs` approve). **Calls only**
//! `compute_admin::handle(APPROVE, …)`; no compute file is edited (§12.3).
//!
//! The node admin request keeps its own id (`car_…`, `held_ref`). It lives
//! 24 h (`REQUEST_TTL_SECS`), so its card does too.
//!
//! While compute is dark on this server (`k2_core::compute::enabled()`
//! false), Settings answers nothing for it, so the card neither asks nor
//! applies. Tests set a thread-local override ([`test_seams`]) instead of
//! flipping the process-wide setting.
//!
//! Raising a node's remote admin level is machine-only: `remote_admin_off`
//! carries the plain-text hint (§5.5), never a card.
//!
//! **Integration note.** `origin/hold/compute-share-0463` gives
//! `compute_admin::handle` a fifth argument (`mgr: Option<&Manager>`, used by
//! the `servers` verbs only); APPROVE ignores it. On integ, pass `None`.

use std::collections::HashMap;

use k2_core::compute::admin as cadmin;
use k2_core::permission::catalog::{
    ActionSpec, Area, ConfirmAction, Direction, Expiry, Floor, Grant, MachineHint, NeverReason, NotCardable, NotCardableSpec,
    ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef, Undo, Wall, WallCoverage, WallMatch, WallMethod, Words, MAX_EXPIRY_SECS,
};
use serde_json::json;

use super::held::{text_param, who_of};
use super::{Answerer, Applied, Check, Ctx, Registered};

pub(crate) static ADMIN_APPROVE: ActionSpec = ActionSpec {
    id: "compute.admin-approve",
    area: Area::Held,
    params: &[ParamSpec { key: "request", kind: ParamKind::HeldRequest, required: true, list: false, choices: &[] }],
    floor: Floor::Owner,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::NodeAdmin },
    // The node admin request itself lives 24 h.
    expiry: Expiry { default_secs: cadmin::REQUEST_TTL_SECS, max_secs: MAX_EXPIRY_SECS },
    // 0.46.9 (Rosson 2026-11-05): the card names the exact change (the
    // request's own summary, `{change}`) and the request's reason;
    // `with_computed` fills both from the stored request.
    words: Words {
        setting: "Compute approvals",
        what_changes: "{change}",
        consequences: "Like every approved node change, it runs on {node} as root, signed with the owner's approval key. This is request {request} in Settings → Compute → Approvals: whichever you answer first decides it, and the other then shows it as decided. The agent's reason when it filed the request: {reason}",
        scope: "node request {request}",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "compute", path: "Settings → Compute → Approvals" },
    walls: &[
        Wall {
            matches: WallMatch::Code { status: 403, code: "owner_only" },
            route_prefix: crate::compute_admin::APPROVE,
            method: WallMethod::Post,
            params: &[("request", ParamFrom::Body("/request"))],
        },
        // 0.46.9: the hold itself (`held: true`, a `car_…` id) teaches
        // the ask: a node admin proposal, a policy widening, a share
        // change, a wall on an older node.
        Wall {
            matches: WallMatch::Held { status: 200, field: "held", value: "true" },
            route_prefix: crate::compute_admin::PROPOSE,
            method: WallMethod::Post,
            params: &[("request", ParamFrom::Response("/request/id"))],
        },
        Wall {
            matches: WallMatch::Held { status: 200, field: "held", value: "true" },
            route_prefix: "/cli/compute/nodes/",
            method: WallMethod::Post,
            params: &[("request", ParamFrom::Response("/request/id"))],
        },
    ],
    coverage: WallCoverage::PrefixOnly,
    machine_hints: &[MachineHint {
        code: cadmin::codes::REMOTE_ADMIN_OFF,
        hint: "At {node}, run: sudo k2-node admin level policy   (switch back after: sudo k2-node admin level off)",
    }],
};

/// Compute is on for this server (the preview switch or `K2_COMPUTE`).
fn compute_on() -> bool {
    #[cfg(test)]
    if let Some(on) = test_seams::COMPUTE_ON.with(std::cell::Cell::get) {
        return on;
    }
    k2_core::compute::enabled()
}

#[cfg(test)]
pub(crate) mod test_seams {
    thread_local! {
        /// `Some(on)` overrides `compute::enabled()` on this thread.
        pub(crate) static COMPUTE_ON: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    }
}

/// The asking workspace may still act on compute nodes: "Allow this agent
/// to operate compute nodes" or "Allow managing compute nodes", the two
/// switches that let an agent file a held node request at all (0.46.9,
/// Rosson 2026-11-05: an agent with permission to manage compute brings
/// its approvals to chat). Without either it can't raise or keep one.
fn may_act_on_nodes(workspace_id: &str) -> bool {
    let operate = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        cadmin::agents_can_operate_compute(&conn, workspace_id)
    };
    operate || crate::compute_manage::workspace_allows(workspace_id)
}

/// The refusal (and the stale reason at the click) when neither is on.
pub(crate) const NO_NODE_SWITCH: &str = "this agent can't operate or manage compute nodes now, so it can't bring their approvals to chat. \
     Ask your owner to turn on \"Allow this agent to operate compute nodes\" or \"Allow managing compute nodes\" \
     (Settings → Workspaces → <agent> → Agent); the request still waits in Settings → Compute → Approvals";

/// The card's words from the stored node request (0.46.9): `{change}` is
/// its summary (what the owner reads in Settings), `{node}` its node,
/// `{reason}` the reason filed with it. Fixed at filing, so the card's
/// text hash holds until the click.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action != ADMIN_APPROVE.id {
        return None;
    }
    let mut out = ctx.clone();
    let id = text_param(ctx, "request").unwrap_or("").to_string();
    let row = (!id.is_empty())
        .then(|| {
            let h = k2_core::db::shared();
            let conn = h.lock();
            cadmin::request_by_id(&conn, &id).map(|r| {
                let node = k2_core::compute::store::node_by_id(&conn, &r.node_id).map(|n| n.name).unwrap_or_else(|| "its node".to_string());
                (r, node)
            })
        })
        .flatten()
        .filter(|(r, _)| r.proposer_workspace.as_deref() == Some(ctx.workspace_id.as_str()));
    let (change, node, reason) = match row {
        Some((r, node)) => {
            let mut s = r.summary.trim().to_string();
            if let Some(c) = s.get(..1) {
                s = c.to_uppercase() + &s[1..];
            }
            (s, node, r.reason.filter(|x| !x.trim().is_empty()).unwrap_or_else(|| "none".to_string()))
        }
        None => (format!("Approve node request {id}"), "its node".to_string(), "none".to_string()),
    };
    out.params.insert("change".into(), json!(change));
    out.params.insert("node".into(), json!(node));
    out.params.insert("reason".into(), json!(reason));
    Some(out)
}

/// The card's Deny denies the node request itself, as the person, through
/// the Settings route's own entry (0.46.9: "Denying it denies"). Already
/// decided in Settings (or withdrawn) is left as it is.
pub(crate) fn on_denied(action: &str, request: &str, who: &Answerer, why: Option<&str>) {
    if action != ADMIN_APPROVE.id || request.is_empty() || !compute_on() {
        return;
    }
    let mut body = json!({ "request": request });
    if let Some(w) = why.filter(|w| !w.trim().is_empty()) {
        body["why"] = json!(w);
    }
    if let Err(r) = crate::compute_admin::handle(crate::compute_admin::DENY, &who_of(who), body.to_string().as_bytes(), &HashMap::new(), None) {
        k2_core::log_debug!("[permission] compute.admin-approve deny of {request}: {} {}", r.code, r.hint);
    }
}

/// The current state of the node admin request.
fn check(ctx: &Ctx) -> Check {
    if !compute_on() {
        return Check::Moved("compute nodes are off on this server".into());
    }
    if !may_act_on_nodes(&ctx.workspace_id) {
        return Check::Unavailable { code: "compute_admin_off", why: NO_NODE_SWITCH.into() };
    }
    let Some(id) = text_param(ctx, "request") else {
        return Check::Moved("name the node admin request: request=<its car_… id> (k2 compute node admin requests)".into());
    };
    let row = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        cadmin::request_by_id(&conn, id)
    };
    let Some(r) = row.filter(|r| r.proposer_workspace.as_deref() == Some(ctx.workspace_id.as_str())) else {
        return Check::Moved(format!("no node admin request {id} of {}'s (k2 compute node admin requests)", ctx.workspace_handle));
    };
    match r.state.as_str() {
        "pending" if r.expires_at > k2_core::compute::now() => Check::Needed,
        "pending" | "expired" => Check::Moved(format!("node admin request {id} expired unanswered")),
        "approved" | "sent" | "done" => Check::AlreadyDone,
        "failed" => Check::Moved(format!("node admin request {id} was approved already and its run failed")),
        other => Check::Moved(format!("node admin request {id} was {other} meanwhile")),
    }
}

/// Approve through the Settings route's own entry, as the answerer, with
/// the `node-admin` confirm the person took.
fn apply(ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    if !compute_on() {
        return Applied::Refused { code: "compute_off".into(), message: "compute nodes are off on this server".into() };
    }
    let Some(id) = text_param(ctx, "request") else {
        return Applied::Refused { code: "bad_params".into(), message: "the request names no node admin request".into() };
    };
    let mut body = json!({ "request": id });
    if let Some(t) = token {
        body["confirmToken"] = json!(t);
    }
    match crate::compute_admin::handle(crate::compute_admin::APPROVE, &who_of(who), body.to_string().as_bytes(), &HashMap::new(), None) {
        Ok(v) if v["sent"] == true => Applied::Done { message: "It is on its way to the node now.".into() },
        Ok(_) => Applied::Done { message: "It goes to the node when the node is online (within the hour).".into() },
        Err(r) => Applied::Refused { code: r.code.to_string(), message: r.hint },
    }
}


// ── compute.ci (prd-compute-job-tiers-v1 §5.1, vs-live T13) ─────────

const CI_SETTING: &str = "Allow CI jobs (Settings → Compute → Grants)";
const CI_NODE: &[ParamSpec] = &[ParamSpec { key: "node", kind: ParamKind::Node, required: true, list: false, choices: &[] }];
const CI_SETTINGS: SettingsRef = SettingsRef { page: "compute", path: "Settings → Compute → Grants" };

/// `compute.ci node=`: let the asking workspace send CI jobs to one node
/// (its grant's `allow_ci`). Admin floor: only the owner or an Admin login
/// may set it (never an agent, not even a compute manager). The node
/// owner's share must still allow `gate` for this server; the card can't
/// change that.
pub(crate) static CI: ActionSpec = ActionSpec {
    id: "compute.ci",
    area: Area::Compute,
    params: CI_NODE,
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: CI_SETTING,
        what_changes: "Let {ws} send CI jobs to {node}",
        consequences: "A CI job takes the next free slot on {node}, ahead of every queued job (it never stops a running one). Give it to CI and release workspaces, not to agents doing normal work.",
        scope: "{ws} on {node}",
        duration: "until someone turns it off",
        retry: "run your command again",
    },
    undo: Undo::Twin { action: "compute.ci-off" },
    settings: CI_SETTINGS,
    walls: &[
        Wall {
            matches: WallMatch::Code { status: 403, code: "ci_not_allowed" },
            route_prefix: "/cli/compute/",
            method: WallMethod::Any,
            params: &[("node", ParamFrom::Body("/node"))],
        },
        // `--exclusive` needs the same right (0.46.7, CE-13, D3).
        Wall {
            matches: WallMatch::Code { status: 403, code: "exclusive_needs_grant" },
            route_prefix: "/cli/compute/run",
            method: WallMethod::Post,
            params: &[("node", ParamFrom::Body("/node"))],
        },
    ],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// The twin: stop it again (asked only, no wall).
pub(crate) static CI_OFF: ActionSpec = ActionSpec {
    id: "compute.ci-off",
    area: Area::Compute,
    params: CI_NODE,
    floor: Floor::Admin,
    direction: Direction::Tighten,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: CI_SETTING,
        what_changes: "Stop {ws} sending CI jobs to {node}",
        consequences: "Its CI jobs already queued keep their place; new ones are refused (ci_not_allowed).",
        scope: "{ws} on {node}",
        duration: "until someone turns it on",
        retry: "",
    },
    undo: Undo::None,
    settings: CI_SETTINGS,
    walls: &[],
    coverage: WallCoverage::None,
    machine_hints: &[],
};

/// The asking workspace's grant on `node=`: `Ok((node name, its grant))`.
fn ci_grant(ctx: &Ctx) -> Result<(String, k2_core::compute::store::GrantRow), String> {
    let name = text_param(ctx, "node").ok_or("name the node: node=<name> (k2 compute nodes)")?;
    let h = k2_core::db::shared();
    let conn = h.lock();
    let n = k2_core::compute::store::find_node(&conn, name)
        .filter(|n| n.state == "active")
        .ok_or_else(|| format!("no active compute node '{name}' (k2 compute nodes)"))?;
    let g = k2_core::compute::store::grant(&conn, &n.id, &ctx.workspace_id)
        .ok_or_else(|| format!("{} has no grant on '{}': it needs one first (compute.use)", ctx.workspace_handle, n.name))?;
    Ok((n.name, g))
}

fn check_ci_state(ctx: &Ctx, want_on: bool) -> Check {
    if !compute_on() {
        return Check::Moved("compute nodes are off on this server".into());
    }
    match ci_grant(ctx) {
        Ok((_, g)) if g.allow_ci == want_on => Check::AlreadyDone,
        Ok(_) => Check::Needed,
        Err(m) => Check::Moved(m),
    }
}

fn check_ci(ctx: &Ctx) -> Check {
    check_ci_state(ctx, true)
}

fn check_ci_off(ctx: &Ctx) -> Check {
    check_ci_state(ctx, false)
}

/// Through the Settings route's own entry (`POST /cli/compute/grants/set`,
/// Admin), as the answerer: the grant's current limits plus `allowCi`
/// (that route resets a limit it isn't sent).
fn apply_ci_to(ctx: &Ctx, who: &Answerer, on: bool) -> Applied {
    if !compute_on() {
        return Applied::Refused { code: "compute_off".into(), message: "compute nodes are off on this server".into() };
    }
    let (node, g) = match ci_grant(ctx) {
        Ok(x) => x,
        Err(m) => return Applied::Refused { code: "bad_params".into(), message: m },
    };
    let body = json!({
        "node": node,
        "workspace": ctx.workspace_handle,
        "maxJobSecs": g.max_job_secs,
        "maxDiskGb": g.max_disk_gb,
        "maxParallel": g.max_parallel,
        "maxQueued": g.max_queued,
        "allowCi": on,
    });
    let path = "/cli/compute/grants/set";
    if let Err(r) = super::switches::route_floor(who, path) {
        return super::switches::applied(&r, "");
    }
    let r = crate::compute_routes::handle(path, who_of(who), body.to_string().as_bytes(), &HashMap::new());
    let message = if on {
        format!("{} may send CI jobs to {node} now.", ctx.workspace_handle)
    } else {
        format!("{} sends no more CI jobs to {node}. Turn it back on in Settings → Compute → Grants.", ctx.workspace_handle)
    };
    super::switches::applied(&r, message)
}

fn apply_ci(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    apply_ci_to(ctx, who, true)
}

fn apply_ci_off(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    apply_ci_to(ctx, who, false)
}

// ── compute.allow-nodes (0.46.8, `crate::compute_allow`) ────────────

const ALLOW_SETTING: &str = "Compute node list";
const ALLOW_SETTINGS: SettingsRef = SettingsRef { page: "compute", path: "k2 compute allow-nodes (a terminal outside K2)" };
const ALLOW_PARAMS: &[ParamSpec] = &[ParamSpec { key: "nodes", kind: ParamKind::Node, required: true, list: true, choices: &[] }];

/// `compute.allow-nodes nodes=`: add nodes to the asking workspace's
/// compute node list. Every job from it sends its source to the node it
/// runs on, so a person decides (the owner or an Admin). Narrowing the
/// list is the agent's own (`k2 compute allow-nodes --remove`), so there
/// is no twin.
pub(crate) static ALLOW_NODES: ActionSpec = ActionSpec {
    id: "compute.allow-nodes",
    area: Area::Compute,
    params: ALLOW_PARAMS,
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: ALLOW_SETTING,
        what_changes: "Add {nodes} to {ws}'s compute node list",
        consequences: "{ws}'s jobs may then run on {nodes}, and each job sends its whole source tree there (its --with repos too). Add only machines that may hold that code.",
        scope: "{ws}'s jobs on {nodes}",
        duration: "until someone takes it off the list",
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: ALLOW_SETTINGS,
    walls: &[
        // A job (run, retry, prewarm) aimed at a node off the list.
        Wall {
            matches: WallMatch::Code { status: 403, code: crate::compute_allow::NODE_NOT_ALLOWED },
            route_prefix: "/cli/compute/",
            method: WallMethod::Any,
            params: &[("nodes", ParamFrom::Response("/error/nodes"))],
        },
        // The agent's own list change that adds a node.
        Wall {
            matches: WallMatch::Code { status: 403, code: crate::compute_allow::NEEDS_PERSON },
            route_prefix: crate::compute_allow::SET,
            method: WallMethod::Post,
            params: &[("nodes", ParamFrom::Response("/error/nodes"))],
        },
    ],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// `compute.allow-nodes-clear`: remove the asking workspace's list (every
/// granted node again).
pub(crate) static ALLOW_NODES_CLEAR: ActionSpec = ActionSpec {
    id: "compute.allow-nodes-clear",
    area: Area::Compute,
    params: &[],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: ALLOW_SETTING,
        what_changes: "Remove {ws}'s compute node list",
        consequences: "{ws}'s jobs may then run on any node granted to it again, and each job sends its whole source tree to the node it runs on.",
        scope: "{ws}",
        duration: "until someone sets a list again",
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: ALLOW_SETTINGS,
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: crate::compute_allow::NEEDS_PERSON },
        route_prefix: crate::compute_allow::SET,
        method: WallMethod::Post,
        params: &[],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// The `nodes=` names as stored (a list, or one string).
fn node_names(ctx: &Ctx) -> Vec<String> {
    match ctx.params.get("nodes") {
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(serde_json::Value::String(s)) => s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// Each named node (its id), checked: it exists and this workspace holds
/// a grant on it (a list entry alone never lets a job in).
fn allow_ids(ctx: &Ctx) -> Result<Vec<String>, String> {
    let names = node_names(ctx);
    if names.is_empty() {
        return Err("name the nodes: nodes=<node>[,<node>…] (k2 compute nodes)".into());
    }
    let h = k2_core::db::shared();
    let conn = h.lock();
    let mut ids = Vec::new();
    for name in &names {
        let n = k2_core::compute::store::find_node(&conn, name)
            .filter(|n| n.state != "revoked")
            .ok_or_else(|| format!("no compute node '{name}' (k2 compute nodes)"))?;
        if k2_core::compute::store::grant(&conn, &n.id, &ctx.workspace_id).is_none() {
            return Err(format!("{} has no grant on '{}': it needs one first (compute.use)", ctx.workspace_handle, n.name));
        }
        ids.push(n.id);
    }
    Ok(ids)
}

fn check_allow_nodes(ctx: &Ctx) -> Check {
    if !compute_on() {
        return Check::Moved("compute nodes are off on this server".into());
    }
    let ids = match allow_ids(ctx) {
        Ok(ids) => ids,
        Err(m) => return Check::Moved(m),
    };
    let permit = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::compute::allow::permit(&conn, &ctx.workspace_id)
    };
    // No list: every granted node is allowed already.
    if ids.iter().all(|id| permit.allows(id)) {
        Check::AlreadyDone
    } else {
        Check::Needed
    }
}

fn check_allow_nodes_clear(ctx: &Ctx) -> Check {
    if !compute_on() {
        return Check::Moved("compute nodes are off on this server".into());
    }
    let has = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        !k2_core::compute::allow::permit(&conn, &ctx.workspace_id).is_any()
    };
    if has {
        Check::Needed
    } else {
        Check::AlreadyDone
    }
}

/// Through the route's own entry (`POST /cli/compute/allow-nodes/set`), as
/// the answerer: its floor check (a person: the owner or an Admin) runs
/// again at the click.
fn apply_allow_to(who: &Answerer, body: serde_json::Value, message: String) -> Applied {
    if !compute_on() {
        return Applied::Refused { code: "compute_off".into(), message: "compute nodes are off on this server".into() };
    }
    let path = crate::compute_allow::SET;
    if let Err(r) = super::switches::route_floor(who, path) {
        return super::switches::applied(&r, "");
    }
    let r = crate::compute_routes::handle(path, who_of(who), body.to_string().as_bytes(), &HashMap::new());
    super::switches::applied(&r, message)
}

fn apply_allow_nodes(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let names = match allow_ids(ctx) {
        Ok(_) => node_names(ctx),
        Err(m) => return Applied::Refused { code: "bad_params".into(), message: m },
    };
    let body = json!({ "workspace": ctx.workspace_id, "add": names });
    let message = format!("{}'s jobs may run on {} now.", ctx.workspace_handle, names.join(", "));
    apply_allow_to(who, body, message)
}

fn apply_allow_nodes_clear(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let body = json!({ "workspace": ctx.workspace_id, "clear": true });
    let message = format!("{} has no node list now: its jobs may go to any node granted to it.", ctx.workspace_handle);
    apply_allow_to(who, body, message)
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &ADMIN_APPROVE, check, apply },
        Registered { spec: &CI, check: check_ci, apply: apply_ci },
        Registered { spec: &CI_OFF, check: check_ci_off, apply: apply_ci_off },
        Registered { spec: &ALLOW_NODES, check: check_allow_nodes, apply: apply_allow_nodes },
        Registered { spec: &ALLOW_NODES_CLEAR, check: check_allow_nodes_clear, apply: apply_allow_nodes_clear },
    ]
}

// ── Appendix A4: compute rows that aren't card-able in v1 ───────────

// `compute.grant` and `compute.node.pause` are E1's.
const IN_SETTINGS: &str = "Settings → Compute";

static NOT_CARDABLE: [NotCardableSpec; 9] = [
    NotCardableSpec { id: "compute.agents", area: Area::Compute, why: NotCardable::Covered { by: "compute.use" }, r#where: IN_SETTINGS },
    NotCardableSpec { id: "compute.node.rename", area: Area::Compute, why: NotCardable::Later, r#where: IN_SETTINGS },
    NotCardableSpec { id: "compute.node.routes", area: Area::Compute, why: NotCardable::Later, r#where: IN_SETTINGS },
    NotCardableSpec {
        id: "compute.node.enroll",
        area: Area::Compute,
        why: NotCardable::Never { reason: NeverReason::BoxLevel },
        r#where: "Settings → Compute (the owner, with the node in hand)",
    },
    NotCardableSpec {
        id: "compute.node.remove",
        area: Area::Compute,
        why: NotCardable::Never { reason: NeverReason::Irreversible },
        r#where: "Settings → Compute (the owner)",
    },
    NotCardableSpec {
        id: "compute.node.retire",
        area: Area::Compute,
        why: NotCardable::Never { reason: NeverReason::Irreversible },
        r#where: "Settings → Compute (the owner)",
    },
    NotCardableSpec {
        id: "compute.local",
        area: Area::Compute,
        why: NotCardable::Never { reason: NeverReason::BoxLevel },
        r#where: "Settings → Compute → This computer (the owner)",
    },
    NotCardableSpec {
        id: "compute.node.remote-admin",
        area: Area::Compute,
        why: NotCardable::Never { reason: NeverReason::BoxLevel },
        r#where: "the node itself: sudo k2-node admin level … at that machine",
    },
    NotCardableSpec { id: "compute.admin-deny", area: Area::Compute, why: NotCardable::Dropped, r#where: "Settings → Compute → Approvals" },
];

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    NOT_CARDABLE.iter().collect()
}

#[cfg(test)]
#[path = "compute_tests.rs"]
mod tests;
