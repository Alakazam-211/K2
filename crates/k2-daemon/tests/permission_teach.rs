//! Permission cards, lane C (prd-agent-settings-cards-v1 rev 3.1 §3.1, §5.5,
//! §11.4; vs-live C2, C3, C8): refusals teach, but only while BOTH switches
//! are on for the calling agent.
//!
//! Against a real in-process daemon, an agent passport on the TCP port and
//! on its own cell socket:
//! - **server off** (the default), and **server on with the agent's switch
//!   off**: no refusal carries `permission`, the cell's non-agent route is
//!   today's codeless 403, and no `k2-permissions` skill or AGENTS.md
//!   pointer exists (the off-goldens pin the exact bytes; this pins the
//!   second state too);
//! - **both on**: a mapped wall carries `permission.{action, params,
//!   approvers, command}` on TCP and on the cell; a hold names its own
//!   request id; any other passport refusal carries `requestable:false`;
//!   the cell's non-agent route answers the teaching `owner_only` body for
//!   a valid passport only (a bad token keeps today's text); people never
//!   get a `permission` object; the k2-permissions skill and the AGENTS.md
//!   pointer appear, and the k2-cli skill stays byte-identical.
//!
//! No vault, keychain, real `~/.k2` or running daemon is touched.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-permission-teach";

struct Env {
    home: PathBuf,
    prev_home: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Env {
    fn drop(&mut self) {
        k2_core::permission::gate::set_server_switch(false);
        match self.prev_home.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        for k in ["K2_COMPUTE", "K2_APPS"] {
            std::env::remove_var(k);
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = PathBuf::from(format!("/tmp/k2pct{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = std::fs::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    env
}

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.body))
    }
}

fn parse_resp(text: &str) -> Resp {
    let status_line = text.lines().next().unwrap_or_else(|| panic!("no status line: {text:?}"));
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("bad status line: {status_line:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("").to_string();
    Resp { status, body }
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

async fn tcp(port: u16, method: &str, path_q: &str, token: Option<&str>, body: Option<&str>) -> Resp {
    let pq = match token {
        Some(t) => format!("{path_q}{}token={}", if path_q.contains('?') { '&' } else { '?' }, enc(t)),
        None => path_q.to_string(),
    };
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    tokio::task::spawn_blocking(move || {
        let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match s.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
                Err(e) => panic!("{e}"),
            }
        }
        parse_resp(&String::from_utf8_lossy(&out))
    })
    .await
    .unwrap()
}

async fn cell(sock: &Path, method: &str, path_q: &str, bearer: &str, body: Option<&str>) -> Resp {
    let raw = match body {
        Some(b) => format!(
            "{method} {path_q} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path_q} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {bearer}\r\n\r\n"),
    };
    let mut stream = UnixStream::connect(sock).await.expect("connect cell socket");
    stream.write_all(raw.as_bytes()).await.expect("write request");
    stream.flush().await.ok();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    parse_resp(&String::from_utf8_lossy(&buf))
}

fn set_agent_switch(ws_id: &str, on: bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let n = conn
        .execute("UPDATE projects SET agents_can_request_permissions = ?1 WHERE id = ?2", params![on as i64, ws_id])
        .expect("set the agent's switch");
    assert_eq!(n, 1, "the workspace row exists");
}

/// The teaching files for one workspace: (k2-cli skill, k2-permissions
/// skill if any, AGENTS.md).
fn teaching(ws_path: &Path) -> (String, Option<String>, String) {
    let ws = ws_path.to_string_lossy().to_string();
    k2_core::workspace::skill_regen::write_workspace_skill_file(&ws);
    let dot = k2_core::workspace_dot_dir(&ws);
    let cli = std::fs::read_to_string(dot.join("skills/k2-cli/SKILL.md")).expect("k2-cli skill");
    let perm = std::fs::read_to_string(dot.join("skills/k2-permissions/SKILL.md")).ok();
    let agents = std::fs::read_to_string(dot.join("AGENTS.md")).expect("AGENTS.md");
    (cli, perm, agents)
}

fn strip_stamp(s: &str) -> String {
    s.lines().map(|l| if l.contains("GENERATED by K2 at") { "<stamp>" } else { l }).collect::<Vec<_>>().join("\n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refusals_teach_only_while_both_switches_are_on() {
    let env = setup();
    std::env::set_var("K2_COMPUTE", "1");
    std::env::set_var("K2_APPS", "1");
    let _shim = k2_core::test_env::AgentShim::install();
    let d = test_harness::start(OWNER).await;
    let port = d.port;

    let path = env.home.join("ws-pct-billing");
    std::fs::create_dir_all(&path).unwrap();
    let ws_id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'pct-billing', ?2, 'pct-billing')",
            params![ws_id, path.to_string_lossy()],
        )
        .unwrap();
    }
    let sid = SessionId::new();
    let passport = k2_daemon::session_token::mint_session_token(
        &sid,
        "pane-pct",
        HookPrincipal { workspace_uuid: ws_id.clone(), agent_address: "pct-billing".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    );
    let listener = k2_daemon::cell_uds::bind_cell_socket(&sid).expect("bind cell socket");
    let sock = k2_daemon::cell_uds::cell_socket_path(&sid);
    k2_daemon::cell_server::serve_cell(sid, listener, None);
    tokio::time::sleep(Duration::from_millis(80)).await;

    let (cli_off, perm_off, agents_off) = teaching(&path);
    assert!(perm_off.is_none(), "no k2-permissions skill while off");
    assert!(!agents_off.contains("Permission cards"), "no AGENTS.md pointer while off");

    // ── server off, then server on with the agent's switch off ──────────
    for (state, server) in [("server off", false), ("server on, agent off", true)] {
        k2_core::permission::gate::set_server_switch(server);
        let t = tcp(port, "GET", "/cli/compute/nodes", Some(&passport), None).await;
        assert_eq!(t.status, 403, "{state}: {}", t.body);
        assert!(t.json().get("permission").is_none(), "{state}: TCP taught: {}", t.body);
        let c = cell(&sock, "GET", "/cli/compute/nodes", &passport, None).await;
        assert!(c.json().get("permission").is_none(), "{state}: cell taught: {}", c.body);
        let g = cell(&sock, "POST", "/cli/compute/admin/approve", &passport, Some(r#"{"request":"nar_1"}"#)).await;
        assert_eq!((g.status, g.body.as_str()), (403, r#"{"error":"Invalid or missing auth token"}"#), "{state}");
        let (cli, perm, agents) = teaching(&path);
        assert_eq!(cli, cli_off, "{state}: the k2-cli skill changed");
        assert!(perm.is_none(), "{state}: a k2-permissions skill exists");
        assert_eq!(strip_stamp(&agents), strip_stamp(&agents_off), "{state}: AGENTS.md changed");
    }

    // ── both on ─────────────────────────────────────────────────────────
    k2_core::permission::gate::set_server_switch(true);
    set_agent_switch(&ws_id, true);

    // A code wall, on both transports.
    for (label, r) in [
        ("tcp", tcp(port, "GET", "/cli/compute/nodes", Some(&passport), None).await),
        ("cell", cell(&sock, "GET", "/cli/compute/nodes", &passport, None).await),
    ] {
        assert_eq!(r.status, 403, "{label}: {}", r.body);
        let v = r.json();
        assert_eq!(v["error"]["code"], "compute_off", "{label}: the refusal itself is unchanged: {v}");
        assert_eq!(
            v["permission"],
            serde_json::json!({
                "action": "compute.use",
                "params": {},
                "approvers": "owner or admin",
                "command": "k2 permission request compute.use --why \"<what you need it for>\"",
            }),
            "{label}: {v}"
        );
    }

    // A value from the request (the connection's target).
    let r = tcp(port, "POST", "/cli/connections?action=add&target=pct-sales", Some(&passport), None).await;
    assert_eq!(r.json()["permission"]["command"], "k2 permission request connections.add to=pct-sales --why \"<what you need it for>\"", "{}", r.body);

    // A hold names its own held request.
    let h = tcp(port, "POST", "/cli/app/build", Some(&passport), Some(r#"{"folder":"pct-app"}"#)).await;
    assert_eq!(h.status, 202, "{}", h.body);
    let hv = h.json();
    let held_id = hv["request"]["id"].as_str().unwrap_or_else(|| panic!("no held id: {hv}")).to_string();
    assert_eq!(hv["permission"]["action"], "apps.approve", "{hv}");
    assert_eq!(hv["permission"]["params"]["request"], held_id.as_str(), "{hv}");
    assert_eq!(hv["permission"]["approvers"], "owner", "{hv}");

    // Any other passport refusal: not requestable.
    let n = tcp(port, "POST", "/cli/compute/nodes/remove", Some(&passport), Some(r#"{"node":"x"}"#)).await;
    assert_eq!(n.status, 403, "{}", n.body);
    assert_eq!(n.json()["permission"]["requestable"], false, "{}", n.body);
    assert!(n.json()["permission"].get("command").is_none(), "{}", n.body);

    // The cell's non-agent route: a valid passport is taught …
    let g = cell(&sock, "POST", "/cli/compute/admin/approve", &passport, Some(r#"{"request":"nar_1"}"#)).await;
    assert_eq!(g.status, 403, "{}", g.body);
    let gv = g.json();
    assert_eq!(gv["error"]["code"], "owner_only", "{gv}");
    assert_eq!(gv["permission"]["action"], "compute.admin-approve", "{gv}");
    assert_eq!(gv["permission"]["params"]["request"], "nar_1", "{gv}");
    // … a bad token keeps today's text.
    let bad = cell(&sock, "POST", "/cli/compute/admin/approve", "not-a-passport.deadbeef", Some("{}")).await;
    assert_eq!((bad.status, bad.body.as_str()), (403, r#"{"error":"Invalid or missing auth token"}"#));

    // People never get a `permission` object; neither does no credential.
    let o = tcp(port, "POST", "/cli/compute/nodes/remove", Some(OWNER), Some("{}")).await;
    assert!(o.json().get("permission").is_none(), "owner taught: {}", o.body);
    let none = tcp(port, "GET", "/cli/compute/nodes", None, None).await;
    assert!(!none.body.contains("\"permission\""), "no credential taught: {}", none.body);

    // Keep-alive: a passport request followed by a people request on the
    // same connection never leaks the passport's context.
    let ka_passport = passport.clone();
    let ka = tokio::task::spawn_blocking(move || {
        let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let first = format!("GET /cli/compute/nodes?token={} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", enc(&ka_passport));
        let second = "GET /cli/compute/nodes HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
        // One request at a time (the daemon doesn't pipeline): read the
        // first answer to its Content-Length, then send the second.
        s.write_all(first.as_bytes()).unwrap();
        let mut out = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = s.read(&mut chunk).expect("read the first answer");
            assert!(n > 0, "the connection closed after the first answer");
            out.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&out).to_string();
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let len: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("Content-Length: "))
                    .and_then(|v| v.trim().parse().ok())
                    .expect("Content-Length");
                if body.len() >= len {
                    break;
                }
            }
        }
        s.write_all(second.as_bytes()).unwrap();
        let _ = s.read_to_end(&mut out);
        String::from_utf8_lossy(&out).to_string()
    })
    .await
    .unwrap();
    let parts: Vec<&str> = ka.split("HTTP/1.1 ").filter(|p| !p.is_empty()).collect();
    assert_eq!(parts.len(), 2, "two answers on one connection: {ka}");
    assert!(parts[0].contains("\"permission\""), "the passport's answer teaches: {}", parts[0]);
    assert!(!parts[1].contains("\"permission\""), "the next request inherited the passport: {}", parts[1]);

    // Teaching files: the skill and the pointer appear; k2-cli is identical.
    let (cli_on, perm_on, agents_on) = teaching(&path);
    assert_eq!(cli_on, cli_off, "the k2-cli skill must be byte-identical in every switch state");
    let perm_on = perm_on.expect("the k2-permissions skill is written while both are on");
    assert!(perm_on.contains("# Permission cards (Beta)"), "{perm_on}");
    assert!(agents_on.contains("## Permission cards (Beta)"), "{agents_on}");
    assert!(agents_on.contains(".k2/skills/k2-permissions/SKILL.md"), "{agents_on}");

    // Turn the agent's switch off: teaching goes, refusals are today's.
    set_agent_switch(&ws_id, false);
    k2_core::workspace::skill_regen::sync_permission_teaching(&path.to_string_lossy());
    let (_, perm, agents) = teaching(&path);
    assert!(perm.is_none(), "the k2-permissions skill must go when the agent's switch is off");
    assert_eq!(strip_stamp(&agents), strip_stamp(&agents_off), "AGENTS.md is today's again");
    let t = tcp(port, "GET", "/cli/compute/nodes", Some(&passport), None).await;
    assert!(t.json().get("permission").is_none(), "{}", t.body);
    drop(env);
}
