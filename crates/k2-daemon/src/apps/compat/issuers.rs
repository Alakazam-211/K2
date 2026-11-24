//! Third-party tokens (prd-app-supabase-compat-v1 §6; lane A, S1b): a phone
//! app's ID token from an issuer the App trusts, checked as Supabase's
//! third-party auth checks it, and mapped to one of the App's users.
//!
//! - **Issuers, per App** (`app_identity_issuers`): the issuer string, the
//!   audience and the key-set URL, each pinned. Nothing else is trusted.
//! - **Check:** RS256 or ES256 only; the signature against the issuer's
//!   published keys; `iss` and `aud` equal to the pinned values; `exp` and
//!   `iat` with 60 s of slack; `email_verified` true.
//! - **Keys:** cached as long as the issuer's `Cache-Control: max-age` says
//!   (10 minutes without one), refreshed at half that, fetched once more
//!   for an unknown key id (at most once a minute). Expired and not
//!   fetchable: **every token is refused, 401** (fail closed). The first
//!   failed refresh is kept for `k2 app compat status` as a warning.
//! - **Mapping (§6.1):** the token's email, lower-cased, must be the email
//!   of one of the App's users. Else 403 `not_a_user` (the default; or the
//!   App's `unknown-email authenticated` setting). A disabled user is
//!   refused. The first match records (issuer, `sub`); a later mismatch is
//!   403 `identity_changed` with an audit row.
//! - **`sub` (§6.2):** `token` keeps the issuer's id; `user` puts the App
//!   user's id there and the issuer's id in `<issuer_key>_uid`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use jsonwebtoken::{jwk::JwkSet, Algorithm, DecodingKey, Validation};
use rusqlite::params;
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::claims::DoorRole;
use super::door_config::{SubMode, UnknownEmail};

/// Slack on `exp` and `iat` (§6).
pub const LEEWAY_SECS: u64 = 60;
/// Keys without a cache header live this long.
pub const DEFAULT_KEYS_TTL_SECS: i64 = 600;
/// An unknown key id fetches the keys again at most this often.
pub const UNKNOWN_KID_REFETCH_SECS: i64 = 60;
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// One issuer an App trusts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issuer {
    pub app_id: String,
    /// Short name: under `sub user` the issuer's id is `<key>_uid`.
    pub key: String,
    pub issuer: String,
    pub audience: String,
    pub jwks_url: String,
}

/// Why a token was refused: its code (the doors answer with it), HTTP
/// status and words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRefusal {
    pub code: &'static str,
    pub status: u16,
    pub message: String,
}

fn refuse(code: &'static str, status: u16, message: impl Into<String>) -> TokenRefusal {
    TokenRefusal { code, status, message: message.into() }
}

/// A token that passed: the role and claims the request runs with.
#[derive(Debug, Clone, PartialEq)]
pub struct Verified {
    pub role: DoorRole,
    pub claims: Value,
    /// The App user it mapped to (None under `unknown-email authenticated`).
    pub user_id: Option<String>,
}

// ── issuers (app_identity_issuers) ───────────────────────────────────

pub fn issuer_key_ok(k: &str) -> bool {
    !k.is_empty() && k.len() <= 32 && k.bytes().next().is_some_and(|b| b.is_ascii_lowercase()) && k.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

pub(crate) fn list(app_id: &str) -> Result<Vec<Issuer>, String> {
    let h = k2_core::db::shared();
    let c = h.lock();
    let mut st = c
        .prepare("SELECT app_id, issuer_key, issuer, audience, jwks_url FROM app_identity_issuers WHERE app_id = ?1 ORDER BY issuer_key")
        .map_err(|e| format!("issuers: {e}"))?;
    let rows = st
        .query_map(params![app_id], |r| Ok(Issuer { app_id: r.get(0)?, key: r.get(1)?, issuer: r.get(2)?, audience: r.get(3)?, jwks_url: r.get(4)? }))
        .map_err(|e| format!("issuers: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("issuers: {e}"))
}

/// Add (or replace, by issuer string) one trusted issuer. The key-set URL
/// must be https (a loopback http URL is allowed for tests on the box).
pub(crate) fn add(i: &Issuer, by: &str, now: i64) -> Result<(), String> {
    if !issuer_key_ok(&i.key) {
        return Err("--key must be a short lower-case name ([a-z][a-z0-9_]*, at most 32): firebase, google".into());
    }
    if i.issuer.trim().is_empty() || i.audience.trim().is_empty() {
        return Err("--issuer and --audience are both required (the exact iss and aud the tokens carry)".into());
    }
    let u = url::Url::parse(&i.jwks_url).map_err(|e| format!("--jwks is not a URL: {e}"))?;
    let loopback = matches!(u.host_str(), Some("127.0.0.1") | Some("localhost") | Some("[::1]"));
    if u.scheme() != "https" && !(u.scheme() == "http" && loopback) {
        return Err("--jwks must be an https URL".into());
    }
    let h = k2_core::db::shared();
    let c = h.lock();
    c.execute(
        "INSERT INTO app_identity_issuers (app_id, issuer_key, issuer, audience, jwks_url, created_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(app_id, issuer) DO UPDATE SET issuer_key = excluded.issuer_key, audience = excluded.audience, jwks_url = excluded.jwks_url",
        params![i.app_id, i.key, i.issuer, i.audience, i.jwks_url, by, now],
    )
    .map_err(|e| if e.to_string().contains("UNIQUE") { format!("another issuer of {} already uses the key {}", i.app_id, i.key) } else { format!("issuers: {e}") })?;
    Ok(())
}

pub(crate) fn remove(app_id: &str, issuer_or_key: &str) -> Result<bool, String> {
    let h = k2_core::db::shared();
    let c = h.lock();
    Ok(c
        .execute("DELETE FROM app_identity_issuers WHERE app_id = ?1 AND (issuer = ?2 OR issuer_key = ?2)", params![app_id, issuer_or_key])
        .map_err(|e| format!("issuers: {e}"))?
        > 0)
}

// ── the key cache ────────────────────────────────────────────────────

/// What a key-set fetch gave: the body and its max-age, if any.
pub(crate) type Fetched = Result<(String, Option<i64>), String>;
type Fetcher = fn(&str) -> Fetched;

#[derive(Debug, Clone)]
struct Keys {
    set: Option<JwkSet>,
    fetched_at: i64,
    expires_at: i64,
    last_attempt: i64,
    last_error: Option<String>,
}

static CACHE: Mutex<Option<HashMap<String, Keys>>> = Mutex::new(None);
static FETCHER: Mutex<Option<Fetcher>> = Mutex::new(None);

/// Tests: answer key-set fetches without the network (None = the real one).
#[cfg(test)]
pub(crate) fn set_fetcher(f: Option<Fetcher>) {
    *FETCHER.lock().unwrap_or_else(|e| e.into_inner()) = f;
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn max_age(cache_control: &str) -> Option<i64> {
    cache_control.split(',').map(str::trim).find_map(|d| d.strip_prefix("max-age=")).and_then(|v| v.trim().parse::<i64>().ok()).filter(|v| *v >= 0)
}

/// The real fetch, on its own thread (a door may call from an async task).
fn http_fetch(url: &str) -> Fetched {
    let url = url.to_string();
    std::thread::spawn(move || -> Fetched {
        let client = reqwest::blocking::Client::builder().timeout(FETCH_TIMEOUT).build().map_err(|e| e.to_string())?;
        let r = client.get(&url).send().map_err(|e| format!("fetch {url}: {e}"))?;
        if !r.status().is_success() {
            return Err(format!("fetch {url}: HTTP {}", r.status()));
        }
        let age = r.headers().get(reqwest::header::CACHE_CONTROL).and_then(|v| v.to_str().ok()).and_then(max_age);
        Ok((r.text().map_err(|e| format!("fetch {url}: {e}"))?, age))
    })
    .join()
    .unwrap_or_else(|_| Err("the key fetch panicked".into()))
}

fn fetch(url: &str) -> Fetched {
    let f = *FETCHER.lock().unwrap_or_else(|e| e.into_inner());
    match f {
        Some(f) => f(url),
        None => http_fetch(url),
    }
}

fn refresh(url: &str, now: i64) {
    let got = fetch(url).and_then(|(body, age)| {
        let set: JwkSet = serde_json::from_str(&body).map_err(|e| format!("{url}: not a JSON key set: {e}"))?;
        Ok((set, age.unwrap_or(DEFAULT_KEYS_TTL_SECS)))
    });
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    let e = map.entry(url.to_string()).or_insert(Keys { set: None, fetched_at: 0, expires_at: 0, last_attempt: 0, last_error: None });
    e.last_attempt = now;
    match got {
        Ok((set, ttl)) => {
            e.set = Some(set);
            e.fetched_at = now;
            e.expires_at = now + ttl;
            e.last_error = None;
        }
        Err(err) => {
            k2_core::log_debug!("[compat] issuer keys {url}: {err}");
            e.last_error = Some(err);
        }
    }
}

/// The keys for `url` now: refreshed when past half their life, or when
/// `kid` is unknown (once a minute). `None`: none usable (expired and not
/// fetchable).
fn keys_for(url: &str, kid: Option<&str>, now: i64) -> Option<JwkSet> {
    let snapshot = || CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(url).cloned());
    let due = match snapshot() {
        None => true,
        Some(k) => {
            let half = k.fetched_at + (k.expires_at - k.fetched_at) / 2;
            let stale = k.set.is_none() || now >= half;
            let unknown = kid.is_some_and(|id| k.set.as_ref().is_none_or(|s| s.find(id).is_none()));
            let since = now - k.last_attempt;
            if stale {
                // A failing issuer is tried again once a minute, not on
                // every request.
                k.last_error.is_none() || since >= UNKNOWN_KID_REFETCH_SECS
            } else {
                unknown && since >= UNKNOWN_KID_REFETCH_SECS
            }
        }
    };
    if due {
        refresh(url, now);
    }
    snapshot().filter(|k| now < k.expires_at).and_then(|k| k.set)
}

/// For `k2 app compat status`: each issuer's keys (fetched, expiry, the
/// first failed refresh).
pub(crate) fn key_status(url: &str) -> Value {
    match CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(url).cloned()) {
        None => json!({ "fetched": false }),
        Some(k) => json!({ "fetched": k.set.is_some(), "fetchedAt": k.fetched_at, "expiresAt": k.expires_at, "warning": k.last_error }),
    }
}

// ── the check ────────────────────────────────────────────────────────

fn peek_claims(token: &str) -> Option<Value> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Is this bearer a third-party ID token, for [`verify`]? A JWT signed
/// RS256 or ES256 that is not a K2 pass in a JWT's shape: the auth door
/// hands supabase-js `header.claims.<k2 pass>`, so a bearer whose last part
/// is a K2 pass, or whose claims say a K2 issuer (`k2:`), goes to the pass
/// check, never here.
pub fn looks_like_jwt(bearer: &str) -> bool {
    let parts: Vec<&str> = bearer.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
        return false;
    }
    if k2_core::skin::is_skin_token(parts[2]) || k2_core::skin::is_api_key_prefix(parts[2]) {
        return false;
    }
    let Ok(h) = jsonwebtoken::decode_header(bearer) else { return false };
    if !matches!(h.alg, Algorithm::RS256 | Algorithm::ES256) {
        return false;
    }
    !peek_claims(bearer).and_then(|c| c.get("iss").and_then(Value::as_str).map(|i| i.starts_with("k2:") || i == "k2")).unwrap_or(false)
}

/// Check `token` for App `app_id` (its service id `service_id`) and map it
/// to the App's user. `now`: unix seconds.
pub fn verify(app_id: &str, service_id: &str, token: &str, now: i64) -> Result<Verified, TokenRefusal> {
    let cfg = super::door_config::for_app(app_id).ok_or_else(|| refuse("compat_off", 404, format!("the Supabase-compatible layer is off for {app_id}")))?;
    let header = jsonwebtoken::decode_header(token).map_err(|_| refuse("token_malformed", 401, "not a signed token"))?;
    if !matches!(header.alg, Algorithm::RS256 | Algorithm::ES256) {
        return Err(refuse("token_algorithm", 401, format!("{:?} tokens are not accepted (RS256 or ES256 only)", header.alg)));
    }
    let iss = peek_claims(token).and_then(|c| c.get("iss").and_then(Value::as_str).map(str::to_string)).ok_or_else(|| refuse("token_malformed", 401, "the token has no iss"))?;
    let issuers = list(app_id).map_err(|e| refuse("compat_failed", 500, e))?;
    let issuer = issuers.iter().find(|i| i.issuer == iss).ok_or_else(|| refuse("issuer_unknown", 401, format!("{app_id} doesn't trust tokens from {iss}")))?;
    let set = keys_for(&issuer.jwks_url, header.kid.as_deref(), now)
        .ok_or_else(|| refuse("issuer_keys_unavailable", 401, format!("{iss}'s signing keys have expired and can't be fetched; every token from it is refused until they can")))?;
    let jwk = match header.kid.as_deref() {
        Some(kid) => set.find(kid),
        None if set.keys.len() == 1 => set.keys.first(),
        None => None,
    }
    .ok_or_else(|| refuse("token_signature", 401, "the token names no key the issuer publishes"))?;
    let key = DecodingKey::from_jwk(jwk).map_err(|_| refuse("token_signature", 401, "the issuer's key can't be used"))?;
    let mut v = Validation::new(header.alg);
    v.leeway = LEEWAY_SECS;
    v.set_issuer(&[issuer.issuer.as_str()]);
    v.set_audience(&[issuer.audience.as_str()]);
    v.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    let data = jsonwebtoken::decode::<Map<String, Value>>(token, &key, &v).map_err(|e| {
        use jsonwebtoken::errors::ErrorKind as K;
        match e.kind() {
            K::ExpiredSignature => refuse("token_expired", 401, "the token has expired"),
            K::InvalidAudience => refuse("token_audience", 401, format!("the token is not for {}", issuer.audience)),
            K::InvalidIssuer => refuse("issuer_unknown", 401, "the token's issuer is not the pinned one"),
            K::InvalidSignature | K::InvalidEcdsaKey | K::InvalidRsaKey(_) => refuse("token_signature", 401, "the signature doesn't verify"),
            K::MissingRequiredClaim(c) => refuse("token_malformed", 401, format!("the token has no {c}")),
            _ => refuse("token_malformed", 401, "the token can't be read"),
        }
    })?;
    let tc = data.claims;
    if tc.get("iat").and_then(Value::as_i64).is_some_and(|iat| iat > now + LEEWAY_SECS as i64) {
        return Err(refuse("token_expired", 401, "the token was issued in the future"));
    }
    let verified = matches!(tc.get("email_verified"), Some(Value::Bool(true))) || tc.get("email_verified").and_then(Value::as_str) == Some("true");
    if !verified {
        return Err(refuse("email_unverified", 401, "the token's email is not verified"));
    }
    let email = tc.get("email").and_then(Value::as_str).map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty()).ok_or_else(|| refuse("token_malformed", 401, "the token has no email"))?;
    let sub = tc.get("sub").and_then(Value::as_str).unwrap_or_default().to_string();
    let user = k2_core::skin::compat::app_user_by_email(service_id, &email).map_err(|e| refuse("compat_failed", 500, e))?;
    let Some(user) = user else {
        return match cfg.unknown_email {
            UnknownEmail::Refuse => Err(refuse("not_a_user", 403, format!("{email} is not a user of {app_id}"))),
            UnknownEmail::Authenticated => Ok(Verified { role: DoorRole::Authenticated, claims: token_claims(app_id, &tc, None), user_id: None }),
        };
    };
    if !user.enabled {
        return Err(refuse("user_disabled", 403, format!("{} is disabled on {app_id}", user.username)));
    }
    match k2_core::skin::compat::match_identity(service_id, &issuer.issuer, &sub, &user.id, now).map_err(|e| refuse("compat_failed", 500, e))? {
        k2_core::skin::compat::LinkOutcome::Ok { .. } => {}
        k2_core::skin::compat::LinkOutcome::Changed { recorded_principal, recorded_sub } => {
            audit_identity_changed(app_id, &issuer.issuer, &sub, &user.id, recorded_principal.as_deref(), recorded_sub.as_deref());
            return Err(refuse(
                "identity_changed",
                403,
                format!("this sign-in doesn't match the one recorded for {} ({cli} app identity unlink {app_id} {} clears it)", user.username, user.username, cli = k2_core::brand::CLI_NAME),
            ));
        }
    }
    let mut claims = token_claims(app_id, &tc, Some(&user));
    if cfg.sub_mode == SubMode::User {
        claims["sub"] = json!(user.id);
        claims[format!("{}_uid", issuer.key)] = json!(sub);
    }
    Ok(Verified { role: DoorRole::Authenticated, claims, user_id: Some(user.id) })
}

/// The claims of a verified third-party token (`sub` = the issuer's id):
/// the token's own `iss`, `aud`, `iat`, `exp`, `email`; the App user's
/// metadata when it mapped to one.
fn token_claims(app_id: &str, tc: &Map<String, Value>, user: Option<&k2_core::skin::compat::AppUser>) -> Value {
    let mut am: Map<String, Value> = user.and_then(|u| u.app_metadata.as_object().cloned()).unwrap_or_default();
    am.insert("k2_app".into(), json!(app_id));
    if let Some(r) = user.and_then(|u| u.role_name.clone()) {
        am.insert("k2_role".into(), json!(r));
    }
    json!({
        "sub": tc.get("sub"),
        "role": "authenticated",
        "email": tc.get("email").and_then(Value::as_str).map(str::to_lowercase),
        "iss": tc.get("iss"),
        "aud": tc.get("aud"),
        "session_id": Value::Null,
        "app_metadata": am,
        "user_metadata": user.map(|u| u.user_metadata.clone()).unwrap_or_else(|| json!({})),
        "iat": tc.get("iat"),
        "exp": tc.get("exp"),
    })
}

fn audit_identity_changed(app_id: &str, issuer: &str, sub: &str, user: &str, recorded_principal: Option<&str>, recorded_sub: Option<&str>) {
    let h = k2_core::db::shared();
    let c = h.lock();
    let payload = json!({ "issuer": issuer, "sub": sub, "user": user, "recordedUser": recorded_principal, "recordedSub": recorded_sub });
    if let Err(e) = k2_core::deploy::audit::append(&c, k2_core::deploy::audit::SUBJECT_APP, app_id, "identity.changed", "system:compat", None, &payload) {
        k2_core::log_debug!("[compat] audit {app_id} identity.changed: {e}");
    }
}

/// [`verify`] in the shape the doors' slot takes
/// (`door::set_third_party_verifier`, registered at boot): the claims and
/// the App user, or `(status, code, message)`.
pub fn door_verifier(cfg: &super::door_config::DoorConfig, token: &str) -> Result<(Value, Option<String>), (u16, String, String)> {
    let Some((_, service_id)) = super::door::config_for(&cfg.app_id) else {
        return Err((404, "compat_off".into(), format!("the Supabase-compatible layer is off for {}", cfg.app_id)));
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    match verify(&cfg.app_id, &service_id, token, now) {
        Ok(v) => Ok((v.claims, v.user_id)),
        Err(r) => Err((r.status, r.code.to_string(), r.message)),
    }
}

#[cfg(test)]
#[path = "issuers_tests.rs"]
mod tests;
