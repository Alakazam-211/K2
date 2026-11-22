//! `/auth/v1`: the small GoTrue-shaped subset supabase-js's auth calls use
//! (prd-app-supabase-compat-v1 §8.1), over K2 App users. K2 stays the user
//! store and the session store: every session is a K2 session of this App.
//!
//! ```text
//! GET  /auth/v1/user                         getUser
//! PUT  /auth/v1/user  {password}             updateUser
//! POST /auth/v1/token?grant_type=password    signInWithPassword {email, password}
//! POST /auth/v1/token?grant_type=refresh_token  refreshSession / setSession {refresh_token}
//! POST /auth/v1/token?grant_type=pkce        exchangeCodeForSession {auth_code, code_verifier}
//! POST /auth/v1/verify {type, token_hash}    verifyOtp (invite, recovery)
//! GET  /auth/v1/verify?token=&type=&redirect_to=   a mailed link, clicked
//! POST /auth/v1/logout?scope=local|global|others   signOut
//! GET|POST /auth/v1/admin/users              admin.listUsers / createUser   (the server key)
//! GET|PUT|DELETE /auth/v1/admin/users/<id>   admin.getUserById / updateUserById / deleteUser
//! POST /auth/v1/admin/generate_link          admin.generateLink (invite, recovery; the key's links scope)
//! ```
//!
//! **The token never sits in the browser** (R4). On the App's public host a
//! session is answered as a `Set-Cookie` (`k2_skin_session`, HttpOnly,
//! Secure, SameSite=Lax) and the JSON's `access_token` and `refresh_token`
//! are empty. On the gateway link (server code) they hold the K2 pass and
//! the device refresh token; the shim's cookie adapter writes them HttpOnly.
//!
//! **Invite and recovery links** keep K2's rule that a link alone never
//! opens a session: the link's token is checked, not used, and the answer is
//! a short-lived pending pass (`k2vrf_…`, [`PENDING_TTL`]) that opens only
//! `GET /user`, `PUT /user {password}` and `logout`. Setting the password
//! uses the link (K2's own consume: it ends the person's other sessions,
//! clears their lockout) and turns the pending pass into a real session:
//! the next answer sets the cookie (public host), and on the link the same
//! `k2vrf_` keeps working as that session until it expires.
//!
//! Errors are GoTrue's shape for the 2024-01-01 API (`x-supabase-api-version`):
//! `{code, message}` plus the older `error_code` and `msg`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::skin::{self, app_login, auth_door as store, SkinLoginOutcome, SkinPass, SkinPrincipal};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::door::{self, Ctx, Ingress, SESSION_COOKIE};
use super::http::{query_pairs, DoorAnswer, DoorRequest};

/// A pending pass lives this long.
pub const PENDING_TTL: Duration = Duration::from_secs(15 * 60);
/// A pending pass's prefix (the door's own).
pub use super::door::PENDING_PREFIX;
/// Most pending passes held at once (the oldest go first past it).
const MAX_PENDING: usize = 10_000;
/// The API version K2's answers follow.
pub const API_VERSION: &str = "2024-01-01";

// ── answers ─────────────────────────────────────────────────────────

/// A GoTrue-shaped refusal.
pub fn gerr(status: u16, code: &str, message: impl Into<String>) -> DoorAnswer {
    let m = message.into();
    DoorAnswer::json(status, &json!({ "code": code, "error_code": code, "message": m, "msg": m })).header("x-supabase-api-version", API_VERSION)
}

fn ok(v: Value) -> DoorAnswer {
    DoorAnswer::json(200, &v).header("x-supabase-api-version", API_VERSION)
}

fn iso(secs: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap_or_default()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// A K2 App user as GoTrue's user object.
pub fn user_json(svc: &str, p: &SkinPrincipal) -> Value {
    let created = iso(p.created_at);
    json!({
        "id": p.id,
        "aud": "authenticated",
        "role": "authenticated",
        "email": p.email.clone().unwrap_or_default(),
        "email_confirmed_at": p.email.as_ref().map(|_| created.clone()),
        "phone": "",
        "confirmed_at": created,
        "last_sign_in_at": Value::Null,
        "app_metadata": { "provider": "email", "providers": ["email"], "k2_app": svc, "k2_role": p.role_name },
        "user_metadata": { "full_name": p.full_name },
        "identities": [],
        "created_at": created,
        "updated_at": created,
        "is_anonymous": false,
    })
}

fn principal_by_id(id: &str) -> Option<SkinPrincipal> {
    skin::list_principals().ok()?.into_iter().find(|p| p.id == id)
}

/// The cookie that carries a session on the public host (`None`: clear it).
/// The cookie with the same pass in its JWT shape, for server code's
/// supabase-js (the shim's cookie adapter reads it; HttpOnly too).
pub const JWT_COOKIE: &str = "k2_skin_jwt";

/// Both session cookies on an answer: the pass and its JWT shape
/// (`None`: clear both).
fn with_cookies(a: DoorAnswer, value: Option<(&str, &str, i64)>) -> DoorAnswer {
    match value {
        Some((pass, jwt, max_age)) => a.header("Set-Cookie", cookie(Some((pass, max_age)))).header(
            "Set-Cookie",
            format!("{JWT_COOKIE}={jwt}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={}", max_age.max(0)),
        ),
        None => a.header("Set-Cookie", cookie(None)).header("Set-Cookie", format!("{JWT_COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0")),
    }
}

fn cookie(value: Option<(&str, i64)>) -> String {
    match value {
        Some((v, max_age)) => format!("{SESSION_COOKIE}={v}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={}", max_age.max(0)),
        None => format!("{SESSION_COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0"),
    }
}

/// A session pass's claims and `exp`, read from the stores now.
fn pass_facts(ctx: &Ctx, pass: &str) -> (Value, i64) {
    match session_of(pass, &ctx.service_id) {
        Some(p) => {
            let c = door::session_claims(&ctx.service_id, &p);
            let exp = c["exp"].as_i64().unwrap_or(door::NO_EXPIRY);
            (c, exp)
        }
        None => (Value::Null, now_secs()),
    }
}

/// A session answer. Public host: the pass goes in the cookie only. On the
/// link: the pass in a JWT's shape (supabase-js decodes it in setSession),
/// and a refresh token that is never empty (setSession needs one): the
/// device sign-in's, else the pass itself (a refresh then answers the same
/// session while it lives).
fn session_answer(ctx: &Ctx, pass: &str, claims: Value, exp: i64, refresh: Option<&str>, user: Value) -> DoorAnswer {
    let now = now_secs();
    let public = matches!(ctx.ingress, Ingress::Public { .. });
    let token = door::wrap_pass(pass, &claims);
    let refresh = refresh.unwrap_or(pass);
    let body = json!({
        "access_token": if public { "" } else { token.as_str() },
        "token_type": "bearer",
        "expires_in": (exp - now).max(0),
        "expires_at": exp,
        "refresh_token": if public { "" } else { refresh },
        "user": user,
    });
    let a = ok(body);
    if public {
        with_cookies(a, Some((pass, &token, exp - now)))
    } else {
        a
    }
}

// ── pending passes (invite and recovery links) ───────────────────────

#[derive(Debug, Clone)]
struct Pending {
    app: String,
    /// The link's own token, used when the password is set.
    link_token: String,
    principal_id: String,
    made: Instant,
    /// `exp` of the pending pass (unix seconds).
    exp: i64,
    /// The session the password set turned it into.
    session: Option<(String, Option<i64>)>,
}

fn pendings() -> &'static Mutex<HashMap<String, Pending>> {
    static P: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

fn digest(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn new_pending(p: Pending) -> String {
    // Two v4 UUIDs: 244 random bits from the OS generator.
    let raw = format!("{PENDING_PREFIX}{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let mut g = pendings().lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|_, v| v.made.elapsed() < PENDING_TTL);
    if g.len() >= MAX_PENDING {
        if let Some(oldest) = g.iter().min_by_key(|(_, v)| v.made).map(|(k, _)| k.clone()) {
            g.remove(&oldest);
        }
    }
    g.insert(digest(&raw), p);
    raw
}

fn pending(raw: &str, app: &str) -> Option<Pending> {
    let g = pendings().lock().unwrap_or_else(|e| e.into_inner());
    g.get(&digest(raw)).filter(|p| p.app == app && p.made.elapsed() < PENDING_TTL).cloned()
}

/// The session pass a pending pass turned into (its password was set), for
/// the data door: server code that kept the `k2vrf_` from `verifyOtp` keeps
/// working as the person until it expires.
pub fn upgraded_pass(token: &str, app: &str) -> Option<String> {
    let raw = door::unwrap_pass(token);
    let p = pending(raw, app)?;
    if !door::claims_match(token, &pending_claims(app, &p)) {
        return None;
    }
    p.session.map(|(s, _)| s)
}

/// The claims a pending pass carries.
fn pending_claims(app: &str, p: &Pending) -> Value {
    let email = principal_by_id(&p.principal_id).and_then(|u| u.email).unwrap_or_default();
    door::pass_claims(app, &p.principal_id, &email, Some(p.exp))
}

// ── who ──────────────────────────────────────────────────────────────

/// Who an auth call is about.
enum Holder {
    Session(SkinPass),
    Pending(String, Pending),
}

fn session_of(raw: &str, svc: &str) -> Option<SkinPass> {
    skin::resolve_skin_token(raw).filter(|p| p.session && p.app == svc)
}

/// The caller's session (or pending pass): the bearer, else (public host)
/// the cookie. On the public host a web session's pass is never a bearer.
fn holder(req: &DoorRequest, ctx: &Ctx) -> Result<Option<Holder>, DoorAnswer> {
    let apikey = req.header("apikey").unwrap_or("");
    let public = matches!(ctx.ingress, Ingress::Public { .. });
    let bearer = req.bearer().filter(|b| *b != apikey && !door::is_publishable(b) && !k2_core::skin::is_api_key_prefix(b));
    let edited = || gerr(403, "bad_jwt", "invalid JWT: its claims are not the ones K2 wrote");
    let from = |token: &str, via_bearer: bool| -> Result<Option<Holder>, DoorAnswer> {
        let raw = door::unwrap_pass(token);
        if raw.starts_with(PENDING_PREFIX) {
            return match pending(raw, &ctx.service_id) {
                Some(p) if !door::claims_match(token, &pending_claims(&ctx.service_id, &p)) => Err(edited()),
                Some(p) => match &p.session {
                    Some((pass, _)) => Ok(session_of(pass, &ctx.service_id).map(Holder::Session)),
                    None => Ok(Some(Holder::Pending(raw.to_string(), p))),
                },
                None => Err(gerr(403, "bad_jwt", "invalid JWT: the link's pass has expired; open the link again")),
            };
        }
        let Some(pass) = session_of(raw, &ctx.service_id) else {
            return Err(gerr(403, "bad_jwt", "invalid JWT: unable to parse or verify signature, the session has ended"));
        };
        if !door::claims_match(token, &door::session_claims(&ctx.service_id, &pass)) {
            return Err(edited());
        }
        if public && via_bearer && app_login::device_for_session(&pass.id).ok().flatten().is_none() {
            return Err(gerr(403, "web_pass_as_bearer", "a web session's pass is not accepted as a bearer on the App's public host"));
        }
        Ok(Some(Holder::Session(pass)))
    };
    if let Some(b) = bearer {
        return from(b, true);
    }
    if public {
        if let Some(c) = req.cookie(SESSION_COOKIE) {
            return from(&c, false);
        }
    }
    Ok(None)
}

/// The apikey: the publishable key, or the server key (`Some(grant)`).
fn key_of(req: &DoorRequest, ctx: &Ctx) -> Result<Option<(String, k2_core::app_grace::keys::AppKeyGrant)>, DoorAnswer> {
    let Some(apikey) = req.header("apikey").filter(|k| !k.is_empty()) else {
        return Err(gerr(401, "no_api_key", "No API key found in request"));
    };
    let raw = if k2_core::skin::is_api_key_prefix(apikey) { Some(apikey) } else { req.bearer().filter(|b| k2_core::skin::is_api_key_prefix(b)) };
    if let Some(raw) = raw {
        let Some(key) = k2_core::api_keys::resolve_app_key(raw).filter(|k| k.service_id == ctx.service_id) else {
            return Err(gerr(401, "bad_server_key", "this key is not this App's server key"));
        };
        let grant = k2_core::app_grace::keys::AppKeyGrant::from_stored(key.grant_json.as_deref());
        if !grant.has(k2_core::app_grace::keys::SCOPE_SERVICE) {
            return Err(gerr(401, "bad_server_key", "the App key needs the service scope"));
        }
        if matches!(ctx.ingress, Ingress::Public { .. }) && !ctx.cfg.service_key_public {
            return Err(gerr(401, "server_key_on_public_host", "the server key works on the App's gateway link only"));
        }
        return Ok(Some((key.id, grant)));
    }
    if apikey != door::publishable_key(&ctx.service_id) {
        return Err(gerr(401, "bad_api_key", "the apikey is not this App's publishable key"));
    }
    Ok(None)
}

fn body_json(req: &DoorRequest) -> Result<Value, DoorAnswer> {
    if req.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(&req.body) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err(gerr(400, "bad_json", "the body is a JSON object")),
    }
}

fn text<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

// ── the routes ───────────────────────────────────────────────────────

/// One `/auth/v1` request. Blocking work runs off the async threads.
pub async fn handle(req: DoorRequest, ctx: &Ctx) -> DoorAnswer {
    let c = ctx.clone();
    door::blocking(move || route(&req, &c)).await.unwrap_or_else(|| gerr(500, "unexpected_failure", "worker join"))
}

fn route(req: &DoorRequest, ctx: &Ctx) -> DoorAnswer {
    let key = match key_of(req, ctx) {
        Ok(k) => k,
        Err(a) => return a,
    };
    let rest = req.path.strip_prefix(door::AUTH).unwrap_or("");
    let q: HashMap<String, String> = query_pairs(&req.query).into_iter().collect();
    let m = req.method.as_str();
    match (m, rest) {
        ("GET", "/user") => get_user(req, ctx),
        ("PUT", "/user") => update_user(req, ctx),
        ("POST", "/token") => match q.get("grant_type").map(String::as_str) {
            Some("password") => sign_in(req, ctx),
            Some("refresh_token") => refresh(req, ctx),
            Some("pkce") => pkce(req, ctx),
            Some(g) => gerr(400, "unsupported_grant_type", format!("grant_type {g} is not supported by K2")),
            None => gerr(400, "unsupported_grant_type", "grant_type is required"),
        },
        ("POST", "/verify") => verify_post(req, ctx),
        ("GET", "/verify") => verify_link(req, ctx, &q),
        ("POST", "/logout") => logout(req, ctx, q.get("scope").map(String::as_str).unwrap_or("global")),
        (_, r) if r == "/admin" || r.starts_with("/admin/") => match key {
            Some((key_id, grant)) => super::auth_admin::route(req, ctx, &key_id, &grant, &q),
            None => gerr(403, "not_admin", "User not allowed: the admin routes take the App's server key (the service scope)"),
        },
        ("POST", "/signup") | ("POST", "/otp") | ("POST", "/magiclink") | ("POST", "/recover") | ("GET", "/authorize") | ("POST", "/factors") | ("GET", "/settings") => {
            gerr(404, "not_supported", format!("{m} /auth/v1{rest} is not supported by K2 (k2 study app-rest)"))
        }
        _ => gerr(404, "not_found", format!("{m} /auth/v1{rest}: no such route")),
    }
}

fn get_user(req: &DoorRequest, ctx: &Ctx) -> DoorAnswer {
    match holder(req, ctx) {
        Err(a) => a,
        Ok(None) => gerr(401, "no_authorization", "This endpoint requires a valid Bearer token"),
        Ok(Some(Holder::Session(pass))) => match pass.principal_id.as_deref().and_then(principal_by_id) {
            Some(p) => ok(user_json(&ctx.service_id, &p)),
            None => gerr(403, "user_not_found", "User from sub claim in JWT does not exist"),
        },
        Ok(Some(Holder::Pending(_, p))) => match principal_by_id(&p.principal_id) {
            Some(u) => ok(user_json(&ctx.service_id, &u)),
            None => gerr(403, "user_not_found", "User from sub claim in JWT does not exist"),
        },
    }
}

fn update_user(req: &DoorRequest, ctx: &Ctx) -> DoorAnswer {
    let v = match body_json(req) {
        Ok(v) => v,
        Err(a) => return a,
    };
    if v.get("email").is_some() || v.get("phone").is_some() {
        return gerr(422, "not_supported", "changing the email or phone through /auth/v1 is not supported by K2: the App's owner changes it");
    }
    if v.get("data").is_some_and(|d| d.as_object().is_some_and(|o| !o.is_empty())) {
        return gerr(422, "not_supported", "user_metadata is not stored by K2 yet");
    }
    let password = text(&v, "password");
    if let Some(pw) = password {
        if skin::password_too_long(pw) {
            return gerr(422, "weak_password", "a password is at most 256 bytes");
        }
        if pw.len() < skin::PUBLIC_PASSWORD_MIN_LEN {
            return gerr(422, "weak_password", format!("Password should be at least {} characters.", skin::PUBLIC_PASSWORD_MIN_LEN));
        }
    }
    match holder(req, ctx) {
        Err(a) => a,
        Ok(None) => gerr(401, "no_authorization", "This endpoint requires a valid Bearer token"),
        Ok(Some(Holder::Session(pass))) => {
            let Some(p) = pass.principal_id.as_deref().and_then(principal_by_id) else {
                return gerr(403, "user_not_found", "User from sub claim in JWT does not exist");
            };
            if let Some(pw) = password {
                // The person chose it: their other sessions end, this one stays.
                if let Err(e) = skin::set_principal_password_staying(&p.username, pw, &pass.id) {
                    return gerr(422, "weak_password", e);
                }
            }
            ok(user_json(&ctx.service_id, &p))
        }
        Ok(Some(Holder::Pending(raw, p))) => {
            let Some(pw) = password else {
                return ok(principal_by_id(&p.principal_id).map(|u| user_json(&ctx.service_id, &u)).unwrap_or(Value::Null));
            };
            match skin::invites::consume(&p.link_token, pw) {
                Ok(skin::invites::Consume::Ok) => {}
                Ok(skin::invites::Consume::BadToken) => return gerr(403, "otp_expired", "Email link is invalid or has expired"),
                Err(e) => return gerr(422, "weak_password", e),
            }
            let minted = skin::test_guests::with_link_login(Some(&ctx.service_id), || {
                let hash = store::password_hash_of(&p.principal_id)?.ok_or_else(|| "no password stored".to_string())?;
                store::mint(&p.principal_id, &ctx.service_id, &hash, None)
            });
            let Ok(store::Minted::Plain(s)) = minted else {
                // The password is set; no session where a login wouldn't get one.
                return ok(principal_by_id(&p.principal_id).map(|u| user_json(&ctx.service_id, &u)).unwrap_or(Value::Null));
            };
            skin::profile::record_session(&s.meta.id, skin::profile::VIA_PASSWORD, Some("password link"));
            let exp = skin::resolve_skin_token(&s.raw).and_then(|x| x.expires_at);
            if let Some(e) = pendings().lock().unwrap_or_else(|e| e.into_inner()).get_mut(&digest(&raw)) {
                e.session = Some((s.raw.clone(), exp));
            }
            let user = principal_by_id(&p.principal_id).map(|u| user_json(&ctx.service_id, &u)).unwrap_or(Value::Null);
            let a = ok(user);
            match ctx.ingress {
                Ingress::Public { .. } => {
                    let (claims, exp) = pass_facts(ctx, &s.raw);
                    with_cookies(a, Some((&s.raw, &door::wrap_pass(&s.raw, &claims), exp - now_secs())))
                }
                Ingress::Link => a,
            }
        }
    }
}

fn sign_in(req: &DoorRequest, ctx: &Ctx) -> DoorAnswer {
    let v = match body_json(req) {
        Ok(v) => v,
        Err(a) => return a,
    };
    if v.get("phone").is_some() {
        return gerr(400, "phone_provider_disabled", "Phone logins are not supported by K2");
    }
    let (Some(who), Some(password)) = (text(&v, "email"), v.get("password").and_then(Value::as_str)) else {
        return gerr(400, "validation_failed", "send email and password");
    };
    let bad = || gerr(400, "invalid_credentials", "Invalid login credentials");
    let username = match skin::login_principal_username(who) {
        Ok(Some(u)) => u,
        Ok(None) => {
            // Same cost as a wrong password (no account oracle).
            skin::reset_miss_dummy();
            return bad();
        }
        Err(e) => return gerr(500, "unexpected_failure", e),
    };
    match skin::check_and_record_login_hashed(&username, password) {
        (SkinLoginOutcome::Ok(p), Some(hash)) => {
            let minted = skin::test_guests::with_link_login(Some(&ctx.service_id), || store::mint(&p.id, &ctx.service_id, &hash, None));
            match minted {
                Ok(store::Minted::Plain(s)) => {
                    skin::profile::record_session(&s.meta.id, skin::profile::VIA_PASSWORD, Some("supabase-js"));
                    let (claims, exp) = pass_facts(ctx, &s.raw);
                    session_answer(ctx, &s.raw, claims, exp, None, user_json(&ctx.service_id, &p))
                }
                Ok(store::Minted::Device(d)) => {
                    let (claims, exp) = pass_facts(ctx, &d.session.raw);
                    session_answer(ctx, &d.session.raw, claims, exp, Some(&d.refresh), user_json(&ctx.service_id, &p))
                }
                Err(e) if e == skin::HOST_LOGIN_DENIED => bad(),
                Err(e) => gerr(500, "unexpected_failure", e),
            }
        }
        (SkinLoginOutcome::MustChange(_), _) => gerr(
            403,
            "password_change_required",
            "someone else set this password: the person chooses their own first (the App's set-password page, or the K2 sign-in door's change-password)",
        ),
        (SkinLoginOutcome::LockedOut { retry_after_secs }, _) => {
            gerr(429, "over_request_rate_limit", format!("too many wrong passwords for this person: try again in {retry_after_secs} seconds")).header("Retry-After", retry_after_secs.to_string())
        }
        _ => bad(),
    }
}

fn refresh(req: &DoorRequest, ctx: &Ctx) -> DoorAnswer {
    let v = match body_json(req) {
        Ok(v) => v,
        Err(a) => return a,
    };
    let Some(raw) = text(&v, "refresh_token") else {
        return gerr(400, "refresh_token_not_found", "Invalid Refresh Token: Refresh Token Not Found");
    };
    // A plain session's refresh token is its own pass: the same session
    // again while it lives (it never outlives the pass).
    if raw.starts_with(skin::SKIN_KEY_PREFIX) {
        let Some(s) = session_of(raw, &ctx.service_id) else {
            return gerr(400, "refresh_token_not_found", "Invalid Refresh Token: Refresh Token Not Found");
        };
        let (claims, exp) = pass_facts(ctx, raw);
        let user = s.principal_id.as_deref().and_then(principal_by_id).map(|p| user_json(&ctx.service_id, &p)).unwrap_or(Value::Null);
        return session_answer(ctx, raw, claims, exp, None, user);
    }
    if raw.starts_with(PENDING_PREFIX) {
        let Some(p) = pending(raw, &ctx.service_id) else {
            return gerr(400, "refresh_token_not_found", "Invalid Refresh Token: Refresh Token Not Found");
        };
        let user = principal_by_id(&p.principal_id).map(|u| user_json(&ctx.service_id, &u)).unwrap_or(Value::Null);
        return session_answer(ctx, raw, pending_claims(&ctx.service_id, &p), p.exp, None, user);
    }
    match app_login::refresh(raw, Some(&ctx.service_id)) {
        Ok(app_login::Refresh::Ok(d)) => {
            let p = skin::resolve_skin_token(&d.session.raw).and_then(|x| x.principal_id).and_then(|id| principal_by_id(&id));
            let (claims, exp) = pass_facts(ctx, &d.session.raw);
            session_answer(ctx, &d.session.raw, claims, exp, Some(&d.refresh), p.map(|p| user_json(&ctx.service_id, &p)).unwrap_or(Value::Null))
        }
        Ok(app_login::Refresh::Reused) => gerr(400, "refresh_token_already_used", "Invalid Refresh Token: Already Used"),
        Ok(app_login::Refresh::Invalid) => gerr(400, "refresh_token_not_found", "Invalid Refresh Token: Refresh Token Not Found"),
        Err(e) => gerr(500, "unexpected_failure", e),
    }
}

fn pkce(req: &DoorRequest, ctx: &Ctx) -> DoorAnswer {
    let v = match body_json(req) {
        Ok(v) => v,
        Err(a) => return a,
    };
    let (Some(code), Some(verifier)) = (text(&v, "auth_code"), text(&v, "code_verifier")) else {
        return gerr(400, "validation_failed", "send auth_code and code_verifier");
    };
    let body = json!({ "code": code, "verifier": verifier }).to_string();
    let r = crate::apps::auth_door::exchange_response(&ctx.service_id, body.as_bytes());
    let out: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        return gerr(400, "flow_state_not_found", "invalid flow state, no valid flow state found: sign in again");
    }
    let Some(pass) = out["token"].as_str() else {
        return gerr(500, "unexpected_failure", "the exchange gave no pass");
    };
    let p = out["principalId"].as_str().and_then(principal_by_id);
    let (claims, exp) = pass_facts(ctx, pass);
    session_answer(ctx, pass, claims, exp, None, p.map(|p| user_json(&ctx.service_id, &p)).unwrap_or(Value::Null))
}

/// A mailed link's token for this App: who it is for (not used yet).
fn link_target(ctx: &Ctx, token: &str) -> Option<store::TokenTarget> {
    store::token_target(token).ok().flatten().filter(|t| t.app == ctx.service_id && !t.owner_reset && token.starts_with(skin::invites::INVITE_PREFIX))
}

fn pending_session(ctx: &Ctx, t: &store::TokenTarget, token: &str) -> (String, Value, Pending) {
    let p = Pending {
        app: ctx.service_id.clone(),
        link_token: token.to_string(),
        principal_id: t.principal_id.clone(),
        made: Instant::now(),
        exp: now_secs() + PENDING_TTL.as_secs() as i64,
        session: None,
    };
    let raw = new_pending(p.clone());
    let user = principal_by_id(&t.principal_id).map(|p| user_json(&ctx.service_id, &p)).unwrap_or(Value::Null);
    (raw, user, p)
}

fn verify_post(req: &DoorRequest, ctx: &Ctx) -> DoorAnswer {
    let v = match body_json(req) {
        Ok(v) => v,
        Err(a) => return a,
    };
    let kind = text(&v, "type").unwrap_or("");
    if !matches!(kind, "invite" | "recovery") {
        return gerr(400, "not_supported", format!("verifyOtp type {kind} is not supported by K2 (invite and recovery are)"));
    }
    let Some(token) = text(&v, "token_hash").or_else(|| text(&v, "token")) else {
        return gerr(400, "validation_failed", "send token_hash");
    };
    let Some(t) = link_target(ctx, token) else {
        return gerr(403, "otp_expired", "Email link is invalid or has expired");
    };
    let (raw, user, p) = pending_session(ctx, &t, token);
    session_answer(ctx, &raw, pending_claims(&ctx.service_id, &p), p.exp, None, user)
}

/// May a mailed link send the browser to `url`? The App's own host (the
/// request's Host), the App's public origins, or a listed origin.
fn redirect_allowed(req: &DoorRequest, ctx: &Ctx, url: &str) -> bool {
    let Some((scheme, host, port)) = skin::invites::url_host(url) else { return false };
    if scheme != "https" && !(host == "localhost" || host == "127.0.0.1") {
        return false;
    }
    let origin = match port {
        Some(p) => format!("{scheme}://{host}:{p}"),
        None => format!("{scheme}://{host}"),
    };
    if req.header("host").is_some_and(|h| h.eq_ignore_ascii_case(&origin.split("://").nth(1).unwrap_or(""))) {
        return true;
    }
    ctx.cfg.origins.iter().any(|o| o.trim_end_matches('/').eq_ignore_ascii_case(&origin)) || super::auth_admin::app_origins(&ctx.service_id).iter().any(|o| o.eq_ignore_ascii_case(&origin))
}

fn verify_link(req: &DoorRequest, ctx: &Ctx, q: &HashMap<String, String>) -> DoorAnswer {
    let redirect = q.get("redirect_to").cloned().unwrap_or_default();
    if redirect.is_empty() || !redirect_allowed(req, ctx, &redirect) {
        return gerr(400, "validation_failed", "redirect_to must be a page of this App");
    }
    let token = q.get("token").map(String::as_str).unwrap_or("");
    let Some(t) = link_target(ctx, token) else {
        let sep = if redirect.contains('#') { '&' } else { '#' };
        return DoorAnswer::new(303).header(
            "Location",
            format!("{redirect}{sep}error=access_denied&error_code=otp_expired&error_description=Email+link+is+invalid+or+has+expired"),
        );
    };
    let (raw, _, p) = pending_session(ctx, &t, token);
    let a = DoorAnswer::new(303).header("Location", redirect);
    match ctx.ingress {
        Ingress::Public { .. } => with_cookies(a, Some((&raw, &door::wrap_pass(&raw, &pending_claims(&ctx.service_id, &p)), PENDING_TTL.as_secs() as i64))),
        // On the link a click can't happen; answer the pass for server code.
        Ingress::Link => a.header("x-k2-pending-pass", raw),
    }
}

fn logout(req: &DoorRequest, ctx: &Ctx, scope: &str) -> DoorAnswer {
    let clear = |a: DoorAnswer| match ctx.ingress {
        Ingress::Public { .. } => with_cookies(a, None),
        Ingress::Link => a,
    };
    if !matches!(scope, "local" | "global" | "others") {
        return gerr(400, "validation_failed", "scope is local, global or others");
    }
    match holder(req, ctx) {
        Err(_) | Ok(None) => clear(DoorAnswer::new(204)),
        Ok(Some(Holder::Pending(raw, _))) => {
            pendings().lock().unwrap_or_else(|e| e.into_inner()).remove(&digest(&raw));
            clear(DoorAnswer::new(204))
        }
        Ok(Some(Holder::Session(pass))) => {
            let pid = pass.principal_id.clone().unwrap_or_default();
            if scope != "local" {
                // Every other session of the person in this App: browser
                // sessions and device sign-ins.
                let rows = skin::profile::sessions_for(&pid, &ctx.service_id, &pass.id).unwrap_or_default();
                for r in rows.iter().filter(|r| !r.current) {
                    let _ = if r.kind == "device" {
                        app_login::revoke_own(&pid, &ctx.service_id, &r.key)
                    } else {
                        skin::profile::revoke_browser_session(&pid, &ctx.service_id, &r.key)
                    };
                }
            }
            if scope == "others" {
                return DoorAnswer::new(204);
            }
            if let Some(d) = app_login::device_for_session(&pass.id).ok().flatten() {
                let _ = app_login::revoke_own(&pid, &ctx.service_id, &d.id);
            }
            let _ = skin::revoke_token(&pass.id);
            clear(DoorAnswer::new(204))
        }
    }
}

#[cfg(test)]
#[path = "auth_door_v1_tests.rs"]
mod tests;
