//! Sew chat-pass keys (S28 §8, §10; K2 work item K6) through the REAL
//! dispatcher (`k2_daemon::test_harness`): the public `GET /v1/chat/jwks`
//! (loopback and tunnel, while the `/v1` surface is dark, during boot
//! readiness, cache headers), and the owner key routes (method guard,
//! tunnel refusal, role gate, app pass, emergency person-confirm).
//!
//! ISOLATION: `$HOME` points at a fresh temp dir; the vault opens on a file
//! key under that home (never the Keychain); the DB is the in-memory test
//! DB. Every test serializes on `TEST_LOCK`.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::connect_users::{self, Role};
use k2_daemon::test_harness;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

const OWNER_TOKEN: &str = "owner-token-chat-pass-k6";

struct Resp {
    status: u16,
    headers: String,
    body: Value,
    raw_body: String,
}

fn http(port: u16, method: &str, path: &str, bearer: Option<&str>, body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(b) = bearer {
        head.push_str(&format!("Authorization: Bearer {b}\r\n"));
    }
    let payload = body.unwrap_or("");
    if body.is_some() || method == "POST" {
        head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", payload.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(payload.as_bytes()).expect("write body");
    stream.flush().expect("flush");
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

fn header<'a>(r: &'a Resp, name: &str) -> Option<&'a str> {
    r.headers.lines().find_map(|l| {
        let (n, v) = l.split_once(':')?;
        n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

fn kids(v: &Value) -> Vec<String> {
    v["keys"].as_array().unwrap_or_else(|| panic!("no keys in {v}")).iter().map(|k| k["kid"].as_str().expect("kid").to_string()).collect()
}

fn assert_public_only(text: &str) {
    assert!(!text.contains("PRIVATE KEY"), "private key in {text}");
    assert!(!text.contains("\"d\""), "private JWK member in {text}");
}

struct Env {
    prev_home: Option<std::ffi::OsString>,
    prev_api: Option<std::ffi::OsString>,
    tmp: std::path::PathBuf,
}

impl Env {
    fn set() -> Self {
        let prev_home = std::env::var_os("HOME");
        let prev_api = std::env::var_os("K2_API");
        let tmp = std::env::temp_dir().join(format!(
            "k2-chat-pass-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos()
        ));
        std::fs::create_dir_all(tmp.join("home")).expect("mkdir home");
        std::env::set_var("HOME", tmp.join("home"));
        // The /v1 API surface stays dark: the JWKS must not depend on it.
        std::env::remove_var("K2_API");
        Self { prev_home, prev_api, tmp }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        match &self.prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        if let Some(v) = &self.prev_api {
            std::env::set_var("K2_API", v);
        }
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

fn login(username: &str, role: Role) -> String {
    connect_users::add_user(username, "password-for-tests-1").expect("add_user");
    connect_users::set_role(username, role).expect("set_role");
    connect_users::create_session(username)
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_jwks_is_public_cacheable_and_served_while_v1_is_dark_and_during_readiness() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    let _env = Env::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());

    // The /v1 surface is dark: an ordinary /v1 path is 404, the chat JWKS is not.
    let dark = http(d.port, "GET", "/v1/sandboxes", None, None);
    assert_eq!(dark.status, 404, "{}", dark.raw_body);

    let r = http(d.port, "GET", "/v1/chat/jwks", None, None);
    assert_eq!(r.status, 200, "{}", r.raw_body);
    assert_eq!(header(&r, "Cache-Control"), Some("public, max-age=300"));
    let first = kids(&r.body);
    assert_eq!(first.len(), 2, "current + next: {}", r.body);
    for k in r.body["keys"].as_array().expect("keys") {
        let mut names: Vec<&str> = k.as_object().expect("jwk").keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["alg", "crv", "kid", "kty", "use", "x", "y"]);
    }
    assert_public_only(&r.raw_body);

    // Same keys on the next request (no regeneration), and through the tunnel.
    assert_eq!(kids(&http(d.port, "GET", "/v1/chat/jwks", None, None).body), first);
    let t = http(d.tunnel_port, "GET", "/v1/chat/jwks", None, None);
    assert_eq!(t.status, 200, "{}", t.raw_body);
    assert_eq!(kids(&t.body), first);

    // Not the capability key set.
    let cap = http(d.port, "GET", "/v1/jwks", None, None);
    assert_eq!(cap.status, 200, "{}", cap.raw_body);
    for k in kids(&cap.body) {
        assert!(!first.contains(&k), "chat kid equals the capability kid");
    }

    // GET only.
    let p = http(d.port, "POST", "/v1/chat/jwks", None, Some("{}"));
    assert_eq!(p.status, 405, "{}", p.raw_body);

    // The approval JWKS URL is reserved, not served (dark /v1 → 404).
    assert_eq!(http(d.port, "GET", "/v1/chat/jwks/approval", None, None).status, 404);

    // Readiness: served while the daemon is still migrating; real routes 503.
    k2_daemon::boot_status::set_migrating("k6 test");
    let m = http(d.port, "GET", "/v1/chat/jwks", None, None);
    let blocked = http(d.port, "GET", "/cli/sew/keys", Some(OWNER_TOKEN), None);
    k2_daemon::boot_status::set_ready();
    assert_eq!(m.status, 200, "{}", m.raw_body);
    assert_eq!(kids(&m.body), first);
    assert_eq!(blocked.status, 503, "{}", blocked.raw_body);
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_key_routes_gate_method_ingress_role_and_person() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    let env = Env::set();
    let d = test_harness::start(OWNER_TOKEN).await;
    k2_daemon::vault::boot(&k2_core::paths::k2_home());

    // Before any JWKS/mint: no keyring, and reading doesn't make one.
    let r = http(d.port, "GET", "/cli/sew/keys", Some(OWNER_TOKEN), None);
    assert_eq!(r.status, 200, "{}", r.raw_body);
    assert_eq!(r.body["exists"], false);
    assert_eq!(header(&r, "Cache-Control"), Some("no-store"));

    let jwks0 = kids(&http(d.port, "GET", "/v1/chat/jwks", None, None).body);
    let r = http(d.port, "GET", "/cli/sew/keys", Some(OWNER_TOKEN), None);
    assert_eq!(r.status, 200, "{}", r.raw_body);
    assert_eq!(r.body["exists"], true);
    let listed: Vec<String> = r.body["keys"].as_array().expect("keys").iter().map(|k| k["kid"].as_str().expect("kid").to_string()).collect();
    assert_eq!(listed, jwks0);
    assert_eq!(r.body["keys"][0]["state"], "current");
    assert_eq!(r.body["keys"][1]["state"], "next");
    assert_public_only(&r.raw_body);

    // POST-only routes 405 on GET (before auth).
    for p in ["/cli/sew/keys/rotate", "/cli/sew/keys/drop-previous"] {
        for bearer in [Some(OWNER_TOKEN), None] {
            let r = http(d.port, "GET", p, bearer, None);
            assert_eq!(r.status, 405, "{p}: {}", r.raw_body);
        }
    }

    // Tunnel ingress: refused even with the owner token; nothing rotates.
    let r = http(d.tunnel_port, "POST", "/cli/sew/keys/rotate", Some(OWNER_TOKEN), Some("{}"));
    assert_eq!(r.status, 403, "{}", r.raw_body);
    let r = http(d.tunnel_port, "GET", "/cli/sew/keys", Some(OWNER_TOKEN), None);
    assert_eq!(r.status, 403, "{}", r.raw_body);

    // No token / an app pass / a Member login are refused; an Admin login reads.
    let r = http(d.port, "GET", "/cli/sew/keys", None, None);
    assert_eq!(r.status, 401, "{}", r.raw_body);
    let r = http(d.port, "POST", "/cli/sew/keys/rotate", Some("k2skn_not_a_person_credential"), Some("{}"));
    assert_eq!(r.status, 403, "{}", r.raw_body);
    assert_eq!(r.body["error"]["code"], "credential_not_allowed", "{}", r.body);
    let member = login("k6member", Role::Member);
    let r = http(d.port, "POST", "/cli/sew/keys/rotate", Some(&member), Some("{}"));
    assert_eq!(r.status, 403, "{}", r.raw_body);
    let r = http(d.port, "GET", "/cli/sew/keys", Some(&member), None);
    assert_eq!(r.status, 403, "{}", r.raw_body);
    let admin = login("k6admin", Role::Admin);
    let r = http(d.port, "GET", "/cli/sew/keys", Some(&admin), None);
    assert_eq!(r.status, 200, "{}", r.raw_body);

    // A normal rotation right after bootstrap waits for the publish lead.
    let r = http(d.port, "POST", "/cli/sew/keys/rotate", Some(&admin), Some("{}"));
    assert_eq!(r.status, 409, "{}", r.raw_body);
    assert_eq!(r.body["error"]["code"], "lead_too_short");
    assert!(r.body["error"]["details"]["retry_after_ms"].as_i64().expect("retry_after_ms") > 0);

    // Emergency: the owner token alone is not a person.
    let r = http(d.port, "POST", "/cli/sew/keys/rotate", Some(OWNER_TOKEN), Some(r#"{"emergency":true}"#));
    assert_eq!(r.status, 403, "{}", r.raw_body);
    assert_eq!(r.body["error"]["code"], "vault_confirm_required");
    assert_eq!(kids(&http(d.port, "GET", "/v1/chat/jwks", None, None).body), jwks0, "nothing changed");

    // With the person's confirm (Touch ID / vault password stand-in) it runs.
    let tok = k2_daemon::vault::get().expect("vault open").confirm.mint("chat-keys-emergency", None, "owner", "local-auth");
    let b = json!({ "emergency": true, "confirmToken": tok }).to_string();
    let r = http(d.port, "POST", "/cli/sew/keys/rotate", Some(OWNER_TOKEN), Some(&b));
    assert_eq!(r.status, 200, "{}", r.raw_body);
    assert_public_only(&r.raw_body);
    let jwks1 = kids(&http(d.port, "GET", "/v1/chat/jwks", None, None).body);
    assert_eq!(jwks1.len(), 2);
    for k in &jwks0 {
        assert!(!jwks1.contains(k), "old kid {k} still published");
    }
    // Single use.
    let r = http(d.port, "POST", "/cli/sew/keys/rotate", Some(OWNER_TOKEN), Some(&b));
    assert_eq!(r.status, 403, "{}", r.raw_body);

    // The auth audit names the step and holds no key material.
    let audit = std::fs::read_to_string(k2_core::paths::k2_home().join("auth-audit.jsonl")).expect("auth audit written");
    assert!(audit.contains("chat-keys-emergency"), "{audit}");
    assert!(!audit.contains("PRIVATE KEY"));
    // No key files outside the vault.
    let k2 = k2_core::paths::k2_home();
    assert!(!k2.join("chat-pass").exists(), "no ~/.k2/chat-pass dir");
    drop(env);
}
