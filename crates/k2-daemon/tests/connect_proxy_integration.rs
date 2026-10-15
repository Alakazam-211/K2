//! `POST /cli/connect/proxy` (0.45.7) through the REAL dispatcher
//! (`k2_daemon::test_harness`), with an in-memory tunnel-token vault and a
//! fake K2 Connect control plane (`K2_CONNECT_BASE`).
//!
//! Since 0.45.4 a desktop keeps the tunnel token in the K2 vault and
//! `tunnel.json`'s `token` is empty, so the `k2` CLI's own control-plane
//! calls (`k2 publish status`, `subdomain list|create|point|rm`) had no
//! token. The CLI now asks the daemon to make them. Pinned here:
//!
//!   1. The owner token (and an Admin login) gets the control plane's
//!      status + body back, and the request the control plane sees carries
//!      the VAULT token (tunnel.json has none). Bodies are forwarded.
//!   2. A control-plane error passes through as `{status: 4xx, body}`.
//!   3. An endpoint off the allowlist → 400 `not_allowed`, nothing sent.
//!   4. GET → 405. Agent passport, app pass, Member login, no token → 403;
//!      nothing sent.
//!   5. An unreadable vault → 503 `token_unreadable`, nothing sent, token
//!      kept. No token at all → 400 `no_token`. Air-gap → 403.
//!   6. The token is never in a response, and never written anywhere under
//!      `$HOME` (logs included).
//!   7. An agent, with only its passport, runs the REAL `cli/k2`
//!      (`publish subdomain create` → `target` → `delete`, `publish
//!      status`, `subdomain list`) end to end; the label is claimed for
//!      its workspace and released on delete.
//!   8. An agent cannot touch another workspace's label, an unclaimed
//!      label, the primary, a reserved label or a non-allowlisted
//!      endpoint, and its creates/deletes are rate-limited per workspace.
//!
//! ISOLATION: one in-memory vault for this test binary (the holder is a
//! process `OnceLock`); every test serializes on `TEST_LOCK`, runs under a
//! fresh temp `$HOME`, and points `K2_CONNECT_BASE` at its own fake.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream as StdTcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use k2_core::session::SessionId;
use k2_core::test_env::{EnvVar, TempHome};
use k2_core::tunnel::config::{self, TokenVault, TunnelConfig};
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

const OWNER_TOKEN: &str = "owner-token-connect-proxy-5c1e";
const VAULT_TOKEN: &str = "k2c_rosson_vaulttoken9d8e7f6a5b4c";

// ── in-memory vault (installed once for this binary) ──────────────────

static VAULT_TOKEN_SLOT: StdMutex<Option<String>> = StdMutex::new(None);
static VAULT_FAIL_READS: AtomicBool = AtomicBool::new(false);

struct MemVault;

impl TokenVault for MemVault {
    fn get(&self) -> Result<Option<String>, String> {
        if VAULT_FAIL_READS.load(Ordering::SeqCst) {
            return Err("test vault: locked".to_string());
        }
        Ok(VAULT_TOKEN_SLOT.lock().expect("vault").clone())
    }
    fn set(&self, token: &str) -> Result<(), String> {
        *VAULT_TOKEN_SLOT.lock().expect("vault") = Some(token.to_string());
        Ok(())
    }
    fn clear(&self) -> Result<(), String> {
        *VAULT_TOKEN_SLOT.lock().expect("vault") = None;
        Ok(())
    }
}

fn install_vault() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        assert!(config::install_token_vault(Box::new(MemVault)), "vault holder installs once");
    });
    VAULT_FAIL_READS.store(false, Ordering::SeqCst);
    *VAULT_TOKEN_SLOT.lock().expect("vault") = None;
}

/// Pair the device the desktop way: token into the vault, tunnel.json
/// without it.
fn seed_vaulted_tunnel() {
    config::save(&TunnelConfig {
        token: VAULT_TOKEN.to_string(),
        subdomain: "rosson".to_string(),
        ..Default::default()
    })
    .expect("seed tunnel config");
    assert_eq!(VAULT_TOKEN_SLOT.lock().expect("vault").as_deref(), Some(VAULT_TOKEN));
    let raw = std::fs::read_to_string(config::config_path()).expect("read tunnel.json");
    let on_disk: Value = serde_json::from_str(&raw).expect("tunnel.json is JSON");
    assert_eq!(on_disk["token"], json!(""), "setup: tunnel.json keeps no token: {raw}");
}

// ── fake control plane ────────────────────────────────────────────────

struct FakeCp {
    base: String,
    seen: Arc<StdMutex<Vec<String>>>,
}

impl FakeCp {
    fn start(status: &'static str, body: &'static str) -> Self {
        Self::by_method(move |_| (status, body))
    }

    /// Answer by request method (GET / POST / PUT / DELETE).
    fn by_method(reply: impl Fn(&str) -> (&'static str, &'static str) + Send + 'static) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake control plane");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let rec = seen.clone();
        std::thread::spawn(move || {
            while let Ok((mut sock, _)) = listener.accept() {
                sock.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = sock.read(&mut buf).expect("fake cp read");
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                        let clen = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().expect("clen"))
                            })
                            .unwrap_or(0);
                        if rest.len() >= clen {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&raw).into_owned();
                let (status, body) = reply(text.split(' ').next().unwrap_or(""));
                rec.lock().expect("seen").push(text);
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).expect("fake cp write");
            }
        });
        FakeCp { base: format!("http://127.0.0.1:{port}"), seen }
    }

    fn requests(&self) -> Vec<String> {
        self.seen.lock().expect("seen").clone()
    }
}

// ── HTTP client ───────────────────────────────────────────────────────

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream.set_read_timeout(Some(Duration::from_secs(40))).expect("set read timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let text = String::from_utf8_lossy(&raw).into_owned();
        if let Some((head, rest)) = text.split_once("\r\n\r\n") {
            let clen = head.lines().find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse::<usize>().expect("clen"))
            });
            if clen.is_some_and(|c| rest.len() >= c) {
                break;
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status line: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).expect("response body");
    Resp { status, body }
}

fn proxy(port: u16, token: &str, body: &str) -> Resp {
    let r = http(port, "POST", &format!("/cli/connect/proxy?token={token}"), Some(body));
    assert!(!r.body.contains(VAULT_TOKEN), "the token must never be in a response: {}", r.body);
    r
}

fn json_of(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

/// users/add (+ set-role) via the owner token, then log in; returns the
/// session token.
fn login_as(port: u16, username: &str, role: &str) -> String {
    let pw = "connect-proxy-pass-1";
    let r = http(
        port,
        "POST",
        &format!("/cli/users/add?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"username":"{username}","password":"{pw}","mustChangePassword":false}}"#)),
    );
    assert_eq!(r.status, 200, "users/add {username}: {}", r.body);
    if role != "member" {
        let r = http(
            port,
            "POST",
            &format!("/cli/users/set-role?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"username":"{username}","role":"{role}"}}"#)),
        );
        assert_eq!(r.status, 200, "set-role {username}: {}", r.body);
    }
    let r = http(
        port,
        "POST",
        "/cli/auth/login",
        Some(&format!(r#"{{"username":"{username}","password":"{pw}"}}"#)),
    );
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    json_of(&r.body)["token"].as_str().expect("session token").to_string()
}

/// Every file under `$HOME` (logs, stores, tunnel.json) — none may hold
/// the vault token.
fn assert_token_nowhere_under(dir: &std::path::Path) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let p = entry.expect("dir entry").path();
        if p.is_dir() {
            assert_token_nowhere_under(&p);
        } else if let Ok(bytes) = std::fs::read(&p) {
            assert!(
                !String::from_utf8_lossy(&bytes).contains(VAULT_TOKEN),
                "the tunnel token was written to {}",
                p.display()
            );
        }
    }
}

fn setup() -> (std::sync::MutexGuard<'static, ()>, TempHome) {
    let g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = TempHome::new();
    install_vault();
    (g, home)
}

// ── (1)(2)(6) owner + Admin get the control plane's answer ────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_and_admin_proxy_with_the_vault_token() {
    let (_g, home) = setup();
    let _ag = EnvVar::remove("K2_AIRGAP");
    seed_vaulted_tunnel();
    let d = futures_block(test_harness::start(OWNER_TOKEN));

    // GET /subdomains with the owner token.
    let cp = FakeCp::start("200 OK", r#"{"subdomains":[{"label":"rosson","tier":"pro","connected":true}]}"#);
    let _cp = EnvVar::set("K2_CONNECT_BASE", &cp.base);
    let r = proxy(d.port, OWNER_TOKEN, r#"{"method":"GET","endpoint":"/subdomains"}"#);
    assert_eq!(r.status, 200, "{}", r.body);
    let v = json_of(&r.body);
    assert_eq!(v["status"], json!(200), "{v}");
    assert_eq!(
        v["body"],
        json!(r#"{"subdomains":[{"label":"rosson","tier":"pro","connected":true}]}"#),
        "{v}"
    );
    let reqs = cp.requests();
    assert_eq!(reqs.len(), 1, "{reqs:?}");
    assert!(reqs[0].starts_with("GET /subdomains "), "{}", reqs[0]);
    assert!(
        reqs[0].to_ascii_lowercase().contains(&format!("authorization: bearer {VAULT_TOKEN}").to_ascii_lowercase()),
        "the control plane must see the vault token:\n{}",
        reqs[0]
    );
    drop(_cp);

    // POST /subdomains from an Admin login, body as a JSON object.
    let admin = login_as(d.port, "cp_admin", "admin");
    let cp = FakeCp::start("201 Created", r#"{"host":"staging.rosson.k2.dev","status":"active"}"#);
    let _cp = EnvVar::set("K2_CONNECT_BASE", &cp.base);
    let r = proxy(
        d.port,
        &admin,
        r#"{"method":"POST","endpoint":"/subdomains","body":{"label":"staging","target":"localhost:3000"}}"#,
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(json_of(&r.body)["status"], json!(201));
    let reqs = cp.requests();
    assert_eq!(reqs.len(), 1, "{reqs:?}");
    assert!(reqs[0].starts_with("POST /subdomains "), "{}", reqs[0]);
    assert!(reqs[0].contains(r#"{"label":"staging","target":"localhost:3000"}"#), "{}", reqs[0]);
    assert!(reqs[0].to_ascii_lowercase().contains("content-type: application/json"), "{}", reqs[0]);
    drop(_cp);

    // A control-plane error passes through; PUT with a string body.
    let cp = FakeCp::start("403 Forbidden", r#"{"error":"pro_required","detail":"staging"}"#);
    let _cp = EnvVar::set("K2_CONNECT_BASE", &cp.base);
    let r = proxy(
        d.port,
        OWNER_TOKEN,
        r#"{"method":"PUT","endpoint":"/subdomains/staging","body":"{\"target\":\"localhost:3001\"}"}"#,
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let v = json_of(&r.body);
    assert_eq!(v["status"], json!(403), "{v}");
    assert_eq!(v["body"], json!(r#"{"error":"pro_required","detail":"staging"}"#), "{v}");
    let reqs = cp.requests();
    assert!(reqs[0].starts_with("PUT /subdomains/staging "), "{}", reqs[0]);
    assert!(reqs[0].ends_with(r#"{"target":"localhost:3001"}"#), "{}", reqs[0]);
    drop(_cp);

    assert_token_nowhere_under(home.path());
}

// ── (3)(4) refusals send nothing ──────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_callers_and_endpoints_send_nothing() {
    let (_g, home) = setup();
    let _ag = EnvVar::remove("K2_AIRGAP");
    seed_vaulted_tunnel();
    let d = futures_block(test_harness::start(OWNER_TOKEN));
    let cp = FakeCp::start("200 OK", "{}");
    let _cp = EnvVar::set("K2_CONNECT_BASE", &cp.base);
    let ok_body = r#"{"method":"GET","endpoint":"/subdomains"}"#;

    // GET on the POST route.
    let r = http(d.port, "GET", &format!("/cli/connect/proxy?token={OWNER_TOKEN}"), None);
    assert_eq!(r.status, 405, "{}", r.body);

    // Off the allowlist.
    for body in [
        r#"{"method":"GET","endpoint":"/api/dns/ptr"}"#,
        r#"{"method":"GET","endpoint":"//evil.example/subdomains"}"#,
        r#"{"method":"PUT","endpoint":"/subdomains/a%2Fb","body":{}}"#,
        r#"{"method":"PATCH","endpoint":"/subdomains"}"#,
    ] {
        let r = proxy(d.port, OWNER_TOKEN, body);
        assert_eq!(r.status, 400, "{body}: {}", r.body);
        assert_eq!(json_of(&r.body)["code"], json!("not_allowed"), "{body}: {}", r.body);
    }
    let r = proxy(d.port, OWNER_TOKEN, "not json");
    assert_eq!(r.status, 400, "{}", r.body);

    // An agent passport whose workspace isn't registered here may not write.
    let passport = passport_for(&uuid::Uuid::new_v4().to_string());
    let r = proxy(
        d.port,
        &passport,
        r#"{"method":"POST","endpoint":"/subdomains","body":{"label":"x1","target":"localhost:1"}}"#,
    );
    assert_eq!(r.status, 403, "unregistered passport: {}", r.body);
    // A made-up passport-shaped token is just a bad credential.
    let r = proxy(d.port, "00000000-0000-0000-0000-000000000000.notasecret", ok_body);
    assert_eq!(r.status, 403, "fake passport: {}", r.body);

    // An app pass, a Member login, no token.
    let r = proxy(d.port, "k2skn_notarealpass0000", ok_body);
    assert_eq!(r.status, 403, "app pass: {}", r.body);
    let member = login_as(d.port, "cp_member", "member");
    let r = proxy(d.port, &member, ok_body);
    assert_eq!(r.status, 403, "member: {}", r.body);
    let r = http(d.port, "POST", "/cli/connect/proxy", Some(ok_body));
    assert_eq!(r.status, 403, "no token: {}", r.body);

    assert!(cp.requests().is_empty(), "nothing may reach the control plane: {:?}", cp.requests());
    assert_token_nowhere_under(home.path());
}

// ── (5) unreadable vault / no token / air-gap ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreadable_vault_no_token_and_air_gap_are_soft_errors() {
    let (_g, home) = setup();
    let _ag = EnvVar::remove("K2_AIRGAP");
    seed_vaulted_tunnel();
    let d = futures_block(test_harness::start(OWNER_TOKEN));
    let cp = FakeCp::start("200 OK", "{}");
    let _cp = EnvVar::set("K2_CONNECT_BASE", &cp.base);
    let body = r#"{"method":"GET","endpoint":"/subdomains"}"#;

    VAULT_FAIL_READS.store(true, Ordering::SeqCst);
    let r = proxy(d.port, OWNER_TOKEN, body);
    VAULT_FAIL_READS.store(false, Ordering::SeqCst);
    assert_eq!(r.status, 503, "{}", r.body);
    let v = json_of(&r.body);
    assert_eq!(v["code"], json!("token_unreadable"), "{v}");
    assert!(v["error"].as_str().expect("error").contains("nothing was sent"), "{v}");
    assert_eq!(
        VAULT_TOKEN_SLOT.lock().expect("vault").as_deref(),
        Some(VAULT_TOKEN),
        "an unreadable vault must keep the token"
    );

    {
        let _airgap = EnvVar::set("K2_AIRGAP", "1");
        let r = proxy(d.port, OWNER_TOKEN, body);
        assert_eq!(r.status, 403, "{}", r.body);
        assert_eq!(json_of(&r.body)["code"], json!("airgap"), "{}", r.body);
    }

    *VAULT_TOKEN_SLOT.lock().expect("vault") = None;
    let r = proxy(d.port, OWNER_TOKEN, body);
    assert_eq!(r.status, 400, "{}", r.body);
    assert_eq!(json_of(&r.body)["code"], json!("no_token"), "{}", r.body);

    assert!(cp.requests().is_empty(), "nothing may reach the control plane: {:?}", cp.requests());
    assert_token_nowhere_under(home.path());
}

// ── agents publish their own nested labels, end to end ────────────────

fn passport_for(workspace_uuid: &str) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: workspace_uuid.to_string(), agent_address: "agent".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

/// A registered workspace (projects row + its directory). Returns (id, path).
fn register_workspace(home: &std::path::Path, name: &str) -> (String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    let path = home.join("ws").join(name);
    std::fs::create_dir_all(path.join(".k2")).expect("workspace dir");
    let path = path.to_string_lossy().into_owned();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
        rusqlite::params![id, name, path],
    )
    .expect("insert project");
    (id, path)
}

fn owner_of(label: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::SubdomainWorkspace::owner_of(&conn, label).expect("owner_of")
}

/// The control-plane writes (POST/PUT/DELETE) the fake saw — the daemon's
/// own map refreshes (GET) are filtered out.
fn writes(cp: &FakeCp) -> Vec<String> {
    cp.requests().into_iter().filter(|r| !r.starts_with("GET ")).collect()
}

/// Run the REAL `cli/k2` as a workspace agent in its K2 terminal: only
/// its passport (K2_HOOK_TOKEN + a K2_HOOK_SOCK marker), the daemon port,
/// and a HOME with no owner token. Nothing else from this process's env.
fn agent_cli(port: u16, home: &std::path::Path, ws: &str, passport: &str, args: &[&str]) -> (i32, String) {
    let cli = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
    let out = std::process::Command::new("bash")
        .arg(&cli)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").expect("PATH"))
        .env("HOME", home)
        .env("TMPDIR", std::env::temp_dir())
        .env("K2_PORT", port.to_string())
        .env("K2_HOOK_TOKEN", passport)
        .env("K2_HOOK_SOCK", home.join("no-cell.sock"))
        .env("K2_PROJECT_PATH", ws)
        .current_dir(ws)
        .output()
        .expect("run cli/k2");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code().expect("exit code"), text)
}

fn smart_cp() -> FakeCp {
    FakeCp::by_method(|m| match m {
        "GET" => (
            "200 OK",
            // `web` is there so `k2 publish run web` sees its nested label
            // land in the daemon's map after the create.
            r#"{"subdomains":[{"label":"rosson","host":"rosson.k2.dev","tier":"pro","primary":true,"connected":true},{"label":"web","host":"web.rosson.k2.dev","target":"127.0.0.1:1","tier":"pro"}]}"#,
        ),
        "POST" => (
            "201 Created",
            r#"{"label":"staging","host":"staging.rosson.k2.dev","target":"localhost:3999","status":"active","effective":"live"}"#,
        ),
        "PUT" => (
            "200 OK",
            r#"{"label":"staging","host":"staging.rosson.k2.dev","target":"localhost:4000","status":"active","effective":"live"}"#,
        ),
        _ => ("200 OK", r#"{"removed":"staging"}"#),
    })
}

/// Desktop, token only in the vault, tunnel.json's empty: an agent
/// creates → retargets → deletes its own nested label and reads the plan,
/// all with the real `cli/k2` and its passport. No human, no Settings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_publishes_end_to_end_with_the_real_cli() {
    let (_g, home) = setup();
    let _ag = EnvVar::remove("K2_AIRGAP");
    seed_vaulted_tunnel();
    let d = futures_block(test_harness::start(OWNER_TOKEN));
    let cp = smart_cp();
    let _cp = EnvVar::set("K2_CONNECT_BASE", &cp.base);
    let (ws_id, ws_path) = register_workspace(home.path(), "docs");
    let passport = passport_for(&ws_id);
    let bearer = format!("authorization: bearer {VAULT_TOKEN}").to_ascii_lowercase();

    let (rc, out) = agent_cli(
        d.port,
        home.path(),
        &ws_path,
        &passport,
        &["publish", "subdomain", "create", "staging", "--target", "localhost:3999"],
    );
    assert_eq!(rc, 0, "create: {out}");
    assert!(out.contains("Created staging.rosson.k2.dev"), "create: {out}");
    let w = writes(&cp);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].starts_with("POST /subdomains "), "{}", w[0]);
    assert!(w[0].to_ascii_lowercase().contains(&bearer), "vault token on the wire:\n{}", w[0]);
    assert!(w[0].ends_with(r#"{"label":"staging","target":"localhost:3999"}"#), "{}", w[0]);
    assert_eq!(owner_of("staging").as_deref(), Some(ws_id.as_str()), "create claims the label");

    let (rc, out) = agent_cli(
        d.port,
        home.path(),
        &ws_path,
        &passport,
        &["publish", "subdomain", "target", "staging", "localhost:4000"],
    );
    assert_eq!(rc, 0, "target: {out}");
    assert!(out.contains("Repointed staging.rosson.k2.dev -> localhost:4000"), "target: {out}");
    let w = writes(&cp);
    assert_eq!(w.len(), 2, "{w:?}");
    assert!(w[1].starts_with("PUT /subdomains/staging "), "{}", w[1]);
    assert!(w[1].ends_with(r#"{"target":"localhost:4000"}"#), "{}", w[1]);

    let (rc, out) = agent_cli(d.port, home.path(), &ws_path, &passport, &["publish", "status"]);
    assert_eq!(rc, 0, "status: {out}");
    assert!(out.contains("Connected: rosson.k2.dev") && out.contains("Pro"), "status: {out}");
    let (rc, out) = agent_cli(d.port, home.path(), &ws_path, &passport, &["publish", "subdomain", "list"]);
    assert_eq!(rc, 0, "list: {out}");
    assert!(out.contains("rosson.k2.dev"), "list: {out}");

    let (rc, out) = agent_cli(
        d.port,
        home.path(),
        &ws_path,
        &passport,
        &["publish", "subdomain", "delete", "staging"],
    );
    assert_eq!(rc, 0, "delete: {out}");
    assert!(out.contains("Removed staging"), "delete: {out}");
    let w = writes(&cp);
    assert_eq!(w.len(), 3, "{w:?}");
    assert!(w[2].starts_with("DELETE /subdomains/staging "), "{}", w[2]);
    assert_eq!(owner_of("staging"), None, "delete drops the claim");

    // `k2 publish run` (process + nested URL) and `k2 publish rm`, same
    // agent, same vault-only token.
    let port = {
        let l = TcpListener::bind(("127.0.0.1", 0)).expect("free port");
        l.local_addr().expect("addr").port()
    };
    let cmd = format!("exec python3 -m http.server {port} --bind 127.0.0.1");
    let port_s = port.to_string();
    let (rc, out) = agent_cli(
        d.port,
        home.path(),
        &ws_path,
        &passport,
        &["publish", "run", "web", "--cmd", &cmd, "--port", &port_s],
    );
    assert_eq!(rc, 0, "publish run: {out}");
    let w = writes(&cp);
    assert_eq!(w.len(), 4, "{w:?}");
    assert!(w[3].starts_with("POST /subdomains "), "{}", w[3]);
    assert!(w[3].contains(r#""label":"web""#), "{}", w[3]);
    assert_eq!(owner_of("web").as_deref(), Some(ws_id.as_str()), "run claims the label");
    let (rc, out) = agent_cli(d.port, home.path(), &ws_path, &passport, &["publish", "rm", "web"]);
    assert_eq!(rc, 0, "publish rm: {out}");
    let w = writes(&cp);
    assert!(
        w.iter().any(|r| r.starts_with("DELETE /subdomains/web ")),
        "publish rm drops the hostname: {w:?}"
    );

    for r in cp.requests() {
        assert!(r.to_ascii_lowercase().contains(&bearer), "every call carries the vault token:\n{r}");
    }
    assert!(!out.contains(VAULT_TOKEN));
    assert_token_nowhere_under(home.path());
}

/// What an agent may NOT do through the proxy, and the write limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_limits_own_labels_no_primary_no_reserved_rate_limited() {
    let (_g, home) = setup();
    let _ag = EnvVar::remove("K2_AIRGAP");
    seed_vaulted_tunnel();
    let d = futures_block(test_harness::start(OWNER_TOKEN));
    let cp = smart_cp();
    let _cp = EnvVar::set("K2_CONNECT_BASE", &cp.base);
    let (mine, _) = register_workspace(home.path(), "mine");
    let (theirs, _) = register_workspace(home.path(), "theirs");
    let agent = passport_for(&mine);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::SubdomainWorkspace::self_stamp(&conn, "their-app", &theirs, Some("localhost:5000"))
            .expect("seed the other workspace's label");
    }

    let refused = |body: &str, status: u16, code: &str| {
        let r = proxy(d.port, &agent, body);
        assert_eq!(r.status, status, "{body}: {}", r.body);
        assert_eq!(json_of(&r.body)["code"], json!(code), "{body}: {}", r.body);
    };
    // Another workspace's label: no retarget, no delete, no create over it.
    refused(r#"{"method":"DELETE","endpoint":"/subdomains/their-app"}"#, 409, "label_owned_by_other_workspace");
    refused(
        r#"{"method":"PUT","endpoint":"/subdomains/their-app","body":{"target":"localhost:1"}}"#,
        409,
        "label_owned_by_other_workspace",
    );
    refused(
        r#"{"method":"POST","endpoint":"/subdomains","body":{"label":"their-app","target":"localhost:1"}}"#,
        409,
        "label_owned_by_other_workspace",
    );
    // A label nobody on this server claimed: not this agent's to change.
    refused(r#"{"method":"DELETE","endpoint":"/subdomains/stray"}"#, 403, "label_not_claimed");
    // The primary subdomain.
    refused(
        r#"{"method":"POST","endpoint":"/subdomains","body":{"label":"rosson","target":"localhost:1"}}"#,
        403,
        "primary_label",
    );
    refused(r#"{"method":"DELETE","endpoint":"/subdomains/rosson"}"#, 403, "primary_label");
    // Reserved labels.
    for l in ["skin", "meet", "chat", "mail"] {
        refused(
            &format!(r#"{{"method":"POST","endpoint":"/subdomains","body":{{"label":"{l}","target":"localhost:1"}}}}"#),
            400,
            "reserved_label",
        );
    }
    // Off the allowlist.
    refused(r#"{"method":"GET","endpoint":"/api/account"}"#, 400, "not_allowed");
    refused(r#"{"method":"POST","endpoint":"/api/dns/ptr"}"#, 400, "not_allowed");
    assert!(cp.requests().is_empty(), "no refused call reached the control plane: {:?}", cp.requests());
    assert_eq!(owner_of("their-app").as_deref(), Some(theirs.as_str()), "untouched");

    // Rate limit: creates + deletes per workspace per hour.
    let limit = k2_daemon::connect_proxy::AGENT_WRITES_PER_HOUR;
    for i in 0..limit {
        let r = proxy(
            d.port,
            &agent,
            &format!(r#"{{"method":"POST","endpoint":"/subdomains","body":{{"label":"rl-{i}","target":"localhost:3000"}}}}"#),
        );
        assert_eq!(r.status, 200, "create #{i}: {}", r.body);
        assert_eq!(owner_of(&format!("rl-{i}")).as_deref(), Some(mine.as_str()));
    }
    refused(r#"{"method":"DELETE","endpoint":"/subdomains/rl-0"}"#, 429, "rate_limited");
    assert_eq!(writes(&cp).len(), limit, "the over-limit call sent nothing");
    // Another workspace's agent has its own budget.
    let other = passport_for(&theirs);
    let r = proxy(d.port, &other, r#"{"method":"DELETE","endpoint":"/subdomains/their-app"}"#);
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(owner_of("their-app"), None);
    // The owner keeps the whole allowlist (no agent rules, no limit).
    let r = proxy(d.port, OWNER_TOKEN, r#"{"method":"DELETE","endpoint":"/subdomains/rl-0"}"#);
    assert_eq!(r.status, 200, "{}", r.body);

    assert_token_nowhere_under(home.path());
}
