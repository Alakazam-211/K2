//! The sender against a fake Apple and Google: hosts, headers, the JWT and
//! OAuth caches with an injected clock, every CP21 mapping, the breaker,
//! air-gap, the check, and a log capture that carries no secret. No test
//! here (or anywhere) opens a socket to Apple or Google.

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use k2_core::skin::{self, app_push as store};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::super::payload::{self, Notification, Stamps};
use super::super::test_keys::*;
use super::*;

thread_local! {
    static CLOCK: Cell<i64> = const { Cell::new(1_800_000_000) };
}

fn clock() -> i64 {
    CLOCK.with(|c| c.get())
}

fn advance(secs: i64) {
    CLOCK.with(|c| c.set(c.get() + secs));
}

type Script = Box<dyn Fn(&HttpReq) -> Result<HttpResp, TransportErr> + Send + Sync>;

/// A fake vendor: records every request, answers from a queue (then the
/// default).
struct Fake {
    seen: Mutex<Vec<(String, Vec<(String, String)>, Vec<u8>)>>,
    answers: Mutex<VecDeque<Result<HttpResp, TransportErr>>>,
    default: Script,
}

impl Fake {
    fn new(default: Script) -> Arc<Self> {
        Arc::new(Fake { seen: Mutex::new(Vec::new()), answers: Mutex::new(VecDeque::new()), default })
    }
    fn then(&self, r: Result<HttpResp, TransportErr>) {
        self.answers.lock().expect("lock").push_back(r);
    }
    fn seen(&self) -> Vec<(String, Vec<(String, String)>, Vec<u8>)> {
        self.seen.lock().expect("lock").clone()
    }
}

impl Transport for Fake {
    fn post(&self, r: &HttpReq) -> Result<HttpResp, TransportErr> {
        self.seen.lock().expect("lock").push((r.url.clone(), r.headers.clone(), r.body.clone()));
        if let Some(a) = self.answers.lock().expect("lock").pop_front() {
            return a;
        }
        (self.default)(r)
    }
}

fn resp(status: u16, body: J) -> Result<HttpResp, TransportErr> {
    Ok(HttpResp { status, retry_after: None, body: body.to_string().into_bytes() })
}

/// 200 for every APNs call; an OAuth token for the token URL; 200 for FCM.
fn happy() -> Script {
    Box::new(|r: &HttpReq| {
        if r.url.ends_with("/token") {
            resp(200, json!({"access_token": "ya29.fake-access-token-XYZ", "expires_in": 3600, "token_type": "Bearer"}))
        } else if r.url.contains("/messages:send") {
            resp(200, json!({"name": "projects/k2-test-proj/messages/1"}))
        } else {
            resp(200, json!({}))
        }
    })
}

/// Google's token answer (queued ahead of an FCM answer when the sender
/// has no access token yet, or will refresh one).
fn token_ok() -> Result<HttpResp, TransportErr> {
    resp(200, json!({"access_token": "ya29.fake-access-token-XYZ", "expires_in": 3600}))
}

fn hosts() -> Hosts {
    Hosts {
        apns_production: "http://apns.test".into(),
        apns_sandbox: "http://apns-sandbox.test".into(),
        fcm: "http://fcm.test".into(),
        google_token: "http://google.test/token".into(),
    }
}

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    app: String,
    dana: String,
}

const APNS_TOKEN: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
const FCM_TOKEN: &str = "fcm-device-token-SECRET-0123456789:APA91b";
const TITLE: &str = "Secret title about job 4471";
const BODY: &str = "Secret body for Dana only";
const DATA_VALUE: &str = "secret-data-value-889";

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    crate::vault::ensure_test_vault();
    let ws = format!("ws-{tag}");
    let app = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let path = home.path().join(&ws);
        std::fs::create_dir_all(&path).expect("dir");
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![ws, path.to_string_lossy()]).expect("project");
        k2_core::published_services::insert(&conn, &ws, "jobs", "node s.js", "/tmp", 4330, "local", "running", "cmd", "").expect("svc").id
    };
    let dana = skin::add_principal_for_app("dana", None, &app, None, "test").expect("dana").id;
    let apns = creds::validate_apns(EC_P256_P8, "ABC123DEFG", "TEAM123456", "com.example.app").expect("apns");
    creds::store_apns(&app, &apns, "owner").expect("store apns");
    let sa = json!({
        "type": "service_account", "project_id": "k2-test-proj", "private_key_id": "kid1", "private_key": RSA_P8,
        "client_email": "push@k2-test-proj.iam.gserviceaccount.com", "token_uri": creds::GOOGLE_TOKEN_URI,
    });
    creds::store_fcm(&app, &creds::validate_fcm(&sa.to_string()).expect("fcm"), "owner").expect("store fcm");
    crate::apps::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    Fx { _db: db, _home: home, app, dana }
}

fn device(f: &Fx, id: &str, platform: &str, env: &str) -> Target {
    let s = skin::create_app_session_token("dana", &f.app).expect("session");
    let token = if platform == "apns" { APNS_TOKEN } else { FCM_TOKEN };
    let reg = store::parse_registration(&json!({"deviceId": id, "platform": platform, "pushToken": token, "environment": env})).expect("reg");
    let d = store::register(&f.app, &f.dana, &s.meta.id, &reg).expect("register");
    Target::from(&d)
}

fn note() -> Notification {
    payload::parse(&json!({"title": TITLE, "body": BODY, "data": {"jobId": DATA_VALUE}, "topic": "jobs", "collapseId": "job:1", "sound": "default", "badge": 3}))
        .expect("valid")
}

fn stamps(f: &Fx) -> Stamps {
    Stamps { app: f.app.clone(), send_id: payload::new_send_id(), kind: payload::KIND_APP, who: "ab".repeat(16) }
}

fn header<'a>(h: &'a [(String, String)], k: &str) -> Option<&'a str> {
    h.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

#[test]
fn release_builds_never_take_the_test_hosts() {
    let r = hosts_for(false, Some("http://127.0.0.1:9999"), Some("http://127.0.0.1:9998"));
    assert_eq!(r, Hosts::vendor(), "a release build ignores the overrides");
    let d = hosts_for(true, Some("http://127.0.0.1:9999/"), Some("http://localhost:9998"));
    assert_eq!(d.apns_production, "http://127.0.0.1:9999");
    assert_eq!(d.apns_sandbox, "http://127.0.0.1:9999/sandbox");
    assert_eq!(d.fcm, "http://localhost:9998");
    assert_eq!(d.google_token, "http://localhost:9998/token");
    for bad in ["https://evil.example", "http://10.0.0.1:1", "http://127.0.0.1.evil.example", "ftp://127.0.0.1"] {
        assert_eq!(hosts_for(true, Some(bad), Some(bad)), Hosts::vendor(), "{bad}: loopback http only");
    }
    assert_eq!(Hosts::vendor().apns_production, "https://api.push.apple.com");
    assert_eq!(Hosts::vendor().google_token, "https://oauth2.googleapis.com/token");
    assert_eq!(redact_url("https://api.push.apple.com/3/device/abcdef"), "https://api.push.apple.com/3/device/<redacted>");
}

#[test]
fn apns_sends_with_a_cached_es256_token_per_environment() {
    let f = fx("apns");
    let fake = Fake::new(happy());
    let s = Sender::with_clock(hosts(), fake.clone(), clock);
    let prod = device(&f, "p1", "apns", "production");
    assert_eq!(s.deliver(&prod, &note(), &stamps(&f)), Outcome::Sent);
    let sand = {
        let ses = skin::create_app_session_token("dana", &f.app).expect("session");
        let reg = store::parse_registration(&json!({"deviceId": "p2", "platform": "apns", "pushToken": "ff".repeat(32), "environment": "sandbox"})).expect("reg");
        Target::from(&store::register(&f.app, &f.dana, &ses.meta.id, &reg).expect("register"))
    };
    assert_eq!(s.deliver(&sand, &note(), &stamps(&f)), Outcome::Sent);
    let seen = fake.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].0, format!("http://apns.test/3/device/{APNS_TOKEN}"));
    assert_eq!(seen[1].0, format!("http://apns-sandbox.test/3/device/{}", "ff".repeat(32)));
    let h = &seen[0].1;
    assert_eq!(header(h, "apns-topic"), Some("com.example.app"));
    assert_eq!(header(h, "apns-push-type"), Some("alert"));
    assert_eq!(header(h, "apns-priority"), Some("10"));
    assert_eq!(header(h, "apns-collapse-id"), Some("job:1"));
    assert_eq!(header(h, "apns-expiration"), Some((clock() + 86_400).to_string().as_str()));
    let auth = header(h, "authorization").expect("auth");
    let jwt = auth.strip_prefix("bearer ").expect("bearer");
    let hd = jsonwebtoken::decode_header(jwt).expect("jwt");
    assert_eq!((hd.alg, hd.kid.as_deref()), (jsonwebtoken::Algorithm::ES256, Some("ABC123DEFG")));
    let mut val = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::ES256);
    val.required_spec_claims.clear();
    val.validate_exp = false;
    let claims = jsonwebtoken::decode::<J>(jwt, &jsonwebtoken::DecodingKey::from_ec_pem(EC_P256_PUB.as_bytes()).expect("pub"), &val).expect("verifies").claims;
    assert_eq!(claims, json!({"iss": "TEAM123456", "iat": clock()}));
    assert_eq!(header(&seen[1].1, "authorization"), Some(auth), "one token per App, reused");
    let body: J = serde_json::from_slice(&seen[0].2).expect("json");
    assert_eq!(body["aps"]["alert"], json!({"title": TITLE, "body": BODY}));
    assert_eq!((body["aps"]["badge"].clone(), body["aps"]["sound"].clone()), (json!(3), json!("default")));
    assert_eq!(body["jobId"], DATA_VALUE);
    assert_eq!((body["k2App"].as_str(), body["k2Kind"].as_str(), body["k2Topic"].as_str()), (Some(f.app.as_str()), Some("app"), Some("jobs")));
    let raw = String::from_utf8_lossy(&seen[0].2).to_string();
    assert!(!raw.contains("dana") && !raw.contains(&f.dana), "no username or principal id rides a push: {raw}");

    // 50 minutes later the token is re-minted.
    advance(JWT_REFRESH_SECS);
    assert_eq!(s.deliver(&prod, &note(), &stamps(&f)), Outcome::Sent);
    let seen = fake.seen();
    assert_ne!(header(&seen[2].1, "authorization"), Some(auth), "re-minted after 50 minutes");
}

#[test]
fn apns_answers_map_to_dead_failed_and_the_breaker() {
    let f = fx("apnsmap");
    let fake = Fake::new(happy());
    let s = Sender::with_clock(hosts(), fake.clone(), clock);
    // 410 Unregistered: dead, the row goes.
    let t = device(&f, "p1", "apns", "production");
    fake.then(resp(410, json!({"reason": "Unregistered"})));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Dead { wrong_app: false });
    assert!(store::devices_for_app(&f.app, None).expect("list").is_empty());
    // DeviceTokenNotForTopic: dead and counted as another app's token.
    let t = device(&f, "p1", "apns", "production");
    fake.then(resp(400, json!({"reason": "DeviceTokenNotForTopic"})));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Dead { wrong_app: true });
    // BadDeviceToken: dead.
    let t = device(&f, "p1", "apns", "production");
    fake.then(resp(400, json!({"reason": "BadDeviceToken"})));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Dead { wrong_app: false });
    // A payload problem is failed, and the row stays.
    let t = device(&f, "p1", "apns", "production");
    fake.then(resp(400, json!({"reason": "PayloadTooLarge"})));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Failed("apns:PayloadTooLarge".into()));
    assert_eq!(store::devices_for_app(&f.app, None).expect("list").len(), 1);
    // 5xx, then 200: retried once.
    fake.then(resp(503, json!({"reason": "ServiceUnavailable"})));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Sent);
    // A transport error twice: failed.
    fake.then(Err(TransportErr::Timeout));
    fake.then(Err(TransportErr::Network));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Failed("network".into()));
    // ExpiredProviderToken: re-minted once, then sent.
    let before = fake.seen().len();
    fake.then(resp(403, json!({"reason": "ExpiredProviderToken"})));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Sent);
    assert_eq!(fake.seen().len(), before + 2);
    // TooManyProviderTokenUpdates: failed, and the token is held past its
    // usual 50 minutes.
    advance(JWT_REFRESH_SECS - 10);
    fake.then(resp(429, json!({"reason": "TooManyProviderTokenUpdates"})));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Failed("apns:TooManyProviderTokenUpdates".into()));
    advance(60);
    let held = header(&fake.seen().last().expect("req").1, "authorization").map(str::to_string);
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Sent);
    assert_eq!(header(&fake.seen().last().expect("req").1, "authorization").map(str::to_string), held, "held: no re-mint for 20 minutes after Apple said too many");
    // Any 403: the breaker. The platform pauses, the code is stored.
    fake.then(resp(403, json!({"reason": "InvalidProviderToken"})));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Failed("apns:InvalidProviderToken".into()));
    assert_eq!(store::config(&f.app).expect("cfg").cred_error.as_deref(), Some("apns:InvalidProviderToken"));
    let n = fake.seen().len();
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Failed("apns:paused".into()));
    assert_eq!(fake.seen().len(), n, "a paused platform calls nobody");
    assert!(s.paused(&f.app, "apns") && !s.paused(&f.app, "fcm"));
    s.reset_app(&f.app);
    assert!(!s.paused(&f.app, "apns"), "new keys lift the pause");
    // TopicDisallowed is a credential error too.
    fake.then(resp(400, json!({"reason": "TopicDisallowed"})));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Failed("apns:TopicDisallowed".into()));
    assert!(s.paused(&f.app, "apns"));
}

#[test]
fn fcm_sends_with_a_cached_oauth_token_and_maps_its_errors() {
    let f = fx("fcm");
    let fake = Fake::new(happy());
    let s = Sender::with_clock(hosts(), fake.clone(), clock);
    let t = device(&f, "a1", "fcm", "production");
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Sent);
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Sent);
    let seen = fake.seen();
    assert_eq!(seen.len(), 3, "one token fetch, two sends");
    assert_eq!(seen[0].0, "http://google.test/token");
    let form = String::from_utf8(seen[0].2.clone()).expect("form");
    assert!(form.starts_with("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion="), "{form}");
    let assertion = form.split("assertion=").nth(1).expect("assertion");
    let mut val = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
    val.validate_exp = false;
    val.set_audience(&[creds::GOOGLE_TOKEN_URI]);
    let c = jsonwebtoken::decode::<J>(assertion, &jsonwebtoken::DecodingKey::from_rsa_pem(RSA_PUB.as_bytes()).expect("pub"), &val).expect("verifies").claims;
    assert_eq!(c["aud"], creds::GOOGLE_TOKEN_URI, "the assertion only ever names Google's endpoint");
    assert_eq!(seen[1].0, "http://fcm.test/v1/projects/k2-test-proj/messages:send");
    assert_eq!(header(&seen[1].1, "authorization"), Some("Bearer ya29.fake-access-token-XYZ"));
    let m: J = serde_json::from_slice(&seen[1].2).expect("json");
    assert_eq!(m["message"]["token"], FCM_TOKEN);
    assert_eq!(m["message"]["notification"], json!({"title": TITLE, "body": BODY}));
    assert_eq!(m["message"]["data"]["jobId"], DATA_VALUE);
    assert_eq!(m["message"]["data"]["k2Kind"], "app");
    assert_eq!(m["message"]["android"]["priority"], "HIGH");
    assert_eq!(m["message"]["android"]["notification"]["tag"], "job:1");
    // 55 minutes later the token is refreshed (5 minutes early).
    advance(3600 - OAUTH_EARLY_SECS);
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Sent);
    assert_eq!(fake.seen()[3].0, "http://google.test/token");

    // INVALID_ARGUMENT naming message.token: dead. Naming the payload: failed, kept.
    let bad_tok = json!({"error": {"code": 400, "status": "INVALID_ARGUMENT", "details": [
        {"@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError", "errorCode": "INVALID_ARGUMENT"},
        {"@type": "type.googleapis.com/google.rpc.BadRequest", "fieldViolations": [{"field": "message.token", "description": "Invalid registration token"}]}]}});
    let bad_data = json!({"error": {"code": 400, "status": "INVALID_ARGUMENT", "details": [
        {"@type": "type.googleapis.com/google.rpc.BadRequest", "fieldViolations": [{"field": "message.data[0].key", "description": "bad"}]}]}});
    fake.then(resp(400, bad_data));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Failed("fcm:INVALID_ARGUMENT".into()));
    assert_eq!(store::devices_for_app(&f.app, None).expect("list").len(), 1, "a payload bug never wipes devices");
    fake.then(resp(400, bad_tok));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Dead { wrong_app: false });
    let t = device(&f, "a1", "fcm", "production");
    fake.then(resp(404, json!({"error": {"code": 404, "status": "NOT_FOUND", "details": [{"errorCode": "UNREGISTERED"}]}})));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Dead { wrong_app: false });
    let t = device(&f, "a1", "fcm", "production");
    fake.then(resp(403, json!({"error": {"code": 403, "status": "PERMISSION_DENIED", "details": [{"errorCode": "SENDER_ID_MISMATCH"}]}})));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Dead { wrong_app: true });
    let t = device(&f, "a1", "fcm", "production");
    // 401 once: refresh the access token, then sent.
    fake.then(resp(401, json!({"error": {"code": 401, "status": "UNAUTHENTICATED"}})));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Sent);
    // 429 with Retry-After: the whole App's FCM pauses.
    fake.then(Ok(HttpResp { status: 429, retry_after: Some(120), body: json!({"error": {"status": "RESOURCE_EXHAUSTED", "details": [{"errorCode": "QUOTA_EXCEEDED"}]}}).to_string().into_bytes() }));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Failed("fcm:QUOTA_EXCEEDED".into()));
    assert!(s.paused(&f.app, "fcm"));
    s.reset_app(&f.app);
    // 401 after a refresh too: the breaker (the reset dropped the token:
    // fetch, 401, refresh, 401).
    fake.then(token_ok());
    fake.then(resp(401, json!({"error": {"status": "UNAUTHENTICATED"}})));
    fake.then(token_ok());
    fake.then(resp(401, json!({"error": {"status": "UNAUTHENTICATED"}})));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Failed("fcm:UNAUTHENTICATED".into()));
    assert!(s.paused(&f.app, "fcm"));
    // A refused service account at the token endpoint is a credential error.
    s.reset_app(&f.app);
    fake.then(resp(400, json!({"error": "invalid_grant"})));
    assert_eq!(s.deliver(&t, &note(), &stamps(&f)), Outcome::Failed("fcm:oauth_invalid_grant".into()));
    assert_eq!(store::config(&f.app).expect("cfg").cred_error.as_deref(), Some("fcm:oauth_invalid_grant"));
}

#[test]
fn air_gap_drops_every_send_before_any_call() {
    let f = fx("gap");
    let fake = Fake::new(happy());
    let s = Sender::with_clock(hosts(), fake.clone(), clock);
    let t = device(&f, "p1", "apns", "production");
    crate::push_routes::set_test_airgap(true);
    let out = s.deliver(&t, &note(), &stamps(&f));
    crate::push_routes::set_test_airgap(false);
    assert_eq!(out, Outcome::Dropped);
    assert!(fake.seen().is_empty());
}

#[test]
fn the_check_reads_a_token_refusal_as_a_working_key() {
    let f = fx("check");
    let fake = Fake::new(happy());
    let s = Sender::with_clock(hosts(), fake.clone(), clock);
    fake.then(resp(400, json!({"reason": "BadDeviceToken"})));
    assert_eq!(s.check(&f.app, "apns"), Ok("ok"));
    fake.then(resp(403, json!({"reason": "InvalidProviderToken"})));
    assert_eq!(s.check(&f.app, "apns"), Err("apns:InvalidProviderToken".into()));
    fake.then(token_ok());
    fake.then(resp(400, json!({"error": {"status": "INVALID_ARGUMENT", "details": [{"errorCode": "INVALID_ARGUMENT"}]}})));
    assert_eq!(s.check(&f.app, "fcm"), Ok("ok"));
    assert_eq!(fake.seen()[0].0, format!("http://apns.test/3/device/{}", "0".repeat(64)), "a fake token, production");
    assert!(s.check(&f.app, "web").is_err());
}

#[test]
fn no_log_line_carries_a_token_text_data_jwt_or_key() {
    let f = fx("logs");
    let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    set_log_sink(Some(Box::new(move |l: &str| sink.lock().expect("lock").push(l.to_string()))));
    let fake = Fake::new(happy());
    let s = Sender::with_clock(hosts(), fake.clone(), clock);
    let a = device(&f, "p1", "apns", "production");
    let g = device(&f, "a1", "fcm", "production");
    for r in [
        resp(200, json!({})),
        resp(410, json!({"reason": "Unregistered"})),
        resp(403, json!({"reason": "InvalidProviderToken"})),
    ] {
        fake.then(r);
        s.deliver(&a, &note(), &stamps(&f));
        s.reset_app(&f.app);
    }
    fake.then(token_ok());
    fake.then(resp(429, json!({"error": {"details": [{"errorCode": "QUOTA_EXCEEDED"}]}})));
    s.deliver(&g, &note(), &stamps(&f));
    set_log_sink(None);
    let all = lines.lock().expect("lock").join("\n");
    assert!(all.contains("token gone") && all.contains("key refused") && all.contains("back off"), "the sends logged: {all}");
    let jwt_prefix = fake.seen().iter().find_map(|(_, h, _)| header(h, "authorization").map(|a| a.split('.').next().unwrap_or("").to_string())).expect("auth");
    for secret in [APNS_TOKEN, FCM_TOKEN, TITLE, BODY, DATA_VALUE, "ya29.fake-access-token", "BEGIN PRIVATE KEY", &jwt_prefix[7..], "dana"] {
        assert!(!all.contains(secret), "a log line carries {secret:?}:\n{all}");
    }
}
