//! `/v1/apps/<service id>/…`: the door an App-scoped key opens (vs-live 2
//! §6, S45; the 0.46.0 grace release). The App's BFF calls these instead
//! of `/cli/skin/*` with the owner token.
//!
//! ```text
//! GET  /v1/apps/<id>/whoami                      the key: App, scopes, ceiling
//! GET  /v1/apps/<id>/users                       users          this App's users
//! POST /v1/apps/<id>/users       {username, email?, password?, rooms?, role?}   users (+ rooms for rooms/role)
//! POST /v1/apps/<id>/users/rooms {username, rooms, applyTokens?}                rooms
//! POST /v1/apps/<id>/users/disable|enable|remove|unlock {username}              users
//! POST /v1/apps/<id>/users/password-reset {username}                            users
//! POST /v1/apps/<id>/fetch       {url, secret|secrets, method?, headers?, bodyB64?, maxBytes?, timeoutSecs?}  fetch
//! POST /v1/apps/<id>/push/send   {to, title, body, data?, topic?, …}             push (prd-custom-app-push-v1)
//! GET  /v1/apps/<id>/push/status?sendId=                                         push
//! ```
//!
//! Rules:
//! - Only an App key opens these, and only for its own App: no key, an
//!   ordinary key, the owner token, a revoked key, a key of another App or
//!   a scope the key lacks all get the same `404` (no oracle). These routes
//!   are open whether or not the `/v1` surface is (Apps run on every box).
//! - A user is "this App's" when it holds a `kind=app` grant on it. Rooms,
//!   disable/enable and remove act on that grant only. A password reset and
//!   removing the person outright need the person to belong to this App
//!   alone (no box login, no other App), so a key can never take over
//!   someone else's account.
//! - Rooms and roles stay inside the key's ceiling. Never host grants,
//!   roles, templates or other Apps.
//! - `fetch` resolves a secret granted to the App's service, by name, and
//!   makes the call from the daemon: the App never holds the value; echoes
//!   come back redacted (`k2 vault fetch`'s engine, host-bound).
//! - Every call writes one `auth-audit.jsonl` line: `app-key:<id>`, the
//!   App, the verb, the outcome.

use std::collections::HashMap;
use std::time::Duration;

use k2_core::app_grace::keys::{self, AppKeyGrant};
use k2_core::skin;
use serde_json::{json, Value as J};

use crate::cli_response::CliResponse;

pub const PREFIX: &str = "/v1/apps/";
pub const MAX_BODY: usize = 96 * 1024;

/// Is `path` one of these routes (`/v1/apps/<id>/<verb…>`)?
pub fn is_route(path: &str) -> bool {
    split(path).is_some()
}

/// `(service id, verb)` from `/v1/apps/<id>/<verb…>`.
fn split(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix(PREFIX)?;
    let (id, verb) = rest.split_once('/')?;
    if id.is_empty() || verb.is_empty() || id.contains("..") {
        return None;
    }
    Some((id, verb))
}

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        409 => "409 Conflict",
        413 => "413 Payload Too Large",
        429 => "429 Too Many Requests",
        502 => "502 Bad Gateway",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}

fn err(status: u16, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status: status_line(status),
        content_type: "application/json",
        body: json!({ "error": { "code": code, "hint": hint.into() } }).to_string(),
    }
}

/// The uniform answer for anything a key may not see.
pub fn not_found() -> CliResponse {
    err(404, "not_found", "not found")
}

pub fn too_large() -> CliResponse {
    err(413, "too_large", format!("request bodies here are at most {} KiB", MAX_BODY / 1024))
}

fn ok(v: J) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

/// The scope each verb needs (`None` = no such verb).
fn scope_for(method_post: bool, verb: &str) -> Option<&'static str> {
    match (method_post, verb) {
        (false, "whoami") => Some(""),
        (false, "users") => Some(keys::SCOPE_USERS),
        (true, "users") => Some(keys::SCOPE_USERS),
        (true, "users/rooms") => Some(keys::SCOPE_ROOMS),
        (true, "users/disable" | "users/enable" | "users/remove" | "users/unlock" | "users/password-reset") => Some(keys::SCOPE_USERS),
        (true, "fetch") => Some(keys::SCOPE_FETCH),
        (true, "push/send") | (false, "push/status") => Some(keys::SCOPE_PUSH),
        _ => None,
    }
}

/// Is `(method, path)` a POST these routes take (the top-level method
/// guard's allowlist)?
pub fn post_allowed(path: &str) -> bool {
    split(path).is_some_and(|(_, verb)| scope_for(true, verb).is_some())
}

struct Ctx {
    key_id: String,
    app: String,
    workspace: String,
    grant: AppKeyGrant,
}

impl Ctx {
    fn actor(&self) -> String {
        format!("app-key:{}", self.key_id)
    }
}

fn audit(c: &Ctx, verb: &str, outcome: &str) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        &format!("app-key-{}", verb.replace('/', "-")),
        &c.actor(),
        format!("{outcome} app={}", c.app),
        "app",
        "-",
        "apps",
    ));
}

/// Handle one call. `presented` is the Bearer (or `?token=`) credential.
/// Blocking (fetch makes an outbound call): run it on a blocking task.
pub fn handle(path: &str, is_post: bool, presented: Option<&str>, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    let Some((app, verb)) = split(path) else { return not_found() };
    let Some(scope) = scope_for(is_post, verb) else {
        // A GET to a POST verb (or the reverse) is a 405 only for a key
        // that may see this App at all.
        return if presented.and_then(k2_core::api_keys::resolve_app_key).is_some_and(|k| k.service_id == app)
            && (scope_for(!is_post, verb).is_some())
        {
            err(405, "method_not_allowed", "wrong method for this route")
        } else {
            not_found()
        };
    };
    let Some(key) = presented.and_then(k2_core::api_keys::resolve_app_key) else { return not_found() };
    if key.service_id != app {
        return not_found();
    }
    let grant = AppKeyGrant::from_stored(key.grant_json.as_deref());
    if !scope.is_empty() && !grant.has(scope) {
        return not_found();
    }
    let workspace = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match k2_core::published_services::get_by_id(&conn, app) {
            Ok(Some(row)) => row.project_id,
            // The App was removed: its keys open nothing.
            _ => return not_found(),
        }
    };
    let c = Ctx { key_id: key.id.clone(), app: app.to_string(), workspace, grant };
    // App push keeps its own answers (202, sendId, the push refusal codes).
    if verb == "push/send" || verb == "push/status" {
        let r = push(&c, verb, body, params);
        audit(&c, verb, if r.status.starts_with('2') { "ok" } else { r.status });
        return r;
    }
    let body_v: J = if is_post {
        if body.is_empty() {
            json!({})
        } else {
            match serde_json::from_slice(body) {
                Ok(v @ J::Object(_)) => v,
                _ => return err(400, "bad_json", "the body is a JSON object"),
            }
        }
    } else {
        J::Null
    };
    let r = match (is_post, verb) {
        (false, "whoami") => whoami(&c),
        (false, "users") => list_users(&c),
        (true, "users") => create_user(&c, &body_v),
        (true, "users/rooms") => set_rooms(&c, &body_v),
        (true, "users/disable") => set_enabled(&c, &body_v, false),
        (true, "users/enable") => set_enabled(&c, &body_v, true),
        (true, "users/remove") => remove_user(&c, &body_v),
        (true, "users/unlock") => unlock(&c, &body_v),
        (true, "users/password-reset") => password_reset(&c, &body_v),
        (true, "fetch") => fetch(&c, &body_v),
        _ => Err(not_found()),
    };
    match r {
        Ok(v) => {
            if verb != "whoami" && !(verb == "users" && !is_post) {
                audit(&c, verb, "ok");
            }
            ok(v)
        }
        Err(resp) => {
            audit(&c, verb, &format!("refused {}", resp.status));
            resp
        }
    }
}

type R = Result<J, CliResponse>;

/// `/v1/apps/<id>/push/*` (scope `push`): the same rules as the link's
/// `push.send` (the App comes from the key, never the body).
fn push(c: &Ctx, verb: &str, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    use crate::app_push::send;
    if verb == "push/status" {
        let Some(id) = params.get("sendId").map(|s| s.trim()).filter(|s| !s.is_empty()) else {
            return err(400, "usage", "send ?sendId= (from push/send)");
        };
        return match crate::app_push::sendlog::get(&c.app, id) {
            Some(r) => {
                let mut v = send::record_json(&r);
                v["ok"] = json!(true);
                ok(v)
            }
            None => err(404, "not_found", "no send of this App by that id in the last hour"),
        };
    }
    let (to, n) = match send::parse_request(body) {
        Ok(x) => x,
        Err(e) => return send::err_response(&e),
    };
    let caller = send::Caller { actor: c.actor(), kind: crate::app_push::payload::KIND_APP };
    match send::send(&c.app, &caller, &to, n) {
        Ok(v) => CliResponse { status: "202 Accepted", content_type: "application/json", body: v.to_string() },
        Err(e) => send::err_response(&e),
    }
}

fn whoami(c: &Ctx) -> R {
    Ok(json!({
        "app": c.app,
        "keyId": c.key_id,
        "scopes": c.grant.scopes,
        "rooms": c.grant.rooms,
        "roomHandles": skin::handles_for_project_ids(&c.grant.rooms),
        "roleIds": c.grant.roles,
    }))
}

fn field<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    v.get(k).and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn username(v: &J) -> Result<String, CliResponse> {
    let u = field(v, "username").ok_or_else(|| err(400, "username_required", "username is required"))?;
    skin::normalize_username(u).map_err(|e| err(400, "bad_username", e))
}

/// This App's grant for `username` (and the principal id), or 404.
fn app_grant(c: &Ctx, username: &str) -> Result<(String, skin::SkinGrant), CliResponse> {
    let principals = skin::list_principals().map_err(|e| err(500, "store", e))?;
    let p = principals.into_iter().find(|p| p.username == username).ok_or_else(|| err(404, "user_not_found", "no such user on this App"))?;
    let g = skin::list_grants()
        .map_err(|e| err(500, "store", e))?
        .into_iter()
        .find(|g| g.subject_kind == "principal" && g.subject_id == p.id && g.kind == "app" && g.target_id == c.app)
        .ok_or_else(|| err(404, "user_not_found", "no such user on this App"))?;
    Ok((p.id, g))
}

/// Does the person belong to this App alone (no box login, no other App)?
fn only_this_app(c: &Ctx, principal_id: &str) -> Result<bool, CliResponse> {
    let gs = skin::list_grants().map_err(|e| err(500, "store", e))?;
    Ok(gs.iter().filter(|g| g.subject_kind == "principal" && g.subject_id == principal_id).all(|g| g.kind == "app" && g.target_id == c.app))
}

fn rooms_of(v: &J) -> Option<Vec<String>> {
    match v.get("rooms")? {
        J::Array(a) => Some(a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
        J::String(s) => Some(s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()),
        _ => None,
    }
}

fn resolve_rooms(c: &Ctx, tokens: &[String]) -> Result<Vec<String>, CliResponse> {
    let ids = skin::resolve_room_tokens(tokens).map_err(|e| err(400, "bad_rooms", e))?;
    if !c.grant.allows_rooms(&ids) {
        return Err(err(403, "outside_ceiling", "a room is outside this key's rooms (the owner sets them at mint)"));
    }
    Ok(ids)
}

fn user_json(c: &Ctx, p: &skin::SkinPrincipal, g: &skin::SkinGrant) -> J {
    let rooms = skin::parse_rooms_json(g.scope.as_deref());
    json!({
        "username": p.username,
        "email": p.email,
        "fullName": p.full_name,
        "hasPassword": p.has_password,
        "enabled": g.enabled,
        "roleId": g.role_id,
        "rooms": rooms,
        "roomHandles": skin::handles_for_project_ids(&rooms),
        "app": c.app,
    })
}

fn list_users(c: &Ctx) -> R {
    let principals = skin::list_principals().map_err(|e| err(500, "store", e))?;
    let grants = skin::list_grants().map_err(|e| err(500, "store", e))?;
    let mut out = Vec::new();
    for g in grants.iter().filter(|g| g.subject_kind == "principal" && g.kind == "app" && g.target_id == c.app) {
        if let Some(p) = principals.iter().find(|p| p.id == g.subject_id) {
            out.push(user_json(c, p, g));
        }
    }
    Ok(json!({ "app": c.app, "users": out }))
}

fn create_user(c: &Ctx, v: &J) -> R {
    let user = username(v)?;
    let rooms = rooms_of(v);
    let role = field(v, "role");
    if (rooms.is_some() || role.is_some()) && !c.grant.has(keys::SCOPE_ROOMS) {
        return Err(err(403, "scope_required", "setting rooms or a role needs the key's rooms scope"));
    }
    let role_id = match role {
        Some(r) => Some(c.grant.allowed_role(&c.app, r).ok_or_else(|| err(403, "outside_ceiling", "that role is outside this key's ceiling"))?),
        None => None,
    };
    let room_ids = match &rooms {
        Some(t) => Some(resolve_rooms(c, t)?),
        None => None,
    };
    let actor = c.actor();
    // agent-app-users Q3/A12: the key lives in the App's runtime, which
    // that workspace's agent can read, so a person the key makes (or gives
    // a password) is tied to the App's workspace like an agent's.
    let ws = skin::ties::app_workspace(&c.app).ok_or_else(|| err(409, "app_gone", "this App's service is gone"))?;
    let by = skin::AddedBy { app_key_workspace: Some(&ws), ..Default::default() };
    let p = skin::add_principal_for_app_by(&user, field(v, "email"), &c.app, role_id.as_deref(), &actor, by).map_err(|e| {
        if e.contains("already exists") {
            err(409, "user_exists", e)
        } else {
            err(400, "bad_request", e)
        }
    })?;
    // Round-2 review B1: the key holder may set this guest's password, so
    // it is tied (not "a person" for card answers) until an owner reset:
    // written above, in the same transaction as the guest (`app_key`).
    if let Some(ids) = room_ids.filter(|_| role_id.is_none()) {
        skin::set_app_grant_rooms(&user, &c.app, &ids, true, &actor).map_err(|e| err(400, "bad_request", e))?;
    }
    if let Some(pw) = field(v, "password") {
        let tie = skin::ties::TieWrite { workspace_id: &ws, set_by: &actor, reason: skin::ties::REASON_APP_KEY };
        skin::set_principal_password_tied(&user, Some(pw), Some(tie)).map_err(|e| err(400, "bad_password", e))?;
    }
    let (_, g) = app_grant(c, &p.username)?;
    let p = skin::list_principals().map_err(|e| err(500, "store", e))?.into_iter().find(|x| x.id == p.id).unwrap_or(p);
    Ok(json!({ "ok": true, "user": user_json(c, &p, &g) }))
}

fn set_rooms(c: &Ctx, v: &J) -> R {
    let user = username(v)?;
    let tokens = rooms_of(v).ok_or_else(|| err(400, "rooms_required", "rooms is required ([] takes every room away)"))?;
    let ids = resolve_rooms(c, &tokens)?;
    let _ = app_grant(c, &user)?;
    let apply = v.get("applyTokens").and_then(J::as_bool).unwrap_or(true);
    let g = skin::set_app_grant_rooms(&user, &c.app, &ids, apply, &c.actor()).map_err(|e| err(409, "not_settable", e))?;
    Ok(json!({ "ok": true, "username": user, "app": c.app, "rooms": ids, "roomHandles": skin::handles_for_project_ids(&ids), "grant": g }))
}

fn set_enabled(c: &Ctx, v: &J, on: bool) -> R {
    let user = username(v)?;
    let (_, g) = app_grant(c, &user)?;
    let g = skin::set_grant_enabled(&g.id, on).map_err(|e| err(500, "store", e))?;
    Ok(json!({ "ok": true, "username": user, "enabled": g.enabled }))
}

fn remove_user(c: &Ctx, v: &J) -> R {
    let user = username(v)?;
    let (pid, g) = app_grant(c, &user)?;
    if only_this_app(c, &pid)? {
        let removed = skin::remove_principal(&user).map_err(|e| err(500, "store", e))?;
        if removed {
            // app-read-state-v1: the guest's read state goes with them.
            crate::read_routes::forget_guest(&pid);
        }
        return Ok(json!({ "ok": true, "username": user, "removed": "person", "done": removed }));
    }
    let done = skin::delete_grant_by_id(&g.id).map_err(|e| err(500, "store", e))?;
    Ok(json!({ "ok": true, "username": user, "removed": "app_access", "done": done }))
}

fn unlock(c: &Ctx, v: &J) -> R {
    let user = username(v)?;
    let _ = app_grant(c, &user)?;
    let (_, cleared) = skin::clear_login_lockout(&user).map_err(|e| err(500, "store", e))?;
    Ok(json!({ "ok": true, "username": user, "cleared": cleared }))
}

fn password_reset(c: &Ctx, v: &J) -> R {
    let user = username(v)?;
    let (pid, _) = app_grant(c, &user)?;
    if !only_this_app(c, &pid)? {
        return Err(err(403, "not_only_this_app", "this person also signs in elsewhere on this box; only the owner resets their password"));
    }
    // Security review 2026-10-25, item 4: the key never sees the token.
    // The person is tied to the App's workspace (`app_key`) and the link is
    // mailed to their own email; using it ties them too.
    crate::app_user_invites::app_key_reset(&c.app, &user, &c.actor()).map_err(|(status, code, hint)| err(status, code, hint))
}

// ── fetch: outside APIs with secrets by name ──────────────────────────

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct FetchBody {
    url: Option<String>,
    method: Option<String>,
    secret: Option<String>,
    secrets: Vec<String>,
    /// `[["Name", "value"], …]`.
    headers: Vec<(String, String)>,
    body_b64: Option<String>,
    max_bytes: usize,
    timeout_secs: u64,
    include_headers: bool,
}

/// The runtime App behind a published service when its egress list is
/// enforced (a walled engine: container or VM). A `--cmd` service, or a
/// preview App on the unwalled process engine, has no enforced list (its
/// raw traffic isn't walled either); the secret's host binding still
/// holds.
fn walled_app_of(service_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let app = k2_core::deploy::store::get_app_by_service(&conn, service_id).ok().flatten()?;
    matches!(app.isolation.as_str(), "container" | "vm").then_some(app.app_id)
}

fn fetch(c: &Ctx, v: &J) -> R {
    use base64::Engine as _;
    use k2_vault::hosts::{Origin, OriginPattern};
    use k2_vault::{Requester, Selector, SecretStore, Use};
    let b: FetchBody = serde_json::from_value(v.clone()).map_err(|e| err(400, "bad_json", format!("fetch body: {e}")))?;
    let url = b.url.clone().ok_or_else(|| err(400, "url_required", "give the URL to call"))?;
    let (_, origin): (url::Url, Origin) = Origin::parse_url(&url).map_err(|e| err(400, &e.code, e.hint))?;
    // One host policy for both ways out (S15): a walled App's key-proxy
    // call passes its egress allowlist first, with the same own-name,
    // local-name and first-seen rules and counters as its egress proxy.
    // The secret's own host binding still applies after it.
    if let Some(app_id) = walled_app_of(&c.app) {
        crate::apps::egress_proxy::check_for_key_proxy(&app_id, &origin.host_str(), origin.port).map_err(|r| err(r.status, r.code, r.hint))?;
    }
    let mut names = b.secrets.clone();
    if let Some(n) = b.secret.as_deref().filter(|n| !n.is_empty()) {
        names.push(n.to_string());
    }
    if names.is_empty() {
        return Err(err(400, "secret_required", "name the secret granted to this App (secret)"));
    }
    let vault = crate::vault::get().map_err(|e| err(503, "vault_closed", format!("the vault isn't open: {}", e.hint)))?;
    let who = Requester::PublishedService { id: c.app.clone(), workspace_id: c.workspace.clone() };
    // A name means a secret granted to this service: look through its
    // grants (never another scope's secret of the same name).
    let granted = vault
        .grants_to(&k2_vault::Consumer::published_service(c.app.clone()))
        .map_err(|e| err(e.status, &e.code, e.hint))?;
    let mut injections = Vec::new();
    for n in &names {
        let (g, _) = granted
            .iter()
            .find(|(_, name)| name == n)
            .ok_or_else(|| err(403, "not_granted", format!("{n} isn't granted to this App (the owner: k2 publish secret add <app> {n} --as fetch)")))?;
        let r = vault.resolve(&Selector::Id(g.secret_id.clone()), Use::Fetch(&origin), &who).map_err(|e| err(e.status, &e.code, e.hint))?;
        let (dst, fmt) = r.inject.choose(None, None).map_err(|e| err(e.status, &e.code, e.hint))?;
        let allowed: Vec<OriginPattern> = k2_vault::hosts::parse_stored(&r.meta.hosts);
        injections.push(crate::vault_fetch::Injection { name: r.meta.name.clone(), value: r.value, dst, fmt, allowed, sign: r.inject.sign.clone() });
    }
    let req_body = match b.body_b64.as_deref() {
        Some(s) => Some(base64::engine::general_purpose::STANDARD.decode(s.trim()).map_err(|_| err(400, "bad_body", "bodyB64 isn't base64"))?),
        None => None,
    };
    let spec = crate::vault_fetch::FetchSpec {
        method: b.method.clone().unwrap_or_else(|| if req_body.is_some() { "POST".into() } else { "GET".into() }),
        url,
        headers: b.headers.clone(),
        body: req_body,
        max_bytes: b.max_bytes,
        timeout: Duration::from_secs(b.timeout_secs),
        include_headers: b.include_headers,
        purpose: crate::egress::Purpose::AppFetch,
    };
    let redactor = vault.redactor();
    let allow = crate::vault_fetch::Allowlist::from_injections(&injections);
    let res = crate::vault_fetch::execute(spec, &allow, injections, &redactor).map_err(|e| err(e.status, &e.code, e.hint))?;
    let headers: Vec<J> = res.headers.iter().map(|(k, v)| json!([k, v])).collect();
    let (text, b64) = match String::from_utf8(res.body.clone()) {
        Ok(t) => (Some(t), None),
        Err(_) => (None, Some(base64::engine::general_purpose::STANDARD.encode(&res.body))),
    };
    Ok(json!({
        "status": res.status,
        "headers": headers,
        "truncated": res.truncated,
        "redirected": res.redirected,
        "body": text,
        "bodyB64": b64,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_split_and_scopes_map() {
        assert_eq!(split("/v1/apps/svc-1/users/rooms"), Some(("svc-1", "users/rooms")));
        assert_eq!(split("/v1/apps/svc-1"), None);
        assert_eq!(split("/v1/apps//users"), None);
        assert_eq!(split("/v1/apps/../users"), None);
        assert_eq!(scope_for(true, "users/rooms"), Some(keys::SCOPE_ROOMS));
        assert_eq!(scope_for(true, "fetch"), Some(keys::SCOPE_FETCH));
        assert_eq!(scope_for(false, "fetch"), None);
        assert_eq!(scope_for(true, "roles"), None, "never roles");
        assert!(post_allowed("/v1/apps/x/users/remove") && !post_allowed("/v1/apps/x/whoami"));
    }

    #[test]
    fn anything_without_a_live_app_key_is_the_same_404() {
        let body = b"{}";
        for (path, post, cred) in [
            ("/v1/apps/svc-1/users", false, None),
            ("/v1/apps/svc-1/users", false, Some("k2sk_not_a_real_key")),
            ("/v1/apps/svc-1/whoami", false, Some("")),
            ("/v1/apps/svc-1/roles", true, Some("k2sk_x")),
        ] {
            let r = handle(path, post, cred, body, &HashMap::new());
            assert_eq!((r.status, r.body.as_str()), (not_found().status, not_found().body.as_str()), "{path}");
        }
    }
}
