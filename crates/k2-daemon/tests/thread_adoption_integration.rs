//! A Codex (or Hermes) Thread survives adoption — headless, real
//! dispatcher (`test_harness`), temp `$HOME`, every harness a shim.
//!
//! Rosson 2026-10-07: "When opening a new chat, if you send a message in
//! the Thread for Codex, the response from Codex doesn't land in the
//! Thread. You have to refresh, and then the first message you sent
//! disappears, but the response the agent sent becomes visible. Claude and
//! Grok don't have this problem."
//!
//! Codex mints its own conversation id, so until K2 finds it the tab's
//! handle (and its Thread) is keyed on the pane id. Adoption rekeyed the
//! handle to the provider id but left the Thread on the pane key: the
//! address then resolved to an empty Thread, the reply landed there, and
//! the open view stayed subscribed to the pane key. Now the Thread moves
//! with the handle (k2-core `overlay::move_conversation`, called from
//! `stamp_session_id`) and live views get a `moved` frame.
//!
//! Two adoption doors, one contract each:
//!   1. A plain Codex tab (the + menu's new chat, `/cli/sessions/v2/spawn`),
//!      adopted by the activity transcript follower.
//!   2. A `k2 sidecar new … codex`, adopted by the sidecar's own watch.
//!
//! For both: the first message (sent before the id was known) and the
//! agent's reply (sent after) are both in `GET /cli/thread?addr=`, under the
//! provider id, in order, once each; a socket on the pane key and one on
//! the provider id each get the `moved` frame.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::Value as J;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

const OWNER: &str = "owner-token-thread-adoption";

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

// ── environment ───────────────────────────────────────────────────────

struct TestEnv {
    home: PathBuf,
    prev_home: Option<std::ffi::OsString>,
    prev_shim: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        for (key, s) in k2_daemon::v2_session_map::snapshot() {
            if s.cwd.as_ref().is_some_and(|p| p.starts_with(&self.home)) {
                k2_daemon::v2_session_map::unregister(&key);
                s.kill();
            }
        }
        match self.prev_home.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        match self.prev_shim.take() {
            Some(p) => std::env::set_var("K2_TEST_AGENT_SHIM_DIR", p),
            None => std::env::remove_var("K2_TEST_AGENT_SHIM_DIR"),
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// Temp HOME (never the real `~/.codex` / `~/.claude`) and a shim for
/// every harness, so no real CLI can start: each one just `cat`s.
fn setup() -> TestEnv {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    // Short: the per-cell socket path must fit SUN_LEN.
    let home = PathBuf::from(format!("/tmp/k2ta-{:x}", nanos % 0xffff_ffff));
    let shim_dir = home.join("shim");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    for name in ["claude", "grok", "codex", "hermes", "cat"] {
        let p = shim_dir.join(name);
        std::fs::write(&p, "#!/bin/sh\nexec /bin/cat\n").expect("shim");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod shim");
    }
    let env = TestEnv {
        prev_home: std::env::var_os("HOME"),
        prev_shim: std::env::var_os("K2_TEST_AGENT_SHIM_DIR"),
        home: home.clone(),
        _guard: guard,
    };
    std::env::set_var("HOME", &home);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
    env
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

// ── HTTP ──────────────────────────────────────────────────────────────

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
    let b = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
        b.len()
    );
    s.write_all(req.as_bytes()).expect("write");
    let mut raw = Vec::new();
    match s.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("read: {e:?}"),
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status: {text:?}"));
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_else(|| panic!("no body: {text:?}"));
    Resp { status, body }
}

fn json(r: &Resp) -> J {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", r.body))
}

fn post(port: u16, route: &str, body: J) -> J {
    let r = http(port, "POST", &format!("{route}?token={OWNER}"), Some(&body.to_string()));
    assert_eq!(r.status, 200, "{route}: {}", r.body);
    json(&r)
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn get_thread(port: u16, addr: &str) -> J {
    let r = http(
        port,
        "GET",
        &format!("/cli/thread?token={OWNER}&addr={}&limit=0", enc(addr)),
        None,
    );
    assert_eq!(r.status, 200, "GET thread {addr}: {}", r.body);
    json(&r)
}

fn bodies(thread: &J) -> Vec<String> {
    thread["items"]
        .as_array()
        .unwrap_or_else(|| panic!("items: {thread}"))
        .iter()
        .map(|i| i["doc"]["body"].as_str().unwrap_or_else(|| panic!("body: {i}")).to_string())
        .collect()
}

// ── workspace / codex helpers ────────────────────────────────────────

struct Workspace {
    id: String,
    path: PathBuf,
    handle: String,
}

fn seed_ws(env: &TestEnv, prefix: &str) -> Workspace {
    let handle = format!("{prefix}{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).expect("workspace dir");
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path.to_string_lossy()],
    )
    .expect("seed project");
    conn.execute(
        "INSERT INTO workspaces (id, project_id, name) VALUES (?1, ?2, 'main')",
        params![uuid::Uuid::new_v4().to_string(), id],
    )
    .expect("seed workspaces row");
    Workspace { id, path, handle }
}

/// What Codex writes once its first turn starts: a rollout whose header
/// names its conversation id and cwd.
fn plant_codex_rollout(env: &TestEnv, ws: &Workspace, id: &str, first_user_text: &str) {
    let now = chrono::Utc::now();
    let dir = env
        .home
        .join(".codex")
        .join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    std::fs::create_dir_all(&dir).expect("codex sessions dir");
    let meta = serde_json::json!({
        "type": "session_meta",
        "payload": { "id": id, "timestamp": now.to_rfc3339(), "cwd": ws.path.to_string_lossy() }
    });
    let user = serde_json::json!({
        "type": "response_item",
        "payload": { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": first_user_text }] }
    });
    std::fs::write(
        dir.join(format!("rollout-{}-{id}.jsonl", now.format("%Y-%m-%dT%H-%M-%S"))),
        format!("{meta}\n{user}\n"),
    )
    .expect("codex rollout");
}

fn tab_session_id(ws: &Workspace, pg: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT session_id FROM workspace_tab_sessions WHERE project_id = ?1 AND pane_group_id = ?2",
        params![ws.id, pg],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

fn wait_adopted(ws: &Workspace, pg: &str, want: &str) {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if tab_session_id(ws, pg).as_deref() == Some(want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "conversation {want} was not adopted for {pg} within 45 s (now {:?})",
            tab_session_id(ws, pg)
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

// ── overlay socket ───────────────────────────────────────────────────

async fn open_ws(port: u16, conv: &str) -> Ws {
    let url = format!(
        "ws://127.0.0.1:{port}/cli/overlay/events?conversation={}&token={OWNER}",
        enc(conv)
    );
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .unwrap_or_else(|e| panic!("overlay WS must open for {conv}: {e}"));
    ws
}

/// Frames until the `moved` one (Thread frames for the posts may come
/// first; strip frames are skipped). Fails loudly after 10 s.
async fn moved_frame(ws: &mut Ws, what: &str) -> J {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let msg = timeout(left, ws.next())
            .await
            .unwrap_or_else(|_| panic!("{what}: no `moved` frame within 10 s"))
            .unwrap_or_else(|| panic!("{what}: overlay WS closed before `moved`"))
            .unwrap_or_else(|e| panic!("{what}: overlay WS error: {e}"));
        match msg {
            Message::Text(t) => {
                let f: J = serde_json::from_str(&t).unwrap_or_else(|e| panic!("frame JSON ({e}): {t}"));
                if f["collection"] == "moved" {
                    return f;
                }
            }
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("{what}: unexpected overlay message {other:?}"),
        }
    }
}

/// The shared contract once the tab's first message is in the pane-keyed
/// Thread and both sockets are open: adopt, follow, reply, read.
#[allow(clippy::too_many_arguments)]
fn adopt_then_reply_and_check(
    env: &TestEnv,
    port: u16,
    ws: &Workspace,
    pg: &str,
    addr: &str,
    codex_id: &str,
    first_user_text: &str,
    on_pane: &mut Ws,
    on_provider: &mut Ws,
) {
    plant_codex_rollout(env, ws, codex_id, first_user_text);
    wait_adopted(ws, pg, codex_id);

    // Live views follow: both keys' subscribers are told where it went.
    for (sock, who) in [(&mut *on_pane, "pane-key socket"), (&mut *on_provider, "provider-id socket")] {
        let f = futures_block(moved_frame(sock, who));
        assert_eq!(f["to"], codex_id, "{who}: {f}");
        assert_eq!(f["id"], codex_id, "{who}: {f}");
        assert_eq!(f["from"], pg, "{who}: {f}");
        assert_eq!(f["seq"], 1, "{who}: one Thread row moved: {f}");
        assert!(f.get("doc").is_none(), "{who}: a move carries no doc: {f}");
    }

    // The agent answers in Thread (`k2 thread <addr> "..."`).
    let reply = post(
        port,
        "/cli/thread/post",
        serde_json::json!({ "addr": addr, "text": "hi from codex", "from": addr }),
    );
    assert_eq!(reply["conversation_id"], codex_id, "the reply lands on the provider id: {reply}");
    assert_eq!(reply["seq"], 2, "after the moved first message: {reply}");

    // A refresh (or any reader) sees both, in order, once each.
    let thread = get_thread(port, addr);
    assert_eq!(thread["conversation_id"], codex_id, "{thread}");
    assert_eq!(bodies(&thread), ["hello codex", "hi from codex"], "{thread}");
    let seqs: Vec<i64> = thread["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| i["seq"].as_i64().expect("seq"))
        .collect();
    assert_eq!(seqs, [1, 2], "{thread}");
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert!(
        k2_core::overlay::catalog::get(&conn, pg).expect("catalog").is_none(),
        "nothing stranded on the pane key"
    );
    assert!(k2_core::overlay::read_thread(pg, 0).expect("pane thread").is_empty());
}

// ── 1. plain Codex tab (the + menu's new chat) ────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn codex_tab_thread_keeps_first_message_and_reply_through_adoption() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let ws = seed_ws(&env, "cxtab");
    let pg = format!("pane{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let agent = format!("tab-{pg}");

    let spawned = post(
        d.port,
        "/cli/sessions/v2/spawn",
        serde_json::json!({
            "agent_name": agent,
            "cwd": ws.path.to_string_lossy(),
            "command": "codex",
            "args": [],
            "cols": 80,
            "rows": 24,
        }),
    );
    assert!(spawned.get("sessionId").is_some() || spawned.get("session_id").is_some(), "{spawned}");
    assert_eq!(tab_session_id(&ws, &pg), None, "codex mints its own id: nothing known yet");

    // The tab's Thread address: the handle the register path issued, keyed
    // on the pane until the id is known.
    let ordinal = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace_session_handles::get(&conn, &ws.id, &pg)
            .expect("handle lookup")
            .unwrap_or_else(|| panic!("a codex tab gets a pane-keyed handle ({pg})"))
            .ordinal
    };
    let addr = format!("{}/{ordinal}", ws.handle);

    // The human's first message from the Thread compose bar.
    let first = post(
        d.port,
        "/cli/thread/post",
        serde_json::json!({ "addr": addr, "text": "hello codex", "via": "compose" }),
    );
    assert_eq!(first["conversation_id"], pg.as_str(), "before discovery the Thread is pane-keyed: {first}");

    let codex_id = uuid::Uuid::new_v4().to_string();
    let (mut on_pane, mut on_provider) =
        futures_block(async { (open_ws(d.port, &pg).await, open_ws(d.port, &codex_id).await) });
    adopt_then_reply_and_check(
        &env,
        d.port,
        &ws,
        &pg,
        &addr,
        &codex_id,
        "hello codex",
        &mut on_pane,
        &mut on_provider,
    );
}

// ── 2. `k2 sidecar new … codex` ──────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn codex_sidecar_thread_keeps_first_message_and_reply_through_adoption() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let ws = seed_ws(&env, "cxsc");
    let v = post(
        d.port,
        "/cli/sidecar/new",
        serde_json::json!({ "workspace": ws.handle, "name": "Scout", "harness": "codex", "brief": "# scout\n" }),
    );
    assert!(v["conversationId"].is_null(), "codex mints its own id: {v}");
    let pg = v["paneGroupId"].as_str().expect("paneGroupId").to_string();
    let addr = v["address"].as_str().expect("address").to_string();

    let first = post(
        d.port,
        "/cli/thread/post",
        serde_json::json!({ "addr": addr, "text": "hello codex", "via": "compose" }),
    );
    assert_eq!(first["conversation_id"], pg.as_str(), "before discovery the Thread is pane-keyed: {first}");

    let codex_id = format!("codex-conv-{}", uuid::Uuid::new_v4());
    let (mut on_pane, mut on_provider) =
        futures_block(async { (open_ws(d.port, &pg).await, open_ws(d.port, &codex_id).await) });
    // The sidecar watch adopts the rollout that carries its brief pointer.
    let marker = k2_core::sidecar::pointer_message(&addr, &ws.handle, "owner", ".k2/sidecars/scout/BRIEF.md");
    adopt_then_reply_and_check(
        &env,
        d.port,
        &ws,
        &pg,
        &addr,
        &codex_id,
        &marker,
        &mut on_pane,
        &mut on_provider,
    );
    // The name and the handle followed too.
    let db = k2_core::db::shared();
    let conn = db.lock();
    assert_eq!(
        k2_core::workspace_session_handles::resolve_handle(&conn, &ws.id, "scout").expect("handle"),
        codex_id
    );
}
