//! The person's and the phone's pairing routes (`prd-app-phone-pairing-v1`
//! §4, CP1–CP21; S2).
//!
//! ```text
//! POST /cli/skin/pair/start   {host?, password?}            a guest's own web session pass   → {pairingId, code, pairUrl, qr, …}
//! GET  /cli/skin/pair/status  ?pairingId=                   the same person on the same App   → {state, expiresInSecs?, device?}
//! POST /cli/skin/pair/cancel  {pairingId}                   the same                          → {cancelled}
//! POST /cli/skin/pair/check   {code, app}                   Public (the code), never consumes → {app, appName, username, fullName?, expiresInSecs}
//! POST /cli/skin/pair         {code, app, device, push?}    Public (the code), one-time       → the device-login answer + via, push
//! ```
//!
//! - **Start** takes a guest's own session pass only (`principal_id` set,
//!   `session=1`), on the main port (the A′ fence admits it as a catalog
//!   `app` row) or the App's link (a guest session of that App, AR50). An
//!   App's own link pass, an App key, a platform token, the owner token or
//!   a Connect login is `guest_required`. The App and the person come from
//!   the pass, never the body.
//! - A pass that is the current pass of a device sign-in can't start
//!   (`pairing_from_device`, CP3): a phone never mints more phones.
//! - The pass must be at most [`store::FRESH_SIGNIN_SECS`] old, or the body
//!   carries `password`, checked on the pass's own username with the
//!   person's lockout (`reauth_required`, `bad_login`, `423 locked`; CP4,
//!   V25).
//! - **Check and redeem** are Public: the one-time code is the credential.
//!   The App is the link's (on a link) or the body's `app` (required on the
//!   main port), and a code of another App is the same `pair_code_invalid`
//!   as an unknown one. A browser-shaped request is refused before the code
//!   is looked at (`pairing_not_from_browser`), except the webview origins
//!   the owner listed for the App (CP20, V11).
//! - The redeem consumes the code whatever the outcome, re-checks it in one
//!   skin.db transaction (`skin::app_pairing::login_device_paired`) and
//!   answers what a device login answers, plus `via: "pairing"` and, when
//!   the body had `push`, the push registration's outcome (a push refusal
//!   never fails the pairing, CP21).
//! - Throttles (the dispatcher): a per-IP bucket and pairing's own global
//!   ceiling (never the App-login door, R4), its own per-App keys on a link,
//!   and a 200 ms delay on a 401.
//! - Never logged: the code, a pass, a refresh token, a push token. Audit
//!   rows carry the pairingId.

use std::collections::HashMap;

use k2_core::skin::{self, app_login, app_pairing as store, SkinPass};
use serde_json::{json, Value};

use super::{off_response, refusal, refusal_with};
use crate::cli_response::CliResponse;

pub const START: &str = "/cli/skin/pair/start";
pub const STATUS: &str = "/cli/skin/pair/status";
pub const CANCEL: &str = "/cli/skin/pair/cancel";
pub const CHECK: &str = "/cli/skin/pair/check";
pub const REDEEM: &str = "/cli/skin/pair";

/// The routes a pass calls (the dispatcher resolves the pass).
pub const PASS_ROUTES: &[&str] = &[START, STATUS, CANCEL];
/// The Public ones (the code is the credential).
pub const PUBLIC_ROUTES: &[&str] = &[CHECK, REDEEM];
/// Every POST route (the dispatcher's method guard).
pub const POST_ROUTES: &[&str] = &[START, CANCEL, CHECK, REDEEM];
/// Request bodies here are at most this many bytes.
pub const MAX_BODY: usize = 16 * 1024;

/// Where a request came in (audit only).
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    /// The App whose gateway link this came through, if any.
    pub link_app: Option<String>,
    /// The client address the dispatcher vouches for, or `-`.
    pub ip: String,
}

impl Ctx {
    fn door(&self) -> &'static str {
        if self.link_app.is_some() {
            "link"
        } else {
            "main"
        }
    }
}

fn usage(hint: &str) -> CliResponse {
    refusal("400 Bad Request", "usage", hint)
}

fn guest_required() -> CliResponse {
    refusal("403 Forbidden", "guest_required", "pairing starts from a person's own web sign-in: send their session pass (Authorization: Bearer k2skn_…)")
}

/// No credential at all on a pass route.
pub fn pass_required() -> CliResponse {
    refusal("401 Unauthorized", "pass_required", "send the person's session pass (Authorization: Bearer k2skn_…)")
}

/// A credential that isn't a pass (the owner token, an API key, a
/// passport) on a pass route.
pub fn not_a_pass() -> CliResponse {
    guest_required()
}

pub fn too_large() -> CliResponse {
    refusal("413 Payload Too Large", "too_large", "request bodies here are at most 16 KiB")
}

fn json_body(body: &[u8]) -> Result<Value, CliResponse> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err(usage("the body must be a JSON object")),
    }
}

fn known_fields(v: &Value, known: &[&str]) -> Result<(), CliResponse> {
    if let Some(k) = v.as_object().and_then(|o| o.keys().find(|k| !known.contains(&k.as_str()))) {
        let shown: String = k.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(32).collect();
        return Err(usage(&format!("unknown field {shown} (the App and the person come from the pass)")));
    }
    Ok(())
}

fn guest(pass: &SkinPass) -> Result<&str, CliResponse> {
    pass.principal_id.as_deref().filter(|_| pass.session).ok_or_else(guest_required)
}

fn answer(r: Result<Value, CliResponse>) -> CliResponse {
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => e,
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

// ── start / status / cancel ──────────────────────────────────────────

/// `POST /cli/skin/pair/start {host?, password?}`.
pub fn handle_start(pass: &SkinPass, body: &[u8], ctx: &Ctx) -> CliResponse {
    let r = start(pass, body, ctx);
    let outcome = match &r {
        Ok(_) => "ok".to_string(),
        Err(e) => error_code(&e.body).unwrap_or_else(|| "failed".into()),
    };
    if !outcome.starts_with("guest_required") {
        let pairing = r.as_ref().ok().and_then(|v| v["pairingId"].as_str().map(str::to_string)).unwrap_or_else(|| "-".into());
        super::audit(
            "app-pair-start",
            &format!("guest:{}", pass.principal_id.as_deref().unwrap_or("-")),
            &outcome,
            &ctx.ip,
            ctx.door(),
            &format!("app={} pairing={pairing}", pass.app),
        );
    }
    answer(r)
}

fn start(pass: &SkinPass, body: &[u8], ctx: &Ctx) -> Result<Value, CliResponse> {
    let pid = guest(pass)?;
    let app = pass.app.as_str();
    if app.trim().is_empty() || app == skin::HOST_APP_ID {
        return Err(off_response(super::Off::Host));
    }
    if ctx.link_app.as_deref().is_some_and(|l| l != app) {
        return Err(guest_required());
    }
    let v = json_body(body)?;
    known_fields(&v, &["host", "password"])?;
    // CP3: a phone (a device sign-in's current pass) never mints phones.
    if app_login::device_for_session(&pass.id).map_err(CliResponse::internal_error)?.is_some() {
        return Err(refusal(
            "403 Forbidden",
            "pairing_from_device",
            "a phone that is signed in can't pair another one: start from the person's web sign-in",
        ));
    }
    let cfg = super::state(app).map_err(off_response)?;
    if app_login::settings(app).map_err(CliResponse::internal_error)?.stay_signed_in_days == 0 {
        return Err(refusal(
            "409 Conflict",
            "device_signin_off",
            "this App's \"Stay signed in\" is off, so a phone can't stay signed in: its owner turns it on (k2 app people)",
        ));
    }
    let all = super::pairing_hosts(app);
    let served = super::served_hosts(&cfg, &all);
    if served.is_empty() {
        return Err(refusal(
            "409 Conflict",
            "no_public_host",
            "this App has no public k2.dev host to pair on (it needs K2 Connect and its own label)",
        ));
    }
    if let Err(why) = super::front_rewrites(app) {
        return Err(refusal("409 Conflict", "pairing_not_served", why.hint()));
    }
    let host = match v.get("host").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
        Some(h) => match store::normalize_host(h).filter(|h| served.contains(h)) {
            Some(h) => h,
            None => {
                let mut extra = serde_json::Map::new();
                extra.insert("hosts".into(), json!(served));
                return Err(refusal_with("400 Bad Request", "host_not_paired", "that host isn't one this App pairs on", extra));
            }
        },
        None => super::default_host(app, &served).ok_or_else(|| refusal("409 Conflict", "no_public_host", "this App has no public host to pair on"))?,
    };
    // CP4: a fresh sign-in, or the password again (on the pass's own
    // username, its principal must match: V25).
    let info = store::session_info(&pass.id)
        .map_err(CliResponse::internal_error)?
        .ok_or_else(|| refusal("401 Unauthorized", "pass_invalid", "the pass is expired or revoked: sign in again"))?;
    if info.principal_id != pid {
        return Err(refusal("401 Unauthorized", "pass_invalid", "the pass is expired or revoked: sign in again"));
    }
    if now() - info.created_at > store::FRESH_SIGNIN_SECS {
        let Some(password) = v.get("password").and_then(Value::as_str).filter(|p| !p.is_empty()) else {
            return Err(refusal(
                "401 Unauthorized",
                "reauth_required",
                "this sign-in is more than 15 minutes old: ask for the password and send it with the start ({password})",
            ));
        };
        match skin::check_and_record_login(&info.username, password) {
            skin::SkinLoginOutcome::Ok(p) if p.id == pid => {}
            skin::SkinLoginOutcome::LockedOut { retry_after_secs } => {
                let mut extra = serde_json::Map::new();
                extra.insert("retryAfterSecs".into(), json!(retry_after_secs));
                return Err(refusal_with("423 Locked", "locked", "too many wrong passwords: try again later", extra));
            }
            _ => return Err(refusal("401 Unauthorized", "bad_login", "that password is wrong")),
        }
    }
    let minted = match store::mint(app, pid, &pass.id, &host) {
        Ok(m) => m,
        Err(store::MintError::RateLimited { retry_after_ms }) => {
            let mut extra = serde_json::Map::new();
            extra.insert("retryAfterMs".into(), json!(retry_after_ms));
            extra.insert("limit".into(), json!(store::STARTS_PER_HOUR));
            return Err(refusal_with("429 Too Many Requests", "rate_limited", "too many pairing codes this hour; try again later", extra));
        }
    };
    let url = format!("https://{host}{}#{}", store::PAIR_PATH, minted.code);
    let rows = k2_core::qr::encode(&url).ok_or_else(|| CliResponse::internal_error("the pairing link is too long for a QR code"))?;
    let qr = json!({ "size": rows.len(), "rows": rows });
    Ok(json!({
        "ok": true,
        "pairingId": minted.pairing_id,
        "code": minted.code,
        "pairUrl": url,
        "qr": qr,
        "expiresAt": minted.expires_at,
        "expiresInSecs": store::CODE_TTL_SECS,
        "host": host,
    }))
}

fn pairing_id(v: Option<&str>) -> Result<&str, CliResponse> {
    v.map(str::trim).filter(|s| store::is_pairing_id(s)).ok_or_else(|| usage("send the pairingId (apr_…) from pairing.start"))
}

/// `GET /cli/skin/pair/status?pairingId=`.
pub fn handle_status(pass: &SkinPass, params: &HashMap<String, String>) -> CliResponse {
    answer((|| {
        let pid = guest(pass)?;
        let id = pairing_id(params.get("pairingId").map(String::as_str))?;
        let Some(st) = store::status(&pass.app, pid, id) else {
            return Err(refusal("404 Not Found", "not_found", "no pairing of yours by that id (it ended more than 15 minutes ago, or isn't yours)"));
        };
        let mut out = json!({ "ok": true, "pairingId": id, "state": st.state.as_str() });
        if let Some(s) = st.expires_in_secs {
            out["expiresInSecs"] = json!(s);
        }
        if let store::State::Paired { device_name, device_login_id, paired_at } = &st.state {
            out["device"] = json!({ "name": device_name, "deviceLoginId": device_login_id, "pairedAt": paired_at });
        }
        Ok(out)
    })())
}

/// `POST /cli/skin/pair/cancel {pairingId}`.
pub fn handle_cancel(pass: &SkinPass, body: &[u8], ctx: &Ctx) -> CliResponse {
    answer((|| {
        let pid = guest(pass)?;
        let v = json_body(body)?;
        known_fields(&v, &["pairingId"])?;
        let id = pairing_id(v.get("pairingId").and_then(Value::as_str))?;
        let cancelled = store::cancel(&pass.app, pid, id);
        if cancelled {
            super::audit("app-pair-cancel", &format!("guest:{pid}"), "ok", &ctx.ip, ctx.door(), &format!("app={} pairing={id}", pass.app));
        }
        Ok(json!({ "ok": true, "cancelled": cancelled }))
    })())
}

/// The pass routes (the dispatcher resolved `pass`).
pub fn handle_pass(path: &str, pass: &SkinPass, is_post: bool, body: &[u8], params: &HashMap<String, String>, ctx: &Ctx) -> CliResponse {
    match (path, is_post) {
        (START, true) => handle_start(pass, body, ctx),
        (STATUS, false) => handle_status(pass, params),
        (CANCEL, true) => handle_cancel(pass, body, ctx),
        _ => CliResponse::method_not_allowed(),
    }
}

// ── check / redeem (Public) ──────────────────────────────────────────

/// What the dispatcher needs after a check or a redeem (it throttles on a
/// 401; the audit row is written here).
pub struct Reply {
    pub response: CliResponse,
    /// `ok`, or why it was refused (audit only).
    pub outcome: String,
}

fn invalid() -> CliResponse {
    refusal(
        "401 Unauthorized",
        "pair_code_invalid",
        "that code is unknown, used, expired or for another App: start again from the web App",
    )
}

fn not_from_browser() -> CliResponse {
    refusal(
        "403 Forbidden",
        "pairing_not_from_browser",
        "a phone app redeems the code, never a web page (an App's owner lists its webview origins with k2 app pairing setup --native-origin)",
    )
}

/// The App a Public call is for: the link's, else the body's `app`
/// (required off a link). A body naming another App than the link's is
/// the generic `pair_code_invalid` (no cross-App oracle).
fn public_app(v: &Value, ctx: &Ctx) -> Result<String, (CliResponse, String)> {
    let named = v.get("app").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    match (&ctx.link_app, named) {
        (Some(l), Some(n)) if n != l => Err((invalid(), "wrong_app".into())),
        (Some(l), _) => Ok(l.clone()),
        (None, Some(n)) if n != skin::HOST_APP_ID => Ok(n.to_string()),
        (None, _) => Err((usage("send {code, app} (app: this App's service id)"), "usage".into())),
    }
}

/// The browser-shape rule with the App's own webview origins (CP20, V11):
/// checked before the code is looked at or consumed.
fn browser_refused(app: &str, headers: &str) -> bool {
    let allowed = store::config(app).ok().flatten().map(|c| c.native_origins).unwrap_or_default();
    crate::routes::http::pair_request_is_browser_shaped_allowing(headers, &allowed)
}

/// `POST /cli/skin/pair/check {code, app}`: what the code would pair as,
/// without using it.
pub fn handle_check(body: &[u8], headers: &str, ctx: &Ctx) -> Reply {
    let r = check(body, headers, ctx);
    let (response, outcome) = match r {
        Ok(v) => (CliResponse::ok_json(v.to_string()), "ok".to_string()),
        Err((resp, why)) => (resp, why),
    };
    if outcome != "ok" {
        super::audit("app-pair-check", "-", &outcome, &ctx.ip, ctx.door(), "");
    }
    Reply { response, outcome }
}

fn check(body: &[u8], headers: &str, ctx: &Ctx) -> Result<Value, (CliResponse, String)> {
    let v = json_body(body).map_err(|e| (e, "usage".to_string()))?;
    known_fields(&v, &["code", "app"]).map_err(|e| (e, "usage".to_string()))?;
    let app = public_app(&v, ctx)?;
    if browser_refused(&app, headers) {
        return Err((not_from_browser(), "browser".into()));
    }
    let code = v.get("code").and_then(Value::as_str).unwrap_or("");
    let c = store::check(code, &app).map_err(|r| (invalid(), r.as_str().to_string()))?;
    super::state(&app).map_err(|o| (invalid(), o.as_str().to_string()))?;
    let pass_live = store::session_info(&c.minted_from)
        .map_err(|e| (CliResponse::internal_error(e), "error".to_string()))?
        .is_some_and(|i| i.principal_id == c.principal_id && i.app == app);
    if !pass_live {
        return Err((invalid(), "web_pass_ended".into()));
    }
    if app_login::settings(&app).map_err(|e| (CliResponse::internal_error(e), "error".to_string()))?.stay_signed_in_days == 0 {
        return Err((invalid(), "device_signin_off".into()));
    }
    let who = store::person(&c.principal_id)
        .map_err(|e| (CliResponse::internal_error(e), "error".to_string()))?
        .ok_or_else(|| (invalid(), "person_gone".to_string()))?;
    let app_name = super::published(&app).map(|r| r.name).unwrap_or_default();
    let mut out = json!({
        "ok": true,
        "app": app,
        "appName": app_name,
        "username": who.username,
        "expiresInSecs": c.expires_in_secs,
    });
    if let Some(n) = who.full_name {
        out["fullName"] = json!(n);
    }
    Ok(out)
}

/// `POST /cli/skin/pair {code, app, device: {id, name}, push?}`.
pub fn handle_redeem(body: &[u8], headers: &str, ctx: &Ctx) -> Reply {
    let mut principal = String::from("-");
    let mut pairing = String::from("-");
    let mut app_seen = String::from("-");
    let r = redeem(body, headers, ctx, &mut principal, &mut pairing, &mut app_seen);
    let (response, outcome) = match r {
        Ok(v) => (CliResponse::ok_json(v.to_string()), "ok".to_string()),
        Err((resp, why)) => (resp, why),
    };
    super::audit(
        "app-pair",
        &if principal == "-" { "-".to_string() } else { format!("guest:{principal}") },
        &outcome,
        &ctx.ip,
        ctx.door(),
        &format!("app={app_seen} pairing={pairing}"),
    );
    Reply { response, outcome }
}

fn redeem(
    body: &[u8],
    headers: &str,
    ctx: &Ctx,
    principal: &mut String,
    pairing: &mut String,
    app_seen: &mut String,
) -> Result<Value, (CliResponse, String)> {
    let u = |e: CliResponse| (e, "usage".to_string());
    let v = json_body(body).map_err(u)?;
    known_fields(&v, &["code", "app", "device", "push"]).map_err(u)?;
    let app = public_app(&v, ctx)?;
    *app_seen = app.clone();
    if browser_refused(&app, headers) {
        return Err((not_from_browser(), "browser".into()));
    }
    // The shape is checked before the code is used (a typo never burns it).
    let device = match app_login::parse_device(v.get("device")) {
        Ok(Some(d)) => d,
        Ok(None) => return Err(u(usage("send device {id, name}: the phone app's install id and a name the person will recognise"))),
        Err(e) => return Err(u(usage(&e))),
    };
    let push = match v.get("push") {
        None | Some(Value::Null) => None,
        Some(p @ Value::Object(_)) => Some(p.clone()),
        Some(_) => return Err(u(usage("push is {platform, pushToken, environment?, previews?, mutedTopics?}"))),
    };
    let code = v.get("code").and_then(Value::as_str).unwrap_or("");
    let p = store::take(code, &app).map_err(|r| (invalid(), r.as_str().to_string()))?;
    *principal = p.principal_id.clone();
    *pairing = p.pairing_id.clone();
    let fail = |why: &str| {
        store::finish_failed(&p.pairing_id);
        (invalid(), why.to_string())
    };
    // A21 (V13): the daemon-DB reads before the skin transaction.
    if let Err(o) = super::state(&app) {
        return Err(fail(o.as_str()));
    }
    if super::published(&app).is_none() {
        return Err(fail("no_app"));
    }
    let app_ws = skin::ties::app_workspace(&app);
    // V14: a test guest pairs only through its App's own link.
    let r = skin::test_guests::with_link_login(ctx.link_app.as_deref(), || store::login_device_paired(&p, &device, app_ws.as_deref()))
        .map_err(|e| {
            store::finish_failed(&p.pairing_id);
            (CliResponse::internal_error(e), "error".to_string())
        })?;
    let (d, pid) = match r {
        store::Paired::Ok { session, principal_id } => (*session, principal_id),
        store::Paired::Refused(why) => return Err(fail(why)),
    };
    store::finish_paired(&p.pairing_id, &d.device.name, &d.device_login_id);
    let mut out = crate::app_mobile_routes::device_session_json(&d, &pid);
    out["rooms"] = json!(d.session.meta.rooms);
    out["roomHandles"] = json!(d.session.meta.room_handles);
    out["caps"] = json!(d.session.meta.caps);
    out["role"] = json!(d.session.role_name);
    out["roomAccess"] = json!(d.session.meta.room_access);
    out["via"] = json!(app_login::VIA_PAIRING);
    if let Some(push) = push {
        out["push"] = register_push(&d, &device, push);
    }
    first_pairing_note(&app);
    Ok(out)
}

/// CP21: push in the same step, after the pairing committed (V15), with
/// push's own rules. Never fails the pairing.
fn register_push(d: &app_login::DeviceSession, device: &app_login::Device, mut push: Value) -> Value {
    let refused = |code: &str| json!({ "registered": false, "code": code });
    if push.get("deviceId").and_then(Value::as_str).is_some_and(|x| x.trim() != device.id) {
        return refused("usage");
    }
    push["deviceId"] = json!(device.id);
    let reg = match skin::app_push::parse_registration(&push) {
        Ok(r) => r,
        Err(_) => return refused("usage"),
    };
    let Some(pass) = skin::resolve_skin_token(&d.session.raw) else { return refused("failed") };
    match crate::app_push::guest_routes::register_parsed(&pass, &reg) {
        Ok(v) => json!({ "registered": true, "id": v["id"], "who": v["who"], "configured": v["configured"] }),
        Err(e) => refused(&error_code(&e.body).unwrap_or_else(|| "failed".into())),
    }
}

fn error_code(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body).ok().and_then(|v| v["error"]["code"].as_str().map(str::to_string))
}

/// The first phone that pairs with an App: one note to its workspace (and
/// so its owner), never one per pairing (§9.2).
fn first_pairing_note(app: &str) {
    if !store::first_pairing(app).unwrap_or(false) {
        return;
    }
    if let Some(row) = super::published(app) {
        crate::apps::alerts::note(
            &row.project_id,
            "Phones can now pair",
            &format!("The first phone paired with {} by QR. People see and end their phones in the App; the owner sees them with k2 app people.", row.name),
        );
    }
}

#[cfg(test)]
#[path = "guest_routes_tests.rs"]
pub(crate) mod tests;
