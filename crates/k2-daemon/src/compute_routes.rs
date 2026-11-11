//! `/cli/compute/*` HTTP routes (`prd-k2-compute-nodes-v1` §15.1).
//!
//! Who may do what (CN18):
//! - Route floors (`route_policy.rs`) stop Connect logins below the role.
//! - Agent verbs (`nodes`, `nodes/info`, `run`, `jobs*`, `grants` GET,
//!   `local/status`) are in `session_token::is_agent_verb`; a passport
//!   there needs the workspace switch "Allow compute nodes"
//!   (`compute_off`) and a grant on the node (`not_granted`), and only ever
//!   sees its own workspace.
//! - Owner/admin verbs refuse every passport with `owner_only`, including
//!   a K2 shell tab (a human typing inside K2 sends the tab's passport):
//!   the teaching text says to use a terminal outside K2.
//! - "Allow managing compute nodes" (`compute_manage`, 0194,
//!   [`crate::compute_manage`]): a local agent passport whose workspace has
//!   the switch may also run node add/confirm/rename/pause/drain/resume/
//!   retire/route and grant/revoke (any workspace), audited. Preview
//!   on/off, node remove and the switch itself stay the owner's; a passport
//!   from off the loopback listener never gets it (`owner_only` as before).
//!
//! The TCP dispatcher and the per-cell socket both build a [`Who`] and
//! call [`handle`] (blocking: DB, git, the log long-poll).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use k2_core::compute::proto::cache as pcache;
use k2_core::compute::proto::frames::{Blob, Control, JobLimits, JobState, Plan, Src};
use k2_core::compute::proto::share as pshare;
use k2_core::compute::proto::tiers::{self, Tier};
use k2_core::compute::proto::{crypto, env as penv, needs, pairing, results};
use k2_core::compute::share::{self, pending};
use k2_core::compute::{self as compute, codes, logs, scheduler, store};
use k2_core::connect_users::Role;

use crate::cli_response::CliResponse;
use crate::compute_manage::Manager;
use crate::compute_views::{self as views, cancel_one, granted_node_ids, job_summary, node_visible, queue_view};
use crate::compute_ws;

/// Largest request body.
pub const MAX_BODY: usize = 1 << 20;

/// Every compute route (the two sockets included).
pub const ROUTES: &[&str] = &[
    // `k2 ci` (0.46.10, C6a, `ci_routes`): CI on compute, same callers.
    crate::ci_routes::EVIDENCE,
    crate::ci_routes::FREEZE,
    crate::ci_routes::INIT,
    crate::ci_routes::RELEASE,
    crate::ci_routes::RUN,
    crate::ci_routes::STATUS,
    crate::ci_routes::WATCH,
    "/cli/compute/admin/approval-key",
    "/cli/compute/admin/approve",
    "/cli/compute/admin/deny",
    "/cli/compute/admin/propose",
    "/cli/compute/admin/requests",
    "/cli/compute/admin/withdraw",
    "/cli/compute/agents",
    "/cli/compute/agents/set",
    // A workspace's node list (0.46.8, `compute_allow`).
    crate::compute_allow::SHOW,
    crate::compute_allow::SET,
    compute_ws::ATTACH_PATH,
    "/cli/compute/cache",
    "/cli/compute/cache/clear",
    compute_ws::ENROLL_PATH,
    "/cli/compute/events",
    // Git access for jobs (0.46.9, GH#78, `compute_git`).
    crate::compute_git::SHOW,
    crate::compute_git::GRANT,
    crate::compute_git::REVOKE,
    crate::compute_git::SETUP,
    "/cli/compute/grants",
    "/cli/compute/grants/revoke",
    "/cli/compute/grants/set",
    "/cli/compute/jobs",
    "/cli/compute/jobs/artifact",
    "/cli/compute/jobs/cancel",
    "/cli/compute/jobs/get",
    "/cli/compute/jobs/logs",
    crate::compute_move::MOVE,
    "/cli/compute/jobs/receipt",
    "/cli/compute/jobs/retry",
    "/cli/compute/jobs/tier",
    "/cli/compute/local/disable",
    "/cli/compute/local/drain",
    "/cli/compute/local/enable",
    "/cli/compute/local/pause",
    "/cli/compute/local/policy",
    "/cli/compute/local/remote-admin",
    "/cli/compute/local/resume",
    "/cli/compute/local/status",
    "/cli/compute/nodes",
    "/cli/compute/nodes/alerts",
    "/cli/compute/nodes/confirm",
    "/cli/compute/nodes/enroll-code",
    "/cli/compute/nodes/health",
    "/cli/compute/nodes/info",
    crate::compute_pkg::STATUS,
    crate::compute_pkg::DECIDE,
    crate::compute_pkg::POLICY,
    crate::compute_pkg::REQUEST,
    crate::compute_pkg::REQUESTS,
    "/cli/compute/nodes/pause",
    "/cli/compute/nodes/policy",
    "/cli/compute/nodes/policy/set",
    "/cli/compute/nodes/remove",
    "/cli/compute/nodes/rename",
    "/cli/compute/nodes/retire",
    "/cli/compute/nodes/rollback",
    "/cli/compute/nodes/rollout",
    "/cli/compute/nodes/rollout/pause",
    "/cli/compute/nodes/rollout/resume",
    "/cli/compute/nodes/rollout/stop",
    "/cli/compute/nodes/routes",
    "/cli/compute/nodes/servers",
    "/cli/compute/nodes/servers/add",
    "/cli/compute/nodes/servers/limits",
    "/cli/compute/nodes/servers/owner",
    "/cli/compute/nodes/servers/pause",
    "/cli/compute/nodes/servers/revoke",
    "/cli/compute/nodes/take-owner",
    "/cli/compute/nodes/update",
    "/cli/compute/nodes/wall",
    "/cli/compute/prewarm",
    "/cli/compute/queue",
    "/cli/compute/run",
    "/cli/compute/usage",
];

/// POST-only rows (`require_post` → 405 on GET).
pub const POST_ROUTES: &[&str] = &[
    crate::ci_routes::EVIDENCE,
    crate::ci_routes::FREEZE,
    crate::ci_routes::INIT,
    crate::ci_routes::RELEASE,
    crate::ci_routes::RUN,
    crate::ci_routes::WATCH,
    "/cli/compute/admin/approve",
    "/cli/compute/admin/deny",
    "/cli/compute/admin/propose",
    "/cli/compute/admin/withdraw",
    "/cli/compute/agents/set",
    crate::compute_allow::SET,
    "/cli/compute/cache/clear",
    crate::compute_git::GRANT,
    crate::compute_git::REVOKE,
    crate::compute_git::SETUP,
    "/cli/compute/grants/revoke",
    "/cli/compute/grants/set",
    "/cli/compute/jobs/cancel",
    crate::compute_move::MOVE,
    "/cli/compute/jobs/retry",
    "/cli/compute/jobs/tier",
    "/cli/compute/local/disable",
    "/cli/compute/local/drain",
    "/cli/compute/local/enable",
    "/cli/compute/local/pause",
    "/cli/compute/local/policy",
    "/cli/compute/local/remote-admin",
    "/cli/compute/local/resume",
    "/cli/compute/nodes/alerts",
    "/cli/compute/nodes/confirm",
    "/cli/compute/nodes/enroll-code",
    crate::compute_pkg::DECIDE,
    crate::compute_pkg::POLICY,
    crate::compute_pkg::REQUEST,
    "/cli/compute/nodes/pause",
    "/cli/compute/nodes/policy/set",
    "/cli/compute/nodes/remove",
    "/cli/compute/nodes/rename",
    "/cli/compute/nodes/retire",
    "/cli/compute/nodes/rollback",
    "/cli/compute/nodes/rollout/pause",
    "/cli/compute/nodes/rollout/resume",
    "/cli/compute/nodes/rollout/stop",
    "/cli/compute/nodes/routes",
    "/cli/compute/nodes/servers/add",
    "/cli/compute/nodes/servers/limits",
    "/cli/compute/nodes/servers/owner",
    "/cli/compute/nodes/servers/pause",
    "/cli/compute/nodes/servers/revoke",
    "/cli/compute/nodes/take-owner",
    "/cli/compute/nodes/update",
    "/cli/compute/nodes/wall",
    "/cli/compute/prewarm",
    "/cli/compute/run",
];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p) || crate::compute_trust::is_route(p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p) || crate::compute_trust::POST_ROUTES.contains(&p)
}

pub fn is_socket(p: &str) -> bool {
    p == compute_ws::ATTACH_PATH || p == compute_ws::ENROLL_PATH
}

// ── who is calling ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    /// The owner daemon token (a terminal outside K2, a script).
    Owner,
    /// A Connect login (its floor already cleared `role_gate`).
    Login { username: String, role: Role },
    /// A harness session's passport.
    Agent { project_id: String, session_id: String },
    /// A plain K2 shell tab's passport (a human typing inside K2).
    Shell { project_id: String, session_id: String },
    /// App pass, API key, API session.
    Wrong,
    Invalid,
}

impl Who {
    pub(crate) fn label(&self) -> String {
        match self {
            Who::Owner => "owner".into(),
            Who::Login { username, .. } => format!("login:{username}"),
            Who::Agent { project_id, .. } => format!("agent:{}", ws_handle(project_id)),
            Who::Shell { project_id, .. } => format!("shell:{}", ws_handle(project_id)),
            Who::Wrong => "wrong-credential".into(),
            Who::Invalid => "-".into(),
        }
    }
    pub(crate) fn passport(&self) -> Option<(&str, &str)> {
        match self {
            Who::Agent { project_id, session_id } | Who::Shell { project_id, session_id } => {
                Some((project_id.as_str(), session_id.as_str()))
            }
            _ => None,
        }
    }
    pub(crate) fn is_owner(&self) -> bool {
        matches!(self, Who::Owner | Who::Login { role: Role::Owner, .. })
    }
    pub(crate) fn is_admin(&self) -> bool {
        matches!(self, Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. })
    }
}

fn from_session(session_id: &str, principal: &crate::session_token::HookPrincipal) -> Who {
    match crate::sidecar_routes::caller_from_session(session_id, principal) {
        crate::sidecar_routes::Caller::Agent(a) => Who::Agent { project_id: a.project_id, session_id: session_id.to_string() },
        crate::sidecar_routes::Caller::Shell { project_id } => Who::Shell { project_id, session_id: session_id.to_string() },
        _ => Who::Wrong,
    }
}

/// Classify a TCP caller (the dispatcher's effective auth query).
pub fn who_from_tcp(query: &str, bearer: Option<&str>, owner_token: &str) -> Who {
    let presented = crate::routes::http::extract_token(query)
        .filter(|s| !s.is_empty())
        .or_else(|| bearer.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("");
    if presented.is_empty() {
        return Who::Invalid;
    }
    if k2_core::skin::is_skin_token(presented) || presented.starts_with("k2sk_") {
        return Who::Wrong;
    }
    if crate::routes::http::ct_eq_token(presented, owner_token) {
        return Who::Owner;
    }
    if let Some(username) = k2_core::connect_users::validate_session(presented) {
        // V10: an unreadable role fails closed to the lowest.
        let role = k2_core::connect_users::role_for_user(&username).unwrap_or(Role::VoiceChat);
        return Who::Login { username, role };
    }
    // Any valid passport is classified (even on an owner route) so the
    // refusal can teach `owner_only` instead of a bare 403.
    if let Some(v) = crate::session_token::validate_hook(presented) {
        return from_session(&v.session_id, &v.principal);
    }
    Who::Invalid
}

/// Classify a per-cell socket caller.
pub fn who_from_cell(session_id: &str, principal: &crate::session_token::HookPrincipal) -> Who {
    from_session(session_id, principal)
}

// ── responses ────────────────────────────────────────────────────────

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        409 => "409 Conflict",
        410 => "410 Gone",
        413 => "413 Payload Too Large",
        502 => "502 Bad Gateway",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

#[derive(Debug)]
pub(crate) struct Refusal {
    pub(crate) code: &'static str,
    pub(crate) status: u16,
    pub(crate) hint: String,
    /// More fields of the `error` object (after `code` and `hint`), for a
    /// refusal the permission-card walls read values from
    /// (`node_not_allowed`'s `nodes`).
    pub(crate) extra: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Refusal {
    /// Add one field to the `error` object.
    pub(crate) fn with(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.extra.get_or_insert_with(serde_json::Map::new).insert(key.to_string(), value.into());
        self
    }
}

pub(crate) type R<T> = Result<T, Refusal>;

pub(crate) fn no(code: &'static str, status: u16, hint: impl Into<String>) -> Refusal {
    Refusal { code, status, hint: hint.into(), extra: None }
}

pub(crate) fn refusal_response(r: &Refusal) -> CliResponse {
    let mut error = serde_json::json!({ "code": r.code, "hint": r.hint });
    if let (Some(extra), Some(e)) = (r.extra.as_ref(), error.as_object_mut()) {
        for (k, v) in extra {
            e.insert(k.clone(), v.clone());
        }
    }
    CliResponse {
        status: status_line(r.status),
        content_type: "application/json",
        body: serde_json::json!({ "error": error }).to_string(),
    }
}

pub fn too_large() -> CliResponse {
    refusal_response(&no(codes::BAD_REQUEST, 413, format!("request bodies here are at most {} KiB", MAX_BODY / 1024)))
}

/// CI on compute (0.46.10 ships it dark): the refusal every CI surface
/// answers while [`compute::ci_enabled`] is off (409 `ci_preview_off`,
/// with how the owner turns it on).
pub(crate) fn ci_preview_off(what: &str) -> Refusal {
    no(compute::CI_PREVIEW_OFF, 409, compute::ci_preview_hint(what))
}

/// `Ok` when CI on compute is on, else [`ci_preview_off`].
pub(crate) fn ci_gate(what: &str) -> R<()> {
    if compute::ci_enabled() {
        Ok(())
    } else {
        Err(ci_preview_off(what))
    }
}

/// The routes CI on compute added on the compute router (0.46.10): `k2 ci`
/// (`ci_routes`) and node rosters, lanes, seats, owners, trust requests and
/// signer pairing (`compute_trust`). All of them answer [`ci_preview_off`]
/// while the CI preview is off; the federation leg
/// (`/cli/federation/compute-seat`) does too, in its own handler.
pub fn is_ci_route(p: &str) -> bool {
    crate::ci_routes::is_route(p) || crate::compute_trust::is_route(p)
}

/// 404 for every compute route while the preview flag is off (CN30).
pub fn dark() -> CliResponse {
    CliResponse {
        status: "404 Not Found",
        content_type: "application/json",
        body: r#"{"error":{"code":"compute_disabled","hint":"compute nodes are a preview: the owner turns them on (k2 compute preview on, or K2_COMPUTE=1)"}}"#
            .to_string(),
    }
}

pub(crate) fn owner_only() -> Refusal {
    no(
        codes::OWNER_ONLY,
        403,
        "only the server's owner (or an admin, where noted) can do this, and not through an agent or a K2 terminal \
         (those send the tab's passport). Use Settings or a terminal outside K2.",
    )
}

pub(crate) fn need_admin(who: &Who) -> R<()> {
    if who.is_admin() {
        Ok(())
    } else if matches!(who, Who::Invalid) {
        Err(no("forbidden", 403, "invalid or missing token"))
    } else {
        Err(owner_only())
    }
}

pub(crate) fn need_owner(who: &Who) -> R<()> {
    if who.is_owner() {
        Ok(())
    } else if matches!(who, Who::Invalid) {
        Err(no("forbidden", 403, "invalid or missing token"))
    } else {
        Err(owner_only())
    }
}

/// The owner, or a local agent under "Allow managing compute nodes". With
/// no manager this is exactly [`need_owner`] (same refusal bytes).
fn owner_or_manager(who: &Who, mgr: Option<&Manager>) -> R<()> {
    if mgr.is_some() {
        return Ok(());
    }
    need_owner(who)
}

/// Owner/admin, or a local agent under "Allow managing compute nodes".
/// With no manager this is exactly [`need_admin`].
fn admin_or_manager(who: &Who, mgr: Option<&Manager>) -> R<()> {
    if mgr.is_some() {
        return Ok(());
    }
    need_admin(who)
}

pub(crate) fn need_credential(who: &Who) -> R<()> {
    match who {
        Who::Invalid => Err(no("forbidden", 403, "invalid or missing token")),
        Who::Wrong => Err(no("wrong_credential", 403, "app passes, API keys and API sessions can't use compute nodes")),
        _ => Ok(()),
    }
}

// ── small helpers ────────────────────────────────────────────────────

pub(crate) fn ws_handle(project_id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(project_id).unwrap_or_else(|_| project_id.to_string())
}

#[derive(Debug, Clone)]
pub(crate) struct Ws {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) handle: String,
}

pub(crate) fn ws_by_id(id: &str) -> Option<Ws> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let p = k2_core::db::schema::Project::get(&conn, id).ok()?;
    let handle = k2_core::workspace_session_handles::workspace_address_name(&conn, &p.id).unwrap_or_else(|_| p.id.clone());
    Some(Ws { id: p.id, path: p.path, handle })
}

pub(crate) fn ws_by_token(token: &str) -> Option<Ws> {
    let t = token.trim();
    if t.is_empty() {
        return None;
    }
    if let Some(w) = ws_by_id(t) {
        return Some(w);
    }
    let path = crate::workspace_msg::resolve_workspace(t).or_else(|| {
        t.starts_with('/')
            .then(|| dunce::canonicalize(&t).ok())
            .flatten()
            .and_then(|c| crate::workspace_msg::resolve_workspace(&c.to_string_lossy()))
    })?;
    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)?
    };
    ws_by_id(&id)
}

/// [`ws_by_token`] for callers WITHOUT a passport (the owner token, a
/// Connect login): a path inside a git worktree of a workspace's repo
/// (`.claude/worktrees/x`, where the CLI's walk-up stops because worktrees
/// carry tracked `.k2/` files) answers as that workspace. Passports never
/// get this: they only ever compare against their own workspace.
pub(crate) fn ws_by_token_or_worktree(token: &str) -> Option<Ws> {
    if let Some(w) = ws_by_token(token) {
        return Some(w);
    }
    let t = Path::new(token.trim());
    if !t.is_absolute() || !t.is_dir() {
        return None;
    }
    let main = dunce::canonicalize(&compute::sync::main_checkout_of(t)?).ok()?;
    if let Some(w) = ws_by_token(&main.to_string_lossy()) {
        return Some(w);
    }
    // git answers with the real path (/private/tmp on macOS); the project
    // row may hold the path as it was added (/tmp/…): compare real paths.
    let id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::list(&conn)
            .ok()?
            .into_iter()
            .find(|p| dunce::canonicalize(&Path::new(&p.path)).is_ok_and(|c| c == main))
            .map(|p| p.id)
    }?;
    ws_by_id(&id)
}

pub(crate) fn no_workspace(req: &str) -> Refusal {
    no(
        "workspace_not_found",
        404,
        format!("no workspace '{req}'; from a folder outside the workspace pass --workspace <handle> (k2 compute grants lists them)"),
    )
}

/// The workspace a request acts for. A passport's comes from the passport
/// (naming another is `forbidden`); others must name one.
pub(crate) fn target_ws(who: &Who, requested: Option<&str>) -> R<Ws> {
    let requested = requested.map(str::trim).filter(|s| !s.is_empty());
    if let Some((own, _)) = who.passport() {
        let ws = ws_by_id(own).ok_or_else(|| no("workspace_not_found", 404, "your workspace is not registered"))?;
        if let Some(req) = requested {
            if ws_by_token(req).is_none_or(|o| o.id != ws.id) {
                return Err(no("forbidden", 403, "an agent may act only for its own workspace"));
            }
        }
        return Ok(ws);
    }
    let req = requested.ok_or_else(|| no("workspace_not_found", 404, "pass --workspace <workspace>"))?;
    ws_by_token_or_worktree(req).ok_or_else(|| no_workspace(req))
}

/// The workspace a grant/revoke acts for. A manager may name any workspace
/// (its own when it names none); everyone else follows [`target_ws`].
pub(crate) fn grant_target_ws(who: &Who, requested: Option<&str>, mgr: Option<&Manager>) -> R<Ws> {
    let Some(m) = mgr else { return target_ws(who, requested) };
    match requested.map(str::trim).filter(|s| !s.is_empty()) {
        Some(req) => ws_by_token_or_worktree(req).ok_or_else(|| no_workspace(req)),
        None => ws_by_id(&m.project_id).ok_or_else(|| no("workspace_not_found", 404, "your workspace is not registered")),
    }
}

/// For passports: the workspace switch must be on.
pub(crate) fn passport_gate(who: &Who) -> R<Option<String>> {
    let Some((own, _)) = who.passport() else { return Ok(None) };
    let on = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::agents_can_use_compute(&conn, own)
    };
    if !on {
        return Err(no(
            codes::COMPUTE_OFF,
            403,
            "compute nodes are off for this workspace: ask your owner to turn on \"Allow compute nodes\" in Settings → Workspaces → <agent> → Agent",
        ));
    }
    Ok(Some(own.to_string()))
}

pub(crate) fn json_body(body: &[u8]) -> R<serde_json::Value> {
    if body.is_empty() {
        return Ok(serde_json::json!({}));
    }
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| no(codes::BAD_REQUEST, 400, format!("invalid JSON body: {e}")))?;
    if !v.is_object() {
        return Err(no(codes::BAD_REQUEST, 400, "expected a JSON object"));
    }
    Ok(v)
}

pub(crate) fn s<'a>(v: &'a serde_json::Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str()).map(str::trim).filter(|x| !x.is_empty())
}

pub(crate) fn b(v: &serde_json::Value, k: &str) -> Option<bool> {
    v.get(k).and_then(|x| x.as_bool())
}

pub(crate) fn n(v: &serde_json::Value, k: &str) -> Option<i64> {
    v.get(k).and_then(|x| x.as_i64())
}

pub(crate) fn node_or_404(token: Option<&str>) -> R<store::NodeRow> {
    let t = token.ok_or_else(|| no(codes::BAD_REQUEST, 400, "name a node"))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::find_node(&conn, t)
        .filter(|n| n.state != "revoked")
        .ok_or_else(|| no(codes::NODE_NOT_FOUND, 404, format!("no compute node '{t}' (k2 compute nodes)")))
}

/// A node-name prefix that matches more than one node the caller may see
/// (CE-11; exit 2).
pub(crate) const NODE_AMBIGUOUS: &str = "node_ambiguous";

/// A node for a read or run path (CE-11, CE-A11): the exact name or id, as
/// [`node_or_404`], else a prefix that matches exactly one node the caller
/// may see (`k2n12` → `k2n12-vin`). Visible = granted to `ws_id`, except
/// for the owner and Admins, who see every node. Owner and destructive
/// verbs (node remove/retire/rename/pause, grant, revoke) keep
/// [`node_or_404`]: they need the full name.
pub(crate) fn resolve_node_for(who: &Who, token: Option<&str>, ws_id: Option<&str>) -> R<store::NodeRow> {
    let t = token.map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| no(codes::BAD_REQUEST, 400, "name a node"))?;
    let (exact, all) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (store::find_node(&conn, t).filter(|n| n.state != "revoked"), store::list_nodes(&conn, false))
    };
    if let Some(n) = exact {
        return Ok(n);
    }
    let granted: Option<Vec<String>> = (!who.is_admin()).then(|| ws_id.map(granted_node_ids).unwrap_or_default());
    let mut hits: Vec<store::NodeRow> = all
        .into_iter()
        .filter(|n| n.state != "revoked" && n.name.starts_with(t) && granted.as_ref().is_none_or(|g| g.contains(&n.id)))
        .collect();
    hits.sort_by(|a, b| a.name.cmp(&b.name));
    match hits.len() {
        1 => Ok(hits.remove(0)),
        0 => Err(no(codes::NODE_NOT_FOUND, 404, format!("no compute node '{t}' (k2 compute nodes)"))),
        _ => {
            let names: Vec<&str> = hits.iter().map(|n| n.name.as_str()).collect();
            Err(no(NODE_AMBIGUOUS, 400, format!("'{t}' matches {} nodes ({}): name one of them in full", names.len(), names.join(", "))))
        }
    }
}

pub(crate) fn audit(who: &Who, kind: &str, node: Option<&str>, ws: Option<&str>, job: Option<&str>, detail: serde_json::Value) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::record_event(&conn, &who.label(), kind, node, ws, job, detail);
}

// ── entry ────────────────────────────────────────────────────────────

/// Route a compute request that did not come in on the loopback listener
/// (the per-cell socket). Blocking. "Allow managing compute nodes" never
/// applies here: see [`handle_from`].
pub fn handle(path: &str, who: Who, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    handle_from(path, who, body, params, false)
}

/// Route a compute request. Blocking. `local` = it reached the TCP
/// loopback listener (not LAN, the Connect tunnel, an App link or a
/// loopback proxy): only then may a passport act under "Allow managing
/// compute nodes" ([`crate::compute_manage`]).
pub fn handle_from(path: &str, who: Who, body: &[u8], params: &HashMap<String, String>, local: bool) -> CliResponse {
    if !compute::enabled() {
        return dark();
    }
    // CI on compute (0.46.10, dark): its routes answer `ci_preview_off`
    // before anything is read or written.
    if is_ci_route(path) && !compute::ci_enabled() {
        let what = if path.starts_with("/cli/ci/") { "k2 ci" } else { "k2 compute roster, lanes, seats, trust and owners" };
        return refusal_response(&ci_preview_off(what));
    }
    let mgr = crate::compute_manage::manager(&who, local);
    let mgr = mgr.as_ref();
    // Queue timeouts and moves need the ticker even before any node attached.
    compute_ws::ensure_ticker_started();
    let r = match path {
        "/cli/compute/nodes" => views::nodes_list(&who, params, mgr),
        "/cli/compute/nodes/info" => views::node_info(&who, params, mgr),
        "/cli/compute/nodes/enroll-code" => enroll_code(&who, body, mgr),
        "/cli/compute/nodes/confirm" => node_confirm(&who, body, mgr),
        // Hard delete: the owner's, switch or not.
        "/cli/compute/nodes/remove" => node_remove(&who, body),
        "/cli/compute/nodes/retire" => node_retire(&who, body, mgr),
        "/cli/compute/nodes/rename" => node_rename(&who, body, mgr),
        "/cli/compute/nodes/pause" => node_pause(&who, body, mgr),
        "/cli/compute/nodes/routes" => node_routes(&who, body, mgr),
        // Package requests (protocol 7, prd-compute-self-serve-environments).
        p if crate::compute_pkg::is_route(p) => crate::compute_pkg::handle(p, &who, body, params),
        "/cli/compute/grants" => grants_list(&who, params, mgr),
        "/cli/compute/grants/set" => grant_set(&who, body, mgr),
        "/cli/compute/grants/revoke" => grant_revoke(&who, body, mgr),
        "/cli/compute/agents" => agents_list(&who),
        "/cli/compute/agents/set" => agents_set(&who, body),
        // A workspace's node list (0.46.8): read; set or clear (a person,
        // or an agent narrowing its own).
        crate::compute_allow::SHOW => crate::compute_allow::show(&who, params),
        crate::compute_allow::SET => crate::compute_allow::set(&who, body),
        // Git access for jobs (0.46.9): read; grant (a person); revoke (a
        // person, or an agent for its own workspace); setup (owner/Admin).
        crate::compute_git::SHOW => crate::compute_git::show(&who, params),
        crate::compute_git::GRANT => crate::compute_git::grant(&who, body),
        crate::compute_git::REVOKE => crate::compute_git::revoke(&who, body),
        crate::compute_git::SETUP => crate::compute_git::setup(&who, body),
        "/cli/compute/run" => run(&who, body),
        // `k2 compute prewarm` (CE-18): tools onto matching granted nodes.
        "/cli/compute/prewarm" => prewarm(&who, body),
        "/cli/compute/jobs" => views::jobs_list(&who, params),
        "/cli/compute/jobs/get" => views::job_get(&who, params),
        "/cli/compute/jobs/logs" => job_logs(&who, params),
        "/cli/compute/jobs/receipt" => job_receipt(&who, params),
        "/cli/compute/jobs/artifact" => job_artifact(&who, params),
        "/cli/compute/jobs/cancel" => views::job_cancel(&who, body),
        "/cli/compute/jobs/retry" => job_retry(&who, body),
        "/cli/compute/jobs/tier" => job_retier(&who, body),
        // `k2 compute move` (0.46.8): a queued job to auto or a named node.
        crate::compute_move::MOVE => crate::compute_move::handle(&who, body),
        "/cli/compute/usage" => usage(&who, params),
        "/cli/compute/events" => events(&who, params),
        "/cli/compute/local/status" => local_status(&who),
        "/cli/compute/local/pause" => local_control(&who, body, "pause"),
        "/cli/compute/local/drain" => local_control(&who, body, "drain"),
        "/cli/compute/local/resume" => local_control(&who, body, "resume"),
        "/cli/compute/local/policy" => local_policy(&who, body),
        "/cli/compute/local/enable" => local_enable(&who, body),
        "/cli/compute/local/disable" => local_disable(&who, body),
        "/cli/compute/local/remote-admin" => local_remote_admin(&who, body),
        // Node sharing (prd-compute-shared-queue-v1 §9).
        "/cli/compute/queue" => views::queue(&who, params),
        // Job caches on the nodes (`--cache NAME`, protocol 6).
        "/cli/compute/cache" => caches(&who, params, mgr),
        "/cli/compute/cache/clear" => cache_clear(&who, body, mgr),
        // Node admin (prd-k2-node-admin-v1): approvals, policy, update,
        // health, alerts; and a shared node's servers.
        p if crate::compute_admin::is_route(p) => crate::compute_admin::handle(p, &who, body, params, mgr),
        // `k2 ci` (0.46.10, C6a): watch, run, status, init, freeze.
        p if crate::ci_routes::is_route(p) => crate::ci_routes::handle(p, &who, body, params, mgr),
        // Node rosters, lanes, seats, signer pairing (0.46.10 C1/C3a).
        p if crate::compute_trust::is_route(p) => crate::compute_trust::handle(p, &who, body, params),
        _ => Err(no("not_found", 404, "no such compute route")),
    };
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refusal_response(&e),
    }
}

fn controller_url(override_url: Option<&str>) -> String {
    if let Some(u) = override_url {
        return u.trim_end_matches('/').to_string();
    }
    format!("https://{}", compute_ws::controller_label())
}

fn enroll_code(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<serde_json::Value> {
    owner_or_manager(who, mgr)?;
    let v = json_body(body)?;
    let name = s(&v, "name").ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the node: k2 compute node add <name>"))?;
    if !compute::enroll::valid_name(name) {
        return Err(no(codes::BAD_REQUEST, 400, "node names are 1–32 of a-z, 0-9 and '-'"));
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if store::live_node_by_name(&conn, name).is_some() {
            return Err(no("name_taken", 409, format!("a node named '{name}' exists; remove it first or pick another name")));
        }
    }
    let url = controller_url(s(&v, "url"));
    if let Some(u) = s(&v, "url") {
        if !(u.starts_with("https://") || u.starts_with("http://")) {
            return Err(no(codes::BAD_REQUEST, 400, "url must be http(s)://"));
        }
    }
    let key = compute::controller_key().map_err(|e| no("controller_key", 500, e))?;
    let fp = key.fingerprint();
    let (code, entry) = compute::enroll::mint(name, &who.label()).map_err(|e| no(codes::BAD_REQUEST, 409, e))?;
    let enroll = pairing::EnrollString::new(&code, &fp).ok_or_else(|| no("internal", 500, "enroll string"))?.render();
    audit(who, "enroll_code", None, None, None, serde_json::json!({"name": name, "url": url}));
    if let Some(m) = mgr {
        crate::compute_manage::audit(m, "node-add", name, &[]);
        crate::compute_manage::owner_notice(m, "added", name);
    }
    // The printed commands take the code on stdin (`--enroll -`, `--code
    // -` from a file only the admin can read): in argv, `ps` shows it to
    // every user on the node machine (2026-10-28).
    let args = format!("--controller {url} --enroll - --name {name}");
    Ok(serde_json::json!({
        "name": name,
        "code": code,
        "controllerFp": fp,
        "enroll": enroll,
        "expiresAt": entry.expires_at,
        "controllerUrl": url,
        "install": {
            "linux": format!("sudo scripts/node/install-node-linux.sh --binary ./k2-node {args} --human \"$USER\" < code-file"),
            "macos": format!("sudo scripts/node/install-node-macos.sh --binary ./k2-node {args} --human \"$USER\" < code-file"),
            "enrollOnly": format!("k2-node enroll {args} < code-file"),
            // A machine that is already a node for another K2 server.
            "servers": format!("sudo k2-node servers add --url {url} --code - < code-file"),
        },
        "confirm": format!("k2 compute node confirm {name} <code shown on the node>"),
    }))
}

fn node_confirm(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<serde_json::Value> {
    owner_or_manager(who, mgr)?;
    let v = json_body(body)?;
    let n = node_or_404(s(&v, "node"))?;
    let typed = s(&v, "sas").ok_or_else(|| no(codes::BAD_REQUEST, 400, "pass the 6-digit code the node shows"))?;
    if n.state != "pending" {
        return Err(no("not_pending", 409, format!("'{}' is already {}", n.name, n.state)));
    }
    let expected = n.sas.clone().unwrap_or_default();
    if !pairing::sas_matches(&expected, typed) {
        audit(who, "confirm_refused", Some(&n.id), None, None, serde_json::json!({"reason": "sas_mismatch"}));
        return Err(no(
            "sas_mismatch",
            409,
            "the code doesn't match what this server computed. Don't confirm: remove the node and enroll again (k2 compute node remove <name>)",
        ));
    }
    let auto_granted = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::confirm_node(&conn, &n.id, &who.label(), compute::now()).map_err(|e| no("not_pending", 409, e))?;
        store::record_event(&conn, &who.label(), "confirm", Some(&n.id), None, None, serde_json::json!({"name": n.name}));
        // Every agent with compute on gets the new machine (the Agent
        // tab's "Allow compute nodes").
        let granted = compute::access::grant_new_node(&conn, &n.id, &who.label(), compute::now()).map_err(|e| no("internal", 500, e))?;
        for ws in &granted {
            store::record_event(&conn, &who.label(), "grant", Some(&n.id), Some(ws), None, serde_json::json!({"auto": true}));
        }
        granted
    };
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, serde_json::json!({"compute": "node_confirmed"}));
    if let Some(m) = mgr {
        crate::compute_manage::audit(m, "node-confirm", &n.name, &[]);
        crate::compute_manage::owner_notice(m, "confirmed", &n.name);
    }
    let handles: Vec<String> = auto_granted.iter().map(|w| ws_handle(w)).collect();
    let next = if handles.is_empty() {
        format!("k2 compute grant {} --workspace <ws> (no workspace may use it yet)", n.name)
    } else {
        format!("granted to every workspace with compute on ({}); k2 compute grant {} --workspace <ws> for others", handles.join(", "), n.name)
    };
    Ok(serde_json::json!({"ok": true, "node": n.name, "state": "active", "autoGranted": handles, "next": next}))
}

fn node_remove(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_owner(who)?;
    let v = json_body(body)?;
    let n = node_or_404(s(&v, "node"))?;
    let ended = remove_now(&who.label(), &n)?;
    Ok(serde_json::json!({"ok": true, "node": n.name, "state": "revoked", "jobsCancelled": ended}))
}

/// Revoke a node now: cancel its queued and running jobs, tell it, forget
/// it in the retiring set. Returns how many jobs were cancelled.
fn remove_now(by: &str, n: &store::NodeRow) -> R<usize> {
    let ended = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::revoke_node(&conn, &n.id, by, compute::now()).map_err(|e| no("not_found", 404, e))?;
        let mut ended = Vec::new();
        for j in store::jobs_in_states(&conn, &n.id, &["queued", "assigned", "preparing", "running", "finishing"]) {
            let _ = store::set_job_state(&conn, &j.id, JobState::Cancelled, Some("node_removed"), None, compute::now());
            ended.push(j.id);
        }
        store::record_event(&conn, by, "remove", Some(&n.id), None, None, serde_json::json!({"name": n.name, "jobsCancelled": ended.len()}));
        ended
    };
    let _ = compute::access::unmark_retiring_at(&compute::access::retiring_path(), &n.id);
    compute_ws::revoke(&n.id);
    for id in &ended {
        compute_ws::finalize(id);
    }
    compute_ws::notify_change();
    Ok(ended.len())
}

/// "Use compute from this machine" OFF (Settings → Compute): queued jobs
/// are cancelled, running ones finish (the node drains meanwhile), then
/// the node is removed. Idle → removed now. Owner only, like remove.
fn node_retire(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<serde_json::Value> {
    owner_or_manager(who, mgr)?;
    let v = json_body(body)?;
    let n = node_or_404(s(&v, "node"))?;
    let now = compute::now();
    let (cancelled, running) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut cancelled = Vec::new();
        for j in store::jobs_in_states(&conn, &n.id, &["queued"]) {
            let _ = store::set_job_state(&conn, &j.id, JobState::Cancelled, Some("node_retired"), None, now);
            cancelled.push(j.id);
        }
        (cancelled, store::jobs_in_states(&conn, &n.id, store::ACTIVE_STATES).len())
    };
    for id in &cancelled {
        compute_ws::finalize(id);
    }
    if running == 0 {
        let more = remove_now(&who.label(), &n)?;
        if let Some(m) = mgr {
            crate::compute_manage::audit(m, "node-retire", &n.name, &[("state", "revoked")]);
        }
        return Ok(serde_json::json!({"ok": true, "node": n.name, "state": "revoked", "jobsCancelled": cancelled.len() + more}));
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_controller_pause(&conn, &n.id, Some("draining"), &who.label(), now).map_err(|e| no("internal", 500, e))?;
        store::record_event(
            &conn,
            &who.label(),
            "retire",
            Some(&n.id),
            None,
            None,
            serde_json::json!({"name": n.name, "running": running, "queuedCancelled": cancelled.len()}),
        );
    }
    compute::access::mark_retiring_at(&compute::access::retiring_path(), &n.id, &who.label(), now).map_err(|e| no("local_write", 500, e))?;
    if let Some(m) = mgr {
        crate::compute_manage::audit(m, "node-retire", &n.name, &[("state", "draining")]);
    }
    compute_ws::kick(&n.id);
    compute_ws::notify_change();
    Ok(serde_json::json!({"ok": true, "node": n.name, "state": "draining", "running": running, "jobsCancelled": cancelled.len(),
        "note": "running jobs finish, then the node is removed"}))
}

/// Finish retiring nodes whose jobs are done (the compute ticker, the
/// nodes list and the retire route call this).
pub fn sweep_retiring() {
    let path = compute::access::retiring_path();
    for (id, r) in compute::access::retiring_at(&path) {
        let step = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            compute::access::retire_step(&conn, &id)
        };
        match step {
            compute::access::RetireStep::Forget => {
                let _ = compute::access::unmark_retiring_at(&path, &id);
            }
            compute::access::RetireStep::Wait { .. } => {}
            compute::access::RetireStep::Remove => {
                let row = {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    store::node_by_id(&conn, &id)
                };
                if let Some(n) = row {
                    if let Err(e) = remove_now(&r.by, &n) {
                        eprintln!("[compute] retiring {}: {}", n.name, e.hint);
                    }
                }
            }
        }
    }
}

fn node_rename(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<serde_json::Value> {
    admin_or_manager(who, mgr)?;
    let v = json_body(body)?;
    let n = node_or_404(s(&v, "node"))?;
    let name = s(&v, "name").ok_or_else(|| no(codes::BAD_REQUEST, 400, "pass the new name"))?;
    if !compute::enroll::valid_name(name) {
        return Err(no(codes::BAD_REQUEST, 400, "node names are 1–32 of a-z, 0-9 and '-'"));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if store::live_node_by_name(&conn, name).is_some_and(|o| o.id != n.id) {
        return Err(no("name_taken", 409, format!("'{name}' is taken")));
    }
    store::rename_node(&conn, &n.id, name).map_err(|e| no("internal", 500, e))?;
    store::record_event(&conn, &who.label(), "rename", Some(&n.id), None, None, serde_json::json!({"from": n.name, "to": name}));
    drop(conn);
    if let Some(m) = mgr {
        crate::compute_manage::audit(m, "node-rename", &n.name, &[("to", name)]);
    }
    Ok(serde_json::json!({"ok": true, "node": name}))
}

fn node_pause(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<serde_json::Value> {
    admin_or_manager(who, mgr)?;
    let v = json_body(body)?;
    let n = node_or_404(s(&v, "node"))?;
    let action = s(&v, "action").unwrap_or("pause");
    let pause = match action {
        "pause" => Some("paused"),
        "drain" => Some("draining"),
        "resume" => None,
        _ => return Err(no(codes::BAD_REQUEST, 400, "action is pause, drain or resume")),
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_controller_pause(&conn, &n.id, pause, &who.label(), compute::now()).map_err(|e| no("internal", 500, e))?;
        store::record_event(&conn, &who.label(), "controller_pause", Some(&n.id), None, None, serde_json::json!({"action": action}));
    }
    if let Some(m) = mgr {
        crate::compute_manage::audit(m, &format!("node-{action}"), &n.name, &[]);
    }
    if pause.is_none() {
        // Resuming a retiring node keeps it (the machine was switched back on).
        let _ = compute::access::unmark_retiring_at(&compute::access::retiring_path(), &n.id);
        // …and puts a node set aside for infra failures back at once.
        crate::compute_place::clear_set_aside(&n.id);
    }
    compute_ws::kick(&n.id);
    Ok(serde_json::json!({"ok": true, "node": n.name, "controllerPause": pause,
        "note": "this only stops new assignments from this server; the machine's owner pauses the node itself on that machine"}))
}

/// A route a node may dial: https/wss anywhere; http/ws only to a
/// private, loopback, link-local or tailnet (100.64/10) address.
fn route_ok(u: &str) -> bool {
    if u.starts_with("https://") || u.starts_with("wss://") {
        return u.len() > 10;
    }
    let rest = match u.strip_prefix("http://").or_else(|| u.strip_prefix("ws://")) {
        Some(r) => r,
        None => return false,
    };
    let host = rest.split(['/', ':']).next().unwrap_or("");
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => {
            ip.is_private() || ip.is_loopback() || ip.is_link_local() || (ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64)
        }
        Err(_) => host == "localhost" || host.ends_with(".local") || host.ends_with(".ts.net"),
    }
}

fn node_routes(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<serde_json::Value> {
    admin_or_manager(who, mgr)?;
    let v = json_body(body)?;
    let n = node_or_404(s(&v, "node"))?;
    let mut routes = n.routes.clone();
    if let Some(add) = s(&v, "add") {
        if !route_ok(add) {
            return Err(no(codes::BAD_REQUEST, 400, "routes are https:// (anywhere) or http:// to a private, tailnet or loopback address"));
        }
        if !routes.iter().any(|r| r == add) {
            routes.push(add.trim_end_matches('/').to_string());
        }
    }
    if let Some(rm) = s(&v, "remove") {
        routes.retain(|r| r != rm.trim_end_matches('/'));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::set_routes(&conn, &n.id, &routes).map_err(|e| no("internal", 500, e))?;
    store::record_event(&conn, &who.label(), "routes", Some(&n.id), None, None, serde_json::json!({"routes": routes}));
    drop(conn);
    if let Some(m) = mgr {
        if let Some(add) = s(&v, "add") {
            crate::compute_manage::audit(m, "node-route-add", &n.name, &[("url", add)]);
        }
        if let Some(rm) = s(&v, "remove") {
            crate::compute_manage::audit(m, "node-route-rm", &n.name, &[("url", rm)]);
        }
    }
    Ok(serde_json::json!({"ok": true, "node": n.name, "routes": routes, "note": "the node learns new routes on its next connect"}))
}

// ── grants ───────────────────────────────────────────────────────────

fn grants_list(who: &Who, params: &HashMap<String, String>, mgr: Option<&Manager>) -> R<serde_json::Value> {
    need_credential(who)?;
    let requested = params.get("workspace").map(|s| s.trim()).filter(|s| !s.is_empty());
    // An agent (no manager switch) sees its own workspace's grants only:
    // naming another is refused, never silently swapped for its own.
    if let (Who::Agent { .. }, None) = (who, mgr) {
        let own = passport_gate(who)?.ok_or_else(|| no("forbidden", 403, "an agent's passport names its workspace"))?;
        if let Some(t) = requested {
            if ws_by_token_or_worktree(t).is_none_or(|w| w.id != own) {
                return Err(no(
                    "forbidden",
                    403,
                    format!(
                        "an agent sees only its own workspace's grants, not '{t}' (k2 compute grants with no --workspace); \
                         the owner, an Admin or a person in a K2 shell tab sees any workspace's"
                    ),
                ));
            }
        }
        return grants_json(params, Some(own));
    }
    // The owner, a login, a manager (it grants any workspace) or a person
    // in a K2 shell tab: any workspace, or all of them. A shell tab with no
    // --workspace lists its own, as before.
    let ws = match (requested, who, mgr) {
        (Some(t), _, _) => Some(ws_by_token_or_worktree(t).ok_or_else(|| no_workspace(t))?.id),
        (None, Who::Shell { project_id, .. }, None) => Some(project_id.clone()),
        (None, _, _) => None,
    };
    grants_json(params, ws)
}

fn grants_json(params: &HashMap<String, String>, ws: Option<String>) -> R<serde_json::Value> {
    let node = match params.get("node").map(String::as_str).filter(|s| !s.is_empty()) {
        Some(t) => Some(node_or_404(Some(t))?.id),
        None => None,
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let rows: Vec<serde_json::Value> = store::grants(&conn, node.as_deref(), ws.as_deref())
        .into_iter()
        .map(|g| {
            let mut v = serde_json::to_value(&g).unwrap_or_default();
            v["node"] = store::node_by_id(&conn, &g.node_id).map(|n| n.name).into();
            v["workspace"] = ws_handle(&g.workspace_id).into();
            v
        })
        .collect();
    Ok(serde_json::json!({ "grants": rows }))
}

fn grant_set(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<serde_json::Value> {
    admin_or_manager(who, mgr)?;
    let v = json_body(body)?;
    let n = node_or_404(s(&v, "node"))?;
    if n.state != "active" {
        return Err(no("not_active", 409, format!("'{}' is {}; confirm it first", n.name, n.state)));
    }
    let ws = grant_target_ws(who, s(&v, "workspace"), mgr)?;
    // CI jumps every queue on the node: only the owner or an Admin login
    // turns it on or off, never an agent, not even one that may manage
    // compute nodes (prd-compute-job-tiers-v1 §5.1, T12). Agents ask with a
    // `compute.ci` permission card.
    let allow_ci = match v.get("allowCi") {
        None | Some(serde_json::Value::Null) => None,
        Some(x) => Some(x.as_bool().ok_or_else(|| no(codes::BAD_REQUEST, 400, "allowCi is true or false"))?),
    };
    if allow_ci.is_some() && (mgr.is_some() || !who.is_admin()) {
        if let Some(m) = mgr {
            crate::compute_manage::audit(m, "grant_ci_refused", &n.name, &[("for", &ws.handle)]);
        }
        return Err(no(
            tiers::codes::CI_NOT_ALLOWED,
            403,
            format!(
                "only the owner or an Admin turns CI on or off for a grant (CI jumps every queue): the owner runs k2 compute grant {} --workspace {} --ci",
                n.name, ws.handle
            ),
        ));
    }
    // GPU jobs (0.46.11): the same rule as CI. Only the owner or an Admin
    // turns `--gpu` on or off for a grant; an agent asks with a
    // `compute.gpu` card. The node's own owner decides separately, at the
    // machine, whether the node runs GPU jobs for this server at all.
    let allow_gpu = match v.get("allowGpu") {
        None | Some(serde_json::Value::Null) => None,
        Some(x) => Some(x.as_bool().ok_or_else(|| no(codes::BAD_REQUEST, 400, "allowGpu is true or false"))?),
    };
    if allow_gpu.is_some() && (mgr.is_some() || !who.is_admin()) {
        if let Some(m) = mgr {
            crate::compute_manage::audit(m, "grant_gpu_refused", &n.name, &[("for", &ws.handle)]);
        }
        return Err(no(
            k2_core::compute::proto::gpu::codes::GPU_NEEDS_GRANT,
            403,
            format!("only the owner or an Admin turns GPU jobs on or off for a grant: the owner runs k2 compute grant {} --workspace {} --gpu", n.name, ws.handle),
        )
        .with("node", n.name.clone()));
    }
    let limits = store::GrantLimits {
        max_job_secs: n_pos(&v, "maxJobSecs", 60, 7 * 86_400)?,
        max_disk_gb: n_pos(&v, "maxDiskGb", 1, 10_000)?,
        max_parallel: n_pos(&v, "maxParallel", 1, 64)?,
        max_queued: n_pos(&v, "maxQueued", 1, 1000)?,
        allow_ci,
        // L3 (prd-compute-job-tiers-v2): 0 = off; absent keeps the grant's.
        submitter_max_running: n_pos(&v, "submitterMaxRunning", 0, 64)?,
        allow_gpu,
    };
    let (g, before) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let before = store::grant(&conn, &n.id, &ws.id);
        let g = store::set_grant(&conn, &n.id, &ws.id, &limits, &who.label(), compute::now()).map_err(|e| no("internal", 500, e))?;
        store::record_event(&conn, &who.label(), "grant", Some(&n.id), Some(&ws.id), None, serde_json::to_value(&g).unwrap_or_default());
        (g, before)
    };
    if let Some(m) = mgr {
        crate::compute_manage::audit(m, "grant", &n.name, &[("for", &ws.handle)]);
    }
    // L5 (prd-compute-job-tiers-v2 T23/T24): each job in a shared node's
    // queue carries its workspace's cap from when it was sent. A changed
    // cap re-sends this workspace's jobs waiting there (withdraw, then
    // send again with the grant's cap, oldest first). A solo node reads
    // the grant live and needs nothing.
    let resent = if before.as_ref().is_none_or(|b| b.max_parallel != g.max_parallel || b.submitter_max_running != g.submitter_max_running) {
        compute_ws::resend_queued(&n.id, &ws.id, None)
    } else {
        0
    };
    // A solo node reads the grant live: place what the new caps allow.
    compute_ws::kick(&n.id);
    let switch_on = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::agents_can_use_compute(&conn, &ws.id)
    };
    Ok(serde_json::json!({"ok": true, "node": n.name, "workspace": ws.handle, "grant": g, "agentsCanUseCompute": switch_on, "resent": resent,
        "note": if switch_on { "agents in this workspace can use the node now" } else { "agents also need Settings → Workspaces → <agent> → Agent → Allow compute nodes (k2 compute access on --workspace <ws>)" }}))
}

fn n_pos(v: &serde_json::Value, k: &str, lo: i64, hi: i64) -> R<Option<i64>> {
    match v.get(k) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(x) => match x.as_i64() {
            Some(n) if (lo..=hi).contains(&n) => Ok(Some(n)),
            _ => Err(no(codes::BAD_REQUEST, 400, format!("{k} must be a number from {lo} to {hi}"))),
        },
    }
}

fn grant_revoke(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<serde_json::Value> {
    admin_or_manager(who, mgr)?;
    let v = json_body(body)?;
    let n = node_or_404(s(&v, "node"))?;
    let ws = grant_target_ws(who, s(&v, "workspace"), mgr)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let removed = store::revoke_grant(&conn, &n.id, &ws.id).map_err(|e| no("internal", 500, e))?;
    store::record_event(&conn, &who.label(), "grant_revoke", Some(&n.id), Some(&ws.id), None, serde_json::json!({"removed": removed}));
    drop(conn);
    if let Some(m) = mgr {
        crate::compute_manage::audit(m, "revoke", &n.name, &[("for", &ws.handle)]);
    }
    Ok(serde_json::json!({"ok": true, "removed": removed, "node": n.name, "workspace": ws.handle,
        "note": "queued and running jobs keep going; new runs are refused"}))
}

// ── agents (the Agent tab's "Allow compute nodes") ───────────────────

/// Every workspace with its "Allow compute nodes" switch and grants, plus
/// the default limits a toggle applies. Owner/admin. (Settings → Compute
/// no longer lists agents, Rosson 2026-10-11; kept for the CLI/scripts.)
fn agents_list(who: &Who) -> R<serde_json::Value> {
    need_admin(who)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let names: HashMap<String, String> = store::list_nodes(&conn, false).into_iter().map(|n| (n.id, n.name)).collect();
    let active = compute::access::active_nodes(&conn).len();
    let rows: Vec<serde_json::Value> = compute::access::agents(&conn)
        .into_iter()
        .map(|a| {
            let grants: Vec<serde_json::Value> = a
                .grants
                .iter()
                .map(|g| {
                    let mut v = serde_json::to_value(g).unwrap_or_default();
                    v["node"] = names.get(&g.node_id).cloned().into();
                    v
                })
                .collect();
            serde_json::json!({
                "workspaceId": a.workspace_id,
                "workspace": k2_core::workspace_session_handles::workspace_address_name(&conn, &a.workspace_id).unwrap_or_else(|_| a.workspace_id.clone()),
                "name": a.name,
                "agentsCanUseCompute": a.agents_can_use_compute,
                // Node admin (Q14): "Allow this agent to operate compute nodes".
                "agentsCanOperateCompute": compute::admin::agents_can_operate_compute(&conn, &a.workspace_id),
                // "Allow managing compute nodes" (0194).
                // (Read on the connection this list already holds.)
                "agentsCanManageCompute": conn
                    .query_row("SELECT agents_can_manage_compute FROM projects WHERE id = ?1", [&a.workspace_id], |r| r.get::<_, i64>(0))
                    .map(|v| v == 1)
                    .unwrap_or(false),
                "grants": grants,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "agents": rows,
        "activeNodes": active,
        "defaults": {
            "maxJobSecs": compute::DEFAULT_MAX_JOB_SECS,
            "maxParallel": compute::DEFAULT_MAX_PARALLEL,
            "maxQueued": compute::DEFAULT_MAX_QUEUED,
            "maxDiskGb": compute::DEFAULT_MAX_DISK_GB,
            "disk": "the smaller of 25% of the node's free disk and 60 GB",
        },
    }))
}

/// The one writer behind an agent's compute toggle — Settings →
/// Workspaces → <agent> → Agent → "Allow compute nodes"
/// (`/cli/agent-access/set` toggle `compute`, Rosson 2026-10-11) and the
/// older `/cli/compute/agents/set`. On: the workspace switch plus a grant
/// on every active node (default limits, or the ones sent); nodes added
/// later are granted on confirm ([`compute::access::grant_new_node`]).
/// Off: switch off and every grant of the workspace removed. The caller
/// has already checked owner/admin.
pub(crate) fn set_agent_compute(
    ws_id: &str,
    ws_handle: &str,
    on: bool,
    limits: &store::GrantLimits,
    by: &str,
) -> Result<serde_json::Value, String> {
    let now = compute::now();
    let out = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if on {
            let out = compute::access::enable_workspace(&conn, ws_id, limits, by, now)?;
            let name_of = |id: &String| store::node_by_id(&conn, id).map(|n| n.name).unwrap_or_else(|| id.clone());
            let granted: Vec<String> = out.granted.iter().map(name_of).collect();
            let kept: Vec<String> = out.kept.iter().map(name_of).collect();
            store::record_event(&conn, by, "agent_compute_on", None, Some(ws_id), None, serde_json::json!({"granted": granted, "kept": kept}));
            serde_json::json!({"ok": true, "workspace": ws_handle, "toggle": "compute", "agentsCanUseCompute": true, "granted": granted, "kept": kept,
                "note": "agents in this workspace can use every machine switched on here, and ones you add later"})
        } else {
            let removed = compute::access::disable_workspace(&conn, ws_id)?;
            store::record_event(&conn, by, "agent_compute_off", None, Some(ws_id), None, serde_json::json!({"grantsRemoved": removed}));
            serde_json::json!({"ok": true, "workspace": ws_handle, "toggle": "compute", "agentsCanUseCompute": false, "grantsRemoved": removed,
                "note": "queued and running jobs keep going; new runs are refused"})
        }
    };
    // Projects carry the switch (Agent tab); Settings → Compute shows grants.
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncProjects, serde_json::Value::Null);
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, serde_json::json!({"compute": "agent_access"}));
    Ok(out)
}

/// `/cli/compute/agents/set` (kept for older clients and
/// scripts): same writer as the Agent tab.
fn agents_set(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_admin(who)?;
    let v = json_body(body)?;
    let ws = target_ws(who, s(&v, "workspace"))?;
    let on = b(&v, "value").ok_or_else(|| no(codes::BAD_REQUEST, 400, "pass \"value\": true or false"))?;
    let limits = store::GrantLimits {
        max_job_secs: n_pos(&v, "maxJobSecs", 60, 7 * 86_400)?,
        max_disk_gb: n_pos(&v, "maxDiskGb", 1, 10_000)?,
        max_parallel: n_pos(&v, "maxParallel", 1, 64)?,
        max_queued: n_pos(&v, "maxQueued", 1, 1000)?,
        // The agents switch never touches CI (owner/Admin, per grant), the
        // per-submitter cap or GPU jobs.
        allow_ci: None,
        submitter_max_running: None,
        allow_gpu: None,
    };
    set_agent_compute(&ws.id, &ws.handle, on, &limits, &who.label()).map_err(|e| no("internal", 500, e))
}

// ── run ──────────────────────────────────────────────────────────────

fn valid_client_id(s: &str) -> bool {
    (16..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn valid_cwd(c: &str) -> bool {
    !c.starts_with('/') && !c.split('/').any(|p| p == "..") && !c.contains('\0') && c.len() < 1024
}

/// `needs` from a run body: an array of tokens (or one comma string),
/// checked with the same parser the node uses; canonical tokens out.
/// `--distro ubuntu@24.04` (protocol 7), checked.
fn distro_from(v: &serde_json::Value) -> R<Option<String>> {
    match s(v, "distro").map(str::trim).filter(|x| !x.is_empty()) {
        None => Ok(None),
        Some(d) => k2_core::compute::proto::host::parse_distro(d).map(|_| Some(d.to_ascii_lowercase())).map_err(|e| no(codes::BAD_REQUEST, 400, e)),
    }
}

fn needs_from(v: &serde_json::Value) -> R<Vec<String>> {
    let raw: Vec<String> = match v.get("needs") {
        None | Some(serde_json::Value::Null) => return Ok(Vec::new()),
        Some(serde_json::Value::String(s)) => s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect(),
        Some(serde_json::Value::Array(a)) => {
            let mut out = Vec::new();
            for x in a {
                out.push(x.as_str().ok_or_else(|| no("need_invalid", 400, "needs are strings like node@22"))?.to_string());
            }
            out
        }
        Some(_) => return Err(no("need_invalid", 400, "needs is a list like [\"node@22\", \"python@3.12\"]")),
    };
    // An unsupported tool is refused here (400 → the CLI exits 2), never
    // dropped: the job must not start without what it asked for. Rust
    // components ride as `rust@<channel>+clippy+rustfmt` (CE-16).
    crate::compute_place::check_needs(&raw).map_err(|e| no(e.code, 400, format!("--needs: {}", e.message)))
}

/// `artifacts` from a run body: globs (array, or one string), checked
/// with the same rules the node uses.
fn artifacts_from(v: &serde_json::Value) -> R<Vec<String>> {
    let raw: Vec<String> = match v.get("artifacts") {
        None | Some(serde_json::Value::Null) => return Ok(Vec::new()),
        Some(serde_json::Value::String(s)) => vec![s.trim().to_string()],
        Some(serde_json::Value::Array(a)) => {
            let mut out = Vec::new();
            for x in a {
                out.push(x.as_str().ok_or_else(|| no("artifact_glob_invalid", 400, "artifacts are glob strings like target/nextest/**/junit.xml"))?.trim().to_string());
            }
            out
        }
        Some(_) => return Err(no("artifact_glob_invalid", 400, "artifacts is a list of globs")),
    };
    results::check_globs(&raw).map_err(|(code, g)| {
        let code: &'static str = match code.as_str() {
            "artifact_glob_absolute" => "artifact_glob_absolute",
            "artifact_glob_escapes" => "artifact_glob_escapes",
            _ => "artifact_glob_invalid",
        };
        no(code, 400, format!("--artifact {g}: globs are relative to the job's working directory (target/… also reads the node's cargo target dir), without '..'"))
    })?;
    Ok(raw)
}

/// What a node's k2-node understands (live offer, else the last one).
fn node_features(n: &store::NodeRow) -> Vec<String> {
    let (_, _, live) = compute_ws::live_view(&n.id);
    live.or_else(|| n.last_offer.clone()).map(|o| o.features).unwrap_or_default()
}

fn run(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_credential(who)?;
    // The workspace switch (`compute_off`) for passports.
    passport_gate(who)?;
    let mut v = json_body(body)?;
    let ws = target_ws(who, s(&v, "workspace"))?;
    // Idempotency is over the body as sent; `.k2/compute.toml` and
    // `--preset` then fill what no flag set (CE-3).
    let sent = v.clone();
    apply_presets(&ws, &mut v)?;
    // `--plan` (dry run): every check `run` makes, nothing stored or sent.
    if b(&v, "plan") == Some(true) {
        if v.get("shards").is_some_and(|x| !x.is_null()) {
            return Err(no(codes::BAD_REQUEST, 400, "--plan shows one job; drop --shards (each shard is planned the same way)"));
        }
        let sub = submit_one(who, &v, &ws, "plan-0000000000000000", "", &SubmitOpts { dry: true, ..Default::default() })?;
        return Ok(sub.preview.unwrap_or_default());
    }
    let client_id = s(&v, "clientJobId").ok_or_else(|| no(codes::BAD_REQUEST, 400, "clientJobId (16–64 chars) is required"))?;
    if !valid_client_id(client_id) {
        return Err(no(codes::BAD_REQUEST, 400, "clientJobId must be 16–64 of A-Z a-z 0-9 - _"));
    }
    let request_sha = crypto::sha256_hex(&k2_core::compute::proto::canonical::to_vec(&sent).map_err(|e| no(codes::BAD_REQUEST, 400, e))?);
    // Idempotency (§9.1): same key + same body → the same job.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(j) = store::job_by_client_id(&conn, &ws.id, client_id) {
            if j.request_sha256 != request_sha {
                return Err(no(codes::IDEMPOTENCY_CONFLICT, 409, "this clientJobId was already used for a different request"));
            }
            drop(conn);
            return Ok(serde_json::json!({ "job": job_summary(&j), "existing": true }));
        }
    }
    if v.get("shards").is_some_and(|x| !x.is_null()) {
        if b(&v, "now") == Some(true) || s(&v, "sameNodeAs").is_some() {
            return Err(no(codes::BAD_REQUEST, 400, "--shards places each shard itself: drop --now and --same-node-as"));
        }
        return run_sharded(who, &v, &ws, client_id, &request_sha);
    }
    let session = who.passport().map(|(_, sid)| sid.to_string());
    let sub = submit_one(who, &v, &ws, client_id, &request_sha, &SubmitOpts { session, ..Default::default() })?;
    Ok(submitted_json(who, &sub))
}

/// `k2 ci` (0.46.10, C6a): submit one check's job for workspace `ws_id`
/// through the normal job path ([`submit_one`]: placement, grants, needs,
/// limits, the tier and supersede rules), with **the workspace's own
/// authority**: a non-admin caller of that workspace, so its compute
/// grants are the fence (a CI job never gets the owner's reach to every
/// node, and `--tier ci` needs a grant with `--ci`). `run_id` names the
/// submitter (`<workspace>/ci-<run>/ci`); no session is woken. Idempotent
/// on the body's `clientJobId` like `/cli/compute/run`. The answer is the
/// run route's JSON (`job`, `notices`, `warnings`, …).
pub(crate) fn submit_for_ci(ws_id: &str, run_id: &str, body: &serde_json::Value) -> R<serde_json::Value> {
    let ws = ws_by_id(ws_id).ok_or_else(|| no("workspace_not_found", 404, "the workspace is no longer registered"))?;
    let who = Who::Agent { project_id: ws.id.clone(), session_id: format!("ci-{}", &run_id[..run_id.len().min(8)]) };
    let client_id = s(body, "clientJobId").ok_or_else(|| no(codes::BAD_REQUEST, 400, "clientJobId is required"))?;
    if !valid_client_id(client_id) {
        return Err(no(codes::BAD_REQUEST, 400, "clientJobId must be 16–64 of A-Z a-z 0-9 - _"));
    }
    let request_sha = crypto::sha256_hex(&k2_core::compute::proto::canonical::to_vec(body).map_err(|e| no(codes::BAD_REQUEST, 400, e))?);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(j) = store::job_by_client_id(&conn, &ws.id, client_id) {
            if j.request_sha256 != request_sha {
                return Err(no(codes::IDEMPOTENCY_CONFLICT, 409, "this clientJobId was already used for a different request"));
            }
            drop(conn);
            return Ok(serde_json::json!({ "job": job_summary(&j), "existing": true }));
        }
    }
    let sub = submit_one(&who, body, &ws, client_id, &request_sha, &SubmitOpts::default())?;
    Ok(submitted_json(&who, &sub))
}

/// `k2 ci release` (C5): would this job body place, and how soon? The
/// normal job path's `--plan` as the workspace's CI identity (its own
/// grants, as [`submit_for_ci`]): nothing is stored. `Ok` = the preview
/// (`placement.startsNow`, `placement.etaSecs`), `Err` = why not.
pub(crate) fn plan_for_ci(ws_id: &str, run_id: &str, body: &serde_json::Value) -> R<serde_json::Value> {
    let ws = ws_by_id(ws_id).ok_or_else(|| no("workspace_not_found", 404, "the workspace is no longer registered"))?;
    let who = Who::Agent { project_id: ws.id.clone(), session_id: format!("ci-{}", &run_id[..run_id.len().min(8)]) };
    let sub = submit_one(&who, body, &ws, "ci-release-placement-probe", "", &SubmitOpts { dry: true, ..SubmitOpts::default() })?;
    Ok(sub.preview.unwrap_or_default())
}

/// What a prewarm job reserves (it runs `sh -c true` after the install).
const PREWARM_MEM: u64 = 512 << 20;

/// `POST /cli/compute/prewarm` (`k2 compute prewarm`, CE-18, CE-A22):
/// for each granted node matching `--os/--arch/--label` (and `nodes`, when
/// named) where any need isn't ready, one `--no-source --tier batch -- sh
/// -c true` job with those needs, so the node installs them while idle.
/// The caller's grant and the Batch tier: it never holds anyone up. A node
/// that already has everything is reported, not sent a job; a second
/// prewarm therefore submits nothing. `plan: true` says what it would do.
fn prewarm(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    use crate::compute_place as place;
    need_credential(who)?;
    passport_gate(who)?;
    let mut v = json_body(body)?;
    let ws = target_ws(who, s(&v, "workspace"))?;
    apply_presets(&ws, &mut v)?;
    let needs = needs_from(&v)?;
    if needs.is_empty() {
        return Err(no(codes::BAD_REQUEST, 400, "prewarm installs --needs: pass --needs rust@stable,cmake@3, or a --preset (or [defaults]) in .k2/compute.toml that lists needs"));
    }
    let tag = tag_from(&v)?;
    let dry = b(&v, "plan") == Some(true);
    let filter = store::AnyFilter {
        os: s(&v, "os").map(str::to_string),
        arch: s(&v, "arch").map(str::to_string),
        labels: v
            .get("labels")
            .and_then(|l| l.as_object())
            .map(|m| m.iter().filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string()))).collect())
            .unwrap_or_default(),
        distro: distro_from(&v)?,
        ..Default::default()
    };
    // `--node N` (repeatable): only these (full name or a unique prefix).
    let named: Option<Vec<String>> = match v.get("nodes").and_then(|n| n.as_array()) {
        Some(a) if !a.is_empty() => {
            let mut ids = Vec::new();
            for x in a {
                let t = x.as_str().ok_or_else(|| no(codes::BAD_REQUEST, 400, "nodes is a list of node names"))?;
                let n = resolve_node_for(who, Some(t), Some(&ws.id))?;
                crate::compute_allow::check_node(&ws, &n, "--node")?;
                ids.push(n.id);
            }
            Some(ids)
        }
        _ => None,
    };
    let want = place::Want {
        ws_id: ws.id.clone(),
        admin: who.is_admin(),
        auto: true,
        filter: filter.clone(),
        needs: needs.clone(),
        artifacts: false,
        exclusive: false,
        needs_git: false,
        cpu_millis: None,
        mem_bytes: Some(PREWARM_MEM),
        mem_defaulted: false,
        timeout_secs: None,
        stall: false,
        secret_ids: Vec::new(),
        tier: Tier::Batch,
        cache: None,
        multi_repo: false,
        receipt_v2: false,
        gpu: None,
        gpu_mem: None,
    };
    // Every matching node: each one's package states are reported, and the
    // names their owners must approve go out as ONE batched request (§7.3).
    let report = place::candidates_report_raw(&want, &[], compute::now());
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut jobs: Vec<String> = Vec::new();
    let mut ask_nodes: Vec<store::NodeRow> = Vec::new();
    let mut ask_names: Vec<String> = Vec::new();
    let mut cands: Vec<&place::Cand> = report.cands.iter().filter(|c| named.as_ref().is_none_or(|ids| ids.contains(&c.node.id))).collect();
    cands.sort_by(|a, b| a.node.name.cmp(&b.node.name));
    for c in cands {
        let states = c.needs(&want);
        let installs: Vec<&str> = states.iter().filter(|x| x.state != place::NeedState::Ready).map(|x| x.need.as_str()).collect();
        let mut row = serde_json::json!({
            "node": c.node.name,
            "needs": states.iter().map(place::NeedStatus::json).collect::<Vec<_>>(),
        });
        if !c.offer.features.iter().any(|f| f == place::FEATURE_TOOLCHAIN_BUDGET) {
            row["note"] = "no tool-cache cap on this node (older k2-node)".into();
        }
        if installs.is_empty() {
            row["state"] = "ready".into();
            rows.push(row);
            continue;
        }
        let (ask, never) = place::package_gaps(&states);
        if !never.is_empty() || states.iter().any(|x| x.state == place::NeedState::UnsupportedOnOs) {
            row["state"] = "cant_install".into();
            row["note"] = format!("can't install {} there (no name for its distro, K2's deny list, or a Mac)", [never, states.iter().filter(|x| x.state == place::NeedState::UnsupportedOnOs).map(|x| x.need.clone()).collect()].concat().join(", ")).into();
            rows.push(row);
            continue;
        }
        if !ask.is_empty() {
            row["state"] = if dry { "would_ask" } else { "asked" }.into();
            row["ask"] = ask.clone().into();
            for a in ask {
                if !ask_names.contains(&a) {
                    ask_names.push(a);
                }
            }
            ask_nodes.push(c.node.clone());
            rows.push(row);
            continue;
        }
        row["installs"] = installs.clone().into();
        if dry {
            row["state"] = "would_install".into();
            rows.push(row);
            continue;
        }
        let client_id = crypto::random_id().map_err(|e| no("internal", 500, e))?;
        let mut job_body = serde_json::json!({
            "node": c.node.name,
            "argv": ["sh", "-c", "true"],
            "needs": needs,
            "noSource": true,
            "tier": "batch",
            "detach": true,
            "memBytes": PREWARM_MEM,
            "tag": tag,
        });
        job_body["effective"] = v.get("effective").cloned().unwrap_or_default();
        let request_sha = crypto::sha256_hex(&k2_core::compute::proto::canonical::to_vec(&job_body).map_err(|e| no(codes::BAD_REQUEST, 400, e))?);
        let session = who.passport().map(|(_, sid)| sid.to_string());
        match submit_one(who, &job_body, &ws, &client_id, &request_sha, &SubmitOpts { session, ..Default::default() }) {
            Ok(Submitted { job: Some(j), .. }) => {
                row["state"] = "installing".into();
                row["job"] = j.id.clone().into();
                row["short"] = j.id.chars().take(8).collect::<String>().into();
                jobs.push(j.id);
            }
            Ok(_) => row["state"] = "ready".into(),
            Err(r) => {
                row["state"] = "refused".into();
                row["error"] = serde_json::json!({"code": r.code, "hint": r.hint});
            }
        }
        rows.push(row);
    }
    if rows.is_empty() {
        return Err(no(codes::NO_NODE_FITS, 404, no_fit_hint(&ws, &want, &report)));
    }
    // One card for every name on every node that must ask. `pkgHeld` only
    // when this server's people decide part of it (a request that went only
    // to other servers' owners is no card here).
    let mut pkg_held = false;
    let request = if !dry && !ask_nodes.is_empty() {
        let session = who.passport().map(|(_, sid)| sid.to_string());
        match crate::compute_pkg::file(who, &ws, session.as_deref(), &ws.handle, &ask_nodes, &ask_names, Some("k2 compute prewarm"), None) {
            Ok(v) => {
                pkg_held = v["pkgHeld"] == true;
                v.get("request").cloned()
            }
            Err(r) => Some(serde_json::json!({"error": {"code": r.code, "hint": r.hint}})),
        }
    } else {
        None
    };
    let skipped: BTreeMap<String, u32> = report.skipped.iter().map(|(k, n)| (k.to_string(), *n)).collect();
    Ok(serde_json::json!({
        "pkgHeld": pkg_held,
        "request": request,
        "plan": dry,
        "needs": needs,
        "nodes": rows,
        "jobs": jobs,
        "fits": place::fits_words(report.cands.len() as u32, &skipped, &want),
        "effective": v.get("effective").cloned().unwrap_or_default(),
    }))
}

/// `.k2/compute.toml` and `--preset NAME` (CE-3, CE-A1): the daemon finds
/// the file under the workspace jail (the caller's checkout, the `--repo`
/// folder, the workspace folder), fills the run body where no flag set a
/// value, and records the effective settings in the body's `effective`
/// (always set here, so a client can't send its own).
fn apply_presets(ws: &Ws, v: &mut serde_json::Value) -> R<()> {
    use crate::compute_preset as preset;
    let name = s(v, "preset").map(str::to_string);
    if let Some(p) = name.as_deref().filter(|p| !preset::valid_preset_name(p)) {
        return Err(no(preset::codes::PRESET_NOT_FOUND, 400, format!("--preset {p}: a preset name is 1–40 of a-z 0-9 . _ -")));
    }
    let extra: Vec<PathBuf> = match s(v, "repo") {
        Some(_) => repo_pick(ws, v).map(|p| [p.named, Some(p.home)].into_iter().flatten().collect()).unwrap_or_default(),
        None => Vec::new(),
    };
    let loaded = preset::load(&ws.path, v, &extra).map_err(|e| no(preset::codes::COMPUTE_TOML_INVALID, 400, e))?;
    let eff = preset::apply(v, loaded.as_ref(), name.as_deref()).map_err(|(c, m)| no(c, 400, m))?;
    v["effective"] = eff;
    Ok(())
}

/// How one job is submitted (a plain run, or one shard of a split run).
#[derive(Debug, Default)]
struct SubmitOpts {
    shard: Option<store::ShardRef>,
    /// Nodes to avoid when another fits as well (sibling shards).
    prefer_not: Vec<String>,
    /// The session the wake message goes to (shards: none, the parent has it).
    session: Option<String>,
    /// `--plan`: stop before anything is stored; `Submitted::preview`.
    dry: bool,
}

struct Submitted {
    job: Option<store::JobRow>,
    warnings: Vec<String>,
    /// Where the code came from (slice A's `source` in the run response).
    source: Option<serde_json::Value>,
    preview: Option<serde_json::Value>,
    /// Job tiers: one-line notes (`{code, message, by?, jobs?}`; stderr,
    /// exit 0) and the queued jobs `--supersede` replaced.
    notices: Vec<serde_json::Value>,
    replaced: Vec<String>,
}

fn submitted_json(who: &Who, sub: &Submitted) -> serde_json::Value {
    let Some(job) = sub.job.as_ref() else { return sub.preview.clone().unwrap_or_default() };
    let mut out = serde_json::json!({ "job": job_summary(job), "existing": false, "warnings": sub.warnings, "source": sub.source,
        "notices": sub.notices, "replaced": sub.replaced, "placement": job.ctl.placement });
    if job.state == "queued" {
        if let Some(q) = queue_view(who, job, &compute_ws::decision_for(&job.node_id)) {
            out["queue"] = q;
        }
    }
    out
}

/// Check, place and store one job, then try to start it.
fn submit_one(who: &Who, v: &serde_json::Value, ws: &Ws, client_id: &str, request_sha: &str, opts: &SubmitOpts) -> R<Submitted> {
    // argv / env / cwd.
    let argv: Vec<String> = v
        .get("argv")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if argv.is_empty() || argv.len() > 256 || argv.iter().map(String::len).sum::<usize>() > 128 * 1024 || argv.iter().any(|a| a.contains('\0')) {
        return Err(no(codes::BAD_REQUEST, 400, "pass the command after --: k2 compute run [<node>] -- <command…>"));
    }
    let env: BTreeMap<String, String> = v
        .get("env")
        .and_then(|e| e.as_object())
        .map(|m| m.iter().filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string()))).collect())
        .unwrap_or_default();
    if let Err((code, name)) = penv::check_env_pairs(env.iter().map(|(k, x)| (k.as_str(), x.as_str()))) {
        return Err(no("env_refused", 400, format!("--env {name}: {code} (K2 tokens, sockets and the node's own variables can't be set)")));
    }
    let cwd = s(&v, "cwd").map(str::to_string).filter(|c| !c.is_empty() && c != ".");
    if cwd.as_deref().is_some_and(|c| !valid_cwd(c)) {
        return Err(no(codes::BAD_REQUEST, 400, "cwd must be relative to the repo root, without '..'"));
    }
    // Toolchains / system packages the job needs (compute polish v1).
    let needs = needs_from(&v)?;
    let (cpu_millis, mem_bytes) = job_resources(&v)?;
    // `--mem` is required (Rosson 10-28): without it the job reserves the
    // default (clamped to a smaller node below) and a notice says to set
    // it. The node stops a job that goes over (`failed/mem_cap`), and only
    // starts one when its slots and memory both fit.
    let mem_defaulted = mem_bytes.is_none();
    let mem_bytes = mem_bytes.or(Some(compute::DEFAULT_MEM_BYTES));
    // Files to keep after the command exits (results back, G1).
    let artifacts = artifacts_from(&v)?;
    // Vault secrets as env (secret-env SE1–SE4): names → ids now (no
    // value), env names and grant coverage once the node is known.
    let secret_asks = crate::compute_secrets::asks_from(v)?;
    let secrets_found = crate::compute_secrets::find(&ws.id, &ws.handle, who.is_admin(), &secret_asks)?;
    // `--cache NAME` (protocol 6): a folder on the node that outlives the
    // job. Never with secret env: a secret job keeps nothing between runs.
    let cache = cache_from(v)?;
    // GPU jobs (0.46.11, `--gpu`): one GPU of the node, to this job alone.
    let gpu = gpu_from(v)?;
    // GPU jobs S6 (`--gpu-mem`): its GPU memory budget, only with `--gpu`.
    let gpu_mem = gpu_mem_from(v, gpu.is_some())?;
    if cache.is_some() && !secret_asks.is_empty() {
        return Err(no(pcache::codes::CACHE_WITH_SECRET, 400, "--cache and --secret don't mix: a job with secret env keeps nothing between runs on the node"));
    }
    // Job tiers (prd-compute-job-tiers-v1, v2): `--tier` (`--gate` = ci,
    // `--priority` = priority with its `--why`), the supersede key and the
    // submitter's own name. The node decides below.
    let tier_ask = tier_from(v)?;
    let why = why_from(v, tier_ask)?;
    // `--exclusive` holds a whole node: it needs a one-line reason, shown
    // in the queue and `status` like Priority's (CE-13, D3; exit 2).
    if b(&v, "exclusive") == Some(true) && why.is_none() {
        return Err(no(
            EXCLUSIVE_NEEDS_WHY,
            400,
            "--exclusive needs a one-line reason: --exclusive --why \"benchmark needs the whole node\" (it shows in the queue; the job holds the whole node while it runs)",
        ));
    }
    // Batch is always detached (it waits for an idle node: hours, maybe).
    let detach = b(&v, "detach") == Some(true) || tier_ask == Tier::Batch;
    let supersede_key = supersede_from(v)?;
    let submitter_name = submitter_name_from(v)?;
    let tag = tag_from(v)?;
    // Never stuck (G3/G4): queue timeout, infra retries. Batch: 48 h.
    let tun = compute::retry::tuning();
    let queue_timeout = match n(&v, "queueTimeoutSecs") {
        Some(0) => None,
        Some(x) if (1..=7 * 86_400).contains(&x) => Some(x),
        Some(_) => return Err(no(codes::BAD_REQUEST, 400, "--queue-timeout is between 1s and 7d, or off")),
        None if tier_ask == Tier::Batch => Some(tiers::BATCH_QUEUE_TIMEOUT_SECS),
        None => Some(if detach { tun.queue_timeout_detached_secs } else { tun.queue_timeout_attached_secs }),
    };
    let retries = match n(&v, "retries") {
        Some(r) if (0..=compute::retry::MAX_RETRIES as i64).contains(&r) => r as u32,
        Some(_) => return Err(no(codes::BAD_REQUEST, 400, format!("--retries is 0–{}", compute::retry::MAX_RETRIES))),
        None if b(&v, "retryInterrupted") == Some(false) => 0,
        None => 1,
    };
    // Protocol 9 (`receiptV: 2`, prd-ci-on-compute-for-clients-v1 C0): the
    // node signs receipt v2 (its run facts and self-statement); placed
    // only on nodes offering `receipt-v2`. Absent = v1, as before.
    let receipt_v = match v.get("receiptV") {
        None | Some(serde_json::Value::Null) => None,
        Some(x) if x.as_u64() == Some(u64::from(k2_core::compute::proto::frames::RECEIPT_V2)) => Some(k2_core::compute::proto::frames::RECEIPT_V2),
        Some(_) => return Err(no(codes::BAD_REQUEST, 400, "receiptV is 2 (receipt v2), or absent for v1")),
    };
    // Receipt v2 is CI on compute's (0.46.10 ships it dark): a v1 submit is
    // 0.46.9's, untouched; asking for v2 needs the CI preview.
    if receipt_v.is_some() {
        ci_gate("--receipt-v2 (receiptV: 2)")?;
    }
    let stall = match n(&v, "stallSecs") {
        None => None,
        Some(0) => Some(0),
        Some(x) if (1..=7 * 86_400).contains(&x) => Some(x),
        Some(_) => return Err(no(codes::BAD_REQUEST, 400, "--stall-timeout is between 1s and 7d, or off")),
    };
    // No node named = auto-placement, with or without `any` (CP-A2): K2
    // picks a granted node; `--any` is the same thing. `--same-node-as`
    // pins like a name (CE-6): the job never moves off that node.
    let any = s(&v, "node").is_none() && s(&v, "sameNodeAs").is_none();
    // `--now` (CE-17): start now or refuse; a Batch job waits for an idle
    // node by design.
    if b(&v, "now") == Some(true) && tier_ask == Tier::Batch {
        return Err(no(codes::BAD_REQUEST, 400, "--now and --tier batch don't mix: a Batch job waits for an idle node"));
    }
    // `--avoid NODE` (0.46.8): nodes auto-placement never picks for it. A
    // name that isn't one of this workspace's nodes is a note (0.46.12).
    let (avoid, avoid_notes) = crate::compute_move::avoid_from(who, v, &ws.id)?;
    let avoid_asked = v.get("avoid").and_then(|a| a.as_array()).is_some_and(|a| !a.is_empty());
    if avoid_asked && !any {
        return Err(crate::compute_move::avoid_with_pinned(s(&v, "node").unwrap_or("--same-node-as")));
    }
    // `--only NODE` (0.46.9): the auto-placement allow list.
    let only = crate::compute_move::only_from(who, v, &ws.id)?;
    if !only.is_empty() && !any {
        return Err(no(codes::BAD_REQUEST, 400, "--only is for auto-placement; this job names a node, which pins it there. Leave the node off, or drop --only"));
    }
    let filter = store::AnyFilter {
        os: s(&v, "os").map(str::to_string),
        arch: s(&v, "arch").map(str::to_string),
        labels: v
            .get("labels")
            .and_then(|l| l.as_object())
            .map(|m| m.iter().filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string()))).collect())
            .unwrap_or_default(),
        avoid,
        distro: distro_from(v)?,
        only,
    };
    let want = crate::compute_place::Want {
        ws_id: ws.id.clone(),
        admin: who.is_admin(),
        auto: any,
        filter: filter.clone(),
        needs: needs.clone(),
        artifacts: !artifacts.is_empty(),
        exclusive: b(&v, "exclusive") == Some(true),
        needs_git: b(&v, "noSource") != Some(true),
        cpu_millis,
        mem_bytes,
        mem_defaulted,
        timeout_secs: n(&v, "timeoutSecs"),
        stall: stall.is_some_and(|x| x > 0),
        secret_ids: secrets_found.iter().map(|f| f.secret_id.clone()).collect(),
        tier: tier_ask,
        cache: cache.clone(),
        multi_repo: !with_asks(v)?.is_empty(),
        receipt_v2: receipt_v.is_some(),
        gpu,
        gpu_mem,
    };
    // Node: by name (pinned), or auto (no node named, or --any).
    let mut warnings: Vec<String> = avoid_notes;
    let (node, placement) = pick_node(who, v, ws, &want, queue_timeout, &opts.prefer_not, &mut warnings)?;
    // The node list again, whichever way the node was picked (auto never
    // offers a node off it; a pinned one was checked in pick_node).
    crate::compute_allow::check_node(ws, &node, "node")?;
    // What `.k2/compute.toml` and `--preset` set, and from where (CE-3;
    // `run` always writes it).
    let mut effective: Option<serde_json::Value> = v.get("effective").filter(|e| e.is_object()).cloned();
    // The default `--mem` never asks more than a small node has, nor more
    // than this server's share on a shared node allows per job.
    let mem_bytes = match (mem_defaulted, compute_ws::live_view(&node.id).2.or_else(|| node.last_offer.clone())) {
        (true, Some(o)) => {
            let node_cap = (o.caps.mem_bytes > 0).then_some(o.caps.mem_bytes);
            let share_cap = o.share.as_ref().filter(|v| v.shared && v.limits.max_mem_gb > 0).map(|v| u64::from(v.limits.max_mem_gb) << 30);
            Some([Some(compute::DEFAULT_MEM_BYTES), node_cap, share_cap].into_iter().flatten().min().unwrap_or(compute::DEFAULT_MEM_BYTES))
        }
        _ => mem_bytes,
    };
    if !artifacts.is_empty() && !node_features(&node).iter().any(|f| f == results::FEATURE_ARTIFACTS) {
        return Err(no(
            "node_upgrade_required",
            409,
            format!("'{}' runs an older k2-node that can't keep --artifact files; update k2-node on that machine (sudo k2-node setup), or run without --artifact", node.name),
        ));
    }
    if !secrets_found.is_empty() && !node_features(&node).iter().any(|f| f == k2_core::compute::proto::secret::FEATURE_SECRET_ENV) {
        return Err(no(
            "node_upgrade_required",
            409,
            format!(
                "'{}' can't take --secret: it needs k2-node 0.46.4 on Linux with the job wall on, and this server's trust there must be 'own' (k2 compute info {} shows its features)",
                node.name, node.name
            ),
        ));
    }
    // Env names and coverage on this node (SE2, SE4, SE7).
    let secret_envs = if secrets_found.is_empty() { Vec::new() } else { crate::compute_secrets::env_names(&ws.id, &node, &secrets_found, &env)? };
    if !opts.dry {
        if let Some(r) = crate::compute_secrets::not_granted(&secret_envs, &ws.handle, &node) {
            return Err(r);
        }
        // C3b (V25): a lane's rules where they apply, before anything is
        // queued; checked again at release.
        if !secret_envs.is_empty() {
            let offer = compute_ws::live_view(&node.id).2.or_else(|| node.last_offer.clone());
            crate::compute_secrets::gate_all(&ws.id, &node, offer.as_ref(), &secret_envs, b(&v, "exclusive") == Some(true))?;
        }
    }
    // CE-15: CARGO_BUILD_JOBS & co. = the job's CPUs (--cpus, else the
    // node's default reservation), unless the caller set one (or a secret
    // uses the name). Listed under `effective.env.cpuHints`.
    let mut env = env;
    let cpu_hints = {
        let offer = compute_ws::live_view(&node.id).2.or_else(|| node.last_offer.clone());
        let secret_names: Vec<String> = secret_envs.iter().map(|(_, name, _)| name.clone()).collect();
        crate::compute_place::set_cpu_hints(&mut env, &[], &secret_names, crate::compute_place::job_cpus(cpu_millis, offer.as_ref()))
    };
    if let Some(e) = effective.as_mut() {
        if !cpu_hints.is_empty() {
            if e["env"].is_null() {
                e["env"] = serde_json::json!({"value": [], "from": null});
            }
            e["env"]["cpuHints"] = cpu_hints.iter().map(|n| (n.clone(), serde_json::Value::String(env[n].clone()))).collect::<serde_json::Map<_, _>>().into();
        }
    }
    if cache.is_some() && !node_features(&node).iter().any(|f| f == pcache::FEATURE_JOB_CACHE) {
        return Err(no(
            pcache::codes::NODE_TOO_OLD_FOR_CACHE,
            409,
            format!(
                "'{}' runs a k2-node older than 0.46.6, which has no job caches; update it (k2 compute node update {}), pick another node, or run without --cache",
                node.name, node.name
            ),
        ));
    }
    if want.multi_repo && !node_features(&node).iter().any(|f| f == k2_core::compute::proto::frames::FEATURE_MULTI_REPO) {
        return Err(no(
            "node_upgrade_required",
            409,
            format!(
                "'{}' runs a k2-node older than 0.46.6, which can't lay out --with repos; update it (k2 compute node update {}), or pick another node",
                node.name, node.name
            ),
        ));
    }
    if want.receipt_v2 && !node_features(&node).iter().any(|f| f == k2_core::compute::proto::ci::FEATURE_RECEIPT_V2) {
        return Err(no(
            "node_upgrade_required",
            409,
            format!(
                "'{}' runs a k2-node that doesn't sign receipt v2 (it needs protocol 9); update it (k2 compute node update {}), pick another node, or run without --receipt-v2",
                node.name, node.name
            ),
        ));
    }
    if want.stall && !node_features(&node).iter().any(|f| f == k2_core::compute::proto::frames::FEATURE_STALL) {
        return Err(no(
            "node_upgrade_required",
            409,
            format!("'{}' runs an older k2-node that can't watch for stalls; update k2-node on that machine (sudo k2-node setup), or run without --stall-timeout", node.name),
        ));
    }
    if !needs.is_empty() {
        let feats = node_features(&node);
        if !feats.iter().any(|f| f == needs::FEATURE_NEEDS) {
            return Err(no(
                "node_upgrade_required",
                409,
                format!(
                    "'{}' runs an older k2-node that can't install --needs; update k2-node on that machine (sudo k2-node setup), or run without --needs",
                    node.name
                ),
            ));
        }
        if crate::compute_place::wants_components(&needs) && !feats.iter().any(|f| f == crate::compute_place::FEATURE_RUST_COMPONENTS) {
            return Err(no(
                "node_upgrade_required",
                409,
                format!(
                    "'{}' runs a k2-node older than 0.46.7, which can't install Rust components (rust@<channel>+clippy); update it (k2 compute node update {}), pick another node, or drop the +components",
                    node.name, node.name
                ),
            ));
        }
        let wants_sys = needs.iter().any(|n| n.starts_with("sys:") || n.starts_with("profile:"));
        if wants_sys && !feats.iter().any(|f| f == needs::FEATURE_SYSTEM_PACKAGES) {
            warnings.push(format!(
                "'{}' can't install system packages itself (macOS, or no sudo rule): the job fails if one is missing, with the command its owner should run",
                node.name
            ));
        }
        if crate::compute_place::wants_pkg_map(&needs) && !feats.iter().any(|f| f == k2_core::compute::proto::pkgmap::FEATURE_PKG_MAP) {
            return Err(no(
                "node_upgrade_required",
                409,
                format!(
                    "'{}' runs a k2-node older than 0.46.9, which knows no profile: or sys:<manager>:<name> needs; update it (k2 compute node update {}), pick another node, or name Debian sys: packages",
                    node.name, node.name
                ),
            ));
        }
        if crate::compute_place::wants_minimal(&needs) && !feats.iter().any(|f| f == crate::compute_place::FEATURE_RUST_DEFAULT_COMPONENTS) {
            return Err(no(
                "node_upgrade_required",
                409,
                format!("'{}' runs a k2-node older than 0.46.9, where rust@<channel> is already minimal: drop +minimal, or update it (k2 compute node update {})", node.name, node.name),
            ));
        }
        if crate::compute_place::plan_packages(&needs).len() > k2_core::compute::proto::profiles::MAX_PLAN_PACKAGES {
            return Err(no(needs::NEED_INVALID, 400, format!("at most {} system packages per job, profiles included", k2_core::compute::proto::profiles::MAX_PLAN_PACKAGES)));
        }
        // Protocol 7 (§5.3, §6.1): what this node can give. A name it can
        // never give is refused; one its owner must approve becomes a
        // request (a card for the owner) and the job isn't queued.
        if let Some(o) = compute_ws::live_view(&node.id).2.or_else(|| node.last_offer.clone()) {
            let states = crate::compute_place::need_states(&o, &crate::compute_place::expand_profiles(&needs));
            let (ask, never) = crate::compute_place::package_gaps(&states);
            if !never.is_empty() {
                let mgr = o.pkg.as_ref().map(|p| p.manager.clone()).unwrap_or_else(|| "its".into());
                return Err(no(
                    k2_core::compute::proto::pkgpolicy::codes::NO_PACKAGE_MAPPING,
                    409,
                    format!(
                        "'{}' can't install {} ({} names it nothing, or K2 never installs it). It can't be requested: use a native name (sys:{mgr}:<name>), pick another distro (--distro ubuntu), or another node",
                        node.name,
                        never.join(", "),
                        o.host.as_ref().map(|h| h.short()).unwrap_or_else(|| "this distro".into())
                    ),
                ));
            }
            if !ask.is_empty() && !opts.dry {
                let filed = crate::compute_pkg::file(who, ws, opts.session.as_deref(), &ws.handle, std::slice::from_ref(&node), &ask, Some("a job's --needs"), None)?;
                let id = filed["request"]["id"].as_str().unwrap_or_default().to_string();
                let to = if node.is_tenant() { format!("{}'s owner ({})", node.name, node.owner_label.as_deref().unwrap_or("another server")) } else { "this server's owner or an Admin".to_string() };
                return Err(no(
                    k2_core::compute::proto::pkgpolicy::codes::PACKAGE_NOT_APPROVED,
                    403,
                    format!(
                        "'{}' needs its owner's approval to install {}: request {id} is filed for {to}. Run the job again once it's installed (k2 compute node packages requests shows it)",
                        node.name,
                        ask.join(", ")
                    ),
                )
                .with("request", id)
                .with("node", node.name.clone())
                .with("packages", ask));
            }
        }
    }
    // Grant (owner/admin may run on any active node without one).
    let grant = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::grant(&conn, &node.id, &ws.id)
    };
    if grant.is_none() && !who.is_admin() {
        return Err(no(
            codes::NOT_GRANTED,
            403,
            format!("'{}' isn't granted to workspace {}: ask your owner (k2 compute grant {} --workspace {})", node.name, ws.handle, node.name, ws.handle),
        ));
    }
    // CI needs the node owner's share `gate` for this server and a grant
    // with `allow_ci` (the owner and Admin logins pass the grant check);
    // Priority with --exclusive would drain the node, so it needs CI's
    // grant too (prd-compute-job-tiers-v1 §5.1, T12, T18).
    let tier_offer = compute_ws::live_view(&node.id).2.or_else(|| node.last_offer.clone());
    let grant_ci = who.is_admin() || grant.as_ref().is_some_and(|g| g.allow_ci);
    if tier_ask == Tier::Ci && !share::share_allows_ci(tier_offer.as_ref()) {
        return Err(no(
            pshare::codes::GATE_NOT_ALLOWED,
            409,
            format!(
                "'{}': this server's share on the node doesn't allow CI jobs (gate); the node's owner allows it with k2 compute node servers {} limits <this server> --gate",
                node.name, node.name
            ),
        ));
    }
    if tier_ask == Tier::Ci && !grant_ci {
        return Err(ci_not_allowed(&node.name, &ws.handle, "--tier ci (or --gate)"));
    }
    if tier_ask == Tier::Priority && want.exclusive && !grant_ci {
        return Err(ci_not_allowed(&node.name, &ws.handle, "--tier priority with --exclusive (it would drain the whole node)"));
    }
    // `--exclusive` needs the right CI needs: the owner, an Admin, or a
    // grant with CI allowed (CE-13, D3: no new grant flag yet).
    if want.exclusive && !grant_ci {
        return Err(exclusive_needs_grant(&node.name, &ws.handle));
    }
    // GPU jobs (0.46.11): the node must run GPU jobs for this server (a GPU
    // of that vendor open to it), and the grant must say `--gpu` unless the
    // caller is the owner or an Admin. Auto-placement already kept only
    // such nodes; a named one is checked here. Never a GPU job on the CPU.
    if let Some(code) = tier_offer.as_ref().map_or(want.gpu.map(|_| crate::compute_place::skip::NO_GPU), |o| crate::compute_place::gpu_skip(&want, grant.as_ref(), o)) {
        return Err(gpu_node_refusal(code, &node.name, &ws.handle));
    }
    // Limits first, before any source is read or bundled (A8): each
    // refusal names the whole grant and the owner's command.
    let max_secs = grant.as_ref().map(|g| g.max_job_secs).unwrap_or(compute::DEFAULT_MAX_JOB_SECS);
    let timeout = n(&v, "timeoutSecs").unwrap_or(max_secs);
    if timeout < 1 || timeout > max_secs {
        let asked = human_secs(timeout.max(0));
        let hint = match &grant {
            Some(g) => format!(
                "--timeout {asked} is over this grant's {} per job on {} ({}). Ask your owner: k2 compute grant {} --workspace {} --max-time {asked}",
                human_secs(max_secs),
                node.name,
                grant_line(g),
                node.name,
                ws.handle
            ),
            None => format!("--timeout {asked} is over the default {} per job (no grant on {})", human_secs(max_secs), node.name),
        };
        return Err(no(TIMEOUT_OVER_GRANT, 400, hint));
    }
    if let Some(g) = &grant {
        let queued = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::queued_count(&conn, &node.id, &ws.id)
        };
        if queued >= g.max_queued {
            return Err(no(
                codes::QUEUE_FULL,
                409,
                format!(
                    "{queued} jobs from this workspace already wait on {} ({}). Wait for one to start (k2 compute jobs --node {} --state queued), or ask your owner: k2 compute grant {} --workspace {} --max-queued {}",
                    node.name,
                    grant_line(g),
                    node.name,
                    node.name,
                    ws.handle,
                    g.max_queued * 2
                ),
            ));
        }
    }
    // A job over the node's caps would sit at the head of its queue
    // forever and hold every later job from this workspace: refuse it now.
    let (_, _, live) = compute_ws::live_view(&node.id);
    if let Some(offer) = live.or_else(|| node.last_offer.clone()) {
        // GPU jobs S6: with its GPU memory on a unified GPU there.
        if let Some(r) = scheduler::never_fits(&offer, &probe_job(&ws, &v, cpu_millis, crate::compute_place::Want { mem_bytes, ..want.clone() }.mem_on(&offer))) {
            return Err(no(
                scheduler::EXCEEDS_NODE_CAPS,
                400,
                format!("'{}': {}; ask for less, or pick a bigger node (k2 compute nodes)", node.name, r.message),
            ));
        }
    }
    // Node sharing (protocol 3): on a shared node this server's share is
    // checked here first (exit 3; the node checks again at assign). A solo
    // node takes today's path.
    let live_offer = tier_offer;
    let queue_node = live_offer.as_ref().is_some_and(share::queue_on);
    let tiers_node = live_offer.as_ref().is_some_and(share::tiers_on);
    let tiers2_node = live_offer.as_ref().is_some_and(|o| crate::compute_place::offers_tiers2(&o.features));
    let shared_view = live_offer.as_ref().filter(|o| share::is_shared(o)).and_then(|o| o.share.clone());
    // The tier the queue orders it by: a guest's Priority where the node's
    // owner didn't allow it runs as Normal, said once (T7).
    let mut notices: Vec<serde_json::Value> = Vec::new();
    if mem_defaulted {
        let gb = mem_bytes.unwrap_or(compute::DEFAULT_MEM_BYTES) as f64 / (1u64 << 30) as f64;
        notices.push(notice(
            tiers::codes::MEM_DEFAULTED,
            format!(
                "no --mem: this job reserves {gb:.0} GB and is stopped if it uses more. Set --mem to what it needs (e.g. --mem 24G for a cargo build of K2, --mem 2G for a small script)"
            ),
        ));
    }
    // 0.46.8 (advisory): no --cpus reserves the node's CPUs ÷ its slots,
    // used or not; the next job waits for unreserved CPUs even while they
    // sit idle (11-03: k2n10 at 0/12 unreserved, ~12 idle).
    // A warning line, not a coded notice: notices are the tier/memory
    // record scripts read exactly.
    if cpu_millis.is_none() {
        if let Some(m) = crate::compute_move::cpus_hint(&node.name, live_offer.as_ref()) {
            warnings.push(m);
        }
    }
    let tier = if tier_ask == Tier::Priority && share::priority_counts(live_offer.as_ref()) == Some(false) {
        notices.push(notice(
            tiers::codes::PRIORITY_DOWNGRADED,
            format!("'{}' doesn't count this server's Priority (a guest share; its owner can allow it): this job runs as Standard there", node.name),
        ));
        Tier::Normal
    } else if tier_ask == Tier::Priority && priority_open(&ws.id, None) >= tiers::PRIORITY_CAP {
        // The per-workspace Priority cap (Rosson 10-28): more run Standard.
        notices.push(notice(
            tiers::codes::PRIORITY_CAP_REACHED,
            format!(
                "workspace {} already has {} Priority jobs queued or running (the cap): this one runs as Standard. When one ends: k2 compute retier <job> --priority --why \"…\"",
                ws.handle,
                tiers::PRIORITY_CAP
            ),
        ));
        Tier::Normal
    } else {
        tier_ask
    };
    if tier == Tier::Batch && shared_view.is_some() && !tiers2_node {
        notices.push(notice(
            tiers::codes::BATCH_IDLE_ONLY,
            format!("'{}' runs a k2-node older than 0.46.6: this Batch job waits on this server until the node is idle, then runs there as Standard", node.name),
        ));
    }
    if tier == Tier::Priority && shared_view.is_some() && !tiers_node {
        notices.push(notice(
            tiers::codes::NODE_TOO_OLD_FOR_TIERS,
            format!("'{}' runs a k2-node older than 0.46.5: it orders its queue without tiers, so Priority has no effect there (CI still goes first)", node.name),
        ));
    }
    // The class old nodes order by comes from the tier (T11): CI is
    // `gate`, everything else `interactive` (`--detach` and shard counts no
    // longer lower a job, G4), lowered to a share's top class (T9).
    let class = match &shared_view {
        Some(view) => {
            let class = share::clamp_class(&view.limits, tier.class());
            let ask = share::ShareAsk { cpu_millis, mem_bytes, max_secs: timeout as u64, exclusive: want.exclusive, class };
            if let Some((code, msg)) = share::share_refusal(&view.limits, &ask) {
                return Err(no(code, 409, format!("'{}': {msg} (k2 compute nodes shows your share)", node.name)));
            }
            class
        }
        None => tier.class(),
    };
    let est = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::median_p90(&store::recent_durations(&conn, &ws.id, &argv, Some(&node.id), 20))
            .or_else(|| store::median_p90(&store::recent_durations(&conn, &ws.id, &argv, None, 20)))
    };
    let disk_gb = grant.as_ref().map(|g| g.max_disk_gb).unwrap_or(compute::DEFAULT_MAX_DISK_GB);
    if opts.dry {
        // `--plan` checks the source exactly as `run` does (K2 Engine
        // 10-29): the repo (`--repo`, or found from your worktree), the
        // commit, `--dirty` (its size too: the blob is built, then
        // dropped) and `--with`. A source `run` would refuse fails here.
        let plan_source = if b(&v, "noSource") == Some(true) {
            if s(&v, "repo").is_some_and(|r| !r.is_empty()) || !with_asks(&v)?.is_empty() {
                return Err(no(codes::BAD_REQUEST, 400, "--repo/--with pick the code to ship; --no-source ships none (drop one)"));
            }
            None
        } else {
            let tmp = compute_ws::dirty_blob_path(&format!("plan-{}", crypto::random_id().map_err(|e| no("internal", 500, e))?));
            let r = source_for(ws, v, &tmp, &mut warnings);
            let _ = std::fs::remove_file(&tmp);
            let (_, info) = r?;
            with_for(ws, v, info.repo.as_deref().unwrap_or(""), &mut warnings)?;
            Some(info.json())
        };
        let probe_ctl = store::JobCtl { stall_secs: stall, ..Default::default() };
        let mut preview = plan_preview(&PlanFacts {
            ws,
            v,
            node: &node,
            want: &want,
            grant: grant.as_ref(),
            argv: &argv,
            timeout,
            disk_gb,
            stall: crate::compute_place::stall_for(&probe_ctl, &node_features(&node), timeout as u64),
            queue_timeout,
            retries,
            needs: &needs,
            artifacts: &artifacts,
            warnings: &warnings,
            class,
            tier,
            tier_asked: (tier != tier_ask).then_some(tier_ask),
            notices: &notices,
            shared: shared_view.as_ref(),
            secrets: &secret_envs,
            placement: &placement,
            effective: effective.as_ref(),
        });
        preview["source"] = plan_source.unwrap_or_default();
        return Ok(Submitted { job: None, warnings, source: None, preview: Some(preview), notices: Vec::new(), replaced: Vec::new() });
    }
    // Source: the workspace's own repo only (never a path the client names
    // outside it, so a passport can't ship another repo off this machine).
    let job_id = crypto::random_id().map_err(|e| no("internal", 500, e))?;
    let (src, source, repo, src_path, with) = if b(&v, "noSource") == Some(true) {
        if s(&v, "repo").is_some_and(|r| !r.is_empty()) || !with_asks(&v)?.is_empty() {
            return Err(no(codes::BAD_REQUEST, 400, "--repo/--with pick the code to ship; --no-source ships none (drop one)"));
        }
        (None, None, None, None, Vec::new())
    } else {
        let (src, info) = source_for(&ws, &v, &compute_ws::dirty_blob_path(&job_id), &mut warnings)?;
        let repo = info.repo.clone();
        let with = with_for(&ws, &v, repo.as_deref().unwrap_or(""), &mut warnings)?;
        let src_path = (!with.is_empty()).then(|| repo.clone().unwrap_or_default());
        (Some(src), Some(info), repo, src_path, with)
    };
    let cwd = if source.as_ref().is_some_and(|i| i.caller_elsewhere) { None } else { cwd };
    let ctl = store::JobCtl {
        any: any.then_some(filter),
        admin: who.is_admin(),
        timeout_secs: n(&v, "timeoutSecs"),
        queue_timeout_secs: queue_timeout,
        queued_at: compute::now(),
        retries,
        stall_secs: stall,
        shard: opts.shard.clone(),
        secrets: crate::compute_secrets::pins(&secret_envs).0,
        repo,
        why: why.clone(),
        mem_defaulted,
        placement: Some(placement.clone()),
        effective: effective.clone(),
        cpu_hints,
        tag: tag.clone(),
        ..Default::default()
    };
    let stall_secs = crate::compute_place::stall_for(&ctl, &node_features(&node), timeout as u64);
    // Who sent it (display and the Priority note; not a security boundary).
    let submitter = submitter_for(who, ws, source.as_ref(), submitter_name.as_deref());
    // The Priority courtesy (R4, §3.5): one line when another submitter on
    // this server already has Priority work queued or running. Never a
    // refusal; once per submit (a split run: its first shard only).
    if tier_ask == Tier::Priority && opts.shard.as_ref().is_none_or(|s| s.index == 1) {
        if let Some(n) = critical_path_note(&submitter) {
            notices.push(n);
        }
    }
    let plan = Plan {
        // Protocol 3 fields only to nodes offering `queue` (§10.2).
        class: queue_node.then_some(class),
        // Protocol 5: the tier only to nodes offering `tiers` (T11); Batch
        // only to one offering `tiers2` (T27).
        tier: tiers_node.then_some(tier.for_peer(tiers2_node)),
        // Protocol 6 (`tiers2`): the submitter and the grant's per-submitter
        // cap (refreshed before every send).
        submitter: (tiers2_node && queue_node).then(|| submitter.clone()),
        submitter_max_running: grant.as_ref().filter(|g| tiers2_node && queue_node && g.submitter_max_running > 0).map(|g| g.submitter_max_running as u32),
        // Protocol 7: profiles expanded, only to a `pkg-map` node.
        packages: if node_features(&node).iter().any(|f| f == k2_core::compute::proto::pkgmap::FEATURE_PKG_MAP) { crate::compute_place::plan_packages(&needs) } else { Vec::new() },
        session_id: opts.session.clone().filter(|_| queue_node),
        est_secs: est.filter(|_| queue_node).map(|(m, _)| m.max(0) as u64),
        est_p90_secs: est.filter(|_| queue_node).map(|(_, p)| p.max(0) as u64),
        ws_max_running: queue_node.then(|| grant.as_ref().map(|g| g.max_parallel.max(1) as u32).unwrap_or(1)),
        job_id: job_id.clone(),
        node_id: node.id.clone(),
        workspace_id: ws.id.clone(),
        workspace_label: ws.handle.clone(),
        requested_by: who.label(),
        argv,
        env,
        cwd,
        limits: JobLimits {
            max_secs: timeout as u64,
            cpu_millis,
            mem_bytes,
            disk_bytes: (disk_gb.max(1) as u64) << 30,
            log_cap_bytes: compute::LOG_CAP_BYTES,
        },
        src,
        exclusive: b(&v, "exclusive") == Some(true),
        created_at: compute::now(),
        needs: needs.clone(),
        artifacts,
        stall_secs,
        // Names only (SE5): the values come sealed at spawn.
        secret_env: crate::compute_secrets::pins(&secret_envs).1,
        // Protocol 8 (GH#78): filled per node at every assign
        // (`compute_ws::fit_plan_to`), from the grants live then.
        git_access: Vec::new(),
        // Protocol 9: only ever placed on nodes offering `receipt-v2`.
        receipt_v,
        // Protocol 6: only ever placed on nodes offering `job-cache` (above,
        // and `required_features` for --any and moves).
        cache: cache.clone(),
        // Protocol 6, `--with`: only on nodes offering `multi-repo`.
        src_path,
        with,
        // GPU jobs (0.46.11): only ever placed on nodes offering `gpu` for
        // this server (placement's `no_gpu` skip, the pinned check above).
        gpu: want.gpu,
        // GPU jobs S6: only an explicit `--gpu-mem`, and only ever placed
        // on nodes offering `gpu-mem` (the default is the node's own).
        gpu_mem: want.gpu_mem,
    };
    let job = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let session = opts.session.as_deref();
        let j = store::insert_job(
            &conn,
            &store::NewJob {
                plan: &plan,
                client_job_id: client_id,
                session_id: session,
                request_sha256: request_sha,
                detach,
                retry_interrupted: retries > 0,
                ctl,
            },
        )
        .map_err(|e| no("internal", 500, e))?;
        store::set_job_queue_facts(&conn, &j.id, class.as_str(), est.map(|(m, _)| m)).map_err(|e| no("internal", 500, e))?;
        let asked = (tier != tier_ask).then(|| tier_ask.as_str());
        store::set_job_tier_facts(&conn, &j.id, tier.as_str(), asked, Some(&submitter), supersede_key.as_deref()).map_err(|e| no("internal", 500, e))?;
        // Audit: every submit says its tier (a CI submit is the one that
        // jumps queues, §5.1).
        store::record_event(
            &conn,
            &who.label(),
            "job_submit",
            Some(&node.id),
            Some(&ws.id),
            Some(&j.id),
            serde_json::json!({"argv": plan.argv, "commit": plan.src.as_ref().map(|s| s.commit.clone()), "detach": detach, "needs": plan.needs,
                "source": source.as_ref().map(SourceInfo::json), "any": any, "retries": retries, "queueTimeoutSecs": queue_timeout,
                "shard": opts.shard, "secrets": plan.secret_env, "tier": tier.as_str(), "tierAsked": asked, "submitter": submitter,
                "supersede": supersede_key, "placement": placement, "effective": effective, "tag": tag}),
        );
        j
    };
    // A burst spreads: this controller's recent placements count (§4.1).
    crate::compute_place::note_placed(&node.id, compute::now());
    // `--supersede KEY`: this workspace's older jobs with the key that are
    // still queued end `cancelled/replaced`; a started one runs on (R3).
    let replaced = match supersede_key.as_deref() {
        Some(key) => supersede_older(ws, key, &job),
        None => Vec::new(),
    };
    compute_ws::kick(&node.id);
    compute_ws::notify_change();
    let job = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::job_by_id(&conn, &job.id).unwrap_or(job)
    };
    // A named node that can't start it while another could (0.46.8):
    // advisory only, the job stays where it was named.
    if s(&v, "node").is_some() {
        if let Some(h) = crate::compute_move::pinned_hint(&job, &want) {
            warnings.push(h);
        }
    }
    Ok(Submitted { job: Some(job), warnings, source: source.as_ref().map(SourceInfo::json), preview: None, notices, replaced })
}

// ── job caches (`k2 compute cache`, protocol 6) ──────────────────────

/// How long `cache` waits for each node's `cache_view`.
const CACHE_WAIT: Duration = Duration::from_secs(20);

/// This server's job caches on its nodes, newest node first: every node
/// (or `node`), every workspace (or `workspace`). With `clear`, the
/// matching caches no running job holds go first. Owner and Admin logins,
/// or a local agent under "Allow managing compute nodes"; any other agent
/// or K2 shell tab with compute on reaches only its own workspace's caches
/// (on the nodes granted to it).
fn cache_op(who: &Who, node: Option<&str>, workspace: Option<&str>, name: Option<&str>, clear: bool, all: bool, mgr: Option<&Manager>) -> R<serde_json::Value> {
    need_credential(who)?;
    // Own-workspace scope for a passport with no wider standing.
    let own = if mgr.is_some() || who.is_admin() { None } else { passport_gate(who)? };
    if own.is_none() {
        admin_or_manager(who, mgr)?;
    }
    if let Some(n) = name.filter(|n| !pcache::valid_name(n)) {
        return Err(no(pcache::codes::CACHE_INVALID, 400, format!("{n:?} isn't a cache name (1–{} of a-z 0-9 . _ -)", pcache::NAME_MAX)));
    }
    let ws = workspace.map(|w| ws_by_token_or_worktree(w).ok_or_else(|| no_workspace(w))).transpose()?;
    if let Some(o) = &own {
        own_caches_only(&ws_handle(o), o, ws.as_ref().map(|w| w.id.as_str()), all)?;
    }
    // Which workspace's caches: the passport's own when scoped; else the
    // named one; a clear naming none is the caller's own, never everyone's.
    let want_ws: Option<String> = match (&own, &ws) {
        (Some(o), _) => Some(o.clone()),
        (None, Some(w)) => Some(w.id.clone()),
        (None, None) if clear && !all => who.passport().map(|(p, _)| p.to_string()),
        (None, None) => None,
    };
    let nodes: Vec<store::NodeRow> = match node {
        // A prefix reads (CE-11); clearing needs the full name (the owner's
        // clears; an agent's reach only its own workspace's caches).
        Some(n) if !clear || own.is_some() => vec![resolve_node_for(who, Some(n), who.passport().map(|(ws, _)| ws))?],
        Some(n) => vec![node_or_404(Some(n))?],
        None => {
            let granted = own.as_deref().map(crate::compute_views::granted_node_ids);
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::list_nodes(&conn, false).into_iter().filter(|n| n.state == "active" && granted.as_ref().is_none_or(|g| g.contains(&n.id))).collect()
        }
    };
    // Which of this server's workspaces a node's `ws_key` is.
    let fp = compute::controller_key().map_err(|e| no("internal", 500, format!("controller key: {e}")))?.fingerprint();
    let names: HashMap<String, (String, String)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::list(&conn)
            .unwrap_or_default()
            .into_iter()
            .map(|p| (pcache::ws_key(&fp, &p.id), (p.id.clone(), ws_handle(&p.id))))
            .collect()
    };
    let entry = |e: &pcache::CacheEntry| {
        let (id, handle) = names.get(&e.ws_key).cloned().map(|(i, h)| (Some(i), Some(h))).unwrap_or((None, None));
        serde_json::json!({"workspace": handle, "workspaceId": id, "wsKey": e.ws_key, "name": e.name, "bytes": e.bytes, "usedAt": e.used_at, "busy": e.busy})
    };
    let mut out = Vec::new();
    for n in &nodes {
        let (online, _, live) = compute_ws::live_view(&n.id);
        let feats = live.as_ref().or(n.last_offer.as_ref()).map(|o| o.features.clone()).unwrap_or_default();
        let mut row = serde_json::json!({"node": n.name, "online": online});
        if !feats.iter().any(|f| f == pcache::FEATURE_JOB_CACHE) {
            row["supported"] = false.into();
            row["note"] = "its k2-node has no job caches (0.46.6+)".into();
            out.push(row);
            continue;
        }
        row["supported"] = true.into();
        if !online {
            row["note"] = "offline".into();
            out.push(row);
            continue;
        }
        let ask = pcache::CacheGet {
            request_id: crypto::random_id().map_err(|e| no("internal", 500, e))?,
            workspace_id: want_ws.clone(),
            clear,
            name: name.map(str::to_string),
        };
        match compute_ws::cache_get(&n.id, ask, CACHE_WAIT) {
            Ok(v) => {
                if let Some(code) = v.refused {
                    row["error"] = code.into();
                } else {
                    row["caches"] = v.caches.iter().map(entry).collect::<Vec<_>>().into();
                    if clear {
                        row["cleared"] = v.cleared.iter().map(entry).collect::<Vec<_>>().into();
                    }
                }
            }
            Err((code, hint)) => {
                row["error"] = code.into();
                row["note"] = hint.into();
            }
        }
        out.push(row);
    }
    if clear {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::record_event(
            &conn,
            &who.label(),
            "cache_clear",
            node.and_then(|_| nodes.first().map(|n| n.id.as_str())),
            want_ws.as_deref(),
            None,
            serde_json::json!({"name": name, "cleared": out.iter().map(|r| r.get("cleared").and_then(|c| c.as_array()).map(Vec::len).unwrap_or(0)).sum::<usize>()}),
        );
    }
    Ok(serde_json::json!({"nodes": out}))
}

/// `GET /cli/compute/cache?node=&workspace=&name=`.
fn caches(who: &Who, params: &HashMap<String, String>, mgr: Option<&Manager>) -> R<serde_json::Value> {
    let p = |k: &str| params.get(k).map(String::as_str).filter(|v| !v.is_empty());
    cache_op(who, p("node"), p("workspace"), p("name"), false, false, mgr)
}

/// An agent's `k2 compute cache` reaches its own workspace (`handle`,
/// id `own`) only.
fn own_caches_only(handle: &str, own: &str, named: Option<&str>, all: bool) -> R<()> {
    if all || named.is_some_and(|n| n != own) {
        return Err(no(
            codes::OWNER_ONLY,
            403,
            format!("an agent sees and clears only its own workspace's job caches ({handle}); another workspace's are for the owner, from Settings or a terminal outside K2. Run it without --workspace or --all for your own."),
        ));
    }
    Ok(())
}

/// `POST /cli/compute/cache/clear {node?, workspace?, name?, all?}`: a
/// workspace, or `all` for every workspace of this server (never by
/// accident: one of the two is required, except for a passport, whose
/// default is its own workspace).
fn cache_clear(who: &Who, body: &[u8], mgr: Option<&Manager>) -> R<serde_json::Value> {
    let v = json_body(body)?;
    let workspace = s(&v, "workspace").filter(|w| !w.is_empty());
    let all = b(&v, "all") == Some(true);
    if workspace.is_none() && !all && who.passport().is_none() {
        return Err(no(codes::BAD_REQUEST, 400, "name a workspace (--workspace WS), or pass --all for every workspace's caches"));
    }
    cache_op(who, s(&v, "node").filter(|n| !n.is_empty()), workspace, s(&v, "name").filter(|n| !n.is_empty()), true, all, mgr)
}

// ── job tiers (prd-compute-job-tiers-v1) ─────────────────────────────

/// `with` from a run request (`--with DIR[@SHA]`): `[{repo, sha?}]`.
fn with_asks(v: &serde_json::Value) -> R<Vec<(String, Option<String>)>> {
    let bad = || no(REPO_INVALID, 400, "--with DIR[@SHA]: a git repo (or a worktree of one) inside the workspace, relative to its folder, optionally at a commit");
    match v.get("with") {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(a)) => {
            if a.len() > k2_core::compute::proto::frames::MAX_WITH {
                return Err(no(REPO_INVALID, 400, format!("at most {} --with repos", k2_core::compute::proto::frames::MAX_WITH)));
            }
            a.iter()
                .map(|x| {
                    let repo = x.get("repo").and_then(|r| r.as_str()).filter(|r| !r.is_empty()).ok_or_else(bad)?;
                    let sha = match x.get("sha") {
                        None | Some(serde_json::Value::Null) => None,
                        Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s.clone()),
                        _ => return Err(bad()),
                    };
                    Ok((repo.to_string(), sha))
                })
                .collect()
        }
        Some(_) => Err(bad()),
    }
}

/// `--with` (protocol 6): the sibling repos a job ships beside its main one
/// (`main_rel`, `""` = the workspace's own repo), each at its HEAD or
/// `@SHA`, jailed like `--repo`, laid out by its folder in the workspace.
/// Only their commits go (never `--dirty`; a note when one has changes).
fn with_for(ws: &Ws, v: &serde_json::Value, main_rel: &str, warnings: &mut Vec<String>) -> R<Vec<k2_core::compute::proto::frames::SrcWith>> {
    let mut out = Vec::new();
    for (repo, sha) in with_asks(v)? {
        let n = compute::sync::nested_repo(Path::new(&ws.path), &repo).map_err(|e| no(REPO_INVALID, 400, e.replace("--repo", "--with")))?;
        if n.rel.is_empty() {
            return Err(no(REPO_INVALID, 400, format!("--with {repo} is the workspace's own repo; ship it with --repo (or run from it) and name the others with --with")));
        }
        let commit = compute::sync::resolve_commit(&n.root, sha.as_deref().unwrap_or("HEAD")).map_err(|e| no(codes::BAD_REQUEST, 400, format!("--with {repo}: {e}")))?;
        if sha.is_none() && compute::sync::is_dirty(&n.root).unwrap_or(false) {
            warnings.push(format!("--with {repo} has uncommitted changes; only its commit {} is sent (commit them to send them)", &commit[..8]));
        }
        let remote = compute::sync::origin_url(&n.root);
        out.push(k2_core::compute::proto::frames::SrcWith {
            project_key: compute::sync::project_key_in(&ws.id, remote.as_deref(), Some(&n.rel)),
            path: n.rel,
            remote_url: remote,
            commit,
        });
    }
    if !out.is_empty() {
        k2_core::compute::proto::frames::check_layout(main_rel, &out).map_err(|e| no(REPO_INVALID, 400, format!("--with: {e}")))?;
    }
    Ok(out)
}

/// `cache` from a run request: a cache name (`--cache NAME`), or none.
fn cache_from(v: &serde_json::Value) -> R<Option<String>> {
    match v.get("cache") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(n)) if pcache::valid_name(n) => Ok(Some(n.clone())),
        Some(_) => Err(no(
            pcache::codes::CACHE_INVALID,
            400,
            format!("--cache NAME: 1–{} of a-z 0-9 . _ -, starting with a letter or digit (e.g. fuzz-corpus)", pcache::NAME_MAX),
        )),
    }
}

/// `gpu` from a run request (`--gpu [any|amd|intel]`): `{"vendor": "any",
/// "count": 1}`. NVIDIA is refused until tested on real hardware.
fn gpu_from(v: &serde_json::Value) -> R<Option<k2_core::compute::proto::gpu::GpuAsk>> {
    use k2_core::compute::proto::gpu::{GpuAsk, GpuVendor};
    let g = match v.get("gpu") {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(g) => g,
    };
    let bad = || no(codes::BAD_REQUEST, 400, "--gpu is any, amd or intel (one GPU per job): gpu = {\"vendor\": \"any\", \"count\": 1}");
    let vendor = g.get("vendor").and_then(|x| x.as_str()).and_then(GpuVendor::parse).ok_or_else(bad)?;
    if vendor == GpuVendor::Nvidia {
        return Err(no(codes::BAD_REQUEST, 400, "--gpu nvidia: NVIDIA GPU jobs aren't supported yet (not tested on real hardware); use --gpu any, amd or intel"));
    }
    let count = match g.get("count") {
        None | Some(serde_json::Value::Null) => 1,
        Some(c) => c.as_u64().filter(|c| *c <= u64::from(u32::MAX)).ok_or_else(bad)? as u32,
    };
    let ask = GpuAsk { vendor, count };
    ask.check().map_err(|e| no(codes::BAD_REQUEST, 400, format!("--gpu: {e}")))?;
    Ok(Some(ask))
}

/// `gpuMemBytes` from a run request (`--gpu-mem 4G`): only with `--gpu`.
fn gpu_mem_from(v: &serde_json::Value, gpu: bool) -> R<Option<u64>> {
    let b = match v.get("gpuMemBytes") {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(x) => x.as_u64().ok_or_else(|| no(codes::BAD_REQUEST, 400, "--gpu-mem is a size: --gpu-mem 4G (gpuMemBytes, bytes)"))?,
    };
    if !gpu {
        return Err(no(codes::BAD_REQUEST, 400, "--gpu-mem needs --gpu: it is the GPU memory budget of a GPU job (k2 compute run --gpu --gpu-mem 4G --mem 8G -- <command>)"));
    }
    k2_core::compute::proto::gpu::check_gpu_mem(b).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    Ok(Some(b))
}

/// A named node that can't take a `--gpu` job (`gpu_skip`'s code): exit 3.
fn gpu_node_refusal(code: &str, node: &str, ws: &str) -> Refusal {
    use k2_core::compute::proto::gpu::codes as g;
    if code == crate::compute_place::skip::GPU_MEM_NEEDS_UPDATE {
        return no(g::GPU_MEM_NEEDS_UPDATE, 409, crate::compute_place::gpu_mem_needs_update_hint(Some(node))).with("node", node.to_string());
    }
    if code == crate::compute_place::skip::GPU_NEEDS_GRANT {
        return no(
            g::GPU_NEEDS_GRANT,
            403,
            format!("--gpu: '{node}' runs GPU jobs for this server, but workspace {ws}'s grant there doesn't allow them. The owner or an Admin: k2 compute grant {node} --workspace {ws} --gpu"),
        )
        .with("node", node.to_string());
    }
    no(
        g::NO_GPU_NODE,
        404,
        format!(
            "--gpu: '{node}' runs no GPU jobs for this server (or none of that vendor). Its owner turns them on at the machine: sudo k2-node admin gpu on --server <this server>; then: k2 compute grant {node} --workspace {ws} --gpu. Or leave the node off and K2 picks one that has a GPU for you"
        ),
    )
}

/// `tier` from a run request (`gate: true` is the old spelling of CI).
fn tier_from(v: &serde_json::Value) -> R<Tier> {
    let asked = match s(v, "tier") {
        None => None,
        Some(t) => Some(Tier::parse(t).ok_or_else(|| no(codes::BAD_REQUEST, 400, "--tier is ci, priority, standard (normal) or batch"))?),
    };
    // `priority: true` is `--priority` (0.46.6).
    let asked = match (b(v, "priority") == Some(true), asked) {
        (true, None | Some(Tier::Priority)) => Some(Tier::Priority),
        (true, Some(_)) => return Err(no(codes::BAD_REQUEST, 400, "--priority is --tier priority: drop one of them")),
        (false, t) => t,
    };
    match (b(v, "gate") == Some(true), asked) {
        (true, None | Some(Tier::Ci)) => Ok(Tier::Ci),
        (true, Some(_)) => Err(no(codes::BAD_REQUEST, 400, "--gate is --tier ci: drop one of them")),
        (false, t) => Ok(t.unwrap_or_default()),
    }
}

/// `--why "<reason>"`: required with Priority (Rosson 10-28), one line,
/// shown in the queue and `k2 compute status`. Exit 2 without it.
fn why_from(v: &serde_json::Value, tier: Tier) -> R<Option<String>> {
    let why = s(v, "why").map(str::trim).filter(|w| !w.is_empty());
    match (tier, why) {
        (_, Some(w)) if !tiers::valid_why(w) => Err(no(codes::BAD_REQUEST, 400, format!("--why is one line of 3–{} characters", tiers::WHY_MAX))),
        (Tier::Priority, None) => Err(no(
            tiers::codes::PRIORITY_NEEDS_WHY,
            400,
            "--priority needs a one-line reason: --priority --why \"release gate for 0.46.6\" (it shows in the queue; Priority is for the critical path only)",
        )),
        (_, w) => Ok(w.map(str::to_string)),
    }
}

/// Priority jobs this workspace has queued or running on this server (the
/// per-workspace cap, [`tiers::PRIORITY_CAP`]), not counting `except`.
fn priority_open(ws_id: &str, except: Option<&str>) -> usize {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM compute_jobs WHERE workspace_id = ?1 AND tier = 'priority' \
         AND state IN ('queued','assigned','preparing','running','finishing') AND id != ?2",
        rusqlite::params![ws_id, except.unwrap_or("")],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n.max(0) as usize)
    .unwrap_or(0)
}

/// `--supersede KEY` (1–120 printable characters).
fn supersede_from(v: &serde_json::Value) -> R<Option<String>> {
    match s(v, "supersede") {
        None => Ok(None),
        Some(k) if tiers::valid_supersede(k) => Ok(Some(k.to_string())),
        Some(_) => Err(no(codes::BAD_REQUEST, 400, format!("--supersede is 1–{} printable characters", tiers::SUPERSEDE_MAX))),
    }
}

/// A `--tag` outside its charset or length (exit 2).
pub(crate) const TAG_INVALID: &str = "tag_invalid";
/// Longest `--tag`.
pub(crate) const TAG_MAX: usize = 64;

/// `--tag NAME` (CE-9, D2): the job's own label, 1–64 of
/// `A-Za-z0-9 . _ : / -` (`--label k=v` is the node-label filter).
pub(crate) fn valid_tag(t: &str) -> bool {
    (1..=TAG_MAX).contains(&t.len()) && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'))
}

fn tag_from(v: &serde_json::Value) -> R<Option<String>> {
    match v.get("tag") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(t)) if valid_tag(t) => Ok(Some(t.clone())),
        Some(_) => Err(no(TAG_INVALID, 400, format!("--tag is 1–{TAG_MAX} of A-Z a-z 0-9 . _ : / - (e.g. fuzz:p3)"))),
    }
}

/// `--submitter NAME` (1–40 of a-z 0-9 . _ -).
fn submitter_name_from(v: &serde_json::Value) -> R<Option<String>> {
    match s(v, "submitter") {
        None => Ok(None),
        Some(n) if tiers::valid_submitter_name(n) => Ok(Some(n.to_string())),
        Some(_) => Err(no(codes::BAD_REQUEST, 400, format!("--submitter is 1–{} of a-z 0-9 . _ -", tiers::SUBMITTER_NAME_MAX))),
    }
}

/// Who sent a job (prd §3.3): `<workspace>/<session short>` for an agent or
/// a K2 tab, `owner` / `login:<user>` otherwise, then `/<name>` from
/// `--submitter`, else `/<worktree path>` when the run came from a git
/// worktree inside the workspace. Display and the Priority note only: an
/// agent may call itself anything inside its own workspace.
fn submitter_for(who: &Who, ws: &Ws, source: Option<&SourceInfo>, name: Option<&str>) -> String {
    let base = match who.passport() {
        Some((_, sid)) => format!("{}/{}", ws.handle, sid.chars().take(8).collect::<String>()),
        None => who.label(),
    };
    let last = name.map(str::to_string).or_else(|| source.filter(|s| s.checkout == "worktree" && !s.path.is_empty()).map(|s| s.path.clone()));
    match last {
        Some(l) => format!("{base}/{l}"),
        None => base,
    }
}

/// One notice in a run answer (a stderr line, exit 0).
fn notice(code: &str, message: String) -> serde_json::Value {
    serde_json::json!({"code": code, "message": message})
}

/// The Priority courtesy note (§3.5): another submitter on this server has
/// Priority work queued or running. `None` when nobody else does.
fn critical_path_note(me: &str) -> Option<serde_json::Value> {
    let holders = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::priority_holders(&conn)
    };
    let h = holders.into_iter().find(|h| h.submitter != me)?;
    let mut n = notice(
        tiers::codes::CRITICAL_PATH_TAKEN,
        format!("another submitter is on the critical path: {} ({} job{}). Use Normal unless you're it.", h.submitter, h.jobs, if h.jobs == 1 { "" } else { "s" }),
    );
    n["by"] = h.submitter.into();
    n["jobs"] = h.jobs.into();
    Some(n)
}

/// `ci_not_allowed` (exit 3), naming the human fix: permission cards are
/// Beta and off by default, so the text alone must say what to do (T12).
fn ci_not_allowed(node: &str, ws: &str, what: &str) -> Refusal {
    no(
        tiers::codes::CI_NOT_ALLOWED,
        403,
        format!(
            "{what} on '{node}' needs a grant with CI allowed: CI jumps every queue. The owner or an Admin turns it on: k2 compute grant {node} --workspace {ws} --ci. Otherwise run it Normal (no flag)"
        ),
    )
}

/// `--exclusive` without a reason (CE-13; exit 2).
pub(crate) const EXCLUSIVE_NEEDS_WHY: &str = "exclusive_needs_why";
/// `--exclusive` without the right (CE-13, CE-A13; exit 3). Not the share
/// code `exclusive_not_allowed`, which means the node owner's share
/// forbids it.
pub(crate) const EXCLUSIVE_NEEDS_GRANT: &str = "exclusive_needs_grant";

/// `exclusive_needs_grant` (exit 3), naming the human fix. The permission
/// card is `compute.ci node=<node>` (the grant's CI right, D3).
fn exclusive_needs_grant(node: &str, ws: &str) -> Refusal {
    no(
        EXCLUSIVE_NEEDS_GRANT,
        403,
        format!(
            "--exclusive on '{node}' needs a grant with CI allowed: an exclusive job holds the whole node while it runs. The owner or an Admin turns it on: k2 compute grant {node} --workspace {ws} --ci. Otherwise run it without --exclusive (--cpus/--mem reserve what it needs)"
        ),
    )
}

/// `--supersede KEY`: end this workspace's older jobs with the same key
/// while they are still queued, `cancelled/replaced` (in a shared node's
/// queue: an "only if not started" cancel, V57). A started job is never
/// touched (R3). The shards of one split run never replace each other, and
/// a split run replaces once (its first shard). Returns the ids replaced
/// or asked of their node.
fn supersede_older(ws: &Ws, key: &str, new_job: &store::JobRow) -> Vec<String> {
    if new_job.ctl.shard.as_ref().is_some_and(|s| s.index != 1) {
        return Vec::new();
    }
    let parent = new_job.ctl.shard.as_ref().map(|s| s.parent.clone());
    let olds = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::queued_with_supersede_key(&conn, &ws.id, key)
    };
    let mut replaced = Vec::new();
    let mut ended = Vec::new();
    for old in olds {
        if old.id == new_job.id || (parent.is_some() && old.ctl.shard.as_ref().map(|s| &s.parent) == parent.as_ref()) {
            continue;
        }
        // Decide under the placement lock on a fresh read (like cancel):
        // `kick` can't start it between the read and the write.
        let outcome = compute_ws::with_kick_lock(|| -> Option<bool> {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let cur = store::job_by_id(&conn, &old.id)?;
            let mut ctl = cur.ctl.clone();
            ctl.replaced_by = Some(new_job.id.clone());
            // The same requester replacing its own job needs no message.
            ctl.cancelled_by_requester |= cur.requested_by == new_job.requested_by;
            let event = |how: &str| {
                store::record_event(
                    &conn,
                    &new_job.requested_by,
                    "job_replaced",
                    Some(&cur.node_id),
                    Some(&cur.workspace_id),
                    Some(&cur.id),
                    serde_json::json!({"by": new_job.id, "key": key, "how": how}),
                )
            };
            if cur.state == "queued" {
                store::set_job_ctl(&conn, &cur.id, &ctl).ok()?;
                if store::replace_if_queued(&conn, &cur.id, &new_job.id, compute::now()).ok()? {
                    event("queued_here");
                    return Some(true);
                }
                return None;
            }
            let asked_already = pending::any(cur.reason.as_deref()) || cur.reason.as_deref() == Some("cancel_requested");
            if cur.state == "assigned" && cur.node_queued && !asked_already {
                store::set_job_ctl(&conn, &cur.id, &ctl).ok()?;
                let sent = compute_ws::send_cancel_if_not_started(&cur.node_id, &cur.id, cur.generation, k2_core::compute::proto::frames::CANCEL_IF_QUEUED);
                store::set_job_reason(&conn, &cur.id, Some(pending::CANCEL), None);
                event(if sent { "asked_the_node" } else { "asked_when_the_node_is_back" });
                return Some(false);
            }
            None
        });
        match outcome {
            Some(true) => {
                ended.push(old.id.clone());
                replaced.push(old.id);
            }
            Some(false) => replaced.push(old.id),
            None => {}
        }
    }
    for id in &ended {
        compute_ws::finalize(id);
    }
    replaced
}

/// What `--plan` looks at.
struct PlanFacts<'a> {
    ws: &'a Ws,
    v: &'a serde_json::Value,
    node: &'a store::NodeRow,
    want: &'a crate::compute_place::Want,
    grant: Option<&'a store::GrantRow>,
    argv: &'a [String],
    timeout: i64,
    disk_gb: i64,
    stall: Option<u64>,
    queue_timeout: Option<i64>,
    retries: u32,
    needs: &'a [String],
    artifacts: &'a [String],
    warnings: &'a [String],
    class: pshare::JobClass,
    /// The tier the queue would order it by, what was asked when lowered,
    /// and the notices a real run would print.
    tier: Tier,
    tier_asked: Option<Tier>,
    notices: &'a [serde_json::Value],
    /// This server's share, when the node is shared.
    shared: Option<&'a pshare::ShareView>,
    /// `--secret`s: (found, env name, covered on this node) (SE7).
    secrets: &'a [(crate::compute_secrets::Found, String, bool)],
    /// Where it would be placed and why (prd-compute-auto-placement-v1 §7).
    placement: &'a store::Placement,
    /// The effective settings (CE-3) and the CPU hints (CE-15).
    effective: Option<&'a serde_json::Value>,
}

/// `--plan`: the node it would pick and whether it starts now, the queue
/// in front of it, a warm slot at this commit, an estimate from earlier
/// runs of the same command, and what is left of the grant.
fn plan_preview(f: &PlanFacts<'_>) -> serde_json::Value {
    let now = compute::now();
    let n = f.node;
    let (online, route, live) = compute_ws::live_view(&n.id);
    let offer = live.or_else(|| n.last_offer.clone());
    let report = crate::compute_place::candidates_report(f.want, &[], now);
    let cand = report.cands.iter().find(|c| c.node.id == n.id);
    let starts_now = cand.is_some_and(|c| c.starts_now(f.want));
    // CE-1/CE-2: each need's state on this node, and the top three
    // candidates the ranking saw (P2).
    let need_states = match (cand, offer.as_ref()) {
        (Some(c), _) => c.needs(f.want),
        (None, Some(o)) => crate::compute_place::need_states(o, &crate::compute_place::expand_profiles(f.needs)),
        (None, None) => f
            .needs
            .iter()
            .map(|t| crate::compute_place::NeedStatus { need: t.clone(), state: crate::compute_place::NeedState::Install, have: None, native: None })
            .collect(),
    };
    let (pk, commit) = warm_key(f.ws, f.v).map(|(p, c)| (Some(p), c)).unwrap_or((None, None));
    let ranked = crate::compute_place::rank(compute::placement_policy(), &report.cands, f.want, &[], pk.as_deref(), commit.as_deref());
    let ranking = crate::compute_place::ranking_json(&ranked, f.want, pk.as_deref(), commit.as_deref());
    let decision = compute_ws::decision_for(&n.id);
    let ahead = decision.waiting.len();
    let blocking = decision.waiting.values().min_by_key(|(pos, _)| *pos).map(|(_, r)| serde_json::json!({"code": r.code, "message": r.message}));
    let (queued_ws, load) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (store::queued_count(&conn, &n.id, &f.ws.id), compute_ws::queued_view(&conn, &n.id).1)
    };
    let running_ws = load.running_by_workspace.get(&f.ws.id).copied().unwrap_or(0);
    // Warm slot: the commit this run would use, against the node's slots.
    let warm = warm_key(f.ws, f.v)
        .map(|(pk, commit)| {
            let slots: Vec<_> = offer.as_ref().map(|o| o.slots.iter().filter(|x| x.project_key == pk).cloned().collect()).unwrap_or_default();
            if commit.is_some() && slots.iter().any(|x| x.last_sha == commit) {
                "warm (a slot built this commit)"
            } else if !slots.is_empty() {
                "slot (built an earlier commit: incremental)"
            } else {
                "cold (first build of this project there)"
            }
        });
    let (here, anywhere) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (
            store::recent_durations(&conn, &f.ws.id, f.argv, Some(&n.id), 20),
            store::recent_durations(&conn, &f.ws.id, f.argv, None, 20),
        )
    };
    let estimate = if let Some((m, p)) = store::median_p90(&here) {
        Some(serde_json::json!({"medianSecs": m, "p90Secs": p, "samples": here.len(), "basis": "this command on this node"}))
    } else {
        store::median_p90(&anywhere).map(|(m, p)| serde_json::json!({"medianSecs": m, "p90Secs": p, "samples": anywhere.len(), "basis": "this command on any node"}))
    };
    let mut out = serde_json::json!({
        "plan": true,
        "node": n.name,
        "nodeId": n.id,
        "online": online,
        "route": route,
        "startsNow": starts_now,
        "queue": {"ahead": if starts_now { 0 } else { ahead }, "blockedBy": if starts_now { serde_json::Value::Null } else { blocking.unwrap_or_default() }},
        "warmSlot": warm,
        "estimate": estimate,
        "grant": f.grant.map(|g| serde_json::json!({
            "maxJobSecs": g.max_job_secs, "maxDiskGb": g.max_disk_gb,
            "parallel": {"running": running_ws, "max": g.max_parallel, "left": (g.max_parallel - running_ws as i64).max(0)},
            "queued": {"waiting": queued_ws, "max": g.max_queued, "left": (g.max_queued - queued_ws).max(0)},
        })),
        "limits": {"timeoutSecs": f.timeout, "diskGb": f.disk_gb, "stallSecs": f.stall, "queueTimeoutSecs": f.queue_timeout, "retries": f.retries},
        "needs": need_states.iter().map(crate::compute_place::NeedStatus::json).collect::<Vec<_>>(),
        "toolsLine": crate::compute_place::tools_words(&need_states),
        "ranking": ranking,
        "artifacts": f.artifacts,
        "warnings": f.warnings,
    });
    if !f.secrets.is_empty() {
        // Names and coverage only, never a value (SE7).
        out["secrets"] = crate::compute_secrets::plan_view(f.secrets);
    }
    out["placement"] = serde_json::to_value(f.placement).unwrap_or_default();
    out["effective"] = f.effective.cloned().unwrap_or_default();
    out["tier"] = f.tier.as_str().into();
    // GPU jobs (0.46.11): the ask, the GPUs there open to this server, and
    // how many are free now (root picks the device at the start).
    if let Some(g) = f.want.gpu {
        let gpus: Vec<_> = offer.as_ref().and_then(|o| o.host.as_ref()).map(|h| h.gpus.iter().filter(|x| x.jobs && g.vendor.matches(&x.vendor)).cloned().collect()).unwrap_or_default();
        out["gpu"] = serde_json::json!({"ask": g, "gpus": gpus, "free": offer.as_ref().map(|o| o.free.gpus)});
        // GPU jobs S6: its GPU memory there: the budget the node watches,
        // and what it adds to the job's memory (a unified GPU only).
        use k2_core::compute::proto::gpu as pgpu;
        let unified = offer.as_ref().and_then(|o| o.host.as_ref()).is_some_and(|h| pgpu::unified_for(&h.gpus, &g));
        let watched = offer.as_ref().is_some_and(|o| o.features.iter().any(|x| x == pgpu::FEATURE_GPU_MEM));
        out["gpu"]["mem"] = serde_json::json!({
            "unified": unified,
            "budgetBytes": pgpu::cap_bytes(Some(&g), f.want.gpu_mem, unified),
            "countedBytes": pgpu::counted_bytes(Some(&g), f.want.gpu_mem, unified),
            "watched": watched,
        });
    }
    if let Some(a) = f.tier_asked {
        out["tierAsked"] = a.as_str().into();
    }
    if !f.notices.is_empty() {
        out["notices"] = f.notices.to_vec().into();
    }
    // A shared node (§6 `--plan`): who owns it, this server's share and
    // the node's quote for this ask. Absent on solo nodes.
    if let Some(view) = f.shared {
        let q = share::quote_for(view, f.want.cpu_millis, f.want.mem_bytes);
        out["shared"] = true.into();
        out["owner"] = view.owner_label.clone().unwrap_or_else(compute_ws::controller_label).into();
        out["class"] = f.class.as_str().into();
        out["yourShare"] = serde_json::json!({
            "role": view.role.as_str(),
            "state": view.state.as_str(),
            "servers": view.servers,
            "limits": view.limits,
            "running": view.running,
            "queued": view.queued,
            "cpuSecsToday": view.cpu_secs_today,
            "expiresInSecs": view.expires_in_secs,
            "line": share::share_line(view),
        });
        out["quote"] = serde_json::json!({
            "startsNow": share::starts_now(view, f.want.cpu_millis, f.want.mem_bytes),
            "etaSecs": q.and_then(|q| q.eta_secs),
            "etaP90Secs": q.and_then(|q| q.eta_p90_secs),
            "blockedBy": q.and_then(|q| q.blocked_by.clone()),
            "quoted": q.is_some(),
        });
    }
    out
}

/// `--timeout` over the grant's (or the default) time per job.
pub(crate) const TIMEOUT_OVER_GRANT: &str = "timeout_over_grant";

/// 7200 → "2h", 5400 → "90m", 45 → "45s".
fn human_secs(s: i64) -> String {
    if s >= 3600 && s % 3600 == 0 {
        format!("{}h", s / 3600)
    } else if s >= 60 && s % 60 == 0 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

/// "grant: 2h/job, 1 parallel, 10 queued, 60 GB disk".
fn grant_line(g: &store::GrantRow) -> String {
    format!(
        "grant: {}/job, {} parallel{}, {} queued, {} GB disk",
        human_secs(g.max_job_secs),
        g.max_parallel,
        if g.submitter_max_running > 0 { format!(" ({} per submitter)", g.submitter_max_running) } else { String::new() },
        g.max_queued,
        g.max_disk_gb
    )
}

/// The project key and commit this run would build, for warm-slot
/// matching (CP-A9): `--sha`, else HEAD (`--dirty` is HEAD plus changes).
/// `None` for `--no-source`.
fn warm_key(ws: &Ws, v: &serde_json::Value) -> Option<(String, Option<String>)> {
    if b(v, "noSource") == Some(true) {
        return None;
    }
    let pick = repo_pick(ws, v).ok()?;
    let pk = pick.project_key(ws);
    let commit = compute::sync::resolve_commit(pick.named.as_ref().unwrap_or(&pick.home), s(v, "sha").unwrap_or("HEAD")).ok();
    Some((pk, commit))
}

/// The node a job goes to, and the record of why
/// (prd-compute-auto-placement-v1). A named node pins it (an ungranted
/// one is refused by the caller with a teaching `not_granted`, except for
/// the owner or an Admin); so does `--same-node-as <job>` (CE-6). No node
/// named (or `--any`): auto-placement over the nodes this workspace is
/// granted, for every caller (D1), where `--cache NAME` prefers the node
/// that holds it (CE-6).
fn pick_node(
    who: &Who,
    v: &serde_json::Value,
    ws: &Ws,
    want: &crate::compute_place::Want,
    queue_timeout: Option<i64>,
    prefer_not: &[String],
    warnings: &mut Vec<String>,
) -> R<(store::NodeRow, store::Placement)> {
    use crate::compute_place::{self as place, Tier};
    let now = compute::now();
    let same = same_node_as(ws, v)?;
    if let Some(name) = s(v, "node") {
        // The full name, or a unique prefix of a node it may see (CE-11).
        let n = resolve_node_for(who, Some(name), Some(&ws.id))?;
        if n.state != "active" {
            return Err(no(codes::NODE_NOT_FOUND, 404, format!("'{}' is waiting for its code to be confirmed", n.name)));
        }
        // The workspace's node list (0.46.8): before --now, the grant and
        // every other check, so nothing about the run is read or sent.
        crate::compute_allow::check_node(ws, &n, "node")?;
        if let Some((other, job)) = same.as_ref().filter(|(o, _)| o.id != n.id) {
            return Err(no(
                codes::BAD_REQUEST,
                400,
                format!("--same-node-as {job} ran on {}, not {}: drop the node name or --same-node-as", other.name, n.name),
            ));
        }
        if b(v, "now") == Some(true) {
            pinned_now(who, ws, want, &n, now)?;
        }
        let rec = place::pinned_record(&n.name, now);
        return Ok((n, rec));
    }
    if let Some((n, job)) = same {
        crate::compute_allow::check_node(ws, &n, &format!("--same-node-as {job}: node"))?;
        if b(v, "now") == Some(true) {
            pinned_now(who, ws, want, &n, now)?;
        }
        // CE-6: resume on the node that ran the earlier job (its checkpoint
        // and caches are there). Pinned: it waits for that node.
        let mut rec = place::pinned_record(&n.name, now);
        rec.why = format!("same node as job {job} (--same-node-as)");
        return Ok((n, rec));
    }
    let report = place::candidates_report(want, &[], now);
    // `--avoid` (0.46.8): the avoid list alone leaving nothing is its own
    // refusal (never queued on a node it avoids, never queued forever).
    if report.cands.is_empty() && !want.filter.avoid.is_empty() {
        crate::compute_move::avoid_leaves_none(ws, want, now)?;
    }
    // `--tier ci`: candidates keep only nodes where this workspace may
    // send CI; refused only when that rules out every one (T12).
    if report.cands.is_empty() && want.tier == tiers::Tier::Ci {
        let any_tier = place::Want { tier: tiers::Tier::Normal, ..want.clone() };
        if !place::candidates(&any_tier, &[], now).is_empty() {
            return Err(no(
                tiers::codes::CI_NOT_ALLOWED,
                403,
                format!(
                    "--tier ci: no node granted to workspace {} allows CI for it (a grant with --ci, and the node's share allows gate). The owner or an Admin: k2 compute grant <node> --workspace {} --ci",
                    ws.handle, ws.handle
                ),
            ));
        }
    }
    // GPU jobs (0.46.11): no node left for `--gpu` is `no_gpu_node` (or
    // `gpu_needs_grant` naming a GPU node whose grant lacks `--gpu`), never
    // a job placed without its GPU.
    if let Some((code, status, hint, node)) = place::gpu_refusal(want, &report, &ws.handle) {
        let mut r = no(code, status, hint);
        if let Some(n) = node {
            r = r.with("node", n);
        }
        return Err(r);
    }
    // `--exclusive` (CE-13): only nodes whose grant allows CI; refused
    // with the fix when that rules out every one.
    if report.cands.is_empty() && want.exclusive && report.skipped.contains_key(place::skip::EXCLUSIVE_NEEDS_GRANT) {
        return Err(no(
            EXCLUSIVE_NEEDS_GRANT,
            403,
            format!(
                "--exclusive: no node granted to workspace {} allows CI for it, which an exclusive job needs (it holds the whole node). The owner or an Admin: k2 compute grant <node> --workspace {} --ci. Otherwise run it without --exclusive",
                ws.handle, ws.handle
            ),
        ));
    }
    if report.cands.is_empty() {
        return Err(no(codes::NO_NODE_FITS, 404, no_fit_hint(ws, want, &report)));
    }
    let (pk, commit) = warm_key(ws, v).map(|(p, c)| (Some(p), c)).unwrap_or((None, None));
    let policy = compute::placement_policy();
    let ranked = place::rank(policy, &report.cands, want, prefer_not, pk.as_deref(), commit.as_deref());
    let Some(&(first, first_tier)) = ranked.first() else {
        return Err(no(
            codes::NO_NODE_FITS,
            404,
            format!(
                "every node granted to workspace {} that fits is paused, draining or unavailable right now ({}); k2 compute nodes says why",
                ws.handle,
                place::skip_words(place::skip::UNAVAILABLE, report.cands.len() as u32, want)
            ),
        ));
    };
    let (mut c, mut tier) = (first, first_tier);
    // CE-6 (v2): `--cache NAME` — the node that ran this workspace's last
    // job with it ranks first when it starts now (warmth 3); when it is
    // busy, the job waits there if that wait is under 15 min, else it goes
    // elsewhere and the record says where the cache was.
    let mut cache_why: Option<String> = None;
    let mut for_cache = false;
    if let Some(name) = want.cache.as_deref() {
        let at = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            place::cache_node(&conn, &ws.id, name).and_then(|id| store::node_by_id(&conn, &id))
        };
        match at {
            None => {
                cache_why = Some(format!("cache {name} starts empty here (no earlier job of this workspace used it)"));
                warnings.push(format!("--cache {name}: no earlier job of this workspace used it, so it starts empty on {}", c.node.name));
            }
            Some(n) if n.id == c.node.id => {
                if tier != Tier::Now {
                    cache_why = Some(format!("cache {name} was here"));
                }
            }
            Some(n) if policy == compute::PlacementPolicy::V1 => {
                let note = format!("cache {name} was on {} (placement v1 doesn't follow caches; name the node to reuse it)", n.name);
                warnings.push(format!("--cache {name}: it was on {}; this job runs on {} with an empty cache there. Name the node (or --same-node-as <job>) to reuse it", n.name, c.node.name));
                cache_why = Some(note);
            }
            Some(n) => {
                let there = ranked.iter().find(|(x, _)| x.node.id == n.id).copied();
                match there {
                    Some((x, Tier::Busy)) if b(v, "now") != Some(true) && x.wait_eta(want).is_some_and(|(e, _)| e <= place::CACHE_QUEUE_SECS) => {
                        c = x;
                        tier = Tier::Busy;
                        for_cache = true;
                    }
                    other => {
                        let state = match other {
                            Some((x, Tier::Busy)) => match x.wait_eta(want) {
                                Some((e, basis)) => format!("busy {} ({})", place::wait_words(e), if basis == "quote" { "quoted" } else { basis }),
                                None => "busy, wait unknown".to_string(),
                            },
                            Some((_, Tier::Offline)) => "offline".to_string(),
                            Some((_, Tier::Now)) => "free".to_string(),
                            None => "it can't take this job now".to_string(),
                        };
                        cache_why = Some(format!("cache {name} isn't on this node (it was on {}, {state})", n.name));
                        warnings.push(format!(
                            "--cache {name}: it was on {} ({state}); this job runs on {} with an empty cache there. To wait for it: --same-node-as <job> or name the node",
                            n.name, c.node.name
                        ));
                    }
                }
            }
        }
    }
    // `--now` (CE-17): start now or refuse, with the best queue choice;
    // nothing is stored.
    if b(v, "now") == Some(true) && tier != Tier::Now {
        return Err(no(
            NOT_NOW,
            409,
            format!(
                "--now: no node granted to workspace {} can start it now; the shortest wait is {}. Run without --now to queue there (it moves if another frees up first)",
                ws.handle,
                wait_line(c, tier, want)
            ),
        ));
    }
    if tier == Tier::Offline {
        warnings.push(format!(
            "no matching node is online: queued on {} (offline); it moves to the first matching node that comes online{}",
            c.node.name,
            queue_timeout.map(|t| format!(", or fails with queue_timeout after {}", compute::message::duration(t))).unwrap_or_default()
        ));
    }
    let ex = place::Explain { policy, cands: &report.cands, skipped: &report.skipped, project_key: pk.as_deref(), commit: commit.as_deref() };
    let mut rec = place::placement_record(c, tier, want, &ex, now);
    if for_cache {
        let name = want.cache.as_deref().unwrap_or_default();
        let wait = c.wait_eta(want).map(|(e, basis)| format!("{} ({})", place::wait_words(e), if basis == "quote" { "quoted" } else { basis })).unwrap_or_default();
        rec.code = place::QUEUED_FOR_CACHE.into();
        rec.why = format!("waits for its cache: cache {name} was on this node · about {wait} · it moves elsewhere only if the wait passes 15 min");
    } else if let Some(w) = cache_why {
        rec.why.push_str(&format!(" · {w}"));
    }
    Ok((c.node.clone(), rec))
}

/// `--now` refused: nothing could start the job at once (CE-17; exit 3).
pub(crate) const NOT_NOW: &str = "not_now";

/// "k2n10-vin, ~6 min (quoted)" · "k2n2-m1 (wait unknown)" · "k2n3-m1 (offline)".
fn wait_line(c: &crate::compute_place::Cand, tier: crate::compute_place::Tier, want: &crate::compute_place::Want) -> String {
    use crate::compute_place::{self as place, Tier};
    match (tier, c.wait_eta(want)) {
        (Tier::Offline, _) => format!("{} (offline)", c.node.name),
        (_, Some((e, basis))) => format!("{}, {} ({})", c.node.name, place::wait_words(e), if basis == "quote" { "quoted" } else { basis }),
        (_, None) => format!("{} (wait unknown)", c.node.name),
    }
}

/// `--now` on a named node (or `--same-node-as`): refused unless that node
/// starts the job now (CE-17). A node this workspace has no grant on is
/// left to the grant check (`not_granted` teaches the fix).
fn pinned_now(who: &Who, ws: &Ws, want: &crate::compute_place::Want, n: &store::NodeRow, now: i64) -> R<()> {
    use crate::compute_place::{self as place, Tier};
    let pinned = place::Want { auto: false, ..want.clone() };
    let report = place::candidates_report(&pinned, &[], now);
    let Some(c) = report.cands.iter().find(|c| c.node.id == n.id) else {
        let granted = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::grant(&conn, &n.id, &ws.id).is_some()
        };
        if !granted && !who.is_admin() {
            return Ok(());
        }
        return Err(no(NOT_NOW, 409, format!("--now: {} can't take this job now (k2 compute nodes says why). Run without --now to queue there", n.name)));
    };
    match c.tier(&pinned) {
        Some(Tier::Now) => Ok(()),
        Some(t) => Err(no(NOT_NOW, 409, format!("--now: {} can't start it now; its wait: {}. Run without --now to queue there", n.name, wait_line(c, t, &pinned)))),
        None => Err(no(NOT_NOW, 409, format!("--now: {} is paused, draining or unavailable right now. Run without --now to queue there", n.name))),
    }
}

/// `--same-node-as <job>` (CE-6): one of this workspace's jobs, and the
/// node it ran on. Another workspace's job is `job_not_found` (no leak).
fn same_node_as(ws: &Ws, v: &serde_json::Value) -> R<Option<(store::NodeRow, String)>> {
    let Some(t) = s(v, "sameNodeAs") else { return Ok(None) };
    let (job, node) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let j = store::find_job(&conn, t).filter(|j| j.workspace_id == ws.id);
        let n = j.as_ref().and_then(|j| store::node_by_id(&conn, &j.node_id));
        (j, n)
    };
    let Some(j) = job else {
        return Err(no(codes::JOB_NOT_FOUND, 404, format!("--same-node-as: no job '{t}' in workspace {} (k2 compute jobs)", ws.handle)));
    };
    let short: String = j.id.chars().take(8).collect();
    match node.filter(|n| n.state == "active") {
        Some(n) => Ok(Some((n, short))),
        None => Err(no(
            codes::NODE_NOT_FOUND,
            404,
            format!("--same-node-as {short}: the node that ran it is no longer active (k2 compute nodes)"),
        )),
    }
}

/// `no_node_fits`: why no granted node can take the job, by count
/// (granted nodes only: an agent never learns of another).
pub(crate) fn no_fit_hint(ws: &Ws, want: &crate::compute_place::Want, report: &crate::compute_place::CandReport) -> String {
    use crate::compute_place as place;
    if report.visible == 0 {
        return format!(
            "no compute node is granted to workspace {}: K2 places jobs only on granted nodes (the owner's too; the owner may still name a node to pin a job there). Ask your owner: k2 compute grant <node> --workspace {} (k2 compute nodes)",
            ws.handle, ws.handle
        );
    }
    let parts: Vec<String> = report.skipped.iter().map(|(k, n)| place::skip_words(k, *n, want)).collect();
    let mut hint = format!("no node granted to workspace {} can take this job: {}", ws.handle, parts.join(", "));
    if report.skipped.keys().all(|k| *k == place::skip::TOO_SMALL) {
        hint.push_str("; ask for less --mem/--cpus (k2 compute nodes shows each node's caps)");
    } else if report.skipped.contains_key(place::skip::TIMEOUT_OVER_GRANT) {
        hint.push_str(&format!("; ask for a shorter --timeout, or your owner: k2 compute grant <node> --workspace {} --max-time <D>", ws.handle));
    } else {
        hint.push_str("; k2 compute nodes says why");
    }
    if report.skipped.contains_key(crate::compute_allow::SKIP) {
        let names = crate::compute_allow::names_of(&ws.id).unwrap_or_default();
        hint.push_str(&format!(
            ". Jobs from {} go only to its node list: {} (k2 compute allow-nodes)",
            ws.handle,
            if names.is_empty() { "nothing".to_string() } else { names.join(", ") }
        ));
    }
    hint
}

/// `--cpus` (as millicores) and `--mem` (bytes) from a run request.
fn job_resources(v: &serde_json::Value) -> R<(Option<u64>, Option<u64>)> {
    let cpu_millis = match v.get("cpus").and_then(|x| x.as_f64()) {
        Some(c) if c > 0.0 && c <= 1024.0 => Some((c * 1000.0) as u64),
        Some(_) => return Err(no(codes::BAD_REQUEST, 400, "--cpus must be between 0 and 1024")),
        None => None,
    };
    let mem_bytes = match n(v, "memBytes") {
        Some(m) if m > 0 => Some(m as u64),
        Some(_) => return Err(no(codes::BAD_REQUEST, 400, "--mem must be positive")),
        None => None,
    };
    Ok((cpu_millis, mem_bytes))
}

/// The scheduler's view of a job not yet queued.
fn probe_job(ws: &Ws, v: &serde_json::Value, cpu_millis: Option<u64>, mem_bytes: Option<u64>) -> scheduler::QueuedJob {
    scheduler::QueuedJob {
        id: "probe".into(),
        workspace_id: ws.id.clone(),
        created_at: compute::now(),
        exclusive: b(v, "exclusive") == Some(true),
        cpu_millis,
        mem_bytes,
        disk_bytes: 0,
        needs_git: b(v, "noSource") != Some(true),
        tier: Tier::Normal, submitter: None,
        gpus: u32::from(v.get("gpu").is_some_and(|g| !g.is_null())),
    }
}

/// `--dirty` with `--sha` naming anything but the checkout's own HEAD.
pub(crate) const DIRTY_NEEDS_HEAD: &str = "dirty_needs_head";

/// Where a job's code came from, for the `run` response and the
/// `job_submit` event (not stored on the job row: no migration, and the
/// plan digest stays as it was).
#[derive(Debug, Clone)]
pub(crate) struct SourceInfo {
    /// `"worktree"` (a git worktree inside the workspace) or `"workspace"`.
    pub(crate) checkout: &'static str,
    /// The checkout, relative to the workspace folder ("" = its root).
    pub(crate) path: String,
    pub(crate) commit: String,
    /// (changed, untracked, bytes) when `--dirty` sent changes.
    pub(crate) dirty: Option<(usize, usize, u64)>,
    /// `--repo`: the nested repo (its folder in the workspace), else none.
    pub(crate) repo: Option<String>,
    /// `--repo` names a repo the caller doesn't run from: its `cwd` (a
    /// folder of another repo) is dropped.
    pub(crate) caller_elsewhere: bool,
}

impl SourceInfo {
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "checkout": self.checkout,
            "path": self.path,
            "commit": self.commit,
            "dirty": self.dirty.map(|(c, u, b)| serde_json::json!({"changed": c, "untracked": u, "bytes": b})),
            "repo": self.repo,
        })
    }
}

/// The repo a job's code comes from (`--repo`, 0.46.6).
#[derive(Debug, Clone)]
pub(crate) struct RepoPick {
    /// The repo's main checkout: bundles are built from it, and a caller's
    /// worktree of it may stand in for its HEAD.
    pub(crate) home: PathBuf,
    /// A checkout `--repo` named that isn't `home` (a worktree): its HEAD
    /// and `--dirty`, whatever folder the caller ran from.
    pub(crate) named: Option<PathBuf>,
    /// `home` relative to the workspace folder; `None` = the workspace's
    /// own repo (today's path, today's project key).
    pub(crate) rel: Option<String>,
    /// Found from the caller's folder (no `--repo`; the workspace root
    /// isn't a repo).
    pub(crate) auto: bool,
}

impl RepoPick {
    /// The warm-slot key on the node for this repo.
    pub(crate) fn project_key(&self, ws: &Ws) -> String {
        compute::sync::project_key_in(&ws.id, compute::sync::origin_url(&self.home).as_deref(), self.rel.as_deref())
    }
}

/// `--repo` refused (bad folder, escapes the jail, not a repo there).
pub(crate) const REPO_INVALID: &str = "repo_invalid";

fn nested_pick(n: compute::sync::NestedRepo, auto: bool) -> RepoPick {
    let named = (n.root != n.home).then(|| n.root.clone());
    RepoPick { home: n.home, named, rel: Some(n.rel).filter(|r| !r.is_empty()), auto }
}

/// Which repo of the workspace a job ships: `--repo <subdir>` (a nested
/// repo, jailed like a caller's worktree: `sync::nested_repo`), else the
/// workspace's own repo, else (a multi-repo workspace whose root isn't a
/// repo) the nested repo the caller runs from. Never a repo outside the
/// workspace, never `.k2/`.
pub(crate) fn repo_pick(ws: &Ws, v: &serde_json::Value) -> R<RepoPick> {
    let ws_dir = Path::new(&ws.path);
    if let Some(rel) = s(v, "repo").filter(|r| !r.is_empty()) {
        let n = compute::sync::nested_repo(ws_dir, rel).map_err(|e| no(REPO_INVALID, 400, e))?;
        return Ok(nested_pick(n, false));
    }
    match compute::sync::repo_root(ws_dir) {
        Ok(home) => Ok(RepoPick { home, named: None, rel: None, auto: false }),
        Err(_) => {
            let from_caller = s(v, "repoDir")
                .filter(|d| !d.is_empty())
                .map(|d| compute::sync::nested_repo_of_caller(ws_dir, Path::new(d)))
                .transpose()
                .map_err(|e| no(REPO_INVALID, 400, e))?
                .flatten();
            match from_caller {
                Some(n) => Ok(nested_pick(n, true)),
                None => Err(no(
                    codes::NOT_A_REPO,
                    400,
                    format!(
                        "workspace {} isn't a git repo; run from inside one of its repos, pass --repo <folder> (a git repo inside the workspace), or --no-source to run without code",
                        ws.handle
                    ),
                )),
            }
        }
    }
}

fn source_for(ws: &Ws, v: &serde_json::Value, blob_out: &Path, warnings: &mut Vec<String>) -> R<(Src, SourceInfo)> {
    let pick = repo_pick(ws, v)?;
    let ws_root = pick.home.clone();
    // Run from a git worktree inside the workspace (e.g.
    // .claude/worktrees/x): HEAD and --dirty come from that checkout, not
    // the main one. Objects are shared, so bundling by commit still works
    // from the repo's main checkout. A worktree `--repo` named itself wins.
    // `--repo` naming a repo the caller isn't in: its folder means nothing
    // there, so the job starts at the repo's root (`cwd` is dropped).
    let caller_elsewhere = pick.rel.is_some()
        && !pick.auto
        && s(v, "repoDir")
            .filter(|d| !d.is_empty())
            .is_none_or(|d| compute::sync::caller_checkout(Path::new(&ws.path), &pick.home, Path::new(d)).is_err());
    let (root, checkout) = match (&pick.named, s(v, "repoDir").filter(|d| !d.is_empty())) {
        (Some(named), _) => (named.clone(), "worktree"),
        (None, Some(d)) => match compute::sync::caller_checkout(Path::new(&ws.path), &ws_root, Path::new(d)) {
            Ok(Some(wt)) => (wt, "worktree"),
            Ok(None) => (ws_root, "workspace"),
            // Run from outside the --repo you named: no warning, that's
            // what --repo is for.
            Err(_) if caller_elsewhere => (ws_root, "workspace"),
            Err(e) => {
                let whose = match &pick.rel {
                    Some(r) => format!("repo {r}'s"),
                    None => format!("workspace {}'s", ws.handle),
                };
                warnings.push(format!("{e}; using {whose} own checkout"));
                (ws_root, "workspace")
            }
        },
        (None, None) => (ws_root, "workspace"),
    };
    if pick.auto {
        if let Some(r) = &pick.rel {
            warnings.push(format!("workspace {} isn't a git repo: shipping its repo {r} (the one you're in; --repo picks another)", ws.handle));
        }
    }
    let dirty = b(v, "dirty") == Some(true);
    let sha = s(v, "sha");
    // Every revision resolves in the checkout actually used (a worktree's
    // HEAD is its own), never the main one.
    let commit = compute::sync::resolve_commit(&root, sha.unwrap_or("HEAD")).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    let rel = {
        let base = dunce::canonicalize(&Path::new(&ws.path)).ok();
        let r = dunce::canonicalize(&root).unwrap_or_else(|_| root.clone());
        // A relative path reads with '/' on every OS (Windows: '\' → '/';
        // on Unix a '\' is part of a file name and stays).
        let slash = |s: String| if cfg!(windows) { s.replace('\\', "/") } else { s };
        base.and_then(|b| r.strip_prefix(&b).ok().map(|p| slash(p.to_string_lossy().into_owned()))).unwrap_or_else(|| r.to_string_lossy().into_owned())
    };
    if dirty && sha.is_some() {
        // --dirty is a diff against the checkout's HEAD: --sha may only
        // name that same commit.
        let head = compute::sync::resolve_commit(&root, "HEAD").map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
        if head != commit {
            let place = if rel.is_empty() { format!("workspace {}", ws.handle) } else { rel.clone() };
            return Err(no(
                DIRTY_NEEDS_HEAD,
                400,
                format!("--dirty sends {place}'s changes on top of its HEAD ({}); drop --sha or pass --sha HEAD", &head[..head.len().min(8)]),
            ));
        }
    }
    let remote = compute::sync::origin_url(&root);
    let (blob, dirty_info) = if dirty {
        let out = blob_out.to_path_buf();
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d).map_err(|e| no("internal", 500, format!("blob dir: {e}")))?;
        }
        let (sha256, bytes) = compute::sync::create_dirty_blob(&root, &out, compute::MAX_DIRTY_BYTES)
            .map_err(|e| no(codes::DIRTY_TOO_LARGE, 400, e))?;
        let (changed, untracked) = compute::sync::dirty_counts(&root).map_err(|e| no("internal", 500, e))?;
        (Some(Blob { sha256, bytes }), Some((changed, untracked, bytes)))
    } else {
        // With --sha the caller picked a commit on purpose: the working
        // tree is irrelevant, so no warning.
        if sha.is_none() && compute::sync::is_dirty(&root).unwrap_or(false) {
            warnings.push("the working tree has uncommitted changes; they are NOT sent (add --dirty to send them)".into());
        }
        (None, None)
    };
    if remote.is_none() {
        warnings.push("no fetchable https origin: the node gets history as git bundles from this server".into());
    }
    let pk = compute::sync::project_key_in(&ws.id, remote.as_deref(), pick.rel.as_deref());
    let info = SourceInfo { checkout, path: rel, commit: commit.clone(), dirty: dirty_info, repo: pick.rel, caller_elsewhere };
    Ok((Src { project_key: pk, remote_url: remote, commit, dirty: blob, slots: 1 }, info))
}

pub(crate) fn job_for(who: &Who, token: Option<&str>) -> R<store::JobRow> {
    need_credential(who)?;
    let gate = passport_gate(who)?;
    let t = token.ok_or_else(|| no(codes::BAD_REQUEST, 400, "name a job"))?;
    let j = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::find_job(&conn, t)
    }
    .ok_or_else(|| no(codes::JOB_NOT_FOUND, 404, format!("no job '{t}'")))?;
    if gate.as_deref().is_some_and(|ws| ws != j.workspace_id) {
        return Err(no(codes::JOB_NOT_FOUND, 404, format!("no job '{t}'")));
    }
    Ok(j)
}

// ── split runs (`--shards`, compute B) ───────────────────────────────

/// `run` with `shards: N`: one parent row (no node, `running` until its
/// shards end) and N `--any` child jobs, `{shard}`/`{shards}` filled in,
/// spread across nodes. One wake message for the whole run.
fn run_sharded(who: &Who, v: &serde_json::Value, ws: &Ws, client_id: &str, request_sha: &str) -> R<serde_json::Value> {
    use k2_core::compute::shards;
    let count = n(v, "shards").filter(|x| (2..=shards::MAX_SHARDS as i64).contains(x)).ok_or_else(|| {
        no(codes::BAD_REQUEST, 400, format!("--shards is 2–{} (one job per shard, each on its own node when it can)", shards::MAX_SHARDS))
    })? as u32;
    // Each shard is auto-placed (CP-A11): no `any` needed, a node refused.
    if s(v, "node").is_some() {
        return Err(no(codes::BAD_REQUEST, 400, "--shards picks a node for each shard itself: drop the node name"));
    }
    let argv: Vec<String> = v
        .get("argv")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if !shards::has_placeholders(&argv) {
        return Err(no(
            codes::BAD_REQUEST,
            400,
            "put {shard} and {shards} in the command (e.g. cargo nextest run --partition hash:{shard}/{shards}), or use k2 compute test --shards N",
        ));
    }
    let detach = b(v, "detach") == Some(true);
    let parent_id = crypto::random_id().map_err(|e| no("internal", 500, e))?;
    let now = compute::now();
    let parent_plan = Plan {
        class: None,
        session_id: None,
        est_secs: None,
        est_p90_secs: None,
        ws_max_running: None,
        job_id: parent_id.clone(),
        node_id: String::new(),
        workspace_id: ws.id.clone(),
        workspace_label: ws.handle.clone(),
        requested_by: who.label(),
        argv: argv.clone(),
        env: BTreeMap::new(),
        cwd: None,
        limits: JobLimits { max_secs: 1, cpu_millis: None, mem_bytes: None, disk_bytes: 0, log_cap_bytes: 0 },
        src: None,
        exclusive: false,
        created_at: now,
        needs: Vec::new(),
        artifacts: Vec::new(),
        stall_secs: None,
        secret_env: Vec::new(),
        tier: None,
        cache: None,
        src_path: None, submitter: None, submitter_max_running: None, packages: Vec::new(), git_access: Vec::new(), receipt_v: None, gpu: None, gpu_mem: None,
        with: Vec::new(),
    };
    let session = who.passport().map(|(_, sid)| sid.to_string());
    let parent = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let p = store::insert_job(
            &conn,
            &store::NewJob {
                plan: &parent_plan,
                client_job_id: client_id,
                session_id: session.as_deref(),
                request_sha256: request_sha,
                detach,
                retry_interrupted: false,
                ctl: store::JobCtl { admin: who.is_admin(), queued_at: now, tag: tag_from(v)?, effective: v.get("effective").filter(|e| e.is_object()).cloned(), ..Default::default() },
            },
        )
        .map_err(|e| no("internal", 500, e))?;
        store::set_parent_running(&conn, &p.id).map_err(|e| no("internal", 500, e))?;
        p
    };
    let mut kids: Vec<store::JobRow> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    // Every shard gets the run's tier; notes print once (T11).
    let mut notices: Vec<serde_json::Value> = Vec::new();
    let mut replaced: Vec<String> = Vec::new();
    let stem: String = client_id.chars().take(56).collect();
    for i in 1..=count {
        let mut cv = v.clone();
        if let Some(m) = cv.as_object_mut() {
            m.remove("shards");
            m.insert("argv".into(), serde_json::json!(shards::argv_for(&argv, i, count)));
            m.insert("clientJobId".into(), serde_json::json!(format!("{stem}-s{i}")));
        }
        let child_client = format!("{stem}-s{i}");
        let child_sha = crypto::sha256_hex(&k2_core::compute::proto::canonical::to_vec(&cv).map_err(|e| no(codes::BAD_REQUEST, 400, e))?);
        let opts = SubmitOpts {
            shard: Some(store::ShardRef { parent: parent_id.clone(), index: i, of: count }),
            prefer_not: kids.iter().map(|k| k.node_id.clone()).collect(),
            session: None,
            dry: false,
        };
        match submit_one(who, &cv, ws, &child_client, &child_sha, &opts) {
            Ok(sub) => {
                for w in sub.warnings {
                    if !warnings.contains(&w) {
                        warnings.push(w);
                    }
                }
                for n in sub.notices {
                    if !notices.iter().any(|x| x["code"] == n["code"]) {
                        notices.push(n);
                    }
                }
                replaced.extend(sub.replaced);
                kids.extend(sub.job);
            }
            Err(e) => {
                // All or nothing: stop the shards already placed (forced:
                // one may have started already).
                for k in &kids {
                    let _ = cancel_one(who, k, false);
                }
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = store::set_job_state(&conn, &parent_id, JobState::Failed, Some(e.code), Some(&format!("shard {i}/{count}: {}", e.hint)), compute::now());
                return Err(e);
            }
        }
    }
    let parent = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut ctl = parent.ctl.clone();
        ctl.shards = kids.iter().map(|k| k.id.clone()).collect();
        store::set_job_ctl(&conn, &parent_id, &ctl).map_err(|e| no("internal", 500, e))?;
        store::record_event(
            &conn,
            &who.label(),
            "job_submit_shards",
            None,
            Some(&ws.id),
            Some(&parent_id),
            serde_json::json!({"argv": argv, "shards": count, "children": ctl.shards}),
        );
        store::job_by_id(&conn, &parent_id).unwrap_or(parent)
    };
    // A shard may have ended before the parent knew its shards.
    crate::compute_shards::child_ended(&parent_id);
    compute_ws::notify_change();
    let list: Vec<serde_json::Value> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        kids.iter()
            .map(|k| {
                let k = store::job_by_id(&conn, &k.id).unwrap_or_else(|| k.clone());
                serde_json::json!({
                    "index": k.ctl.shard.as_ref().map(|s| s.index),
                    "job": k.id,
                    "short": compute::message::short(&k.id),
                    "node": store::node_by_id(&conn, &k.node_id).map(|n| n.name),
                    "state": k.state,
                })
            })
            .collect()
    };
    Ok(serde_json::json!({ "job": job_summary(&parent), "existing": false, "warnings": warnings, "shards": list, "notices": notices, "replaced": replaced }))
}

/// Fetch failure codes as the stable `&'static str` refusals carry.
fn fetch_code(code: &str) -> &'static str {
    match code {
        "node_offline" => codes::NODE_OFFLINE,
        "node_upgrade_required" => "node_upgrade_required",
        "relay_cap" => "relay_cap",
        "artifact_timeout" => "artifact_timeout",
        "artifact_gone" => "artifact_gone",
        "artifact_changed" => "artifact_changed",
        "artifact_not_found" => "artifact_not_found",
        "local_write" => "local_write",
        _ => "artifact_unavailable",
    }
}

/// `GET /cli/compute/jobs/artifact?job=…[&name=…&offset=…&max=…]`.
/// Without `name`: the kept artifacts. With it: pull the file from the
/// node into this server's cache once (blocking), then a base64 slice
/// at `offset`. The CLI loops until `eof`.
fn job_artifact(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    let j = job_for(who, params.get("job").map(String::as_str))?;
    if !j.ctl.shards.is_empty() {
        // A split run: every shard's files as `shard-<i>/<name>`.
        let kids = crate::compute_shards::children(&j);
        let name = params.get("name").map(String::as_str).filter(|n| !n.is_empty());
        let Some(name) = name else {
            let mut all = Vec::new();
            for (k, _) in &kids {
                let i = k.ctl.shard.as_ref().map(|s| s.index).unwrap_or(0);
                for a in compute::result::outputs(k).unwrap_or_default().artifacts {
                    all.push(serde_json::json!({"name": format!("shard-{i}/{}", a.name), "sha256": a.sha256, "bytes": a.bytes}));
                }
            }
            return Ok(serde_json::json!({"job": j.id, "short": compute::message::short(&j.id), "state": j.state, "artifacts": all}));
        };
        let (prefix, rest) = name.split_once('/').unwrap_or((name, ""));
        let idx: Option<u32> = prefix.strip_prefix("shard-").and_then(|x| x.parse().ok());
        let child = kids.iter().find(|(k, _)| k.ctl.shard.as_ref().map(|s| s.index) == idx).map(|(k, _)| k.clone());
        let Some(child) = child.filter(|_| !rest.is_empty()) else {
            return Err(no("artifact_not_found", 404, format!("a split run's files are shard-<i>/<name> (k2 compute fetch {} lists them)", compute::message::short(&j.id))));
        };
        let mut p = params.clone();
        p.insert("job".into(), child.id.clone());
        p.insert("name".into(), rest.to_string());
        let mut out = job_artifact(who, &p)?;
        out["job"] = j.id.clone().into();
        out["name"] = name.into();
        return Ok(out);
    }
    let o =compute::result::outputs(&j).unwrap_or_default();
    let short = compute::message::short(&j.id).to_string();
    let Some(name) = params.get("name").map(String::as_str).filter(|n| !n.is_empty()) else {
        return Ok(serde_json::json!({
            "job": j.id,
            "short": short,
            "generation": j.generation,
            "state": j.state,
            "artifacts": o.artifacts,
        }));
    };
    let info = o.artifacts.iter().find(|a| a.name == name).ok_or_else(|| {
        no("artifact_not_found", 404, format!("job {short} kept no artifact named '{name}' (k2 compute fetch {short} lists them)"))
    })?;
    // GH#80: no deadline for the whole file; the pull fails when the node
    // goes quiet (a slow relay is slow, not stuck).
    let timeout = compute_ws::PULL_IDLE;
    // `over_relay=1`: the caller means to pull a file over the relay cap
    // through the relay (up to the LAN cap). `wait=<s>`: don't hold the
    // request for the whole transfer; answer `pending` after that long
    // and let the caller ask again (old CLIs send neither and block).
    let over_relay = params.get("over_relay").is_some_and(|v| v == "1" || v == "true");
    let wait = params.get("wait").and_then(|w| w.parse::<u64>().ok()).map(|w| Duration::from_secs(w.min(25)));
    let fail = |(code, hint): (String, String)| {
        let c = fetch_code(&code);
        let status = match c {
            "artifact_gone" | "artifact_not_found" => 410,
            "artifact_timeout" | "artifact_unavailable" => 503,
            _ => 409,
        };
        no(c, status, hint)
    };
    let path = match wait {
        None => compute_ws::fetch_artifact(&j, info, over_relay, timeout).map_err(fail)?,
        Some(w) => match compute_ws::fetch_artifact_wait(&j, info, over_relay, timeout, w).map_err(fail)? {
            compute_ws::Fetched::Ready(p) => p,
            compute_ws::Fetched::Pending { got } => {
                return Ok(serde_json::json!({
                    "job": j.id,
                    "name": info.name,
                    "sha256": info.sha256,
                    "bytes": info.bytes,
                    "pending": true,
                    "got": got,
                }));
            }
        },
    };
    let offset = params.get("offset").and_then(|x| x.parse::<u64>().ok()).unwrap_or(0);
    let max = params.get("max").and_then(|x| x.parse::<usize>().ok()).unwrap_or(4 << 20).clamp(64 << 10, 8 << 20);
    let mut f = std::fs::File::open(&path).map_err(|e| no("local_write", 500, format!("cache: {e}")))?;
    use std::io::{Read, Seek, SeekFrom};
    f.seek(SeekFrom::Start(offset.min(info.bytes))).map_err(|e| no("local_write", 500, format!("cache: {e}")))?;
    let mut buf = Vec::with_capacity(max.min(info.bytes as usize + 1));
    f.take(max as u64).read_to_end(&mut buf).map_err(|e| no("local_write", 500, format!("cache: {e}")))?;
    let end = offset.min(info.bytes) + buf.len() as u64;
    Ok(serde_json::json!({
        "job": j.id,
        "name": info.name,
        "sha256": info.sha256,
        "bytes": info.bytes,
        "offset": offset,
        "data": crypto::b64(&buf),
        "eof": end >= info.bytes,
    }))
}

fn job_logs(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    let j = job_for(who, params.get("job").map(String::as_str))?;
    compute_ws::touch_follower(&j.id);
    if !j.ctl.shards.is_empty() {
        let shards: Vec<String> = j.ctl.shards.clone();
        return Ok(serde_json::json!({
            "job": job_summary(&j),
            "generation": j.generation,
            "cursor": 0,
            "chunks": [],
            "end": j.is_terminal(),
            "shards": shards,
            "hint": "a split run: each shard has its own log (k2 compute logs <run> reads them all)",
        }));
    }
    let generation = params.get("generation").and_then(|g| g.parse::<i64>().ok()).unwrap_or(j.generation);
    let mut cursor = match params.get("cursor").and_then(|c| c.parse::<u64>().ok()) {
        Some(c) => c,
        None => logs::cursor_after_seq(&j.id, generation, params.get("since_seq").and_then(|s| s.parse().ok()).unwrap_or(0)),
    };
    let max = params.get("max").and_then(|m| m.parse::<usize>().ok()).unwrap_or(256 * 1024).clamp(1024, 1 << 20);
    let wait = params.get("wait").and_then(|w| w.parse::<u64>().ok()).unwrap_or(0).min(25);
    let deadline = Instant::now() + Duration::from_secs(wait);
    loop {
        let seen = compute_ws::change_counter();
        let (recs, next) = logs::read_from(&j.id, generation, cursor, max);
        let row = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::job_by_id(&conn, &j.id)
        }
        .ok_or_else(|| no(codes::JOB_NOT_FOUND, 404, "the job vanished"))?;
        let terminal = row.is_terminal() || row.generation != generation;
        let now = Instant::now();
        if !recs.is_empty() || terminal || now >= deadline {
            cursor = next;
            let chunks: Vec<serde_json::Value> = recs
                .iter()
                .map(|r| serde_json::json!({"seq": r.seq, "stream": logs::stream_name(r.stream), "data": crypto::b64(&r.data)}))
                .collect();
            let more = !logs::read_from(&j.id, generation, cursor, 1).0.is_empty();
            let mut out = serde_json::json!({
                "job": job_summary(&row),
                "generation": generation,
                "cursor": cursor,
                "chunks": chunks,
                "end": terminal && !more,
                "nodeOnline": compute_ws::is_online(&row.node_id),
            });
            // The wait line a follower prints (L6): why it waits and where,
            // on this server or in the node's own queue.
            if row.state == "queued" {
                if let Some(q) = queue_view(who, &row, &compute_ws::decision_for(&row.node_id)) {
                    out["queue"] = q;
                }
            } else if row.node_queued {
                out["queue"] = out["job"]["queue"].clone();
            }
            return Ok(out);
        }
        compute_ws::wait_change(seen, deadline - now);
    }
}

fn job_receipt(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    let j = job_for(who, params.get("job").map(String::as_str))?;
    let Some(raw) = j.receipt_json.as_deref() else {
        return Err(no("no_receipt", 404, "no receipt yet (the node sends it when the job ends)"));
    };
    let signed: k2_core::compute::proto::frames::SignedReceipt =
        serde_json::from_str(raw).map_err(|e| no("internal", 500, format!("stored receipt: {e}")))?;
    let body: serde_json::Value = serde_json::from_str(&signed.body).unwrap_or_default();
    // The node's public key as this server pinned it at enrollment. Display
    // only: whoever checks the receipt elsewhere (the release signer, S3b)
    // pins node keys itself and never trusts this field.
    let node_key = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, &j.node_id).map(|n| n.public_key_pem)
    };
    let mut out = serde_json::json!({
        "job": compute::message::short(&j.id),
        "verified": j.receipt_ok == Some(true),
        "receipt": body,
        // The exact bytes the node signed (canonical JSON), so a checker
        // never has to re-serialize `receipt` to verify `signature`.
        "body": signed.body,
        "signature": signed.sig,
        "node_public_key_pem": node_key,
        // The plan the node ran (its canonical JSON hashes to the receipt's
        // `plan_digest`), so a checker sees every input the receipt only
        // digests: --with, cwd, --cache, needs, env. Env VALUES are in it
        // (as in every plan); vault secrets never are (names only).
        "plan": j.plan().and_then(|p| serde_json::to_value(p).ok()),
        "proves": "which node said it ran which inputs and got which outputs — not that the node is honest",
    });
    // SE8/SE29: the env names the node says the job got from the vault.
    if let Some(s) = body.get("secret_env").filter(|s| s.as_array().is_some_and(|a| !a.is_empty())) {
        out["secrets"] = s.clone();
    }
    // Protocol 9, receipt v2: the receipt's version, and the node's signed
    // self-statement it names (`run.statement_digest`), exactly as the node
    // signed it, for the evidence bundle (`docs/ci-formats-v1.md` §2, §8).
    // A v1 receipt's answer is unchanged (S3b reads it).
    if body.get("v").and_then(|v| v.as_u64()) == Some(u64::from(k2_core::compute::proto::frames::RECEIPT_V2)) {
        out["v"] = serde_json::json!(k2_core::compute::proto::frames::RECEIPT_V2);
        let stored: serde_json::Value = serde_json::from_str(raw).unwrap_or_default();
        out["statement"] = stored.get("statement").cloned().unwrap_or(serde_json::Value::Null);
    }
    // C1: the node's owner revoked this key in its roster: the receipt is
    // void (compromised: every receipt, past and future). Absent otherwise,
    // so a v1 answer is unchanged.
    if let Some(fp) = body.get("node_fp").and_then(|v| v.as_str()) {
        let started = body.get("started_at").and_then(|v| v.as_i64());
        if let Some(why) = crate::compute_trust::receipt_void(fp, started) {
            out["voided"] = why.into();
        }
    }
    Ok(out)
}

/// `POST /cli/compute/jobs/tier` (`k2 compute retier <job> --tier T`,
/// prd-compute-job-tiers-v2 L2, T29): change a QUEUED job's tier. Same
/// checks as submit (CI needs `allow_ci` and the share's gate, Priority
/// needs `--why` and room under the workspace's Priority cap). A job
/// waiting on this server is updated and placed again; one in a shared
/// node's queue is re-sent to the same node with the new tier (it gets a
/// new arrival there: moving down loses its place, moving up doesn't need
/// it). A started job is left alone (409 `job_started`). Who: an agent for
/// its own workspace's jobs; the owner and Admin logins for any job.
fn job_retier(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    let v = json_body(body)?;
    if who.passport().is_none() && !who.is_admin() {
        return Err(no(codes::OWNER_ONLY, 403, "only the job's own workspace, the owner or an Admin changes a job's tier"));
    }
    let j = job_for(who, s(&v, "job"))?;
    if s(&v, "tier").is_none() && b(&v, "priority") != Some(true) && b(&v, "gate") != Some(true) {
        return Err(no(codes::BAD_REQUEST, 400, "name a tier: k2 compute retier <job> --tier standard|priority|batch|ci (--priority needs --why \"…\")"));
    }
    let tier = tier_from(&v)?;
    let why = why_from(&v, tier)?;
    let short = compute::message::short(&j.id);
    let on_node = j.node_queued && j.state == "assigned";
    if j.state != "queued" && !on_node {
        return Err(no(
            pshare::codes::JOB_STARTED,
            409,
            format!("job {short} is {}: only a queued job changes tier (a running job is never stopped or reordered)", if j.node_queued { "being answered by its node" } else { j.state.as_str() }),
        ));
    }
    if !j.ctl.shards.is_empty() {
        return Err(no(codes::BAD_REQUEST, 400, format!("{short} is a split run: retier its shards (k2 compute get {short} lists them)")));
    }
    let node = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, &j.node_id)
    }
    .ok_or_else(|| no(codes::NODE_NOT_FOUND, 404, "the job's node is gone"))?;
    let offer = compute_ws::live_view(&node.id).2.or_else(|| node.last_offer.clone());
    let grant = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::grant(&conn, &node.id, &j.workspace_id)
    };
    let ws = ws_handle(&j.workspace_id);
    if tier == Tier::Ci {
        if !share::share_allows_ci(offer.as_ref()) {
            return Err(no(pshare::codes::GATE_NOT_ALLOWED, 409, format!("'{}': this server's share on the node doesn't allow CI jobs (gate)", node.name)));
        }
        if !(who.is_admin() || grant.as_ref().is_some_and(|g| g.allow_ci)) {
            return Err(ci_not_allowed(&node.name, &ws, "--tier ci"));
        }
    }
    if tier == Tier::Priority && j.tier() != Tier::Priority && priority_open(&j.workspace_id, Some(&j.id)) >= tiers::PRIORITY_CAP {
        return Err(no(
            tiers::codes::PRIORITY_CAP_REACHED,
            409,
            format!("workspace {ws} already has {} Priority jobs queued or running (the cap); retier one of them down, or wait for one to end", tiers::PRIORITY_CAP),
        ));
    }
    // A guest share where this server's Priority doesn't count runs it as
    // Standard there, like at submit.
    let tier_set = if tier == Tier::Priority && share::priority_counts(offer.as_ref()) == Some(false) { Tier::Normal } else { tier };
    let class = match offer.as_ref().filter(|o| share::is_shared(o)).and_then(|o| o.share.as_ref()) {
        Some(view) => share::clamp_class(&view.limits, tier_set.class()),
        None => tier_set.class(),
    };
    let from = j.tier();
    let changed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let n = conn
            .execute(
                "UPDATE compute_jobs SET tier = ?2, tier_asked = ?3, class = ?4 WHERE id = ?1 AND generation = ?5 AND state IN ('queued','assigned')",
                rusqlite::params![j.id, tier_set.as_str(), (tier_set != tier).then(|| tier.as_str()), class.as_str(), j.generation],
            )
            .map_err(|e| no("internal", 500, e.to_string()))?;
        if n == 1 {
            let mut ctl = j.ctl.clone();
            ctl.why = if tier == Tier::Priority { why.clone() } else { None };
            store::set_job_ctl(&conn, &j.id, &ctl).map_err(|e| no("internal", 500, e))?;
            store::record_event(
                &conn,
                &who.label(),
                "job_retier",
                Some(&node.id),
                Some(&j.workspace_id),
                Some(&j.id),
                serde_json::json!({"from": from.as_str(), "to": tier_set.as_str(), "asked": tier.as_str(), "why": why, "onNode": on_node}),
            );
        }
        n == 1
    };
    if !changed {
        return Err(no(pshare::codes::JOB_STARTED, 409, format!("job {short} started (or ended) just now; its tier stays {}", from.word())));
    }
    // Waiting here: the next pass orders it by the new tier. In a node's
    // queue: give it back and send it again (T24's marker; no move).
    let resent = if on_node { compute_ws::resend_queued(&node.id, &j.workspace_id, Some(std::slice::from_ref(&j.id))) } else { 0 };
    compute_ws::kick(&node.id);
    compute_ws::notify_change();
    Ok(serde_json::json!({
        "ok": true, "job": short, "from": from.as_str(), "tier": tier_set.as_str(), "asked": tier.as_str(), "node": node.name,
        "resent": resent > 0, "why": why,
        "note": if on_node { "re-sent to the node with the new tier: it gets a new place in the node's queue" } else { "it waits on this server under the new tier" },
    }))
}

fn job_retry(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    let v = json_body(body)?;
    let j = job_for(who, s(&v, "job"))?;
    if !j.is_terminal() {
        let short = compute::message::short(&j.id);
        return Err(no("still_running", 409, format!("the job hasn't ended; k2 compute cancel {short} --force first")));
    }
    if !j.ctl.shards.is_empty() {
        // Re-run only the shards that didn't pass; the run reopens.
        let mut again = Vec::new();
        for (k, _) in crate::compute_shards::children(&j) {
            if !(k.state == "done" && k.exit_code == Some(0)) {
                retry_one(who, &k)?;
                again.push(k.ctl.shard.as_ref().map(|s| s.index).unwrap_or(0));
            }
        }
        if again.is_empty() {
            return Err(no("nothing_to_retry", 409, "every shard passed"));
        }
        let r = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::set_parent_running(&conn, &j.id).map_err(|e| no("internal", 500, e))?;
            store::record_event(&conn, &who.label(), "job_retry", None, Some(&j.workspace_id), Some(&j.id), serde_json::json!({"shards": again}));
            store::job_by_id(&conn, &j.id).unwrap_or(j)
        };
        compute_ws::notify_change();
        return Ok(serde_json::json!({ "ok": true, "job": job_summary(&r), "shards": again }));
    }
    let r = retry_one(who, &j)?;
    Ok(serde_json::json!({ "ok": true, "job": job_summary(&r) }))
}

/// A new attempt of one ended job. An `--any` job whose node is gone
/// still retries: the sweeper moves it.
fn retry_one(who: &Who, j: &store::JobRow) -> R<store::JobRow> {
    let n = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, &j.node_id)
    };
    let live = n.clone().filter(|n| n.state == "active");
    if live.is_none() && j.ctl.any.is_none() {
        return Err(no(codes::NODE_NOT_FOUND, 404, "the job's node was removed; submit a new run"));
    }
    if let (Some(n), true) = (&live, who.passport().is_some()) {
        node_visible(who, n)?;
    }
    // The workspace's node list (0.46.8): a retry runs where the job ran,
    // so a node that left the list since is refused (a new run is placed
    // on the list).
    if let Some(n) = &live {
        let ws = ws_by_id(&j.workspace_id).ok_or_else(|| no("workspace_not_found", 404, "the job's workspace is gone"))?;
        let short = compute::message::short(&j.id);
        crate::compute_allow::check_node(&ws, n, &format!("retry {short}: it ran on node"))?;
    }
    let r = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let r = store::requeue_new_attempt(&conn, &j.id, "retry").map_err(|e| no("internal", 500, e))?;
        let mut ctl = r.ctl.clone();
        ctl.queued_at = compute::now();
        ctl.not_before = None;
        ctl.retries_used = 0;
        let _ = store::set_job_ctl(&conn, &r.id, &ctl);
        let r = store::job_by_id(&conn, &r.id).unwrap_or(r);
        store::record_event(&conn, &who.label(), "job_retry", Some(&j.node_id), Some(&j.workspace_id), Some(&j.id), serde_json::json!({"generation": r.generation}));
        r
    };
    compute_ws::kick(&r.node_id);
    compute_ws::notify_change();
    Ok(r)
}

fn usage(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    need_admin(who)?;
    let days = params.get("days").and_then(|d| d.parse::<i64>().ok()).unwrap_or(7).clamp(1, 90);
    let since = (chrono::Utc::now() - chrono::Duration::days(days - 1)).format("%Y-%m-%d").to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    let rows: Vec<serde_json::Value> = store::usage(&conn, &since)
        .into_iter()
        .map(|u| {
            let mut v = serde_json::to_value(&u).unwrap_or_default();
            v["node"] = store::node_by_id(&conn, &u.node_id).map(|n| n.name).into();
            v
        })
        .collect();
    Ok(serde_json::json!({ "since": since, "usage": rows, "metered": "relay bytes count toward a future budget; LAN and tailnet are never metered" }))
}

fn events(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    need_admin(who)?;
    let limit = params.get("limit").and_then(|l| l.parse().ok()).unwrap_or(100);
    let db = k2_core::db::shared();
    let conn = db.lock();
    Ok(serde_json::json!({ "events": store::events(&conn, limit) }))
}

// ── this computer ────────────────────────────────────────────────────

fn local_status(who: &Who) -> R<serde_json::Value> {
    need_credential(who)?;
    // Passports (agents) see the node's state, not the machine owner's
    // binary paths, home check or install route.
    if who.passport().is_some() {
        return Ok(compute::local::status());
    }
    Ok(crate::compute_local::status())
}

/// The machine owner's word: owner only, never a passport (Rosson
/// 2026-10-08: agents never pause or resume a node).
fn local_control(who: &Who, body: &[u8], action: &str) -> R<serde_json::Value> {
    need_owner(who)?;
    let v = json_body(body)?;
    let state = match action {
        "pause" if b(&v, "now") == Some(true) => Control::Stopped,
        "pause" => Control::Paused,
        "drain" => Control::Draining,
        _ => Control::Active,
    };
    compute::local::set_control(state, &who.label()).map_err(|e| no("local_write", 409, e))?;
    Ok(serde_json::json!({"ok": true, "control": state.as_str(),
        "note": match state {
            Control::Stopped => "running jobs are stopped now and new ones are refused",
            Control::Paused => "running jobs finish; new ones are refused",
            Control::Draining => "running jobs finish; new ones are refused until you resume",
            Control::Active => "the node takes jobs again",
        }}))
}

fn local_policy(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_owner(who)?;
    let v = json_body(body)?;
    let set = v.get("set").and_then(|s| s.as_object()).ok_or_else(|| no(codes::BAD_REQUEST, 400, "pass {\"set\": {key: value}}"))?;
    let updates: Vec<(String, String)> = set
        .iter()
        .map(|(k, x)| (k.clone(), x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())))
        .collect();
    // A walled local node (0.46.3+, `network_none` in its status) enforces
    // it; an older one would take no jobs, so it stays refused there.
    if updates.iter().any(|(k, v)| k == "network" && v.trim() == "none") && !compute::local::local_node_enforces_network_none() {
        use k2_core::compute::proto::refusal::{NETWORK_NONE_MESSAGE, NETWORK_NONE_NOT_ENFORCED};
        return Err(no(NETWORK_NONE_NOT_ENFORCED, 400, NETWORK_NONE_MESSAGE));
    }
    let t = compute::local::set_policy(&updates).map_err(|e| no("local_write", 400, e))?;
    Ok(serde_json::json!({"ok": true, "policy": serde_json::to_value(t).unwrap_or_default()}))
}

/// "Make this computer a compute node" (compute polish v1): install once
/// (root helper → passwordless sudo → macOS prompt → a command to copy),
/// then enroll with a code or a one-click ticket. Owner only, never a
/// passport (a K2 terminal included).
fn local_enable(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_owner(who)?;
    let v = json_body(body)?;
    let req = crate::compute_local::EnableReq {
        controller: s(&v, "controller").map(|c| c.trim_end_matches('/').to_string()),
        enroll: s(&v, "enroll").map(str::to_string),
        ticket: s(&v, "ticket").map(str::to_string),
        name: s(&v, "name").map(str::to_string),
        private_home: b(&v, "privateHome"),
        allow_prompt: b(&v, "elevate") == Some(true),
        // "Let K2 keep this node updated and healthy": what the request
        // names; unnamed = policy on a first install, the owner's earlier
        // choice on a reinstall (`setup_remote_admin`).
        remote_admin: match v.get("remoteAdmin") {
            None | Some(serde_json::Value::Null) => None,
            x => Some(crate::compute_local::remote_admin_arg(x, None).map_err(|(code, status, hint)| no(code, status, hint))?),
        },
    };
    crate::compute_local::enable(&req, &who.label()).map_err(|(code, status, hint)| no(code, status, hint))
}

/// The remote-admin toggles under "Make this computer a compute node",
/// changed later: `{level: off|policy|full, elevate?}`. Owner only; the
/// daemon runs root (never the renderer).
fn local_remote_admin(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_owner(who)?;
    let v = json_body(body)?;
    let level = crate::compute_local::remote_admin_arg(v.get("level"), None).map_err(|(code, status, hint)| no(code, status, hint))?;
    crate::compute_local::set_remote_admin(&level, &who.label(), b(&v, "elevate") == Some(true)).map_err(|(code, status, hint)| no(code, status, hint))
}

fn local_disable(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_owner(who)?;
    let v = json_body(body)?;
    crate::compute_local::disable(&who.label(), b(&v, "elevate") == Some(true)).map_err(|(code, status, hint)| no(code, status, hint))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_are_refused_with_a_code_never_dropped() {
        let v = serde_json::json!({"needs": ["cmake@3", "node@22"]});
        assert_eq!(needs_from(&v).unwrap(), vec!["cmake@3".to_string(), "node@22".to_string()]);
        let e = needs_from(&serde_json::json!({"needs": "node@22, make@4"})).unwrap_err();
        assert_eq!((e.code, e.status), ("tool_unsupported", 400));
        assert!(e.hint.contains("make@4") && e.hint.contains("cmake@<v>"), "{}", e.hint);
        let e = needs_from(&serde_json::json!({"needs": ["node@$(x)"]})).unwrap_err();
        assert_eq!((e.code, e.status), ("need_invalid", 400));
        assert!(needs_from(&serde_json::json!({})).unwrap().is_empty());
    }

    #[test]
    fn route_lists_agree() {
        for p in POST_ROUTES {
            assert!(ROUTES.contains(p), "{p} not in ROUTES");
        }
        for p in crate::compute_admin::ROUTES {
            assert!(ROUTES.contains(p), "admin route {p} not in ROUTES");
            assert_eq!(
                POST_ROUTES.contains(p),
                crate::compute_admin::POST_ROUTES.contains(p),
                "{p}: POST-only in one list but not the other"
            );
        }
        let mut sorted = ROUTES.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, ROUTES, "keep ROUTES sorted");
    }

    #[test]
    fn route_urls_private_only_for_plaintext() {
        assert!(route_ok("https://rosson.k2.dev"));
        assert!(route_ok("http://192.168.1.5:38471"));
        assert!(route_ok("http://100.101.2.3:1"));
        assert!(route_ok("ws://127.0.0.1:9"));
        assert!(route_ok("http://mini-1.local:9"));
        assert!(!route_ok("http://8.8.8.8:80"));
        assert!(!route_ok("http://example.com"));
        assert!(!route_ok("ftp://x"));
    }

    #[test]
    fn client_ids_and_cwd() {
        assert!(valid_client_id("abcdefghijklmnop"));
        assert!(!valid_client_id("short"));
        assert!(!valid_client_id("has spaces in it ok?"));
        assert!(valid_cwd("crates/k2-core"));
        assert!(!valid_cwd("/abs"));
        assert!(!valid_cwd("a/../../b"));
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn source_comes_from_the_callers_worktree_inside_the_workspace() {
        let d = std::env::temp_dir().join(format!("k2-compute-src-wt-{}-{}", std::process::id(), compute::now()));
        let _ = std::fs::remove_dir_all(&d);
        let r = d.join("ws");
        std::fs::create_dir_all(&r).expect("mkdir");
        git(&r, &["init", "-q"]);
        std::fs::write(r.join("a.txt"), "one\n").expect("write");
        git(&r, &["add", "."]);
        git(&r, &["commit", "-q", "-m", "one"]);
        let main_head = git(&r, &["rev-parse", "HEAD"]);
        let wt = r.join(".claude/worktrees/x");
        git(&r, &["worktree", "add", "-q", "-b", "wt", wt.to_str().expect("utf8")]);
        std::fs::write(wt.join("a.txt"), "two\n").expect("write");
        git(&wt, &["commit", "-q", "-am", "two"]);
        let wt_head = git(&wt, &["rev-parse", "HEAD"]);
        assert_ne!(wt_head, main_head);
        let ws = Ws { id: "ws-test".into(), path: r.to_str().expect("utf8").into(), handle: "ws".into() };
        let commit_for = |v: serde_json::Value| -> (String, Vec<String>) {
            let mut warnings = vec![];
            match source_for(&ws, &v, &d.join("unused.dirty.tar"), &mut warnings) {
                Ok((src, info)) => {
                    assert_eq!(src.commit, info.commit);
                    (src.commit, warnings)
                }
                Err(e) => panic!("source_for refused {v}: {}", e.hint),
            }
        };
        // From the worktree: its HEAD, not the main checkout's.
        let (c, w) = commit_for(serde_json::json!({ "repoDir": wt.to_str().expect("utf8") }));
        assert_eq!(c, wt_head);
        assert!(w.iter().all(|x| !x.contains("own checkout")), "{w:?}");
        // From the main checkout, or no repoDir (older CLI): the main HEAD.
        assert_eq!(commit_for(serde_json::json!({ "repoDir": r.to_str().expect("utf8") })).0, main_head);
        assert_eq!(commit_for(serde_json::json!({})).0, main_head);
        // A repo outside the workspace: refused as a source, with a note.
        let other = d.join("other");
        std::fs::create_dir_all(&other).expect("mkdir");
        git(&other, &["init", "-q"]);
        let (c, w) = commit_for(serde_json::json!({ "repoDir": other.to_str().expect("utf8") }));
        assert_eq!(c, main_head);
        assert!(w.iter().any(|x| x.contains("outside the workspace")), "{w:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A1 C3/C4/C5: the warning, `--dirty` and `--dirty --sha` all follow
    /// the checkout actually used (a worktree's own HEAD and files).
    #[test]
    fn dirty_and_the_warning_follow_the_worktree_not_the_main_checkout() {
        let d = std::env::temp_dir().join(format!("k2-compute-src-dirty-{}-{}", std::process::id(), compute::now()));
        let _ = std::fs::remove_dir_all(&d);
        let r = d.join("ws");
        std::fs::create_dir_all(&r).expect("mkdir");
        git(&r, &["init", "-q"]);
        std::fs::write(r.join("a.txt"), "one\n").expect("write");
        git(&r, &["add", "."]);
        git(&r, &["commit", "-q", "-m", "one"]);
        let wt = r.join(".claude/worktrees/x");
        git(&r, &["worktree", "add", "-q", "-b", "wt", wt.to_str().expect("utf8")]);
        std::fs::write(wt.join("a.txt"), "two\n").expect("write");
        git(&wt, &["commit", "-q", "-am", "two"]);
        let wt_head = git(&wt, &["rev-parse", "HEAD"]);
        let wt_prev = git(&wt, &["rev-parse", "HEAD~1"]);
        // The main checkout is dirty (always is in real life: .cursor/, notes).
        std::fs::write(r.join("main-untracked.txt"), "main\n").expect("write");
        let ws = Ws { id: "ws-test".into(), path: r.to_str().expect("utf8").into(), handle: "ws".into() };
        let wt_s = wt.to_str().expect("utf8");
        // Never under ~/.k2: the blob goes to this test's temp dir.
        let job = d.join("blob.dirty.tar");
        let go = |v: serde_json::Value| -> (R<(Src, SourceInfo)>, Vec<String>) {
            let mut w = vec![];
            let res = source_for(&ws, &v, &job, &mut w);
            (res, w)
        };
        let warned = |w: &[String]| w.iter().any(|x| x.contains("uncommitted changes"));
        // Clean worktree while main is dirty: no warning.
        let (res, w) = go(serde_json::json!({"repoDir": wt_s}));
        let (_, info) = res.expect("clean worktree run");
        assert!(!warned(&w), "{w:?}");
        assert_eq!((info.checkout, info.path.as_str(), info.commit.as_str()), ("worktree", ".claude/worktrees/x", wt_head.as_str()));
        assert!(info.dirty.is_none());
        // --sha on a dirty main checkout: never a warning.
        let (res, w) = go(serde_json::json!({"sha": "HEAD"}));
        res.expect("sha run from main");
        assert!(!warned(&w), "{w:?}");
        // No --sha from the dirty main checkout: warned.
        let (res, w) = go(serde_json::json!({}));
        res.expect("plain run from main");
        assert!(warned(&w), "{w:?}");
        // --dirty from the worktree: its untracked file, not main's.
        std::fs::write(wt.join("wt-untracked.txt"), "wt\n").expect("write");
        let (res, _) = go(serde_json::json!({"repoDir": wt_s, "dirty": true}));
        let (src, info) = res.expect("dirty run from worktree");
        let blob = src.dirty.expect("dirty blob");
        assert_eq!(info.dirty.expect("dirty info"), (0, 1, blob.bytes));
        assert_eq!(src.commit, wt_head);
        let blob_path = job.clone();
        let listing = std::process::Command::new("tar").arg("-tf").arg(&blob_path).output().expect("tar -tf");
        assert!(listing.status.success(), "tar -tf {}", blob_path.display());
        let listing = String::from_utf8_lossy(&listing.stdout).to_string();
        assert!(listing.contains("untracked/wt-untracked.txt"), "{listing}");
        assert!(!listing.contains("main-untracked.txt"), "{listing}");
        let _ = std::fs::remove_file(&blob_path);
        // --dirty --sha <the worktree's HEAD>: fine; --sha HEAD~1: refused.
        let (res, _) = go(serde_json::json!({"repoDir": wt_s, "dirty": true, "sha": wt_head}));
        assert_eq!(res.expect("dirty with sha = HEAD").0.commit, wt_head);
        let (res, _) = go(serde_json::json!({"repoDir": wt_s, "dirty": true, "sha": wt_prev}));
        let e = res.err().expect("dirty with an older sha is refused");
        assert_eq!((e.code, e.status), (DIRTY_NEEDS_HEAD, 400));
        assert!(e.hint.contains(".claude/worktrees/x") && e.hint.contains(&wt_head[..8]), "{}", e.hint);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A3 C7: an owner-token path inside a worktree answers as the
    /// workspace whose repo it belongs to; an unrelated repo doesn't.
    #[test]
    fn a_worktree_path_resolves_to_its_main_checkout_only() {
        let d = std::env::temp_dir().join(format!("k2-compute-ws-wt-{}-{}", std::process::id(), compute::now()));
        let _ = std::fs::remove_dir_all(&d);
        let r = d.join("ws");
        std::fs::create_dir_all(&r).expect("mkdir");
        git(&r, &["init", "-q"]);
        std::fs::write(r.join("a.txt"), "one\n").expect("write");
        git(&r, &["add", "."]);
        git(&r, &["commit", "-q", "-m", "one"]);
        let wt = r.join(".claude/worktrees/x");
        git(&r, &["worktree", "add", "-q", "-b", "wt", wt.to_str().expect("utf8")]);
        std::fs::create_dir_all(wt.join("sub")).expect("mkdir");
        let main = |p: &Path| compute::sync::main_checkout_of(p).map(|m| dunce::canonicalize(&m).expect("canon"));
        let want = Some(dunce::canonicalize(&r).expect("canon"));
        assert_eq!(main(&wt), want);
        assert_eq!(main(&wt.join("sub")), want);
        assert_eq!(main(&r), want);
        let other = d.join("other");
        std::fs::create_dir_all(&other).expect("mkdir");
        git(&other, &["init", "-q"]);
        assert_ne!(main(&other), want);
        let plain = d.join("plain");
        std::fs::create_dir_all(&plain).expect("mkdir");
        assert_eq!(compute::sync::main_checkout_of(&plain).filter(|p| p.starts_with(&d)), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `--repo` (0.46.6): a multi-repo workspace whose root isn't a repo
    /// (k2-cores) ships one nested repo at its own HEAD (or a worktree's),
    /// under its own project key; the caller's folder in another repo
    /// isn't the job's cwd; escapes and `.k2/` are refused.
    #[test]
    fn a_nested_repo_ships_on_its_own() {
        let d = std::env::temp_dir().join(format!("k2-compute-src-nested-{}-{}", std::process::id(), compute::now()));
        let ws_dir = d.join("k2-cores");
        let init = |at: &Path, file: &str| -> String {
            std::fs::create_dir_all(at).expect("mkdir");
            git(at, &["init", "-q"]);
            std::fs::write(at.join(file), "x\n").expect("write");
            git(at, &["add", "."]);
            git(at, &["commit", "-q", "-m", "init"]);
            git(at, &["rev-parse", "HEAD"])
        };
        let base = ws_dir.join("k2-multiplayer-base");
        let md = ws_dir.join("k2-multiplayer-markdown");
        let base_head = init(&base, "base.rs");
        init(&md, "md.rs");
        std::fs::create_dir_all(ws_dir.join(".k2")).expect("mkdir");
        let wt = ws_dir.join("wt-base-x");
        git(&base, &["worktree", "add", "-q", "-b", "x", wt.to_str().expect("utf8")]);
        std::fs::write(wt.join("base.rs"), "y\n").expect("write");
        git(&wt, &["commit", "-q", "-am", "x"]);
        let wt_head = git(&wt, &["rev-parse", "HEAD"]);
        let ws = Ws { id: "ws-cores".into(), path: ws_dir.to_str().expect("utf8").into(), handle: "k2-cores".into() };
        let go = |v: serde_json::Value| -> (R<(Src, SourceInfo)>, Vec<String>) {
            let mut w = vec![];
            let r = source_for(&ws, &v, &d.join("blob.dirty.tar"), &mut w);
            (r, w)
        };
        let key = compute::sync::project_key_in("ws-cores", None, Some("k2-multiplayer-base"));

        // --repo from the workspace root: the repo's own HEAD, its own key.
        let (r, w) = go(serde_json::json!({"repo": "k2-multiplayer-base"}));
        let (src, info) = r.expect("--repo k2-multiplayer-base");
        assert_eq!((src.commit.as_str(), src.project_key.as_str(), info.repo.as_deref()), (base_head.as_str(), key.as_str(), Some("k2-multiplayer-base")));
        assert!(info.caller_elsewhere, "no caller folder in that repo: the job starts at its root");
        assert!(w.iter().all(|x| !x.contains("own checkout")), "{w:?}");
        // From the sibling repo: still base, and the sibling's folder isn't the cwd.
        let (r, _) = go(serde_json::json!({"repo": "k2-multiplayer-base", "repoDir": md.to_str().expect("utf8")}));
        assert!(r.expect("from the sibling").1.caller_elsewhere);
        // --repo naming the worktree: its HEAD, the main checkout's key.
        let (r, _) = go(serde_json::json!({"repo": "wt-base-x"}));
        let (src, info) = r.expect("--repo wt-base-x");
        assert_eq!((src.commit.as_str(), src.project_key.as_str(), info.checkout), (wt_head.as_str(), key.as_str(), "worktree"));
        // No --repo, run from inside the worktree: found for you, with a note.
        let (r, w) = go(serde_json::json!({"repoDir": wt.to_str().expect("utf8")}));
        let (src, info) = r.expect("found from the caller's worktree");
        assert_eq!((src.commit.as_str(), info.repo.as_deref(), info.caller_elsewhere), (wt_head.as_str(), Some("k2-multiplayer-base"), false));
        assert!(w.iter().any(|x| x.contains("isn't a git repo: shipping its repo k2-multiplayer-base")), "{w:?}");
        // --repo base from inside the worktree: the worktree's HEAD (as today).
        let (r, _) = go(serde_json::json!({"repo": "k2-multiplayer-base", "repoDir": wt.to_str().expect("utf8")}));
        let (src, info) = r.expect("worktree of the named repo");
        assert_eq!((src.commit.as_str(), info.caller_elsewhere), (wt_head.as_str(), false));
        // --dirty from the worktree: its own changes only.
        std::fs::write(wt.join("new.rs"), "n\n").expect("write");
        std::fs::create_dir_all(wt.join(".k2")).expect("mkdir");
        std::fs::write(wt.join(".k2/note.md"), "private\n").expect("write");
        let (r, _) = go(serde_json::json!({"repo": "wt-base-x", "dirty": true}));
        let (src, info) = r.expect("dirty nested worktree");
        assert!(src.dirty.is_some());
        assert_eq!(info.dirty.map(|(c, u, _)| (c, u)), Some((0, 1)), "new.rs only, never .k2/");
        // Nothing to go on: not_a_repo, teaching --repo.
        let (r, _) = go(serde_json::json!({}));
        let e = r.err().expect("refused");
        assert_eq!(e.code, codes::NOT_A_REPO);
        assert!(e.hint.contains("--repo"), "{}", e.hint);
        // The jail.
        for bad in ["../k2-cores/k2-multiplayer-base", ".k2", "/etc", "k2-multiplayer-base/..", "nope"] {
            let (r, _) = go(serde_json::json!({"repo": bad}));
            assert_eq!(r.err().map(|e| e.code), Some(REPO_INVALID), "{bad}");
        }
        // --with: the sibling at its HEAD (or @SHA), laid out by its folder,
        // its own project key; overlaps, escapes and too many are refused.
        let mut w = vec![];
        let with = with_for(&ws, &serde_json::json!({"with": [{"repo": "k2-multiplayer-base"}]}), "k2-multiplayer-markdown", &mut w).expect("--with base");
        assert_eq!(with.len(), 1);
        assert_eq!((with[0].path.as_str(), with[0].commit.as_str(), with[0].project_key.as_str()), ("k2-multiplayer-base", base_head.as_str(), key.as_str()));
        let with = with_for(&ws, &serde_json::json!({"with": [{"repo": "wt-base-x"}]}), "k2-multiplayer-markdown", &mut w).expect("--with a worktree");
        assert_eq!((with[0].path.as_str(), with[0].commit.as_str()), ("k2-multiplayer-base", wt_head.as_str()), "laid out where its repo lives, at the worktree's HEAD");
        assert!(w.iter().any(|x| x.contains("--with wt-base-x has uncommitted changes")), "{w:?}");
        let with = with_for(&ws, &serde_json::json!({"with": [{"repo": "k2-multiplayer-base", "sha": base_head}]}), "k2-multiplayer-markdown", &mut w).expect("@sha");
        assert_eq!(with[0].commit, base_head);
        for bad in [
            serde_json::json!({"with": [{"repo": "k2-multiplayer-markdown"}]}),
            serde_json::json!({"with": [{"repo": "k2-multiplayer-base"}, {"repo": "wt-base-x"}]}),
            serde_json::json!({"with": [{"repo": "../x"}]}),
            serde_json::json!({"with": [{"repo": ".k2"}]}),
            serde_json::json!({"with": "k2-multiplayer-base"}),
            serde_json::json!({"with": [{"repo": "k2-multiplayer-base", "sha": "nope"}]}),
        ] {
            let e = with_for(&ws, &bad, "k2-multiplayer-markdown", &mut w).err().unwrap_or_else(|| panic!("{bad} must be refused"));
            assert!(matches!(e.code, REPO_INVALID | codes::BAD_REQUEST), "{bad}: {}", e.code);
        }
        assert!(with_for(&ws, &serde_json::json!({}), "", &mut w).expect("none").is_empty());
        // The bundle comes from the stored rel, checked again.
        let n = compute::sync::nested_repo(&ws_dir, "k2-multiplayer-base").expect("again");
        assert_eq!(n.home, base.canonicalize().expect("canon"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cache_names_are_checked_at_submit() {
        assert_eq!(cache_from(&serde_json::json!({})).unwrap(), None);
        assert_eq!(cache_from(&serde_json::json!({"cache": "fuzz-corpus"})).unwrap().as_deref(), Some("fuzz-corpus"));
        for bad in [serde_json::json!({"cache": "../x"}), serde_json::json!({"cache": ""}), serde_json::json!({"cache": 3}), serde_json::json!({"cache": "Fuzz"})] {
            let e = cache_from(&bad).unwrap_err();
            assert_eq!((e.code, e.status), (pcache::codes::CACHE_INVALID, 400), "{bad}");
        }
    }

    /// A malformed `--with` names the flag as the CLI help does (`DIR[@SHA]`).
    #[test]
    fn a_bad_with_names_dir_at_sha_like_the_help() {
        for bad in [serde_json::json!({"with": "x"}), serde_json::json!({"with": [{"repo": ""}]}), serde_json::json!({"with": [{"repo": "a", "sha": 7}]})] {
            let e = with_asks(&bad).expect_err("refused");
            assert_eq!((e.code, e.status), (REPO_INVALID, 400), "{bad}");
            assert!(e.hint.starts_with("--with DIR[@SHA]: "), "{bad}: {}", e.hint);
            assert!(!e.hint.contains("REPO"), "{bad}: {}", e.hint);
        }
        assert_eq!(with_asks(&serde_json::json!({"with": [{"repo": "a", "sha": "abc"}]})).expect("ok"), vec![("a".to_string(), Some("abc".to_string()))]);
    }

    /// `k2 compute grants --workspace WS` (0.46.6 papercut): the filter is
    /// honoured for the owner, an Admin and a person in a K2 shell tab; an
    /// agent sees its own grants, and naming another workspace is a loud
    /// 403, never its own list in disguise.
    #[test]
    fn grants_workspace_filter_is_honoured_and_agents_see_only_their_own() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            for (id, h) in [("w-ga", "ga"), ("w-gb", "gb")] {
                conn.execute(
                    "INSERT INTO projects (id, name, path, handle, agents_can_use_compute) VALUES (?1, ?2, ?3, ?2, 1)",
                    rusqlite::params![id, h, format!("/tmp/{id}")],
                )
                .expect("project");
            }
            store::insert_pending_node(&conn, "n-1", "mini-1", &format!("{:0>64}", "n1"), "pem", &Default::default(), "owner", "123456", 1)
                .expect("node");
            for ws in ["w-ga", "w-gb"] {
                store::set_grant(&conn, "n-1", ws, &store::GrantLimits::default(), "owner", 2).expect("grant");
            }
        }
        let ws_of = |v: &serde_json::Value| -> Vec<String> {
            let rows = v["grants"].as_array().unwrap_or_else(|| panic!("no grants list: {v}"));
            rows.iter().map(|g| g["workspaceId"].as_str().unwrap_or_else(|| panic!("row: {g}")).to_string()).collect()
        };
        let q = |ws: Option<&str>| -> HashMap<String, String> {
            ws.map(|w| HashMap::from([("workspace".to_string(), w.to_string())])).unwrap_or_default()
        };
        let list = |who: &Who, ws: Option<&str>| grants_list(who, &q(ws), None);

        // The owner and an Admin: the named workspace, or all of them.
        let admin = Who::Login { username: "a".into(), role: Role::Admin };
        for who in [Who::Owner, admin] {
            assert_eq!(ws_of(&list(&who, Some("w-gb")).expect("named")), vec!["w-gb"], "{who:?}");
            assert_eq!(ws_of(&list(&who, None).expect("all")), vec!["w-ga", "w-gb"], "{who:?}");
        }
        // A person in a K2 shell tab: any named workspace; none = its own.
        let shell = Who::Shell { project_id: "w-ga".into(), session_id: "s-sh".into() };
        assert_eq!(ws_of(&list(&shell, Some("w-gb")).expect("shell names another")), vec!["w-gb"]);
        assert_eq!(ws_of(&list(&shell, None).expect("shell default")), vec!["w-ga"]);
        // An agent: its own, named or not; another workspace is refused.
        let agent = Who::Agent { project_id: "w-ga".into(), session_id: "s-ag".into() };
        assert_eq!(ws_of(&list(&agent, None).expect("own")), vec!["w-ga"]);
        assert_eq!(ws_of(&list(&agent, Some("w-ga")).expect("own, named")), vec!["w-ga"]);
        for other in ["w-gb", "no-such-ws"] {
            let e = list(&agent, Some(other)).expect_err("another workspace is refused");
            assert_eq!((e.code, e.status), ("forbidden", 403), "{other}: {}", e.hint);
            assert!(e.hint.contains("only its own workspace's grants"), "{}", e.hint);
        }
        // An unknown name for a person is a 404, not an empty list.
        let e = list(&Who::Owner, Some("no-such-ws")).expect_err("unknown");
        assert_eq!((e.code, e.status), ("workspace_not_found", 404));
    }

    /// The permission loop: `k2 compute cache` from an agent reaches its
    /// own workspace with no switch, and another workspace's is refused
    /// with words that don't send it to compute.manage.
    #[test]
    fn an_agent_reaches_its_own_workspaces_caches_only() {
        let _home = crate::test_support::TempHome::new();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            for (id, h) in [("w-ca", "ca"), ("w-cb", "cb")] {
                conn.execute(
                    "INSERT INTO projects (id, name, path, handle, agents_can_use_compute) VALUES (?1, ?2, ?3, ?2, 1)",
                    rusqlite::params![id, h, format!("/tmp/{id}")],
                )
                .expect("project");
            }
        }
        let q = |ws: Option<&str>| -> HashMap<String, String> { ws.map(|w| HashMap::from([("workspace".to_string(), w.to_string())])).unwrap_or_default() };
        let agent = Who::Agent { project_id: "w-ca".into(), session_id: "s-ag".into() };
        // Its own: named or not, list and clear, with no manage switch.
        assert!(caches(&agent, &q(None), None).expect("own list")["nodes"].is_array());
        assert!(caches(&agent, &q(Some("w-ca")), None).expect("own, named")["nodes"].is_array());
        assert!(cache_clear(&agent, br#"{}"#, None).expect("own clear")["nodes"].is_array());
        // Another workspace's, and --all: owner_only, naming the owner path.
        for e in [caches(&agent, &q(Some("w-cb")), None).expect_err("other"), cache_clear(&agent, br#"{"workspace":"w-cb"}"#, None).expect_err("other clear"), cache_clear(&agent, br#"{"all":true}"#, None).expect_err("all")] {
            assert_eq!((e.code, e.status), ("owner_only", 403), "{}", e.hint);
            assert!(e.hint.contains("only its own workspace's job caches (ca)") && e.hint.contains("for the owner"), "{}", e.hint);
            assert!(!e.hint.contains("compute.manage"), "{}", e.hint);
        }
        // Compute off for the workspace: compute_off, not a way in.
        {
            let db = k2_core::db::shared();
            db.lock().execute("UPDATE projects SET agents_can_use_compute = 0 WHERE id = 'w-ca'", []).expect("off");
        }
        assert_eq!(caches(&agent, &q(None), None).expect_err("off").code, codes::COMPUTE_OFF);
        // A person with no standing still can't list.
        let member = Who::Login { username: "m".into(), role: Role::Member };
        assert_eq!(caches(&member, &q(None), None).expect_err("member").code, "owner_only");
    }
}
