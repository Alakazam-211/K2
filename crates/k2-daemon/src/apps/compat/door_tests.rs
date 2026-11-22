//! The compat doors' front: paths, who a request is (PRD §4's table),
//! origins, and keep-alive on an App's public host.

use super::*;
use crate::apps::compat::door_config::{DoorConfig, Limits, SubMode, UnknownEmail};
use k2_core::deploy::store;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) fn cfg_for(app: &str, db: &str) -> DoorConfig {
    DoorConfig {
        app_id: app.into(),
        database: db.into(),
        database_id: format!("dbid-{db}"),
        authenticator: format!("{db}_authenticator"),
        authenticator_secret_ref: "dbsec_missing".into(),
        anon: false,
        service_key_public: false,
        origins: vec!["https://admin.example.com".into()],
        schemas: vec!["public".into()],
        limits: Limits::default(),
        sub_mode: SubMode::Token,
        unknown_email: UnknownEmail::Refuse,
    }
}

/// K2's records for one App with two people and a server key.
pub(crate) struct Fx {
    pub _db: k2_core::db::TestDbGuard,
    pub _home: crate::test_support::TempHome,
    pub app: String,
    pub svc: String,
    pub other_svc: String,
    /// Ann's web session pass, Ann's device pass, Bo's web pass on the other App.
    pub web: String,
    pub device: String,
    pub other_app_pass: String,
    pub ann_id: String,
    pub service_key: String,
    pub users_key: String,
    pub other_service_key: String,
}

fn add_app(app: &str, ws: &str) -> String {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let svc = k2_core::published_services::insert(
        &conn,
        ws,
        app,
        k2_core::published_services::CMD_APP_SENTINEL,
        "/tmp/k2-compat-test/apps",
        0,
        k2_core::published_services::EXPOSE_TUNNEL,
        k2_core::published_services::DESIRED_STOPPED,
        k2_core::published_services::KIND_CMD,
        "",
    )
    .unwrap();
    store::insert_app(
        &conn,
        &store::NewApp {
            app_id: app,
            service_id: &svc.id,
            seed_id: None,
            owner_workspace: ws,
            isolation: "process",
            memory_limit_bytes: Some(256 << 20),
            front_port: 0,
            created_by: "owner",
        },
    )
    .unwrap();
    svc.id
}

pub(crate) fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    let _ = crate::vault::ensure_test_vault();
    let ws = format!("cmp-ws-{tag}");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", rusqlite::params![ws, format!("/tmp/k2-compat-test/{ws}")])
            .unwrap();
    }
    let (app, other) = (format!("cmp-{tag}"), format!("cmpo-{tag}"));
    let svc = add_app(&app, &ws);
    let other_svc = add_app(&other, &ws);
    let ann = k2_core::skin::add_principal_for_app(&format!("ann-{tag}"), Some(&format!("ann-{tag}@example.com")), &svc, None, "owner").unwrap();
    k2_core::skin::add_principal_for_app(&format!("bo-{tag}"), None, &other_svc, None, "owner").unwrap();
    let web = k2_core::skin::create_app_session_token(&ann.username, &svc).unwrap().raw;
    k2_core::skin::app_login::set_settings(&svc, None, Some(30), "owner").unwrap();
    let dev = k2_core::skin::app_login::Device { id: format!("phone-{tag}"), name: "Phone".into() };
    let device = k2_core::skin::app_login::login_device(&ann.username, &svc, &dev).unwrap().expect("device sign-in on").session.raw;
    let other_app_pass = k2_core::skin::create_app_session_token(&format!("bo-{tag}"), &other_svc).unwrap().raw;
    let (_, service_key) = k2_core::api_keys::create_app_key("srv", &svc, r#"{"scopes":["service"]}"#).unwrap();
    let (_, users_key) = k2_core::api_keys::create_app_key("usr", &svc, r#"{"scopes":["users"]}"#).unwrap();
    let (_, other_service_key) = k2_core::api_keys::create_app_key("srv2", &other_svc, r#"{"scopes":["service"]}"#).unwrap();
    Fx { _db: db, _home: home, app, svc, other_svc, web, device, other_app_pass, ann_id: ann.id, service_key, users_key, other_service_key }
}

fn ctx(f: &Fx, ingress: Ingress, edit: impl FnOnce(&mut DoorConfig)) -> Ctx {
    let mut cfg = cfg_for(&f.app, "shop");
    edit(&mut cfg);
    Ctx { cfg, service_id: f.svc.clone(), ingress }
}

fn public() -> Ingress {
    Ingress::Public { visitor: None }
}

fn req(method: &str, headers: &[(&str, &str)]) -> DoorRequest {
    let mut r = DoorRequest { method: method.into(), path: "/rest/v1/notes".into(), ..Default::default() };
    r.headers = headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())).collect();
    r
}

fn code(a: &DoorAnswer) -> String {
    let v: Value = serde_json::from_slice(&a.body).unwrap();
    v["code"].as_str().unwrap_or_default().to_string()
}

fn refused(r: Result<Resolved, DoorAnswer>) -> (u16, String) {
    match r {
        Err(a) => (a.status, code(&a)),
        Ok(w) => panic!("expected a refusal, got {w:?}"),
    }
}

#[test]
fn door_paths_are_exact_prefixes() {
    for (p, d) in [("/rest/v1", Door::Rest), ("/rest/v1/notes", Door::Rest), ("/rest/v1/rpc/f", Door::Rest), ("/auth/v1/user", Door::Auth), ("/storage/v1/object/x", Door::Storage)] {
        assert_eq!(door_of(p), Some(d), "{p}");
    }
    for p in ["/rest/v10", "/rest", "/restv1/x", "/api/rest/v1", "/auth/v1x", "/"] {
        assert_eq!(door_of(p), None, "{p} is the App's");
    }
    assert!(door_of_head(b"GET /rest/v1/t?select=* HTTP/1.1\r\nHost: h\r\n\r\n").is_some());
    assert!(door_of_head(b"GET /rest/v1/t HTTP/1.1\r\nHost: h\r\n").is_none(), "an incomplete head is not served yet");
    assert!(looks_like_jwt("eyJhbGciOi.eyJzdWIiOi.c2ln-_x"));
    assert!(!looks_like_jwt("k2skn_abc") && !looks_like_jwt("a.b") && !looks_like_jwt("a..c"));
}

#[test]
fn who_a_request_is_follows_the_table() {
    let f = fx("who");
    let pk = publishable_key(&f.svc);
    let other_pk = publishable_key(&f.other_svc);
    let pub_ctx = ctx(&f, public(), |_| {});
    let link_ctx = ctx(&f, Ingress::Link, |_| {});
    let cookie = format!("theme=dark; {SESSION_COOKIE}={}", f.web);

    // Every request names the App with an apikey.
    assert_eq!(refused(resolve(&req("GET", &[]), &pub_ctx)), (401, "no_api_key".into()));
    assert_eq!(refused(resolve(&req("GET", &[("apikey", &other_pk)]), &pub_ctx)), (401, "wrong_app_key".into()));
    assert_eq!(refused(resolve(&req("GET", &[("apikey", "sb_publishable_x")]), &pub_ctx)), (401, "bad_api_key".into()));

    // The publishable key alone: anon, only with the lane on. supabase-js
    // also sends it as the bearer when nobody is signed in.
    let bare = req("GET", &[("apikey", &pk), ("authorization", &format!("Bearer {pk}"))]);
    assert_eq!(refused(resolve(&bare, &pub_ctx)), (401, "anon_off".into()));
    let w = resolve(&bare, &ctx(&f, public(), |c| c.anon = true)).unwrap();
    assert_eq!((w.caller.role, w.who.clone()), (DbRole::Anon, Who::Anon));
    assert_eq!(w.caller.claims["role"], "anon");

    // The browser: the session cookie.
    let w = resolve(&req("GET", &[("apikey", &pk), ("cookie", &cookie)]), &pub_ctx).unwrap();
    assert_eq!(w.caller.role, DbRole::Authenticated);
    assert!(matches!(&w.who, Who::Person { device: false, .. }), "{w:?}");
    let c = &w.caller.claims;
    assert_eq!(c["sub"], f.ann_id.as_str());
    assert_eq!(c["role"], "authenticated");
    assert_eq!(c["aud"], "authenticated");
    assert_eq!(c["email"], "ann-who@example.com");
    assert_eq!(c["app_metadata"]["k2_app"], f.app.as_str());
    assert_eq!(c["iss"], format!("k2:app:{}", f.app), "PRD 5.2: iss names the App id");
    assert!(c["session_id"].as_str().is_some_and(|s| !s.is_empty()) && c["exp"].as_i64().is_some() && c["iat"].as_i64().is_some(), "{c}");
    // A cookie write needs an Origin.
    assert_eq!(refused(resolve(&req("POST", &[("apikey", &pk), ("cookie", &cookie)]), &pub_ctx)), (403, "origin_required".into()));
    assert!(resolve(&req("POST", &[("apikey", &pk), ("cookie", &cookie), ("origin", "https://shop.example")]), &pub_ctx).is_ok());
    // Another App's session in the cookie is no session here.
    let other_cookie = format!("{SESSION_COOKIE}={}", f.other_app_pass);
    assert_eq!(refused(resolve(&req("GET", &[("apikey", &pk), ("cookie", &other_cookie)]), &pub_ctx)), (401, "session_expired".into()));
    // The link carries no cookie lane.
    assert_eq!(refused(resolve(&req("GET", &[("apikey", &pk), ("cookie", &cookie)]), &link_ctx)), (401, "anon_off".into()));

    // A web session's pass as a bearer: refused on the public host (the
    // token never sits in the browser), the person on the link.
    let web_bearer = format!("Bearer {}", f.web);
    assert_eq!(refused(resolve(&req("GET", &[("apikey", &pk), ("authorization", &web_bearer)]), &pub_ctx)), (401, "web_pass_as_bearer".into()));
    let w = resolve(&req("GET", &[("apikey", &pk), ("authorization", &web_bearer)]), &link_ctx).unwrap();
    assert_eq!(w.caller.role, DbRole::Authenticated);
    assert_eq!(w.caller.claims["sub"], f.ann_id.as_str());
    // A device pass: the person, on either.
    let dev_bearer = format!("Bearer {}", f.device);
    for c in [&pub_ctx, &link_ctx] {
        let w = resolve(&req("GET", &[("apikey", &pk), ("authorization", &dev_bearer)]), c).unwrap();
        assert!(matches!(&w.who, Who::Person { device: true, .. }), "{w:?}");
        assert_eq!(w.caller.role, DbRole::Authenticated);
    }
    // Another App's pass, or garbage.
    let other_bearer = format!("Bearer {}", f.other_app_pass);
    assert_eq!(refused(resolve(&req("GET", &[("apikey", &pk), ("authorization", &other_bearer)]), &link_ctx)), (401, "pass_invalid".into()));
    assert_eq!(refused(resolve(&req("GET", &[("apikey", &pk), ("authorization", "Bearer nonsense")]), &link_ctx)), (401, "bad_credential".into()));
    // A third-party token with no issuer configured.
    let jwt = "Bearer eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ4In0.c2ln";
    assert_eq!(refused(resolve(&req("GET", &[("apikey", &pk), ("authorization", jwt)]), &pub_ctx)), (401, "issuer_unknown".into()));

    // The server key: service_role on the link; on the public host only
    // with the owner's setting.
    let sk = f.service_key.as_str();
    let w = resolve(&req("GET", &[("apikey", sk), ("authorization", &format!("Bearer {sk}"))]), &link_ctx).unwrap();
    assert_eq!(w.caller.role, DbRole::Service);
    assert_eq!(w.caller.claims["role"], "service_role");
    assert!(matches!(w.who, Who::Service { .. }));
    // As the bearer beside the publishable key, too.
    let w = resolve(&req("GET", &[("apikey", &pk), ("authorization", &format!("Bearer {sk}"))]), &link_ctx).unwrap();
    assert_eq!(w.caller.role, DbRole::Service);
    assert_eq!(refused(resolve(&req("GET", &[("apikey", sk)]), &pub_ctx)), (401, "server_key_on_public_host".into()));
    let w = resolve(&req("GET", &[("apikey", sk)]), &ctx(&f, public(), |c| c.service_key_public = true)).unwrap();
    assert_eq!(w.caller.role, DbRole::Service);
    // A key without the service scope, or another App's server key.
    assert_eq!(refused(resolve(&req("GET", &[("apikey", &f.users_key)]), &link_ctx)), (401, "bad_server_key".into()));
    assert_eq!(refused(resolve(&req("GET", &[("apikey", &f.other_service_key)]), &link_ctx)), (401, "bad_server_key".into()));
}

/// The coordinator's guard: `service_role` only ever comes from the server
/// key's scope. No session, device pass, cookie or claim reaches it.
#[test]
fn service_role_comes_only_from_the_server_key() {
    let f = fx("svc");
    let pk = publishable_key(&f.svc);
    let cookie = format!("{SESSION_COOKIE}={}", f.web);
    let open = |c: &mut DoorConfig| {
        c.anon = true;
        c.service_key_public = true;
    };
    for ingress in [public(), Ingress::Link] {
        let cx = ctx(&f, ingress.clone(), open);
        for r in [
            req("GET", &[("apikey", &pk)]),
            req("GET", &[("apikey", &pk), ("cookie", &cookie)]),
            req("GET", &[("apikey", &pk), ("authorization", &format!("Bearer {}", f.device))]),
            req("GET", &[("apikey", &pk), ("authorization", &format!("Bearer {}", f.web))]),
            req("GET", &[("apikey", &pk), ("authorization", &format!("Bearer {}", f.users_key))]),
        ] {
            if let Ok(w) = resolve(&r, &cx) {
                assert_ne!(w.caller.role, DbRole::Service, "{ingress:?} {r:?} reached service_role");
                assert!(!matches!(w.who, Who::Service { .. }));
            }
        }
    }
}

#[test]
fn a_third_party_check_can_only_give_authenticated() {
    // Lane A's verifier is a process-wide hook; this test registers one
    // that echoes the role it is told, to prove the door refuses any but
    // `authenticated`.
    fn echo(_: &DoorConfig, token: &str) -> Result<(Value, Option<String>), (u16, String, String)> {
        let role = token.split('.').next().unwrap_or("");
        Ok((json!({ "role": role, "sub": "x" }), None))
    }
    TEST_VERIFIER.with(|t| t.set(Some(echo)));
    let f = fx("tp");
    let pk = publishable_key(&f.svc);
    let cx = ctx(&f, public(), |_| {});
    let w = resolve(&req("GET", &[("apikey", &pk), ("authorization", "Bearer authenticated.e30.c2ln")]), &cx).unwrap();
    assert_eq!(w.caller.role, DbRole::Authenticated);
    for forged in ["service_role", "postgres", "anon"] {
        let h = format!("Bearer {forged}.e30.c2ln");
        let (st, c) = refused(resolve(&req("GET", &[("apikey", &pk), ("authorization", &h)]), &cx));
        assert_eq!((st, c.as_str()), (500, "bad_claims"), "{forged}");
    }
    // A K2 pass in its JWT shape goes to the pass check, never to the
    // issuer check (lane A's issuers::verify sits behind this hook).
    fn never(_: &DoorConfig, t: &str) -> Result<(Value, Option<String>), (u16, String, String)> {
        panic!("a K2 pass reached the third-party check: {t}")
    }
    TEST_VERIFIER.with(|t| t.set(Some(never)));
    let link = ctx(&f, Ingress::Link, |_| {});
    let pass = k2_core::skin::resolve_skin_token(&f.web).unwrap();
    let wrapped = wrap_pass(&f.web, &session_claims(&link.cfg.app_id, &pass));
    assert!(looks_like_jwt(&wrapped), "it has a JWT's shape");
    let w = resolve(&req("GET", &[("apikey", &pk), ("authorization", &format!("Bearer {wrapped}"))]), &link).expect("the pass check takes it");
    assert!(matches!(w.who, Who::Person { .. }), "{w:?}");
    assert_eq!(w.caller.claims["sub"], f.ann_id.as_str());
    TEST_VERIFIER.with(|t| t.set(None));
}

#[test]
fn origins_are_the_host_itself_or_listed() {
    let cfg = cfg_for("o", "shop");
    let r = |o: Option<&str>| {
        let mut h = vec![("host", "shop.example")];
        if let Some(o) = o {
            h.push(("origin", o));
        }
        req("GET", &h)
    };
    assert_eq!(origin_allowed(&cfg, &r(None)).unwrap(), None, "no Origin: a native client or curl");
    assert_eq!(origin_allowed(&cfg, &r(Some("https://shop.example"))).unwrap(), None);
    assert_eq!(origin_allowed(&cfg, &r(Some("HTTPS://Shop.Example/"))).unwrap(), None);
    assert_eq!(origin_allowed(&cfg, &r(Some("https://admin.example.com"))).unwrap().as_deref(), Some("https://admin.example.com"));
    let a = origin_allowed(&cfg, &r(Some("https://evil.example"))).unwrap_err();
    assert_eq!((a.status, code(&a)), (403, "origin_not_allowed".into()));
}

/// On an App's public host the door serves its requests one after another
/// on the same connection, and the first request for the App ends the
/// loop with that request's bytes untouched for the App.
#[tokio::test(flavor = "multi_thread")]
async fn the_public_host_keeps_door_requests_alive_and_hands_the_rest_to_the_app() {
    let f = fx("ka");
    let pk = publishable_key(&f.svc);
    set_test_config(&f.app, Some((cfg_for(&f.app, "nodb_here"), f.svc.clone())));
    let (mut client, mut server) = tokio::io::duplex(64 * 1024);
    let app = f.app.clone();
    let task = tokio::spawn(async move { first_request(&mut server, &app, &app, None, None).await });
    let door_req = format!("GET /rest/v1/notes HTTP/1.1\r\nHost: shop.example\r\napikey: {pk}\r\n\r\n");
    let app_req = "GET /index.html HTTP/1.1\r\nHost: shop.example\r\n\r\n";
    client.write_all(door_req.as_bytes()).await.unwrap();
    client.write_all(door_req.as_bytes()).await.unwrap();
    client.write_all(app_req.as_bytes()).await.unwrap();
    let got = task.await.unwrap();
    match got {
        FirstRequest::Pass(bytes) => assert_eq!(String::from_utf8(bytes).unwrap(), app_req, "the App's request, whole"),
        FirstRequest::Answered => panic!("the App's request must pass"),
    }
    let mut buf = vec![0u8; 16 * 1024];
    let n = tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut buf)).await.unwrap().unwrap();
    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
    // Two answers, both kept alive. No anon lane: 401 before any database.
    assert_eq!(text.matches("HTTP/1.1 401 Unauthorized").count(), 2, "{text}");
    assert_eq!(text.matches("Connection: keep-alive").count(), 2, "{text}");
    assert!(text.contains("\"anon_off\""), "{text}");
    set_test_config(&f.app, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cross_origin_browser_is_refused_before_credentials_and_a_listed_one_gets_cors() {
    let f = fx("cors");
    let pk = publishable_key(&f.svc);
    let cx = ctx(&f, public(), |c| c.anon = true);
    let evil = req("GET", &[("apikey", &pk), ("host", "shop.example"), ("origin", "https://evil.example")]);
    let a = handle(evil, &cx).await;
    assert_eq!((a.status, code(&a)), (403, "origin_not_allowed".into()));
    let pre = req("OPTIONS", &[("host", "shop.example"), ("origin", "https://admin.example.com")]);
    let a = handle(pre, &cx).await;
    assert_eq!(a.status, 204);
    let h = |n: &str| a.headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    assert_eq!(h("Access-Control-Allow-Origin").as_deref(), Some("https://admin.example.com"));
    assert_eq!(h("Access-Control-Allow-Credentials").as_deref(), Some("true"));
    assert!(h("Access-Control-Allow-Headers").unwrap().contains("apikey"));
}
