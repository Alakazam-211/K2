//! Garden and widget edits through the daemon, and the proposal queue
//! (agents run Gardens end to end, 0.46.0, migration 0181).
//!
//! `zen_routes::handle` has already applied Zen's rules (this computer's
//! owner token, or a LOCAL agent / K2 shell passport; a Connect login, an
//! app pass or a sandboxed agent gets 403 `zen_local_only`), refused a GET
//! on a POST row, and refused a put that names a locked Garden (423).
//!
//! | route                         | person | agent with the switch | other agent |
//! |-------------------------------|--------|-----------------------|-------------|
//! | POST garden/put, widget/put   | writes | writes (audited)      | proposes    |
//! | GET  proposals, proposals/show| yes    | yes                   | yes         |
//! | POST proposals/approve        | yes    | yes (audited)         | 403         |
//! | POST proposals/reject         | yes    | yes (audited)         | own only (withdraw) |
//!
//! "The switch" is "Manage Gardens and data sources" (`gardens_manage`,
//! [`crate::gardens_manage`]). A write is checked before it lands: the
//! Garden must exist, be unlocked and check clean; a widget must be a user
//! folder (never K2's built-in, frozen `k2:<name>`), one code file. A
//! proposal is checked the same way when it is made, and again when it is
//! approved, which also needs the file unchanged since (409
//! `proposal_stale`). Nothing here writes zen.toml, themes, sync state,
//! `.defaults/` or a template.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::zen::agent_edit::{self, NewProposal, Proposal, ProposalKind};
use k2_core::zen::{ZenError, ZenFiles};

use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

pub const POST_ROUTES: &[&str] =
    &["/cli/zen/garden/put", "/cli/zen/proposals/approve", "/cli/zen/proposals/reject", "/cli/zen/widget/put"];

pub const GET_ROUTES: &[&str] = &["/cli/zen/proposals", "/cli/zen/proposals/show"];

pub fn is_route(p: &str) -> bool {
    POST_ROUTES.contains(&p) || GET_ROUTES.contains(&p)
}

fn resp(status: &'static str, body: J) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: body.to_string() }
}

fn refuse(status: &'static str, code: &str, message: impl Into<String>) -> CliResponse {
    resp(status, json!({ "ok": false, "error": code, "message": message.into() }))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn files() -> ZenFiles {
    ZenFiles::local()
}

/// Who is asking, as far as these routes care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// This computer's owner token (the person).
    Person,
    /// A local passport whose workspace has "Manage Gardens and data
    /// sources".
    Manager,
    /// Any other local passport.
    Agent,
}

fn role(owner_token: bool, caller: &Caller) -> Role {
    if owner_token {
        Role::Person
    } else if crate::gardens_manage::caller_may(caller) {
        Role::Manager
    } else {
        Role::Agent
    }
}

fn project_of(caller: &Caller) -> Option<&str> {
    match caller {
        Caller::Agent(a) => Some(&a.project_id),
        Caller::Shell { project_id } => Some(project_id),
        _ => None,
    }
}

/// The calling passport's session id (for the proposal row and audit).
fn session_of(caller: &Caller) -> Option<String> {
    match caller {
        Caller::Agent(a) => a
            .map_key
            .as_deref()
            .and_then(crate::v2_session_map::lookup_by_agent_name)
            .map(|s| s.session_id.to_string()),
        _ => None,
    }
}

/// One Access Audit line for a passport's act; a person's put or decision
/// gets one too (`owner`), so the queue's history is in one place.
fn audit(event: &str, caller: &Caller, target: &str) {
    let user = caller.audit_user();
    crate::gardens_manage::audit(event, &user, target, None, session_of(caller).as_deref());
}

/// `ZenError` → the same JSON `zen_routes` answers.
fn zen_err(e: ZenError) -> CliResponse {
    crate::zen_routes::error_response(e)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GardenPut {
    garden: String,
    text: String,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WidgetPut {
    widget: String,
    file: String,
    text: String,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdBody {
    id: String,
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8], shape: &str) -> Result<T, CliResponse> {
    serde_json::from_slice(body).map_err(|e| refuse("400 Bad Request", "bad_request", format!("{shape}: {e}")))
}

fn clean_note(note: Option<String>) -> Result<Option<String>, CliResponse> {
    let n = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    if n.as_ref().is_some_and(|n| n.chars().count() > agent_edit::MAX_NOTE_CHARS || n.chars().any(|c| c.is_control() && c != '\n')) {
        return Err(refuse("400 Bad Request", "bad_request", "a note is one paragraph of at most 500 characters"));
    }
    Ok(n)
}

/// Run one route. `owner_token`: this computer's owner token presented
/// (the person). Answers 404 for a path that isn't this module's.
pub fn handle(path: &str, owner_token: bool, caller: &Caller, params: &HashMap<String, String>, body: &[u8]) -> CliResponse {
    let r = role(owner_token, caller);
    match path {
        "/cli/zen/garden/put" => garden_put(r, caller, body),
        "/cli/zen/widget/put" => widget_put(r, caller, body),
        "/cli/zen/proposals" => list(params),
        "/cli/zen/proposals/show" => show(params),
        "/cli/zen/proposals/approve" => approve(r, caller, body),
        "/cli/zen/proposals/reject" => reject(r, caller, body),
        _ => refuse("404 Not Found", "not_found", "no such zen route"),
    }
}

fn after_write() {
    if let Err(e) = crate::zen_routes::refresh_and_emit() {
        k2_core::log_debug!("[zen/agent] refresh after write: {e}");
    }
}

fn propose(caller: &Caller, kind: ProposalKind, target: &str, file: Option<&str>, text: &str, base: Option<&str>, note: Option<&str>) -> CliResponse {
    let Some(project) = project_of(caller) else {
        return refuse("403 Forbidden", "zen_local_only", "only a person or a local agent may change Gardens");
    };
    let session = session_of(caller);
    let made_by = format!("agent:{project}");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let p = agent_edit::insert(
        &conn,
        &NewProposal {
            kind,
            target,
            file,
            content: text,
            base_hash: &agent_edit::text_hash(base),
            note,
            made_by: &made_by,
            made_by_project: project,
            made_by_session: session.as_deref(),
            now: now(),
        },
    );
    drop(conn);
    match p {
        Ok(p) => {
            audit("zen-propose", caller, &format!("{} {}", p.id, describe(&p)));
            resp(
                "200 OK",
                json!({
                    "ok": true,
                    "proposed": true,
                    "proposal": p.summary(),
                    "message": format!(
                        "Proposed {}. A person approves it (Settings → Gardens, or k2 zen proposals approve {} outside K2), or an agent with \"Manage Gardens and data sources\".",
                        p.id, p.id
                    ),
                }),
            )
        }
        Err(e) => refuse("500 Internal Server Error", "db_error", format!("the proposal wasn't saved: {e}")),
    }
}

fn describe(p: &Proposal) -> String {
    match &p.file {
        Some(f) => format!("widgets/{}/{f}", p.target),
        None => format!("gardens/{}.toml", p.target),
    }
}

fn garden_put(r: Role, caller: &Caller, body: &[u8]) -> CliResponse {
    let b: GardenPut = match parse(body, "garden/put takes {garden, text, note?}") {
        Ok(b) => b,
        Err(e) => return e,
    };
    let note = match clean_note(b.note) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let f = files();
    match r {
        Role::Person | Role::Manager => match f.put_garden(&b.garden, &b.text, r == Role::Person) {
            Ok(v) => {
                after_write();
                if r == Role::Manager {
                    audit("zen-garden-put", caller, v["file"].as_str().unwrap_or(&b.garden));
                }
                resp("200 OK", v)
            }
            Err(e) => zen_err(e),
        },
        Role::Agent => {
            let id = match f.check_garden_put(&b.garden, &b.text, false) {
                Ok(id) => id,
                Err(e) => return zen_err(e),
            };
            let base = match f.garden_text(&id) {
                Ok((_, t)) => t,
                Err(e) => return zen_err(e),
            };
            propose(caller, ProposalKind::Garden, &id, None, &b.text, base.as_deref(), note.as_deref())
        }
    }
}

fn widget_put(r: Role, caller: &Caller, body: &[u8]) -> CliResponse {
    let b: WidgetPut = match parse(body, "widget/put takes {widget, file, text, note?}") {
        Ok(b) => b,
        Err(e) => return e,
    };
    let note = match clean_note(b.note) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let f = files();
    match r {
        Role::Person | Role::Manager => match f.put_widget_file(&b.widget, &b.file, &b.text) {
            Ok(v) => {
                after_write();
                if r == Role::Manager {
                    audit("zen-widget-put", caller, v["file"].as_str().unwrap_or(&b.widget));
                }
                resp("200 OK", v)
            }
            Err(e) => zen_err(e),
        },
        Role::Agent => {
            let name = match f.check_widget_put(&b.widget, &b.file, &b.text) {
                Ok(n) => n,
                Err(e) => return zen_err(e),
            };
            let base = match f.widget_file_text(&name, &b.file) {
                Ok((_, t)) => t,
                Err(e) => return zen_err(e),
            };
            propose(caller, ProposalKind::Widget, &name, Some(&b.file), &b.text, base.as_deref(), note.as_deref())
        }
    }
}

fn list(params: &HashMap<String, String>) -> CliResponse {
    let all = params.get("all").is_some_and(|v| v == "1" || v == "true");
    let db = k2_core::db::shared();
    let conn = db.lock();
    match agent_edit::list(&conn, all) {
        Ok(rows) => resp("200 OK", json!({ "ok": true, "proposals": rows.iter().map(Proposal::summary).collect::<Vec<_>>() })),
        Err(e) => refuse("500 Internal Server Error", "db_error", e.to_string()),
    }
}

fn load(id: &str) -> Result<Proposal, CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match agent_edit::get(&conn, id.trim()) {
        Ok(Some(p)) => Ok(p),
        Ok(None) => Err(refuse("404 Not Found", "proposal_not_found", format!("no proposal {id} (k2 zen proposals)"))),
        Err(e) => Err(refuse("500 Internal Server Error", "db_error", e.to_string())),
    }
}

/// The file's text now (`None`: no file).
fn current_text(f: &ZenFiles, p: &Proposal) -> Result<Option<String>, ZenError> {
    match (&p.kind, &p.file) {
        (ProposalKind::Widget, Some(file)) => f.widget_file_text(&p.target, file).map(|(_, t)| t),
        _ => f.garden_text(&p.target).map(|(_, t)| t),
    }
}

fn show(params: &HashMap<String, String>) -> CliResponse {
    let Some(id) = params.get("id").filter(|s| !s.is_empty()) else {
        return refuse("400 Bad Request", "bad_request", "proposals/show needs id=<proposal>");
    };
    let p = match load(id) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let f = files();
    let current = current_text(&f, &p).ok().flatten();
    let stale = agent_edit::text_hash(current.as_deref()) != p.base_hash;
    let mut v = p.summary();
    v["content"] = json!(p.content);
    v["current"] = json!(current);
    v["stale"] = json!(stale);
    v["path"] = json!(describe(&p));
    resp("200 OK", json!({ "ok": true, "proposal": v }))
}

fn decided_by(caller: &Caller) -> String {
    caller.audit_user()
}

fn approve(r: Role, caller: &Caller, body: &[u8]) -> CliResponse {
    if r == Role::Agent {
        return refuse(
            "403 Forbidden",
            "person_only",
            "A person approves Garden proposals (Settings → Gardens, or k2 zen proposals approve outside K2), or an agent with \"Manage Gardens and data sources\".",
        );
    }
    let b: IdBody = match parse(body, "proposals/approve takes {id}") {
        Ok(b) => b,
        Err(e) => return e,
    };
    let p = match load(&b.id) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if p.status != "pending" {
        return refuse("409 Conflict", "not_pending", format!("{} is {}, not waiting", p.id, p.status));
    }
    let f = files();
    let current = match current_text(&f, &p) {
        Ok(t) => t,
        Err(e) => return zen_err(e),
    };
    if agent_edit::text_hash(current.as_deref()) != p.base_hash {
        return refuse(
            "409 Conflict",
            "proposal_stale",
            format!("{} changed since this was proposed; reject it and ask for a new one", describe(&p)),
        );
    }
    let applied = match (&p.kind, &p.file) {
        (ProposalKind::Widget, Some(file)) => f.put_widget_file(&p.target, file, &p.content),
        _ => f.put_garden(&p.target, &p.content, r == Role::Person),
    };
    let v = match applied {
        Ok(v) => v,
        Err(e) => return zen_err(e),
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let closed = agent_edit::decide(&conn, &p.id, "approved", &decided_by(caller), now());
    drop(conn);
    if !matches!(closed, Ok(true)) {
        k2_core::log_debug!("[zen/agent] {} applied but was already decided: {closed:?}", p.id);
    }
    after_write();
    audit("zen-proposal-approve", caller, &format!("{} {}", p.id, describe(&p)));
    resp("200 OK", json!({ "ok": true, "id": p.id, "status": "approved", "applied": v }))
}

fn reject(r: Role, caller: &Caller, body: &[u8]) -> CliResponse {
    let b: IdBody = match parse(body, "proposals/reject takes {id}") {
        Ok(b) => b,
        Err(e) => return e,
    };
    let p = match load(&b.id) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let status = match r {
        Role::Person | Role::Manager => "rejected",
        Role::Agent if project_of(caller) == Some(p.made_by_project.as_str()) => "withdrawn",
        Role::Agent => {
            return refuse("403 Forbidden", "not_own", "an agent can only withdraw its own proposals");
        }
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let closed = agent_edit::decide(&conn, &p.id, status, &decided_by(caller), now());
    drop(conn);
    match closed {
        Ok(true) => {
            audit(&format!("zen-proposal-{}", if status == "withdrawn" { "withdraw" } else { "reject" }), caller, &p.id);
            resp("200 OK", json!({ "ok": true, "id": p.id, "status": status }))
        }
        Ok(false) => refuse("409 Conflict", "not_pending", format!("{} is {}, not waiting", p.id, p.status)),
        Err(e) => refuse("500 Internal Server Error", "db_error", e.to_string()),
    }
}

#[cfg(test)]
#[path = "zen_agent_routes_tests.rs"]
mod tests;
