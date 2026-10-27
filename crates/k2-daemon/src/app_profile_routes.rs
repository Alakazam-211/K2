//! App user profile (prd-app-user-profile-v1 P2/P5/P6, 0.46.5; vs-live
//! U16–U19, U24 override the PRD).
//!
//! The person's own routes (catalog `app` rows on both doors and the main
//! port; the App and the person come from the pass, never the body). A
//! guest's own login session pass only: an App's link pass, an App key, a
//! platform pass, the owner token or a Connect login is `403
//! guest_required`.
//! - `GET /cli/skin/profile` (`profile.get`): their profile in this App.
//!   The picture rides only with `?image=1`, or when `?ifImageHash=` differs
//!   from the stored hash (U17 e); `imageHash` is always there.
//! - `POST /cli/skin/profile {fullName?, displayName?}` (`profile.update`).
//! - `POST /cli/skin/profile/image {image}` (`profile.setImage`),
//!   `POST /cli/skin/profile/image/clear` (`profile.clearImage`).
//! - `GET /cli/skin/sessions` (`profile.sessions`),
//!   `POST /cli/skin/sessions/revoke {id}` (`profile.revokeSession`),
//!   `POST /cli/skin/sessions/revoke-others` (`profile.signOutOthers`).
//!
//! Per person: an agent login reads but never writes (`403 agent_login`);
//! a test guest edits its names and picture (they die with it). Per App:
//! `editNames` / `editImage` (`403 edit_off`, nothing written). Box:
//! `K2_APP_PROFILE=off` turns the writes off (reads stay).
//!
//! Owner routes (`k2 app user show|image|sessions`, `k2 app profile`):
//! - `GET /cli/skin/access/profile?app=&user=[&image=1]`;
//! - `POST /cli/skin/access/profile/image/clear {user}`;
//! - `POST /cli/skin/access/sessions/revoke {app, user, id | all}`;
//! - `GET /cli/app/profile-settings?app=`, `POST /cli/app/profile-settings
//!   {app, editNames?, editImage?}`: the owner, or the App's own agent
//!   with "manage its Apps' users" on, lower only, never `host` (U16).

use std::collections::HashMap;
use std::sync::OnceLock;

use k2_core::skin::{self, profile, SkinPass, SkinPrincipal};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::cli_response::CliResponse;

pub const PROFILE: &str = "/cli/skin/profile";
pub const PROFILE_IMAGE: &str = "/cli/skin/profile/image";
pub const PROFILE_IMAGE_CLEAR: &str = "/cli/skin/profile/image/clear";
pub const SESSIONS: &str = "/cli/skin/sessions";
pub const SESSIONS_REVOKE: &str = "/cli/skin/sessions/revoke";
pub const SESSIONS_REVOKE_OTHERS: &str = "/cli/skin/sessions/revoke-others";
pub const OWNER_PROFILE: &str = "/cli/skin/access/profile";
pub const OWNER_IMAGE_CLEAR: &str = "/cli/skin/access/profile/image/clear";
pub const OWNER_SESSIONS_REVOKE: &str = "/cli/skin/access/sessions/revoke";
pub const SETTINGS: &str = "/cli/app/profile-settings";

/// The person's own routes (the dispatcher resolves the pass).
pub const PASS_ROUTES: &[&str] = &[PROFILE, PROFILE_IMAGE, PROFILE_IMAGE_CLEAR, SESSIONS, SESSIONS_REVOKE, SESSIONS_REVOKE_OTHERS];
/// The POST-only ones among them.
pub const POST_ROUTES: &[&str] = &[PROFILE_IMAGE, PROFILE_IMAGE_CLEAR, SESSIONS_REVOKE, SESSIONS_REVOKE_OTHERS];
/// Body cap: a base64 picture of 2 MiB plus slack (§3.4).
pub const MAX_BODY: usize = 3 * 1024 * 1024;
/// The handle prefix of a browser session (U18: never the token id).
pub const SESSION_HANDLE_PREFIX: &str = "ses_";
/// The refusal code for an agent without "manage its Apps' users" (the
/// code the App-grants `apps.manage-users` wall teaches).
pub const APP_USERS_OFF: &str = "app_users_off";

// ── answers ──────────────────────────────────────────────────────────

fn refusal(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint } }).to_string(),
    }
}

fn usage(hint: &str) -> CliResponse {
    refusal("400 Bad Request", "usage", hint)
}

pub fn guest_required() -> CliResponse {
    refusal("403 Forbidden", "guest_required", "only a person's own login pass has a profile and sessions")
}

/// A credential that isn't a skin pass on a pass route.
pub fn pass_required() -> CliResponse {
    refusal("403 Forbidden", "skin_token", "this route is for a person's own login pass (k2skn_)")
}

pub fn too_large() -> CliResponse {
    refusal("413 Payload Too Large", "too_large", "a picture is at most 2 MiB (send data:image/png|jpeg|webp;base64,…)")
}

fn not_found() -> CliResponse {
    refusal("404 Not Found", "not_found", "no session of yours in this App by that id")
}

fn rate_limited(retry_after_secs: i64) -> CliResponse {
    CliResponse {
        status: "429 Too Many Requests",
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": "rate_limited", "hint": "too many profile changes lately: wait, then try again", "retryAfterSecs": retry_after_secs } })
            .to_string(),
    }
}

fn json_body(body: &[u8]) -> Result<Value, CliResponse> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(json!({}));
    }
    serde_json::from_slice::<Value>(body).ok().filter(Value::is_object).ok_or_else(|| usage("the body must be a JSON object"))
}

fn text<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str)).map(str::trim).filter(|s| !s.is_empty())
}

fn audit(event: &str, actor: &str, line: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, actor, line, "-", "-", "cli"));
}

// ── session handles (U18) ────────────────────────────────────────────

fn handle_key() -> &'static [u8; 32] {
    static K: OnceLock<[u8; 32]> = OnceLock::new();
    K.get_or_init(|| {
        // Two v4 UUIDs: 244 random bits from the OS generator.
        let mut k = [0u8; 32];
        k[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        k[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        k
    })
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    k[..key.len().min(64)].copy_from_slice(&key[..key.len().min(64)]);
    let mut inner = Sha256::new();
    inner.update(k.iter().map(|b| b ^ 0x36).collect::<Vec<u8>>());
    inner.update(msg);
    let ih = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(k.iter().map(|b| b ^ 0x5c).collect::<Vec<u8>>());
    outer.update(ih);
    outer.finalize().into()
}

/// A browser session's opaque handle: `ses_` + 32 hex of an HMAC of its
/// token id under a key this daemon keeps in memory (never the token id,
/// a key prefix or a hash anyone else can compute; stable while the
/// daemon runs: list again after a restart).
pub fn session_handle(token_id: &str) -> String {
    let mac = hmac_sha256(handle_key(), token_id.as_bytes());
    format!("{SESSION_HANDLE_PREFIX}{}", mac[..16].iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn session_json(s: &profile::SessionRow) -> Value {
    let id = if s.kind == "browser" { session_handle(&s.key) } else { s.key.clone() };
    let mut v = json!({
        "id": id,
        "kind": s.kind,
        "client": s.client,
        "via": s.via,
        "createdAt": s.created_at,
        "lastSeenAt": s.last_seen_at,
        "expiresAt": s.expires_at,
        "current": s.current,
        "push": s.push,
    });
    if let Some(p) = &s.push_platform {
        v["pushPlatform"] = json!(p);
    }
    v
}

// ── the person ───────────────────────────────────────────────────────

/// Who the pass is, for a profile route: a live guest session or `Err`.
fn person(pass: &SkinPass) -> Result<SkinPrincipal, CliResponse> {
    let Some(pid) = pass.principal_id.as_deref().filter(|_| pass.session) else {
        return Err(guest_required());
    };
    match skin::list_principals() {
        Ok(ps) => ps.into_iter().find(|p| p.id == pid).ok_or_else(guest_required),
        Err(e) => Err(CliResponse::internal_error(e)),
    }
}

fn is_test_guest(pid: &str) -> Result<bool, CliResponse> {
    skin::test_guests::get(pid).map(|t| t.is_some()).map_err(CliResponse::internal_error)
}

/// An agent's own login (`principals.agent_workspace`, or a tie whose
/// reasons include `agent_login`): reads only.
fn is_agent_login(p: &SkinPrincipal) -> Result<bool, CliResponse> {
    if p.agent_workspace.is_some() {
        return Ok(true);
    }
    let tie = skin::ties::get(&p.id).map_err(CliResponse::internal_error)?;
    Ok(tie.is_some_and(|t| t.reasons.iter().any(|r| r == skin::ties::REASON_AGENT_LOGIN)))
}

/// The role on this person's grant for `app` (the login's rule), by name.
fn role_on_app(pid: &str, app: &str) -> Option<String> {
    let grants = skin::list_grants().ok()?;
    let g = grants.iter().find(|g| g.subject_kind == "principal" && g.subject_id == pid && g.kind == "app" && g.target_id == app)?;
    let rid = g.role_id.as_deref()?;
    skin::list_roles().ok()?.into_iter().find(|r| r.id == rid).map(|r| r.name)
}

fn edits_refused(p: &SkinPrincipal) -> Option<CliResponse> {
    if !profile::writes_enabled() {
        return Some(refusal("403 Forbidden", "edit_off", "profile changes are off on this server"));
    }
    match is_agent_login(p) {
        Ok(true) => Some(refusal(
            "403 Forbidden",
            "agent_login",
            "an agent's login takes its name and picture from its workspace; the owner manages it (k2 app user …)",
        )),
        Ok(false) => None,
        Err(r) => Some(r),
    }
}

/// The profile object (`profile.get`, and the answer of every edit).
/// `with_image`: carry the picture itself; `if_hash`: carry it only when
/// the stored hash differs.
pub fn profile_view(pass: &SkinPass, p: &SkinPrincipal, with_image: bool, if_hash: Option<&str>) -> Result<Value, CliResponse> {
    let app = pass.app.clone();
    let row = profile::row(&p.id).map_err(CliResponse::internal_error)?;
    let settings = profile::settings(&app).map_err(CliResponse::internal_error)?;
    let test = is_test_guest(&p.id)?;
    let agent_login = is_agent_login(p)?;
    let writes = profile::writes_enabled() && !agent_login;
    let room_ids: Vec<String> = pass.room_policy.keys().cloned().collect();
    let rooms: Vec<Value> = skin::live_agents(&room_ids)
        .into_iter()
        .map(|a| json!({ "room": a.project_id, "handle": a.handle, "name": a.display_name }))
        .collect();
    let sessions = profile::sessions_for(&p.id, &app, &pass.id).map_err(CliResponse::internal_error)?;
    let browsers = sessions.iter().filter(|s| s.kind == "browser").count();
    let devices = sessions.iter().filter(|s| s.kind == "device").count();
    let push = sessions.iter().filter(|s| s.push).count();
    let mut v = json!({
        "ok": true,
        "username": p.username,
        "fullName": p.full_name,
        "nameSetBySelf": row.name_self,
        "displayName": row.display_name,
        "email": p.email,
        "imageHash": row.image_hash,
        "app": app,
        "role": role_on_app(&p.id, &app),
        "rooms": rooms,
        "createdAt": p.created_at,
        "lastSignInAt": profile::last_sign_in(&p.id, &app).map_err(CliResponse::internal_error)?,
        "hasPassword": p.has_password,
        "test": test,
        "agentLogin": agent_login,
        "can": { "editNames": writes && settings.edit_names, "editImage": writes && settings.edit_image },
        "counts": { "sessions": browsers, "devices": devices, "pushDevices": push },
    });
    let send_image = with_image || if_hash.is_some_and(|h| row.image_hash.as_deref() != Some(h.trim()));
    if send_image {
        v["image"] = json!(row.image_data_url());
    }
    Ok(v)
}

/// `GET /cli/skin/profile`.
pub fn handle_get(pass: &SkinPass, params: &HashMap<String, String>) -> CliResponse {
    let p = match person(pass) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let with_image = params.get("image").is_some_and(|v| matches!(v.trim(), "1" | "true" | "yes"));
    let if_hash = params.get("ifImageHash").map(String::as_str).filter(|s| !s.trim().is_empty());
    match profile_view(pass, &p, with_image, if_hash) {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(r) => r,
    }
}

/// A name field: absent → keep; `null` or `""` → clear; a string → cleaned.
fn name_field(v: &Value, key: &str, max: usize) -> Result<Option<Option<String>>, CliResponse> {
    match v.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(s)) => Ok(Some(profile::clean_name(s, max))),
        Some(_) => Err(usage(&format!("{key} must be text or null"))),
    }
}

/// `POST /cli/skin/profile {fullName?, displayName?}`.
pub fn handle_update(pass: &SkinPass, body: &[u8]) -> CliResponse {
    let p = match person(pass) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Some(k) = v.as_object().and_then(|o| o.keys().find(|k| !matches!(k.as_str(), "fullName" | "displayName"))) {
        return usage(&format!("unknown field {k}: send fullName and/or displayName"));
    }
    let full = match name_field(&v, "fullName", profile::MAX_FULL_NAME_CHARS) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let display = match name_field(&v, "displayName", profile::MAX_DISPLAY_NAME_CHARS) {
        Ok(x) => x,
        Err(r) => return r,
    };
    if full.is_none() && display.is_none() {
        return usage("send fullName and/or displayName (null or \"\" clears one)");
    }
    if let Some(r) = edits_refused(&p) {
        return r;
    }
    match profile::settings(&pass.app) {
        Ok(s) if !s.edit_names => {
            let field = if full.is_some() { "fullName" } else { "displayName" };
            return CliResponse {
                status: "403 Forbidden",
                content_type: "application/json",
                body: json!({ "ok": false, "error": { "code": "edit_off", "field": field, "hint": "this App doesn't let people change their names" } }).to_string(),
            };
        }
        Ok(_) => {}
        Err(e) => return CliResponse::internal_error(e),
    }
    if let Err(retry) = profile::take_write(&p.id) {
        return rate_limited(retry);
    }
    let actor = format!("{}{}", profile::SELF_ACTOR_PREFIX, p.id);
    if let Some(d) = &display {
        if let Err(e) = profile::set_display_name(&p.id, d.as_deref()) {
            return CliResponse::internal_error(e);
        }
    }
    let mut p = p;
    if let Some(f) = &full {
        let before = p.full_name.as_deref().map_or(0, |s| s.chars().count());
        match skin::set_principal_full_name_as(&p.username, f.as_deref(), &actor) {
            Ok(np) => p = np,
            Err(e) => return CliResponse::internal_error(e),
        }
        audit(
            "app_user_profile_name",
            &actor,
            &format!("{} app={} fullName {} -> {} chars", p.username, pass.app, before, f.as_deref().map_or(0, |s| s.chars().count())),
        );
    }
    if display.is_some() {
        audit("app_user_profile_name", &actor, &format!("{} app={} displayName set", p.username, pass.app));
    }
    match profile_view(pass, &p, false, None) {
        Ok(view) => CliResponse::ok_json(json!({ "ok": true, "profile": view }).to_string()),
        Err(r) => r,
    }
}

fn image_refused(p: &SkinPrincipal, app: &str) -> Option<CliResponse> {
    if let Some(r) = edits_refused(p) {
        return Some(r);
    }
    match profile::settings(app) {
        Ok(s) if !s.edit_image => Some(CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: json!({ "ok": false, "error": { "code": "edit_off", "field": "image", "hint": "this App doesn't let people change their picture" } }).to_string(),
        }),
        Ok(_) => None,
        Err(e) => Some(CliResponse::internal_error(e)),
    }
}

/// `POST /cli/skin/profile/image {image}`.
pub fn handle_set_image(pass: &SkinPass, body: &[u8]) -> CliResponse {
    let p = match person(pass) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(raw) = text(&v, &["image"]) else {
        return usage("send {image: \"data:image/png|jpeg|webp;base64,…\"}");
    };
    if let Some(r) = image_refused(&p, &pass.app) {
        return r;
    }
    if let Err(retry) = profile::take_write(&p.id) {
        return rate_limited(retry);
    }
    use k2_core::workspace::agent_look::{normalize_profile_image, LookError};
    let jpeg = match normalize_profile_image(raw) {
        Ok(b) => b,
        Err(e) => {
            let status = match e {
                LookError::TypeRefused(_) => "415 Unsupported Media Type",
                LookError::TooLarge(_) => "413 Payload Too Large",
                _ => "400 Bad Request",
            };
            let code = match e {
                LookError::NotDataUrl => "usage",
                _ => e.code(),
            };
            return refusal(status, code, &e.to_string());
        }
    };
    let hash = match profile::set_image(&p.id, &jpeg) {
        Ok(h) => h,
        Err(e) => return CliResponse::internal_error(e),
    };
    audit("app_user_profile_image", &format!("{}{}", profile::SELF_ACTOR_PREFIX, p.id), &format!("{} app={} set", p.username, pass.app));
    let row = profile::ProfileRow { image: Some(jpeg), ..Default::default() };
    CliResponse::ok_json(json!({ "ok": true, "image": row.image_data_url(), "imageHash": hash }).to_string())
}

/// `POST /cli/skin/profile/image/clear`.
pub fn handle_clear_image(pass: &SkinPass) -> CliResponse {
    let p = match person(pass) {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Some(r) = image_refused(&p, &pass.app) {
        return r;
    }
    if let Err(retry) = profile::take_write(&p.id) {
        return rate_limited(retry);
    }
    match profile::clear_image(&p.id) {
        Ok(had) => {
            if had {
                audit("app_user_profile_image", &format!("{}{}", profile::SELF_ACTOR_PREFIX, p.id), &format!("{} app={} cleared", p.username, pass.app));
            }
            CliResponse::ok_json(json!({ "ok": true }).to_string())
        }
        Err(e) => CliResponse::internal_error(e),
    }
}

/// `GET /cli/skin/sessions`.
pub fn handle_sessions(pass: &SkinPass) -> CliResponse {
    let p = match person(pass) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match profile::sessions_for(&p.id, &pass.app, &pass.id) {
        Ok(rows) => CliResponse::ok_json(json!({ "ok": true, "sessions": rows.iter().map(session_json).collect::<Vec<_>>() }).to_string()),
        Err(e) => CliResponse::internal_error(e),
    }
}

/// End one session of `principal_id` in `app` by its wire id (a `ses_`
/// handle or a device sign-in id). `Ok(false)`: none of theirs by that id.
fn revoke_by_wire_id(principal_id: &str, app: &str, id: &str, owner: bool) -> Result<bool, String> {
    if id.starts_with(SESSION_HANDLE_PREFIX) {
        let rows = profile::sessions_for(principal_id, app, "")?;
        match rows.iter().find(|s| s.kind == "browser" && session_handle(&s.key) == id) {
            Some(s) => profile::revoke_browser_session(principal_id, app, &s.key),
            None => Ok(false),
        }
    } else if owner {
        let mine = skin::app_login::devices_for(principal_id, app)?.iter().any(|d| d.id == id);
        if mine {
            skin::app_login::revoke_by_owner(id)
        } else {
            Ok(false)
        }
    } else {
        skin::app_login::revoke_own(principal_id, app, id)
    }
}

/// `POST /cli/skin/sessions/revoke {id}`: one browser session or device
/// sign-in of this person in this App (their current one too: a logout).
pub fn handle_revoke(pass: &SkinPass, body: &[u8]) -> CliResponse {
    let p = match person(pass) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = text(&v, &["id"]) else {
        return usage("send {id} (from profile.sessions)");
    };
    match revoke_by_wire_id(&p.id, &pass.app, id, false) {
        Ok(true) => {
            audit("app_user_session_revoked", &format!("{}{}", profile::SELF_ACTOR_PREFIX, p.id), &format!("{} app={} one", p.username, pass.app));
            CliResponse::ok_json(json!({ "ok": true }).to_string())
        }
        Ok(false) => not_found(),
        Err(e) => CliResponse::internal_error(e),
    }
}

/// `POST /cli/skin/sessions/revoke-others`: every other session and
/// device sign-in of this person in this App ends; this pass stays.
pub fn handle_revoke_others(pass: &SkinPass) -> CliResponse {
    let p = match person(pass) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match profile::sign_out_others(&p.id, &pass.app, &pass.id) {
        Ok(n) => {
            audit("app_user_session_revoked", &format!("{}{}", profile::SELF_ACTOR_PREFIX, p.id), &format!("{} app={} others={n}", p.username, pass.app));
            CliResponse::ok_json(json!({ "ok": true, "ended": n }).to_string())
        }
        Err(e) => CliResponse::internal_error(e),
    }
}

/// The pass routes (the dispatcher resolved `pass`).
pub fn handle_pass(path: &str, pass: &SkinPass, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    match (path, is_post) {
        (PROFILE, false) => handle_get(pass, params),
        (PROFILE, true) => handle_update(pass, body),
        (PROFILE_IMAGE, true) => handle_set_image(pass, body),
        (PROFILE_IMAGE_CLEAR, true) => handle_clear_image(pass),
        (SESSIONS, false) => handle_sessions(pass),
        (SESSIONS_REVOKE, true) => handle_revoke(pass, body),
        (SESSIONS_REVOKE_OTHERS, true) => handle_revoke_others(pass),
        _ => CliResponse::method_not_allowed(),
    }
}

// ── the owner ────────────────────────────────────────────────────────

fn principal_named(user: &str) -> Result<Option<SkinPrincipal>, CliResponse> {
    let Some(u) = skin::login_principal_username(user).map_err(CliResponse::internal_error)? else { return Ok(None) };
    skin::list_principals().map(|ps| ps.into_iter().find(|p| p.username == u)).map_err(CliResponse::internal_error)
}

fn resolve_app(raw: Option<&str>) -> Result<String, CliResponse> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else { return Err(usage("name the App (app)")) };
    crate::skin_routes::resolve_app_target(raw).map_err(|e| usage(&e))
}

/// `GET /cli/skin/access/profile?app=&user=[&image=1]` (owner): the
/// person's profile in that App as the owner sees it, with their sessions.
pub fn handle_owner_profile(params: &HashMap<String, String>) -> CliResponse {
    let app = match resolve_app(params.get("app").map(String::as_str)) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let Some(user) = params.get("user").or_else(|| params.get("username")).map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        return usage("name the person (user)");
    };
    let p = match principal_named(user) {
        Ok(Some(p)) => p,
        Ok(None) => return refusal("404 Not Found", "not_found", "no such App user"),
        Err(r) => return r,
    };
    let row = match profile::row(&p.id) {
        Ok(r) => r,
        Err(e) => return CliResponse::internal_error(e),
    };
    let sessions = match profile::sessions_for(&p.id, &app, "") {
        Ok(s) => s,
        Err(e) => return CliResponse::internal_error(e),
    };
    let with_image = params.get("image").is_some_and(|v| matches!(v.trim(), "1" | "true" | "yes"));
    let mut v = json!({
        "ok": true,
        "username": p.username,
        "fullName": p.full_name,
        // U24: a self-set name is shown with the username beside it.
        "nameSetBySelf": row.name_self,
        "displayName": row.display_name,
        "email": p.email,
        "imageHash": row.image_hash,
        "app": app,
        "role": role_on_app(&p.id, &app),
        "createdAt": p.created_at,
        "lastSignInAt": profile::last_sign_in(&p.id, &app).ok().flatten(),
        "hasPassword": p.has_password,
        "sessions": sessions.iter().map(session_json).collect::<Vec<_>>(),
        "settings": profile::settings(&app).ok(),
    });
    if with_image {
        v["image"] = json!(row.image_data_url());
    }
    CliResponse::ok_json(v.to_string())
}

/// `POST /cli/skin/access/profile/image/clear {user}` (owner).
pub fn handle_owner_image_clear(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(user) = text(&v, &["user", "username"]) else { return usage("name the person (user)") };
    let p = match principal_named(user) {
        Ok(Some(p)) => p,
        Ok(None) => return refusal("404 Not Found", "not_found", "no such App user"),
        Err(r) => return r,
    };
    match profile::clear_image(&p.id) {
        Ok(had) => {
            audit("app_user_profile_image", actor, &format!("{} cleared by the owner", p.username));
            CliResponse::ok_json(json!({ "ok": true, "username": p.username, "cleared": had }).to_string())
        }
        Err(e) => CliResponse::internal_error(e),
    }
}

/// `POST /cli/skin/access/sessions/revoke {app, user, id | all: true}`
/// (owner): one session (`id` from the owner's profile view) or every
/// session and device sign-in of the person in that App.
pub fn handle_owner_sessions_revoke(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let app = match resolve_app(text(&v, &["app", "appId"])) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let Some(user) = text(&v, &["user", "username"]) else { return usage("name the person (user)") };
    let p = match principal_named(user) {
        Ok(Some(p)) => p,
        Ok(None) => return refusal("404 Not Found", "not_found", "no such App user"),
        Err(r) => return r,
    };
    let all = v.get("all").and_then(Value::as_bool) == Some(true);
    let ended = if all {
        match profile::sign_out_others(&p.id, &app, "") {
            Ok(n) => n,
            Err(e) => return CliResponse::internal_error(e),
        }
    } else {
        let Some(id) = text(&v, &["id"]) else { return usage("send {id} or {all: true}") };
        match revoke_by_wire_id(&p.id, &app, id, true) {
            Ok(true) => 1,
            Ok(false) => return refusal("404 Not Found", "not_found", "no live session of that person in that App by that id"),
            Err(e) => return CliResponse::internal_error(e),
        }
    };
    audit("app_user_session_revoked", actor, &format!("{} app={app} ended={ended} by the owner", p.username));
    CliResponse::ok_json(json!({ "ok": true, "username": p.username, "app": app, "ended": ended }).to_string())
}

// ── per-App settings (owner; the App's agent lowers) ────────────────

fn workspace_path(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![project_id], |r| r.get::<_, String>(0)).ok()
}

fn app_of_workspace(app: &str, ws: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    matches!(k2_core::published_services::get_by_id(&conn, app), Ok(Some(svc)) if svc.project_id == ws)
}

fn app_users_off() -> CliResponse {
    refusal(
        "403 Forbidden",
        APP_USERS_OFF,
        "changing your Apps' profile settings needs \"manage its Apps' users\" for this agent (off). Ask your owner to turn it on; with permission cards on, run the Ask with: line below.",
    )
}

/// Who may write: `Ok(true)` = an agent (lower only), `Ok(false)` = a
/// person (owner, Admin, a K2 shell tab), `Err` = refused. U16: an agent
/// only for an App of its own workspace, never `host`, and only with the
/// App-user switch on.
fn settings_writer(who: &crate::compute_routes::Who, app: &str) -> Result<bool, CliResponse> {
    use crate::compute_routes::Who;
    use k2_core::connect_users::Role;
    match who {
        Who::Owner | Who::Shell { .. } | Who::Login { role: Role::Owner | Role::Admin, .. } => Ok(false),
        Who::Agent { project_id, .. } => {
            if app == skin::HOST_APP_ID || !app_of_workspace(app, project_id) {
                return Err(refusal("403 Forbidden", "outside_workspace", "an agent sets profile settings only for its own workspace's Apps (never the box login)"));
            }
            let path = workspace_path(project_id).unwrap_or_default();
            if path.is_empty() || !crate::skin_routes::agent_may_manage_app_users(&path) {
                return Err(app_users_off());
            }
            Ok(true)
        }
        Who::Login { .. } => Err(refusal("403 Forbidden", "owner_only", "the owner or an Admin sets an App's profile settings")),
        Who::Wrong | Who::Invalid => Err(refusal("401 Unauthorized", "unauthorized", "invalid or missing auth token")),
    }
}

fn bool_field(v: &Value, key: &str) -> Result<Option<bool>, CliResponse> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(Value::String(s)) if matches!(s.as_str(), "on" | "true") => Ok(Some(true)),
        Some(Value::String(s)) if matches!(s.as_str(), "off" | "false") => Ok(Some(false)),
        Some(_) => Err(usage(&format!("{key} must be true/false (on/off)"))),
    }
}

/// `GET|POST /cli/app/profile-settings`.
pub fn handle_settings(who: &crate::compute_routes::Who, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if !is_post {
        let app = match resolve_app(params.get("app").map(String::as_str)) {
            Ok(a) => a,
            Err(r) => return r,
        };
        if let Err(r) = settings_writer(who, &app) {
            return r;
        }
        return match profile::settings(&app) {
            Ok(s) => CliResponse::ok_json(json!({ "ok": true, "settings": s }).to_string()),
            Err(e) => CliResponse::internal_error(e),
        };
    }
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let app = match resolve_app(text(&v, &["app", "appId"])) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let agent = match settings_writer(who, &app) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Some(k) = v.as_object().and_then(|o| o.keys().find(|k| !matches!(k.as_str(), "app" | "appId" | "editNames" | "editImage"))) {
        return usage(&format!("unknown setting {k} (editNames, editImage)"));
    }
    let patch = match (bool_field(&v, "editNames"), bool_field(&v, "editImage")) {
        (Ok(n), Ok(i)) => profile::ProfileSettingsPatch { edit_names: n, edit_image: i },
        (Err(r), _) | (_, Err(r)) => return r,
    };
    if patch == profile::ProfileSettingsPatch::default() {
        return usage("send editNames and/or editImage");
    }
    let actor = who.label();
    match profile::set_settings(&app, &patch, &actor, agent) {
        Ok(s) => {
            audit("app_profile_settings", &actor, &format!("app={app} editNames={} editImage={}", s.edit_names, s.edit_image));
            CliResponse::ok_json(json!({ "ok": true, "settings": s }).to_string())
        }
        Err(profile::SettingsError::NotLower(field)) => CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: json!({ "ok": false, "error": { "code": "owner_only", "field": field, "hint": "an agent may only turn a profile setting off; the owner turns it on" } }).to_string(),
        },
        Err(profile::SettingsError::Store(e)) => CliResponse::internal_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn handles_are_opaque_and_stable() {
        let a = session_handle("tok-1");
        assert!(a.starts_with(SESSION_HANDLE_PREFIX) && a.len() == SESSION_HANDLE_PREFIX.len() + 32, "{a}");
        assert_eq!(a, session_handle("tok-1"));
        assert_ne!(a, session_handle("tok-2"));
        assert!(!a.contains("tok-1"));
    }
}
