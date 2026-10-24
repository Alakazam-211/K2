//! Companion phone pairing (phone-web-off, Rosson 2026-10-24).
//!
//! "Web access off" (`webClientEnabled=false`) blocks browsers. The
//! Companion phone app signs in through the same public door as the web
//! app, so the daemon needs something a paired phone holds and a browser
//! does not: a **device session** (`k2_core::connect_users`), minted only
//! by redeeming a one-time pairing code (`k2_core::connect_device_pairing`).
//! A request that carries a live device session (as a Bearer or `?token=`,
//! never a cookie) passes the web wall; see
//! `routes::http::device_session_passes_web_wall`.
//!
//! Routes:
//! - `POST /cli/auth/device/pair-code {user?}`: mint a code. The owner
//!   token or an Owner login may name any login (`user`); any other login
//!   gets a code for itself. Refused for a request shaped like a browser
//!   ([`crate::routes::http::request_is_browser_shaped`]): the edge's
//!   markers, a session cookie, or a browser `Origin` / `Sec-Fetch-*`. So
//!   the web app can never mint one, web on or off.
//! - `POST /cli/auth/device/pair {code, device: {id, name}}` (Public): the
//!   phone trades the code for its device session. Open while web access
//!   is off (the code is the credential). Never sets a cookie.
//! - `GET /cli/auth/devices`: live paired phones. The owner token or an
//!   Owner login sees everyone's; any other login sees its own.
//! - `POST /cli/auth/device/revoke {id}`: end one paired phone. The owner
//!   token or an Owner login may end anyone's; any other login its own.

use k2_core::connect_device_pairing as pairing;
use k2_core::connect_users;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;

pub const PAIR_CODE: &str = "/cli/auth/device/pair-code";
pub const PAIR: &str = "/cli/auth/device/pair";
pub const DEVICES: &str = "/cli/auth/devices";
pub const REVOKE: &str = "/cli/auth/device/revoke";

/// Who is calling, resolved by the dispatcher from the token (never the
/// body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The owner token or an Owner login (`owner_role_identity`).
    Owner { login: Option<String> },
    /// Any other live Connect login.
    Login(String),
}

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

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// The answer for a browser-shaped request on a minting route.
pub fn browser_refused() -> CliResponse {
    refusal(
        "403 Forbidden",
        "device_pairing_not_from_browser",
        "a phone can't be paired from a web browser: make the pairing code with the owner token (the desktop app or the k2 CLI) or from a signed-in Companion app",
    )
}

/// `POST /cli/auth/device/pair-code {user?}`.
pub fn handle_pair_code(caller: &Caller, body: &[u8]) -> CliResponse {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let asked = text(&v, "user").or_else(|| text(&v, "username"));
    let username = match (caller, asked) {
        (Caller::Owner { .. }, Some(u)) => u.to_string(),
        (Caller::Owner { login: Some(me) }, None) => me.clone(),
        (Caller::Owner { login: None }, None) => {
            return usage("the owner token has no login of its own: send {user} (the login the phone signs in as)")
        }
        (Caller::Login(me), None) => me.clone(),
        (Caller::Login(me), Some(u)) => match connect_users::normalize_username(u) {
            Ok(n) if n == *me => me.clone(),
            _ => {
                return refusal(
                    "403 Forbidden",
                    "owner_required",
                    "only the owner can make a pairing code for another login",
                )
            }
        },
    };
    match pairing::mint(&username) {
        Ok(code) => CliResponse::ok_json(
            json!({
                "ok": true,
                "code": code.code,
                "user": code.username,
                "expiresAt": code.expires_at.to_rfc3339(),
                "expiresInSecs": pairing::CODE_TTL_SECS,
            })
            .to_string(),
        ),
        Err(e) => refusal("400 Bad Request", "no_such_login", &e),
    }
}

/// What the dispatcher audits about a pair attempt.
pub struct PairReply {
    pub response: CliResponse,
    pub user: String,
    pub outcome: &'static str,
}

fn pair_invalid() -> CliResponse {
    refusal(
        "401 Unauthorized",
        "pair_code_invalid",
        "that pairing code is unknown, already used or expired: make a new one",
    )
}

/// `POST /cli/auth/device/pair {code, device: {id, name}}` (Public).
pub fn handle_pair(body: &[u8]) -> PairReply {
    let reply = |response, user: &str, outcome| PairReply { response, user: user.to_string(), outcome };
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return reply(r, "", "rejected"),
    };
    let Some(code) = text(&v, "code") else {
        return reply(usage("send {code, device: {id, name}}"), "", "rejected");
    };
    let device = match k2_core::skin::app_login::parse_device(v.get("device")) {
        Ok(Some(d)) => d,
        Ok(None) => return reply(usage("send {code, device: {id, name}}: device is required"), "", "rejected"),
        Err(e) => return reply(usage(&e), "", "rejected"),
    };
    match pairing::redeem(code, &device.id, &device.name) {
        Ok(Ok(paired)) => {
            let role = connect_users::role_for_user(&paired.username)
                .map(|r| r.as_wire())
                .unwrap_or("member");
            let expires_at = (chrono::Utc::now()
                + chrono::Duration::days(connect_users::DEVICE_SESSION_TTL_DAYS))
            .to_rfc3339();
            let body = json!({
                "ok": true,
                "token": paired.token,
                "username": paired.username,
                "role": role,
                "expiresAt": expires_at,
                "slidingDays": connect_users::DEVICE_SESSION_TTL_DAYS,
                "mustChangePassword": connect_users::must_change_password(&paired.username),
                "device": { "sessionId": paired.session_id, "id": device.id, "name": device.name },
            });
            reply(CliResponse::ok_json(body.to_string()), &paired.username, "ok")
        }
        Ok(Err(why)) => reply(pair_invalid(), "", why.as_str()),
        Err(e) => reply(CliResponse::internal_error(e), "", "error"),
    }
}

/// `GET /cli/auth/devices`. `current` = the caller's own session id (to
/// mark "this phone").
pub fn handle_devices(caller: &Caller, current: Option<&str>) -> CliResponse {
    let who = match caller {
        Caller::Owner { .. } => None,
        Caller::Login(me) => Some(me.as_str()),
    };
    match connect_users::device_sessions(who) {
        Ok(list) => {
            let devices: Vec<Value> = list
                .iter()
                .map(|d| {
                    json!({
                        "id": d.id,
                        "username": d.username,
                        "deviceId": d.device_id,
                        "name": d.name,
                        "createdAt": d.created_at,
                        "expiresAt": d.expires_at,
                        "current": current == Some(d.id.as_str()),
                    })
                })
                .collect();
            CliResponse::ok_json(json!({ "ok": true, "devices": devices }).to_string())
        }
        Err(e) => CliResponse::internal_error(e),
    }
}

/// `POST /cli/auth/device/revoke {id}`. Returns the response and the
/// person whose phone ended (for the audit line).
pub fn handle_revoke(caller: &Caller, body: &[u8]) -> (CliResponse, Option<String>) {
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return (r, None),
    };
    let Some(id) = text(&v, "id") else {
        return (usage("send {id} (from GET /cli/auth/devices)"), None);
    };
    let only = match caller {
        Caller::Owner { .. } => None,
        Caller::Login(me) => Some(me.as_str()),
    };
    match connect_users::revoke_device_session(id, only) {
        Ok(Some(user)) => (
            CliResponse::ok_json(json!({ "ok": true, "revoked": id, "username": user }).to_string()),
            Some(user),
        ),
        Ok(None) => (
            refusal("404 Not Found", "no_such_device", "no paired phone with that id is yours to end"),
            None,
        ),
        Err(e) => (CliResponse::internal_error(e), None),
    }
}
