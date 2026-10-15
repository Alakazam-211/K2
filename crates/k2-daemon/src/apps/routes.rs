//! `/cli/app/*` runtime routes (`prd-k2-app-runtime-v1` §8.5, stage 1).
//!
//! Who may do what:
//! - Route floors (`route_policy.rs`) stop Connect logins below the role.
//! - Passports (agents) act only on their own workspace's Apps and
//!   folders. Changing anything (build, deploy, …) also needs the
//!   workspace switch "Allow this agent to manage Apps"
//!   (`agents_can_manage_skin`, §5.11.1): without it the call is refused
//!   `apps_manage_off`. (The held-request path for agents without the
//!   switch is S42; widenings held for the owner are S20.)
//! - A plain K2 shell tab is a human: treated as the owner (NT5).
//! - App passes, API keys and API sessions are refused (`wrong_credential`).
//!
//! The TCP dispatcher and the per-cell socket both classify the caller as
//! a [`Who`] (the compute classifier) and call [`handle`] (blocking).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use k2_core::connect_users::Role;
use k2_core::deploy::store::{self, AppRow};
use k2_core::deploy::{self, codes};
use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;
pub use crate::compute_routes::Who;

/// Largest request body.
pub const MAX_BODY: usize = 256 * 1024;

/// Every runtime route.
pub const ROUTES: &[&str] = &[
    "/cli/app/box",
    "/cli/app/box/prepare",
    "/cli/app/build",
    "/cli/app/config",
    "/cli/app/deploy",
    "/cli/app/deployment",
    "/cli/app/engine/install",
    "/cli/app/errors",
    "/cli/app/history",
    "/cli/app/inspect",
    "/cli/app/install",
    "/cli/app/link",
    "/cli/app/link/rotate",
    "/cli/app/list",
    "/cli/app/logs",
    "/cli/app/restart",
    "/cli/app/rollback",
    "/cli/app/start",
    "/cli/app/stop",
    "/cli/app/uninstall",
    "/cli/app/validate",
];

/// POST-only rows (`require_post` → 405 on GET).
pub const POST_ROUTES: &[&str] = &[
    "/cli/app/box/prepare",
    "/cli/app/build",
    "/cli/app/config",
    "/cli/app/deploy",
    "/cli/app/engine/install",
    "/cli/app/install",
    "/cli/app/link/rotate",
    "/cli/app/restart",
    "/cli/app/rollback",
    "/cli/app/start",
    "/cli/app/stop",
    "/cli/app/uninstall",
    "/cli/app/validate",
];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

// ── refusals ─────────────────────────────────────────────────────────

pub(crate) struct Refusal {
    pub code: &'static str,
    pub status: u16,
    pub hint: String,
    pub extra: Option<J>,
}

pub(crate) type R<T> = Result<T, Refusal>;

pub(crate) fn no(code: &'static str, status: u16, hint: impl Into<String>) -> Refusal {
    Refusal { code, status, hint: hint.into(), extra: None }
}

impl From<super::supervisor::Fail> for Refusal {
    fn from(f: super::supervisor::Fail) -> Self {
        no(f.code, f.status, f.message)
    }
}

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        202 => "202 Accepted",
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        422 => "422 Unprocessable Entity",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

fn refusal_response(r: &Refusal) -> CliResponse {
    let mut err = json!({ "code": r.code, "hint": r.hint });
    if let Some(J::Object(m)) = &r.extra {
        for (k, v) in m {
            err[k] = v.clone();
        }
    }
    CliResponse { status: status_line(r.status), content_type: "application/json", body: json!({ "error": err }).to_string() }
}

pub fn too_large() -> CliResponse {
    refusal_response(&no(codes::BAD_REQUEST, 413, format!("request bodies here are at most {} KiB", MAX_BODY / 1024)))
}

/// 404 for every runtime route while the preview flag is off.
pub fn dark() -> CliResponse {
    refusal_response(&no(
        codes::APPS_OFF,
        404,
        "the App runtime is a preview: the owner turns it on with K2_APPS=1 or the Settings switch appsPreview \
         (our own boxes only in this release)",
    ))
}

pub(crate) fn db_err(e: impl std::fmt::Display) -> Refusal {
    no("db_error", 500, e.to_string())
}

// ── who ──────────────────────────────────────────────────────────────

pub(crate) fn ws_handle(project_id: &str) -> String {
    k2_core::workspace_session_handles::workspace_address_name_shared(project_id).unwrap_or_else(|_| project_id.to_string())
}

/// The audit actor for a caller (`agent:<ws>` | `owner` | `user:<name>`).
pub(crate) fn actor(who: &Who) -> String {
    match who {
        Who::Owner => "owner".into(),
        Who::Login { username, role: Role::Owner } => format!("owner:{username}"),
        Who::Login { username, .. } => format!("user:{username}"),
        Who::Agent { project_id, .. } => format!("agent:{}", ws_handle(project_id)),
        Who::Shell { project_id, .. } => format!("shell:{}", ws_handle(project_id)),
        Who::Wrong => "wrong-credential".into(),
        Who::Invalid => "-".into(),
    }
}

pub(crate) fn need_credential(who: &Who) -> R<()> {
    match who {
        Who::Invalid => Err(no(codes::FORBIDDEN, 403, "invalid or missing token")),
        Who::Wrong => Err(no(codes::WRONG_CREDENTIAL, 403, "App passes, API keys and API sessions can't manage Apps")),
        _ => Ok(()),
    }
}

/// The agent's workspace when the caller is an agent passport (a shell
/// tab is a human and counts as the owner, NT5).
pub(crate) fn agent_ws(who: &Who) -> Option<&str> {
    match who {
        Who::Agent { project_id, .. } => Some(project_id.as_str()),
        _ => None,
    }
}

/// Agents need "Allow this agent to manage Apps" on their workspace to
/// change anything (§5.11.1). Humans pass.
pub(crate) fn need_manage(who: &Who) -> R<()> {
    need_credential(who)?;
    let Some(ws) = agent_ws(who) else { return Ok(()) };
    let path = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::get(&conn, ws).map(|p| p.path).map_err(|_| no(codes::FORBIDDEN, 403, "your workspace is not registered"))?
    };
    if !k2_core::workspace::settings::agents_can_manage_skin_for_path(&path) {
        return Err(no(
            codes::APPS_MANAGE_OFF,
            403,
            "Apps are off for this agent: ask your owner to turn on \"Allow this agent to manage Apps\" \
             in Workspace settings → Agent (k2 app access arrives with the admin switches)",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct Ws {
    pub id: String,
    pub path: String,
}

pub(crate) fn ws_by_id(id: &str) -> Option<Ws> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let p = k2_core::db::schema::Project::get(&conn, id).ok()?;
    Some(Ws { id: p.id, path: p.path })
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

/// The workspace a call acts for: a passport's own (naming another is
/// `forbidden`); others name one with `workspace`.
pub(crate) fn target_ws(who: &Who, requested: Option<&str>) -> R<Ws> {
    let requested = requested.map(str::trim).filter(|s| !s.is_empty());
    let own = match who {
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => Some(project_id.as_str()),
        _ => None,
    };
    if let Some(own) = own {
        let ws = ws_by_id(own).ok_or_else(|| no(codes::FORBIDDEN, 404, "your workspace is not registered"))?;
        if let Some(req) = requested {
            if ws_by_token(req).is_none_or(|o| o.id != ws.id) {
                return Err(no(codes::FORBIDDEN, 403, "an agent acts only for its own workspace"));
            }
        }
        return Ok(ws);
    }
    let req = requested.ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the workspace (--workspace)"))?;
    ws_by_token(req).ok_or_else(|| no(codes::BAD_REQUEST, 404, format!("no workspace '{req}'")))
}

/// A folder an agent names must be inside its own workspace.
pub(crate) fn resolve_folder(who: &Who, raw: &str) -> R<PathBuf> {
    let p = Path::new(raw.trim());
    if !p.is_absolute() {
        return Err(no(codes::BAD_REQUEST, 400, "give the folder as an absolute path"));
    }
    let canon = std::fs::canonicalize(p).map_err(|e| no(codes::BAD_REQUEST, 400, format!("{}: {e}", p.display())))?;
    if let Some(ws) = agent_ws(who) {
        let w = ws_by_id(ws).ok_or_else(|| no(codes::FORBIDDEN, 404, "your workspace is not registered"))?;
        let root = std::fs::canonicalize(&w.path).unwrap_or_else(|_| PathBuf::from(&w.path));
        if !canon.starts_with(&root) {
            return Err(no(
                codes::FOLDER_OUTSIDE_WORKSPACE,
                403,
                format!("an agent builds Apps only from folders inside its workspace ({})", root.display()),
            ));
        }
    }
    Ok(canon)
}

/// The signer recorded on a Seed (§9.1 `seeds.signer`).
pub(crate) fn signer(who: &Who) -> String {
    match who {
        Who::Agent { project_id, .. } => format!("agent:{}", ws_handle(project_id)),
        Who::Login { username, role: Role::Owner } => format!("owner:{username}"),
        Who::Login { username, .. } => format!("user:{username}"),
        _ => "owner".into(),
    }
}

// ── body helpers ─────────────────────────────────────────────────────

pub(crate) fn json_body(body: &[u8]) -> R<J> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    let v: J = serde_json::from_slice(body).map_err(|e| no(codes::BAD_REQUEST, 400, format!("invalid JSON body: {e}")))?;
    if !v.is_object() {
        return Err(no(codes::BAD_REQUEST, 400, "expected a JSON object"));
    }
    Ok(v)
}

pub(crate) fn s<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str()).map(str::trim).filter(|x| !x.is_empty())
}

pub(crate) fn b(v: &J, k: &str) -> bool {
    v.get(k).and_then(|x| x.as_bool()).unwrap_or(false)
}

fn validate_opts(v: &J) -> deploy::validate::Options {
    deploy::validate::Options { types: b(v, "types"), checker: None }
}

// ── entry ────────────────────────────────────────────────────────────

/// Route a runtime request. Blocking.
pub fn handle(path: &str, who: Who, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if !deploy::enabled() {
        return dark();
    }
    super::supervisor::ensure_started();
    let r = match path {
        // Box prep (S10): the doctor for anyone with a credential, prepare
        // for the owner only.
        "/cli/app/box" => super::box_prep::box_route(&who),
        "/cli/app/box/prepare" => json_body(body).and_then(|v| super::box_prep::prepare_route(&who, &v)),
        // The guarded engine (S11): the owner installs or updates it.
        "/cli/app/engine/install" => json_body(body).and_then(|v| super::engine_install::install_route(&who, &v)),
        "/cli/app/build" => build(&who, body),
        "/cli/app/validate" => validate(&who, body),
        "/cli/app/install" => install(&who, body),
        "/cli/app/deploy" => deploy_route(&who, body),
        "/cli/app/rollback" => rollback(&who, body),
        "/cli/app/stop" => stop(&who, body),
        "/cli/app/start" => start(&who, body, store::KIND_START),
        "/cli/app/restart" => start(&who, body, store::KIND_RESTART),
        "/cli/app/deployment" => deployment(&who, params),
        "/cli/app/config" => config(&who, body),
        "/cli/app/list" => list(&who, params),
        "/cli/app/inspect" => inspect(&who, params),
        "/cli/app/history" => history(&who, params),
        "/cli/app/logs" => logs(&who, params),
        "/cli/app/errors" => errors(&who, params),
        "/cli/app/uninstall" => uninstall(&who, body),
        "/cli/app/link" => link(&who, params),
        "/cli/app/link/rotate" => link_rotate(&who, body),
        _ => Err(no("not_found", 404, "no such App route")),
    };
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refusal_response(&e),
    }
}

/// POST `/cli/app/build {folder, types?}` → the Seed (S6). Deploys nothing.
fn build(who: &Who, body: &[u8]) -> R<J> {
    need_manage(who)?;
    let v = json_body(body)?;
    let folder = resolve_folder(who, s(&v, "folder").ok_or_else(|| no(codes::BAD_REQUEST, 400, "folder is required"))?)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    match deploy::build::build_folder(&conn, &folder, &signer(who), &validate_opts(&v)) {
        Ok(built) => Ok(json!({ "ok": true, "seed": built.seed, "reused": built.reused, "entries": built.entries })),
        Err(f) => {
            let mut r = no(f.code, 422, f.message);
            r.extra = f.report.map(|rep| json!({ "report": rep }));
            Err(r)
        }
    }
}

/// POST `/cli/app/validate {folder | seed, types?}` → the report. Changes
/// nothing (POST only for the body).
fn validate(who: &Who, body: &[u8]) -> R<J> {
    need_credential(who)?;
    let v = json_body(body)?;
    let opts = validate_opts(&v);
    let (root, hash) = if let Some(f) = s(&v, "folder") {
        (resolve_folder(who, f)?, None)
    } else if let Some(t) = s(&v, "seed") {
        let row = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            deploy::store::find_seed(&conn, t).map_err(db_err)?
        }
        .ok_or_else(|| no(codes::SEED_NOT_FOUND, 404, format!("no Seed '{t}' (k2 app build prints the hash)")))?;
        deploy::seed::verify(&row.hash).map_err(|e| no(e.code, 409, e.message))?;
        (deploy::seed::seed_path(&row.hash), Some(row.hash))
    } else {
        return Err(no(codes::BAD_REQUEST, 400, "give a folder or a seed"));
    };
    let (report, manifest) = deploy::validate::validate_tree(&root, &opts);
    Ok(json!({
        "ok": report.ok,
        "report": report,
        "seed": hash,
        "id": manifest.as_ref().map(|m| m.id.clone()),
        "version": manifest.as_ref().map(|m| m.version.clone()),
    }))
}

// ── S7: install, deploy, rollback, stop/start/restart, config ────────

/// The App a call names. A passport sees only its own workspace's Apps (a
/// foreign App is `app_not_found`, never "forbidden": no existence leak).
pub(crate) fn app_for(who: &Who, app_id: Option<&str>) -> R<AppRow> {
    let id = app_id.ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the App (its App id)"))?.to_ascii_lowercase();
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_app(&conn, &id).map_err(db_err)?
    };
    let row = row.ok_or_else(|| no(codes::APP_NOT_FOUND, 404, format!("no App '{id}' on this box (k2 app list)")))?;
    if let Some(ws) = agent_ws(who) {
        if row.owner_workspace != ws {
            return Err(no(codes::APP_NOT_FOUND, 404, format!("no App '{id}' in your workspace (k2 app list)")));
        }
    }
    Ok(row)
}

/// `~/.k2/apps/<id>/app.json`: a read-only mirror of the row (§4.4).
/// Editing it does nothing.
pub(crate) fn mirror(app_id: &str) {
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_app(&conn, app_id).ok().flatten()
    };
    if let Some(r) = row {
        let p = deploy::app_dir(app_id).join("app.json");
        let mut v = serde_json::to_value(&r).unwrap_or(J::Null);
        v["_note"] = json!("Read-only mirror of the App's row. Change it with `k2 app config`; editing this file does nothing.");
        let _ = std::fs::create_dir_all(deploy::app_dir(app_id));
        let _ = k2_core::fs_atomic::atomic_write_str(&p, &(serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"));
    }
}

fn audit_human(app_id: &str, event: &str, who: &Who, payload: J) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, app_id, event, &actor(who), None, &payload) {
        k2_core::log_debug!("[apps] audit {app_id} {event}: {e}");
    }
}

/// POST `/cli/app/install {app, workspace?, tunnel?, memory?, seed?|folder?}`.
/// Creates the App (its `published_services` row with the `(app)`
/// sentinel, AR5), its folder and its front listener; with `tunnel` the
/// URL `<app>.<sub>.k2.dev` points at the front port (one control-plane
/// write, here, never per deploy). With a Seed or folder it deploys too.
fn install(who: &Who, body: &[u8]) -> R<J> {
    need_manage(who)?;
    let v = json_body(body)?;
    let app_id = s(&v, "app").ok_or_else(|| no(codes::BAD_REQUEST, 400, "app is required (the App id)"))?.to_ascii_lowercase();
    if let Some((code, msg)) = deploy::app_id_problem(&app_id) {
        return Err(no(code, 400, msg));
    }
    let ws = target_ws(who, s(&v, "workspace"))?;
    let memory = deploy::parse_memory(s(&v, "memory").unwrap_or("none")).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    let tunnel = b(&v, "tunnel");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if store::get_app(&conn, &app_id).map_err(db_err)?.is_some() {
            return Err(no(codes::APP_EXISTS, 409, format!("App '{app_id}' already exists on this box (k2 app deploy {app_id} …)")));
        }
        let holders: Vec<String> = {
            let mut st = conn.prepare("SELECT project_id FROM published_services WHERE name = ?1").map_err(db_err)?;
            let rows = st.query_map(rusqlite::params![app_id], |r| r.get::<_, String>(0)).map_err(db_err)?;
            rows.collect::<Result<_, _>>().map_err(db_err)?
        };
        if holders.iter().any(|h| h == &ws.id) {
            return Err(no(
                codes::APP_EXISTS,
                409,
                format!("this workspace already publishes a service named '{app_id}'; pick another App id (adopting a published service comes later)"),
            ));
        }
        if !holders.is_empty() {
            return Err(no(codes::LABEL_TAKEN, 409, format!("'{app_id}' is already a published label on this box; App ids are unique box-wide")));
        }
    }
    crate::label_owner::check(&app_id, &ws.id, false).map_err(|r| no(codes::LABEL_TAKEN, 409, r.message()))?;
    let tunnel_cfg = if tunnel {
        Some(crate::publish_runtime::nested_preflight().map_err(|e| no(codes::ENGINE_UNAVAILABLE, 503, e.message))?)
    } else {
        None
    };
    let dir = deploy::app_dir(&app_id);
    std::fs::create_dir_all(dir.join("logs")).map_err(|e| no("io_error", 500, e.to_string()))?;
    let actor_s = actor(who);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let svc = k2_core::published_services::insert(
            &conn,
            &ws.id,
            &app_id,
            k2_core::published_services::CMD_APP_SENTINEL,
            &dir.to_string_lossy(),
            0,
            if tunnel { k2_core::published_services::EXPOSE_TUNNEL } else { k2_core::published_services::EXPOSE_LOCAL },
            k2_core::published_services::DESIRED_STOPPED,
            k2_core::published_services::KIND_CMD,
            "",
        )
        .map_err(db_err)?;
        store::insert_app(
            &conn,
            &store::NewApp {
                app_id: &app_id,
                service_id: &svc.id,
                seed_id: None,
                owner_workspace: &ws.id,
                isolation: "process",
                memory_limit_bytes: memory,
                front_port: 0,
                created_by: &actor_s,
            },
        )
        .map_err(db_err)?;
        store::set_state(&conn, &app_id, store::STATE_STOPPED, Some("installed; no Seed deployed yet")).map_err(db_err)?;
    }
    let undo = |why: String| -> Refusal {
        super::supervisor::forget(&app_id);
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = store::delete_app(&conn, &app_id);
        let _ = k2_core::published_services::delete(&conn, &ws.id, &app_id);
        no(codes::ENGINE_UNAVAILABLE, 503, why)
    };
    let row = app_for(&Who::Owner, Some(&app_id))?;
    let port = match super::supervisor::ensure_front(&row) {
        Ok(p) => p,
        Err(f) => return Err(undo(f.message)),
    };
    if let Some((cfg, _)) = tunnel_cfg {
        if let Err(e) = crate::publish_runtime::attach_hostname(&ws.id, &app_id, port, &cfg.token, &cfg.subdomain, false) {
            return Err(undo(format!("the URL step failed: {e}")));
        }
    }
    audit_human(&app_id, "install", who, json!({ "workspace": ws.id, "tunnel": tunnel, "frontPort": port, "memoryLimitBytes": memory }));
    mirror(&app_id);
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::PublishServicesChanged { project_id: ws.id.clone() });
    let mut out = json!({ "ok": true, "app": app_for(&Who::Owner, Some(&app_id))?, "frontPort": port });
    if s(&v, "seed").is_some() || s(&v, "folder").is_some() {
        let mut dv = v.clone();
        dv["app"] = json!(app_id);
        let d = deploy_inner(who, &dv)?;
        out["deploy"] = d;
    }
    Ok(out)
}

/// The Seed a deploy names: a stored one, or a folder built now.
fn seed_for_deploy(who: &Who, v: &J) -> R<String> {
    if let Some(f) = s(v, "folder") {
        let folder = resolve_folder(who, f)?;
        let db = k2_core::db::shared();
        let conn = db.lock();
        return match deploy::build::build_folder(&conn, &folder, &signer(who), &validate_opts(v)) {
            Ok(b) => Ok(b.seed.hash),
            Err(f) => {
                let mut r = no(f.code, 422, f.message);
                r.extra = f.report.map(|rep| json!({ "report": rep }));
                Err(r)
            }
        };
    }
    let t = s(v, "seed").ok_or_else(|| no(codes::BAD_REQUEST, 400, "give a seed (hash) or a folder"))?;
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::find_seed(&conn, t).map_err(db_err)?
    }
    .ok_or_else(|| no(codes::SEED_NOT_FOUND, 404, format!("no Seed '{t}' (k2 app build prints the hash)")))?;
    if row.report.get("ok").and_then(|x| x.as_bool()) != Some(true) {
        return Err(no(codes::SEED_INVALID, 422, "this Seed's validate report has errors; it can't be deployed"));
    }
    Ok(row.hash)
}

/// `{deployment, done}`; waits up to `wait` seconds (max 900).
fn deployment_answer(id: &str, wait: u64) -> R<J> {
    let row = if wait > 0 {
        super::supervisor::wait_done(id, wait.min(900))
    } else {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_deployment(&conn, id).map_err(db_err)?
    }
    .ok_or_else(|| no("not_found", 404, format!("no deployment '{id}'")))?;
    let done = !matches!(row.result.as_str(), store::RESULT_PENDING | store::RESULT_STARTING);
    Ok(json!({ "ok": true, "deployment": row, "done": done }))
}

fn wait_secs(v: &J) -> u64 {
    v.get("wait").and_then(|x| x.as_u64()).unwrap_or(0)
}

/// `engine`: `guarded` (the container floor, S12) or `process` (stage 1).
/// Absent: the App's current engine. A change is recorded and audited.
fn apply_engine(who: &Who, app: AppRow, v: &J) -> R<AppRow> {
    let Some(e) = s(v, "engine") else { return Ok(app) };
    let iso = match e {
        "guarded" | "container" => "container",
        "process" => "process",
        other => return Err(no(codes::BAD_REQUEST, 400, format!("engine is guarded or process, not '{other}'"))),
    };
    if iso == app.isolation {
        return Ok(app);
    }
    if iso == "container" {
        if !cfg!(target_os = "linux") {
            return Err(no(codes::ENGINE_UNAVAILABLE, 409, "the guarded engine runs on Linux boxes only"));
        }
        if k2_root_proto::apps::AppId::parse(&app.app_id).is_none() {
            return Err(no(
                codes::APP_ID_INVALID,
                409,
                format!("App id '{}' doesn't fit the guarded engine (at most 32 characters, no '--')", app.app_id),
            ));
        }
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_isolation(&conn, &app.app_id, iso).map_err(db_err)?;
    }
    audit_human(&app.app_id, "engine", who, json!({ "isolation": iso, "was": app.isolation }));
    mirror(&app.app_id);
    app_for(who, Some(&app.app_id))
}

fn deploy_inner(who: &Who, v: &J) -> R<J> {
    let app = app_for(who, s(v, "app"))?;
    let app = apply_engine(who, app, v)?;
    let seed = seed_for_deploy(who, v)?;
    let dep = super::supervisor::begin(&app, &seed, store::KIND_DEPLOY, &actor(who), s(v, "reason"))?;
    deployment_answer(&dep, wait_secs(v))
}

/// POST `/cli/app/deploy {app, seed|folder, reason?, wait?, types?}`.
fn deploy_route(who: &Who, body: &[u8]) -> R<J> {
    need_manage(who)?;
    let v = json_body(body)?;
    deploy_inner(who, &v)
}

/// POST `/cli/app/rollback {app, to?, wait?}` (§7.3): instant when the
/// previous instance is still warm; otherwise through the health gate.
fn rollback(who: &Who, body: &[u8]) -> R<J> {
    need_manage(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let target = super::supervisor::rollback_target(&app, s(&v, "to"))?;
    super::supervisor::check_data_revision(&app, &target)?;
    if let Some(id) = super::supervisor::rollback_to_warm(&app, &target, &actor(who))? {
        let mut out = deployment_answer(&id, 0)?;
        out["instant"] = json!(true);
        return Ok(out);
    }
    let dep = super::supervisor::begin(&app, &target, store::KIND_ROLLBACK, &actor(who), s(&v, "reason"))?;
    deployment_answer(&dep, wait_secs(&v))
}

/// POST `/cli/app/stop {app}`: drain, stop, no restarts.
fn stop(who: &Who, body: &[u8]) -> R<J> {
    need_manage(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    super::supervisor::stop_app(&app, &actor(who))?;
    mirror(&app.app_id);
    Ok(json!({ "ok": true, "app": app.app_id, "state": store::STATE_STOPPED }))
}

/// POST `/cli/app/start|restart {app, wait?}`: start the live Seed again
/// (restart is zero-downtime: the new instance passes the gate first).
fn start(who: &Who, body: &[u8], kind: &'static str) -> R<J> {
    need_manage(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let seed = app
        .live_seed
        .clone()
        .ok_or_else(|| no(codes::NOTHING_TO_ROLL_BACK, 409, format!("{} has no Seed deployed yet (k2 app deploy {} <folder|seed>)", app.app_id, app.app_id)))?;
    if kind == store::KIND_START && super::supervisor::live_instance(&app.app_id).is_some() {
        return Ok(json!({ "ok": true, "app": app.app_id, "already": "running" }));
    }
    super::supervisor::reset_backoff(&app.app_id);
    let dep = super::supervisor::begin(&app, &seed, kind, &actor(who), s(&v, "reason"))?;
    deployment_answer(&dep, wait_secs(&v))
}

/// GET `/cli/app/deployment?app=&id=` (litmus L3): who, commit, state
/// (live / failed / stalled / …) of one deployment, or the newest.
fn deployment(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match params.get("id").map(|x| x.trim()).filter(|x| !x.is_empty()) {
            Some(id) => store::get_deployment(&conn, id).map_err(db_err)?.filter(|d| d.app_id == app.app_id),
            None => store::list_deployments(&conn, &app.app_id, 1).map_err(db_err)?.into_iter().next(),
        }
    }
    .ok_or_else(|| no("not_found", 404, format!("no such deployment of {}", app.app_id)))?;
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "deployment": row,
        "who": row.actor,
        "commit": row.commit_ref,
        "state": row.result,
    }))
}

/// POST `/cli/app/config {app, memory}`: stage 1 holds only the per-App
/// memory limit (`none` by default; the user decides). The rest of §4.4 is
/// S13.
fn config(who: &Who, body: &[u8]) -> R<J> {
    need_manage(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let raw = v.get("memory").ok_or_else(|| no(codes::BAD_REQUEST, 400, "nothing to change (memory: 8G | 512M | none)"))?;
    let raw = match raw {
        J::Null => "none".to_string(),
        J::String(x) => x.clone(),
        other => other.to_string(),
    };
    let bytes = deploy::parse_memory(&raw).map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_memory_limit(&conn, &app.app_id, bytes).map_err(db_err)?;
    }
    audit_human(&app.app_id, "config", who, json!({ "memoryLimitBytes": bytes, "was": app.memory_limit_bytes }));
    mirror(&app.app_id);
    Ok(json!({
        "ok": true,
        "app": app_for(who, Some(&app.app_id))?,
        "note": "The process engine in this preview doesn't enforce memory limits; the walled engines do (a later 0.46 release).",
    }))
}

// ── S8: list, inspect, history, logs, errors, uninstall ──────────────

/// Largest framed log or error text returned (the tail is kept).
const LOG_CAP: usize = 256 * 1024;

/// What a stage-1 App's wall is (the §4.5 label function arrives with the
/// walls in S13; preview Apps are always this).
fn label(app: &AppRow) -> &'static str {
    match app.isolation.as_str() {
        "process" => "Preview, unwalled (process)",
        "container" => "Guarded (container)",
        "none" => "Dev (no wall)",
        _ => "Preview",
    }
}

fn url_for(app: &AppRow) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = k2_core::published_services::get_by_id(&conn, &app.service_id).ok().flatten()?;
    let svc = crate::publish_runtime::to_json(&row);
    svc.url
}

fn app_summary(app: &AppRow) -> J {
    json!({
        "appId": app.app_id,
        "serviceId": app.service_id,
        "workspace": app.owner_workspace,
        "workspaceHandle": ws_handle(&app.owner_workspace),
        "state": app.state,
        "stateReason": app.state_reason,
        "liveSeed": app.live_seed,
        "liveDeployment": app.live_deployment,
        "frontPort": app.front_port,
        "memoryLimitBytes": app.memory_limit_bytes,
        "isolation": app.isolation,
        "label": label(app),
        "url": url_for(app),
        "livePort": super::supervisor::live_port(&app.app_id),
        "link": super::gateway::status_json(&app.app_id),
    })
}

/// GET `/cli/app/link?app=` (S14): the App's gateway link — its port, the
/// in-wall URL, the pass's caps, rooms, expiry and rotation, counters.
/// Never the pass itself.
fn link(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    Ok(json!({ "ok": true, "app": app.app_id, "link": super::gateway::status_json(&app.app_id) }))
}

/// POST `/cli/app/link/rotate {app}` (S14): hand the App a new pass now
/// (the old one keeps working until its own expiry). It only refreshes,
/// so the manage-Apps switch is enough for an agent's own App.
fn link_rotate(who: &Who, body: &[u8]) -> R<J> {
    need_manage(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let expires = super::gateway::rotate(&app.app_id).map_err(|e| no("link_unavailable", 409, e))?;
    audit_human(&app.app_id, "link_rotate", who, json!({ "expiresAt": expires }));
    Ok(json!({ "ok": true, "app": app.app_id, "passExpiresAt": expires, "link": super::gateway::status_json(&app.app_id) }))
}

/// Published labels an App id could collide with (§8.4, AR7): reserved
/// words, and labels used by more than one workspace. Checked at daemon
/// boot (logged) and shown by `k2 app list`.
pub fn upgrade_notices() -> Vec<J> {
    let rows: Vec<(String, String)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Ok(mut st) = conn.prepare("SELECT name, project_id FROM published_services WHERE cmd != '(app)' ORDER BY name") else {
            return vec![];
        };
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|it| it.flatten().collect()).unwrap_or_default()
    };
    let mut by_name: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (n, p) in rows {
        by_name.entry(n).or_default().push(p);
    }
    let mut out = Vec::new();
    for (name, owners) in by_name {
        if let Some((code, _)) = deploy::app_id_problem(&name) {
            if code == codes::APP_ID_RESERVED {
                out.push(json!({
                    "label": name,
                    "issue": "reserved",
                    "hint": format!("'{name}' is a k2 app subcommand or k2 noun, so it can't become an App id; rename it before adopting (k2 publish rm, then run it under another name)"),
                }));
            }
        }
        if owners.len() > 1 {
            out.push(json!({
                "label": name,
                "issue": "duplicate",
                "workspaces": owners.iter().map(|o| ws_handle(o)).collect::<Vec<_>>(),
                "hint": format!("'{name}' is published by {} workspaces; App ids are unique on the box, so all but one must rename before adopting", owners.len()),
            }));
        }
    }
    out
}

/// Log the upgrade notices once (daemon boot).
pub fn log_upgrade_notices() {
    for n in upgrade_notices() {
        k2_core::log_debug!("[apps] upgrade check: {}", n["hint"].as_str().unwrap_or(""));
    }
}

/// GET `/cli/app/list[?workspace=]`: a passport sees its own workspace's
/// Apps; others see every App (or one workspace's).
fn list(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let filter = match agent_ws(who) {
        Some(ws) => Some(ws.to_string()),
        None => match params.get("workspace").map(|s| s.trim()).filter(|s| !s.is_empty()) {
            Some(w) => Some(ws_by_token(w).ok_or_else(|| no(codes::BAD_REQUEST, 404, format!("no workspace '{w}'")))?.id),
            None => None,
        },
    };
    let apps = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_apps(&conn, filter.as_deref()).map_err(db_err)?
    };
    Ok(json!({
        "ok": true,
        "apps": apps.iter().map(app_summary).collect::<Vec<_>>(),
        "notices": if agent_ws(who).is_some() { vec![] } else { upgrade_notices() },
    }))
}

/// GET `/cli/app/inspect?app=`: the row, the runtime, the live Seed
/// (manifest, `createdWith`, report), the last deployment, and whether the
/// deploy audit chain verifies.
fn inspect(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let seed = match app.live_seed.as_deref() {
        Some(h) => store::get_seed(&conn, h).map_err(db_err)?,
        None => None,
    };
    let last = store::list_deployments(&conn, &app.app_id, 1).map_err(db_err)?.into_iter().next();
    let chain = deploy::audit::verify(&conn).map_err(db_err)?;
    Ok(json!({
        "ok": true,
        "app": app_summary(&app),
        "row": app,
        "runtime": super::supervisor::runtime_json(&app.app_id),
        "liveSeed": seed,
        "lastDeployment": last,
        "audit": match chain {
            Ok(n) => json!({ "verified": true, "rows": n }),
            Err(b) => json!({ "verified": false, "brokenAt": b.seq, "reason": b.reason }),
        },
    }))
}

/// GET `/cli/app/history?app=[&limit=]` (who / from / to / why / result),
/// or `?widget=<name>` for a Garden widget's deploy-audit rows (S9).
fn history(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let limit = params.get("limit").and_then(|l| l.parse::<usize>().ok()).unwrap_or(20).clamp(1, 200);
    if let Some(w) = params.get("widget").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if agent_ws(who).is_some() {
            return Err(no(codes::OWNER_ONLY, 403, "Garden widget history is the desktop owner's (k2 zen history --widget)"));
        }
        let db = k2_core::db::shared();
        let conn = db.lock();
        let rows = deploy::audit::list_for(&conn, deploy::audit::SUBJECT_GARDEN_WIDGET, w, limit).map_err(db_err)?;
        return Ok(json!({ "ok": true, "widget": w, "audit": rows }));
    }
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let deps = store::list_deployments(&conn, &app.app_id, limit).map_err(db_err)?;
    let audit = deploy::audit::list_for(&conn, deploy::audit::SUBJECT_APP, &app.app_id, limit).map_err(db_err)?;
    let chain = deploy::audit::verify(&conn).map_err(db_err)?;
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "liveSeed": app.live_seed,
        "deployments": deps,
        "audit": audit,
        "auditVerified": chain.is_ok(),
    }))
}

fn read_log(app_id: &str, deployment: &str) -> String {
    let p = deploy::deployment_log_path(app_id, deployment);
    let mut text = std::fs::read_to_string(p.with_extension("log.1")).unwrap_or_default();
    text.push_str(&std::fs::read_to_string(&p).unwrap_or_default());
    // A guarded instance's own output is in its unit's journal (rate
    // limited per unit, W §5.3); the file holds K2's lines about it.
    let guarded = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_app(&conn, app_id).ok().flatten().is_some_and(|a| a.isolation == "container")
    };
    if guarded && k2_root_proto::apps::AppId::parse(app_id).is_some() {
        match super::guarded::logs(app_id, deployment, 5000) {
            Ok(j) => text.push_str(&j),
            Err(e) => text.push_str(&format!("[k2] the engine's journal for this deployment isn't readable: {}\n", e.message)),
        }
    }
    text
}

/// GET `/cli/app/logs?app=[&deployment=][&tail=N]`: one deployment's log
/// (default: the live one, else the newest), framed as untrusted (§5.9).
fn logs(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let dep = match params.get("deployment").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(d) => d.to_string(),
        None => match app.live_deployment.clone() {
            Some(d) => d,
            None => {
                let db = k2_core::db::shared();
                let conn = db.lock();
                store::list_deployments(&conn, &app.app_id, 1)
                    .map_err(db_err)?
                    .into_iter()
                    .next()
                    .map(|d| d.id)
                    .ok_or_else(|| no("not_found", 404, format!("{} has no deployments yet", app.app_id)))?
            }
        },
    };
    if !dep.starts_with("d-") || !dep[2..].bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(no(codes::BAD_REQUEST, 400, "not a deployment id"));
    }
    let owned = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_deployment(&conn, &dep).map_err(db_err)?.is_some_and(|d| d.app_id == app.app_id)
    };
    if !owned {
        return Err(no("not_found", 404, format!("no deployment {dep} of {}", app.app_id)));
    }
    let tail = params.get("tail").and_then(|t| t.parse::<usize>().ok()).unwrap_or(200).clamp(1, 5000);
    let text = read_log(&app.app_id, &dep);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(tail);
    let body = lines[start..].join("\n");
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "deployment": dep,
        "framed": deploy::framing::frame(&app.app_id, "logs", &body, LOG_CAP),
    }))
}

/// Lines that read like errors (a cheap first cut; stage 0's runtime error
/// pipe, S4, gives structured errors).
fn is_error_line(l: &str) -> bool {
    let x = l.to_ascii_lowercase();
    ["error", "exception", "traceback", "panic", "fatal", "unhandled", "uncaught", "segmentation fault", "killed"]
        .iter()
        .any(|k| x.contains(k))
}

/// GET `/cli/app/errors?app=[&since=<secs>]`: failed/rolled-back/stalled
/// deployments with their reason, and error-looking lines from recent
/// deployment logs, framed as untrusted (§5.9).
fn errors(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let app = app_for(who, params.get("app").map(String::as_str))?;
    let since = params.get("since").and_then(|s| s.parse::<i64>().ok()).unwrap_or(3600).max(1);
    let cutoff = deploy::now() - since;
    let deps = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_deployments(&conn, &app.app_id, 50).map_err(db_err)?
    };
    let recent: Vec<_> = deps.into_iter().filter(|d| d.started_at >= cutoff || d.finished_at.is_some_and(|f| f >= cutoff)).collect();
    let events: Vec<J> = recent
        .iter()
        .filter(|d| d.error.is_some())
        .map(|d| {
            json!({
                "deployment": d.id, "kind": d.kind, "result": d.result, "at": d.finished_at.unwrap_or(d.started_at),
                "error": d.error.as_deref().map(deploy::framing::strip_controls),
            })
        })
        .collect();
    let mut lines = String::new();
    for d in recent.iter().take(5) {
        for l in read_log(&app.app_id, &d.id).lines().filter(|l| is_error_line(l)) {
            lines.push_str(&format!("[{}] {l}\n", d.id));
        }
    }
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "sinceSecs": since,
        "events": events,
        "framed": deploy::framing::frame(&app.app_id, "errors", &lines, LOG_CAP),
    }))
}

/// POST `/cli/app/uninstall {app, purge?}`: stop, release the URL label,
/// drop the App and its service row. Seeds stay (the sweep removes them);
/// the deploy audit stays. `purge` also removes the App folder (its data):
/// owner only, passports refused.
fn uninstall(who: &Who, body: &[u8]) -> R<J> {
    need_manage(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let purge = b(&v, "purge");
    if purge && !matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. }) {
        return Err(no(
            codes::OWNER_ONLY,
            403,
            "--purge deletes the App's data: only the server's owner can, from Settings or a terminal outside K2",
        ));
    }
    super::supervisor::stop_app(&app, &actor(who))?;
    super::supervisor::forget(&app.app_id);
    if cfg!(target_os = "linux") && k2_root_proto::apps::AppId::parse(&app.app_id).is_some() && std::path::Path::new(k2_root_proto::engine::SOCKET).exists() {
        // Guarded leftovers (stopped units, the id slot on purge, the
        // gateway rule). Best effort: a process-only App has none.
        if let Err(e) = super::guarded::remove_app(&app.app_id, purge) {
            k2_core::log_debug!("[apps] uninstall {}: engine: {}", app.app_id, e.message);
        }
    }
    let svc = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::get_by_id(&conn, &app.service_id).map_err(db_err)?
    };
    let tunnel = svc.as_ref().is_some_and(|r| r.expose == k2_core::published_services::EXPOSE_TUNNEL);
    let label_is_ours = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let ours = !matches!(
            k2_core::db::schema::SubdomainWorkspace::self_unclaim(&conn, &app.app_id, &app.owner_workspace),
            Ok(k2_core::db::schema::SelfUnclaim::Foreign { .. })
        );
        store::delete_app(&conn, &app.app_id).map_err(db_err)?;
        let _ = k2_core::published_services::delete(&conn, &app.owner_workspace, &app.app_id);
        ours
    };
    if tunnel && label_is_ours {
        if let Ok(cfg) = k2_core::tunnel::config::load() {
            if !cfg.token.trim().is_empty() {
                let _ = k2_core::tunnel::subdomains::delete_subdomain(&cfg.token, &app.app_id);
            }
        }
        crate::session_events::emit_tunnel_subdomains_changed();
    }
    audit_human(&app.app_id, "uninstall", who, json!({ "purge": purge }));
    if purge {
        let _ = std::fs::remove_dir_all(deploy::app_dir(&app.app_id));
    } else {
        let _ = std::fs::remove_file(deploy::app_dir(&app.app_id).join("app.json"));
    }
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::PublishServicesChanged { project_id: app.owner_workspace.clone() });
    Ok(json!({ "ok": true, "app": app.app_id, "purged": purge }))
}
