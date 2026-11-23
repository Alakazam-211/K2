//! A compute job's `--userns` ask (0.46.18, `k2 compute run --userns`) on
//! the controller side. By default a job can't make user namespaces (the
//! node's job wall denies them); `--userns` lets one job skip that denial
//! (the rules are [`k2_core::compute::proto::userns`]).
//!
//! - [`from_body`]: the ask from a run request (`userns: true`).
//! - [`userns_skip`]: why a node can't take it, the one check placement,
//!   a named node and every assign use. In order: the node must be one
//!   this server owns ([`owns`]; never a node where it is a tenant,
//!   `userns_owner_only`), offer [`FEATURE_JOB_USERNS`]
//!   (`userns_needs_update`), and the job's workspace must hold a grant
//!   there with `allow_userns` (`userns_not_granted`). No owner or Admin
//!   bypass: the grant row is the node owner's explicit yes for that
//!   workspace (the owner or an Admin sets it with `k2 compute grant <node>
//!   --workspace <ws> --userns`).
//! - [`named_refusal`] and [`auto_refusal`]: the run route's refusals for a
//!   named node and for auto-placement with no node left (exit 3). Never a
//!   `--userns` job placed where it can't have it, and never `Plan.userns`
//!   sent to any other node ([`assign_wait`] checks again at every assign).

use k2_core::brand::PRODUCT_NAME;
use k2_core::compute::proto::frames;
use k2_core::compute::proto::userns::{codes, FEATURE_JOB_USERNS};
use k2_core::compute::store::{self, GrantRow, NodeRow};

use crate::compute_place::{skip, CandReport, Want};
use crate::compute_routes::{no, Refusal, R};

/// The k2-node a `--userns` job needs on its node (its first release with
/// [`FEATURE_JOB_USERNS`]), as every hint says it.
pub(crate) const NODE_VERSION: &str = "0.46.18";

/// The ask from a run request: `userns: true` (`--userns`). Absent, null
/// or false: no ask (the job wall's default denial).
pub(crate) fn from_body(v: &serde_json::Value) -> R<bool> {
    match v.get("userns") {
        None | Some(serde_json::Value::Null) => Ok(false),
        Some(serde_json::Value::Bool(b)) => Ok(*b),
        Some(_) => Err(no(
            k2_core::compute::codes::BAD_REQUEST,
            400,
            "userns is true or false (--userns)",
        )),
    }
}

/// `allowUserns` from a grant request (`k2 compute grant <node> --workspace
/// <ws> --userns|--no-userns`): `None` keeps what the grant has. Only the
/// owner or an Admin login (`admin`, with no compute manager `mgr`) turns
/// it on or off, never an agent, not even one that may manage compute
/// nodes (403 `userns_not_granted`). It is turned on only for a node this
/// server owns: never where this server is a tenant (403
/// `userns_owner_only`). Turning it off is always allowed.
pub(crate) fn grant_ask(
    v: &serde_json::Value,
    admin: bool,
    mgr: Option<&crate::compute_manage::Manager>,
    n: &NodeRow,
    ws: &str,
) -> R<Option<bool>> {
    let asked = match v.get("allowUserns") {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(x) => x.as_bool().ok_or_else(|| {
            no(k2_core::compute::codes::BAD_REQUEST, 400, "allowUserns is true or false")
        })?,
    };
    if mgr.is_some() || !admin {
        if let Some(m) = mgr {
            crate::compute_manage::audit(m, "grant_userns_refused", &n.name, &[("for", ws)]);
        }
        return Err(no(
            codes::USERNS_NOT_GRANTED,
            403,
            format!(
                "only the node's owner or an Admin turns --userns on or off for a grant: the owner runs k2 compute grant {} --workspace {ws} --userns",
                n.name
            ),
        )
        .with("node", n.name.clone()));
    }
    if asked && !owns(n) {
        return Err(no(codes::USERNS_OWNER_ONLY, 403, owner_only_hint(&n.name)).with("node", n.name.clone()));
    }
    Ok(Some(asked))
}

/// `--plan`'s view of the job's own asks: `network` (0.46.15) and
/// `userns: true` (0.46.18), each only when asked.
pub(crate) fn plan_view(out: &mut serde_json::Value, w: &Want) {
    if let Some(net) = w.network.as_ref() {
        out["network"] = crate::compute_views::network_json(net);
    }
    if w.userns {
        out["userns"] = true.into();
    }
}

/// This server owns `n` for `--userns`: never a tenant (sticky, from
/// enrolment) and never a node with another server's owner label. Nothing
/// the node's offer says can make this true.
pub fn owns(n: &NodeRow) -> bool {
    n.owned_for_secrets()
}

/// The node's k2-node runs `--userns` jobs.
pub fn offers(features: &[String]) -> bool {
    features.iter().any(|f| f == FEATURE_JOB_USERNS)
}

/// Why a node can't take `w`'s `--userns` ask, or `None` (no ask, or it
/// can). `owned`: [`owns`] for that node; `features`: its offer's (`None`:
/// never seen online, so it can't say).
pub fn userns_skip(
    w: &Want,
    owned: bool,
    grant: Option<&GrantRow>,
    features: Option<&[String]>,
) -> Option<&'static str> {
    if !w.userns {
        return None;
    }
    if !owned {
        return Some(skip::USERNS_OWNER_ONLY);
    }
    if !features.is_some_and(offers) {
        return Some(skip::USERNS_NEEDS_UPDATE);
    }
    if !grant.is_some_and(|g| g.allow_userns) {
        return Some(skip::USERNS_NOT_GRANTED);
    }
    None
}

/// `userns_owner_only`'s words for one node.
pub fn owner_only_hint(node: &str) -> String {
    format!(
        "--userns is only for the node owner's own server: this server is a tenant of {node}, so its jobs can't make user namespaces there. Run it without --userns, or on a node this server owns"
    )
}

/// `userns_not_granted`'s words for one node.
pub fn not_granted_hint(node: &str, ws: &str) -> String {
    format!(
        "--userns: workspace {ws} isn't granted user namespaces on '{node}' (by default a job can't make them). The node's owner or an Admin grants it: k2 compute grant {node} --workspace {ws} --userns"
    )
}

/// `userns_needs_update`'s words (a named node, or every node this
/// workspace may use).
pub fn needs_update_hint(node: Option<&str>) -> String {
    let which = node
        .map(|n| format!("'{n}' can't"))
        .unwrap_or_else(|| "no node this workspace may use can".into());
    format!(
        "--userns: {which} run a job that makes user namespaces yet: that needs k2-node {NODE_VERSION} there (Linux, with its job wall). Its owner updates k2-node. {PRODUCT_NAME} never sends a --userns job to a node that can't run it"
    )
}

/// A named (or `--same-node-as`) node that can't take the ask: `code` is
/// [`userns_skip`]'s. Exit 3.
pub(crate) fn named_refusal(code: &str, node: &str, ws: &str) -> Refusal {
    let r = match code {
        skip::USERNS_OWNER_ONLY => no(codes::USERNS_OWNER_ONLY, 403, owner_only_hint(node)),
        skip::USERNS_NOT_GRANTED => no(codes::USERNS_NOT_GRANTED, 403, not_granted_hint(node, ws)),
        _ => no(codes::USERNS_NEEDS_UPDATE, 409, needs_update_hint(Some(node))),
    };
    r.with("node", node.to_string())
}

/// One of [`userns_skip`]'s codes.
pub fn is_userns_skip(code: &str) -> bool {
    matches!(
        code,
        skip::USERNS_OWNER_ONLY | skip::USERNS_NOT_GRANTED | skip::USERNS_NEEDS_UPDATE
    )
}

/// Auto-placement with no node left, some only for the `--userns` ask:
/// `userns_not_granted` naming a node that could run it with the grant
/// first (the one fix an owner can make), then `userns_needs_update`, then
/// `userns_owner_only` naming a node where this server is a tenant.
/// `(code, status, hint, node)`; `None`: no ask, nodes are left, or none
/// was left out for it.
pub(crate) fn auto_refusal(
    w: &Want,
    report: &CandReport,
    ws_handle: &str,
) -> Option<(&'static str, u16, String, Option<String>)> {
    if !w.userns || !report.cands.is_empty() {
        return None;
    }
    if let Some(node) = report.userns_not_granted.first() {
        return Some((
            codes::USERNS_NOT_GRANTED,
            403,
            not_granted_hint(node, ws_handle),
            Some(node.clone()),
        ));
    }
    if report.skipped.get(skip::USERNS_NEEDS_UPDATE).copied().unwrap_or(0) > 0 {
        return Some((codes::USERNS_NEEDS_UPDATE, 409, needs_update_hint(None), None));
    }
    if let Some(node) = report.userns_owner_only.first() {
        return Some((
            codes::USERNS_OWNER_ONLY,
            403,
            owner_only_hint(node),
            Some(node.clone()),
        ));
    }
    None
}

/// At assign: a `--userns` plan goes only to a node this server owns, that
/// offers [`FEATURE_JOB_USERNS`] right now, where its workspace's grant
/// still says `--userns`. Otherwise it waits on this server with the
/// reason (a node rolled back, a grant turned off since submit); never
/// sent. `None`: no ask, or it may go.
pub(crate) fn assign_wait(
    conn: &rusqlite::Connection,
    plan: &frames::Plan,
    node_id: &str,
    workspace_id: &str,
    features: &[String],
) -> Option<frames::Refusal> {
    if !plan.userns {
        return None;
    }
    let node = store::node_by_id(conn, node_id);
    let name = node.as_ref().map(|n| n.name.clone()).unwrap_or_else(|| node_id.to_string());
    if !node.as_ref().is_some_and(owns) {
        return Some(frames::Refusal::new(codes::USERNS_OWNER_ONLY, owner_only_hint(&name)));
    }
    if !offers(features) {
        return Some(frames::Refusal::new(codes::USERNS_NEEDS_UPDATE, needs_update_hint(Some(&name))));
    }
    if !store::grant(conn, node_id, workspace_id).is_some_and(|g| g.allow_userns) {
        return Some(frames::Refusal::new(
            codes::USERNS_NOT_GRANTED,
            format!(
                "--userns: this workspace's grant on '{name}' no longer allows user namespaces; the node's owner or an Admin grants it again with k2 compute grant {name} --workspace <ws> --userns"
            ),
        ));
    }
    None
}
