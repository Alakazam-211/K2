//! `/cli/app/*` runtime routes (`prd-k2-app-runtime-v1` §8.5, stage 1).
//!
//! Who may do what:
//! - Route floors (`route_policy.rs`) stop Connect logins below the role.
//! - Passports (agents) act only on their own workspace's Apps and
//!   folders. Changing anything (build, deploy, …) also needs the
//!   workspace switch "Allow this agent to manage Apps"
//!   (`agents_can_manage_skin`, §5.11.1): without it the change becomes a
//!   held `manage` request for the owner (S42, `approvals::
//!   hold_without_switch`); `apps_manage_off` is left only for changes
//!   outside that list. Deploys adding caps past the App's approved set,
//!   and first installs, are held for every caller (S20, `grants`). New
//!   egress hosts are held for every caller too (`egress_routes`, S15).
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
    "/cli/app/box/podman/uninstall",
    "/cli/app/box/prepare",
    "/cli/app/build",
    "/cli/app/config",
    "/cli/app/deploy",
    "/cli/app/deployment",
    "/cli/app/engine/install",
    "/cli/app/errors",
    "/cli/app/exception",
    "/cli/app/exec",
    "/cli/app/history",
    "/cli/app/inspect",
    "/cli/app/install",
    "/cli/app/link",
    "/cli/app/link/rotate",
    "/cli/app/list",
    "/cli/app/logs",
    "/cli/app/restart",
    "/cli/app/rollback",
    "/cli/app/shell",
    "/cli/app/start",
    "/cli/app/stop",
    "/cli/app/uninstall",
    "/cli/app/validate",
];

/// POST-only rows (`require_post` → 405 on GET).
pub const POST_ROUTES: &[&str] = &[
    "/cli/app/box/podman/uninstall",
    "/cli/app/box/prepare",
    "/cli/app/build",
    "/cli/app/config",
    "/cli/app/deploy",
    "/cli/app/engine/install",
    "/cli/app/exception",
    "/cli/app/exec",
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
        || super::egress_routes::ROUTES.contains(&p)
        || super::box_service_routes::ROUTES.contains(&p)
        || door_route(p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
        || super::egress_routes::POST_ROUTES.contains(&p)
        || super::box_service_routes::POST_ROUTES.contains(&p)
        || door_post_route(p)
}

/// The 0.46.1 door's routes (lane 2): audit, quotas, suspend/resume and
/// remote passes, each in its own module.
fn door_route(p: &str) -> bool {
    super::audit::ROUTES.contains(&p) || super::quotas::ROUTES.contains(&p) || super::suspend::ROUTES.contains(&p)
}

fn door_post_route(p: &str) -> bool {
    super::quotas::POST_ROUTES.contains(&p) || super::suspend::POST_ROUTES.contains(&p)
}

// ── refusals ─────────────────────────────────────────────────────────

#[derive(Debug)]
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
        410 => "410 Gone",
        413 => "413 Payload Too Large",
        422 => "422 Unprocessable Entity",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

/// The route answer: `200` with the JSON, `202` when the JSON says the
/// change is held for the owner (`status: held`, exit 2 in the CLI), or
/// the refusal.
pub(crate) fn respond(r: R<J>) -> CliResponse {
    match r {
        Ok(v) if v.get("status").and_then(J::as_str) == Some("held") => {
            CliResponse { status: "202 Accepted", content_type: "application/json", body: v.to_string() }
        }
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refusal_response(&e),
    }
}

pub(crate) fn refusal_response(r: &Refusal) -> CliResponse {
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

/// 404 for every runtime route while Apps (beta) are off on this server.
pub fn dark() -> CliResponse {
    refusal_response(&no(codes::APPS_OFF, 404, APPS_OFF_HINT))
}

/// Where the owner turns Apps (beta) back on.
pub const APPS_OFF_HINT: &str = "Apps (beta) are off on this server: the owner turns them on in Settings → Apps → \
     This server (or K2_APPS=1)";

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
    // S42: the replay of a held `manage` request the owner approved.
    if super::approvals::manage_approved_now() {
        return Ok(());
    }
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
             (Settings → Workspaces → <agent> → Agent, or k2 app access <workspace> --manage on)",
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
    // S42: an agent without "Allow this agent to manage Apps" gets a held
    // request for the owner instead of a refusal.
    if let Some(held) = super::approvals::hold_without_switch(path, &who, body) {
        return respond(held);
    }
    let r = match path {
        // Box prep (S10): the doctor for anyone with a credential, prepare
        // for the owner only.
        "/cli/app/box" => super::box_prep::box_route(&who),
        "/cli/app/box/prepare" => json_body(body).and_then(|v| super::box_prep::prepare_route(&who, &v)),
        // Lane P: the owner removes K2's Podman bundle.
        "/cli/app/box/podman/uninstall" => json_body(body).and_then(|v| super::podman_install::uninstall_route(&who, &v)),
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
        // The door (0.46.1 lane 2): audit, quotas, suspend, remote passes.
        p if super::audit::ROUTES.contains(&p) => super::audit::handle(&who, params),
        p if super::quotas::ROUTES.contains(&p) => super::quotas::handle(p, &who, body, params),
        p if super::suspend::ROUTES.contains(&p) => super::suspend::handle(p, &who, body, params),
        // Egress (S15) and held widenings.
        p if super::egress_routes::ROUTES.contains(&p) => super::egress_routes::handle(p, &who, body, params),
        // Box services (0.46.0): forwards into the wall, held for the owner.
        p if super::box_service_routes::ROUTES.contains(&p) => super::box_service_routes::handle(p, &who, body, params),
        // S28 and the process exception.
        "/cli/app/exec" => super::exec::exec_route(&who, body),
        "/cli/app/exception" => super::exception::route(&who, body),
        // A WebSocket: the TCP dispatcher serves it; anything else (the
        // cell socket) is told so.
        "/cli/app/shell" => return super::exec::shell_on_cell(),
        _ => Err(no("not_found", 404, "no such App route")),
    };
    respond(r)
}

/// POST `/cli/app/build {folder, types?}` → the Seed (S6). Deploys nothing.
/// A manifest `build` runs inside the wall where the box has one (S16,
/// guarded: install scripts off), in the reserved `build` slot.
fn build(who: &Who, body: &[u8]) -> R<J> {
    need_manage(who)?;
    let v = json_body(body)?;
    let folder = resolve_folder(who, s(&v, "folder").ok_or_else(|| no(codes::BAD_REQUEST, 400, "folder is required"))?)?;
    let (built, wall) = build_seed(who, &folder, &v, super::wall_build::BUILD_SLOT, false)?;
    Ok(json!({ "ok": true, "seed": built.seed, "reused": built.reused, "entries": built.entries, "build": wall }))
}

/// The Seed for a folder: built in the wall when the manifest declares a
/// build and the box has the wall (S16), else the folder as it is (stage
/// 1; `build.inWall: false` says why).
fn build_seed(who: &Who, folder: &Path, v: &J, slot: &str, scripts: bool) -> R<(deploy::build::Built, J)> {
    match super::wall_build::build_and_store(slot, folder, &signer(who), &validate_opts(v), scripts)? {
        Ok(done) => Ok(done),
        Err(why) => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            match deploy::build::build_folder(&conn, folder, &signer(who), &validate_opts(v)) {
                Ok(b) => {
                    let declared = !b.seed.manifest["build"].as_array().map(Vec::is_empty).unwrap_or(true);
                    let wall = if declared {
                        json!({ "inWall": false, "why": why, "note": "the manifest's build did not run here: the Seed is the folder as it was built outside the wall" })
                    } else {
                        json!({ "inWall": false, "why": why })
                    };
                    Ok((b, wall))
                }
                Err(f) => {
                    let mut r = no(f.code, 422, f.message);
                    r.extra = f.report.map(|rep| json!({ "report": rep }));
                    Err(r)
                }
            }
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
    // 0.46.3: a new App starts out guarded unless the owner opted this box
    // into the unwalled process engine (then stage 1's default stays).
    let isolation = default_isolation();
    if isolation == "container" && k2_root_proto::apps::AppId::parse(&app_id).is_none() {
        return Err(no(
            codes::APP_ID_INVALID,
            400,
            format!("App id '{app_id}' doesn't fit the guarded engine (at most 32 characters, no '--'): pick a shorter id"),
        ));
    }
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
                isolation,
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
        // A first install held for the owner (S20): the App exists, its
        // deploy waits (exit 2 in the CLI).
        if d.get("status").and_then(J::as_str) == Some("held") {
            out["status"] = json!("held");
            out["request"] = d["request"].clone();
            out["hint"] = d["hint"].clone();
        }
        out["deploy"] = d;
    }
    Ok(out)
}

/// A new App's isolation: `process` only where the owner turned the
/// unwalled process engine on; `container` (the guarded engine) otherwise.
pub(crate) fn default_isolation() -> &'static str {
    if deploy::process_engine_enabled() {
        "process"
    } else {
        "container"
    }
}

/// The Seed a deploy names: a stored one, or a folder built now (in the
/// App's own wall when its manifest declares a build, S16; install
/// scripts only on the open path).
fn seed_for_deploy(who: &Who, app: &AppRow, v: &J) -> R<String> {
    if let Some(f) = s(v, "folder") {
        let folder = resolve_folder(who, f)?;
        let scripts = app.path == "open";
        let slot = if k2_root_proto::apps::AppId::parse(&app.app_id).is_some() { app.app_id.as_str() } else { super::wall_build::BUILD_SLOT };
        return build_seed(who, &folder, v, slot, scripts).map(|(b, _)| b.seed.hash);
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
    if app.is_process_exception() {
        return Err(no(
            codes::ENGINE_UNAVAILABLE,
            409,
            format!("{} runs as the process exception; turn that off first (k2 app exception {} off), then pick an engine", app.app_id, app.app_id),
        ));
    }
    let iso = match e {
        "guarded" | "container" => "container",
        "process" => "process",
        other => return Err(no(codes::BAD_REQUEST, 400, format!("engine is guarded or process, not '{other}'"))),
    };
    if iso == app.isolation {
        return Ok(app);
    }
    if iso == "process" && !deploy::process_engine_enabled() {
        return Err(no(codes::ENGINE_UNAVAILABLE, 409, format!("{}: {}", app.app_id, deploy::PROCESS_ENGINE_OFF_HINT)));
    }
    // S13: lowering the wall (guarded → process) is on the always-per-event
    // list (§5.6): it goes through `k2 app config --isolation process`,
    // held for the owner's confirm, never through a deploy flag.
    let state = config_state(&app)?;
    if k2_core::app_grace::config::classify(&k2_core::app_grace::config::Op::Isolation(iso.into()), &state)
        != k2_core::app_grace::config::Class::Narrow
    {
        return Err(no(
            codes::WIDENING_HELD,
            409,
            format!(
                "moving {} from {} to {iso} lowers its wall: ask for it with k2 app config {} --isolation {iso} \
                 (held for the owner's approval)",
                app.app_id, app.isolation, app.app_id
            ),
        ));
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
    super::suspend::refuse_if_suspended(&app.app_id)?;
    let app = apply_engine(who, app, v)?;
    let seed = seed_for_deploy(who, &app, v)?;
    // S20: caps past the App's approved set (or a first install) are held
    // for the owner, whoever deploys; the live Seed keeps serving.
    if let Some(held) = hold_new_caps(who, &app, &seed, v)? {
        return Ok(held);
    }
    // Box services the Seed asks for are filed before the start renders
    // its forwards, so approving one later opens it without a restart.
    let box_services = super::box_service_routes::request_manifest(who, &app, &seed, None);
    let dep = super::supervisor::begin(&app, &seed, store::KIND_DEPLOY, &actor(who), s(v, "reason"))?;
    // S15: hosts the Seed asks for that aren't allowed yet become a held
    // request; the deploy goes on, and the App can't reach them until the
    // owner approves.
    let egress = super::egress_routes::request_manifest_hosts(who, &app.app_id, &seed, &dep);
    let mut out = deployment_answer(&dep, wait_secs(v))?;
    if let Some(e) = egress {
        out["egress"] = e;
    }
    if let Some(b) = box_services {
        out["boxServices"] = b;
    }
    Ok(out)
}

/// S20: hold a deploy whose Seed asks for caps past the App's approved set
/// (`deploy`), or an App's first install (`install`), for every caller
/// (§5.6 always per event, AR14). A first install that asks for nothing,
/// or fits a set the owner approved before for the same Seed id, records
/// its set and goes on.
fn hold_new_caps(who: &Who, app: &AppRow, seed: &str, v: &J) -> R<Option<J>> {
    use super::grants::{self, DeployCheck};
    let check = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let c = grants::check_deploy(&conn, app, seed).map_err(db_err)?;
        if let DeployCheck::FirstFits(set) = &c {
            grants::record(&conn, &app.app_id, set).map_err(db_err)?;
        }
        c
    };
    let DeployCheck::Held { kind, add, set } = check else { return Ok(None) };
    let first = kind == k2_core::app_grace::requests::KIND_INSTALL;
    let summary = if first {
        format!("first install of {}: approve caps {}", app.app_id, add.join(", "))
    } else {
        format!("deploy {} with new caps {}", app.app_id, add.join(", "))
    };
    let r = super::approvals::file(&super::approvals::Filing {
        who,
        kind,
        app: Some(app),
        label: &app.app_id,
        workspace: &app.owner_workspace,
        body: json!({ "app": app.app_id, "seed": seed, "reason": s(v, "reason") }),
        diff: grants::diff_json(kind, &add, &set),
        summary,
        reasons: vec![if first {
            "an App's first install sets the caps it may use: the owner approves them".to_string()
        } else {
            "new caps beyond the App's approved set wait for the owner (every caller)".to_string()
        }],
        reason: s(v, "reason"),
    })?;
    let mut out = super::approvals::held_answer(&r);
    out["seed"] = json!(seed);
    out["note"] = json!(if app.live_seed.is_some() { "the live Seed keeps serving until the owner approves" } else { "nothing runs until the owner approves" });
    Ok(Some(out))
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
    super::suspend::refuse_if_suspended(&app.app_id)?;
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
    super::suspend::refuse_if_suspended(&app.app_id)?;
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

// ── S13: App config (§4.4) — narrowing applies now, widening is held ─
//
// One call, one classifier: every field is classified on its own; what
// the caller may make at once applies now and the rest is one held
// request for the owner. Two families have their own truth and rules:
// - egress (`egressAdd`, `egressRm`, `egressMode`) is the guarded App's
//   enforced host list (S15, `super::egress_routes`): a new host or broad
//   mode is always its own held `egress` request (every caller, the owner
//   token included); removing a host or going back to allowlist applies
//   at once;
// - disk ceilings (`io`, runtime lane D): tightening applies at once; a
//   raise is the owner's (an agent's raise is held).
// The process exception (path open + isolation process) has its own
// owner-only switch (`k2 app exception`); config never enters or leaves it.

use k2_core::app_grace::config::{self as appcfg, Class, Op};

/// The App's config as one state (columns + `config_json`).
pub(crate) fn config_state(app: &AppRow) -> R<appcfg::State> {
    let config = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        appcfg::load(&conn, &app.app_id).map_err(db_err)?
    };
    Ok(appcfg::State {
        memory_limit_bytes: app.memory_limit_bytes,
        isolation: app.isolation.clone(),
        path: app.path.clone(),
        config,
        io_max_json: app.io_max_json.clone(),
    })
}

fn str_list(v: &J, k: &str) -> Vec<String> {
    match v.get(k) {
        Some(J::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        Some(J::String(x)) => x.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect(),
        _ => vec![],
    }
}

fn opt_size(v: &J, k: &str) -> R<Option<Option<i64>>> {
    let Some(raw) = v.get(k) else { return Ok(None) };
    let raw = match raw {
        J::Null => "none".to_string(),
        J::String(x) => x.clone(),
        other => other.to_string(),
    };
    Ok(Some(deploy::parse_memory(&raw).map_err(|e| no(codes::BAD_REQUEST, 400, format!("{k}: {e}")))?))
}

fn opt_u32(v: &J, k: &str) -> R<Option<Option<u32>>> {
    match v.get(k) {
        None => Ok(None),
        Some(J::Null) => Ok(Some(None)),
        Some(J::String(x)) if x == "none" => Ok(Some(None)),
        Some(x) => {
            let n = x.as_u64().or_else(|| x.as_str().and_then(|t| t.parse().ok())).filter(|n| *n > 0 && *n <= u32::MAX as u64);
            n.map(|n| Some(Some(n as u32))).ok_or_else(|| no(codes::BAD_REQUEST, 400, format!("{k}: a positive number or none")))
        }
    }
}

fn opt_bool(v: &J, k: &str) -> R<Option<bool>> {
    match v.get(k) {
        None => Ok(None),
        Some(J::Bool(b)) => Ok(Some(*b)),
        Some(J::String(x)) if x == "on" || x == "true" => Ok(Some(true)),
        Some(J::String(x)) if x == "off" || x == "false" => Ok(Some(false)),
        Some(_) => Err(no(codes::BAD_REQUEST, 400, format!("{k}: on or off"))),
    }
}

fn one_of(v: &J, k: &str, allowed: &[&str]) -> R<Option<String>> {
    match s(v, k) {
        None => Ok(None),
        Some(x) if allowed.contains(&x) => Ok(Some(x.to_string())),
        Some(x) => Err(no(codes::BAD_REQUEST, 400, format!("{k} is {}, not '{x}'", allowed.join(" | ")))),
    }
}

/// The ops one config body asks for (§4.4 names, camelCase). Egress and
/// disk ceilings are read separately ([`egress_asks`], [`io_after`]).
pub(crate) fn config_ops(v: &J) -> R<Vec<Op>> {
    let mut ops = Vec::new();
    if let Some(m) = opt_size(v, "memory")? {
        ops.push(Op::Memory(m));
    }
    if let Some(n) = opt_u32(v, "cpuWeight")? {
        ops.push(Op::CpuWeight(n));
    }
    if let Some(n) = opt_u32(v, "tasks")? {
        ops.push(Op::Tasks(n));
    }
    if let Some(d) = opt_size(v, "disk")? {
        ops.push(Op::Disk(d));
    }
    for p in str_list(v, "portAdd") {
        ops.push(Op::PortAdd(appcfg::parse_port(&p).map_err(|e| no(codes::BAD_REQUEST, 400, e))?));
    }
    for p in str_list(v, "portRm") {
        ops.push(Op::PortRm(appcfg::parse_port(&p).map_err(|e| no(codes::BAD_REQUEST, 400, e))?));
    }
    if v.get("recycleEvery").is_some() {
        let secs = match s(v, "recycleEvery") {
            None | Some("none") => None,
            Some(x) => Some(parse_secs(x).ok_or_else(|| no(codes::BAD_REQUEST, 400, "recycleEvery: 24h, 90m or none"))?),
        };
        ops.push(Op::RecycleEvery(secs));
    }
    if v.get("recycleAt").is_some() {
        let pct = match s(v, "recycleAt") {
            None | Some("none") => None,
            Some(x) => Some(
                x.trim_end_matches('%').parse::<u8>().ok().filter(|p| (10..=99).contains(p)).ok_or_else(|| no(codes::BAD_REQUEST, 400, "recycleAt: 50%–99% or none"))?,
            ),
        };
        ops.push(Op::RecycleAt(pct));
    }
    if let Some(i) = one_of(v, "isolation", &["container", "vm", "process", "none"])? {
        if i == "vm" {
            return Err(no(codes::ENGINE_UNAVAILABLE, 409, "the VM switch isn't offered yet (stage V); container is the strongest wall today"));
        }
        if i == "none" && cfg!(target_os = "linux") {
            return Err(no(codes::BAD_REQUEST, 400, "isolation none is for dev machines only"));
        }
        ops.push(Op::Isolation(i));
    }
    if let Some(p) = one_of(v, "path", &["guarded", "open"])? {
        ops.push(Op::Path(p));
    }
    if let Some(r) = one_of(v, "risk", &["normal", "high"])? {
        ops.push(Op::Risk(r));
    }
    if let Some(b) = opt_bool(v, "dbPrivate")? {
        ops.push(Op::DbPrivate(b));
    }
    if let Some(b) = opt_bool(v, "dbRaw")? {
        ops.push(Op::DbRaw(b));
    }
    Ok(ops)
}

/// The egress part of a config body, checked before anything changes.
#[derive(Default)]
pub(crate) struct EgressAsks {
    add: Vec<String>,
    rm: Vec<String>,
    mode: Option<String>,
}

impl EgressAsks {
    fn is_empty(&self) -> bool {
        self.add.is_empty() && self.rm.is_empty() && self.mode.is_none()
    }
}

fn egress_asks(v: &J) -> R<EgressAsks> {
    let a = EgressAsks { add: str_list(v, "egressAdd"), rm: str_list(v, "egressRm"), mode: one_of(v, "egressMode", &["allowlist", "broad"])? };
    if !a.add.is_empty() {
        super::egress_routes::check_specs(&a.add)?;
    }
    for h in &a.rm {
        k2_core::deploy::egress::parse_spec(h).map_err(|e| no(super::egress_routes::codes::EGRESS_HOST_INVALID, 400, e))?;
    }
    Ok(a)
}

/// The App's disk ceilings after `v`'s `io` (an object with `rbps`,
/// `wbps`, `riops`, `wiops`: a rate like `40M`, a number, or `none`; or
/// the string `default` / `none` for all of them). `None` = no `io` asked.
pub(crate) fn io_after(app: &AppRow, v: &J) -> R<Option<k2_root_proto::apps::IoMax>> {
    use k2_root_proto::apps::{parse_rate, IoMax};
    let Some(io) = v.get("io") else { return Ok(None) };
    let cur = IoMax::from_column(app.io_max_json.as_deref());
    let out = match io {
        J::String(x) if x == "default" => IoMax::k2_default(),
        J::String(x) if x == "none" => IoMax::unlimited(),
        J::Object(m) => {
            let mut n = cur;
            for (k, val) in m {
                let txt = match val {
                    J::String(t) => t.clone(),
                    J::Number(x) => x.to_string(),
                    J::Null => "none".into(),
                    _ => return Err(no(codes::BAD_REQUEST, 400, format!("io.{k}: a rate (40M), a number or none"))),
                };
                let iops = matches!(k.as_str(), "riops" | "wiops");
                let r = parse_rate(&txt, iops).map_err(|e| no(codes::BAD_REQUEST, 400, format!("io.{k}: {e}")))?;
                match k.as_str() {
                    "rbps" => n.rbps = r,
                    "wbps" => n.wbps = r,
                    "riops" => n.riops = r,
                    "wiops" => n.wiops = r,
                    other => return Err(no(codes::BAD_REQUEST, 400, format!("io.{other}: the knobs are rbps, wbps, riops, wiops"))),
                }
            }
            n
        }
        _ => return Err(no(codes::BAD_REQUEST, 400, "io is default, none, or {rbps, wbps, riops, wiops}")),
    };
    out.check().map_err(|e| no(codes::BAD_REQUEST, 400, e))?;
    Ok(Some(out))
}

fn parse_secs(x: &str) -> Option<i64> {
    let x = x.trim();
    let (n, mul) = match x.chars().last()? {
        'h' => (&x[..x.len() - 1], 3600),
        'm' => (&x[..x.len() - 1], 60),
        's' => (&x[..x.len() - 1], 1),
        'd' => (&x[..x.len() - 1], 86400),
        _ => (x, 1),
    };
    n.parse::<i64>().ok().filter(|v| *v > 0).map(|v| v * mul)
}

/// The process exception (path `open` + isolation `process`) is its own
/// owner-only switch with its own setup (devices, the `k2appx` unit):
/// config never enters it or leaves it, now or on approve.
fn exception_guard(app: &AppRow, after: &appcfg::State) -> R<()> {
    let enters = after.path == "open" && after.isolation == "process";
    if app.is_process_exception() && !enters {
        return Err(no(
            codes::ENGINE_UNAVAILABLE,
            409,
            format!("{} runs as the process exception; turn that off first (k2 app exception {} off)", app.app_id, app.app_id),
        ));
    }
    if !app.is_process_exception() && enters {
        return Err(no(
            codes::ENGINE_UNAVAILABLE,
            409,
            format!(
                "path open with isolation process is the process exception: the owner turns it on in person with \
                 k2 app exception {} on (it is never a config change)",
                app.app_id
            ),
        ));
    }
    // 0.46.3: moving an App onto the unwalled process engine while the
    // owner has it off would only file a request no deploy can honour.
    let unwalled_after = after.path != "open" && after.isolation == "process";
    if unwalled_after && !super::supervisor::runs_unwalled(app) && !deploy::process_engine_enabled() {
        return Err(no(codes::ENGINE_UNAVAILABLE, 409, format!("{}: {}", app.app_id, deploy::PROCESS_ENGINE_OFF_HINT)));
    }
    Ok(())
}

/// Apply ops to an App now (callers decided they may). Audited, mirrored.
pub(crate) fn apply_config_ops(app_id: &str, ops: &[Op], by: &str) -> R<J> {
    let app = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_app(&conn, app_id).map_err(db_err)?
    }
    .ok_or_else(|| no(codes::APP_NOT_FOUND, 404, format!("no App '{app_id}'")))?;
    let mut state = config_state(&app)?;
    let lines: Vec<String> = ops.iter().map(|op| appcfg::describe(op, &state)).collect();
    for op in ops {
        appcfg::apply(op, &mut state);
    }
    // Re-checked here too: a held request approved later meets today's App.
    exception_guard(&app, &state)?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        appcfg::store(&conn, &app.app_id, &state, deploy::now()).map_err(db_err)?;
        if let Err(e) = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, &app.app_id, "config", by, None, &json!({ "changes": lines, "ops": ops })) {
            k2_core::log_debug!("[apps] audit {} config: {e}", app.app_id);
        }
    }
    mirror(&app.app_id);
    Ok(json!({ "applied": lines, "config": state }))
}

/// May this caller make a change of this class at once?
fn may_apply(who: &Who, class: Class, open_on: bool) -> bool {
    match class {
        Class::Narrow | Class::Budget => true,
        Class::PerEvent => false,
        Class::Open => matches!(who, Who::Owner | Who::Shell { .. } | Who::Login { role: Role::Owner, .. }) || (agent_ws(who).is_some() && open_on),
        Class::Owner => matches!(who, Who::Owner | Who::Shell { .. } | Who::Login { role: Role::Owner, .. }),
    }
}

fn hold_reason(class: Class) -> &'static str {
    match class {
        Class::Open => "an open-path feature: the owner, or an agent whose workspace has \"…and publish open Apps\"",
        Class::Owner => "a widening only the owner makes",
        Class::PerEvent => "on the always-per-event list: a fresh owner confirm, even from the owner token",
        _ => "",
    }
}

/// POST `/cli/app/config {app, memory?, cpuWeight?, tasks?, disk?,
/// egressAdd?, egressRm?, egressMode?, portAdd?, portRm?, recycleEvery?,
/// recycleAt?, isolation?, path?, risk?, dbPrivate?, dbRaw?, io?}` (S13,
/// §4.4, §6.2). Every value is checked before anything changes; then each
/// change is classified on its own: what the caller may make at once
/// applies now; the rest becomes one held request for the owner (egress
/// widenings are their own `egress` request).
fn config(who: &Who, body: &[u8]) -> R<J> {
    need_manage(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let mut ops = config_ops(&v)?;
    let egress = egress_asks(&v)?;
    let io = io_after(&app, &v)?;
    if ops.is_empty() && egress.is_empty() && io.is_none() {
        return Err(no(
            codes::BAD_REQUEST,
            400,
            "nothing to change (memory, cpuWeight, tasks, disk, egressAdd/Rm, egressMode, portAdd/Rm, recycleEvery/At, isolation, path, \
             risk, dbPrivate, dbRaw, io)",
        ));
    }
    let state = config_state(&app)?;
    // Disk ceilings: tightening narrows; a raise is the owner's.
    let mut io_raise = false;
    if let Some(new) = io {
        use k2_root_proto::apps::IoMax;
        let cur = IoMax::from_column(app.io_max_json.as_deref());
        let col = if new == IoMax::k2_default() { None } else { Some(new.to_column()) };
        if col != app.io_max_json {
            io_raise = !cur.loosened_by(&new).is_empty();
            ops.push(Op::Io(col));
        }
    }
    // The whole call, as if every change applied: never into or out of
    // the process exception.
    let mut after = state.clone();
    for op in &ops {
        appcfg::apply(op, &mut after);
    }
    exception_guard(&app, &after)?;
    if egress.mode.as_deref() == Some("broad") && after.path != "open" {
        return Err(no(
            super::egress_routes::codes::OPEN_PATH_ONLY,
            409,
            format!("broad egress is the open path's; App {} is guarded. Ask for the hosts it needs (egressAdd)", app.app_id),
        ));
    }
    let open_on = agent_ws(who).is_some_and(k2_core::workspace::settings::agents_can_publish_open_apps);
    let mut now_ops: Vec<Op> = Vec::new();
    let mut held: Vec<(Op, Class)> = Vec::new();
    for op in ops {
        let class = match op {
            Op::Io(_) if io_raise => Class::Owner,
            Op::Io(_) => Class::Narrow,
            _ => appcfg::classify(&op, &state),
        };
        if may_apply(who, class, open_on) {
            now_ops.push(op);
        } else {
            held.push((op, class));
        }
    }
    let io_changed = now_ops.iter().any(|o| matches!(o, Op::Io(_)));
    let applied = if now_ops.is_empty() { None } else { Some(apply_config_ops(&app.app_id, &now_ops, &actor(who))?) };
    let mut applied_lines: Vec<J> = applied.as_ref().and_then(|a| a["applied"].as_array().cloned()).unwrap_or_default();
    let after_app = app_for(who, Some(&app.app_id))?;
    // Egress: narrowing at once, widening held (its own request).
    let mut egress_out: Vec<J> = Vec::new();
    let mut egress_held = None;
    for h in &egress.rm {
        let r = super::egress_routes::remove_host(who, &after_app, h)?;
        if r["removed"] == json!(true) {
            applied_lines.push(json!(format!("stop outbound calls to {}", r["host"].as_str().unwrap_or(h))));
        }
        egress_out.push(r);
    }
    if let Some(m) = egress.mode.as_deref() {
        let r = super::egress_routes::change_mode(who, &after_app, m, s(&v, "reason"))?;
        if r["status"] == json!("held") {
            egress_held = Some(r.clone());
        } else {
            applied_lines.push(json!(format!("egress mode → {m}")));
        }
        egress_out.push(r);
    }
    if !egress.add.is_empty() {
        let r = super::egress_routes::ask_hosts(who, &after_app, &egress.add, s(&v, "reason"), None)?;
        if r["status"] == json!("held") {
            egress_held = Some(r.clone());
        }
        egress_out.push(r);
    }
    let mut note = String::from(
        "Stored now. Egress is enforced for guarded Apps (their own proxy). The unwalled process engine (owner opt-in, dev \
         machines) doesn't enforce memory limits; the walled engines do.",
    );
    if io_changed {
        note.push_str(" Walled engines apply disk ceilings at the App's next deploy or restart (k2 app restart).");
    }
    let mut out = json!({
        "ok": true,
        "app": after_app,
        "applied": applied_lines,
        "io": k2_root_proto::apps::IoMax::from_column(after_app.io_max_json.as_deref()),
        "note": note,
    });
    if !egress_out.is_empty() {
        out["egress"] = json!(egress_out);
    }
    if !held.is_empty() {
        let t = super::grace::target(&Who::Owner, Some(&app.service_id))?;
        let diff: Vec<String> = held.iter().map(|(op, _)| appcfg::describe(op, &state)).collect();
        let mut reasons: Vec<String> = held.iter().map(|(op, c)| format!("{}: {}", appcfg::describe(op, &state), hold_reason(*c))).collect();
        reasons.dedup();
        let ops: Vec<&Op> = held.iter().map(|(op, _)| op).collect();
        let h = super::grace::hold(
            who,
            k2_core::app_grace::requests::KIND_CONFIG,
            Some(&t),
            &app.app_id,
            &app.owner_workspace,
            json!({ "app": app.app_id, "ops": ops }),
            json!(diff),
            reasons,
        )?;
        out["status"] = json!("held");
        out["held"] = h;
    }
    if let Some(e) = egress_held {
        out["status"] = json!("held");
        out["egressHeld"] = e;
    }
    Ok(out)
}

// ── S8: list, inspect, history, logs, errors, uninstall ──────────────

/// Largest framed log or error text returned (the tail is kept).
const LOG_CAP: usize = 256 * 1024;

/// The App's wall label (§4.5, S13): computed by `open::label`, never set.
pub(crate) fn label(app: &AppRow) -> String {
    super::open::label_of(app).text
}

fn url_for(app: &AppRow) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = k2_core::published_services::get_by_id(&conn, &app.service_id).ok().flatten()?;
    let svc = crate::publish_runtime::to_json(&row);
    svc.url
}

fn app_summary(app: &AppRow) -> J {
    let lab = super::open::label_of(app);
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
        "path": app.path,
        "source": app.source,
        "folder": app.folder_path,
        "label": lab.text,
        "labelReason": lab.reason,
        "labelBack": lab.back,
        "io": k2_root_proto::apps::IoMax::from_column(app.io_max_json.as_deref()),
        "devices": app.devices(),
        "exception": app.exception_json.as_deref().and_then(|e| serde_json::from_str::<J>(e).ok()),
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
    let box_services = super::box_service_routes::summary_json(&app);
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
        "boxServices": box_services,
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
    // A walled instance's own output is its capped log file, read through
    // the engine (protocol 3, S17; the journal for a deployment an older
    // engine started); the file here holds K2's lines about it.
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
            "--purge deletes the App's data: only the server's owner can (k2 app uninstall <app> --purge from a terminal outside K2)",
        ));
    }
    super::supervisor::stop_app(&app, &actor(who))?;
    super::supervisor::forget(&app.app_id);
    super::egress_proxy::stop(&app.app_id);
    if cfg!(target_os = "linux") && k2_root_proto::apps::AppId::parse(&app.app_id).is_some() && std::path::Path::new(k2_root_proto::engine::SOCKET).exists() {
        // Guarded leftovers (stopped units, the id slot on purge, the
        // gateway rule). Best effort: a process-only App has none.
        if let Err(e) = super::guarded::remove_app(&app.app_id, purge) {
            k2_core::log_debug!("[apps] uninstall {}: engine: {}", app.app_id, e.message);
        }
        // The process exception's own state belongs to k2appx: the root
        // helper deletes it.
        if purge && app.is_process_exception() {
            if let Err(e) = super::process_exception::purge(&app.app_id) {
                k2_core::log_debug!("[apps] uninstall {}: process state: {}", app.app_id, e.message);
            }
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
