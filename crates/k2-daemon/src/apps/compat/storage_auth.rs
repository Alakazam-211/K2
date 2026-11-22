//! Who a `/storage/v1` request is (PRD §4), as a role and claims.
//!
//! | Credential | Where | Role |
//! |---|---|---|
//! | The App's session cookie (`k2_skin_session`, HttpOnly) | the public host | `authenticated`; writes also need a matching `Origin` |
//! | The person's pass as `Authorization: Bearer k2skn_…` | the gateway link | `authenticated` |
//! | A third-party ID token as a bearer | either | `authenticated` (lane A's issuers) |
//! | The App key with the `service` scope (`apikey` or bearer `k2sk_…`) | the link; the public host only with `service-key-public` | `service_role` |
//! | Nothing | either | `anon`, only with the App's anon lane on |
//!
//! **A web session's pass is refused as a bearer on the public host**:
//! only the cookie carries a web session there, so the pass never has to
//! sit where page script can read it (R4). A pass for another App, a
//! revoked one, or a platform pass is refused.
//!
//! **Seams for the other lanes.** Lane A owns the claims object
//! ([`claims_for_pass`] builds §5.2's shape from what K2 has today) and
//! third-party tokens ([`third_party`] refuses every one until its issuer
//! list lands). Lane C owns the publishable key and the `service` scope's
//! mint: this door reads `service` from the key's stored scopes, so a key
//! minted with it works here the day the scope exists.

use serde_json::{json, Value};

use super::door_config::DoorConfig;
use super::storage_err::{SResult, StorageError};
use super::storage_pg::{Caller, DbRole};

/// The App session cookie the server SDK sets (`sdk/server`, `guestFrom`).
pub const SESSION_COOKIE: &str = "k2_skin_session";
/// The App-key scope that runs as `service_role` (lane C mints it).
pub const SCOPE_SERVICE: &str = "service";

/// Which door the request came in by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The App's public host (`<app>.<label>.k2.dev`, its custom names).
    Public,
    /// The App's gateway link (its own server code, inside its wall).
    Link,
}

/// The parts of a request head this reads.
#[derive(Debug, Clone, Default)]
pub struct ReqAuth {
    pub method: String,
    pub host: Option<String>,
    pub origin: Option<String>,
    pub authorization: Option<String>,
    pub apikey: Option<String>,
    pub cookie: Option<String>,
}

/// What K2 knows about the App the request is for.
#[derive(Debug, Clone)]
pub struct AppFacts {
    pub app_id: String,
    /// The published service id App keys and sessions are bound to.
    pub service_id: String,
}

fn bearer(v: Option<&str>) -> Option<&str> {
    let v = v?.trim();
    let (scheme, rest) = v.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim()).filter(|s| !s.is_empty())
}

/// The named cookie's value.
pub fn cookie_value<'a>(cookie: &'a str, name: &str) -> Option<&'a str> {
    cookie.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k.trim() == name).then(|| v.trim()).filter(|v| !v.is_empty())
    })
}

/// A JWT's shape: three dot-separated base64url parts.
fn looks_like_jwt(t: &str) -> bool {
    let parts: Vec<&str> = t.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}

fn is_write(method: &str) -> bool {
    !matches!(method, "GET" | "HEAD" | "OPTIONS")
}

/// Does `origin` match the host the request was sent to, or a listed one?
pub fn origin_ok(origin: &str, host: Option<&str>, listed: &[String]) -> bool {
    let o = origin.trim().trim_end_matches('/');
    if listed.iter().any(|l| l.trim().trim_end_matches('/').eq_ignore_ascii_case(o)) {
        return true;
    }
    let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) else {
        return false;
    };
    o.eq_ignore_ascii_case(&format!("https://{host}"))
}

/// The resolution, with K2's stores behind closures so it tests alone.
pub struct Lookups<'a> {
    pub pass: &'a dyn Fn(&str) -> Result<Option<k2_core::skin::SkinPass>, String>,
    pub app_key: &'a dyn Fn(&str) -> Option<k2_core::api_keys::AppKeyRow>,
}

/// Resolve the caller of one request (blocking: the stores are SQLite).
pub fn resolve(app: &AppFacts, b: &DoorConfig, via: Via, r: &ReqAuth, look: &Lookups<'_>) -> SResult<Caller> {
    let auth_bearer = bearer(r.authorization.as_deref());
    // 1. The server key, as `apikey` or a bearer.
    let key = [r.apikey.as_deref().map(str::trim), auth_bearer]
        .into_iter()
        .flatten()
        .find(|k| k.starts_with(k2_core::api_keys::API_KEY_PREFIX));
    if let Some(k) = key {
        let row = (look.app_key)(k).filter(|row| row.service_id == app.service_id).ok_or_else(|| {
            StorageError::invalid_jwt("this key is not a live key of this App")
        })?;
        let scopes: Vec<String> = row
            .grant_json
            .as_deref()
            .and_then(|g| serde_json::from_str::<Value>(g).ok())
            .and_then(|v| v.get("scopes").cloned())
            .and_then(|s| serde_json::from_value(s).ok())
            .unwrap_or_default();
        if !scopes.iter().any(|s| s == SCOPE_SERVICE) {
            return Err(StorageError::access_denied("this App key has no service scope (k2 app key … --scope service)").with_http(403));
        }
        if via == Via::Public && !b.service_key_public {
            return Err(StorageError::access_denied(
                "the server key is taken on the App's gateway link only (k2 app compat set <app> service-key-public on allows the public host)",
            )
            .with_http(403));
        }
        return Ok(Caller::service(format!("service:{}", row.id)));
    }
    // 2. A bearer that isn't a key.
    if let Some(t) = auth_bearer {
        if k2_core::skin::is_skin_token(t) {
            if via == Via::Public {
                return Err(StorageError::access_denied(
                    "a web session's pass is never a bearer on the App's public host: the browser sends the session cookie",
                )
                .with_http(403));
            }
            return person(app, t, look);
        }
        if looks_like_jwt(t) {
            return third_party(app, t);
        }
        return Err(StorageError::invalid_jwt("invalid credential"));
    }
    // 3. The session cookie (the public host only: the link carries none).
    if let Some(pass) = r.cookie.as_deref().and_then(|c| cookie_value(c, SESSION_COOKIE)) {
        if via == Via::Public && is_write(&r.method) {
            let ok = r.origin.as_deref().is_some_and(|o| origin_ok(o, r.host.as_deref(), &b.origins));
            if !ok {
                return Err(StorageError::access_denied("a write with the session cookie needs this host's Origin").with_http(403));
            }
        }
        return person(app, pass, look);
    }
    // 4. Nothing.
    if b.anon {
        return Ok(Caller::anon());
    }
    Err(StorageError::new(401, "AccessDenied", "Unauthorized", "sign in first: this App's storage takes no anonymous requests").with_http(401))
}

fn person(app: &AppFacts, raw: &str, look: &Lookups<'_>) -> SResult<Caller> {
    let pass = (look.pass)(raw)
        .map_err(|e| StorageError::unavailable(format!("can't read sessions: {e}")))?
        .ok_or_else(|| StorageError::invalid_jwt("the session ended: sign in again"))?;
    let Some(principal) = pass.principal_id.clone().filter(|_| pass.session) else {
        return Err(StorageError::access_denied("not an App user's session").with_http(403));
    };
    if pass.app != app.service_id {
        return Err(StorageError::access_denied("a session of another App").with_http(403));
    }
    Ok(Caller { role: DbRole::Authenticated, claims: claims_for_pass(app, &pass, &principal), label: format!("guest:{principal}") })
}

/// The claims object for a K2 session (PRD §5.2). Lane A's `claims.rs`
/// replaces this with the full builder (metadata, the App role); the
/// fields and their meaning are the contract.
pub fn claims_for_pass(app: &AppFacts, pass: &k2_core::skin::SkinPass, principal: &str) -> Value {
    let now = k2_core::deploy::now();
    let email = pass.username.contains('@').then(|| pass.username.to_ascii_lowercase());
    json!({
        "sub": principal,
        "role": "authenticated",
        "email": email,
        "iss": "k2",
        "aud": "authenticated",
        "session_id": pass.id,
        "app_metadata": { "k2_app": app.app_id },
        "user_metadata": {},
        "iat": now,
        "exp": pass.expires_at.unwrap_or(now + 3600),
    })
}

/// A third-party ID token (PRD §6). Lane A's issuer list verifies it; until
/// that lands, every one is refused, loudly.
pub fn third_party(app: &AppFacts, _token: &str) -> SResult<Caller> {
    Err(StorageError::invalid_jwt(format!(
        "no trusted token issuer is set for {} (k2 app identity issuer add)",
        app.app_id
    )))
}

/// The live stores.
pub fn live_lookups() -> Lookups<'static> {
    Lookups { pass: &k2_core::skin::try_resolve_skin_token, app_key: &k2_core::api_keys::resolve_app_key }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::skin::SkinPass;

    const USER: &str = "6f1c4a52-0b7d-4d0e-9a2c-3b8f1e7d9a10";

    fn app() -> AppFacts {
        AppFacts { app_id: "jobs".into(), service_id: "svc-jobs".into() }
    }
    fn bind() -> DoorConfig {
        crate::apps::compat::storage_bind::test_seams::config_for("jobs", "jobsdb")
    }
    fn pass(app: &str) -> SkinPass {
        SkinPass {
            id: "tok1".into(),
            principal_id: Some(USER.into()),
            username: "Ana@Example.com".into(),
            caps: vec![],
            rooms: vec![],
            session: true,
            room_policy: Default::default(),
            app: app.into(),
            room_paths: k2_core::skin::paths::PassPaths::Whole,
            expires_at: Some(2_000_000_000),
            chat: false,
        }
    }
    fn key_row(service: &str, scopes: &[&str]) -> k2_core::api_keys::AppKeyRow {
        k2_core::api_keys::AppKeyRow {
            id: "key1".into(),
            label: None,
            service_id: service.into(),
            grant_json: Some(json!({ "scopes": scopes }).to_string()),
            created_at: 0,
            revoked_at: None,
            disabled_at: None,
        }
    }

    fn run(b: &DoorConfig, via: Via, r: ReqAuth, p: Option<SkinPass>, k: Option<k2_core::api_keys::AppKeyRow>) -> SResult<Caller> {
        let pf = move |_: &str| Ok(p.clone());
        let kf = move |_: &str| k.clone();
        resolve(&app(), b, via, &r, &Lookups { pass: &pf, app_key: &kf })
    }

    fn cookie_req(method: &str, origin: Option<&str>) -> ReqAuth {
        ReqAuth {
            method: method.into(),
            host: Some("jobs.box.k2.dev".into()),
            origin: origin.map(str::to_string),
            cookie: Some("theme=dark; k2_skin_session=k2skn_abc.def".into()),
            ..Default::default()
        }
    }

    #[test]
    fn the_cookie_is_the_person_with_section_5_2_claims() {
        let c = run(&bind(), Via::Public, cookie_req("GET", None), Some(pass("svc-jobs")), None).expect("person");
        assert_eq!(c.role, DbRole::Authenticated);
        assert_eq!(c.sub(), Some(USER));
        assert_eq!(c.claims["role"], "authenticated");
        assert_eq!(c.claims["email"], "ana@example.com");
        assert_eq!(c.claims["aud"], "authenticated");
        assert_eq!(c.claims["session_id"], "tok1");
        assert_eq!(c.claims["app_metadata"]["k2_app"], "jobs");
    }

    #[test]
    fn cookie_writes_need_this_hosts_origin() {
        let e = run(&bind(), Via::Public, cookie_req("POST", None), Some(pass("svc-jobs")), None).expect_err("no origin");
        assert_eq!((e.status, e.http_status()), (403, 403));
        let e = run(&bind(), Via::Public, cookie_req("POST", Some("https://evil.example")), Some(pass("svc-jobs")), None).expect_err("other origin");
        assert_eq!(e.status, 403);
        run(&bind(), Via::Public, cookie_req("POST", Some("https://jobs.box.k2.dev")), Some(pass("svc-jobs")), None).expect("same origin");
        let mut b = bind();
        b.origins = vec!["https://app.example.com".into()];
        run(&b, Via::Public, cookie_req("DELETE", Some("https://app.example.com")), Some(pass("svc-jobs")), None).expect("listed origin");
    }

    #[test]
    fn another_apps_session_or_an_ended_one_is_refused() {
        let e = run(&bind(), Via::Public, cookie_req("GET", None), Some(pass("svc-other")), None).expect_err("other App");
        assert_eq!(e.status, 403);
        let e = run(&bind(), Via::Public, cookie_req("GET", None), None, None).expect_err("ended");
        assert_eq!(e.code, "InvalidJWT");
        let mut platform = pass("svc-jobs");
        platform.session = false;
        let e = run(&bind(), Via::Public, cookie_req("GET", None), Some(platform), None).expect_err("platform pass");
        assert_eq!(e.status, 403);
    }

    #[test]
    fn a_web_pass_is_never_a_bearer_on_the_public_host() {
        let r = ReqAuth { method: "GET".into(), authorization: Some("Bearer k2skn_abc.def".into()), ..Default::default() };
        let e = run(&bind(), Via::Public, r.clone(), Some(pass("svc-jobs")), None).expect_err("public bearer");
        assert_eq!(e.http_status(), 403);
        let c = run(&bind(), Via::Link, r, Some(pass("svc-jobs")), None).expect("link bearer");
        assert_eq!(c.role, DbRole::Authenticated);
    }

    #[test]
    fn the_server_key_is_service_role_on_the_link_only_by_default() {
        let r = ReqAuth { method: "POST".into(), apikey: Some("k2sk_secret".into()), ..Default::default() };
        let c = run(&bind(), Via::Link, r.clone(), None, Some(key_row("svc-jobs", &["service"]))).expect("link");
        assert_eq!(c.role, DbRole::Service);
        assert_eq!(c.claims["role"], "service_role");
        let e = run(&bind(), Via::Public, r.clone(), None, Some(key_row("svc-jobs", &["service"]))).expect_err("public");
        assert_eq!(e.http_status(), 403);
        let mut b = bind();
        b.service_key_public = true;
        run(&b, Via::Public, r.clone(), None, Some(key_row("svc-jobs", &["service"]))).expect("public with the setting");
        let e = run(&bind(), Via::Link, r.clone(), None, Some(key_row("svc-jobs", &["users"]))).expect_err("no service scope");
        assert_eq!(e.http_status(), 403);
        let e = run(&bind(), Via::Link, r.clone(), None, Some(key_row("svc-other", &["service"]))).expect_err("other App's key");
        assert_eq!(e.code, "InvalidJWT");
        let e = run(&bind(), Via::Link, r, None, None).expect_err("revoked key");
        assert_eq!(e.code, "InvalidJWT");
    }

    #[test]
    fn nothing_is_anon_only_with_the_lane_on() {
        let r = ReqAuth { method: "GET".into(), ..Default::default() };
        let e = run(&bind(), Via::Public, r.clone(), None, None).expect_err("anon off");
        assert_eq!((e.status, e.http_status()), (401, 401));
        let mut b = bind();
        b.anon = true;
        let c = run(&b, Via::Public, r, None, None).expect("anon on");
        assert_eq!(c.role, DbRole::Anon);
        assert_eq!(c.claims, json!({"role": "anon"}));
    }

    #[test]
    fn third_party_tokens_wait_for_lane_a() {
        let r = ReqAuth { method: "GET".into(), authorization: Some("Bearer eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ4In0.c2ln".into()), ..Default::default() };
        let e = run(&bind(), Via::Public, r, None, None).expect_err("no issuer");
        assert_eq!(e.code, "InvalidJWT");
        assert!(e.message.contains("issuer"), "{}", e.message);
    }

    #[test]
    fn cookies_and_origins_parse() {
        assert_eq!(cookie_value("a=1; k2_skin_session=k2skn_x.y ;b=2", SESSION_COOKIE), Some("k2skn_x.y"));
        assert_eq!(cookie_value("k2_skin_sessionX=1", SESSION_COOKIE), None);
        assert!(origin_ok("https://jobs.box.k2.dev/", Some("jobs.box.k2.dev"), &[]));
        assert!(!origin_ok("http://jobs.box.k2.dev", Some("jobs.box.k2.dev"), &[]), "plain http never matches");
        assert!(!origin_ok("null", Some("jobs.box.k2.dev"), &[]));
    }
}
