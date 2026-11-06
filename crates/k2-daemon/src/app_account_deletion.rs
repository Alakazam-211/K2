//! App account deletion (0.46.9, feature `app-account-deletion-v1`).
//!
//! App Store rule 5.1.1(v): an App where people make an account must let
//! them delete it from inside the App; Google Play also wants a web page
//! for people who no longer have the App. Both are this one route, called
//! with the person's own session pass:
//!
//! ```text
//! POST /cli/skin/account/delete {password?}     a guest's own session pass, any door   → what went, what stays
//! GET  /cli/app/settings?app=                   the owner, an Admin, the App's agent   → accountDeletion, clientLogs
//! POST /cli/app/settings {app, accountDeletion?, deletionGraceDays?, clientLogsDays?}   (k2 app set)
//! GET  /cli/app/account-deletion?app=           pending requests and the settings
//! POST /cli/app/account-deletion/complete {app, request}   request = adr_… or the username
//! POST /cli/app/account-deletion/cancel   {app, request}
//! ```
//!
//! - **Recent sign-in.** The sign-in behind the pass must be at most 15
//!   minutes old (a phone's: its device sign-in, not its last refresh), or
//!   the body carries the person's password, checked with their login
//!   lock, as `pairing.start` does (`reauth_required`, `bad_login`,
//!   `423 locked`).
//! - **Modes** (per App, `request` by default): see
//!   `k2_core::skin::account_deletion`. A request signs the person out of
//!   this App everywhere and turns its access off at once; the owner (or
//!   the App's agent) completes it, or K2 does when the grace period ends
//!   ([`sweep`], hourly).
//! - **Never**: the box login (`host`, `not_for_host`) or an agent's own
//!   login (`agent_login`). A test guest may (it is a real login).
//! - **Records**: every request, completion and cancel is a line in the
//!   App's History (`k2 app history <app> --users`) and the Access Audit;
//!   the owner gets an Inbox notice and the App's agent an inbox note (it
//!   deletes what the App keeps in its own data).
//! - The answer names what went (`done`), what goes when a request
//!   completes (`pending`) and what K2 keeps and why (`kept`), so the App
//!   can show it.

use std::collections::HashMap;

use k2_core::skin::{self, account_deletion as store, app_login, app_pairing, SkinPass};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;

pub const FEATURE: &str = "app-account-deletion-v1";
pub const DELETE: &str = "/cli/skin/account/delete";
pub const SETTINGS: &str = "/cli/app/settings";
pub const LIST: &str = "/cli/app/account-deletion";
pub const COMPLETE: &str = "/cli/app/account-deletion/complete";
pub const CANCEL: &str = "/cli/app/account-deletion/cancel";

/// The person's own route (the dispatcher resolves the pass).
pub const PASS_ROUTES: &[&str] = &[DELETE];
/// The owner's and the App agent's routes.
pub const OWNER_ROUTES: &[&str] = &[SETTINGS, LIST, COMPLETE, CANCEL];
/// The POST-only owner routes (SETTINGS takes both).
pub const OWNER_POST_ROUTES: &[&str] = &[COMPLETE, CANCEL];
/// Request bodies here are at most this many bytes.
pub const MAX_BODY: usize = 16 * 1024;

/// History events (the `app_user_` prefix puts them in `--users`).
pub const EV_REQUESTED: &str = "app_user_deletion_requested";
pub const EV_DELETED: &str = "app_user_deleted";
pub const EV_CANCELLED: &str = "app_user_deletion_cancelled";
pub const EV_SETTINGS: &str = "app_user_deletion_settings";

/// Where a request came in (audit only).
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    /// The App whose gateway link this came through, if any.
    pub link_app: Option<String>,
    /// The client address the dispatcher vouches for, or `-`.
    pub ip: String,
}

// ── answers ──────────────────────────────────────────────────────────

fn refusal(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string() }
}

fn refusal_with(status: &'static str, code: &str, hint: &str, extra: Value) -> CliResponse {
    let mut err = json!({ "code": code, "hint": hint });
    if let (Some(e), Some(x)) = (err.as_object_mut(), extra.as_object()) {
        for (k, v) in x {
            e.insert(k.clone(), v.clone());
        }
    }
    CliResponse { status, content_type: "application/json", body: json!({ "ok": false, "error": err }).to_string() }
}

fn usage(hint: &str) -> CliResponse {
    refusal("400 Bad Request", "usage", hint)
}

pub fn guest_required() -> CliResponse {
    refusal("403 Forbidden", "guest_required", "only a person's own sign-in deletes their account: send their session pass (Authorization: Bearer k2skn_…)")
}

pub fn pass_required() -> CliResponse {
    refusal("401 Unauthorized", "pass_required", "send the person's session pass (Authorization: Bearer k2skn_…)")
}

pub fn too_large() -> CliResponse {
    refusal("413 Payload Too Large", "too_large", "request bodies here are at most 16 KiB")
}

fn not_for_host() -> CliResponse {
    refusal("403 Forbidden", "not_for_host", "account deletion is for a published App's own people, never the box login: the owner removes box logins")
}

fn answer(r: Result<Value, CliResponse>) -> CliResponse {
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => e,
    }
}

fn json_body(body: &[u8]) -> Result<Value, CliResponse> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(json!({}));
    }
    serde_json::from_slice::<Value>(body).ok().filter(Value::is_object).ok_or_else(|| usage("the body must be a JSON object"))
}

fn known_fields(v: &Value, known: &[&str]) -> Result<(), CliResponse> {
    if let Some(k) = v.as_object().and_then(|o| o.keys().find(|k| !known.contains(&k.as_str()))) {
        let shown: String = k.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(32).collect();
        return Err(usage(&format!("unknown field {shown} (known: {})", known.join(", "))));
    }
    Ok(())
}

fn text<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str)).map(str::trim).filter(|s| !s.is_empty())
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn auth_audit(event: &str, actor: &str, line: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, actor, line, "-", "-", "cli"));
}

/// The App's name for people (its published service name), else its id.
fn app_name(app: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::get_by_id(&conn, app).ok().flatten().map(|s| s.name).unwrap_or_else(|| app.to_string())
}

fn kept_json(scope: store::Scope) -> Value {
    Value::Array(store::kept(scope).into_iter().map(|(item, why)| json!({ "item": item, "why": why })).collect())
}

fn when(secs: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_else(|| secs.to_string())
}

// ── notes to the owner and the App's agent ───────────────────────────

/// The owner's Inbox notice and the App's agent's inbox note. Never a
/// password or a pass; the username only.
fn tell(app: &str, title: &str, body: &str) {
    let ws = skin::ties::app_workspace(app);
    let project = ws.clone().unwrap_or_else(|| k2_core::db::AUDIT_SENTINEL_IDS[0].to_string());
    if let Err(e) = crate::feedback_routes::file_owners_only_notice(&project, "k2", title, body) {
        k2_core::log_debug!("[account-deletion] owner notice for {app}: {e}");
    }
    if let Some(ws) = ws {
        crate::apps::alerts::note(&ws, title, body);
    }
}

// ── the person's route ───────────────────────────────────────────────

/// `POST /cli/skin/account/delete {password?}`.
pub fn handle_pass(path: &str, pass: &SkinPass, is_post: bool, body: &[u8], ctx: &Ctx) -> CliResponse {
    match (path, is_post) {
        (DELETE, true) => handle_delete(pass, body, ctx),
        _ => CliResponse::method_not_allowed(),
    }
}

fn error_code(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body).ok()?.pointer("/error/code")?.as_str().map(str::to_string)
}

pub fn handle_delete(pass: &SkinPass, body: &[u8], ctx: &Ctx) -> CliResponse {
    let r = delete(pass, body, ctx);
    if let Err(e) = &r {
        let code = error_code(&e.body).unwrap_or_else(|| "failed".into());
        if code != "guest_required" {
            auth_audit(
                "app_account_delete_refused",
                &format!("guest:{}", pass.principal_id.as_deref().unwrap_or("-")),
                &format!("app={} reason={code} ip={}", pass.app, ctx.ip),
            );
        }
    }
    answer(r)
}

fn delete(pass: &SkinPass, body: &[u8], ctx: &Ctx) -> Result<Value, CliResponse> {
    let pid = pass.principal_id.as_deref().filter(|_| pass.session).ok_or_else(guest_required)?.to_string();
    let app = pass.app.trim().to_string();
    if app.is_empty() || app == skin::HOST_APP_ID {
        return Err(not_for_host());
    }
    if ctx.link_app.as_deref().is_some_and(|l| l != app) {
        return Err(guest_required());
    }
    let v = json_body(body)?;
    known_fields(&v, &["password"])?;
    let person = skin::list_principals().map_err(CliResponse::internal_error)?.into_iter().find(|p| p.id == pid).ok_or_else(guest_required)?;
    // An agent's own login is the owner's to remove (k2 app user remove).
    let tie = skin::ties::get(&pid).map_err(CliResponse::internal_error)?;
    if person.agent_workspace.is_some() || tie.is_some_and(|t| t.reasons.iter().any(|r| r == skin::ties::REASON_AGENT_LOGIN)) {
        return Err(refusal("403 Forbidden", "agent_login", "an agent's own login can't delete itself: the owner removes it (k2 app user remove)"));
    }
    // A fresh sign-in, or the password again. A phone's pass is refreshed
    // often, so its age is its device sign-in's, never the pass's.
    let info = app_pairing::session_info(&pass.id)
        .map_err(CliResponse::internal_error)?
        .filter(|i| i.principal_id == pid)
        .ok_or_else(|| refusal("401 Unauthorized", "pass_invalid", "the pass is expired or revoked: sign in again"))?;
    let signed_in_at = match app_login::device_for_session(&pass.id).map_err(CliResponse::internal_error)? {
        Some(d) => d.created_at,
        None => info.created_at,
    };
    if now() - signed_in_at > app_pairing::FRESH_SIGNIN_SECS {
        let Some(password) = v.get("password").and_then(Value::as_str).filter(|p| !p.is_empty()) else {
            return Err(refusal(
                "401 Unauthorized",
                "reauth_required",
                "this sign-in is more than 15 minutes old: ask for the password and send it with the call ({password})",
            ));
        };
        match skin::check_and_record_login(&info.username, password) {
            skin::SkinLoginOutcome::Ok(p) if p.id == pid => {}
            skin::SkinLoginOutcome::MustChange(_) => {
                return Err(refusal(
                    "403 Forbidden",
                    crate::skin_routes::PASSWORD_CHANGE_REQUIRED,
                    "this password was set by someone else: sign in and choose a new one first",
                ))
            }
            skin::SkinLoginOutcome::LockedOut { retry_after_secs } => {
                return Err(refusal_with("423 Locked", "locked", "too many wrong passwords: try again later", json!({ "retryAfterSecs": retry_after_secs })))
            }
            _ => return Err(refusal("401 Unauthorized", "bad_login", "that password is wrong")),
        }
    }
    let settings = store::settings(&app).map_err(CliResponse::internal_error)?;
    let actor = format!("{}{pid}", skin::profile::SELF_ACTOR_PREFIX);
    if settings.mode == store::MODE_IMMEDIATE {
        let out = finish(&app, &pid, &person.username, &actor, None, "the person, from the App")?;
        return Ok(out);
    }
    let scope = store::scope(&app, &pid).map_err(CliResponse::internal_error)?;
    let (r, created) = store::request(&app, &pid, &person.username, settings.grace_days, "app", &actor).map_err(CliResponse::internal_error)?;
    let pending = store::pending_items(scope);
    if created {
        skin::guest_rooms::audit_user_event(
            &app,
            EV_REQUESTED,
            &actor,
            json!({ "username": person.username, "requestId": r.id, "scope": scope.as_str(), "dueAt": r.due_at, "door": if ctx.link_app.is_some() { "link" } else { "main" } }),
        );
        auth_audit(EV_REQUESTED, &actor, &format!("app={app} user={} request={} due={}", person.username, r.id, r.due_at));
        let name = app_name(&app);
        tell(
            &app,
            &format!("{} asked to delete their account in {name}", person.username),
            &format!(
                "{user} asked to delete their account in the App {name}. K2 signed them out of it everywhere and turned their access off. \
                 K2 completes the deletion on {due} (the App's grace period), or now: k2 app account-deletion complete {app} {user}. \
                 If they change their mind: k2 app account-deletion cancel {app} {user}. \
                 When it completes, delete what the App keeps for {user} in its own data. History: k2 app history {app} --users.",
                user = person.username,
                due = when(r.due_at),
            ),
        );
    }
    Ok(json!({
        "ok": true,
        "mode": store::MODE_REQUEST,
        "state": "requested",
        "scope": scope.as_str(),
        "requestId": r.id,
        "requestedAt": r.requested_at,
        "dueAt": r.due_at,
        "done": [store::item::SESSIONS, store::item::DEVICES, store::item::PUSH_DEVICES, store::item::PAIRINGS],
        "pending": pending,
        "kept": kept_json(scope),
    }))
}

/// Delete now (immediate mode, a completed request, the grace sweep):
/// skin.db first, then what lives in the daemon DB (read state, client
/// logs), then the History line, the audit and the notes.
fn finish(app: &str, pid: &str, username: &str, actor: &str, request_id: Option<&str>, by: &str) -> Result<Value, CliResponse> {
    let d = store::delete_now(app, pid, actor).map_err(CliResponse::internal_error)?;
    let mut done: Vec<&str> = d.done.clone();
    if d.found && d.scope == store::Scope::Account {
        crate::read_routes::forget_guest(pid);
        // The person's wallet items go at the wallet's next hourly pass
        // (it notices the person is gone).
        done.push(store::item::READ_STATE);
        done.push(store::item::WALLET);
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::app_client_logs::forget_person(&conn, app, pid).map_err(CliResponse::internal_error)?;
    }
    done.push(store::item::CLIENT_LOGS);
    let mode = if request_id.is_some() { store::MODE_REQUEST } else { store::MODE_IMMEDIATE };
    skin::guest_rooms::audit_user_event(
        app,
        EV_DELETED,
        actor,
        json!({ "username": username, "scope": d.scope.as_str(), "mode": mode, "requestId": request_id, "done": done, "found": d.found }),
    );
    auth_audit(EV_DELETED, actor, &format!("app={app} user={username} scope={} request={}", d.scope.as_str(), request_id.unwrap_or("-")));
    let name = app_name(app);
    tell(
        app,
        &format!("{username}'s account in {name} is deleted"),
        &format!(
            "{username}'s account in the App {name} is deleted ({by}). K2 removed: {}. \
             Now delete what the App keeps for {username} in its own data (K2 can't see inside it). History: k2 app history {app} --users.",
            done.join(", "),
        ),
    );
    Ok(json!({
        "ok": true,
        "mode": mode,
        "state": "deleted",
        "scope": d.scope.as_str(),
        "username": username,
        "done": done,
        "pending": [],
        "kept": kept_json(d.scope),
    }))
}

// ── the grace sweep ──────────────────────────────────────────────────

/// Complete every request whose grace period ended. Hourly (daemon_main).
pub fn sweep() -> usize {
    let due = match store::due(now()) {
        Ok(d) => d,
        Err(e) => {
            k2_core::log_debug!("[account-deletion] WARN sweep: {e}");
            return 0;
        }
    };
    let mut n = 0;
    for r in due {
        match finish(&r.app, &r.principal_id, &r.username, "k2:grace-period", Some(&r.id), "its grace period ended") {
            Ok(_) => n += 1,
            Err(e) => k2_core::log_debug!("[account-deletion] WARN complete {}: {}", r.id, e.body),
        }
    }
    n
}

// ── the owner's routes ───────────────────────────────────────────────

fn resolve_app(raw: Option<&str>) -> Result<String, CliResponse> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else { return Err(usage("name the App (app)")) };
    let app = crate::skin_routes::resolve_app_target(raw).map_err(|e| usage(&e))?;
    if app == skin::HOST_APP_ID {
        return Err(not_for_host());
    }
    Ok(app)
}

fn app_of_workspace(app: &str, ws: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    matches!(k2_core::published_services::get_by_id(&conn, app), Ok(Some(svc)) if svc.project_id == ws)
}

fn workspace_path(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![project_id], |r| r.get::<_, String>(0)).ok()
}

/// Who may read: the owner, a K2 shell tab, any login, the App's own agent.
fn reader(who: &Who, app: &str) -> Result<(), CliResponse> {
    match who {
        Who::Owner | Who::Shell { .. } | Who::Login { .. } => Ok(()),
        Who::Agent { project_id, .. } if app_of_workspace(app, project_id) => Ok(()),
        Who::Agent { .. } => Err(refusal("403 Forbidden", "outside_workspace", "an agent reads this only for its own workspace's Apps")),
        Who::Wrong | Who::Invalid => Err(refusal("401 Unauthorized", "unauthorized", "invalid or missing auth token")),
    }
}

/// Who may change: the owner, an Admin, a K2 shell tab, or the App's own
/// agent with "Let this agent manage its Apps' users" on.
fn writer(who: &Who, app: &str) -> Result<(), CliResponse> {
    use k2_core::connect_users::Role;
    match who {
        Who::Owner | Who::Shell { .. } | Who::Login { role: Role::Owner | Role::Admin, .. } => Ok(()),
        Who::Agent { project_id, .. } => {
            if !app_of_workspace(app, project_id) {
                return Err(refusal("403 Forbidden", "outside_workspace", "an agent manages this only for its own workspace's Apps"));
            }
            let path = workspace_path(project_id).unwrap_or_default();
            if path.is_empty() || !crate::skin_routes::agent_may_manage_app_users(&path) {
                return Err(crate::skin_routes::app_users_off_response());
            }
            Ok(())
        }
        Who::Login { .. } => Err(refusal("403 Forbidden", "owner_only", "the owner or an Admin changes this")),
        Who::Wrong | Who::Invalid => Err(refusal("401 Unauthorized", "unauthorized", "invalid or missing auth token")),
    }
}

fn settings_json(app: &str) -> Result<Value, CliResponse> {
    let s = store::settings(app).map_err(CliResponse::internal_error)?;
    let (days, set) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::app_client_logs::keep_days(&conn, app).map_err(CliResponse::internal_error)?
    };
    Ok(json!({
        "ok": true,
        "app": app,
        "accountDeletion": { "mode": s.mode, "graceDays": s.grace_days, "set": s.set },
        "clientLogs": { "keepDays": days, "set": set },
    }))
}

fn int_field(v: &Value, key: &str) -> Result<Option<i64>, CliResponse> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n.as_i64().map(Some).ok_or_else(|| usage(&format!("{key} must be a whole number"))),
        Some(Value::String(s)) => s.trim().trim_end_matches('d').parse::<i64>().map(Some).map_err(|_| usage(&format!("{key} must be a whole number of days"))),
        Some(_) => Err(usage(&format!("{key} must be a whole number of days"))),
    }
}

fn settings_route(who: &Who, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> Result<Value, CliResponse> {
    if !is_post {
        let app = resolve_app(params.get("app").map(String::as_str))?;
        reader(who, &app)?;
        return settings_json(&app);
    }
    let v = json_body(body)?;
    known_fields(&v, &["app", "accountDeletion", "deletionGraceDays", "clientLogsDays"])?;
    let app = resolve_app(text(&v, &["app"]))?;
    writer(who, &app)?;
    let mode = text(&v, &["accountDeletion"]);
    if let Some(m) = mode {
        if store::parse_mode(m).is_none() {
            return Err(usage("accountDeletion is request or immediate"));
        }
    }
    let grace = int_field(&v, "deletionGraceDays")?;
    if let Some(d) = grace {
        if !(store::MIN_GRACE_DAYS..=store::MAX_GRACE_DAYS).contains(&d) {
            return Err(usage(&format!("deletionGraceDays is {} to {} days", store::MIN_GRACE_DAYS, store::MAX_GRACE_DAYS)));
        }
    }
    let keep = int_field(&v, "clientLogsDays")?;
    if let Some(d) = keep {
        if !(k2_core::app_client_logs::MIN_KEEP_DAYS..=k2_core::app_client_logs::MAX_KEEP_DAYS).contains(&d) {
            return Err(usage(&format!(
                "clientLogsDays is {} to {} days",
                k2_core::app_client_logs::MIN_KEEP_DAYS,
                k2_core::app_client_logs::MAX_KEEP_DAYS
            )));
        }
    }
    if mode.is_none() && grace.is_none() && keep.is_none() {
        return Err(usage("send accountDeletion, deletionGraceDays and/or clientLogsDays"));
    }
    let actor = who.label();
    if mode.is_some() || grace.is_some() {
        let s = store::set_settings(&app, mode, grace, &actor).map_err(CliResponse::internal_error)?;
        skin::guest_rooms::audit_user_event(&app, EV_SETTINGS, &actor, json!({ "mode": s.mode, "graceDays": s.grace_days }));
        auth_audit(EV_SETTINGS, &actor, &format!("app={app} mode={} grace={}", s.mode, s.grace_days));
    }
    if let Some(d) = keep {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::app_client_logs::set_keep_days(&conn, &app, d, &actor).map_err(CliResponse::internal_error)?;
        drop(conn);
        auth_audit("app_client_logs_settings", &actor, &format!("app={app} keepDays={d}"));
    }
    settings_json(&app)
}

fn list_route(who: &Who, params: &HashMap<String, String>) -> Result<Value, CliResponse> {
    let app = resolve_app(params.get("app").map(String::as_str))?;
    reader(who, &app)?;
    let rows = store::pending(&app).map_err(CliResponse::internal_error)?;
    let mut pending = Vec::new();
    for r in rows {
        let scope = store::scope(&app, &r.principal_id).map_err(CliResponse::internal_error)?;
        pending.push(json!({
            "id": r.id,
            "username": r.username,
            "requestedAt": r.requested_at,
            "dueAt": r.due_at,
            "via": r.via,
            "scope": scope.as_str(),
        }));
    }
    let mut out = settings_json(&app)?;
    out["pending"] = Value::Array(pending);
    Ok(out)
}

fn request_of(app: &str, v: &Value) -> Result<store::Request, CliResponse> {
    let key = text(v, &["request", "id", "user", "username"]).ok_or_else(|| usage("name the request (its adr_… id or the username)"))?;
    store::find(app, key)
        .map_err(CliResponse::internal_error)?
        .ok_or_else(|| refusal("404 Not Found", "not_found", "no pending deletion request by that id or username in this App"))
}

fn complete_route(who: &Who, body: &[u8]) -> Result<Value, CliResponse> {
    let v = json_body(body)?;
    known_fields(&v, &["app", "request", "id", "user", "username"])?;
    let app = resolve_app(text(&v, &["app"]))?;
    writer(who, &app)?;
    let r = request_of(&app, &v)?;
    let mut out = finish(&app, &r.principal_id, &r.username, &who.label(), Some(&r.id), &format!("completed by {}", who.label()))?;
    out["requestId"] = json!(r.id);
    Ok(out)
}

fn cancel_route(who: &Who, body: &[u8]) -> Result<Value, CliResponse> {
    let v = json_body(body)?;
    known_fields(&v, &["app", "request", "id", "user", "username"])?;
    let app = resolve_app(text(&v, &["app"]))?;
    writer(who, &app)?;
    let r = request_of(&app, &v)?;
    let actor = who.label();
    let gone = store::cancel(&app, &r.id, &actor).map_err(CliResponse::internal_error)?;
    let restored = gone.map(|g| g.grant_ids.len()).unwrap_or(0);
    skin::guest_rooms::audit_user_event(&app, EV_CANCELLED, &actor, json!({ "username": r.username, "requestId": r.id, "restored": restored }));
    auth_audit(EV_CANCELLED, &actor, &format!("app={app} user={} request={}", r.username, r.id));
    Ok(json!({ "ok": true, "app": app, "requestId": r.id, "username": r.username, "restored": restored }))
}

/// The owner's routes (blocking).
pub fn handle_owner(path: &str, who: &Who, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    answer(match (path, is_post) {
        (SETTINGS, _) => settings_route(who, is_post, body, params),
        (LIST, false) => list_route(who, params),
        (COMPLETE, true) => complete_route(who, body),
        (CANCEL, true) => cancel_route(who, body),
        _ => return CliResponse::method_not_allowed(),
    })
}
