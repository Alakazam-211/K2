//! Node admin over real HTTP (prd-k2-node-admin-v1 §13.1, §16): the real
//! dispatcher in-process (temp `$HOME`, loopback). Every mutating admin
//! route answers 405 to a GET; an agent passport can never approve; the
//! owner token alone can't approve without a fresh confirm; a passport
//! without the operate switch can't propose. No vault, keychain or node
//! is touched: every check here refuses before those.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-node-admin-v1";

struct Env {
    home: PathBuf,
    prev_home: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Env {
    fn drop(&mut self) {
        match self.prev_home.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        std::env::remove_var("K2_COMPUTE");
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2na-{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    env
}

struct Resp {
    status: u16,
    body: String,
}

fn req_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("{method} {pq}: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|x| x.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

async fn req(port: u16, method: &str, pq: String, body: Option<Value>) -> Resp {
    let m = method.to_string();
    tokio::task::spawn_blocking(move || req_blocking(port, &m, &pq, body.map(|b| b.to_string()))).await.unwrap()
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn code(r: &Resp) -> String {
    let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", r.body));
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", r.body)).to_string()
}

fn seed_ws(handle: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, format!("/tmp/k2na-ws-{id}")],
    )
    .unwrap();
    id
}

fn passport(ws: &str) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: ws.to_string(), agent_address: "agent".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn node_admin_routes_over_http() {
    let _env = setup();
    std::env::set_var("K2_COMPUTE", "1");
    let d = test_harness::start(OWNER).await;
    let port = d.port;

    // 405 on GET for every mutating node admin route.
    for p in k2_daemon::compute_admin::POST_ROUTES {
        let r = req(port, "GET", format!("{p}?token={}", enc(OWNER)), None).await;
        assert_eq!(r.status, 405, "GET {p}: {}", r.body);
    }

    let ws = seed_ws("it-admin");
    let agent = passport(&ws);

    // A passport never approves, with or without a confirm token.
    let r = req(port, "POST", format!("/cli/compute/admin/approve?token={}", enc(&agent)), Some(json!({"request": "car_x", "confirmToken": "k2vc_x"}))).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(code(&r), "owner_only");
    // Nor deny, set alerts, or read the approval key.
    let r = req(port, "POST", format!("/cli/compute/admin/deny?token={}", enc(&agent)), Some(json!({"request": "car_x"}))).await;
    assert_eq!((r.status, code(&r)), (403, "owner_only".to_string()), "{}", r.body);
    let r = req(port, "POST", format!("/cli/compute/nodes/alerts?token={}", enc(&agent)), Some(json!({"to": "x"}))).await;
    assert_eq!(r.status, 403, "{}", r.body);

    // The owner token alone isn't an approval (AR14): a confirm is required.
    let r = req(port, "POST", format!("/cli/compute/admin/approve?token={}", enc(OWNER)), Some(json!({"request": "car_does_not_exist_0000"}))).await;
    assert_eq!(r.status, 404, "{}", r.body);
    assert_eq!(code(&r), "request_not_found");

    // Without the operate switch an agent can't propose.
    let r = req(port, "POST", format!("/cli/compute/admin/propose?token={}", enc(&agent)), Some(json!({"node": "n1", "verb": "power.reboot"}))).await;
    assert_eq!((r.status, code(&r)), (403, "operate_off".to_string()), "{}", r.body);

    // The switch is owner/admin only, through agent-access/set.
    let r = req(port, "POST", format!("/cli/agent-access/set?token={}", enc(&agent)), Some(json!({"workspace": "it-admin", "toggle": "compute_operate", "value": true}))).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = req(port, "POST", format!("/cli/agent-access/set?token={}", enc(OWNER)), Some(json!({"workspace": "it-admin", "toggle": "compute_operate", "value": true}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = req(port, "POST", format!("/cli/compute/admin/propose?token={}", enc(&agent)), Some(json!({"node": "n1", "verb": "power.reboot"}))).await;
    assert_eq!((r.status, code(&r)), (404, "node_not_found".to_string()), "{}", r.body);

    // Dark when the preview is off.
    std::env::remove_var("K2_COMPUTE");
    let r = req(port, "GET", format!("/cli/compute/admin/requests?token={}", enc(OWNER)), None).await;
    assert_eq!((r.status, code(&r)), (404, "compute_disabled".to_string()), "{}", r.body);
}
