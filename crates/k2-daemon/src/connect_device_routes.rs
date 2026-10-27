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
//!     pairs as the Owner/Admin login the owner linked to themselves (`ownerAccounts`,
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
//! - `POST /cli/auth/device/pair/check {code}` (Public): what a code would
//!   pair as (`{user, host, expiresInSecs}`) WITHOUT using it; same browser
//!   refusal, buckets and failure accounting as a redeem; a failed check is
//!   audited (`device-pair-check`).
//! - `GET /cli/auth/devices`: live paired phones (the owner token or an
//!   Owner login: everyone's; any other login: its own) plus `pairAs`, the
//!   login a new code from this caller would pair as.
//! - `POST /cli/auth/device/revoke {id}`: end one paired phone (the owner
//!   token or an Owner login: anyone's; any other login: its own).
//! - `GET /cli/auth/device/pair-code/status?id=<codeId>` (0.46.6): one
//!   code's state (`pending` / `paired` / `expired` / `void`) for the actor
//!   that minted it; anyone else, and an unknown id, get 404
//!   `no_such_code`. The fallback for the `pair_code_changed` frame.
//!
//! 0.46.6 pair success: a mint answers `codeId` too. When a code is
//! redeemed, expires, or stops being usable, the daemon puts a
//! `pair_code_changed` frame on the session events bus, person-scoped to
//! the minter ([`code_event`]), so Pair a phone shows its success state and
//! the phones list grows live. The `/pair` answer adds `serverName` (the
//! server's friendly name, else its host) for the phone's Paired screen.
//! Every redeem logs one timing line (outcome and milliseconds only).
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
/// Companion's pair screen: what a code would pair as, without using it.
pub const PAIR_CHECK: &str = "/cli/auth/device/pair/check";
pub const DEVICES: &str = "/cli/auth/devices";
pub const REVOKE: &str = "/cli/auth/device/revoke";
/// One code's state, for its minter (GET, `?id=<codeId>`).
pub const CODE_STATUS: &str = "/cli/auth/device/pair-code/status";

/// The session-events frame for a finished code (0.46.6): person-scoped to
/// whoever minted it. Never the code or a token.
pub fn code_event(s: &pairing::CodeStatus) -> crate::session_events::SessionEvent {
    let (device_name, session_id) = match &s.state {
        pairing::CodeState::Paired { device_name, session_id } => (Some(device_name.clone()), Some(session_id.clone())),
        _ => (None, None),
    };
    crate::session_events::SessionEvent::PairCodeChanged {
        person: person_for_actor(&s.minted_by),
        code_id: s.code_id.clone(),
        state: s.state.as_wire().to_string(),
        user: s.username.clone(),
        device_name,
        session_id,
    }
}

/// The person key an actor's sockets carry (`session_events_ws`): the
/// owner token is `owner`; a login is the owner when the owner linked it,
/// else itself.
pub fn person_for_actor(actor: &str) -> String {
    match actor.strip_prefix("user:") {
        None => "owner".to_string(),
        Some(login) => match k2_core::notify_prefs::People::load() {
            Ok(people) => people.person_key(login),
            Err(_) => login.to_ascii_lowercase(),
        },
    }
}

/// Put every finished code on the events bus (once per process; called
/// before the first mint, which is the only way a code exists).
pub fn install_event_listener() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        pairing::set_listener(|s| {
            let _ = crate::session_events::emit(code_event(s));
        });
    });
}

/// After a code's ten minutes, report it expired if nobody used it (the
/// sweep is a no-op for a code already finished).
pub fn schedule_expiry_sweep() {
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        rt.spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(pairing::CODE_TTL_SECS as u64 + 1)).await;
            let _ = tokio::task::spawn_blocking(pairing::sweep_expired).await;
        });
    }
}

/// The server's friendly name for the phone's Paired screen: the
/// computer's name (macOS Sharing name; Linux `PRETTY_HOSTNAME`), else
/// the public host, else the system host name. Read once per process.
pub fn server_display_name(public_host: Option<&str>) -> String {
    static FRIENDLY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let friendly = FRIENDLY.get_or_init(friendly_computer_name);
    if let Some(n) = friendly {
        return n.clone();
    }
    if let Some(h) = public_host.filter(|h| !h.is_empty()) {
        return h.to_string();
    }
    system_host_name().unwrap_or_else(|| "K2".to_string())
}

fn clean_server_name(raw: &str) -> Option<String> {
    let n = pairing::clean_name(raw, pairing::MAX_NAME_CHARS, "");
    (!n.is_empty()).then_some(n)
}

#[cfg(target_os = "macos")]
fn friendly_computer_name() -> Option<String> {
    let out = std::process::Command::new("/usr/sbin/scutil").args(["--get", "ComputerName"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    clean_server_name(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(target_os = "macos"))]
fn friendly_computer_name() -> Option<String> {
    let text = std::fs::read_to_string("/etc/machine-info").ok()?;
    let line = text.lines().find_map(|l| l.trim().strip_prefix("PRETTY_HOSTNAME="))?;
    clean_server_name(line.trim().trim_matches('"').trim_matches('\''))
}

#[cfg(not(unix))]
fn system_host_name() -> Option<String> {
    std::env::var("COMPUTERNAME").ok().and_then(|n| clean_server_name(&n))
}

#[cfg(unix)]
fn system_host_name() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: buf is valid for buf.len() bytes; gethostname NUL-terminates
    // on success (the last byte is kept 0 in any case).
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len() - 1) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    clean_server_name(&String::from_utf8_lossy(&buf[..end]))
}

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
pub fn owner_linked_login() -> Result<String, LinkedMiss> {
    let linked = k2_core::app_settings::load().owner_accounts;
    let mut best: Option<(u8, String)> = None;
    let mut member_only = false;
    for name in linked {
        let Ok(n) = connect_users::normalize_username(&name) else { continue };
        if connect_users::live_user_epoch(&n).is_none() {
            continue;
        }
        // Review M1 (Rosson: "the admin account"): only an Owner- or
        // Admin-role linked login counts; a Member link is not the owner's
        // admin account.
        let rank = match connect_users::role_for_user(&n) {
            Some(connect_users::Role::Owner) => 0,
            Some(connect_users::Role::Admin) => 1,
            _ => {
                member_only = true;
                continue;
            }
        };
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, n));
        }
    }
    match best {
        Some((_, n)) => Ok(n),
        None if member_only => Err(LinkedMiss::MemberOnly),
        None => Err(LinkedMiss::None),
    }
}

/// Why the owner token has no linked login to pair as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkedMiss {
    /// Nothing linked (or nothing enabled).
    None,
    /// Only Member-role logins are linked.
    MemberOnly,
}

fn not_linked(miss: LinkedMiss) -> CliResponse {
    let hint = match miss {
        LinkedMiss::None => "the owner token has no login of its own to pair a phone as. Link your admin Connect login (an Owner or Admin role; Settings → General → Your account), or name one: k2 users pair-phone --user <login>",
        LinkedMiss::MemberOnly => "the login linked to the owner is a Member; a phone paired from the owner token needs an admin login (an Owner or Admin role). Link your admin Connect login (Settings → General → Your account), or name one: k2 users pair-phone --user <login>",
    };
    refusal("409 Conflict", "owner_login_not_linked", hint)
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
        (Caller::OwnerToken, None) => owner_linked_login().map_err(not_linked),
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
    install_event_listener();
    match pairing::mint(target, actor) {
        Ok(code) => {
            let mut body = json!({
                "ok": true,
                "code": code.code,
                "codeId": code.code_id,
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
    /// How long the redeem itself took (`None` when it never ran: a bad
    /// body, or a check).
    pub redeem_ms: Option<f64>,
}

fn pair_invalid() -> CliResponse {
    refusal(
        "401 Unauthorized",
        "pair_code_invalid",
        "that pairing code is unknown, already used or expired: make a new one",
    )
}

/// `POST /cli/auth/device/pair {code, device: {id, name}}` (Public).
/// `public_host` feeds `serverName`'s fallback.
pub fn handle_pair(body: &[u8], public_host: Option<&str>) -> PairReply {
    let reply = |response: CliResponse, user: &str, outcome: &'static str| PairReply {
        response,
        user: user.to_string(),
        outcome,
        paired: None,
        redeem_ms: None,
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
    let started = std::time::Instant::now();
    let redeemed = pairing::redeem(code, &device.id, &device.name);
    let redeem_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
    let timed = |mut r: PairReply| {
        r.redeem_ms = redeem_ms;
        r
    };
    match redeemed {
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
                // 0.46.6 (additive): the server's friendly name for the
                // phone's Paired screen, else its host.
                "serverName": server_display_name(public_host),
            });
            PairReply {
                response: CliResponse::ok_json(body.to_string()),
                user: paired.username.clone(),
                outcome: "ok",
                paired: Some((paired.device_name, paired.minted_by, paired.session_id)),
                redeem_ms,
            }
        }
        Ok(Err(pairing::Refused::PasswordChangeRequired)) => {
            timed(reply(password_change_required(), "", "password_change_required"))
        }
        Ok(Err(why)) => timed(reply(pair_invalid(), "", why.as_str())),
        Err(e) => timed(reply(CliResponse::internal_error(e), "", "error")),
    }
}

/// The one timing line for a `POST /cli/auth/device/pair` (0.46.6): the
/// outcome and milliseconds only, never the code, token or device.
pub fn pair_timing_line(outcome: &str, redeem_ms: Option<f64>, total_ms: f64) -> String {
    let redeem = redeem_ms.map(|m| format!("{m:.3}")).unwrap_or_else(|| "-".to_string());
    format!("[pairing] side=daemon PAIR_SUMMARY route=/cli/auth/device/pair outcome={outcome} redeem_ms={redeem} total_ms={total_ms:.3}")
}

/// `GET /cli/auth/device/pair-code/status?id=<codeId>`: one code's state
/// for the actor that minted it (0.46.6). Never the code or a token.
pub fn handle_code_status(caller: &Caller, code_id: Option<&str>) -> CliResponse {
    let Some(id) = code_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return usage("send ?id=<codeId> (from POST /cli/auth/device/pair-code)");
    };
    let Some(s) = pairing::status(id, &caller.actor()) else {
        return refusal("404 Not Found", "no_such_code", "no pairing code with that id is yours to watch");
    };
    let mut body = json!({
        "ok": true,
        "codeId": s.code_id,
        "state": s.state.as_wire(),
        "user": s.username,
        "expiresAt": s.expires_at.to_rfc3339(),
        "deviceName": Value::Null,
        "sessionId": Value::Null,
    });
    if let pairing::CodeState::Paired { device_name, session_id } = &s.state {
        body["deviceName"] = json!(device_name);
        body["sessionId"] = json!(session_id);
    }
    CliResponse::ok_json(body.to_string())
}

/// `POST /cli/auth/device/pair/check {code}` (Public): what a still-valid
/// code would pair as, WITHOUT using it. Same refusals as a redeem; the
/// dispatcher throttles and audits failures exactly like a failed pair.
pub fn handle_pair_check(body: &[u8], host: Option<&str>) -> PairReply {
    let reply = |response: CliResponse, user: &str, outcome: &'static str| PairReply {
        response,
        user: user.to_string(),
        outcome,
        paired: None,
        redeem_ms: None,
    };
    let v = match json_body(body) {
        Ok(v) => v,
        Err(r) => return reply(r, "", "rejected"),
    };
    let Some(code) = text(&v, "code") else {
        return reply(usage("send {code}"), "", "rejected");
    };
    match pairing::check(code) {
        Ok(c) => {
            let body = json!({ "ok": true, "user": c.username, "host": host, "expiresInSecs": c.expires_in_secs });
            reply(CliResponse::ok_json(body.to_string()), &c.username, "ok")
        }
        Err(pairing::Refused::PasswordChangeRequired) => reply(password_change_required(), "", "password_change_required"),
        Err(why) => reply(pair_invalid(), "", why.as_str()),
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
    // The title is plain text (already free of control/bidi characters);
    // only the Markdown body escapes the name.
    let title = format!("Phone \"{device_name}\" paired for {username}");
    let name = notice_safe(device_name);
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
pub fn handle_devices(caller: &Caller, current: Option<&str>, mine: bool) -> CliResponse {
    let target = pair_target(caller, None);
    let mut pair_as = match &target {
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
    // F2: only the owner token may pick another login; it gets the enabled
    // logins to choose from (Settings → K2 Companion).
    if *caller == Caller::OwnerToken {
        let mut choices: Vec<String> = connect_users::list_users()
            .unwrap_or_default()
            .into_iter()
            .filter(|u| !u.disabled)
            .map(|u| u.username)
            .collect();
        choices.sort();
        pair_as["choices"] = json!(choices);
    }
    // `mine=1` (the profile modal): only the phones of the login a new code
    // from this caller pairs as (a login: itself; the owner token: its
    // linked login, or none).
    let listed = if mine {
        match &target {
            Ok(user) => connect_users::device_sessions(Some(user)),
            Err(_) => Ok(Vec::new()),
        }
    } else {
        connect_users::device_sessions(if caller.sees_everyone() { None } else { caller.login() })
    };
    match listed {
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
    fn the_timing_line_holds_the_outcome_and_milliseconds_only() {
        let line = pair_timing_line("ok", Some(0.4567), 3.21);
        assert_eq!(
            line,
            "[pairing] side=daemon PAIR_SUMMARY route=/cli/auth/device/pair outcome=ok redeem_ms=0.457 total_ms=3.210"
        );
        assert!(pair_timing_line("rejected", None, 1.0).contains("redeem_ms=-"));
    }

    #[test]
    fn a_finished_code_becomes_a_frame_for_its_minter() {
        let at = chrono::Utc::now();
        let paired = pairing::CodeStatus {
            code_id: "pc_1".into(),
            username: "ada".into(),
            minted_by: "owner-token".into(),
            expires_at: at,
            state: pairing::CodeState::Paired { device_name: "iPad".into(), session_id: "s-1".into() },
        };
        let ev = code_event(&paired);
        assert_eq!(ev.person(), Some("owner"), "the owner token's frames go to the owner person");
        let v: Value = serde_json::to_value(&ev).expect("ser");
        assert_eq!(
            v,
            json!({ "kind": "pair_code_changed", "codeId": "pc_1", "state": "paired", "user": "ada", "deviceName": "iPad", "sessionId": "s-1" })
        );
        let expired = pairing::CodeStatus { state: pairing::CodeState::Expired, ..paired };
        let v: Value = serde_json::to_value(code_event(&expired)).expect("ser");
        assert_eq!((v["state"].clone(), v["deviceName"].clone(), v["sessionId"].clone()), (json!("expired"), Value::Null, Value::Null));
    }

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
