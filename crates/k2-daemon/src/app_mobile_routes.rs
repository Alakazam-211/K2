//! App mobile access (app-mobile-access-v1, 0.46.3;
//! `research-app-mobile-surface-parity-v1` §5 C, E, F).
//!
//! Guest routes (an App pass):
//! - `POST /cli/skin/refresh` (Public, `guests.refresh`): a device's
//!   refresh token → a new pass + the next refresh token (one-time,
//!   rotating; reuse ends that device's sign-in).
//! - `GET /cli/skin/devices` (`guests.devices`): this guest's devices that
//!   stay signed in to this App.
//! - `POST /cli/skin/devices/revoke {id}` (`guests.revokeDevice`).
//!
//! Discovery (F, Rosson 2026-10-20: per App, off by default, as discrete
//! as possible):
//! - `GET /cli/skin/app-info?app=<id>` (Public, `app.info`) answers only
//!   for an App the owner (or the App's agent with "…manage them end to
//!   end") listed, and only with that App's name, kind and how to sign in.
//!   Anything else (no App listed, another App, no such App, no `app`)
//!   answers exactly like a route that doesn't exist.
//!
//! Owner routes (Settings → Apps → *App* → People, `k2 app people`):
//! - `GET /cli/skin/access?app=` the person × agent grid, the App's live
//!   device sign-ins and its sign-in settings;
//! - `POST /cli/skin/access/settings {app, staySignedInDays}`;
//! - `POST /cli/skin/access/listed {app, listed}` (also the App's agent
//!   with switch 2 on its own App; audited, with an inbox note);
//! - `POST /cli/skin/access/devices/revoke {id}`.
//!
//! Suggested role templates (C, Rosson 2026-10-20): never created on their
//! own. `GET /cli/skin/roles/suggest` lists them; `POST
//! /cli/skin/roles/from-template {template, app, name?, rooms?}` makes one
//! (`k2 app roles suggest`, `k2 app roles add --from <template>`).

use std::collections::HashMap;

use k2_core::skin::{self, access, app_login, SkinPass};
use serde_json::{json, Value};

use crate::cli_response::CliResponse;

pub const REFRESH: &str = "/cli/skin/refresh";
pub const DEVICES: &str = "/cli/skin/devices";
pub const DEVICES_REVOKE: &str = "/cli/skin/devices/revoke";
pub const APP_INFO: &str = "/cli/skin/app-info";
pub const ACCESS: &str = "/cli/skin/access";
pub const ACCESS_SETTINGS: &str = "/cli/skin/access/settings";
pub const ACCESS_LISTED: &str = "/cli/skin/access/listed";
pub const ACCESS_DEVICE_REVOKE: &str = "/cli/skin/access/devices/revoke";
pub const ROLES_SUGGEST: &str = "/cli/skin/roles/suggest";
pub const ROLES_FROM_TEMPLATE: &str = "/cli/skin/roles/from-template";

/// The routes a guest's pass calls (the dispatcher resolves the pass).
pub const PASS_ROUTES: &[&str] = &[DEVICES, DEVICES_REVOKE];

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

fn json_body(body: &[u8]) -> Result<Value, CliResponse> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(json!({}));
    }
    serde_json::from_slice(body).map_err(|_| usage("the body must be JSON"))
}

fn text<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str)).map(str::trim).filter(|s| !s.is_empty())
}

// ── device sign-in ───────────────────────────────────────────────────

/// The device-session answer (login with a device, refresh).
pub fn device_session_json(d: &app_login::DeviceSession, principal_id: &str) -> Value {
    json!({
        "ok": true,
        "token": d.session.raw,
        "refresh": d.refresh,
        "passExpiresAt": d.pass_expires_at,
        "refreshExpiresAt": d.refresh_expires_at,
        "username": d.session.meta.name,
        "principalId": principal_id,
        "app": d.session.app,
        "device": { "id": d.device.id, "name": d.device.name },
    })
}

/// `POST /cli/skin/refresh {refresh, app?}` (Public).
pub fn handle_refresh(body: &[u8]) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(raw) = text(&v, &["refresh"]) else {
        return usage("send {refresh} (the k2skr_ token from the last login or refresh)");
    };
    match app_login::refresh(raw, text(&v, &["app", "appId"])) {
        Ok(app_login::Refresh::Ok(d)) => {
            skin::profile::record_session(&d.session.meta.id, skin::profile::VIA_REFRESH, Some(&d.device.name));
            let principal_id = skin::resolve_skin_token(&d.session.raw).and_then(|p| p.principal_id).unwrap_or_default();
            CliResponse::ok_json(device_session_json(&d, &principal_id).to_string())
        }
        Ok(app_login::Refresh::Invalid) => refusal(
            "401 Unauthorized",
            "refresh_invalid",
            "that refresh token is unknown, ended, expired or for another App: sign in again",
        ),
        Ok(app_login::Refresh::Reused) => refusal(
            "401 Unauthorized",
            "refresh_reused",
            "that refresh token was already used, so this device was signed out: sign in again",
        ),
        Err(e) => CliResponse::internal_error(e),
    }
}

fn guest_required() -> CliResponse {
    refusal("403 Forbidden", "guest_required", "only a guest's own login pass has devices")
}

/// `GET /cli/skin/devices` for a resolved pass.
pub fn handle_devices(pass: &SkinPass) -> CliResponse {
    let Some(pid) = pass.principal_id.as_deref().filter(|_| pass.session) else {
        return guest_required();
    };
    match app_login::devices_for(pid, &pass.app) {
        Ok(list) => {
            let devices: Vec<Value> = list
                .iter()
                .map(|d| {
                    json!({
                        "id": d.id,
                        "deviceId": d.device_id,
                        "name": d.name,
                        "createdAt": d.created_at,
                        "lastUsedAt": d.last_used_at,
                        "expiresAt": d.expires_at,
                        "current": d.session_token_id.as_deref() == Some(pass.id.as_str()),
                        // App phone pairing: how it started (pairing | password).
                        "via": d.via,
                    })
                })
                .collect();
            CliResponse::ok_json(json!({ "ok": true, "devices": devices }).to_string())
        }
        Err(e) => CliResponse::internal_error(e),
    }
}

/// `POST /cli/skin/devices/revoke {id}` for a resolved pass.
pub fn handle_device_revoke(pass: &SkinPass, body: &[u8]) -> CliResponse {
    let Some(pid) = pass.principal_id.as_deref().filter(|_| pass.session) else {
        return guest_required();
    };
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = text(&v, &["id"]) else {
        return usage("send {id} (from guests.devices)");
    };
    match app_login::revoke_own(pid, &pass.app, id) {
        Ok(true) => CliResponse::ok_json(json!({ "ok": true }).to_string()),
        Ok(false) => refusal("404 Not Found", "not_found", "no device of yours by that id"),
        Err(e) => CliResponse::internal_error(e),
    }
}

/// The pass routes (the dispatcher resolved `pass`).
pub fn handle_pass(path: &str, pass: &SkinPass, is_post: bool, body: &[u8]) -> CliResponse {
    match (path, is_post) {
        (DEVICES, false) => handle_devices(pass),
        (DEVICES_REVOKE, true) => handle_device_revoke(pass, body),
        _ => CliResponse::method_not_allowed(),
    }
}

/// A credential that isn't a live pass on a pass route.
pub fn pass_required() -> CliResponse {
    refusal("403 Forbidden", "skin_token", "this route is for a guest's own login pass (k2skn_)")
}

// ── discovery ────────────────────────────────────────────────────────

fn published(app: &str) -> Option<k2_core::published_services::PublishedService> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::get_by_id(&conn, app).ok().flatten()
}

/// `GET /cli/skin/app-info?app=<id>`: `Some(answer)` only for a listed
/// App; `None` = answer exactly like a `/cli/skin/…` route that doesn't
/// exist ([`CliResponse::not_found`]).
pub fn app_info(params: &HashMap<String, String>) -> Option<CliResponse> {
    let app = params.get("app").map(|s| s.trim()).filter(|s| !s.is_empty())?;
    if app == skin::HOST_APP_ID {
        return None;
    }
    let settings = app_login::settings(app).ok()?;
    if !settings.listed {
        return None;
    }
    let row = published(app)?;
    let url = crate::publish_runtime::to_json(&row).url;
    Some(CliResponse::ok_json(
        json!({
            "app": row.id,
            "name": row.name,
            "kind": row.kind,
            "signIn": {
                "methods": ["password"],
                "staysSignedIn": settings.stay_signed_in_days > 0,
                "url": url,
            },
        })
        .to_string(),
    ))
}

// ── owner: the People grid and settings ──────────────────────────────

fn resolve_app(raw: Option<&str>) -> Result<String, CliResponse> {
    let Some(raw) = raw else { return Err(usage("name the App (app=<service id>, <workspace>/<name> or host)")) };
    crate::skin_routes::resolve_app_target(raw).map_err(|e| usage(&e))
}

/// `GET /cli/skin/access?app=` (owner).
pub fn handle_access_get(params: &HashMap<String, String>) -> CliResponse {
    let app = match resolve_app(params.get("app").map(String::as_str)) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let grid = match access::grid(&app) {
        Ok(g) => g,
        Err(e) => return CliResponse::internal_error(e),
    };
    let devices = match app_login::devices_for_app(&app) {
        Ok(d) => d,
        Err(e) => return CliResponse::internal_error(e),
    };
    let settings = match app_login::settings(&app) {
        Ok(s) => s,
        Err(e) => return CliResponse::internal_error(e),
    };
    let user = params.get("user").or_else(|| params.get("username")).map(|s| s.trim().to_ascii_lowercase());
    let mut grid = serde_json::to_value(&grid).unwrap_or_default();
    if let Some(u) = user.filter(|u| !u.is_empty()) {
        let keep: Vec<Value> = grid["people"].as_array().cloned().unwrap_or_default().into_iter().filter(|p| p["username"] == u.as_str()).collect();
        if keep.is_empty() {
            return refusal("404 Not Found", "not_found", "that person holds no grant on this App");
        }
        grid["people"] = Value::Array(keep);
    }
    CliResponse::ok_json(
        json!({
            "ok": true,
            "app": grid["app"],
            "agents": grid["agents"],
            "people": grid["people"],
            "devices": devices,
            "settings": settings,
        })
        .to_string(),
    )
}

/// `POST /cli/skin/access/settings {app, staySignedInDays}` (owner).
pub fn handle_access_settings(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let app = match resolve_app(text(&v, &["app", "appId"])) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let Some(days) = v.get("staySignedInDays").and_then(Value::as_i64) else {
        return usage(&format!("send staySignedInDays: 0 (off) or 1 to {}", app_login::MAX_STAY_DAYS));
    };
    match app_login::set_settings(&app, None, Some(days), actor) {
        Ok(s) => {
            record_audit("app_stay_signed_in", actor, &format!("app={app} days={days}"));
            CliResponse::ok_json(json!({ "ok": true, "settings": s }).to_string())
        }
        Err(e) => usage(&e),
    }
}

fn record_audit(event: &str, actor: &str, line: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, actor, line, "-", "-", "daemon"));
}

/// May `who` list or unlist `app` for discovery? The owner in person
/// (owner token, Owner login, a K2 shell tab), or the agent of the App's
/// own workspace with switch 2 ("…manage them end to end").
pub fn may_set_listed(who: &crate::compute_routes::Who, app_workspace: &str) -> bool {
    use crate::compute_routes::Who;
    match who {
        Who::Owner | Who::Login { role: k2_core::connect_users::Role::Owner, .. } | Who::Shell { .. } => true,
        Who::Agent { project_id, .. } => {
            project_id == app_workspace && crate::apps::access::level(project_id) == crate::apps::access::Level::Full
        }
        _ => false,
    }
}

/// `POST /cli/skin/access/listed {app, listed}`: discovery on or off for
/// one App. Audited; the App's agent gets an inbox note.
pub fn handle_access_listed(body: &[u8], who: &crate::compute_routes::Who) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(listed) = v.get("listed").and_then(Value::as_bool) else {
        return usage("send {app, listed: true|false}");
    };
    let app = match resolve_app(text(&v, &["app", "appId"])) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if app == skin::HOST_APP_ID {
        return usage("discovery is per published App, never the box login (host)");
    }
    let Some(row) = published(&app) else {
        return usage("unknown app");
    };
    if !may_set_listed(who, &row.project_id) {
        return refusal(
            "403 Forbidden",
            "owner_only",
            "only the owner, or this App's own agent with \"…manage them end to end\" on, lists an App for discovery",
        );
    }
    let actor = who.label();
    let before = app_login::settings(&app).map(|s| s.listed).unwrap_or(false);
    let s = match app_login::set_settings(&app, Some(listed), None, &actor) {
        Ok(s) => s,
        Err(e) => return CliResponse::internal_error(e),
    };
    if before != listed {
        record_audit("app_discovery", &actor, &format!("app={app} name={} listed={listed}", row.name));
        let text = if listed {
            format!(
                "{} is now listed for sign-in discovery (by {actor}): GET /cli/skin/app-info?app={} answers with its name, kind and how to sign in. Nothing else about this server is shown. Turn it off with k2 app discovery {} off.",
                row.name, row.id, row.name
            )
        } else {
            format!("{} is no longer listed for sign-in discovery (by {actor}).", row.name)
        };
        crate::apps::alerts::note(&row.project_id, "App discovery changed", &text);
    }
    CliResponse::ok_json(json!({ "ok": true, "changed": before != listed, "settings": s }).to_string())
}

/// `POST /cli/skin/access/devices/revoke {id}` (owner).
pub fn handle_access_device_revoke(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(id) = text(&v, &["id"]) else {
        return usage("send {id} (a device sign-in id from GET /cli/skin/access)");
    };
    match app_login::revoke_by_owner(id) {
        Ok(true) => {
            record_audit("app_device_revoke", actor, &format!("device_login={id}"));
            CliResponse::ok_json(json!({ "ok": true }).to_string())
        }
        Ok(false) => refusal("404 Not Found", "not_found", "no live device sign-in by that id"),
        Err(e) => CliResponse::internal_error(e),
    }
}

// ── suggested role templates ─────────────────────────────────────────

/// `GET /cli/skin/roles/suggest`.
pub fn handle_roles_suggest() -> CliResponse {
    CliResponse::ok_json(json!({ "ok": true, "templates": access::TEMPLATES }).to_string())
}

/// `POST /cli/skin/roles/from-template {template, app, name?, rooms?}`.
/// Rooms are handles or ids; none = the App's own workspace (a published
/// App), or no room lines (`host`).
pub fn handle_roles_from_template(body: &[u8], actor: &str) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(template) = text(&v, &["template", "from"]) else {
        return usage("send {template, app} (k2 app roles suggest lists the templates)");
    };
    let app = match resolve_app(text(&v, &["app", "appId"]).or(Some(skin::HOST_APP_ID))) {
        Ok(a) => a,
        Err(r) => return r,
    };
    let row = (app != skin::HOST_APP_ID).then(|| published(&app)).flatten();
    let tokens: Vec<String> = v
        .get("rooms")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let rooms = if tokens.is_empty() {
        row.as_ref().map(|r| vec![r.project_id.clone()]).unwrap_or_default()
    } else {
        match skin::resolve_room_tokens(&tokens) {
            Ok(r) => r,
            Err(e) => return usage(&e),
        }
    };
    let app_name = row.as_ref().map(|r| r.name.clone()).unwrap_or_else(|| "box".to_string());
    match access::create_from_template(template, &app, &app_name, text(&v, &["name"]), &rooms) {
        Ok(role) => {
            k2_core::log_debug!("[skin] actor={actor} created role {} from template {template} app={app}", role.name);
            CliResponse::ok_json(json!({ "ok": true, "template": template, "role": role }).to_string())
        }
        Err(e) => usage(&e),
    }
}
