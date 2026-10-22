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
    words: Words {
        setting: "Compute approvals",
        what_changes: "Run {ws}'s node admin request {request} as root on its node",
        consequences: "K2 signs the step with the owner's approval key and sends it to the node, which runs it as root. Check what it does in Settings → Compute → Approvals first.",
        scope: "node request {request}",
        duration: "once",
        retry: "",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "compute", path: "Settings → Compute → Approvals" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "owner_only" },
        route_prefix: crate::compute_admin::APPROVE,
        method: WallMethod::Post,
        params: &[("request", ParamFrom::Body("/request"))],
    }],
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

/// The current state of the node admin request.
fn check(ctx: &Ctx) -> Check {
    if !compute_on() {
        return Check::Moved("compute nodes are off on this server".into());
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

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &ADMIN_APPROVE, check, apply }]
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
