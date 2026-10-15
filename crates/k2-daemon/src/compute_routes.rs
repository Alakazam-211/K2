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
//!
//! The TCP dispatcher and the per-cell socket both build a [`Who`] and
//! call [`handle`] (blocking: DB, git, the log long-poll).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{Duration, Instant};

use k2_core::compute::proto::frames::{Blob, Control, JobLimits, JobState, Plan, Src};
use k2_core::compute::proto::{crypto, env as penv, needs, pairing, results};
use k2_core::compute::{self as compute, codes, logs, store};
use k2_core::connect_users::Role;

use crate::cli_response::CliResponse;
use crate::compute_ws;

/// Largest request body.
pub const MAX_BODY: usize = 1 << 20;

/// Every compute route (the two sockets included).
pub const ROUTES: &[&str] = &[
    "/cli/compute/admin/approval-key",
    "/cli/compute/admin/approve",
    "/cli/compute/admin/deny",
    "/cli/compute/admin/propose",
    "/cli/compute/admin/requests",
    "/cli/compute/admin/withdraw",
    "/cli/compute/agents",
    "/cli/compute/agents/set",
    compute_ws::ATTACH_PATH,
    compute_ws::ENROLL_PATH,
    "/cli/compute/events",
    "/cli/compute/grants",
    "/cli/compute/grants/revoke",
    "/cli/compute/grants/set",
    "/cli/compute/jobs",
    "/cli/compute/jobs/artifact",
    "/cli/compute/jobs/cancel",
    "/cli/compute/jobs/get",
    "/cli/compute/jobs/logs",
    "/cli/compute/jobs/receipt",
    "/cli/compute/jobs/retry",
    "/cli/compute/local/disable",
    "/cli/compute/local/drain",
    "/cli/compute/local/enable",
    "/cli/compute/local/pause",
    "/cli/compute/local/policy",
    "/cli/compute/local/resume",
    "/cli/compute/local/status",
    "/cli/compute/nodes",
    "/cli/compute/nodes/alerts",
    "/cli/compute/nodes/confirm",
    "/cli/compute/nodes/enroll-code",
    "/cli/compute/nodes/health",
    "/cli/compute/nodes/info",
    "/cli/compute/nodes/pause",
    "/cli/compute/nodes/policy",
    "/cli/compute/nodes/policy/set",
    "/cli/compute/nodes/remove",
    "/cli/compute/nodes/rename",
    "/cli/compute/nodes/retire",
    "/cli/compute/nodes/rollback",
    "/cli/compute/nodes/routes",
    "/cli/compute/nodes/update",
    "/cli/compute/run",
    "/cli/compute/usage",
];

/// POST-only rows (`require_post` → 405 on GET).
pub const POST_ROUTES: &[&str] = &[
    "/cli/compute/admin/approve",
    "/cli/compute/admin/deny",
    "/cli/compute/admin/propose",
    "/cli/compute/admin/withdraw",
    "/cli/compute/agents/set",
    "/cli/compute/grants/revoke",
    "/cli/compute/grants/set",
    "/cli/compute/jobs/cancel",
    "/cli/compute/jobs/retry",
    "/cli/compute/local/disable",
    "/cli/compute/local/drain",
    "/cli/compute/local/enable",
    "/cli/compute/local/pause",
    "/cli/compute/local/policy",
    "/cli/compute/local/resume",
    "/cli/compute/nodes/alerts",
    "/cli/compute/nodes/confirm",
    "/cli/compute/nodes/enroll-code",
    "/cli/compute/nodes/pause",
    "/cli/compute/nodes/policy/set",
    "/cli/compute/nodes/remove",
    "/cli/compute/nodes/rename",
    "/cli/compute/nodes/retire",
    "/cli/compute/nodes/rollback",
    "/cli/compute/nodes/routes",
    "/cli/compute/nodes/update",
    "/cli/compute/run",
];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
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
        let role = k2_core::connect_users::role_for_user(&username).unwrap_or(Role::Member);
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
}

pub(crate) type R<T> = Result<T, Refusal>;

pub(crate) fn no(code: &'static str, status: u16, hint: impl Into<String>) -> Refusal {
    Refusal { code, status, hint: hint.into() }
}

pub(crate) fn refusal_response(r: &Refusal) -> CliResponse {
    CliResponse {
        status: status_line(r.status),
        content_type: "application/json",
        body: serde_json::json!({ "error": { "code": r.code, "hint": r.hint } }).to_string(),
    }
}

pub fn too_large() -> CliResponse {
    refusal_response(&no(codes::BAD_REQUEST, 413, format!("request bodies here are at most {} KiB", MAX_BODY / 1024)))
}

/// 404 for every compute route while the preview flag is off (CN30).
pub fn dark() -> CliResponse {
    CliResponse {
        status: "404 Not Found",
        content_type: "application/json",
        body: r#"{"error":{"code":"compute_disabled","hint":"compute nodes are a preview: the owner turns them on with K2_COMPUTE=1 or Settings computePreview"}}"#
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
            .then(|| std::fs::canonicalize(t).ok())
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

/// The workspace a request acts for. A passport's comes from the passport
/// (naming another is `forbidden`); others must name one.
fn target_ws(who: &Who, requested: Option<&str>) -> R<Ws> {
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
    ws_by_token(req).ok_or_else(|| no("workspace_not_found", 404, format!("no workspace '{req}'")))
}

/// For passports: the workspace switch must be on.
fn passport_gate(who: &Who) -> R<Option<String>> {
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

pub(crate) fn audit(who: &Who, kind: &str, node: Option<&str>, ws: Option<&str>, job: Option<&str>, detail: serde_json::Value) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::record_event(&conn, &who.label(), kind, node, ws, job, detail);
}

// ── entry ────────────────────────────────────────────────────────────

/// Route a compute request. Blocking.
pub fn handle(path: &str, who: Who, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if !compute::enabled() {
        return dark();
    }
    // Queue timeouts and moves need the ticker even before any node attached.
    compute_ws::ensure_ticker_started();
    let r = match path {
        "/cli/compute/nodes" => nodes_list(&who),
        "/cli/compute/nodes/info" => node_info(&who, params),
        "/cli/compute/nodes/enroll-code" => enroll_code(&who, body),
        "/cli/compute/nodes/confirm" => node_confirm(&who, body),
        "/cli/compute/nodes/remove" => node_remove(&who, body),
        "/cli/compute/nodes/retire" => node_retire(&who, body),
        "/cli/compute/nodes/rename" => node_rename(&who, body),
        "/cli/compute/nodes/pause" => node_pause(&who, body),
        "/cli/compute/nodes/routes" => node_routes(&who, body),
        "/cli/compute/grants" => grants_list(&who, params),
        "/cli/compute/grants/set" => grant_set(&who, body),
        "/cli/compute/grants/revoke" => grant_revoke(&who, body),
        "/cli/compute/agents" => agents_list(&who),
        "/cli/compute/agents/set" => agents_set(&who, body),
        "/cli/compute/run" => run(&who, body),
        "/cli/compute/jobs" => jobs_list(&who, params),
        "/cli/compute/jobs/get" => job_get(&who, params),
        "/cli/compute/jobs/logs" => job_logs(&who, params),
        "/cli/compute/jobs/receipt" => job_receipt(&who, params),
        "/cli/compute/jobs/artifact" => job_artifact(&who, params),
        "/cli/compute/jobs/cancel" => job_cancel(&who, body),
        "/cli/compute/jobs/retry" => job_retry(&who, body),
        "/cli/compute/usage" => usage(&who, params),
        "/cli/compute/events" => events(&who, params),
        "/cli/compute/local/status" => local_status(&who),
        "/cli/compute/local/pause" => local_control(&who, body, "pause"),
        "/cli/compute/local/drain" => local_control(&who, body, "drain"),
        "/cli/compute/local/resume" => local_control(&who, body, "resume"),
        "/cli/compute/local/policy" => local_policy(&who, body),
        "/cli/compute/local/enable" => local_enable(&who, body),
        "/cli/compute/local/disable" => local_disable(&who, body),
        // Node admin (prd-k2-node-admin-v1): approvals, policy, update,
        // health, alerts.
        p if crate::compute_admin::is_route(p) => crate::compute_admin::handle(p, &who, body, params),
        _ => Err(no("not_found", 404, "no such compute route")),
    };
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refusal_response(&e),
    }
}

// ── nodes ────────────────────────────────────────────────────────────

fn node_json(n: &store::NodeRow, show_sas: bool) -> serde_json::Value {
    let (online, route, live_offer) = compute_ws::live_view(&n.id);
    let offer = live_offer.or_else(|| n.last_offer.clone());
    let (queued, active) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (
            store::jobs_in_states(&conn, &n.id, &["queued"]).len(),
            store::jobs_in_states(&conn, &n.id, store::ACTIVE_STATES).len(),
        )
    };
    let mut v = serde_json::to_value(n).unwrap_or_default();
    v["online"] = online.into();
    v["liveRoute"] = route.into();
    v["queued"] = queued.into();
    v["running"] = active.into();
    if let Some(o) = offer {
        v["offer"] = serde_json::json!({
            "control": o.control.as_str(),
            "controlBy": o.control_by,
            "available": o.availability.ok,
            "unavailable": o.availability.reasons,
            "refusal": o.refusal,
            "caps": o.caps,
            "free": o.free,
            "parallelMax": o.parallel_max,
            "tools": o.tools,
            "slots": o.slots,
            "protocol": o.protocol,
            "features": o.features,
        });
    }
    if show_sas && n.state == "pending" {
        v["sas"] = n.sas.as_deref().map(pairing::display_sas).into();
    }
    v["retiring"] = compute::access::retiring_at(&compute::access::retiring_path()).contains_key(&n.id).into();
    if let Some(u) = crate::compute_place::set_aside_until(&n.id, compute::now()) {
        // Infra failures in a row: `--any` skips it until then.
        v["setAsideUntil"] = u.into();
    }
    v
}

fn granted_node_ids(ws: &str) -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::grants(&conn, None, Some(ws)).into_iter().map(|g| g.node_id).collect()
}

fn nodes_list(who: &Who) -> R<serde_json::Value> {
    need_credential(who)?;
    let gate = passport_gate(who)?;
    sweep_retiring();
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_nodes(&conn, false)
    };
    let (rows, ws): (Vec<_>, Option<String>) = match gate {
        Some(ws) => {
            let ids = granted_node_ids(&ws);
            (rows.into_iter().filter(|n| n.state == "active" && ids.contains(&n.id)).collect(), Some(ws))
        }
        None => (rows, None),
    };
    let show_sas = who.is_owner();
    let nodes: Vec<_> = rows
        .iter()
        .map(|n| {
            let mut v = node_json(n, show_sas);
            if let Some(ws) = &ws {
                let db = k2_core::db::shared();
                let conn = db.lock();
                v["grant"] = serde_json::to_value(store::grant(&conn, &n.id, ws)).unwrap_or_default();
            }
            v
        })
        .collect();
    let codes: Vec<serde_json::Value> = if who.is_owner() {
        compute::enroll::pending().into_iter().map(|(name, exp)| serde_json::json!({"name": name, "expiresAt": exp})).collect()
    } else {
        Vec::new()
    };
    // `ticketSupported`: this server trusts a one-click ticket key, so a
    // machine on the same Connect account can join it without a code.
    Ok(serde_json::json!({
        "nodes": nodes,
        "waitingCodes": codes,
        "controller": compute_ws::controller_label(),
        "ticketSupported": compute::ticket::supported(),
    }))
}

fn node_visible(who: &Who, n: &store::NodeRow) -> R<()> {
    if let Some(ws) = passport_gate(who)? {
        if n.state != "active" || !granted_node_ids(&ws).contains(&n.id) {
            return Err(no(codes::NODE_NOT_FOUND, 404, format!("no compute node '{}' is granted to this workspace", n.name)));
        }
    }
    Ok(())
}

fn node_info(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    need_credential(who)?;
    let n = node_or_404(params.get("node").map(String::as_str))?;
    node_visible(who, &n)?;
    let mut v = node_json(&n, who.is_owner());
    let (grants, jobs) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let ws = who.passport().map(|(w, _)| w.to_string());
        let grants = store::grants(&conn, Some(&n.id), ws.as_deref());
        let jobs = store::list_jobs(&conn, &store::JobFilter { node_id: Some(&n.id), workspace_id: ws.as_deref(), limit: 10, ..Default::default() });
        (grants, jobs)
    };
    v["grants"] = serde_json::to_value(grants).unwrap_or_default();
    v["recentJobs"] = jobs.iter().map(job_summary).collect::<Vec<_>>().into();
    Ok(v)
}

fn controller_url(override_url: Option<&str>) -> String {
    if let Some(u) = override_url {
        return u.trim_end_matches('/').to_string();
    }
    format!("https://{}", compute_ws::controller_label())
}

fn enroll_code(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_owner(who)?;
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
    let args = format!("--controller {url} --enroll {enroll} --name {name}");
    Ok(serde_json::json!({
        "name": name,
        "code": code,
        "controllerFp": fp,
        "enroll": enroll,
        "expiresAt": entry.expires_at,
        "controllerUrl": url,
        "install": {
            "linux": format!("sudo scripts/node/install-node-linux.sh --binary ./k2-node {args} --human \"$USER\""),
            "macos": format!("sudo scripts/node/install-node-macos.sh --binary ./k2-node {args} --human \"$USER\""),
            "enrollOnly": format!("k2-node enroll {args}"),
        },
        "confirm": format!("k2 compute node confirm {name} <code shown on the node>"),
    }))
}

fn node_confirm(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_owner(who)?;
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
fn node_retire(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_owner(who)?;
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

fn node_rename(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_admin(who)?;
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
    Ok(serde_json::json!({"ok": true, "node": name}))
}

fn node_pause(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_admin(who)?;
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

fn node_routes(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_admin(who)?;
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
    Ok(serde_json::json!({"ok": true, "node": n.name, "routes": routes, "note": "the node learns new routes on its next connect"}))
}

// ── grants ───────────────────────────────────────────────────────────

fn grants_list(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    need_credential(who)?;
    let gate = passport_gate(who)?;
    let node = match params.get("node").map(String::as_str).filter(|s| !s.is_empty()) {
        Some(t) => Some(node_or_404(Some(t))?.id),
        None => None,
    };
    let ws = match gate {
        Some(own) => Some(own),
        None => match params.get("workspace").map(String::as_str).filter(|s| !s.is_empty()) {
            Some(t) => Some(ws_by_token(t).ok_or_else(|| no("workspace_not_found", 404, format!("no workspace '{t}'")))?.id),
            None => None,
        },
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

fn grant_set(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_admin(who)?;
    let v = json_body(body)?;
    let n = node_or_404(s(&v, "node"))?;
    if n.state != "active" {
        return Err(no("not_active", 409, format!("'{}' is {}; confirm it first", n.name, n.state)));
    }
    let ws = target_ws(who, s(&v, "workspace"))?;
    let limits = store::GrantLimits {
        max_job_secs: n_pos(&v, "maxJobSecs", 60, 7 * 86_400)?,
        max_disk_gb: n_pos(&v, "maxDiskGb", 1, 10_000)?,
        max_parallel: n_pos(&v, "maxParallel", 1, 64)?,
        max_queued: n_pos(&v, "maxQueued", 1, 1000)?,
    };
    let g = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let g = store::set_grant(&conn, &n.id, &ws.id, &limits, &who.label(), compute::now()).map_err(|e| no("internal", 500, e))?;
        store::record_event(&conn, &who.label(), "grant", Some(&n.id), Some(&ws.id), None, serde_json::to_value(&g).unwrap_or_default());
        g
    };
    let switch_on = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::agents_can_use_compute(&conn, &ws.id)
    };
    Ok(serde_json::json!({"ok": true, "node": n.name, "workspace": ws.handle, "grant": g, "agentsCanUseCompute": switch_on,
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

fn grant_revoke(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    need_admin(who)?;
    let v = json_body(body)?;
    let n = node_or_404(s(&v, "node"))?;
    let ws = target_ws(who, s(&v, "workspace"))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let removed = store::revoke_grant(&conn, &n.id, &ws.id).map_err(|e| no("internal", 500, e))?;
    store::record_event(&conn, &who.label(), "grant_revoke", Some(&n.id), Some(&ws.id), None, serde_json::json!({"removed": removed}));
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
    // dropped: the job must not start without what it asked for.
    let parsed = needs::parse_all(&raw).map_err(|e| no(e.code, 400, format!("--needs: {}", e.message)))?;
    Ok(parsed.iter().map(|n| n.token()).collect())
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
    let v = json_body(body)?;
    let ws = target_ws(who, s(&v, "workspace"))?;
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
    let request_sha = crypto::sha256_hex(&k2_core::compute::proto::canonical::to_vec(&v).map_err(|e| no(codes::BAD_REQUEST, 400, e))?);
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
        return run_sharded(who, &v, &ws, client_id, &request_sha);
    }
    let session = who.passport().map(|(_, sid)| sid.to_string());
    let sub = submit_one(who, &v, &ws, client_id, &request_sha, &SubmitOpts { session, ..Default::default() })?;
    Ok(submitted_json(&sub))
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
    preview: Option<serde_json::Value>,
}

fn submitted_json(sub: &Submitted) -> serde_json::Value {
    let Some(job) = sub.job.as_ref() else { return sub.preview.clone().unwrap_or_default() };
    let mut out = serde_json::json!({ "job": job_summary(job), "existing": false, "warnings": sub.warnings });
    if job.state == "queued" {
        if let Some((pos, why)) = compute_ws::decision_for(&job.node_id).waiting.get(&job.id) {
            out["queue"] = serde_json::json!({"position": pos, "reason": why.code, "message": why.message});
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
        return Err(no(codes::BAD_REQUEST, 400, "pass the command after --: k2 compute run <node> -- <command…>"));
    }
    let env: BTreeMap<String, String> = v
        .get("env")
        .and_then(|e| e.as_object())
        .map(|m| m.iter().filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string()))).collect())
        .unwrap_or_default();
    if let Err((code, name)) = penv::check_env_pairs(env.iter().map(|(k, x)| (k.as_str(), x.as_str()))) {
        return Err(no("env_refused", 400, format!("--env {name}: {code} (K2 tokens, sockets and the node's own variables can't be set)")));
    }
    let cwd = s(v, "cwd").map(str::to_string).filter(|c| !c.is_empty() && c != ".");
    if cwd.as_deref().is_some_and(|c| !valid_cwd(c)) {
        return Err(no(codes::BAD_REQUEST, 400, "cwd must be relative to the repo root, without '..'"));
    }
    // Toolchains / system packages the job needs (compute polish v1).
    let needs = needs_from(&v)?;
    // Files to keep after the command exits (results back, G1).
    let artifacts = artifacts_from(&v)?;
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
    let detach = b(v, "detach") == Some(true);
    // Never stuck (G3/G4): queue timeout, infra retries.
    let tun = compute::retry::tuning();
    let queue_timeout = match n(v, "queueTimeoutSecs") {
        Some(0) => None,
        Some(x) if (1..=7 * 86_400).contains(&x) => Some(x),
        Some(_) => return Err(no(codes::BAD_REQUEST, 400, "--queue-timeout is between 1s and 7d, or off")),
        None => Some(if detach { tun.queue_timeout_detached_secs } else { tun.queue_timeout_attached_secs }),
    };
    let retries = match n(v, "retries") {
        Some(r) if (0..=compute::retry::MAX_RETRIES as i64).contains(&r) => r as u32,
        Some(_) => return Err(no(codes::BAD_REQUEST, 400, format!("--retries is 0–{}", compute::retry::MAX_RETRIES))),
        None if b(v, "retryInterrupted") == Some(false) => 0,
        None => 1,
    };
    let stall = match n(v, "stallSecs") {
        None => None,
        Some(0) => Some(0),
        Some(x) if (1..=7 * 86_400).contains(&x) => Some(x),
        Some(_) => return Err(no(codes::BAD_REQUEST, 400, "--stall-timeout is between 1s and 7d, or off")),
    };
    let any = s(v, "node").is_none() && b(v, "any") == Some(true);
    let filter = store::AnyFilter {
        os: s(v, "os").map(str::to_string),
        arch: s(v, "arch").map(str::to_string),
        labels: v
            .get("labels")
            .and_then(|l| l.as_object())
            .map(|m| m.iter().filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string()))).collect())
            .unwrap_or_default(),
    };
    let want = crate::compute_place::Want {
        ws_id: ws.id.clone(),
        admin: who.is_admin(),
        filter: filter.clone(),
        needs: needs.clone(),
        artifacts: !artifacts.is_empty(),
        exclusive: b(v, "exclusive") == Some(true),
        needs_git: b(v, "noSource") != Some(true),
        cpu_millis,
        mem_bytes,
        stall: stall.is_some_and(|x| x > 0),
    };
    // Node: by name, or --any.
    let mut warnings: Vec<String> = Vec::new();
    let node = pick_node(v, ws, &want, queue_timeout, &opts.prefer_not, &mut warnings)?;
    if !artifacts.is_empty() && !node_features(&node).iter().any(|f| f == results::FEATURE_ARTIFACTS) {
        return Err(no(
            "node_upgrade_required",
            409,
            format!("'{}' runs an older k2-node that can't keep --artifact files; update k2-node on that machine (sudo k2-node setup), or run without --artifact", node.name),
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
        let wants_sys = needs.iter().any(|n| n.starts_with("sys:"));
        if wants_sys && !feats.iter().any(|f| f == needs::FEATURE_SYSTEM_PACKAGES) {
            warnings.push(format!(
                "'{}' can't install system packages itself (macOS, or no sudo rule): the job fails if one is missing, with the command its owner should run",
                node.name
            ));
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
    if let Some(g) = &grant {
        let queued = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            store::queued_count(&conn, &node.id, &ws.id)
        };
        if queued >= g.max_queued {
            return Err(no(codes::QUEUE_FULL, 409, format!("{queued} jobs from this workspace already wait on {} (grant max {})", node.name, g.max_queued)));
        }
    }
    let max_secs = grant.as_ref().map(|g| g.max_job_secs).unwrap_or(compute::DEFAULT_MAX_JOB_SECS);
    let timeout = n(v, "timeoutSecs").unwrap_or(max_secs);
    if timeout < 1 || timeout > max_secs {
        return Err(no(codes::BAD_REQUEST, 400, format!("--timeout must be at most {max_secs}s on this grant")));
    }
    let disk_gb = grant.as_ref().map(|g| g.max_disk_gb).unwrap_or(compute::DEFAULT_MAX_DISK_GB);
    if opts.dry {
        let probe_ctl = store::JobCtl { stall_secs: stall, ..Default::default() };
        let preview = plan_preview(&PlanFacts {
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
        });
        return Ok(Submitted { job: None, warnings, preview: Some(preview) });
    }
    // Source: the workspace's own repo only (never a path the client names
    // outside it, so a passport can't ship another repo off this machine).
    let job_id = crypto::random_id().map_err(|e| no("internal", 500, e))?;
    let src = if b(v, "noSource") == Some(true) {
        None
    } else {
        Some(source_for(ws, &v, &job_id, &mut warnings)?)
    };
    let ctl = store::JobCtl {
        any: any.then_some(filter),
        admin: who.is_admin(),
        timeout_secs: n(v, "timeoutSecs"),
        queue_timeout_secs: queue_timeout,
        queued_at: compute::now(),
        retries,
        stall_secs: stall,
        shard: opts.shard.clone(),
        ..Default::default()
    };
    let stall_secs = crate::compute_place::stall_for(&ctl, &node_features(&node), timeout as u64);
    let plan = Plan {
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
        exclusive: b(v, "exclusive") == Some(true),
        created_at: compute::now(),
        needs: needs.clone(),
        artifacts,
        stall_secs,
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
        store::record_event(
            &conn,
            &who.label(),
            "job_submit",
            Some(&node.id),
            Some(&ws.id),
            Some(&j.id),
            serde_json::json!({"argv": plan.argv, "commit": plan.src.as_ref().map(|s| s.commit.clone()), "detach": detach, "needs": plan.needs,
                "any": any, "retries": retries, "queueTimeoutSecs": queue_timeout, "shard": opts.shard}),
        );
        j
    };
    compute_ws::kick(&node.id);
    compute_ws::notify_change();
    let job = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::job_by_id(&conn, &job.id).unwrap_or(job)
    };
    Ok(Submitted { job: Some(job), warnings, preview: None })
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
}

/// `--plan`: the node it would pick and whether it starts now, the queue
/// in front of it, a warm slot at this commit, an estimate from earlier
/// runs of the same command, and what is left of the grant.
fn plan_preview(f: &PlanFacts<'_>) -> serde_json::Value {
    let now = compute::now();
    let n = f.node;
    let (online, route, live) = compute_ws::live_view(&n.id);
    let offer = live.or_else(|| n.last_offer.clone());
    let cand = crate::compute_place::candidates(f.want, &[], now).into_iter().find(|c| c.node.id == n.id);
    let starts_now = cand.as_ref().is_some_and(|c| c.starts_now(f.want));
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
    let warm = (b(f.v, "noSource") != Some(true))
        .then(|| compute::sync::repo_root(Path::new(&f.ws.path)).ok())
        .flatten()
        .map(|root| {
            let pk = compute::sync::project_key(&f.ws.id, compute::sync::origin_url(&root).as_deref());
            let commit = compute::sync::resolve_commit(&root, s(f.v, "sha").unwrap_or("HEAD")).ok();
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
    serde_json::json!({
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
        "needs": f.needs,
        "artifacts": f.artifacts,
        "warnings": f.warnings,
    })
}

fn pick_node(
    v: &serde_json::Value,
    ws: &Ws,
    want: &crate::compute_place::Want,
    queue_timeout: Option<i64>,
    prefer_not: &[String],
    warnings: &mut Vec<String>,
) -> R<store::NodeRow> {
    if let Some(name) = s(v, "node") {
        let n = node_or_404(Some(name))?;
        if n.state != "active" {
            return Err(no(codes::NODE_NOT_FOUND, 404, format!("'{}' is waiting for its code to be confirmed", n.name)));
        }
        // An ungranted node is refused by the caller with a teaching `not_granted`.
        return Ok(n);
    }
    if b(v, "any") != Some(true) {
        return Err(no(codes::BAD_REQUEST, 400, "name a node, or pass --any"));
    }
    use crate::compute_place::{candidates, choose, Tier};
    let cands = candidates(want, &[], compute::now());
    if cands.is_empty() {
        return Err(no(
            codes::NO_NODE_FITS,
            404,
            "no node granted to this workspace matches --any (os/arch/labels, --needs/--artifact support, queue room); k2 compute nodes",
        ));
    }
    let pk = compute::sync::repo_root(Path::new(&ws.path))
        .ok()
        .map(|root| compute::sync::project_key(&ws.id, compute::sync::origin_url(&root).as_deref()));
    let Some((c, tier)) = choose(&cands, want, prefer_not, pk.as_deref(), None) else {
        return Err(no(
            codes::NO_NODE_FITS,
            404,
            "every node that matches --any is paused, draining or unavailable right now; k2 compute nodes says why",
        ));
    };
    if tier == Tier::Offline {
        warnings.push(format!(
            "no matching node is online: queued on {} (offline); it moves to the first matching node that comes online{}",
            c.node.name,
            queue_timeout.map(|t| format!(", or fails with queue_timeout after {}", compute::message::duration(t))).unwrap_or_default()
        ));
    }
    Ok(c.node.clone())
}

fn source_for(ws: &Ws, v: &serde_json::Value, job_id: &str, warnings: &mut Vec<String>) -> R<Src> {
    let root = compute::sync::repo_root(Path::new(&ws.path))
        .map_err(|_| no(codes::NOT_A_REPO, 400, format!("workspace {} isn't a git repo; pass --no-source to run without code", ws.handle)))?;
    let dirty = b(v, "dirty") == Some(true);
    let rev = s(v, "sha").unwrap_or("HEAD");
    if dirty && rev != "HEAD" {
        return Err(no(codes::BAD_REQUEST, 400, "--dirty sends your uncommitted changes on top of HEAD; it can't be combined with --sha"));
    }
    let commit = compute::sync::resolve_commit(&root, rev).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    let remote = compute::sync::origin_url(&root);
    let blob = if dirty {
        let out = compute_ws::dirty_blob_path(job_id);
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d).map_err(|e| no("internal", 500, format!("blob dir: {e}")))?;
        }
        let (sha, bytes) = compute::sync::create_dirty_blob(&root, &out, compute::MAX_DIRTY_BYTES)
            .map_err(|e| no(codes::DIRTY_TOO_LARGE, 400, e))?;
        Some(Blob { sha256: sha, bytes })
    } else {
        if compute::sync::is_dirty(&root).unwrap_or(false) {
            warnings.push("the working tree has uncommitted changes; they are NOT sent (add --dirty to send them)".into());
        }
        None
    };
    if remote.is_none() {
        warnings.push("no fetchable https origin: the node gets history as git bundles from this server".into());
    }
    let pk = compute::sync::project_key(&ws.id, remote.as_deref());
    Ok(Src { project_key: pk, remote_url: remote, commit, dirty: blob, slots: 1 })
}

// ── jobs ─────────────────────────────────────────────────────────────

fn job_summary(j: &store::JobRow) -> serde_json::Value {
    let node = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::node_by_id(&conn, &j.node_id).map(|n| n.name)
    };
    let mut v = serde_json::to_value(j).unwrap_or_default();
    v["short"] = compute::message::short(&j.id).into();
    v["node"] = node.into();
    v["workspace"] = ws_handle(&j.workspace_id).into();
    v
}

fn job_for(who: &Who, token: Option<&str>) -> R<store::JobRow> {
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

fn jobs_list(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    need_credential(who)?;
    let gate = passport_gate(who)?;
    let node = match params.get("node").map(String::as_str).filter(|s| !s.is_empty()) {
        Some(t) => Some(node_or_404(Some(t))?.id),
        None => None,
    };
    let ws = match gate {
        Some(own) => Some(own),
        None => match params.get("workspace").map(String::as_str).filter(|s| !s.is_empty()) {
            Some(t) => Some(ws_by_token(t).ok_or_else(|| no("workspace_not_found", 404, format!("no workspace '{t}'")))?.id),
            None => None,
        },
    };
    let mine = params.get("mine").is_some_and(|v| v == "1" || v == "true");
    let session = if mine { who.passport().map(|(_, s)| s.to_string()) } else { None };
    let limit = params.get("limit").and_then(|l| l.parse().ok()).unwrap_or(30);
    let state = params.get("state").map(String::as_str).filter(|s| JobState::parse(s).is_some());
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_jobs(&conn, &store::JobFilter { node_id: node.as_deref(), workspace_id: ws.as_deref(), state, session_id: session.as_deref(), limit })
    };
    Ok(serde_json::json!({ "jobs": rows.iter().map(job_summary).collect::<Vec<_>>() }))
}

fn job_get(who: &Who, params: &HashMap<String, String>) -> R<serde_json::Value> {
    let mut j = job_for(who, params.get("job").map(String::as_str))?;
    // `wait=N` (≤ 25 s): return when something changed (a split run's CLI
    // follows its shards this way) or the job ended.
    let wait = params.get("wait").and_then(|w| w.parse::<u64>().ok()).unwrap_or(0).min(25);
    if wait > 0 {
        compute_ws::touch_follower(&j.id);
        if !j.is_terminal() {
            let seen = compute_ws::change_counter();
            compute_ws::wait_change(seen, Duration::from_secs(wait));
            let db = k2_core::db::shared();
            let conn = db.lock();
            j = store::job_by_id(&conn, &j.id).ok_or_else(|| no(codes::JOB_NOT_FOUND, 404, "the job vanished"))?;
        }
    }
    let mut v = job_summary(&j);
    if j.state == "queued" {
        if let Some((pos, why)) = compute_ws::decision_for(&j.node_id).waiting.get(&j.id) {
            v["queue"] = serde_json::json!({"position": pos, "reason": why.code, "message": why.message});
        }
    }
    v["nodeOnline"] = compute_ws::is_online(&j.node_id).into();
    let node = v["node"].as_str().map(str::to_string);
    v["result"] = if j.ctl.shards.is_empty() {
        compute::result::envelope(&j, node.as_deref())
    } else {
        compute::result::envelope_with_shards(&j, &crate::compute_shards::children(&j))
    };
    Ok(v)
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
    if s(v, "node").is_some() || b(v, "any") != Some(true) {
        return Err(no(codes::BAD_REQUEST, 400, "--shards needs --any: each shard goes to whichever node can take it"));
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
                ctl: store::JobCtl { admin: who.is_admin(), queued_at: now, ..Default::default() },
            },
        )
        .map_err(|e| no("internal", 500, e))?;
        store::set_parent_running(&conn, &p.id).map_err(|e| no("internal", 500, e))?;
        p
    };
    let mut kids: Vec<store::JobRow> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
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
                kids.extend(sub.job);
            }
            Err(e) => {
                // All or nothing: stop the shards already placed.
                for k in &kids {
                    let _ = cancel_one(who, k);
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
    Ok(serde_json::json!({ "job": job_summary(&parent), "existing": false, "warnings": warnings, "shards": list }))
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
    let o = compute::result::outputs(&j).unwrap_or_default();
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
    let timeout = Duration::from_secs(60 + info.bytes / (1 << 20));
    let path = compute_ws::fetch_artifact(&j, info, timeout).map_err(|(code, hint)| {
        let c = fetch_code(&code);
        let status = match c {
            "artifact_gone" | "artifact_not_found" => 410,
            "artifact_timeout" | "artifact_unavailable" => 503,
            _ => 409,
        };
        no(c, status, hint)
    })?;
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
            if row.state == "queued" {
                if let Some((pos, why)) = compute_ws::decision_for(&row.node_id).waiting.get(&row.id) {
                    out["queue"] = serde_json::json!({"position": pos, "reason": why.code, "message": why.message});
                }
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
    Ok(serde_json::json!({
        "job": compute::message::short(&j.id),
        "verified": j.receipt_ok == Some(true),
        "receipt": body,
        "signature": signed.sig,
        "proves": "which node said it ran which inputs and got which outputs — not that the node is honest",
    }))
}

fn job_cancel(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    let v = json_body(body)?;
    let j = job_for(who, s(&v, "job"))?;
    if !j.ctl.shards.is_empty() {
        if j.is_terminal() {
            return Err(no("already_ended", 409, format!("job {} already ended ({})", compute::message::short(&j.id), j.state)));
        }
        let mut n = 0;
        for (k, _) in crate::compute_shards::children(&j) {
            if !k.is_terminal() && cancel_one(who, &k).is_ok() {
                n += 1;
            }
        }
        return Ok(serde_json::json!({"ok": true, "state": j.state, "cancel": format!("sent to {n} shard(s)")}));
    }
    cancel_one(who, &j)
}

/// Cancel one job (queued: at once; on a node: the node is told).
fn cancel_one(who: &Who, j: &store::JobRow) -> R<serde_json::Value> {
    if j.is_terminal() {
        return Err(no("already_ended", 409, format!("job {} already ended ({})", compute::message::short(&j.id), j.state)));
    }
    if j.state == "queued" {
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = store::set_job_state(&conn, &j.id, JobState::Cancelled, Some("cancelled"), None, compute::now());
            store::record_event(&conn, &who.label(), "job_cancel", Some(&j.node_id), Some(&j.workspace_id), Some(&j.id), serde_json::json!({"state": "queued"}));
        }
        compute_ws::finalize(&j.id);
        compute_ws::notify_change();
        return Ok(serde_json::json!({"ok": true, "state": "cancelled"}));
    }
    let sent = compute_ws::send_cancel(&j.node_id, &j.id, j.generation);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_job_reason(&conn, &j.id, Some("cancel_requested"), None);
        store::record_event(&conn, &who.label(), "job_cancel", Some(&j.node_id), Some(&j.workspace_id), Some(&j.id), serde_json::json!({"state": j.state, "sent": sent}));
    }
    compute_ws::notify_change();
    Ok(serde_json::json!({"ok": true, "state": j.state, "cancel": if sent { "sent" } else { "delivered when the node reconnects" }}))
}

fn job_retry(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    let v = json_body(body)?;
    let j = job_for(who, s(&v, "job"))?;
    if !j.is_terminal() {
        return Err(no("still_running", 409, "the job hasn't ended; cancel it first"));
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
    if updates.iter().any(|(k, v)| k == "network" && v.trim() == "none") {
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
    };
    crate::compute_local::enable(&req, &who.label()).map_err(|(code, status, hint)| no(code, status, hint))
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
}
