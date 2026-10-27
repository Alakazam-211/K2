//! Sew chat-pass mint (S28 §9, §13, §17; K2 work items K7 + K8) through the
//! REAL dispatcher (`k2_daemon::test_harness`): passes per caller type
//! verified against the live `/v1/chat/jwks`, the Owner-role Connect
//! ruling, the bearer-only / browser / tunnel / CSRF / skin-Host belts,
//! rate limit, 503 without an install id, the audit, chat-only login, the
//! owner handoff with a person confirm, and Connect logout → `cu_` revoke.
//!
//! ISOLATION: `$HOME` is a fresh temp dir; the vault opens on a file key
//! under it (never the Keychain); every test serializes on `TEST_LOCK`.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::{Arc, Mutex as StdMutex, Once};
use std::time::Duration;

use k2_core::connect_users::{self, Role};
use k2_core::sew_revoke::{self, SewSessionRevoke};
use k2_daemon::chat_pass::{self, verify_with_jwks};
use k2_daemon::test_harness;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER_TOKEN: &str = "owner-token-chat-pass-k7";
const INSTALL: &str = "123456789";
const AUD: &str = "sew:123456789";

struct Resp {
    status: u16,
    headers: String,
    body: Value,
    raw_body: String,
}

fn http_with(port: u16, method: &str, path: &str, extra: &[(&str, &str)], body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let mut head = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\n");
    if !extra.iter().any(|(n, _)| n.eq_ignore_ascii_case("host")) {
        head.push_str("Host: 127.0.0.1\r\n");
    }
    for (n, v) in extra {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    let payload = body.unwrap_or("");
    if body.is_some() || method == "POST" {
        head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", payload.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(payload.as_bytes()).expect("write body");
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status in {text:?}"));
    let (headers, raw_body) = text.split_once("\r\n\r\n").map(|(h, b)| (h.to_string(), b.to_string())).unwrap_or_default();
    let body = serde_json::from_str(&raw_body).unwrap_or(Value::String(raw_body.clone()));
    Resp { status, headers, body, raw_body }
}

fn pass(port: u16, bearer: Option<&str>, body: &str) -> Resp {
    let auth = bearer.map(|b| format!("Bearer {b}"));
    let mut extra: Vec<(&str, &str)> = Vec::new();
    if let Some(a) = auth.as_deref() {
        extra.push(("Authorization", a));
    }
    http_with(port, "POST", "/cli/sew/pass", &extra, Some(body))
}

fn header<'a>(r: &'a Resp, name: &str) -> Option<&'a str> {
    r.headers.lines().find_map(|l| {
        let (n, v) = l.split_once(':')?;
        n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

fn err_code(r: &Resp) -> String {
    r.body["error"]["code"].as_str().unwrap_or_else(|| panic!("no error code: {}", r.raw_body)).to_string()
}

fn jwks(port: u16) -> Value {
    let r = http_with(port, "GET", "/v1/chat/jwks", &[], None);
    assert_eq!(r.status, 200, "{}", r.raw_body);
    r.body
}

/// Verify a 200 mint against the live JWKS; returns the claims.
fn verified(port: u16, r: &Resp) -> chat_pass::ChatPassClaims {
    assert_eq!(r.status, 200, "{}", r.raw_body);
    assert_eq!(header(r, "Cache-Control"), Some("no-store"));
    let now = chat_pass::keyring::now();
    let c = verify_with_jwks(r.body["pass"].as_str().expect("pass"), &jwks(port), AUD, now).expect("pass verifies");
    assert_eq!(r.body["sub"], c.sub.as_str());
    assert_eq!(r.body["sid"], c.sid.as_str());
    assert_eq!(r.body["jti"], c.jti.as_str());
    assert_eq!(r.body["personProof"], false);
    c
}

struct Env {
    prev_home: Option<std::ffi::OsString>,
    tmp: std::path::PathBuf,
}

impl Env {
    fn set() -> Self {
        let prev_home = std::env::var_os("HOME");
        let tmp = std::env::temp_dir().join(format!("k2-chat-mint-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(tmp.join("home/.k2")).expect("mkdir home");
        std::env::set_var("HOME", tmp.join("home"));
        Self { prev_home, tmp }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        match &self.prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

async fn boot() -> (Env, test_harness::TestDaemon) {
    k2_core::test_isolation::assert_no_prod_env();
    let env = Env::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());
    // What `hostchat enable` leaves: the supervisor row with the install id
    // (the mint reads it live; no side file).
    let mut row = k2_daemon::sew::store::Row::new("0.1.2", 0);
    row.status = "running".into();
    row.install_id = Some(INSTALL.into());
    k2_daemon::sew::store::save(&row).expect("sew row");
    chat_pass::ratelimit::MINTS.reset();
    chat_pass::ratelimit::HANDOFFS.reset();
    chat_pass::handoff::CODES.clear();
    (env, d)
}

fn login(username: &str, role: Role) -> String {
    connect_users::add_user(username, "password-for-tests-1").expect("add_user");
    if role != Role::Member {
        connect_users::set_role(username, role).expect("set_role");
    }
    connect_users::create_session(username)
}

/// A skin principal with a password and its host grant; `chat` adds the
/// chat grant. Returns `(principal id, k2skn_ session, token id)`.
fn guest(name: &str, chat: bool) -> (String, String, String) {
    let p = k2_core::skin::add_principal(name).expect("principal");
    k2_core::skin::set_principal_password(name, Some("s3cret-horse-battery")).expect("pw");
    if chat {
        k2_core::skin::insert_chat_grant_for_tests(&p.id).expect("chat grant");
    }
    let (meta, raw) = k2_core::skin::create_session_token(name).expect("login");
    (p.id, raw, meta.id)
}

/// SW2: a Connect login's first mint queues its `k2:connect:<id>`
/// provision (503 `chat_member_pending`); the harness runs no worker, so
/// record the row delivered. Enables the chat target if needed.
fn deliver_connect_member(port: u16, tok: &str, name: &str) {
    k2_core::tool_hook::allow_target_off_linux_for_tests(true);
    k2_core::tool_hook::set_target("chat", true, Some("/tmp/k2-mint-sew.sock"), Some("/tmp/k2-mint-sew.token"), "owner-token")
        .expect("enable chat target");
    let r = pass(port, Some(tok), "{}");
    assert_eq!((r.status, err_code(&r).as_str()), (503, "chat_member_pending"), "{name}: {}", r.raw_body);
    let uuid = connect_users::user_uuid(name).expect("uuid").expect("exists");
    let row = k2_core::tool_hook::log(50, Some(&format!("k2:connect:{uuid}")))
        .expect("outbox log")
        .into_iter()
        .find(|r| r.op == "provision" && r.state == "pending")
        .unwrap_or_else(|| panic!("{name}: no provision row queued"));
    k2_core::tool_hook::record_done(&row.id, 200, "{}").expect("deliver");
}

static SEEN: StdMutex<Vec<SewSessionRevoke>> = StdMutex::new(Vec::new());

fn recorder() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        sew_revoke::set_sink(Arc::new(|_c: &rusqlite::Connection, op: &SewSessionRevoke| {
            SEEN.lock().unwrap().push(op.clone());
            Ok(())
        }));
    });
    SEEN.lock().unwrap().clear();
}

#[tokio::test(flavor = "multi_thread")]
async fn app_session_owner_token_and_connect_logins_get_their_own_passes() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (env, d) = boot().await;

    // App session with a chat grant → k2:principal, skn_ sid.
    let (pid, raw, tid) = guest("ada", true);
    let r = pass(d.port, Some(&raw), "{}");
    let c = verified(d.port, &r);
    assert_eq!(c.sub, format!("k2:principal:{pid}"));
    assert_eq!(c.sid, format!("skn_{tid}"));
    assert_eq!(c.caps, None, "today's App roles hold no chat caps (K9 adds them)");
    assert_eq!(r.body["provenance"], "app-session");
    assert_eq!(r.body["app"], "host");
    assert_eq!(c.app, None, "spec v1.3: a box login (host) is no App: no app claim");
    assert_eq!(c.exp - c.iat, 60);

    // Same session, chat grant revoked → 403 no_chat_grant (read live).
    k2_core::skin::delete_chat_grant_for_tests(&pid).expect("drop chat");
    let r = pass(d.port, Some(&raw), "{}");
    assert_eq!((r.status, err_code(&r).as_str()), (403, "no_chat_grant"));
    assert!(!r.raw_body.contains("\"pass\""));
    // Host grant only (never had chat) → 403; logged out → 401.
    let (_, raw_bob, tid_bob) = guest("bob", false);
    assert_eq!(err_code(&pass(d.port, Some(&raw_bob), "{}")), "no_chat_grant");
    assert!(k2_core::skin::revoke_token(&tid_bob).expect("logout"));
    assert_eq!(pass(d.port, Some(&raw_bob), "{}").status, 401);
    // Platform pass (session=0) and an API key → 403 credential_not_allowed.
    let (_meta, platform) =
        k2_core::skin::create_token("bff-platform", Some(&["thread:read".to_string()]), &["room-1".to_string()]).expect("platform token");
    let r = pass(d.port, Some(&platform), "{}");
    assert_eq!((r.status, err_code(&r).as_str()), (403, "credential_not_allowed"));
    assert_eq!(err_code(&pass(d.port, Some("k2sk_abcdefghijklmnop"), "{}")), "credential_not_allowed");

    // Owner token → k2:owner, own_ sid, marked owner-token.
    let r = pass(d.port, Some(OWNER_TOKEN), "{}");
    let c = verified(d.port, &r);
    assert_eq!(c.sub, "k2:owner");
    assert!(c.sid.starts_with("own_"), "{}", c.sid);
    assert_eq!(r.body["provenance"], "owner-token");
    assert_eq!(c.app, None, "the owner's pass names no App");

    // Connect logins: every role is its own member (Owner included: K2
    // Connect Owner is server control, not Sew admin). sid = cu_<digest>.
    // SW2: Sew never invents members, so the first mint queues the
    // login's `k2:connect:<id>` provision and answers 503
    // chat_member_pending + Retry-After; once it is delivered, the pass.
    k2_core::tool_hook::allow_target_off_linux_for_tests(true);
    k2_core::tool_hook::set_target("chat", true, Some("/tmp/k2-mint-sew.sock"), Some("/tmp/k2-mint-sew.token"), "owner-token")
        .expect("enable chat target");
    for (name, role, sew_role) in [("olga", Role::Owner, "member"), ("adam", Role::Admin, "member"), ("mia", Role::Member, "member")] {
        let tok = login(name, role);
        let uuid = connect_users::user_uuid(name).expect("uuid").expect("exists");
        let r = pass(d.port, Some(&tok), "{}");
        assert_eq!((r.status, err_code(&r).as_str()), (503, "chat_member_pending"), "{name}: {}", r.raw_body);
        assert_eq!(header(&r, "Retry-After"), Some("2"), "{name}");
        assert!(!r.raw_body.contains("\"pass\""));
        let row = k2_core::tool_hook::log(50, Some(&format!("k2:connect:{uuid}")))
            .expect("outbox log")
            .into_iter()
            .find(|r| r.op == "provision")
            .unwrap_or_else(|| panic!("{name}: no provision row queued"));
        assert_eq!((row.state.as_str(), row.actor.as_str()), ("pending", format!("user:{name}").as_str()));
        let r = pass(d.port, Some(&tok), "{}");
        assert_eq!(err_code(&r), "chat_member_pending", "still queued");
        // The worker delivers it (no worker in the harness: record it).
        k2_core::tool_hook::record_done(&row.id, 200, "{}").expect("deliver");
        let r = pass(d.port, Some(&tok), "{}");
        let c = verified(d.port, &r);
        assert_eq!(c.sub, format!("k2:connect:{uuid}"), "{name}");
        assert_eq!(c.sid, connect_users::session_sid(&tok));
        assert_eq!(r.body["sewRole"], sew_role, "{name}");
        assert_eq!(r.body["provenance"], "connect-login");
        assert_ne!(c.sub, "k2:owner");
        assert_eq!(c.app, None, "a Connect pass names no App");
        // The uuid is stable across mints.
        let again = verified(d.port, &pass(d.port, Some(&tok), "{}"));
        assert_eq!(again.sub, c.sub);
    }

    // The audit names every mint by jti and holds no secret.
    let audit = std::fs::read_to_string(k2_core::auth_audit::path()).expect("audit");
    assert!(audit.contains("\"chat-pass\"") && audit.contains("principal:ada") && audit.contains("user:olga"), "{audit}");
    assert!(audit.contains("no_chat_grant") && audit.contains("credential_not_allowed"), "{audit}");
    assert!(!audit.contains("eyJ"), "no pass in the audit");
    assert!(!audit.contains("k2skn_"), "no app session in the audit");
    assert!(!audit.contains(OWNER_TOKEN), "no owner token in the audit");
    drop(env);
}

#[tokio::test(flavor = "multi_thread")]
async fn pass_route_belts_method_bearer_browser_tunnel_csrf_and_skin_host() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (env, d) = boot().await;
    let (_pid, raw, _tid) = guest("ada", true);

    // GET → 405 (POST-only row), with and without a credential.
    for b in [Some(raw.as_str()), None] {
        let auth = b.map(|x| format!("Bearer {x}"));
        let extra: Vec<(&str, &str)> = auth.iter().map(|a| ("Authorization", a.as_str())).collect();
        assert_eq!(http_with(d.port, "GET", "/cli/sew/pass", &extra, None).status, 405);
        assert_eq!(http_with(d.port, "GET", "/cli/sew/owner-handoff", &extra, None).status, 405);
    }
    // Unknown body field → 400 naming it.
    let r = pass(d.port, Some(&raw), r#"{"x":1}"#);
    assert_eq!(r.status, 400, "{}", r.raw_body);
    assert!(r.body["error"]["hint"].as_str().expect("hint").contains("x"));
    // ?token= is never read → 401.
    let r = http_with(d.port, "POST", &format!("/cli/sew/pass?token={raw}"), &[], Some("{}"));
    assert_eq!((r.status, err_code(&r).as_str()), (401, "unauthenticated"));
    assert!(!r.raw_body.contains("\"pass\""));
    // The skin cookie is never read off a skin Host (no fold) → 401 …
    let cookie = format!("k2_skin_session={raw}");
    let r = http_with(d.port, "POST", "/cli/sew/pass", &[("Cookie", &cookie)], Some("{}"));
    assert_eq!((r.status, err_code(&r).as_str()), (401, "unauthenticated"));
    // … on a skin Host the folded cookie dies at the CSRF gate (S28-A6) …
    let r = http_with(d.port, "POST", "/cli/sew/pass", &[("Host", "skin.test.k2.dev"), ("Cookie", &cookie)], Some("{}"));
    assert_eq!(r.status, 403, "{}", r.raw_body);
    assert!(r.raw_body.contains("csrf"), "{}", r.raw_body);
    // … and with the web header on a skin Host it dies at the Host belt.
    let r = http_with(
        d.port,
        "POST",
        "/cli/sew/pass",
        &[("Host", "skin.test.k2.dev"), ("Cookie", &cookie), ("X-K2-Client", "web"), ("Authorization", &format!("Bearer {raw}"))],
        Some("{}"),
    );
    assert_eq!(r.status, 403, "{}", r.raw_body);
    assert!(!r.raw_body.contains("\"pass\""));
    // Browser headers → 403 browser_not_allowed.
    let auth = format!("Bearer {raw}");
    for h in [("Origin", "https://evil.example"), ("Sec-Fetch-Site", "cross-site"), ("Sec-Fetch-Mode", "navigate")] {
        let r = http_with(d.port, "POST", "/cli/sew/pass", &[("Authorization", &auth), h], Some("{}"));
        assert_eq!((r.status, err_code(&r).as_str()), (403, "browser_not_allowed"), "{h:?}");
    }
    // Node's fetch (a BFF on @k2/server) adds only `Sec-Fetch-Mode: cors`:
    // a server, so it mints. (Its own guest: ada's sid keeps its whole
    // rate-limit budget for the check below.)
    let (_bff_pid, bff_raw, _bff_tid) = guest("bff", true);
    let bff_auth = format!("Bearer {bff_raw}");
    let r = http_with(d.port, "POST", "/cli/sew/pass", &[("Authorization", &bff_auth), ("Sec-Fetch-Mode", "cors")], Some("{}"));
    assert_eq!(r.status, 200, "{}", r.raw_body);
    // Tunnel ingress → 403 loopback_only (owner token included).
    for b in [raw.as_str(), OWNER_TOKEN] {
        let r = pass(d.tunnel_port, Some(b), "{}");
        assert_eq!((r.status, err_code(&r).as_str()), (403, "loopback_only"));
    }
    // An agent passport is refused (never an agent verb on the cell).
    assert!(!k2_daemon::session_token::is_agent_verb("/cli/sew/pass"));
    assert!(!k2_daemon::session_token::is_agent_verb("/cli/sew/owner-handoff"));
    // The skin gateway never proxies /cli/sew/.
    assert!(k2_daemon::skin_gateway::never_proxy("/cli/sew/pass"));
    assert!(k2_daemon::skin_gateway::never_proxy("/cli/sew/keys/rotate"));

    // Rate limit: 10 per sid per 60 s, then 429 + Retry-After.
    for i in 0..10 {
        assert_eq!(pass(d.port, Some(&raw), "{}").status, 200, "mint {i}");
    }
    let r = pass(d.port, Some(&raw), "{}");
    assert_eq!((r.status, err_code(&r).as_str()), (429, "rate_limited"));
    assert!(header(&r, "Retry-After").expect("Retry-After").parse::<i64>().expect("secs") > 0);
    assert!(r.body["error"]["details"]["retry_after_ms"].as_i64().expect("ms") > 0);

    // No install id (Sew not enabled) → 503 chat_unavailable.
    k2_daemon::sew::store::delete().expect("purge the row");
    let r = pass(d.port, Some(OWNER_TOKEN), "{}");
    assert_eq!((r.status, err_code(&r).as_str()), (503, "chat_unavailable"));
    drop(env);
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_only_principal_logs_in_thread_dark_and_mints() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (env, d) = boot().await;
    let p = k2_core::skin::add_principal("cleo").expect("principal");
    k2_core::skin::set_principal_password("cleo", Some("s3cret-horse-battery")).expect("pw");
    assert!(k2_core::skin::delete_grant_by_key("principal", &p.id, "app", "host").expect("drop host grant"));
    let login_body = json!({ "username": "cleo", "password": "s3cret-horse-battery" }).to_string();
    // Neither grant → the usual 401.
    let r = http_with(d.port, "POST", "/cli/skin/login", &[], Some(&login_body));
    assert_eq!(r.status, 401, "{}", r.raw_body);
    // Chat only → login 200, Thread dark, mint 200.
    k2_core::skin::insert_chat_grant_for_tests(&p.id).expect("chat grant");
    let r = http_with(d.port, "POST", "/cli/skin/login", &[], Some(&login_body));
    assert_eq!(r.status, 200, "{}", r.raw_body);
    let tok = r.body["token"].as_str().unwrap_or_else(|| panic!("token in {}", r.raw_body)).to_string();
    let t = http_with(d.port, "GET", "/cli/thread?addr=room-1", &[("Authorization", &format!("Bearer {tok}"))], None);
    assert_eq!(t.status, 403, "Thread stays dark: {}", t.raw_body);
    let c = verified(d.port, &pass(d.port, Some(&tok), "{}"));
    assert_eq!(c.sub, format!("k2:principal:{}", p.id));
    drop(env);
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_handoff_needs_a_person_and_redeems_once_for_k2_owner() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (env, d) = boot().await;
    let auth = format!("Bearer {OWNER_TOKEN}");
    // The owner token alone is not a person.
    let r = http_with(d.port, "POST", "/cli/sew/owner-handoff", &[("Authorization", &auth)], Some("{}"));
    assert_eq!((r.status, err_code(&r).as_str()), (403, "vault_confirm_required"));
    // An Admin Connect login can't open Sew as owner, even confirmed.
    let admin = login("adam", Role::Admin);
    let r = http_with(d.port, "POST", "/cli/sew/owner-handoff", &[("Authorization", &format!("Bearer {admin}"))], Some("{}"));
    assert_eq!(r.status, 403, "{}", r.raw_body);
    // Tunnel → 403.
    let r = http_with(d.tunnel_port, "POST", "/cli/sew/owner-handoff", &[("Authorization", &auth)], Some("{}"));
    assert_eq!(r.status, 403, "{}", r.raw_body);
    // With a person's confirm (Touch ID / vault password stand-in).
    let confirm = k2_daemon::vault::get().expect("vault").confirm.mint("chat-owner-handoff", None, "owner", "local-auth");
    let b = json!({ "confirmToken": confirm }).to_string();
    let r = http_with(d.port, "POST", "/cli/sew/owner-handoff", &[("Authorization", &auth)], Some(&b));
    assert_eq!(r.status, 200, "{}", r.raw_body);
    assert_eq!(header(&r, "Cache-Control"), Some("no-store"));
    let code = r.body["code"].as_str().expect("code").to_string();
    assert!(code.starts_with("k2sho_") && code.len() == 49, "{code}");
    // The confirm is single use.
    let r = http_with(d.port, "POST", "/cli/sew/owner-handoff", &[("Authorization", &auth)], Some(&b));
    assert_eq!(r.status, 403, "{}", r.raw_body);

    // Redeem: no bearer → k2:owner, own_ sid of the code.
    let hb = json!({ "handoff": code }).to_string();
    let r = http_with(d.port, "POST", "/cli/sew/pass", &[("Authorization", &auth)], Some(&hb));
    assert_eq!(r.status, 400, "handoff and bearer are exclusive: {}", r.raw_body);
    let r = pass(d.port, None, &hb);
    let c = verified(d.port, &r);
    assert_eq!(c.sub, "k2:owner");
    assert_eq!(c.sid, chat_pass::handoff::sid_for(&code));
    assert_eq!(r.body["provenance"], "owner-handoff");
    // Single use.
    let r = pass(d.port, None, &hb);
    assert_eq!((r.status, err_code(&r).as_str()), (401, "handoff_invalid"));
    let audit = std::fs::read_to_string(k2_core::auth_audit::path()).expect("audit");
    assert!(audit.contains("chat-handoff") && audit.contains("minted:") && audit.contains("redeemed:"), "{audit}");
    assert!(!audit.contains(&code), "never the code");
    drop(env);
}

/// Sew 2026-10-18: the owner's linked Connect login IS the owner person,
/// so its pass names `k2:owner` (one person, one member) with ITS `cu_`
/// sid, minted at once: no `connect` grant, no `k2:connect` provision, no
/// "retry shortly". An unlinked login on the same box still gets its own
/// `k2:connect` member.
#[tokio::test(flavor = "multi_thread")]
async fn owner_linked_connect_login_mints_k2_owner_with_its_cu_sid() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (env, d) = boot().await;
    k2_core::tool_hook::allow_target_off_linux_for_tests(true);
    k2_core::tool_hook::set_target("chat", true, Some("/tmp/k2-mint-sew.sock"), Some("/tmp/k2-mint-sew.token"), "owner-token")
        .expect("enable chat target");
    let tok = login("z3thon", Role::Member);
    let uuid = connect_users::user_uuid("z3thon").expect("uuid").expect("exists");
    k2_core::app_settings::update(json!({ "ownerAccounts": ["z3thon"] })).expect("link owner account");

    let r = pass(d.port, Some(&tok), "{}");
    let c = verified(d.port, &r);
    assert_eq!(c.sub, "k2:owner");
    assert_eq!(c.sid, connect_users::session_sid(&tok), "the Connect session's own sid");
    assert!(c.sid.starts_with("cu_"), "{}", c.sid);
    assert_eq!(r.body["provenance"], "connect-login");
    assert_eq!(r.body["sewRole"], "install-owner");
    assert_eq!(c.caps, None, "a Connect pass's own ceiling (none today), not widened");
    assert_eq!(c.app, None);
    // Never a connect grant or a k2:connect row for it.
    let ext = format!("k2:connect:{uuid}");
    assert!(k2_core::tool_hook::log(50, Some(&ext)).expect("log").is_empty(), "no k2:connect rows");
    assert!(
        k2_core::skin::list_grants().expect("grants").iter().all(|g| g.subject_kind != "connect"),
        "no connect grant"
    );
    // A second session of the same login gets its own cu_ sid.
    let tok2 = connect_users::create_session("z3thon");
    let c2 = verified(d.port, &pass(d.port, Some(&tok2), "{}"));
    assert_eq!((c2.sub.as_str(), c2.sid.clone()), ("k2:owner", connect_users::session_sid(&tok2)));
    assert_ne!(c2.sid, c.sid);

    // Unlinked: its own member, queued first.
    let other = login("mia", Role::Member);
    let r = pass(d.port, Some(&other), "{}");
    assert_eq!((r.status, err_code(&r).as_str()), (503, "chat_member_pending"), "{}", r.raw_body);
    drop(env);
}

#[tokio::test(flavor = "multi_thread")]
async fn revocations_reach_sew_as_ops() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (env, d) = boot().await;
    recorder();
    // Connect logout → {k2_sid: cu_…} equal to the minted sid.
    let tok = login("olga", Role::Owner);
    deliver_connect_member(d.port, &tok, "olga");
    let minted = verified(d.port, &pass(d.port, Some(&tok), "{}"));
    let r = http_with(d.port, "POST", "/cli/auth/logout", &[("Authorization", &format!("Bearer {tok}"))], Some("{}"));
    assert!(r.status == 200 || r.status == 204, "{}", r.raw_body);
    assert_eq!(SEEN.lock().unwrap().clone(), vec![SewSessionRevoke::K2Sid(minted.sid.clone())]);
    assert_eq!(pass(d.port, Some(&tok), "{}").status, 401, "the logged-out login mints nothing");
    // Owner revokes an App session → {k2_sid: skn_…}.
    let (_pid, raw, tid) = guest("ada", true);
    let minted = verified(d.port, &pass(d.port, Some(&raw), "{}"));
    let r = http_with(d.port, "POST", "/cli/skin/logout", &[("Authorization", &format!("Bearer {raw}"))], Some("{}"));
    assert_eq!(r.status, 200, "{}", r.raw_body);
    assert_eq!(SEEN.lock().unwrap().last().cloned(), Some(SewSessionRevoke::K2Sid(minted.sid)));
    assert_eq!(k2_core::sew_revoke::skin_sid(&tid), format!("skn_{tid}"));
    // A Connect role change (log out everywhere) → one op per session.
    let t1 = login("mia", Role::Member);
    let t2 = connect_users::create_session("mia");
    SEEN.lock().unwrap().clear();
    connect_users::set_role("mia", Role::Admin).expect("role change");
    let mut got = SEEN.lock().unwrap().clone();
    got.sort_by_key(|o| format!("{o:?}"));
    let mut want = vec![SewSessionRevoke::K2Sid(connect_users::session_sid(&t1)), SewSessionRevoke::K2Sid(connect_users::session_sid(&t2))];
    want.sort_by_key(|o| format!("{o:?}"));
    assert_eq!(got, want);
    drop(env);
}
