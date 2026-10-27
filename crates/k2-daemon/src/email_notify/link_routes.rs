//! The App's own email door (prd-email-notifications-core-v1 §3.5, E24):
//!
//! ```text
//! POST /cli/skin/email/send     {from?, fromName?, to, cc?, bcc?, subject, text?, html?, replyTo?, headers?, tags?, idempotencyKey?, sendAt?}
//!                                                     → 202 {id, status, recipients}
//! GET  /cli/skin/email/status   ?id=                  → {id, status, createdAt, updatedAt, recipients, error?}
//! GET  /cli/skin/email/log      ?limit=&before=&status=&tag=
//! ```
//!
//! - **This App's own pass, on its own link** (`door: link`), holding
//!   `email:send` (in no App core: a deploy adding it is held for the
//!   owner once). A guest's session pass, a pass of another App, the owner
//!   token or a passport never sends: `app_pass_required`.
//! - Outside the wall: `POST /v1/apps/<id>/email/send`, `GET …/status`,
//!   `GET …/log` with an App key holding scope `email`
//!   ([`handle_key`]). The App always comes from the credential, never the
//!   body.

use std::collections::HashMap;

use k2_core::skin::SkinPass;
use serde_json::Value as J;

use super::{fail, respond, Fail, R};
use crate::cli_response::CliResponse;

pub const SEND: &str = "/cli/skin/email/send";
pub const STATUS: &str = "/cli/skin/email/status";
pub const LOG: &str = "/cli/skin/email/log";

/// The routes a pass calls (the dispatcher resolves the pass).
pub const PASS_ROUTES: &[&str] = &[SEND, STATUS, LOG];
pub const POST_ROUTES: &[&str] = &[SEND];

/// No credential at all on a pass route.
pub fn pass_required() -> CliResponse {
    super::fail_response(&fail(401, "pass_required", "send this App's pass (K2_PASS_FILE) as Authorization: Bearer k2skn_…"))
}

/// A credential that isn't a pass (owner token, API key, passport).
pub fn not_a_pass() -> CliResponse {
    super::fail_response(&app_pass_required())
}

fn app_pass_required() -> Fail {
    fail(403, "app_pass_required", "only this App's own pass, on its own link, sends email (a guest's pass never does)")
}

/// The App a link pass sends for: this link's own App pass, holding
/// `email:send`. `link_app` is the link's App (the dispatcher read it on
/// the request's task).
fn app_on_link(pass: &SkinPass, link_app: Option<&str>) -> R<String> {
    let Some(app) = link_app else { return Err(app_pass_required()) };
    if pass.session || pass.principal_id.is_some() || pass.app != app {
        return Err(app_pass_required());
    }
    if !pass.has_cap(k2_core::skin::CAP_EMAIL_SEND) {
        return Err(fail(403, "cap_not_granted", "this App's pass doesn't carry email:send (add it to the manifest; the owner approves it once)"));
    }
    Ok(app.to_string())
}

fn json_body(body: &[u8]) -> R<J> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Err(fail(400, "usage", "send a JSON body (to, subject, text or html)"));
    }
    match serde_json::from_slice::<J>(body) {
        Ok(v @ J::Object(_)) => Ok(v),
        _ => Err(fail(400, "usage", "the body is a JSON object")),
    }
}

fn log_params(params: &HashMap<String, String>) -> (Option<usize>, Option<i64>, Option<&str>, Option<&str>) {
    (
        params.get("limit").and_then(|l| l.parse().ok()),
        params.get("before").and_then(|l| l.parse().ok()),
        params.get("status").map(String::as_str).filter(|s| !s.is_empty()),
        params.get("tag").map(String::as_str).filter(|s| !s.is_empty()),
    )
}

/// The pass routes (the dispatcher resolved `pass`; `link_app` is the
/// request's link App, if it came in on one).
pub fn handle_pass(path: &str, pass: &SkinPass, link_app: Option<&str>, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    let r = (|| -> R<J> {
        let app = app_on_link(pass, link_app)?;
        match (path, is_post) {
            (SEND, true) => {
                let v = json_body(body)?;
                let caller = super::send::Caller::App { app: app.clone(), actor: format!("app:{app}") };
                super::send::send(&caller, &v, super::now())
            }
            (STATUS, false) => {
                let id = params.get("id").map(|s| s.trim()).filter(|s| !s.is_empty()).ok_or_else(|| fail(400, "usage", "send ?id= (from email.send)"))?;
                super::send::status(&app, id)
            }
            (LOG, false) => {
                let (limit, before, status, tag) = log_params(params);
                super::send::log(&app, limit, before, status, tag)
            }
            _ => Err(fail(405, "method_not_allowed", "wrong method for this route")),
        }
    })();
    respond(r)
}

/// The caller a key's send is: the App, or (a key the App's agent minted
/// itself, review M2) that agent, under its own email sending setting.
fn key_caller(app: &str, actor: &str, minted_by_agent: Option<&str>) -> R<super::send::Caller> {
    let Some(ws) = minted_by_agent else {
        return Ok(super::send::Caller::App { app: app.to_string(), actor: actor.to_string() });
    };
    let path = crate::workspace_msg::resolve_workspace(ws).ok_or_else(|| fail(403, "gated", "the agent that minted this key is gone"))?;
    Ok(super::send::Caller::Agent { app: app.to_string(), actor: actor.to_string(), project_id: ws.to_string(), project_path: path })
}

/// `/v1/apps/<id>/email/*` with an App key of scope `email` (the key
/// route checked the key, its App and the scope; `actor` names the key;
/// `minted_by_agent` is the key's stamp).
pub fn handle_key(app: &str, actor: &str, minted_by_agent: Option<&str>, verb: &str, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    let r = (|| -> R<J> {
        match (verb, is_post) {
            ("email/send", true) => {
                let v = json_body(body)?;
                let caller = key_caller(app, actor, minted_by_agent)?;
                super::send::send(&caller, &v, super::now())
            }
            ("email/status", false) => {
                let id = params.get("id").map(|s| s.trim()).filter(|s| !s.is_empty()).ok_or_else(|| fail(400, "usage", "send ?id= (from email/send)"))?;
                super::send::status(app, id)
            }
            ("email/log", false) => {
                let (limit, before, status, tag) = log_params(params);
                super::send::log(app, limit, before, status, tag)
            }
            _ => Err(fail(404, "not_found", "not found")),
        }
    })();
    respond(r)
}
