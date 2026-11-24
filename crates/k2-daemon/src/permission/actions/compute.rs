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
    ActionSpec, Area, CardForm, ConfirmAction, Direction, Expiry, Floor, Grant, MachineHint, NeverReason, NotCardable, NotCardableSpec,
    ParamFrom, ParamKind, ParamSpec, Proof, SettingsRef, Undo, Wall, WallCoverage, WallMatch, WallMethod, Words, MAX_EXPIRY_SECS,
};
use serde_json::json;

use super::held::{text_param, who_of};
use super::{Answerer, Applied, Check, Ctx, Registered};
use k2_core::brand::CLI_NAME;
use k2_core::cli_name;

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
        consequences: "Like every approved node change, it runs on {node} as root, signed with the owner's approval key. This is request {request} in Settings → Cores → Compute → Machine settings → Waiting for you: whichever you answer first decides it, and the other then shows it as decided. The agent's reason when it filed the request: {reason}",
        scope: "node request {request}",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "compute", path: "Settings → Cores → Compute → Machine settings → Waiting for you" },
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
    form: None,
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
     Ask your owner to turn on \"Let this agent operate compute nodes\" or \"Let this agent manage compute nodes\" \
     (Settings → Workspaces / Agents → <agent> → Permissions → Compute); the request still waits in Settings → Cores → Compute → Machine settings → Waiting for you";

/// The card's words from the stored node request (0.46.9): `{change}` is
/// its summary (what the owner reads in Settings), `{node}` its node,
/// `{reason}` the reason filed with it. Fixed at filing, so the card's
/// text hash holds until the click.
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action == PACKAGES.id {
        return Some(packages_computed(ctx));
    }
    if action == GRANT.id {
        return Some(grant_computed(ctx));
    }
    if action == ALLOW_NODES_FOR.id {
        return Some(allow_for_computed(ctx));
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
        return Check::Moved(concat!("name the node admin request: request=<its car_… id> (", cli_name!(), " compute node admin requests)").into());
    };
    // A package request (`cpr_…`) has its own card; this one never decides it.
    if is_package_request(id) {
        return Check::Moved(format!("{id} is a package request: ask with {CLI_NAME} permission request {} request={id}", PACKAGES.id));
    }
    let row = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        cadmin::request_by_id(&conn, id)
    };
    let Some(r) = row.filter(|r| r.proposer_workspace.as_deref() == Some(ctx.workspace_id.as_str())) else {
        return Check::Moved(format!("no node admin request {id} of {}'s ({CLI_NAME} compute node admin requests)", ctx.workspace_handle));
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

const CI_SETTING: &str = "Allow CI jobs (Settings → Cores → Compute → Machine settings → Workspaces that may use a machine)";
const CI_NODE: &[ParamSpec] = &[ParamSpec { key: "node", kind: ParamKind::Node, required: true, list: false, choices: &[] }];
const CI_SETTINGS: SettingsRef = SettingsRef { page: "compute", path: "Settings → Cores → Compute → Machine settings → Workspaces that may use a machine" };

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
    form: None,
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
    form: None,
    machine_hints: &[],
};

/// The asking workspace's grant on `node=`: `Ok((node name, its grant))`.
fn ci_grant(ctx: &Ctx) -> Result<(String, k2_core::compute::store::GrantRow), String> {
    let name = text_param(ctx, "node").ok_or(concat!("name the node: node=<name> (", cli_name!(), " compute nodes)"))?;
    let h = k2_core::db::shared();
    let conn = h.lock();
    let n = k2_core::compute::store::find_node(&conn, name)
        .filter(|n| n.state == "active")
        .ok_or_else(|| format!("no active compute node '{name}' ({CLI_NAME} compute nodes)"))?;
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
        format!("{} sends no more CI jobs to {node}. Turn it back on in Settings → Cores → Compute → Machine settings → Workspaces that may use a machine.", ctx.workspace_handle)
    };
    super::switches::applied(&r, message)
}

fn apply_ci(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    apply_ci_to(ctx, who, true)
}

fn apply_ci_off(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    apply_ci_to(ctx, who, false)
}

// ── compute.gpu (GPU jobs, 0.46.11, prd-compute-gpu-jobs-v1 S1) ──────

const GPU_SETTING: &str = concat!("Allow GPU jobs (", cli_name!(), " compute grant <node> --workspace <ws> --gpu)");

/// `compute.gpu node=`: let the asking workspace send `--gpu` jobs to one
/// node (its grant's `allow_gpu`). Admin floor, like `compute.ci`: only the
/// owner or an Admin login (never an agent, not even a compute manager).
/// The node's own owner still decides at the machine whether the node runs
/// GPU jobs for this server (`sudo k2-node admin gpu on`); the card can't
/// change that.
pub(crate) static GPU: ActionSpec = ActionSpec {
    id: "compute.gpu",
    area: Area::Compute,
    params: CI_NODE,
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: GPU_SETTING,
        what_changes: "Let {ws} send GPU jobs to {node}",
        consequences: "A GPU job gets one GPU of {node}, alone, while it runs (other GPU jobs wait). GPU drivers are a large attack surface, and on a desktop machine a GPU fault can end its owner's session. Give it to workspaces whose tests need a real GPU.",
        scope: "{ws} on {node}",
        duration: "until someone turns it off",
        retry: "run your command again",
    },
    undo: Undo::Twin { action: "compute.gpu-off" },
    settings: CI_SETTINGS,
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: k2_core::compute::proto::gpu::codes::GPU_NEEDS_GRANT },
        route_prefix: "/cli/compute/",
        method: WallMethod::Any,
        params: &[("node", ParamFrom::Response("/error/node"))],
    }],
    coverage: WallCoverage::Code,
    form: None,
    machine_hints: &[],
};

/// The twin: stop it again (asked only, no wall).
pub(crate) static GPU_OFF: ActionSpec = ActionSpec {
    id: "compute.gpu-off",
    area: Area::Compute,
    params: CI_NODE,
    floor: Floor::Admin,
    direction: Direction::Tighten,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: GPU_SETTING,
        what_changes: "Stop {ws} sending GPU jobs to {node}",
        consequences: "Its GPU jobs already queued there keep their place; new ones are refused (gpu_needs_grant).",
        scope: "{ws} on {node}",
        duration: "until someone turns it on",
        retry: "",
    },
    undo: Undo::None,
    settings: CI_SETTINGS,
    walls: &[],
    coverage: WallCoverage::None,
    form: None,
    machine_hints: &[],
};

fn check_gpu_state(ctx: &Ctx, want_on: bool) -> Check {
    if !compute_on() {
        return Check::Moved("compute nodes are off on this server".into());
    }
    match ci_grant(ctx) {
        Ok((_, g)) if g.allow_gpu == want_on => Check::AlreadyDone,
        Ok(_) => Check::Needed,
        Err(m) => Check::Moved(m),
    }
}

fn check_gpu(ctx: &Ctx) -> Check {
    check_gpu_state(ctx, true)
}

fn check_gpu_off(ctx: &Ctx) -> Check {
    check_gpu_state(ctx, false)
}

/// Through the Settings route's own entry (`POST /cli/compute/grants/set`,
/// Admin), as the answerer: the grant's current limits plus `allowGpu`.
fn apply_gpu_to(ctx: &Ctx, who: &Answerer, on: bool) -> Applied {
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
        "allowGpu": on,
    });
    let path = "/cli/compute/grants/set";
    if let Err(r) = super::switches::route_floor(who, path) {
        return super::switches::applied(&r, "");
    }
    let r = crate::compute_routes::handle(path, who_of(who), body.to_string().as_bytes(), &HashMap::new());
    let message = if on {
        format!("{} may send GPU jobs to {node} now (while that node's owner runs GPU jobs for this server).", ctx.workspace_handle)
    } else {
        format!("{} sends no more GPU jobs to {node}.", ctx.workspace_handle)
    };
    super::switches::applied(&r, message)
}

fn apply_gpu(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    apply_gpu_to(ctx, who, true)
}

fn apply_gpu_off(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    apply_gpu_to(ctx, who, false)
}

// ── compute.grant (0.46.17) ──────────────────────────────────────────

/// The limits `compute.grant` may change, as `k2 compute grant` names them.
const GRANT_LIMIT_KEYS: [&str; 4] = ["max-time", "max-parallel", "max-queued", "max-disk"];

const GRANT_PARAMS: &[ParamSpec] = &[
    ParamSpec { key: "node", kind: ParamKind::Node, required: true, list: false, choices: &[] },
    // A duration as `k2 compute grant --max-time` takes it (4h, 90m,
    // 1h30m, 300s or whole seconds).
    ParamSpec { key: "max-time", kind: ParamKind::Text, required: false, list: false, choices: &[] },
    ParamSpec { key: "max-parallel", kind: ParamKind::Text, required: false, list: false, choices: &[] },
    ParamSpec { key: "max-queued", kind: ParamKind::Text, required: false, list: false, choices: &[] },
    // GB, as `--max-disk 60G` (or 60).
    ParamSpec { key: "max-disk", kind: ParamKind::Text, required: false, list: false, choices: &[] },
];

/// `compute.grant node= [max-time=] [max-parallel=] [max-queued=]
/// [max-disk=]`: change the limits of the asking workspace's own grant on
/// one node, as `k2 compute grant <node> --workspace <its own>` would
/// (Rosson 2026-11-19: "Is this not a card you can present?"). Always the
/// asker's own workspace (the request can't name one). Only a node it holds
/// a grant on, or any active node while its "Let this agent use compute
/// nodes" is on (that switch grants every active node). Never `--ci` or
/// `--gpu`: those keep their own cards (`compute.ci`, `compute.gpu`).
/// Allow goes through `POST /cli/compute/grants/set` as the person, so a
/// changed max-parallel re-sends the jobs waiting there, as the CLI's does.
pub(crate) static GRANT: ActionSpec = ActionSpec {
    id: "compute.grant",
    area: Area::Compute,
    params: GRANT_PARAMS,
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    // K2 writes the words from the stored values and the grant now
    // (`grant_computed`): `{change}` names each limit asked, with what it is
    // now while that differs; `{effects}` the limits that stay and what
    // else changes.
    words: Words {
        setting: "Compute job limits",
        what_changes: "{change}",
        consequences: "{effects}",
        scope: "{ws} on {node}",
        duration: "until someone changes them",
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: CI_SETTINGS,
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "owner_only" },
        route_prefix: "/cli/compute/grants/set",
        method: WallMethod::Post,
        params: &[
            ("node", ParamFrom::Body("/node")),
            ("max-time", ParamFrom::BodySecs("/maxJobSecs")),
            ("max-parallel", ParamFrom::Body("/maxParallel")),
            ("max-queued", ParamFrom::Body("/maxQueued")),
            ("max-disk", ParamFrom::Body("/maxDiskGb")),
        ],
    }],
    coverage: WallCoverage::PrefixOnly,
    form: None,
    machine_hints: &[],
};

/// One grant's four limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    pub secs: i64,
    pub parallel: i64,
    pub queued: i64,
    pub disk_gb: i64,
}

impl Limits {
    fn of(g: &k2_core::compute::store::GrantRow) -> Limits {
        Limits { secs: g.max_job_secs, parallel: g.max_parallel, queued: g.max_queued, disk_gb: g.max_disk_gb }
    }
}

/// What a `compute.grant` request asks, checked: the node, the limits it
/// changes, the grant's limits now, and whether there is a grant yet.
#[derive(Debug, Clone)]
pub(crate) struct GrantAsk {
    pub node: String,
    pub asked: [Option<i64>; 4],
    pub now: Limits,
    pub has_grant: bool,
}

impl GrantAsk {
    /// The grant's limits once applied.
    pub(crate) fn wanted(&self) -> Limits {
        let [t, p, q, d] = self.asked;
        Limits {
            secs: t.unwrap_or(self.now.secs),
            parallel: p.unwrap_or(self.now.parallel),
            queued: q.unwrap_or(self.now.queued),
            disk_gb: d.unwrap_or(self.now.disk_gb),
        }
    }
}

/// `4h`, `90m`, `1h30m`, `300s` or whole seconds: the CLI's `--max-time`.
pub(crate) fn parse_duration(raw: &str) -> Option<i64> {
    let t = raw.trim();
    if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
        return t.parse().ok();
    }
    let (mut total, mut num) = (0i64, String::new());
    for c in t.chars() {
        match c {
            '0'..='9' => num.push(c),
            'h' | 'm' | 's' if !num.is_empty() => {
                let n: i64 = num.parse().ok()?;
                let mul = match c {
                    'h' => 3600,
                    'm' => 60,
                    _ => 1,
                };
                total = total.checked_add(n.checked_mul(mul)?)?;
                num.clear();
            }
            _ => return None,
        }
    }
    (num.is_empty() && !t.is_empty()).then_some(total)
}

/// `60`, `60G`, `60g` or `60GB`: the CLI's `--max-disk`, in GB.
fn parse_disk(raw: &str) -> Option<i64> {
    let t = raw.trim();
    let t = t.strip_suffix("GB").or_else(|| t.strip_suffix("gb")).or_else(|| t.strip_suffix(['G', 'g'])).unwrap_or(t);
    (!t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())).then(|| t.parse().ok()).flatten()
}

/// One asked limit, checked against the same bounds as the grant route
/// (`compute_routes::grant_set`).
fn grant_limit(ctx: &Ctx, key: &str) -> Result<Option<i64>, String> {
    let Some(raw) = text_param(ctx, key) else { return Ok(None) };
    let (v, lo, hi, how) = match key {
        "max-time" => (parse_duration(raw), 60, 7 * 86_400, "a duration from 1m to 168h (4h, 90m, 1h30m, or seconds)"),
        "max-parallel" => (raw.parse().ok(), 1, 64, "a number from 1 to 64"),
        "max-queued" => (raw.parse().ok(), 1, 1000, "a number from 1 to 1000"),
        _ => (parse_disk(raw), 1, 10_000, "GB from 1 to 10000 (60 or 60G)"),
    };
    match v {
        Some(n) if (lo..=hi).contains(&n) => Ok(Some(n)),
        _ => Err(format!("{key}={raw} is not {how}")),
    }
}

/// The request's values and the grant they change, or why it can't be
/// asked: no limit named, a bad value, no such active node, or a node
/// the workspace may not use.
pub(crate) fn grant_ask(ctx: &Ctx) -> Result<GrantAsk, String> {
    let name = text_param(ctx, "node").ok_or(concat!("name the node: node=<name> (", cli_name!(), " compute grants shows yours)"))?;
    let [t, p, q, d] = GRANT_LIMIT_KEYS;
    let asked = [grant_limit(ctx, t)?, grant_limit(ctx, p)?, grant_limit(ctx, q)?, grant_limit(ctx, d)?];
    if asked.iter().all(Option::is_none) {
        return Err("name at least one limit: max-time=, max-parallel=, max-queued= or max-disk= (--ci and --gpu have their own cards: compute.ci, compute.gpu)".into());
    }
    let h = k2_core::db::shared();
    let conn = h.lock();
    let n = k2_core::compute::store::find_node(&conn, name)
        .filter(|n| n.state == "active")
        .ok_or_else(|| format!("no active compute node '{name}' ({CLI_NAME} compute nodes)"))?;
    let (now, has_grant) = match k2_core::compute::store::grant(&conn, &n.id, &ctx.workspace_id) {
        Some(g) => (Limits::of(&g), true),
        None if k2_core::compute::store::agents_can_use_compute(&conn, &ctx.workspace_id) => {
            let l = k2_core::compute::access::limits_for(&n, &Default::default());
            let v = |x: Option<i64>| x.unwrap_or_default();
            (Limits { secs: v(l.max_job_secs), parallel: v(l.max_parallel), queued: v(l.max_queued), disk_gb: v(l.max_disk_gb) }, false)
        }
        None => {
            return Err(format!(
                "{} has no grant on '{}' and can't use compute nodes: ask for that first ({CLI_NAME} permission request compute.use --why \"…\")",
                ctx.workspace_handle, n.name
            ))
        }
    };
    Ok(GrantAsk { node: n.name, asked, now, has_grant })
}

/// 14400 → "4 hours", 5400 → "1 hour 30 minutes", 90 → "1 minute 30 seconds".
pub(crate) fn hours_words(secs: i64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let unit = |n: i64, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
    let mut parts = Vec::new();
    if h > 0 {
        parts.push(unit(h, "hour"));
    }
    if m > 0 {
        parts.push(unit(m, "minute"));
    }
    if s > 0 || parts.is_empty() {
        parts.push(unit(s, "second"));
    }
    parts.join(" ")
}

/// "one job" / "3 jobs".
fn jobs(n: i64) -> String {
    if n == 1 {
        "one job".to_string()
    } else {
        format!("{n} jobs")
    }
}

/// Each limit in plain words: what it lets the agent do ("run jobs of up
/// to 4 hours"), the limit as it stands ("jobs of up to 4 hours") and its
/// value alone, for "(now …)".
fn limit_words(i: usize, l: &Limits) -> (String, String, String) {
    match i {
        0 => {
            let t = hours_words(l.secs);
            (format!("run jobs of up to {t}"), format!("jobs of up to {t}"), t)
        }
        1 => (format!("run {} at a time", jobs(l.parallel)), format!("{} at a time", jobs(l.parallel)), l.parallel.to_string()),
        2 => (format!("keep up to {} waiting", jobs(l.queued)), format!("up to {} waiting", jobs(l.queued)), l.queued.to_string()),
        _ => (format!("use up to {} GB of disk per job", l.disk_gb), format!("{} GB of disk per job", l.disk_gb), format!("{} GB", l.disk_gb)),
    }
}

/// "a, b and c".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The card's words for `compute.grant`: `{change}` ("Let k2-engine run
/// jobs of up to 4 hours on node-2 (now 2 hours)"), `{effects}` (the limits
/// that stay, a grant it doesn't have yet, the waiting jobs a new
/// max-parallel re-sends) and `{node}`. "(now …)" shows only while it
/// differs, so the decided card reads true too.
fn grant_computed(ctx: &Ctx) -> Ctx {
    let mut out = ctx.clone();
    let ws = &ctx.workspace_handle;
    let (change, effects, node) = match grant_ask(ctx) {
        Ok(a) => {
            let want = a.wanted();
            let mut phrases = Vec::new();
            let mut stays = Vec::new();
            for (i, asked) in a.asked.iter().enumerate() {
                let (wanted, _, _) = limit_words(i, &want);
                let (now_words, stands, now_value) = limit_words(i, &a.now);
                if asked.is_none() {
                    stays.push(stands);
                    continue;
                }
                let was = (wanted != now_words).then(|| format!(" (now {now_value})"));
                phrases.push((wanted, was));
            }
            let change = match phrases.as_slice() {
                [(one, was)] => format!("Let {ws} {one} on {}{}", a.node, was.clone().unwrap_or_default()),
                many => {
                    let items: Vec<String> = many.iter().map(|(w, was)| format!("{w}{}", was.clone().unwrap_or_default())).collect();
                    format!("Let {ws} {} on {}", and_list(&items), a.node)
                }
            };
            let mut effects = Vec::new();
            if !a.has_grant {
                effects.push(format!("{ws} has no grant on {} yet: this gives it one, so its jobs may run there.", a.node));
            }
            if !stays.is_empty() {
                effects.push(format!("Its other limits on {} stay as they are: {}.", a.node, and_list(&stays)));
            }
            if a.has_grant && want.parallel != a.now.parallel {
                effects.push(format!("Its jobs already waiting on {} are sent again with the new limit, in the same order.", a.node));
            }
            if want.secs > a.now.secs || want.parallel > a.now.parallel {
                effects.push(format!("Other jobs on {} may wait longer for a slot.", a.node));
            }
            (change, effects.join(" "), a.node)
        }
        Err(_) => {
            let node = text_param(ctx, "node").unwrap_or("its node").to_string();
            (format!("Change {ws}'s compute job limits on {node}"), String::new(), node)
        }
    };
    out.params.insert("change".into(), json!(change));
    out.params.insert("effects".into(), json!(effects));
    out.params.insert("node".into(), json!(node));
    out
}

fn check_grant(ctx: &Ctx) -> Check {
    if !compute_on() {
        return Check::Moved("compute nodes are off on this server".into());
    }
    match grant_ask(ctx) {
        Ok(a) if a.has_grant && a.wanted() == a.now => Check::AlreadyDone,
        Ok(_) => Check::Needed,
        Err(m) => Check::Moved(m),
    }
}

/// Through the grant route's own entry (`POST /cli/compute/grants/set`,
/// the one `k2 compute grant` calls), as the person: the asked limits, the
/// others as they are (that route resets a limit it isn't sent), and no
/// CI, GPU or per-submitter value (absent keeps the grant's).
fn apply_grant(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    if !compute_on() {
        return Applied::Refused { code: "compute_off".into(), message: "compute nodes are off on this server".into() };
    }
    let a = match grant_ask(ctx) {
        Ok(a) => a,
        Err(m) => return Applied::Refused { code: "bad_params".into(), message: m },
    };
    let want = a.wanted();
    let path = "/cli/compute/grants/set";
    if let Err(r) = super::switches::route_floor(who, path) {
        return super::switches::applied(&r, "");
    }
    let body = json!({
        "node": a.node,
        "workspace": ctx.workspace_id,
        "maxJobSecs": want.secs,
        "maxParallel": want.parallel,
        "maxQueued": want.queued,
        "maxDiskGb": want.disk_gb,
    });
    let r = crate::compute_routes::handle(path, who_of(who), body.to_string().as_bytes(), &HashMap::new());
    let resent = serde_json::from_str::<serde_json::Value>(&r.body).ok().and_then(|v| v["resent"].as_i64()).unwrap_or(0);
    let mut message = format!(
        "{}'s limits on {} are now: jobs of up to {}, {} at a time, up to {} waiting, {} GB of disk per job.",
        ctx.workspace_handle,
        a.node,
        hours_words(want.secs),
        jobs(want.parallel),
        jobs(want.queued),
        want.disk_gb
    );
    match resent {
        0 => {}
        1 => message.push_str(" Its job waiting there was sent again with the new limit."),
        n => message.push_str(&format!(" Its {n} jobs waiting there were sent again with the new limit, in the same order.")),
    }
    super::switches::applied(&r, message)
}

// ── compute.allow-nodes (0.46.8, `crate::compute_allow`) ────────────

const ALLOW_SETTING: &str = "Compute node list";
const ALLOW_SETTINGS: SettingsRef = SettingsRef { page: "compute", path: concat!(cli_name!(), " compute allow-nodes (a terminal outside K2)") };
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
    form: None,
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
    form: None,
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
        return Err(concat!("name the nodes: nodes=<node>[,<node>…] (", cli_name!(), " compute nodes)").into());
    }
    let h = k2_core::db::shared();
    let conn = h.lock();
    let mut ids = Vec::new();
    for name in &names {
        let n = k2_core::compute::store::find_node(&conn, name)
            .filter(|n| n.state != "revoked")
            .ok_or_else(|| format!("no compute node '{name}' ({CLI_NAME} compute nodes)"))?;
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

// ── compute.allow-nodes-for (0.46.18, `crate::compute_allow`) ────────

const ALLOW_FOR_PARAMS: &[ParamSpec] = &[
    // The other workspace (its handle, name or id), never the asker's.
    ParamSpec { key: "target", kind: ParamKind::Workspace, required: true, list: false, choices: &[] },
    // Exactly one of the three.
    ParamSpec { key: "add", kind: ParamKind::Node, required: false, list: true, choices: &[] },
    ParamSpec { key: "remove", kind: ParamKind::Node, required: false, list: true, choices: &[] },
    ParamSpec { key: "clear", kind: ParamKind::Choice, required: false, list: false, choices: &["true"] },
];

/// The managing agent's refusal names the workspace and its one change.
const ALLOW_FOR_WALL_PARAMS: &[(&str, ParamFrom)] = &[
    ("target", ParamFrom::Response("/error/workspace")),
    ("add", ParamFrom::Response("/error/add")),
    ("remove", ParamFrom::Response("/error/remove")),
    ("clear", ParamFrom::Response("/error/clear")),
];

/// `compute.allow-nodes-for target= add=|remove=|clear=true` (Rosson
/// 2026-11-23: "This should be a card you can present"): change ANOTHER
/// workspace's compute node list, asked by an agent that manages it (it
/// hired it, or it may manage this server's compute nodes:
/// [`crate::compute_allow::manages`]). A new node joins lists by itself
/// since 0.46.18; this is for the rest (a fixed list, a node to take off).
/// A person at the floor `k2 compute allow-nodes --workspace` needs (the
/// owner or an Admin) taps Allow, and `POST /cli/compute/allow-nodes/set`
/// runs as them, so every check of the route runs again at the click and
/// its `allow_nodes` event names them (and `askedBy` the agent). Only
/// nodes granted to that workspace can be added. One-time, no undo.
pub(crate) static ALLOW_NODES_FOR: ActionSpec = ActionSpec {
    id: "compute.allow-nodes-for",
    area: Area::Compute,
    params: ALLOW_FOR_PARAMS,
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    // K2 writes the words from the stored values (`allow_for_computed`):
    // `{target}` the workspace's handle, `{change}` the one change in plain
    // words, `{effects}` what it does and why this agent may ask.
    words: Words {
        setting: "{target}'s compute node list",
        what_changes: "{change}",
        consequences: "{effects}",
        scope: "{target}'s compute node list",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "compute", path: concat!(cli_name!(), " compute allow-nodes --workspace {target} (a terminal outside K2)") },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: crate::compute_allow::FOR_NEEDS_PERSON },
        route_prefix: crate::compute_allow::SET,
        method: WallMethod::Post,
        params: ALLOW_FOR_WALL_PARAMS,
    }],
    coverage: WallCoverage::Code,
    form: None,
    machine_hints: &[],
};

/// What a `compute.allow-nodes-for` request asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForChange {
    Add(Vec<String>),
    Remove(Vec<String>),
    Clear,
}

/// A `compute.allow-nodes-for` request, checked: the workspace (another
/// one, that the asker manages), its one change, and the node ids it names.
pub(crate) struct ForAsk {
    pub target: crate::compute_routes::Ws,
    pub how: crate::compute_allow::Manages,
    pub change: ForChange,
    pub ids: Vec<String>,
}

/// A list param's names (a list, or one comma string).
fn list_param(ctx: &Ctx, key: &str) -> Vec<String> {
    match ctx.params.get(key) {
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(serde_json::Value::String(s)) => s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// Why it can't be asked: `Err((None, why))` is `bad_params` (values that
/// don't hold), `Err((Some(code), why))` an agent that may not ask.
pub(crate) fn for_ask(ctx: &Ctx) -> Result<ForAsk, (Option<&'static str>, String)> {
    let bad = |m: String| (None, m);
    let token = text_param(ctx, "target").ok_or_else(|| bad("name the workspace: target=<workspace>".into()))?;
    let target = crate::compute_routes::ws_by_token(token).ok_or_else(|| bad(format!("no workspace '{token}' on this server")))?;
    if target.id == ctx.workspace_id {
        return Err(bad(format!(
            "{} is your own workspace: ask with {CLI_NAME} permission request compute.allow-nodes nodes=<node> (to add), or take a node off yourself \
             ({CLI_NAME} compute allow-nodes --remove <node>)",
            target.handle
        )));
    }
    let (add, remove) = (list_param(ctx, "add"), list_param(ctx, "remove"));
    let clear = text_param(ctx, "clear") == Some("true");
    let change = match (add.is_empty(), remove.is_empty(), clear) {
        (false, true, false) => ForChange::Add(add),
        (true, false, false) => ForChange::Remove(remove),
        (true, true, true) => ForChange::Clear,
        _ => return Err(bad("say one change: add=<node>[,<node>…], remove=<node>[,<node>…] or clear=true".into())),
    };
    let Some(how) = crate::compute_allow::manages(&ctx.workspace_id, &target.id) else {
        return Err((
            Some(crate::compute_allow::NOT_MANAGER),
            format!(
                "{} doesn't manage {}: only an agent that hired it, or one that may manage this server's compute nodes, asks for its node list. \
                 {}'s own agent asks for its own list, or a person runs {CLI_NAME} compute allow-nodes --workspace {} from a terminal outside K2",
                ctx.workspace_handle, target.handle, target.handle, target.handle
            ),
        ));
    };
    let h = k2_core::db::shared();
    let conn = h.lock();
    let mut ids = Vec::new();
    if let ForChange::Add(names) | ForChange::Remove(names) = &change {
        for name in names {
            let n = k2_core::compute::store::find_node(&conn, name)
                .filter(|n| n.state != "revoked")
                .ok_or_else(|| bad(format!("no compute node '{name}' ({CLI_NAME} compute nodes)")))?;
            if matches!(change, ForChange::Add(_)) && k2_core::compute::store::grant(&conn, &n.id, &target.id).is_none() {
                return Err(bad(format!(
                    "{} has no grant on '{}' (allow_nodes_not_granted): a person grants it first ({CLI_NAME} compute grant {} --workspace {})",
                    target.handle, n.name, n.name, target.handle
                )));
            }
            if !ids.contains(&n.id) {
                ids.push(n.id);
            }
        }
    }
    Ok(ForAsk { target, how, change, ids })
}

/// "a, b and c" for the node names the request holds.
fn names_words(names: &[String]) -> String {
    and_list(names)
}

/// The card's words for `compute.allow-nodes-for`, from the stored values:
/// `{target}` ("k2-engine"), `{change}` ("Add k2n13 and k2n14 to
/// k2-engine's compute node list"), `{effects}`.
fn allow_for_computed(ctx: &Ctx) -> Ctx {
    let mut out = ctx.clone();
    let ws = &ctx.workspace_handle;
    let (target, change, effects) = match for_ask(ctx) {
        Ok(a) => {
            let t = a.target.handle.clone();
            let why = format!("{ws} asks because {}; it gets no other access to {t}.", a.how.words(&t));
            let (change, effects) = match &a.change {
                ForChange::Add(n) => (
                    format!("Add {} to {t}'s compute node list", names_words(n)),
                    format!(
                        "{t}'s jobs may then run on {}, and each job sends its whole source tree there (its --with repos too). Add only machines that may hold {t}'s code. {why}",
                        names_words(n)
                    ),
                ),
                ForChange::Remove(n) => (
                    format!("Take {} off {t}'s compute node list", names_words(n)),
                    format!(
                        "{t}'s jobs no longer run on {}: a job of {t}'s still waiting there ends (failed/node_not_allowed), and its next jobs go to the nodes left on the list. {why}",
                        names_words(n)
                    ),
                ),
                ForChange::Clear => (
                    format!("Remove {t}'s compute node list"),
                    format!("{t}'s jobs may then run on any node granted to it again, and each job sends its whole source tree to the node it runs on. {why}"),
                ),
            };
            (t, change, effects)
        }
        Err(_) => {
            let t = text_param(ctx, "target").unwrap_or("another workspace").to_string();
            (t.clone(), format!("Change {t}'s compute node list"), String::new())
        }
    };
    out.params.insert("target".into(), json!(target));
    out.params.insert("change".into(), json!(change));
    out.params.insert("effects".into(), json!(effects));
    out
}

fn check_allow_nodes_for(ctx: &Ctx) -> Check {
    if !compute_on() {
        return Check::Moved("compute nodes are off on this server".into());
    }
    let a = match for_ask(ctx) {
        Ok(a) => a,
        Err((Some(code), why)) => return Check::Unavailable { code, why },
        Err((None, why)) => return Check::Moved(why),
    };
    let entry = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::compute::allow::get(&conn, &a.target.id)
    };
    let list = match entry {
        Ok(e) => e.map(|e| e.nodes),
        Err(e) => return Check::Moved(format!("{}'s node list can't be read ({e}); a person sets it again", a.target.handle)),
    };
    match (&a.change, list) {
        // No list: every granted node is allowed already, and nothing to
        // take off or clear.
        (_, None) => Check::AlreadyDone,
        (ForChange::Add(_), Some(l)) if a.ids.iter().all(|id| l.contains(id)) => Check::AlreadyDone,
        (ForChange::Remove(_), Some(l)) if !a.ids.iter().any(|id| l.contains(id)) => Check::AlreadyDone,
        (ForChange::Remove(_), Some(l)) if l.iter().all(|id| a.ids.contains(id)) => Check::Moved(format!(
            "that would leave {}'s list empty; a list keeps at least one node (clear=true removes the list: every granted node again)",
            a.target.handle
        )),
        _ => Check::Needed,
    }
}

/// Through the route's own entry, as the person who clicked: the
/// workspace by id, the one change, and who asked (for its event).
fn apply_allow_nodes_for(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let a = match for_ask(ctx) {
        Ok(a) => a,
        Err((_, m)) => return Applied::Refused { code: "bad_params".into(), message: m },
    };
    let t = &a.target.handle;
    let mut body = json!({ "workspace": a.target.id, "askedBy": format!("agent:{}", ctx.workspace_handle) });
    let message = match &a.change {
        ForChange::Add(n) => {
            body["add"] = json!(n);
            format!("{t}'s jobs may run on {} now.", names_words(n))
        }
        ForChange::Remove(n) => {
            body["remove"] = json!(n);
            format!("{} {} off {t}'s compute node list now.", names_words(n), if n.len() == 1 { "is" } else { "are" })
        }
        ForChange::Clear => {
            body["clear"] = json!(true);
            format!("{t} has no node list now: its jobs may go to any node granted to it.")
        }
    };
    apply_allow_to(who, body, message)
}

/// The card's extra values for `compute.allow-nodes` (0.46.15): one row
/// per named node with what the person is letting the agent's jobs reach,
/// its platform, its size for this server and who owns it ("lc: linux
/// x86_64 · 8 CPUs · 32 GB · owned by this server"). Values sit outside
/// the card's text hash, so a node coming online between the ask and the
/// click never makes the card stale. Empty for every other action.
pub(crate) fn card_values(action: &str, ctx: &Ctx) -> Vec<k2_core::permission::card::KeyValue> {
    // 0.46.18: the nodes another workspace's list would gain, the same way.
    let names = match action {
        a if a == ALLOW_NODES.id => node_names(ctx),
        a if a == ALLOW_NODES_FOR.id => list_param(ctx, "add"),
        _ => return Vec::new(),
    };
    let h = k2_core::db::shared();
    let conn = h.lock();
    names
        .into_iter()
        .map(|name| {
            let value = match k2_core::compute::store::find_node(&conn, &name).filter(|n| n.state != "revoked") {
                Some(n) => node_facts(&n),
                None => "not a compute node on this server".to_string(),
            };
            k2_core::permission::card::KeyValue { key: name, value }
        })
        .collect()
}

/// "linux x86_64 · 8 CPUs · 32 GB · owned by this server" (parts it
/// doesn't know are left out; no offer yet says so).
fn node_facts(n: &k2_core::compute::store::NodeRow) -> String {
    let mut parts: Vec<String> = Vec::new();
    let platform = [n.os.as_deref(), n.arch.as_deref()].into_iter().flatten().filter(|x| !x.trim().is_empty()).collect::<Vec<_>>().join(" ");
    if !platform.is_empty() {
        parts.push(platform);
    }
    match n.last_offer.as_ref() {
        Some(o) => {
            let cpus = o.caps.cpu_millis as f64 / 1000.0;
            parts.push(format!("{} CPU{}", fmt_num(cpus), if (cpus - 1.0).abs() < 1e-9 { "" } else { "s" }));
            parts.push(format!("{} GB", fmt_num((o.caps.mem_bytes as f64 / (1u64 << 30) as f64 * 10.0).round() / 10.0)));
            if o.caps.gpus > 0 {
                parts.push(format!("{} GPU{}", o.caps.gpus, if o.caps.gpus == 1 { "" } else { "s" }));
            }
        }
        None => parts.push("size not reported yet".to_string()),
    }
    parts.push(if n.is_tenant() {
        format!("owned by {}", n.owner_label.as_deref().filter(|x| !x.trim().is_empty()).unwrap_or("another server"))
    } else if n.shared {
        "owned by this server, shared with others".to_string()
    } else {
        "owned by this server".to_string()
    });
    if n.state != "active" {
        parts.push(n.state.clone());
    }
    parts.join(" · ")
}

/// 8 → "8", 7.5 → "7.5".
fn fmt_num(x: f64) -> String {
    if (x - x.round()).abs() < 1e-9 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x:.1}")
    }
}

// ── ci.cache-trust (prd-compute-blacksmith-class-v1 B2, §4.1.5) ─────

/// `ci.cache-trust branches=`: add branches to the asking workspace's
/// trusted branches, whose passing CI checks save the build cache every
/// job of the workspace starts from. A person decides (the owner or an
/// Admin); applying it also records the git remote the workspace fetches
/// from now (the trusted cache is pinned to it). Removing a branch is the
/// agent's own (`k2 ci cache trust --remove`), so there is no twin.
pub(crate) static CI_CACHE_TRUST: ActionSpec = ActionSpec {
    id: "ci.cache-trust",
    area: Area::Compute,
    params: &[ParamSpec { key: "branches", kind: ParamKind::Branch, required: true, list: true, choices: &[] }],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "Trusted CI branches",
        what_changes: "Trust {branches} for {ws}'s CI cache",
        consequences: "Passing CI checks of {branches} (pushed commits, checked by the K2 daemon) then save the build cache every job of {ws} starts from, so code merged there reaches every later build. Trust only branches whose commits you review.",
        scope: "{ws}'s CI cache",
        duration: "until someone removes the branch",
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "compute", path: concat!(cli_name!(), " ci cache trust (a terminal outside K2)") },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: k2_core::ci::CACHE_TRUST_NEEDS_PERSON },
        route_prefix: crate::ci_routes::CACHE_TRUST_SET,
        method: WallMethod::Post,
        params: &[("branches", ParamFrom::Response("/error/branches"))],
    }],
    coverage: WallCoverage::Code,
    form: None,
    machine_hints: &[],
};

/// The `branches=` names (a list, or one comma-separated string).
fn branch_names(ctx: &Ctx) -> Vec<String> {
    match ctx.params.get("branches") {
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(serde_json::Value::String(s)) => s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

fn check_cache_trust(ctx: &Ctx) -> Check {
    if !compute_on() {
        return Check::Moved("compute nodes are off on this server".into());
    }
    let names = branch_names(ctx);
    if names.is_empty() {
        return Check::Moved("name the branches: branches=<branch>[,<branch>…]".into());
    }
    if let Some(bad) = names.iter().find(|b| !k2_core::ci::valid_trusted_branch(b)) {
        return Check::Moved(format!("{bad:?} is not a branch name (no globs, no wt:)"));
    }
    let now = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::ci::trusted(&conn, &ctx.workspace_id)
    };
    if names.iter().all(|b| now.branches.contains(b)) {
        Check::AlreadyDone
    } else {
        Check::Needed
    }
}

fn apply_cache_trust(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let names = branch_names(ctx);
    let path = crate::ci_routes::CACHE_TRUST_SET;
    if let Err(r) = super::switches::route_floor(who, path) {
        return super::switches::applied(&r, "");
    }
    let body = json!({ "workspace": ctx.workspace_id, "add": names });
    let message = format!("{}'s CI checks of {} may save its trusted build cache now.", ctx.workspace_handle, names.join(", "));
    let r = crate::compute_routes::handle(path, who_of(who), body.to_string().as_bytes(), &HashMap::new());
    super::switches::applied(&r, message)
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
        consequences: "Root installs them on {nodes} from the distro's signed official repos, after a dry run that refuses anything that removes a package, comes from another repo or is too big. {shared}This is request {request} in Settings → Cores → Compute → Machine settings → Waiting for you: whichever you answer first decides it. The agent's reason: {reason}",
        scope: "package request {request}",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "compute", path: "Settings → Cores → Compute → Machine settings → Waiting for you" },
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
    form: None,
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
        return Check::Moved(concat!("name the package request: request=<its cpr_… id> (", cli_name!(), " compute node packages requests)").into());
    };
    if !is_package_request(id) {
        return Check::Moved(format!("{id} is not a package request (cpr_…): see {CLI_NAME} compute node packages requests"));
    }
    let rows = package_rows(ctx, id);
    if rows.is_empty() {
        return Check::Moved(format!("no package request {id} of {}'s ({CLI_NAME} compute node packages requests)", ctx.workspace_handle));
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
                    "{} is offline, so it waits there: a person approves it in Settings → Cores → Compute → Machine settings → Waiting for you when it's back.",
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
/// owner approves in Settings → Cores → Compute → Machine settings → Waiting for you (root on the node).
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
        consequences: "list installs only names on {node}'s approved list; profiles adds K2's bundles you turned on; official allows any package from the distro's signed official repos, minus K2's deny list. A package installs for every job on that machine. Narrowing applies at once. Widening runs as root on {node}, so it waits as a node request for you in Settings → Cores → Compute → Machine settings → Waiting for you.",
        scope: "{server} on {node}",
        duration: "until someone changes it",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "compute", path: "Settings → Cores → Compute" },
    walls: &[Wall {
        // The policy route, to an agent that may not operate compute nodes.
        matches: WallMatch::Code { status: 403, code: k2_core::compute::admin::codes::OPERATE_OFF },
        route_prefix: crate::compute_pkg::POLICY,
        method: WallMethod::Post,
        params: &[("node", ParamFrom::Body("/node")), ("server", ParamFrom::Body("/server")), ("level", ParamFrom::Body("/level"))],
    }],
    coverage: WallCoverage::Code,
    form: None,
    machine_hints: &[],
};

/// The named node: active, this server's own. `Ok((node, its offer))`.
fn policy_node(ctx: &Ctx) -> Result<(k2_core::compute::store::NodeRow, Option<k2_core::compute::proto::frames::Offer>), String> {
    let name = text_param(ctx, "node").ok_or(concat!("name the node: node=<name> (", cli_name!(), " compute nodes)"))?;
    let n = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::compute::store::find_node(&conn, name)
    }
    .filter(|n| n.state == "active")
    .ok_or_else(|| format!("no active compute node '{name}' ({CLI_NAME} compute nodes)"))?;
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
        return Check::Moved(format!("{}'s k2-node has no package policy yet (older than 0.46.9): update it ({CLI_NAME} compute node update {})", n.name, n.name));
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
            "That widens what {} installs as root, so it waits as node request {id}: approve it in Settings → Cores → Compute → Machine settings → Waiting for you.",
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
        duration: concat!("until someone revokes it (", cli_name!(), " compute git-access revoke)"),
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "compute", path: concat!(cli_name!(), " compute git-access (a terminal outside K2)") },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: crate::compute_git::NEEDS_PERSON },
        route_prefix: crate::compute_git::GRANT,
        method: WallMethod::Post,
        params: &[("repos", ParamFrom::Response("/error/repos"))],
    }],
    coverage: WallCoverage::Code,
    form: None,
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
    // No credential yet (0.46.10): the grant can't work, so it isn't asked
    // (409 at request time) and a card filed earlier goes stale at the
    // click; both say the exact line for the setup card.
    if !crate::compute_git::has_credential() {
        return Check::Unavailable { code: crate::compute_git::NEEDS_SETUP, why: needs_setup_why() };
    }
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

/// The grant's words while there's no credential: what to ask first.
pub(crate) fn needs_setup_why() -> String {
    format!(
        "{}. Ask for the setup first: {CLI_NAME} permission request {} --why \"<what you need it for>\"",
        crate::compute_git::NEEDS_SETUP_HINT,
        GIT_SETUP.id
    )
}

// ── compute.git-access-setup (0.46.10) ───────────────────────────────

/// Allow before the credential is set: nothing is decided.
pub(crate) const GIT_SETUP_FORM: CardForm = CardForm { id: "git-access-setup", refusal: "git_access_setup_form_first" };
pub(crate) const GIT_SETUP_FORM_FIRST: &str = GIT_SETUP_FORM.refusal;

/// `compute.git-access-setup`: this server's GitHub credential, which
/// every git-access grant needs. A person (the owner or an Admin) types it
/// into K2's own form on the card, which posts it straight to
/// `POST /cli/compute/git-access/setup` (`crate::compute_git::setup`, the
/// route `k2 compute git-access setup` uses) as that person: the key or
/// token goes into this server's vault there and never passes through the
/// card, the Thread or the agent. So the card holds no value: Allow is
/// refused until the credential is set ([`gate_git_setup`]), and a server
/// that has one is "already on". The pattern of `publish.pin` (PV-7).
pub(crate) static GIT_SETUP: ActionSpec = ActionSpec {
    id: "compute.git-access-setup",
    area: Area::Compute,
    params: &[],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::Standing,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "GitHub access for compute jobs",
        what_changes: "Set this server's GitHub credential in K2's own form: a GitHub App (its App ID and private key) or a fine-grained token",
        consequences: "It goes straight into this server's vault; {ws} never sees it. Give it Repository permissions: Contents read-only, on just the repos you'll grant (install the App on them, or pick them for the token). It reads nothing by itself: each workspace's repos are a separate grant.",
        scope: "this server",
        duration: concat!("until someone replaces or clears it (", cli_name!(), " compute git-access setup)"),
        retry: "run your command again: the grant is a second card",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "compute", path: concat!(cli_name!(), " compute git-access setup (a terminal outside K2)") },
    walls: &[
        // The agent's grant while there's no credential.
        Wall {
            matches: WallMatch::Code { status: 403, code: crate::compute_git::NEEDS_SETUP },
            route_prefix: crate::compute_git::GRANT,
            method: WallMethod::Post,
            params: &[],
        },
        // The agent's own `k2 compute git-access setup` (people only).
        Wall {
            matches: WallMatch::Code { status: 403, code: "owner_only" },
            route_prefix: crate::compute_git::SETUP,
            method: WallMethod::Post,
            params: &[],
        },
    ],
    coverage: WallCoverage::Code,
    // The credential goes into K2's own form on the card first.
    form: Some(GIT_SETUP_FORM),
    machine_hints: &[],
};

fn check_git_setup(_ctx: &Ctx) -> Check {
    if !compute_on() {
        return Check::Moved("compute nodes are off on this server".into());
    }
    if crate::compute_git::has_credential() {
        Check::AlreadyDone
    } else {
        Check::Needed
    }
}

/// Reached only past [`gate_git_setup`], and the check above turns a set
/// credential into "already on". Nothing here can set one: it never holds
/// a key or a token.
fn apply_git_setup(_ctx: &Ctx, _who: &Answerer, _token: Option<&str>) -> Applied {
    Applied::Refused { code: GIT_SETUP_FORM_FIRST.to_string(), message: git_setup_form_words() }
}

pub(crate) fn git_setup_form_words() -> String {
    concat!("Set it up first, in K2's own form on this card (K2 on your computer or the web), or from a terminal outside K2: \
     ", cli_name!(), " compute git-access setup --github-app <app id> --key-file <app.pem> (or --github-token-file <file>)")
        .to_string()
}

/// Allow on `compute.git-access-setup` before the server has a credential
/// (a client without the form: the phone, a terminal, an older build):
/// refused, nothing decided, the request stays open.
pub(crate) fn gate_git_setup(req: &k2_core::permission::request::PermissionRequest) -> Result<(), crate::cli_response::CliResponse> {
    if req.action != GIT_SETUP.id {
        return Ok(());
    }
    match check_git_setup(&super::super::present::ctx_of(req)) {
        Check::Needed => Err(super::super::answer::err(409, GIT_SETUP_FORM_FIRST, git_setup_form_words())),
        _ => Ok(()),
    }
}

/// The setup card is read on its own: nothing joins it, never "Allow all".
pub(crate) fn never_batched(action: &str) -> bool {
    action == GIT_SETUP.id
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &GIT_ACCESS, check: check_git_access, apply: apply_git_access },
        Registered { spec: &GIT_SETUP, check: check_git_setup, apply: apply_git_setup },
        Registered { spec: &ADMIN_APPROVE, check, apply },
        Registered { spec: &CI, check: check_ci, apply: apply_ci },
        Registered { spec: &CI_OFF, check: check_ci_off, apply: apply_ci_off },
        Registered { spec: &GPU, check: check_gpu, apply: apply_gpu },
        Registered { spec: &GPU_OFF, check: check_gpu_off, apply: apply_gpu_off },
        Registered { spec: &GRANT, check: check_grant, apply: apply_grant },
        Registered { spec: &ALLOW_NODES, check: check_allow_nodes, apply: apply_allow_nodes },
        Registered { spec: &ALLOW_NODES_CLEAR, check: check_allow_nodes_clear, apply: apply_allow_nodes_clear },
        Registered { spec: &ALLOW_NODES_FOR, check: check_allow_nodes_for, apply: apply_allow_nodes_for },
        Registered { spec: &CI_CACHE_TRUST, check: check_cache_trust, apply: apply_cache_trust },
        Registered { spec: &PACKAGES, check: check_packages, apply: apply_packages },
        Registered { spec: &PACKAGES_POLICY, check: check_packages_policy, apply: apply_packages_policy },
    ]
}

// ── Appendix A4: compute rows that aren't card-able in v1 ───────────

// `compute.node.pause` is E1's; `compute.grant` is a card since 0.46.17.
const IN_SETTINGS: &str = "Settings → Cores → Compute";

static NOT_CARDABLE: [NotCardableSpec; 9] = [
    NotCardableSpec { id: "compute.agents", area: Area::Compute, why: NotCardable::Covered { by: "compute.use" }, r#where: IN_SETTINGS },
    NotCardableSpec { id: "compute.node.rename", area: Area::Compute, why: NotCardable::Later, r#where: IN_SETTINGS },
    NotCardableSpec { id: "compute.node.routes", area: Area::Compute, why: NotCardable::Later, r#where: IN_SETTINGS },
    NotCardableSpec {
        id: "compute.node.enroll",
        area: Area::Compute,
        why: NotCardable::Never { reason: NeverReason::BoxLevel },
        r#where: "Settings → Cores → Compute (the owner, with the node in hand)",
    },
    NotCardableSpec {
        id: "compute.node.remove",
        area: Area::Compute,
        why: NotCardable::Never { reason: NeverReason::Irreversible },
        r#where: "Settings → Cores → Compute (the owner)",
    },
    NotCardableSpec {
        id: "compute.node.retire",
        area: Area::Compute,
        why: NotCardable::Never { reason: NeverReason::Irreversible },
        r#where: "Settings → Cores → Compute (the owner)",
    },
    NotCardableSpec {
        id: "compute.local",
        area: Area::Compute,
        why: NotCardable::Never { reason: NeverReason::BoxLevel },
        r#where: "Settings → Cores → Compute → This computer (the owner)",
    },
    NotCardableSpec {
        id: "compute.node.remote-admin",
        area: Area::Compute,
        why: NotCardable::Never { reason: NeverReason::BoxLevel },
        r#where: "the node itself: sudo k2-node admin level … at that machine",
    },
    NotCardableSpec { id: "compute.admin-deny", area: Area::Compute, why: NotCardable::Dropped, r#where: "Settings → Cores → Compute → Machine settings → Waiting for you" },
];

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    NOT_CARDABLE.iter().collect()
}

#[cfg(test)]
#[path = "compute_tests.rs"]
mod tests;
