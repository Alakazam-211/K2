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
use k2_core::deploy::{self, codes};
use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;
pub use crate::compute_routes::Who;

/// Largest request body.
pub const MAX_BODY: usize = 256 * 1024;

/// Every runtime route.
pub const ROUTES: &[&str] = &["/cli/app/build", "/cli/app/validate"];

/// POST-only rows (`require_post` → 405 on GET).
pub const POST_ROUTES: &[&str] = &["/cli/app/build", "/cli/app/validate"];

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
    pub path: String,
}

pub(crate) fn ws_by_id(id: &str) -> Option<Ws> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let p = k2_core::db::schema::Project::get(&conn, id).ok()?;
    Some(Ws { path: p.path })
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
    let _ = params;
    let r = match path {
        "/cli/app/build" => build(&who, body),
        "/cli/app/validate" => validate(&who, body),
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
