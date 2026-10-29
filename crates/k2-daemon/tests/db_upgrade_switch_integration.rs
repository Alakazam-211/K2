//! "Allow this agent to manage Postgres versions" (`db_upgrade`, migration
//! 0201; prd-pg17-and-db-upgrade-v1 §12) over real HTTP: the real
//! dispatcher in-process (temp `$HOME`, loopback listener).
//!
//! - The switch starts off and shows on the workspace JSON.
//! - An agent passport never sets it: not on, not off, not its own, not
//!   another workspace's.
//! - An Admin login can't raise it (`owner_only`), but may lower it.
//! - A GET on the set route is 405 (POST only).
//! - The owner raises it; the change writes a `db-upgrade-toggle` Access
//!   Audit line and leaves the agent an inbox note.
//!
//! No vault, keychain, Postgres or `~/.k2` is touched (temp `$HOME`).

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
const OWNER: &str = "owner-token-db-upgrade-switch-v1";

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
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2pg-{:x}", nanos % 0xffff_ffff));
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

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

async fn post(port: u16, path: &str, token: &str, body: Value) -> Resp {
    let pq = format!("{path}?token={}", enc(token));
    tokio::task::spawn_blocking(move || req_blocking(port, "POST", &pq, Some(body.to_string()))).await.unwrap()
}

async fn get(port: u16, path: &str, token: &str, query: &str) -> Resp {
    let pq = format!("{path}?token={}&{query}", enc(token));
    tokio::task::spawn_blocking(move || req_blocking(port, "GET", &pq, None)).await.unwrap()
}

fn j(r: &Resp) -> Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", r.body))
}

fn code(r: &Resp) -> String {
    j(r)["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {}", r.body)).to_string()
}

struct Ws {
    id: String,
    path: PathBuf,
}

fn seed_ws(env: &Env, handle: &str) -> Ws {
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, path.to_string_lossy()])
        .unwrap();
    Ws { id, path }
}

fn passport(ws: &Ws) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: ws.id.clone(), agent_address: "agent".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

fn on(ws: &Ws) -> bool {
    k2_core::workspace::settings::agents_can_upgrade_postgres(&ws.id)
}

fn notes(ws: &Ws) -> Vec<String> {
    let dir = k2_core::inbox::inbox_root(&ws.path);
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("md"))
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect()
}

fn toggle_audit() -> Vec<(String, String)> {
    k2_core::auth_audit::tail(500)
        .expect("read the access audit")
        .into_iter()
        .filter(|v| v["event"] == "db-upgrade-toggle")
        .map(|v| (v["user"].as_str().unwrap().to_string(), v["outcome"].as_str().unwrap().to_string()))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn db_upgrade_switch_floors_end_to_end() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    let ops = seed_ws(&env, "pg-ops");
    let web = seed_ws(&env, "pg-web");
    let agent = passport(&ops);
    let set = |ws: &Ws, value: bool| json!({"workspace": ws.id, "toggle": "db_upgrade", "value": value});

    assert!(!on(&ops), "0201 default is OFF");

    // An agent never sets it: its own or another's, on or off.
    for (ws, value) in [(&ops, true), (&web, true), (&ops, false)] {
        let r = post(port, "/cli/agent-access/set", &agent, set(ws, value)).await;
        assert_eq!(r.status, 403, "agent value={value}: {}", r.body);
    }
    assert!(!on(&ops) && !on(&web), "refused flips change nothing");

    // POST only.
    let r = get(port, "/cli/agent-access/set", OWNER, &format!("workspace={}&toggle=db_upgrade&value=true", ops.id)).await;
    assert_eq!(r.status, 405, "{}", r.body);
    assert!(!on(&ops));

    // An Admin login can't raise it.
    k2_core::connect_users::add_user("pg-admin", "correct-horse-battery-staple-1").expect("add admin");
    k2_core::connect_users::set_role("pg-admin", k2_core::connect_users::Role::Admin).expect("admin role");
    let admin = k2_core::connect_users::create_session("pg-admin");
    let r = post(port, "/cli/agent-access/set", &admin, set(&ops, true)).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(code(&r), "owner_only");
    assert!(!on(&ops));
    assert!(toggle_audit().is_empty(), "no toggle line for a refusal");
    assert!(notes(&ops).is_empty());

    // The owner raises it: audit line + the agent's note.
    let r = post(port, "/cli/agent-access/set", OWNER, set(&ops, true)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["agentsCanUpgradePostgres"], true, "{}", r.body);
    assert!(on(&ops));
    assert!(!on(&web), "only the named workspace");
    assert_eq!(toggle_audit(), vec![("owner".to_string(), "ok db_upgrade=on workspace=pg-ops".to_string())]);
    let n = notes(&ops);
    assert_eq!(n.len(), 1, "{n:#?}");
    assert!(n[0].contains("Allow this agent to manage Postgres versions") && n[0].contains("is on"), "{n:#?}");

    // An agent can't lower it either, but an Admin can.
    let r = post(port, "/cli/agent-access/set", &agent, set(&ops, false)).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert!(on(&ops));
    let r = post(port, "/cli/agent-access/set", &admin, set(&ops, false)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(j(&r)["agentsCanUpgradePostgres"], false, "{}", r.body);
    assert!(!on(&ops), "an Admin lowers");
    let lines = toggle_audit();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[1].1, "ok db_upgrade=off workspace=pg-ops");
    assert_eq!(notes(&ops).len(), 2);
}
