//! The compat doors' front (prd-app-supabase-compat-v1 §4): where a door
//! request is noticed, who it is from, and the answer's way back.
//!
//! **Where.** `/rest/v1/…` and `/auth/v1/…` (and lane D's `/storage/v1/…`)
//! on two kinds of connection, and only for an App whose compat is on
//! ([`super::door_config::for_app`]):
//! - **the App's public host** (`<app>.<box>.k2.dev`, its custom names, its
//!   front port): `apps::router` reads every request head on the App's
//!   rewrite path, as it does for the transfer and call doors (a divert,
//!   vs-live §3 item 7; not the sign-in door's hook, which caps bodies at
//!   8 KiB and closes after each answer). A raw-splice App
//!   (`host.clientIp: false`) has no doors. The door keeps the connection
//!   alive: the next request is the door's again or goes to the App.
//! - **the App's gateway link** (`K2_GATEWAY_URL`, server code in the
//!   wall): the dispatcher hands the door its requests before the link's
//!   gate, and the door does the gate's work itself (suspend, quota, audit).
//!
//! **Who** ([`resolve`]; PRD §4's table):
//!
//! | credential | where | runs as |
//! |---|---|---|
//! | the App's session cookie (`k2_skin_session`, an App session of this App) | public host | `authenticated` |
//! | `Authorization: Bearer` a device pass (a device sign-in's session) | public host | `authenticated` |
//! | `Authorization: Bearer` a web session's pass | public host | **refused** (`web_pass_as_bearer`): only the cookie carries a web session there, so the token never sits in the browser |
//! | `Authorization: Bearer` an App session's pass, web or device | gateway link | `authenticated` (server code read it from the cookie) |
//! | `Authorization: Bearer` a third-party ID token (`a.b.c`) | either | as lane A's verifier says ([`set_third_party_verifier`]) |
//! | the App key with the `service` scope, as `apikey` or bearer | gateway link; public host only with `service-key-public` | `service_role` |
//! | the publishable key alone (`k2pk_<service id>`) | either | `anon` when the anon lane is on, else 401 |
//!
//! Every request carries an `apikey` (supabase-js always sends one): the
//! publishable key or the server key. A publishable key for another App is
//! refused. The bearer supabase-js sends when nobody is signed in is the
//! `apikey` itself; it counts as no bearer.
//!
//! **Origins.** A request from a browser (`Origin` set) must come from the
//! host it was sent to or an origin the App lists; any other origin is 403
//! `origin_not_allowed`, before credentials are read. A cookie write needs
//! an `Origin` at all (browsers always send one on a cross-origin-capable
//! write; its absence on a cookie write means a client we can't vouch
//! for). Listed origins get `Access-Control-Allow-Origin` echoed with
//! credentials, and the door answers their preflight.

use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::skin::{SkinPass, SkinPrincipal};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use super::door_config::{self, DoorConfig};
use super::http::{self, DoorAnswer, DoorRequest, ReadFail};
use super::pool::{Caller, DbRole};
use crate::apps::router::FirstRequest;
use crate::client_ip::{LocalAnswer, Replay};

/// The data door's path.
pub const REST: &str = "/rest/v1";
/// The auth door's path.
pub const AUTH: &str = "/auth/v1";
/// The storage door's path (lane D's).
pub const STORAGE: &str = "/storage/v1";

/// Which door a path is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    Rest,
    Auth,
    Storage,
}

fn under(path: &str, prefix: &str) -> bool {
    path == prefix || path.strip_prefix(prefix).is_some_and(|r| r.starts_with('/'))
}

/// The door a request path (no query) is on, if any.
pub fn door_of(path: &str) -> Option<Door> {
    if under(path, REST) {
        Some(Door::Rest)
    } else if under(path, AUTH) {
        Some(Door::Auth)
    } else if under(path, STORAGE) {
        Some(Door::Storage)
    } else {
        None
    }
}

/// The door of the request whose head starts `bytes`, when the head is
/// complete.
pub fn door_of_head(bytes: &[u8]) -> Option<(Door, String, usize)> {
    let (head, len) = http::complete_head(bytes)?;
    let target = head.lines().next()?.split_whitespace().nth(1)?;
    let path = target.split_once('?').map_or(target, |(p, _)| p);
    door_of(path).map(|d| (d, head.clone(), len))
}

// ── who ──────────────────────────────────────────────────────────────

/// Where a request came in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ingress {
    /// The App's public host (cookie lane; the visitor K2 derived).
    Public { visitor: Option<IpAddr> },
    /// The App's gateway link (server code in the wall).
    Link,
}

/// One request's context.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub cfg: DoorConfig,
    /// The App's published service id: an App session's `app`, the App
    /// key's service, the name in the publishable key.
    pub service_id: String,
    pub ingress: Ingress,
}

/// The App session cookie a browser carries (the server SDK's default name,
/// `sdk/server/src/client.ts` `guestFrom`).
pub const SESSION_COOKIE: &str = "k2_skin_session";

/// The publishable key's prefix (PRD §4): not a secret, it names the App
/// and grants nothing by itself. It replaces Supabase's anon key.
pub const PUBLISHABLE_PREFIX: &str = "k2pk_";

/// The publishable key of a service id.
pub fn publishable_key(service_id: &str) -> String {
    format!("{PUBLISHABLE_PREFIX}{service_id}")
}

pub fn is_publishable(raw: &str) -> bool {
    raw.starts_with(PUBLISHABLE_PREFIX)
}

/// `app_id`'s door config and service id, when compat is on for it.
/// Blocking (K2's records).
pub fn config_for(app_id: &str) -> Option<(DoorConfig, String)> {
    #[cfg(test)]
    {
        if let Some(c) = test_configs().lock().unwrap_or_else(|e| e.into_inner()).get(app_id) {
            return Some(c.clone());
        }
    }
    let cfg = door_config::for_app(app_id)?;
    let service_id = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::deploy::store::get_app(&conn, app_id).ok().flatten()?.service_id
    };
    Some((cfg, service_id))
}

#[cfg(test)]
fn test_configs() -> &'static Mutex<std::collections::HashMap<String, (DoorConfig, String)>> {
    static T: OnceLock<Mutex<std::collections::HashMap<String, (DoorConfig, String)>>> = OnceLock::new();
    T.get_or_init(Default::default)
}

/// Tests: App `app_id`'s doors are on with `c` for `service_id` (`None`: off).
#[cfg(test)]
pub fn set_test_config(app_id: &str, c: Option<(DoorConfig, String)>) {
    let mut g = test_configs().lock().unwrap_or_else(|e| e.into_inner());
    match c {
        Some(c) => g.insert(app_id.to_string(), c),
        None => g.remove(app_id),
    };
}

/// Who a request is, past the door.
#[derive(Debug, Clone, PartialEq)]
pub enum Who {
    Anon,
    /// An App user (a K2 session of this App).
    Person { pass: SkinPass, device: bool },
    /// The App key with the `service` scope.
    Service { key_id: String },
    /// A third-party token lane A's verifier accepted.
    ThirdParty { principal_id: Option<String> },
}

/// The caller and who it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub caller: Caller,
    pub who: Who,
}

impl Resolved {
    /// The audit actor.
    pub fn actor(&self) -> String {
        match &self.who {
            Who::Anon => "anon".into(),
            Who::Person { pass, .. } => format!("guest:{}", pass.principal_id.as_deref().unwrap_or(&pass.username)),
            Who::Service { .. } => "app-key".into(),
            Who::ThirdParty { principal_id } => format!("guest:{}", principal_id.as_deref().unwrap_or("third-party")),
        }
    }
}

/// A refusal in PostgREST's error shape with a K2 code.
pub fn refuse(status: u16, code: &str, message: impl Into<String>, hint: Option<&str>) -> DoorAnswer {
    let a = DoorAnswer::pgrst(status, code, message, None, hint.map(str::to_string));
    if status == 401 {
        a.header("WWW-Authenticate", "Bearer")
    } else {
        a
    }
}

/// Lane A's third-party token check (PRD §6): the claims for a request
/// that carried `token`, or the refusal `(status, code, message)`. The
/// claims must hold `role` (`authenticated`) and, when the email maps to an
/// App user, `sub` as the setting says.
pub type ThirdPartyVerifier = fn(&DoorConfig, &str) -> Result<(Value, Option<String>), (u16, String, String)>;

fn verifier() -> &'static OnceLock<ThirdPartyVerifier> {
    static V: OnceLock<ThirdPartyVerifier> = OnceLock::new();
    &V
}

#[cfg(test)]
thread_local! {
    /// Tests: this thread's issuer check (the real one is process-wide).
    pub static TEST_VERIFIER: std::cell::Cell<Option<ThirdPartyVerifier>> = const { std::cell::Cell::new(None) };
}

/// Lane A registers its issuer check here (once, at boot). Until then a
/// third-party token is refused with `issuer_unknown`.
pub fn set_third_party_verifier(f: ThirdPartyVerifier) {
    let _ = verifier().set(f);
}

/// Check a third-party ID token for `cfg`'s App with the registered issuer
/// check: `(claims, App user id)` or `(status, code, message)`. Every door
/// that takes these tokens (`/rest/v1`, `/auth/v1`, `/storage/v1`) asks
/// here, so they agree. Blocking (K2's stores, the key cache).
pub fn third_party_check(cfg: &DoorConfig, token: &str) -> Result<(Value, Option<String>), (u16, String, String)> {
    #[cfg(test)]
    let test_f = TEST_VERIFIER.with(|t| t.get());
    #[cfg(not(test))]
    let test_f: Option<ThirdPartyVerifier> = None;
    let Some(f) = test_f.or_else(|| verifier().get().copied()) else {
        return Err((401, "issuer_unknown".into(), concat!("this App trusts no third-party token issuer (", k2_core::cli_name!(), " app identity issuer add <app> …)").into()));
    };
    f(cfg, token)
}

/// supabase-js decodes the access token as a JWT in `setSession` (and
/// `getClaims`; S0's answer, PRD §18): so the auth door hands server code
/// its pass in a JWT's shape, `<header>.<claims>.<pass>`, header
/// `{"alg":"HS256","typ":"JWT","kid":"k2pass"}` (HS256: supabase-js asks
/// the server, `GET /auth/v1/user`, instead of checking a signature
/// itself). The third part is the pass; the claims are for reading only
/// and the door never trusts them. [`unwrap_pass`] takes the pass back out.
pub fn wrap_pass(pass: &str, claims: &Value) -> String {
    use base64::Engine as _;
    let b = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header = json!({"alg": "HS256", "typ": "JWT", "kid": "k2pass"}).to_string();
    format!("{}.{}.{pass}", b.encode(header), b.encode(claims.to_string()))
}

/// The pass inside a [`wrap_pass`] token, else the token as it came.
pub fn unwrap_pass(token: &str) -> &str {
    let mut parts = token.splitn(3, '.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(_), Some(_), Some(last)) if last.starts_with(k2_core::skin::SKIN_KEY_PREFIX) || last.starts_with(PENDING_PREFIX) => last,
        _ => token,
    }
}

/// A pending pass's prefix: an invite or recovery link's pass that only sets
/// the password (`auth_door_v1`).
pub const PENDING_PREFIX: &str = "k2vrf_";

/// `exp` for a pass that never ends by itself (2100-01-01).
pub const NO_EXPIRY: i64 = 4_102_444_800;

/// The claims a wrapped pass carries: what K2 knows of the session, and
/// nothing a reader could take for more.
pub fn pass_claims(app_id: &str, sub: &str, email: &str, exp: Option<i64>) -> Value {
    json!({ "sub": sub, "role": "authenticated", "aud": "authenticated", "email": email, "iss": issuer(app_id), "exp": exp.unwrap_or(NO_EXPIRY) })
}

/// [`pass_claims`] for a live session pass, read from the user store now.
pub fn session_claims(app_id: &str, pass: &SkinPass) -> Value {
    let pid = pass.principal_id.clone().unwrap_or_default();
    let email = principal(&pid).and_then(|p| p.email).unwrap_or_default();
    pass_claims(app_id, &pid, &email, pass.expires_at)
}

/// The claims inside a [`wrap_pass`] token (`None`: a bare pass).
pub fn wrapped_claims(token: &str) -> Option<Value> {
    use base64::Engine as _;
    if unwrap_pass(token) == token {
        return None;
    }
    let mid = token.split('.').nth(1)?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(mid).ok()?;
    Some(serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

/// Is `token` a bare pass, or wrapped with exactly the claims K2 writes for
/// it (`want`)? supabase-js's `getClaims` trusts a token's claims once
/// `GET /auth/v1/user` accepts it, so a token whose claims someone edited
/// (another `sub`, an `app_metadata` K2 never wrote) is refused outright.
pub fn claims_match(token: &str, want: &Value) -> bool {
    match wrapped_claims(token) {
        None => true,
        Some(c) => &c == want,
    }
}

/// A JWT's shape: three base64url parts.
pub fn looks_like_jwt(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}

/// The claims' `iss` for K2 sessions of this App.
///
/// CONTRACT NOTE: PRD §5.2 leaves `iss` open; lane A's `claims.rs` is the
/// one place that should build these claims. Until it lands the doors
/// build them here, to the §5.2 shape.
pub fn issuer(app_id: &str) -> String {
    format!("k2:app:{app_id}")
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Lane A's claims for the pass's App user (PRD §5.2, with its
/// `app_metadata` and `user_metadata`): the same object the brokered lane
/// and `k2.set_claims` use. `None` when the user can't be read.
pub fn user_claims(app_id: &str, pass: &SkinPass) -> Option<Value> {
    let pid = pass.principal_id.as_deref()?;
    let user = k2_core::skin::compat::app_user(&pass.app, pid).ok().flatten()?;
    Some(super::claims::for_user(app_id, &user, Some(&pass.id)))
}

/// The claims for a K2 session of this App (PRD §5.2) when its App user
/// can't be read ([`user_claims`] first).
pub fn person_claims(app_id: &str, pass: &SkinPass, p: Option<&SkinPrincipal>) -> Value {
    let now = now_secs();
    json!({
        "sub": pass.principal_id.clone().unwrap_or_default(),
        "role": "authenticated",
        "email": p.and_then(|p| p.email.clone()).unwrap_or_default(),
        "phone": "",
        "iss": issuer(app_id),
        "aud": "authenticated",
        "session_id": pass.id,
        "app_metadata": {
            "k2_app": app_id,
            "k2_role": p.and_then(|p| p.role_name.clone()),
            "provider": "email",
            "providers": ["email"],
        },
        "user_metadata": {},
        "is_anonymous": false,
        "iat": now,
        "exp": pass.expires_at.unwrap_or(now + 3600),
    })
}

/// The claims for the server key (Supabase's service key carries these).
pub fn service_claims(app_id: &str) -> Value {
    json!({ "role": "service_role", "iss": issuer(app_id), "iat": now_secs(), "app_metadata": { "k2_app": app_id, "provider": "k2", "providers": ["k2"] } })
}

/// The claims for the anon lane.
pub fn anon_claims(app_id: &str) -> Value {
    json!({ "role": "anon", "iss": issuer(app_id), "iat": now_secs(), "app_metadata": { "k2_app": app_id, "provider": "k2", "providers": ["k2"] } })
}

/// An App user by principal id (cached for two seconds: a role change shows
/// on the next request after that).
pub fn principal(id: &str) -> Option<SkinPrincipal> {
    type Cache = Option<(Instant, Vec<SkinPrincipal>)>;
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    let mut g = C.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    let fresh = matches!(&*g, Some((at, _)) if at.elapsed() < Duration::from_secs(2));
    if !fresh || cfg!(test) {
        *g = Some((Instant::now(), k2_core::skin::list_principals().ok()?));
    }
    g.as_ref()?.1.iter().find(|p| p.id == id).cloned()
}

/// An App key with the `service` scope for this App, or `None`.
fn service_key(service_id: &str, raw: &str) -> Option<String> {
    let key = k2_core::api_keys::resolve_app_key(raw)?;
    if key.service_id != service_id {
        return None;
    }
    let grant = k2_core::app_grace::keys::AppKeyGrant::from_stored(key.grant_json.as_deref());
    grant.has(k2_core::app_grace::keys::SCOPE_SERVICE).then_some(key.id)
}

fn person(app_id: &str, pass: SkinPass, device: bool) -> Resolved {
    let claims = user_claims(app_id, &pass).unwrap_or_else(|| {
        let p = pass.principal_id.as_deref().and_then(principal);
        person_claims(app_id, &pass, p.as_ref())
    });
    Resolved { caller: Caller { role: DbRole::Authenticated, claims }, who: Who::Person { pass, device } }
}

/// Who `req` is, or the refusal. Blocking (K2's stores): run it off the
/// async threads.
pub fn resolve(req: &DoorRequest, ctx: &Ctx) -> Result<Resolved, DoorAnswer> {
    let cfg = &ctx.cfg;
    let svc = ctx.service_id.as_str();
    let app = ctx.cfg.app_id.as_str();
    let link = ctx.ingress == Ingress::Link;
    let Some(apikey) = req.header("apikey").filter(|k| !k.is_empty()) else {
        return Err(refuse(401, "no_api_key", "No API key found in request", Some(concat!("send the App's publishable key (", k2_core::cli_name!(), " app compat status) as the apikey header"))));
    };
    let raw_bearer = req.bearer().filter(|b| *b != apikey && !is_publishable(b));
    let bearer = raw_bearer.map(unwrap_pass);
    // The server key, as apikey or bearer.
    let key_raw = if k2_core::skin::is_api_key_prefix(apikey) {
        Some(apikey)
    } else {
        bearer.filter(|b| k2_core::skin::is_api_key_prefix(b))
    };
    if let Some(raw) = key_raw {
        let Some(key_id) = service_key(svc, raw) else {
            return Err(refuse(401, "bad_server_key", "this key is not this App's server key", Some(concat!("the App key needs the service scope (", k2_core::cli_name!(), " app key mint --scope service)"))));
        };
        if !link && !cfg.service_key_public {
            return Err(refuse(
                401,
                "server_key_on_public_host",
                "the server key works on the App's gateway link only",
                Some(concat!("call K2_GATEWAY_URL from server code, or the owner runs ", k2_core::cli_name!(), " app compat set <app> service-key-public on")),
            ));
        }
        return Ok(Resolved { caller: Caller { role: DbRole::Service, claims: service_claims(app) }, who: Who::Service { key_id } });
    }
    if !is_publishable(apikey) {
        return Err(refuse(401, "bad_api_key", "the apikey is neither this App's publishable key nor its server key", None));
    }
    if apikey != publishable_key(svc) {
        return Err(refuse(401, "wrong_app_key", "this publishable key names another App", Some(concat!("use this App's key (", k2_core::cli_name!(), " app compat status)"))));
    }
    if let Some(b) = bearer {
        if looks_like_jwt(b) {
            return match third_party_check(cfg, b) {
                Ok((claims, principal_id)) => {
                    let role = match claims.get("role").and_then(Value::as_str) {
                        Some("authenticated") => DbRole::Authenticated,
                        _ => return Err(refuse(500, "bad_claims", "the issuer check gave claims without the authenticated role", None)),
                    };
                    Ok(Resolved { caller: Caller { role, claims }, who: Who::ThirdParty { principal_id } })
                }
                Err((status, code, message)) => Err(refuse(status, &code, message, None)),
            };
        }
        if b.starts_with(PENDING_PREFIX) {
            // A recovery or invite link's pass whose password was set: the
            // session it became, for server code (the link) only.
            let Some(raw) = super::auth_door_v1::upgraded_pass(raw_bearer.unwrap_or(b), svc, app).filter(|_| link) else {
                return Err(refuse(401, "pass_invalid", "this pass only sets a password (PUT /auth/v1/user); sign in again", None));
            };
            let Some(pass) = k2_core::skin::resolve_skin_token(&raw).filter(|p| p.session && p.app == svc) else {
                return Err(refuse(401, "pass_invalid", "the pass is not a live session of this App", Some("sign in again")));
            };
            return Ok(person(app, pass, false));
        }
        if !k2_core::skin::is_skin_token(b) {
            return Err(refuse(401, "bad_credential", "the bearer is not a credential this door takes", None));
        }
        let Some(pass) = k2_core::skin::resolve_skin_token(b).filter(|p| p.session && p.app == svc) else {
            return Err(refuse(401, "pass_invalid", "the pass is not a live session of this App", Some("sign in again")));
        };
        if !claims_match(raw_bearer.unwrap_or(b), &session_claims(app, &pass)) {
            return Err(refuse(401, "bad_jwt", concat!("the token's claims are not the ones ", k2_core::product_name!(), " wrote for this session"), Some("sign in again")));
        }
        let device = k2_core::skin::app_login::device_for_session(&pass.id).ok().flatten().is_some();
        if !link && !device {
            return Err(refuse(
                401,
                "web_pass_as_bearer",
                "a web session's pass is not accepted as a bearer on the App's public host",
                Some("in a browser the session cookie carries it; server code sends the pass on K2_GATEWAY_URL"),
            ));
        }
        return Ok(person(app, pass, device));
    }
    if !link {
        if let Some(raw) = req.cookie(SESSION_COOKIE) {
            let Some(pass) = k2_core::skin::resolve_skin_token(&raw).filter(|p| p.session && p.app == svc) else {
                return Err(refuse(401, "session_expired", "the session cookie is not a live session of this App", Some("sign in again")));
            };
            if req.is_write() && req.header("origin").is_none() {
                return Err(refuse(403, "origin_required", "a write with the session cookie needs an Origin header", None));
            }
            return Ok(person(app, pass, false));
        }
    }
    if cfg.anon {
        return Ok(Resolved { caller: Caller { role: DbRole::Anon, claims: anon_claims(app) }, who: Who::Anon });
    }
    Err(refuse(401, "anon_off", "Anonymous access is disabled", Some(concat!("sign in; or the owner turns the anon lane on: ", k2_core::cli_name!(), " app compat set <app> anon on"))))
}

// ── origins ─────────────────────────────────────────────────────────

fn norm_origin(o: &str) -> String {
    o.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// May a browser at `origin` call this App's doors? Its own host (the
/// request's `Host`, either scheme) or a listed origin. `Some(true)`: a
/// listed origin (the answer echoes it for CORS).
pub fn origin_allowed(cfg: &DoorConfig, req: &DoorRequest) -> Result<Option<String>, DoorAnswer> {
    let Some(origin) = req.header("origin") else { return Ok(None) };
    let o = norm_origin(origin);
    if let Some(host) = req.header("host") {
        let h = host.trim().to_ascii_lowercase();
        if o == format!("https://{h}") || o == format!("http://{h}") {
            return Ok(None);
        }
    }
    if cfg.origins.iter().any(|l| norm_origin(l) == o) {
        return Ok(Some(origin.trim().to_string()));
    }
    Err(refuse(403, "origin_not_allowed", format!("{origin} may not call this App's data doors"), Some(concat!("the owner lists it: ", k2_core::cli_name!(), " app compat set <app> origins add <origin>"))))
}

fn cors(a: DoorAnswer, origin: &Option<String>) -> DoorAnswer {
    match origin {
        Some(o) => a
            .header("Access-Control-Allow-Origin", o.clone())
            .header("Access-Control-Allow-Credentials", "true")
            .header("Access-Control-Expose-Headers", "Content-Range, Content-Location, Preference-Applied, Range-Unit, Location")
            .header("Vary", "Origin"),
        None => a,
    }
}

// ── one request ──────────────────────────────────────────────────────

/// Run `f` off the async threads. Tests: with the calling thread's scoped
/// K2 database (`k2_core::db::scoped_for_test` is per thread).
pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    #[cfg(test)]
    let scoped = k2_core::db::scoped_handle_for_test();
    tokio::task::spawn_blocking(move || {
        #[cfg(test)]
        if let Some(h) = scoped {
            return k2_core::db::with_scoped_handle(h, f);
        }
        f()
    })
    .await
    .ok()
}

/// The answer to one request (read whole).
pub async fn handle(req: DoorRequest, ctx: &Ctx) -> DoorAnswer {
    let origin = match origin_allowed(&ctx.cfg, &req) {
        Ok(o) => o,
        Err(a) => return a,
    };
    if req.method == "OPTIONS" {
        let a = DoorAnswer::new(204)
            .header("Access-Control-Allow-Methods", "GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS")
            .header(
                "Access-Control-Allow-Headers",
                "Authorization, apikey, Content-Type, Accept, Accept-Profile, Content-Profile, Prefer, Range, Range-Unit, x-client-info, x-supabase-api-version, x-upsert",
            )
            .header("Access-Control-Max-Age", "600");
        return cors(a, &origin);
    }
    let a = answer(req, ctx).await;
    cors(a, &origin)
}

async fn answer(req: DoorRequest, ctx: &Ctx) -> DoorAnswer {
    let app = ctx.cfg.app_id.clone();
    if blocking(move || crate::apps::suspend::is_suspended(&app)).await.unwrap_or(true) {
        return refuse(401, "app_suspended", "this App is suspended; only the owner resumes it", None);
    }
    if ctx.ingress == Ingress::Link {
        if let Err((wait, q)) = crate::apps::quotas::take(&ctx.cfg.app_id) {
            return refuse(429, "rate_limited", format!("this App may make {} calls a second; retry after {wait} ms", q.rps), None)
                .header("Retry-After", (wait / 1000).max(1).to_string());
        }
        if req.header("cookie").is_some() {
            return refuse(400, "cookies_not_allowed", "the gateway link carries no cookies: send the pass as Authorization: Bearer", None);
        }
    }
    let Some(door) = door_of(&req.path) else {
        return refuse(404, "not_found", "no such door", None);
    };
    if door == Door::Auth {
        // The auth door reads its own credentials (a sign-in has none yet).
        return super::auth_door_v1::handle(req, ctx).await;
    }
    let (r2, c2) = (req.clone(), ctx.clone());
    let who = match blocking(move || resolve(&r2, &c2)).await {
        Some(Ok(w)) => w,
        Some(Err(a)) => return a,
        None => return refuse(500, "failed", "worker join", None),
    };
    match door {
        Door::Rest => super::rest_door::handle(&req, ctx, &who).await,
        Door::Auth => unreachable_auth(),
        // Never reached: serve_one hands storage to serve_storage first.
        Door::Storage => refuse(500, "failed", concat!("the /storage/v1 door answers before this (a ", k2_core::product_name!(), " bug)"), None),
    }
}

fn unreachable_auth() -> DoorAnswer {
    refuse(500, "failed", concat!("the auth door answers before the caller is resolved (a ", k2_core::product_name!(), " bug)"), None)
}

/// Serve one door request whose head (`head_len` bytes) is next on `s`.
/// Returns whether the connection stays open for another request.
pub async fn serve_one<S: AsyncRead + AsyncWrite + Unpin>(s: &mut S, head: &str, head_len: usize, ctx: &Ctx) -> bool {
    let started = Instant::now();
    // Storage streams its own bodies (uploads up to 5 GiB): it reads the
    // head itself, before anything here buffers a body.
    let first_path = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).map(|t| t.split_once('?').map_or(t, |(p, _)| p)).unwrap_or("");
    if door_of(first_path) == Some(Door::Storage) {
        serve_storage(s, head, head_len, ctx).await;
        let _ = s.shutdown().await;
        return false;
    }
    let req = match http::read_request(s, head, head_len, ctx.cfg.limits.max_body_bytes).await {
        Ok(r) => r,
        Err(ReadFail::Gone) => return false,
        Err(ReadFail::Answer(a)) => {
            http::write_answer(s, &a, false, false).await;
            return false;
        }
    };
    let head_only = req.method == "HEAD";
    let close = req.close;
    let (verb, bytes_in) = (format!("{} {}", req.method, req.path), req.body.len() as u64);
    let a = handle(req, ctx).await;
    if ctx.ingress == Ingress::Link {
        let code = (a.status >= 400)
            .then(|| serde_json::from_slice::<Value>(&a.body).ok().and_then(|v| v["code"].as_str().map(str::to_string)))
            .flatten();
        crate::apps::audit::record(k2_core::app_audit::Call {
            at_ms: now_secs() * 1000,
            app_id: ctx.cfg.app_id.clone(),
            actor: "app".into(),
            verb,
            result: a.status,
            code,
            bytes_in,
            bytes_out: a.body.len() as u64,
            latency_ms: started.elapsed().as_millis() as u64,
            ..Default::default()
        });
    }
    http::write_answer(s, &a, head_only, !close).await
}

/// `/storage/v1`: lane D's door (`storage_door`), which reads the head and
/// streams its own bodies. Its caller rules are `storage_auth`'s; third-party
/// tokens go through [`third_party_check`] like every door's. The
/// connection closes after it.
async fn serve_storage<S: AsyncRead + AsyncWrite + Unpin>(s: &mut S, head: &str, head_len: usize, ctx: &Ctx) {
    let via = match ctx.ingress {
        Ingress::Link => super::storage_auth::Via::Link,
        Ingress::Public { .. } => super::storage_auth::Via::Public,
    };
    super::storage_door::serve(s, head, head_len, &ctx.cfg.app_id, via).await;
}

// ── the public host (apps::router) ───────────────────────────────────

/// Does the App path read `app_id`'s request heads for these doors?
pub fn watching(app_id: &str) -> bool {
    #[cfg(test)]
    {
        if test_configs().lock().unwrap_or_else(|e| e.into_inner()).contains_key(app_id) {
            return true;
        }
    }
    door_config::for_app(app_id).is_some()
}

/// Is the request whose head starts `head` one of these doors' on
/// `app_id`'s address? The App's own requests pay one prefix check.
pub fn is_door(app_id: &str, head: &[u8]) -> bool {
    let Some(target) = crate::client_ip::first_target(head) else { return false };
    let path = target.split_once('?').map_or(target.as_str(), |(p, _)| p);
    door_of(path).is_some() && watching(app_id)
}

/// The forwarder's divert for an App whose doors are on (`compat_app`) and
/// its other K2 doors (`apps::voice_door::is_k2s` on `label`).
pub fn divert_for(compat_app: Option<&str>, label: &str) -> crate::client_ip::Divert {
    let voice = crate::apps::voice_door::divert_for(label);
    match compat_app {
        None => voice,
        Some(app) => {
            let app = app.to_string();
            std::sync::Arc::new(move |head: &[u8]| is_door(&app, head) || voice(head))
        }
    }
}

/// A connection's next request on an App's address, read before the App is
/// dialed (or again): one of these doors' (served, with every door request
/// after it on the connection), else the call and transfer doors' and the
/// local answers' (`apps::voice_door`), else the bytes for the App.
pub async fn first_request<S>(client: &mut S, compat_app: &str, label: &str, local: Option<&LocalAnswer>, visitor: Option<IpAddr>) -> FirstRequest
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let p = crate::client_ip::peek_head(client).await;
    if door_of_head(&p.bytes).is_some() {
        let app = compat_app.to_string();
        if let Some(Some((cfg, service_id))) = blocking(move || config_for(&app)).await {
            let ctx = Ctx { cfg, service_id, ingress: Ingress::Public { visitor } };
            return serve_public(client, p.bytes, &ctx).await;
        }
    }
    let mut r = Replay::new(p.bytes, client);
    let got = if crate::apps::voice_door::watching(label) || crate::transfer_door::watching() {
        crate::apps::voice_door::first_request(&mut r, label, local, visitor).await
    } else if let Some(l) = local {
        crate::apps::router::answer_first(&mut r, l).await
    } else {
        FirstRequest::Pass(Vec::new())
    };
    let rest = r.unread();
    match got {
        FirstRequest::Answered => FirstRequest::Answered,
        FirstRequest::Pass(mut b) => {
            b.extend_from_slice(&rest);
            FirstRequest::Pass(b)
        }
    }
}

/// Serve door requests while they keep coming on the connection. `pending`
/// starts with a complete door head. A request for the App ends the loop:
/// its bytes go back for the App (`Pass`). The client closing, or an
/// answer that closes, is `Answered`.
async fn serve_public<S>(client: &mut S, mut pending: Vec<u8>, ctx: &Ctx) -> FirstRequest
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let Some((_, head, len)) = door_of_head(&pending) else {
            return FirstRequest::Pass(pending);
        };
        let (keep, left) = {
            let mut r = Replay::new(pending, client);
            let keep = serve_one(&mut r, &head, len, ctx).await;
            (keep, r.unread())
        };
        if !keep {
            let _ = client.shutdown().await;
            return FirstRequest::Answered;
        }
        let mut next = {
            let mut r = Replay::new(left, client);
            let p = crate::client_ip::peek_head(&mut r).await;
            let mut b = p.bytes;
            b.extend_from_slice(&r.unread());
            b
        };
        if next.is_empty() {
            return FirstRequest::Answered;
        }
        if door_of_head(&next).is_none() {
            return FirstRequest::Pass(std::mem::take(&mut next));
        }
        pending = next;
    }
}

// ── the gateway link (routes::dispatcher) ────────────────────────────

/// The dispatcher asks this first for every request on an App's gateway
/// link: `Some(keep_alive)` when the request was one of these doors' (served
/// on `stream`), `None` when it wasn't (the dispatcher goes on as before).
pub async fn link_door(stream: &mut tokio::net::TcpStream, ingress: crate::routes::dispatcher::Ingress) -> Option<bool> {
    if ingress != crate::routes::dispatcher::Ingress::App {
        return None;
    }
    let link = crate::apps::gateway::current()?;
    let mut buf = vec![0u8; 16 * 1024];
    // The dispatcher saw the first bytes; a head can still be arriving.
    // Wait for the rest only when the request line is already a door's.
    let deadline = Instant::now() + Duration::from_secs(10);
    let (head, len) = loop {
        let n = tokio::time::timeout(Duration::from_secs(30), stream.peek(&mut buf)).await.ok()?.ok()?;
        if let Some((_, head, len)) = door_of_head(&buf[..n]) {
            break (head, len);
        }
        let line_end = buf[..n].windows(2).position(|w| w == b"\r\n");
        let on_door = match line_end {
            Some(e) => {
                let line = String::from_utf8_lossy(&buf[..e]);
                let target = line.split_whitespace().nth(1).unwrap_or("");
                door_of(target.split_once('?').map_or(target, |(p, _)| p)).is_some()
            }
            None => true,
        };
        if !on_door || n == buf.len() || n == 0 || Instant::now() > deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    let app = link.app_id.clone();
    let Some((cfg, service_id)) = blocking(move || config_for(&app)).await.flatten() else {
        let mut sink = vec![0u8; len];
        let _ = tokio::io::AsyncReadExt::read_exact(stream, &mut sink).await;
        let a = refuse(404, "compat_off", "this App's Supabase-compatible doors are off", Some(concat!("the owner turns them on: ", k2_core::cli_name!(), " app compat on <app> --db <db>")));
        return Some(http::write_answer(stream, &a.closing(), false, false).await);
    };
    let ctx = Ctx { cfg, service_id, ingress: Ingress::Link };
    Some(serve_one(stream, &head, len, &ctx).await)
}

#[cfg(test)]
#[path = "door_tests.rs"]
pub(crate) mod tests;
