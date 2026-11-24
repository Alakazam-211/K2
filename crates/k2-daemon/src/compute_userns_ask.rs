//! A compute job's `--userns` ask (0.46.18, `k2 compute run --userns`) on
//! the controller side, and its `--no-addr-filter` beside it. By default a
//! job can't make user namespaces (the node's job wall denies them);
//! `--userns` lets one job skip that denial (the rules are
//! [`k2_core::compute::proto::userns`]). `--no-addr-filter` (only with
//! `--userns` and `--network none`, no `--netns-dummy`) drops the job's
//! per-job address filter; its own loopback-only network namespace stays.
//!
//! - [`from_body`]: the asks from a run request (`userns: true`,
//!   `noAddrFilter: true`; the second alone, or without `--network none`,
//!   or with dummy interfaces, is `no_addr_filter_invalid`, exit 2).
//! - [`userns_skip`]: why a node can't take them, the one check placement,
//!   a named node and every assign use. In order: the node must be one
//!   this server owns ([`owns`]; never a node where it is a tenant,
//!   `userns_owner_only`), offer [`FEATURE_JOB_USERNS`] (and
//!   [`FEATURE_NO_ADDR_FILTER`] for that ask: `userns_needs_update`), and
//!   the job's workspace must hold a grant there with `allow_userns` (and
//!   `allow_no_addr_filter`: `userns_not_granted`). No owner or Admin
//!   bypass: the grant row is the node owner's explicit yes for that
//!   workspace (the owner or an Admin sets it with `k2 compute grant <node>
//!   --workspace <ws> --userns [--no-addr-filter]`).
//! - [`named_refusal`] and [`auto_refusal`]: the run route's refusals for a
//!   named node and for auto-placement with no node left (exit 3). Never a
//!   job placed where it can't have its ask, and never `Plan.userns` or
//!   `Plan.no_addr_filter` sent to any other node ([`assign_wait`] checks
//!   again at every assign).

use k2_core::brand::{CLI_NAME, PRODUCT_NAME};
use k2_core::compute::proto::frames;
use k2_core::compute::proto::jobnet::{JobNet, JobNetMode};
use k2_core::compute::proto::userns::{codes, FEATURE_JOB_USERNS, FEATURE_NO_ADDR_FILTER};
use k2_core::compute::store::{self, GrantRow, NodeRow};

use crate::compute_place::{skip, CandReport, Want};
use crate::compute_routes::{no, Refusal, R};

/// The k2-node a `--userns` job needs on its node (its first release with
/// [`FEATURE_JOB_USERNS`]), as every hint says it.
pub(crate) const NODE_VERSION: &str = "0.46.18";

/// `no_addr_filter_invalid`'s words.
pub const NO_ADDR_FILTER_RULE: &str = concat!(
    "--no-addr-filter goes only with --userns and --network none, never with --netns-dummy: ",
    k2_core::cli_name!(),
    " compute run --userns --network none --no-addr-filter -- <command> (the job's own network namespace, loopback only, stays the wall)"
);

fn flag(v: &serde_json::Value, key: &str, what: &str) -> R<Option<bool>> {
    match v.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(no(
            k2_core::compute::codes::BAD_REQUEST,
            400,
            format!("{key} is true or false ({what})"),
        )),
    }
}

/// The asks from a run request: `(userns, no_addr_filter)`. Absent, null
/// or false: no ask (the job wall's defaults). `noAddrFilter` needs
/// `userns` and `network` none with no dummy interfaces (`network` is the
/// job's own network ask, already read).
pub(crate) fn from_body(v: &serde_json::Value, network: Option<&JobNet>) -> R<(bool, bool)> {
    let userns = flag(v, "userns", "--userns")?.unwrap_or(false);
    let naf = flag(v, "noAddrFilter", "--no-addr-filter")?.unwrap_or(false);
    let lo_only = network.is_some_and(|n| n.mode == JobNetMode::None && n.dummies.is_empty());
    if naf && !(userns && lo_only) {
        return Err(no(codes::NO_ADDR_FILTER_INVALID, 400, NO_ADDR_FILTER_RULE));
    }
    Ok((userns, naf))
}

/// `allowUserns` and `allowNoAddrFilter` from a grant request (`k2 compute
/// grant <node> --workspace <ws> --userns|--no-userns
/// --no-addr-filter|--addr-filter`): `None` keeps what the grant has. Only
/// the owner or an Admin login (`admin`, with no compute manager `mgr`)
/// turns either on or off, never an agent, not even one that may manage
/// compute nodes (403 `userns_not_granted`). Either is turned on only for
/// a node this server owns: never where this server is a tenant (403
/// `userns_owner_only`). `--no-addr-filter` is never granted without
/// `--userns` (asked now, or already on the grant of `ws_id` there): 400
/// `no_addr_filter_invalid`. Turning them off is always allowed (and
/// `--no-userns` takes `--no-addr-filter` too: the store).
pub(crate) fn grant_ask(
    v: &serde_json::Value,
    admin: bool,
    mgr: Option<&crate::compute_manage::Manager>,
    n: &NodeRow,
    ws_id: &str,
    ws: &str,
) -> R<(Option<bool>, Option<bool>)> {
    let userns = flag(v, "allowUserns", "--userns|--no-userns")?;
    let naf = flag(v, "allowNoAddrFilter", "--no-addr-filter|--addr-filter")?;
    if userns.is_none() && naf.is_none() {
        return Ok((None, None));
    }
    let what = if userns.is_some() { "--userns" } else { "--no-addr-filter" };
    if mgr.is_some() || !admin {
        if let Some(m) = mgr {
            crate::compute_manage::audit(m, "grant_userns_refused", &n.name, &[("for", ws)]);
        }
        return Err(no(
            codes::USERNS_NOT_GRANTED,
            403,
            format!(
                "only the node's owner or an Admin turns {what} on or off for a grant: the owner runs {CLI_NAME} compute grant {} --workspace {ws} {what}",
                n.name
            ),
        )
        .with("node", n.name.clone()));
    }
    if (userns == Some(true) || naf == Some(true)) && !owns(n) {
        return Err(no(codes::USERNS_OWNER_ONLY, 403, owner_only_hint(&n.name)).with("node", n.name.clone()));
    }
    if naf == Some(true) && userns != Some(true) {
        let has = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::grant(&conn, &n.id, ws_id).is_some_and(|g| g.allow_userns)
        };
        if userns == Some(false) || !has {
            return Err(no(
                codes::NO_ADDR_FILTER_INVALID,
                400,
                format!(
                    "--no-addr-filter is granted only with --userns: {CLI_NAME} compute grant {} --workspace {ws} --userns --no-addr-filter",
                    n.name
                ),
            ));
        }
    }
    Ok((userns, naf))
}

/// `--plan`'s view of the job's own asks: `network` (0.46.15), `userns:
/// true` and `noAddrFilter: true` (0.46.18), each only when asked.
pub(crate) fn plan_view(out: &mut serde_json::Value, w: &Want) {
    if let Some(net) = w.network.as_ref() {
        out["network"] = crate::compute_views::network_json(net);
    }
    if w.userns {
        out["userns"] = true.into();
    }
    if w.no_addr_filter {
        out["noAddrFilter"] = true.into();
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

/// The node's k2-node runs them with `--no-addr-filter` too.
pub fn offers_no_addr_filter(features: &[String]) -> bool {
    offers(features) && features.iter().any(|f| f == FEATURE_NO_ADDR_FILTER)
}

/// Why a node can't take `w`'s `--userns` (and `--no-addr-filter`) ask, or
/// `None` (no ask, or it can). `owned`: [`owns`] for that node; `features`:
/// its offer's (`None`: never seen online, so it can't say).
pub fn userns_skip(
    w: &Want,
    owned: bool,
    grant: Option<&GrantRow>,
    features: Option<&[String]>,
) -> Option<&'static str> {
    if !w.userns && !w.no_addr_filter {
        return None;
    }
    if !owned {
        return Some(skip::USERNS_OWNER_ONLY);
    }
    let runs = |f: &[String]| if w.no_addr_filter { offers_no_addr_filter(f) } else { offers(f) };
    if !features.is_some_and(runs) {
        return Some(skip::USERNS_NEEDS_UPDATE);
    }
    if !grant.is_some_and(|g| g.allow_userns && (!w.no_addr_filter || g.allow_no_addr_filter)) {
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

/// `userns_not_granted`'s words for one node (`naf`: the job also asks
/// `--no-addr-filter`).
pub fn not_granted_hint(node: &str, ws: &str, naf: bool) -> String {
    if naf {
        return format!(
            "--no-addr-filter: workspace {ws} isn't granted --userns with --no-addr-filter on '{node}'. The node's owner or an Admin grants it: {CLI_NAME} compute grant {node} --workspace {ws} --userns --no-addr-filter"
        );
    }
    format!(
        "--userns: workspace {ws} isn't granted user namespaces on '{node}' (by default a job can't make them). The node's owner or an Admin grants it: {CLI_NAME} compute grant {node} --workspace {ws} --userns"
    )
}

/// `userns_needs_update`'s words (a named node, or every node this
/// workspace may use; `naf`: the job also asks `--no-addr-filter`).
pub fn needs_update_hint(node: Option<&str>, naf: bool) -> String {
    let which = node
        .map(|n| format!("'{n}' can't"))
        .unwrap_or_else(|| "no node this workspace may use can".into());
    let (flag, with) = if naf { ("--no-addr-filter", " without its address filter") } else { ("--userns", "") };
    format!(
        "{flag}: {which} run a job that makes user namespaces{with} yet: that needs k2-node {NODE_VERSION} there (Linux, with its job wall). Its owner updates k2-node. {PRODUCT_NAME} never sends such a job to a node that can't run it"
    )
}

/// A named (or `--same-node-as`) node that can't take the ask: `code` is
/// [`userns_skip`]'s. Exit 3.
pub(crate) fn named_refusal(code: &str, node: &str, ws: &str, naf: bool) -> Refusal {
    let r = match code {
        skip::USERNS_OWNER_ONLY => no(codes::USERNS_OWNER_ONLY, 403, owner_only_hint(node)),
        skip::USERNS_NOT_GRANTED => no(codes::USERNS_NOT_GRANTED, 403, not_granted_hint(node, ws, naf)),
        _ => no(codes::USERNS_NEEDS_UPDATE, 409, needs_update_hint(Some(node), naf)),
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
    if !(w.userns || w.no_addr_filter) || !report.cands.is_empty() {
        return None;
    }
    let naf = w.no_addr_filter;
    if let Some(node) = report.userns_not_granted.first() {
        return Some((
            codes::USERNS_NOT_GRANTED,
            403,
            not_granted_hint(node, ws_handle, naf),
            Some(node.clone()),
        ));
    }
    if report.skipped.get(skip::USERNS_NEEDS_UPDATE).copied().unwrap_or(0) > 0 {
        return Some((codes::USERNS_NEEDS_UPDATE, 409, needs_update_hint(None, naf), None));
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
/// offers [`FEATURE_JOB_USERNS`] right now (and [`FEATURE_NO_ADDR_FILTER`]
/// for a `--no-addr-filter` plan), where its workspace's grant still says
/// so. Otherwise it waits on this server with the reason (a node rolled
/// back, a grant turned off since submit); never sent. `None`: no ask, or
/// it may go.
pub(crate) fn assign_wait(
    conn: &rusqlite::Connection,
    plan: &frames::Plan,
    node_id: &str,
    workspace_id: &str,
    features: &[String],
) -> Option<frames::Refusal> {
    if !plan.userns && !plan.no_addr_filter {
        return None;
    }
    let naf = plan.no_addr_filter;
    let node = store::node_by_id(conn, node_id);
    let name = node.as_ref().map(|n| n.name.clone()).unwrap_or_else(|| node_id.to_string());
    if !node.as_ref().is_some_and(owns) {
        return Some(frames::Refusal::new(codes::USERNS_OWNER_ONLY, owner_only_hint(&name)));
    }
    if !(if naf { offers_no_addr_filter(features) } else { offers(features) }) {
        return Some(frames::Refusal::new(codes::USERNS_NEEDS_UPDATE, needs_update_hint(Some(&name), naf)));
    }
    let granted = store::grant(conn, node_id, workspace_id).is_some_and(|g| g.allow_userns && (!naf || g.allow_no_addr_filter));
    if !granted {
        let (what, flags) = if naf { ("--userns with --no-addr-filter", "--userns --no-addr-filter") } else { ("--userns", "--userns") };
        return Some(frames::Refusal::new(
            codes::USERNS_NOT_GRANTED,
            format!(
                "{what}: this workspace's grant on '{name}' no longer allows it; the node's owner or an Admin grants it again with {CLI_NAME} compute grant {name} --workspace <ws> {flags}"
            ),
        ));
    }
    None
}
