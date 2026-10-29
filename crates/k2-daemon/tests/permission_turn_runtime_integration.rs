//! The permission outcome turn wakes a dormant agent without a panic
//! (0.46.3 r9 smoke, job 961206a2 on k2n6-hil):
//!
//! ```text
//! thread 'permission-turn' panicked at crates/k2-daemon/src/cell_server.rs:244:34:
//! there is no reactor running, must be called from the context of a Tokio 1.x runtime
//! ```
//!
//! `permission::inject::deliver` (the real one: the lib's unit tests swap
//! it for a sink) ran `deliver_live_with_via` on a bare std thread. Waking
//! a dormant agent spawns its session, and `cell_server::serve_cell`
//! adopts the cell socket with `tokio::net::UnixListener::from_std`, which
//! needs a Tokio runtime. The thread died and the turn was lost.
//!
//! Here the real daemon in process, an `exec tee` agent shim that logs
//! what reaches it, and the switch-off close (`permissionCards` on, then
//! off with a request open): its `[permission:<id>] closed …` turn must
//! wake the dormant agent and reach it, and no thread may panic.
//!
//! Its own binary: it flips the process-wide switch and installs a panic
//! hook.
// The suite runs over the per-cell Unix socket (cell_server::serve_cell); on
// Windows cells reach the same gate over the TCP arm (cell_uds is a stub there).
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use k2_core::db::schema::WorkspaceSession;
use k2_daemon::test_harness;
use serde_json::json;

const OWNER: &str = "owner-token-permission-turn";

/// One test at a time (each swaps `$HOME` and the runtime handle).
static SERIAL: Mutex<()> = Mutex::new(());

/// Every panic in this process: `thread name: message`.
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record_panics() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let name = std::thread::current().name().unwrap_or("<unnamed>").to_string();
            PANICS.lock().unwrap_or_else(|e| e.into_inner()).push(format!("{name}: {info}"));
            prev(info);
        }));
    });
}

fn panics() -> Vec<String> {
    PANICS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

struct Resp {
    status: u16,
    body: String,
}

fn tcp_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
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
    let line = text.lines().next().unwrap_or_else(|| panic!("no status line: {text:?}"));
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("bad status: {line}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

async fn post(port: u16, path: &str, body: &str) -> Resp {
    let pq = format!("{path}?token={OWNER}");
    let body = body.to_string();
    tokio::task::spawn_blocking(move || tcp_blocking(port, "POST", &pq, Some(body))).await.unwrap()
}

struct Home {
    home: PathBuf,
    prev: Option<std::ffi::OsString>,
}
impl Drop for Home {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_outcome_turn_wakes_a_dormant_agent_without_a_panic() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    record_panics();
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = k2_core::test_env::short_tmp(format!("k2ptr{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = dunce::canonicalize(&home).unwrap();
    let _home = Home { prev: std::env::var_os("HOME"), home: home.clone() };
    std::env::set_var("HOME", &home);

    // The agent CLI logs what reaches it: its arguments (a fresh wake hands
    // the turn over as the launch prompt) and every line typed into it (a
    // wake of a live session injects). It turns bracketed paste on first,
    // the "ready" sign a wake waits for.
    let heard = home.join("agent-heard.log");
    let _shim = k2_core::test_env::AgentShim::with_body(&format!(
        "printf '%s\\n' \"$@\" >> '{log}'\nprintf '\\033[?2004h'\nexec tee -a '{log}'",
        log = heard.display()
    ));

    let d = test_harness::start(OWNER).await;
    let port = d.port;

    // A workspace with its pinned Chat and no live session (dormant).
    let handle = "ptr-billing";
    let ws_path = home.join("ws-ptr-billing");
    std::fs::create_dir_all(&ws_path).unwrap();
    let ws_id = uuid::Uuid::new_v4().to_string();
    let conv = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            rusqlite::params![ws_id, handle, ws_path.to_string_lossy()],
        )
        .unwrap();
        WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws_id, None, Some(&conv), "claude", "system", "running").unwrap();
    }
    assert!(k2_daemon::v2_session_map::snapshot().is_empty(), "the agent must start dormant");

    let r = post(port, "/cli/settings/update", r#"{"permissionCards":true}"#).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(k2_core::permission::gate::server_on());

    // An open request with its card, seeded into the store.
    let request_id = format!("pr_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let now = k2_core::overlay::now_secs();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let (item, _) = k2_core::overlay::post_choice_with_action(
            &conn,
            &conv,
            &ws_id,
            handle,
            handle,
            "ptr-billing asks: Let ptr-billing use compute nodes (Allow compute nodes).",
            vec!["Allow".into(), "Deny".into()],
            false,
            Some(now + 3600),
            Some(k2_core::overlay::CardAction {
                id: "permission".into(),
                params: json!({ "request": request_id }),
                dedupe: "seed".into(),
                result: None,
                view: None,
            }),
        )
        .expect("post the card");
        conn.execute(
            "INSERT INTO permission_requests (id, workspace_id, session_id, conversation_id, addr, card_id, action, params_json, \
             params_hash, text_hash, why, state, floor, created_at, expires_at) \
             VALUES (?1, ?2, 's', ?3, ?4, ?5, 'seed.switch', '{}', 'h', 't', 'Run the suite', 'pending', 'admin', ?6, ?7)",
            rusqlite::params![request_id, ws_id, conv, handle, item.id, now, now + 3600],
        )
        .expect("seed the request");
    }

    // The switch-off close: one `closed` turn, which wakes the agent.
    let r = post(port, "/cli/settings/update", r#"{"permissionCards":false}"#).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let state: String = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row("SELECT state FROM permission_requests WHERE id = ?1", [&request_id], |r| r.get(0)).unwrap()
    };
    assert_eq!(state, "closed", "the switch-off close must close the open request");

    let want = format!("[permission:{request_id}] closed: permission cards were turned off on this server.");
    let deadline = Instant::now() + Duration::from_secs(60);
    let got = loop {
        let p = panics();
        assert!(p.is_empty(), "a thread panicked delivering the outcome turn: {p:#?}");
        let text = std::fs::read_to_string(&heard).unwrap_or_default();
        if text.contains(&want) {
            break text;
        }
        assert!(
            Instant::now() < deadline,
            "the outcome turn never reached the woken agent in 60 s; it heard: {text:?}; sessions: {:?}",
            k2_daemon::v2_session_map::list_agents()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(got.matches("[permission:").count(), 1, "exactly one turn: {got:?}");
    assert!(!k2_daemon::v2_session_map::snapshot().is_empty(), "the wake left a live agent session");
    let p = panics();
    assert!(p.is_empty(), "no thread may panic: {p:#?}");

    for (name, s) in k2_daemon::v2_session_map::snapshot() {
        if k2_daemon::v2_session_map::unregister(&name).is_some() {
            s.kill();
        }
    }
    drop(d);
}

/// A thread started off the runtime (the card expiry sweep, a supervisor)
/// still gets the daemon's: the exact call that panicked in `serve_cell`
/// works inside `k2_daemon::rt::spawn`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thread_started_off_the_runtime_still_adopts_a_tokio_socket() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    k2_daemon::rt::install(tokio::runtime::Handle::current());
    let dir = std::env::temp_dir().join(format!("k2-rt-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("s.sock");
    let outer = std::thread::spawn(move || {
        assert!(tokio::runtime::Handle::try_current().is_err(), "the outer thread has no runtime");
        let inner = k2_daemon::rt::spawn("rt-test", move || {
            let l = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
            l.set_nonblocking(true).unwrap();
            let l = tokio::net::UnixListener::from_std(l).expect("adopt in the runtime");
            tokio::spawn(async move { drop(l) });
        })
        .expect("spawn with the installed runtime");
        inner.join().expect("the rt thread must not panic");
    });
    tokio::task::spawn_blocking(move || outer.join()).await.unwrap().expect("no panic");
    let _ = std::fs::remove_dir_all(&dir);
}
