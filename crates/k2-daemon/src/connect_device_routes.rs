//! Companion phone pairing (phone-web-off, Rosson 2026-10-24; security
//! review F1–F7 the same day).
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
//! Routes (the dispatcher resolves the caller, throttles and audits):
//! - `POST /cli/auth/device/pair-code {user?, password?}`: mint a code.
//!   - The owner token: `user` names any login; without `user` the code
//!     pairs as the login the owner linked to themselves (`ownerAccounts`,
//!     Settings → General → Your account), else 409
//!     `owner_login_not_linked` (Rosson 2026-10-24).
//!   - A login: for itself only, and only with its `password` typed again
//!     (F1, argon2 behind the login limiter). Naming anyone else is 403
//!     `owner_token_required` for every login role (F2).
//!   - Never for a browser-shaped request (403
//!     `device_pairing_not_from_browser`), a login that must change its
//!     password (409 `password_change_required`), or past the mint
//!     throttle (429 `rate_limited`).
//!   - The answer carries `pairUrl` (`https://<public host>/pair#<CODE>`,
//!     the code in the fragment so no server ever sees it) and its QR
//!     matrix when this server has a public host.
//! - `POST /cli/auth/device/pair {code, device: {id, name}}` (Public): the
//!   phone trades the code for its device session. Open while web access
//!   is off; refuses a browser `Origin` / `Sec-Fetch-*` (F5) but not the
//!   edge's markers; behind the login limiter's per-IP and global buckets
//!   (F3). Never sets a cookie. The person and the owner get a notice.
//! - `GET /cli/auth/devices`: live paired phones (the owner token or an
//!   Owner login: everyone's; any other login: its own) plus `pairAs`, the
//!   login a new code from this caller would pair as.
//! - `POST /cli/auth/device/revoke {id}`: end one paired phone (the owner
//!   token or an Owner login: anyone's; any other login: its own).
//!
//! A device session ends after 30 days unused or 180 days after pairing
//! (F1), on revoke, on the phone's own `POST /cli/auth/logout`, on a
//! password change, disable, kick or removal of its login, or when the
//! same install pairs again. A desktop logout does NOT end it (that ends
//! only the desktop's own session). Once ended, every route answers the
//! token with 403 `device_session_ended`.

use k2_core::connect_device_pairing as pairing;
use k2_core::connect_users;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;

pub const PAIR_CODE: &str = "/cli/auth/device/pair-code";
pub const PAIR: &str = "/cli/auth/device/pair";
pub const DEVICES: &str = "/cli/auth/devices";
pub const REVOKE: &str = "/cli/auth/device/revoke";

/// Mint throttle: codes per target login per [`crate::login_throttle::WINDOW_SECS`] (F4).
pub const MINT_LIMIT: usize = 10;
/// Redeem throttle per client key per window (F3); the shared tunnel key
/// gets [`crate::login_throttle::TUNNEL_SHARED_LIMIT`].
pub const PAIR_LIMIT: usize = 10;

/// Who is calling, resolved by the dispatcher from the token (never the
/// body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The owner daemon token (the desktop app and the CLI on the server).
    OwnerToken,
    /// A live Connect login. `owner_role`: an Owner-role login (sees and
    /// ends everyone's phones, but mints only for itself, F2).
    Login { username: String, owner_role: bool },
}

impl Caller {
    /// The audit actor (`owner-token` / `user:<name>`). Never a secret.
    pub fn actor(&self) -> String {
        match self {
            Caller::OwnerToken => "owner-token".to_string(),
            Caller::Login { username, .. } => format!("user:{username}"),
        }
    }

    fn sees_everyone(&self) -> bool {
        matches!(self, Caller::OwnerToken | Caller::Login { owner_role: true, .. })
    }

    fn login(&self) -> Option<&str> {
        match self {
            Caller::OwnerToken => None,
            Caller::Login { username, .. } => Some(username),
        }
    }
}

pub fn refusal(status: &'static str, code: &str, hint: &str) -> CliResponse {
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

/// No usable credential on a route that needs one.
pub fn login_required() -> CliResponse {
    refusal("401 Unauthorized", "login_required", "sign in first: send the owner token or a login's session token")
}

/// A login's password re-check failed (or the login is locked; same answer).
pub fn password_invalid() -> CliResponse {
    refusal(
        "401 Unauthorized",
        "password_invalid",
        "that password is wrong (or the login is locked for a while): type your password again to make a pairing code",
    )
}

/// Too many attempts from this client or for this login.
pub fn rate_limited() -> CliResponse {
    refusal("429 Too Many Requests", "rate_limited", "too many attempts: wait a few minutes and try again")
}

/// The answer for a browser-shaped request on the mint route.
pub fn browser_refused() -> CliResponse {
    refusal(
        "403 Forbidden",
        "device_pairing_not_from_browser",
        "pairing codes can't be made from a browser or through <label>.app.k2.dev. From the Companion, call https://<label>.k2.dev directly; or ask the server's owner for a code (desktop Settings → Pair a phone, or k2 users pair-phone).",
    )
}

/// The answer for a browser-shaped request on the redeem route (F5).
pub fn pair_browser_refused() -> CliResponse {
    refusal(
        "403 Forbidden",
        "device_pairing_not_from_browser",
        "a pairing code is redeemed by the K2 Companion app, not a web page: open the link or scan the QR with the app",
    )
}

/// Every route's answer for a token whose device session ended.
pub fn device_session_ended() -> CliResponse {
    refusal(
        "403 Forbidden",
        "device_session_ended",
        "this phone's pairing ended (revoked, signed out, unused for 30 days, 180 days old, or its login's password changed): pair it again with a new code",
    )
}

/// The parsed `pair-code` body.
#[derive(Debug, Clone, Default)]
pub struct PairCodeBody {
    pub user: Option<String>,
    pub password: Option<String>,
}

pub fn parse_pair_code_body(body: &[u8]) -> Result<PairCodeBody, CliResponse> {
    let v = json_body(body)?;
    // The password is taken as sent (no trim): it is checked, never stored.
    let password = v.get("password").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    let user = text(&v, "user").or_else(|| text(&v, "username")).map(str::to_string);
    Ok(PairCodeBody { user, password })
}

/// The login the owner linked to themselves (`ownerAccounts`): the first
/// listed that exists and is enabled, preferring an Owner, then an Admin
/// role. `None` when none is linked.
pub fn owner_linked_login() -> Option<String> {
    let linked = k2_core::app_settings::load().owner_accounts;
    let mut best: Option<(u8, String)> = None;
    for name in linked {
        let Ok(n) = connect_users::normalize_username(&name) else { continue };
        if connect_users::live_user_epoch(&n).is_none() {
            continue;
        }
        let rank = match connect_users::role_for_user(&n) {
            Some(connect_users::Role::Owner) => 0,
            Some(connect_users::Role::Admin) => 1,
            _ => 2,
        };
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, n));
        }
    }
    best.map(|(_, n)| n)
}

fn not_linked() -> CliResponse {
    refusal(
        "409 Conflict",
        "owner_login_not_linked",
        "the owner token has no login of its own to pair a phone as. Link the Connect login you use (Settings → General → Your account), or name one: k2 users pair-phone --user <login>",
    )
}

fn password_change_required() -> CliResponse {
    refusal(
        "409 Conflict",
        "password_change_required",
        "that login must change its password before a phone can be paired to it",
    )
}

/// What a new code from `caller` pairs as. `Err` is the refusal.
pub fn pair_target(caller: &Caller, asked: Option<&str>) -> Result<String, CliResponse> {
    match (caller, asked) {
        (Caller::OwnerToken, Some(u)) => connect_users::normalize_username(u).map_err(|e| refusal("400 Bad Request", "no_such_login", &e)),
        (Caller::OwnerToken, None) => owner_linked_login().ok_or_else(not_linked),
        (Caller::Login { username, .. }, None) => Ok(username.clone()),
        (Caller::Login { username, .. }, Some(u)) => match connect_users::normalize_username(u) {
            Ok(n) if n == *username => Ok(username.clone()),
            _ => Err(refusal(
                "403 Forbidden",
                "owner_token_required",
                "only the owner token (the desktop app or the k2 CLI on the server) can make a pairing code for another login",
            )),
        },
    }
}

/// The checks before a login's password re-check: the target rule, a
/// caller login that must change its password, a missing password.
pub fn pre_password_checks(caller: &Caller, req: &PairCodeBody) -> Result<String, CliResponse> {
    let target = pair_target(caller, req.user.as_deref())?;
    if let Some(me) = caller.login() {
        if connect_users::must_change_password(me) {
            return Err(password_change_required());
        }
        if req.password.is_none() {
            return Err(usage("type your password again to make a pairing code: send {password}"));
        }
    }
    Ok(target)
}

/// The pairing link for `code` on `host` (`https://<host>/pair#<CODE>`).
pub fn pair_url(host: &str, code: &str) -> String {
    format!("https://{host}/pair#{code}")
}

/// Mint for `target` (all checks done) and answer.
pub fn mint_response(target: &str, actor: &str, host: Option<&str>) -> CliResponse {
    match pairing::mint(target, actor) {
        Ok(code) => {
            let mut body = json!({
                "ok": true,
                "code": code.code,
                "user": code.username,
                "expiresAt": code.expires_at.to_rfc3339(),
                "expiresInSecs": pairing::CODE_TTL_SECS,
                "host": host,
                "pairUrl": Value::Null,
                "qr": Value::Null,
            });
            if let Some(h) = host {
                let url = pair_url(h, &code.code);
                if let Some(rows) = k2_core::qr::encode(&url) {
                    body["qr"] = json!({ "size": rows.len(), "rows": rows });
                }
                body["pairUrl"] = Value::String(url);
            }
            CliResponse::ok_json(body.to_string())
        }
        Err(pairing::MintError::PasswordChangeRequired) => password_change_required(),
        Err(pairing::MintError::NoSuchLogin(e)) => refusal("400 Bad Request", "no_such_login", &e),
    }
}

/// What the dispatcher audits and notifies about a pair attempt.
pub struct PairReply {
    pub response: CliResponse,
    pub user: String,
    pub outcome: &'static str,
    /// Set on success: (device name, minted by, device session id).
    pub paired: Option<(String, String, String)>,
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
    let reply = |response: CliResponse, user: &str, outcome: &'static str| PairReply {
        response,
        user: user.to_string(),
        outcome,
        paired: None,
    };
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
            let role = connect_users::role_for_user(&paired.username).map(|r| r.as_wire()).unwrap_or("member");
            let expires_at = (chrono::Utc::now() + chrono::Duration::days(connect_users::DEVICE_SESSION_TTL_DAYS)).to_rfc3339();
            let max_expires_at =
                (chrono::Utc::now() + chrono::Duration::days(connect_users::DEVICE_SESSION_MAX_DAYS)).to_rfc3339();
            let body = json!({
                "ok": true,
                "token": paired.token,
                "username": paired.username,
                "role": role,
                "expiresAt": expires_at,
                "maxExpiresAt": max_expires_at,
                "slidingDays": connect_users::DEVICE_SESSION_TTL_DAYS,
                "maxDays": connect_users::DEVICE_SESSION_MAX_DAYS,
                // Kept for compatibility: always false (a login that must
                // change its password can't get or redeem a code).
                "mustChangePassword": false,
                "device": { "sessionId": paired.session_id, "id": device.id, "name": paired.device_name },
            });
            PairReply {
                response: CliResponse::ok_json(body.to_string()),
                user: paired.username.clone(),
                outcome: "ok",
                paired: Some((paired.device_name, paired.minted_by, paired.session_id)),
            }
        }
        Ok(Err(pairing::Refused::PasswordChangeRequired)) => reply(password_change_required(), "", "password_change_required"),
        Ok(Err(why)) => reply(pair_invalid(), "", why.as_str()),
        Err(e) => reply(CliResponse::internal_error(e), "", "error"),
    }
}

/// A device name as plain, single-line Markdown-safe text for a notice
/// (F7): Markdown/HTML punctuation escaped with a backslash.
pub fn notice_safe(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if "\\`*_{}[]()<>#+-.!|~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// F1: tell the person and the owner a phone was paired.
pub fn notify_paired(username: &str, device_name: &str, minted_by: &str) {
    let name = notice_safe(device_name);
    let title = format!("Phone \"{name}\" paired for {username}");
    let body = format!(
        "A phone named \"{name}\" was paired to the login **{username}** (code made by {minted_by}).\n\n\
         Not you? Revoke it: Settings → Admin Access → Paired phones, or `k2 users phones` then `k2 users unpair-phone <id>`. \
         Changing the login's password also ends every phone paired to it."
    );
    if let Err(e) = crate::feedback_routes::file_people_notice(
        k2_core::db::AUDIT_SENTINEL_IDS[0],
        "k2-security",
        &title,
        &body,
        &[username.to_string()],
    ) {
        k2_core::log_debug!("[pairing] paired notice for {username}: {e}");
    }
}

/// `GET /cli/auth/devices`. `current` = the caller's own session id (to
/// mark "this phone").
pub fn handle_devices(caller: &Caller, current: Option<&str>) -> CliResponse {
    let who = if caller.sees_everyone() { None } else { caller.login() };
    let pair_as = match pair_target(caller, None) {
        Ok(user) => json!({
            "user": user,
            "source": if caller.login().is_some() { "self" } else { "owner-linked" },
            "needsPassword": caller.login().is_some(),
        }),
        Err(r) => {
            let err: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
            json!({ "user": Value::Null, "needsPassword": caller.login().is_some(), "error": err["error"].clone() })
        }
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
                        "maxExpiresAt": d.max_expires_at,
                        "pairedBy": d.paired_by,
                        "current": current == Some(d.id.as_str()),
                    })
                })
                .collect();
            CliResponse::ok_json(json!({ "ok": true, "devices": devices, "pairAs": pair_as }).to_string())
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
    let only = if caller.sees_everyone() { None } else { caller.login() };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_names_are_escaped() {
        assert_eq!(notice_safe("[x](http://e)"), "\\[x\\]\\(http://e\\)");
        assert_eq!(notice_safe("Rosson's iPhone"), "Rosson's iPhone");
        assert_eq!(notice_safe("<img>"), "\\<img\\>");
    }

    #[test]
    fn a_login_of_any_role_mints_only_for_itself() {
        for owner_role in [false, true] {
            let me = Caller::Login { username: "ada".into(), owner_role };
            match pair_target(&me, Some("bob")) {
                Err(r) => assert!(r.body.contains("owner_token_required"), "{}", r.body),
                Ok(t) => panic!("a login minted for {t}"),
            }
            for asked in [Some("ADA"), None] {
                match pair_target(&me, asked) {
                    Ok(t) => assert_eq!(t, "ada"),
                    Err(r) => panic!("self refused: {}", r.body),
                }
            }
        }
    }
}
