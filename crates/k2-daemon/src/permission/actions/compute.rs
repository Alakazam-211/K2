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
    if action == PACKAGES.id {
        return Some(packages_computed(ctx));
    }
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
    if action == PACKAGES.id {
        on_packages_denied(request, who, why);
        return;
    }
    if action != ADMIN_APPROVE.id || request.is_empty() || is_package_request(request) || !compute_on() {
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
    // A package request (`cpr_…`) has its own card; this one never decides it.
    if is_package_request(id) {
        return Check::Moved(format!("{id} is a package request: ask with k2 permission request {} request={id}", PACKAGES.id));
    }
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

// ── compute.packages (0.46.9, prd-compute-self-serve-environments-v1 §6, §14) ──

/// A package request batch or row id (`cpr_…`, `k2_core::compute::pkgreq`).
pub(crate) fn is_package_request(id: &str) -> bool {
    id.trim().starts_with("cpr_")
}

/// The answer field a filed package request carries (`pkgHeld: true`). Not
/// `held`: that one is a node admin request's (`compute.admin-approve`).
pub(crate) const PKG_HELD: &str = "pkgHeld";

/// `compute.packages request=<cpr_…>`: install the packages one batched
/// request names, on its nodes. The owner or an Admin, a person (the
/// route's own `decider`). One card per batch; it joins only other
/// `compute.packages` cards ([`batches_alone`]), never an unrelated
/// "Allow all". The request lives 24 h, so its card does too.
pub(crate) static PACKAGES: ActionSpec = ActionSpec {
    id: "compute.packages",
    area: Area::Compute,
    params: &[ParamSpec { key: "request", kind: ParamKind::HeldRequest, required: true, list: false, choices: &[] }],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry { default_secs: k2_core::compute::proto::pkgpolicy::REQUEST_TTL_SECS, max_secs: MAX_EXPIRY_SECS },
    // K2 writes the card from the stored request, never the agent:
    // `with_computed` fills `{change}` (what installs, where, for whom, the
    // dependency count and size), `{nodes}`, `{shared}` and `{reason}`.
    words: Words {
        setting: "Compute packages",
        what_changes: "{change}",
        consequences: "Root installs them on {nodes} from the distro's signed official repos, after a dry run that refuses anything that removes a package, comes from another repo or is too big. {shared}This is request {request} in Settings → Compute → Approvals: whichever you answer first decides it. The agent's reason: {reason}",
        scope: "package request {request}",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "compute", path: "Settings → Compute → Approvals" },
    walls: &[
        // The request verb: filed and held for this server's owner.
        Wall {
            matches: WallMatch::Held { status: 200, field: PKG_HELD, value: "true" },
            route_prefix: crate::compute_pkg::REQUEST,
            method: WallMethod::Post,
            params: &[("request", ParamFrom::Response("/request/id"))],
        },
        // A prewarm that filed one batched request for its nodes.
        Wall {
            matches: WallMatch::Held { status: 200, field: PKG_HELD, value: "true" },
            route_prefix: "/cli/compute/prewarm",
            method: WallMethod::Post,
            params: &[("request", ParamFrom::Response("/request/id"))],
        },
        // A job (run, test, prewarm) refused because a package isn't
        // approved: the refusal filed the request and names it.
        Wall {
            matches: WallMatch::Code { status: 403, code: k2_core::compute::proto::pkgpolicy::codes::PACKAGE_NOT_APPROVED },
            route_prefix: "/cli/compute/",
            method: WallMethod::Any,
            params: &[("request", ParamFrom::Response("/error/request"))],
        },
    ],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// A `compute.packages` card batches only with other `compute.packages`
/// cards (PRD §3.3, §16.9): the person reads every name and the size.
pub(crate) fn batches_alone(action: &str) -> bool {
    action == PACKAGES.id
}

/// The batch's rows that belong to the asking workspace.
fn package_rows(ctx: &Ctx, id: &str) -> Vec<k2_core::compute::pkgreq::PkgRow> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::compute::pkgreq::batch(&conn, id).into_iter().filter(|r| r.workspace_id.as_deref() == Some(ctx.workspace_id.as_str())).collect()
}

/// The card's words from the stored rows: `{change}` is the request's own
/// summary (Settings shows the same line), `{nodes}` its machines,
/// `{shared}` the host-wide line when another server asked, `{reason}`
/// the agent's `--why`.
fn packages_computed(ctx: &Ctx) -> Ctx {
    let mut out = ctx.clone();
    let id = text_param(ctx, "request").unwrap_or("").to_string();
    let rows = if is_package_request(&id) { package_rows(ctx, &id) } else { Vec::new() };
    let (change, nodes) = if rows.is_empty() {
        (format!("Install the packages in request {id}"), "its node".to_string())
    } else {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let name = |nid: &str| k2_core::compute::store::node_by_id(&conn, nid).map(|n| n.name).unwrap_or_else(|| nid.to_string());
        let mut nodes: Vec<String> = rows.iter().map(|r| name(r.node_id.as_str())).collect();
        nodes.dedup();
        (k2_core::compute::pkgreq::summary(&rows, &name), nodes.join(", "))
    };
    let other_server = rows.iter().any(|r| r.server != "own" || r.origin == k2_core::compute::pkgreq::FWD);
    let shared = if other_server { "This is installed for every job on that machine. " } else { "" };
    let reason = rows
        .iter()
        .filter_map(|r| r.why.as_deref())
        .map(str::trim)
        .find(|w| !w.is_empty())
        .unwrap_or("none")
        .to_string();
    out.params.insert("change".into(), json!(change));
    out.params.insert("nodes".into(), json!(nodes));
    out.params.insert("shared".into(), json!(shared));
    out.params.insert("reason".into(), json!(reason));
    out
}

/// The batch's state: still waiting here (Needed), all installed or on
/// their way (AlreadyDone), or decided, expired or sent elsewhere (Moved).
fn check_packages(ctx: &Ctx) -> Check {
    use k2_core::compute::proto::pkgpolicy::state;
    if !compute_on() {
        return Check::Moved("compute nodes are off on this server".into());
    }
    let Some(id) = text_param(ctx, "request") else {
        return Check::Moved("name the package request: request=<its cpr_… id> (k2 compute node packages requests)".into());
    };
    if !is_package_request(id) {
        return Check::Moved(format!("{id} is not a package request (cpr_…): see k2 compute node packages requests"));
    }
    let rows = package_rows(ctx, id);
    if rows.is_empty() {
        return Check::Moved(format!("no package request {id} of {}'s (k2 compute node packages requests)", ctx.workspace_handle));
    }
    let now = k2_core::compute::now();
    let live = |r: &&k2_core::compute::pkgreq::PkgRow| r.state == state::PENDING && r.expires_at > now;
    if rows.iter().filter(live).any(|r| r.origin != k2_core::compute::pkgreq::SENT) {
        return Check::Needed;
    }
    if let Some(r) = rows.iter().find(live) {
        return Check::Moved(format!(
            "package request {id} went to the node's owner ({}): they decide it on their own server, and you hear back here",
            r.server
        ));
    }
    if rows.iter().all(|r| r.state == state::INSTALLED || r.state == "approved") {
        return Check::AlreadyDone;
    }
    let h = k2_core::db::shared();
    let conn = h.lock();
    let states: Vec<String> = rows
        .iter()
        .map(|r| {
            let node = k2_core::compute::store::node_by_id(&conn, &r.node_id).map(|n| n.name).unwrap_or_else(|| r.node_id.clone());
            let st = if r.state == state::PENDING { state::EXPIRED } else { r.state.as_str() };
            match r.note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                Some(n) => format!("{st} on {node} ({n})"),
                None => format!("{st} on {node}"),
            }
        })
        .collect();
    Check::Moved(format!("package request {id} was decided meanwhile: {}", states.join(", ")))
}

/// Allow: the route's own entry, as the answerer (its `decider` checks the
/// person again).
fn apply_packages(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    if !compute_on() {
        return Applied::Refused { code: "compute_off".into(), message: "compute nodes are off on this server".into() };
    }
    let Some(id) = text_param(ctx, "request").filter(|id| is_package_request(id)) else {
        return Applied::Refused { code: "bad_params".into(), message: "the request names no package request (cpr_…)".into() };
    };
    match crate::compute_pkg::decide_batch(&who_of(who), id, true, false, None) {
        Ok(v) => {
            let rows = v["rows"].as_array().cloned().unwrap_or_default();
            let on = |st: &str| -> Vec<String> {
                rows.iter().filter(|r| r["state"] == st).filter_map(|r| r["node"].as_str().map(str::to_string)).collect()
            };
            let (sent, offline) = (on("approved"), on(k2_core::compute::proto::pkgpolicy::state::PENDING));
            let mut msg = Vec::new();
            if !sent.is_empty() {
                msg.push(format!("Root installs them on {} now, after its dry run and guards; you hear back when it's done. Then run your job again.", sent.join(", ")));
            }
            if !offline.is_empty() {
                msg.push(format!(
                    "{} is offline, so it waits there: a person approves it in Settings → Compute → Approvals when it's back.",
                    offline.join(", ")
                ));
            }
            Applied::Done { message: msg.join(" ") }
        }
        Err(r) => Applied::Refused { code: r.code.to_string(), message: r.hint },
    }
}

/// The card's Deny denies the request itself, as the person (the same
/// entry as Settings). Already decided elsewhere is left as it is.
fn on_packages_denied(request: &str, who: &Answerer, why: Option<&str>) {
    if !is_package_request(request) || !compute_on() {
        return;
    }
    let why = why.map(str::trim).filter(|w| !w.is_empty());
    if let Err(r) = crate::compute_pkg::decide_batch(&who_of(who), request.trim(), false, false, why) {
        k2_core::log_debug!("[permission] compute.packages deny of {request}: {} {}", r.code, r.hint);
    }
}

// ── compute.packages-policy (0.46.9, §3.4, §13) ─────────────────────

const POLICY_LEVELS: &[&str] = &["list", "profiles", "official"];

/// `compute.packages-policy node= server= level=`: set what a node installs
/// for a server without asking (its package policy). The owner. Narrowing
/// applies at once; widening goes on as a held node admin request the
/// owner approves in Settings → Compute → Approvals (root on the node).
pub(crate) static PACKAGES_POLICY: ActionSpec = ActionSpec {
    id: "compute.packages-policy",
    area: Area::Compute,
    params: &[
        ParamSpec { key: "node", kind: ParamKind::Node, required: true, list: false, choices: &[] },
        ParamSpec { key: "server", kind: ParamKind::Text, required: true, list: false, choices: &[] },
        ParamSpec { key: "level", kind: ParamKind::Choice, required: true, list: false, choices: POLICY_LEVELS },
    ],
    floor: Floor::Owner,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Compute package policy",
        what_changes: "Set {node}'s package policy for {server} to {level}",
        consequences: "list installs only names on {node}'s approved list; profiles adds K2's bundles you turned on; official allows any package from the distro's signed official repos, minus K2's deny list. A package installs for every job on that machine. Narrowing applies at once. Widening runs as root on {node}, so it waits as a node request for you in Settings → Compute → Approvals.",
        scope: "{server} on {node}",
        duration: "until someone changes it",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "compute", path: "Settings → Compute" },
    walls: &[Wall {
        // The policy route, to an agent that may not operate compute nodes.
        matches: WallMatch::Code { status: 403, code: k2_core::compute::admin::codes::OPERATE_OFF },
        route_prefix: crate::compute_pkg::POLICY,
        method: WallMethod::Post,
        params: &[("node", ParamFrom::Body("/node")), ("server", ParamFrom::Body("/server")), ("level", ParamFrom::Body("/level"))],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// The named node: active, this server's own. `Ok((node, its offer))`.
fn policy_node(ctx: &Ctx) -> Result<(k2_core::compute::store::NodeRow, Option<k2_core::compute::proto::frames::Offer>), String> {
    let name = text_param(ctx, "node").ok_or("name the node: node=<name> (k2 compute nodes)")?;
    let n = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::compute::store::find_node(&conn, name)
    }
    .filter(|n| n.state == "active")
    .ok_or_else(|| format!("no active compute node '{name}' (k2 compute nodes)"))?;
    if n.is_tenant() {
        return Err(format!("{} belongs to another server ({}): its owner sets its package policy", n.name, n.owner_label.as_deref().unwrap_or("its owner")));
    }
    let offer = crate::compute_ws::live_view(&n.id).2.or_else(|| n.last_offer.clone());
    Ok((n, offer))
}

fn check_packages_policy(ctx: &Ctx) -> Check {
    use k2_core::compute::proto::pkgpolicy as pp;
    if !compute_on() {
        return Check::Moved("compute nodes are off on this server".into());
    }
    let (n, offer) = match policy_node(ctx) {
        Ok(x) => x,
        Err(m) => return Check::Moved(m),
    };
    if !offer.as_ref().is_some_and(|o| o.features.iter().any(|f| f == pp::FEATURE_PKG_STATUS)) {
        return Check::Moved(format!("{}'s k2-node has no package policy yet (older than 0.46.9): update it (k2 compute node update {})", n.name, n.name));
    }
    let Some(level) = text_param(ctx, "level").and_then(pp::Level::parse) else {
        return Check::Moved(format!("level= is one of {}", POLICY_LEVELS.join(", ")));
    };
    // The node reports this server's own level; another server's only it knows.
    let own = text_param(ctx, "server") == Some("own");
    if own && offer.as_ref().and_then(|o| o.pkg.as_ref()).is_some_and(|p| p.level == level) {
        return Check::AlreadyDone;
    }
    Check::Needed
}

/// Through the route's own entry (`POST /cli/compute/nodes/packages/policy`),
/// as the answerer.
fn apply_packages_policy(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    if !compute_on() {
        return Applied::Refused { code: "compute_off".into(), message: "compute nodes are off on this server".into() };
    }
    let (n, _) = match policy_node(ctx) {
        Ok(x) => x,
        Err(m) => return Applied::Refused { code: "bad_params".into(), message: m },
    };
    let (server, level) = (text_param(ctx, "server").unwrap_or("own"), text_param(ctx, "level").unwrap_or(""));
    let path = crate::compute_pkg::POLICY;
    if let Err(r) = super::switches::route_floor(who, path) {
        return super::switches::applied(&r, "");
    }
    let body = json!({ "node": n.name, "server": server, "level": level, "reason": format!("package policy for {server}: {level} (a permission card)") });
    let r = crate::compute_routes::handle(path, who_of(who), body.to_string().as_bytes(), &HashMap::new());
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or(serde_json::Value::Null);
    let message = if v["held"] == true {
        let id = v.pointer("/request/id").and_then(serde_json::Value::as_str).unwrap_or("");
        format!(
            "That widens what {} installs as root, so it waits as node request {id}: approve it in Settings → Compute → Approvals.",
            n.name
        )
    } else {
        format!("{}'s package policy for {server} is {level} now.", n.name)
    };
    super::switches::applied(&r, message)
}

// ── compute.git-access (0.46.9, GH#78, `crate::compute_git`) ────────

const GIT_PARAMS: &[ParamSpec] = &[ParamSpec { key: "repos", kind: ParamKind::Text, required: true, list: true, choices: &[] }];

/// `compute.git-access repos=`: give the asking workspace's compute jobs
/// read access to GitHub repos on every node this server owns (short-lived,
/// read-only tokens through git's credential helper). A person decides
/// (the owner or an Admin); revoking is the agent's own.
pub(crate) static GIT_ACCESS: ActionSpec = ActionSpec {
    id: "compute.git-access",
    area: Area::Compute,
    params: GIT_PARAMS,
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Compute git access",
        what_changes: "Let {ws}'s compute jobs read {repos} on GitHub",
        consequences: "Jobs from {ws} on this server's own nodes can then fetch and clone {repos} with short-lived, read-only tokens (and keep their caches). A job can read a token it's handed while it runs.",
        scope: "{ws}'s jobs on this server's own nodes",
        duration: "until someone revokes it (k2 compute git-access revoke)",
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "compute", path: "k2 compute git-access (a terminal outside K2)" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: crate::compute_git::NEEDS_PERSON },
        route_prefix: crate::compute_git::GRANT,
        method: WallMethod::Post,
        params: &[("repos", ParamFrom::Response("/error/repos"))],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// The `repos=` as repo keys, each checked.
fn git_repos(ctx: &Ctx) -> Result<Vec<String>, String> {
    let raw: Vec<String> = match ctx.params.get("repos") {
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(serde_json::Value::String(s)) => s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    if raw.is_empty() {
        return Err("name the repos: repos=<owner>/<repo>[,<owner>/<repo>…]".into());
    }
    raw.iter()
        .map(|r| k2_core::compute::proto::gitcred::parse_repo_arg(r).ok_or_else(|| format!("{r:?} isn't a GitHub repo (<owner>/<repo>)")))
        .collect()
}

fn check_git_access(ctx: &Ctx) -> Check {
    if !compute_on() {
        return Check::Moved("compute nodes are off on this server".into());
    }
    let repos = match git_repos(ctx) {
        Ok(r) => r,
        Err(m) => return Check::Moved(m),
    };
    let have: Vec<String> = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::compute::git::grants_of(&conn, &ctx.workspace_id)
            .into_iter()
            .filter(|g| g.nodes == k2_core::compute::git::ALL_NODES)
            .map(|g| g.repo)
            .collect()
    };
    if repos.iter().all(|r| have.contains(r)) {
        Check::AlreadyDone
    } else {
        Check::Needed
    }
}

/// Through the grant route as the answerer (its person check runs again
/// at the click), on every node this server owns.
fn apply_git_access(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    if !compute_on() {
        return Applied::Refused { code: "compute_off".into(), message: "compute nodes are off on this server".into() };
    }
    let repos = match git_repos(ctx) {
        Ok(r) => r,
        Err(m) => return Applied::Refused { code: "bad_params".into(), message: m },
    };
    let path = crate::compute_git::GRANT;
    if let Err(r) = super::switches::route_floor(who, path) {
        return super::switches::applied(&r, "");
    }
    let body = json!({ "workspace": ctx.workspace_id, "github": repos, "nodes": "all" });
    let r = crate::compute_routes::handle(path, who_of(who), body.to_string().as_bytes(), &HashMap::new());
    let shown: Vec<String> = repos.iter().filter_map(|r| k2_core::compute::proto::gitcred::owner_repo(r).map(|(o, n)| format!("{o}/{n}"))).collect();
    super::switches::applied(&r, format!("{}'s compute jobs may read {} now.", ctx.workspace_handle, shown.join(", ")))
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &GIT_ACCESS, check: check_git_access, apply: apply_git_access },
        Registered { spec: &ADMIN_APPROVE, check, apply },
        Registered { spec: &CI, check: check_ci, apply: apply_ci },
        Registered { spec: &CI_OFF, check: check_ci_off, apply: apply_ci_off },
        Registered { spec: &ALLOW_NODES, check: check_allow_nodes, apply: apply_allow_nodes },
        Registered { spec: &ALLOW_NODES_CLEAR, check: check_allow_nodes_clear, apply: apply_allow_nodes_clear },
        Registered { spec: &PACKAGES, check: check_packages, apply: apply_packages },
        Registered { spec: &PACKAGES_POLICY, check: check_packages_policy, apply: apply_packages_policy },
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
