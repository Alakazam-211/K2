//! The compute store's routes over real HTTP (0.46.17): the real
//! dispatcher in-process (temp `$HOME`, loopback). `prune`, `pin`,
//! `unpin` and `retention/set` answer 405 to a GET; an agent passport is
//! refused with owner_only; the owner's dry run deletes nothing and the
//! real prune removes a file no job owns; `retention` reads the rule.

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
const OWNER: &str = "owner-token-compute-retention";

struct Env {
    home: PathBuf,
    prev_home: Option<std::ffi::OsString>,
    _guard: k2_core::test_env::SerialGuard,
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
    let guard = k2_core::test_env::serial(&TEST_LOCK);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2cr-{:x}", nanos % 0xffff_ffff));
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

fn json_of(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", r.body))
}

fn seed_ws(handle: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, format!("/tmp/k2cr-ws-{id}")]).unwrap();
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

/// Set `p`'s mtime `secs` back.
fn age(p: &std::path::Path, secs: u64) {
    let f = std::fs::File::open(p).unwrap_or_else(|e| panic!("open {}: {e}", p.display()));
    f.set_modified(std::time::SystemTime::now() - Duration::from_secs(secs)).expect("mtime");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compute_store_routes_over_http() {
    let _env = setup();
    std::env::set_var("K2_COMPUTE", "1");
    let d = test_harness::start(OWNER).await;
    let port = d.port;

    // 405 on GET for every mutating route.
    for p in k2_daemon::compute_retention::POST_ROUTES {
        let r = req(port, "GET", format!("{p}?token={}", enc(OWNER)), None).await;
        assert_eq!(r.status, 405, "GET {p}: {}", r.body);
    }

    // An agent passport is refused, before anything is read or removed.
    let agent = passport(&seed_ws("it-retention"));
    for p in k2_daemon::compute_retention::ROUTES {
        let method = if k2_daemon::compute_retention::POST_ROUTES.contains(p) { "POST" } else { "GET" };
        let body = (method == "POST").then(|| json!({"job": "abcd1234", "dryRun": true}));
        let r = req(port, method, format!("{p}?token={}", enc(&agent)), body).await;
        assert_eq!(r.status, 403, "{method} {p}: {}", r.body);
        assert_eq!(json_of(&r)["error"]["code"], "owner_only", "{p}");
    }

    // A fetched-files folder whose job this server doesn't know, 2 h old.
    let dir = k2_core::compute::compute_dir().join("artifacts/0123456789abcdef0123456789abcdef-g1");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("f".repeat(64));
    std::fs::write(&file, vec![7u8; 4096]).unwrap();
    age(&file, 7200);
    age(&dir, 7200);

    // The owner's dry run: says it would go, deletes nothing.
    let r = req(port, "POST", format!("/cli/compute/prune?token={}", enc(OWNER)), Some(json!({"dryRun": true}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = json_of(&r);
    assert_eq!((v["dryRun"].as_bool(), v["removed"]["units"].as_u64(), v["removed"]["byReason"]["orphan"].as_u64()), (Some(true), Some(1), Some(1)), "{v}");
    assert!(file.exists(), "a dry run deletes nothing");

    // The real prune removes it and says so.
    let r = req(port, "POST", format!("/cli/compute/prune?token={}", enc(OWNER)), Some(json!({}))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = json_of(&r);
    assert_eq!(v["removed"]["bytes"].as_u64(), Some(4096), "{v}");
    assert!(v["text"][0].as_str().is_some_and(|t| t.starts_with("removed 1 job(s)' files")), "{v}");
    assert!(!dir.exists(), "the folder is gone");

    // A bad duration is a usage error.
    let r = req(port, "POST", format!("/cli/compute/prune?token={}", enc(OWNER)), Some(json!({"olderThan": "soon"}))).await;
    assert_eq!(r.status, 400, "{}", r.body);

    // The rule reads back, and status carries the one-line summary.
    let r = req(port, "GET", format!("/cli/compute/retention?token={}", enc(OWNER)), None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = json_of(&r);
    assert_eq!((v["policy"]["maxAgeSecs"].as_i64(), v["policy"]["capFrom"].as_str()), (Some(7 * 86_400), Some("auto")), "{v}");
    let r = req(port, "GET", format!("/cli/compute/nodes?detail=1&token={}", enc(OWNER)), None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(json_of(&r)["controllerMachine"]["store"]["line"].as_str().is_some_and(|l| l.starts_with("compute store: ")), "{}", r.body);
}
