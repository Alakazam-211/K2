//! k2 sidecar v1 — headless daemon tests (`prd-k2-sidecar-cli-v1` T1/T4).
//!
//! Real dispatcher (`test_harness::start`, in-process, ephemeral port,
//! temp `$HOME`), raw HTTP over loopback, no client attached. Every harness
//! binary is a shim under `K2_TEST_AGENT_SHIM_DIR` that records its argv and
//! identity env to a file and then `exec cat`s, so no real AI CLI ever starts.
//!
//! Covers:
//! - owner `new` / `list` / `stop` / resume, the brief file (0600, size cap),
//!   the layout tab, the `session_added` label, the Access Audit lines;
//! - SC48: waking a stopped sidecar with `k2 msg` resumes (`--resume <cid>`)
//!   instead of replaying the stored `--session-id <cid>`;
//! - per-harness resume after a daemon restart for the Big 7 (claude, grok,
//!   gemini, pi, cursor, codex, hermes): each comes back as its own CLI with
//!   its conversation, never as a shell;
//! - agent permission refusals (switch off, sidecar caller, other workspace,
//!   more power, human-made sidecar, --force) and the shell-tab / api-cell
//!   passports;
//! - the HTTP shape: Member login, 405, 413 + close, the cap of 8.

#![cfg(unix)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::Value;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

const OWNER: &str = "owner-token-sidecar-cli-v1";

const HARNESS_SHIMS: &[&str] = &[
    "claude",
    "grok",
    "codex",
    "gemini",
    "cursor-agent",
    "pi",
    "hermes",
    "cat",
];

// ── environment ───────────────────────────────────────────────────────

struct TestEnv {
    home: PathBuf,
    shim_log: PathBuf,
    prev_home: Option<std::ffi::OsString>,
    prev_shim: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        // Stop every session this test started so cat children exit.
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

fn setup() -> TestEnv {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    // Short: the per-cell socket path must fit SUN_LEN.
    let home = PathBuf::from(format!("/tmp/k2sc-{:x}", nanos % 0xffff_ffff));
    let shim_dir = home.join("shim");
    let shim_log = home.join("shim-log");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    std::fs::create_dir_all(&shim_log).expect("shim log dir");
    // One shim body for every harness: `create-chat` prints a fresh id (the
    // cursor-agent subcommand); `--help` answers like Codex 0.155+ (the
    // daemon probes Codex for `--no-daemon`; a real `--help` launches
    // nothing, so it is not recorded); anything else records argv
    // (NUL-separated) and the identity env, then becomes `cat` so the PTY
    // stays open.
    let script = format!(
        "#!/bin/sh\n\
         n=$(/usr/bin/basename \"$0\")\n\
         if [ \"$1\" = \"create-chat\" ]; then /usr/bin/uuidgen | /usr/bin/tr 'A-Z' 'a-z'; exit 0; fi\n\
         if [ \"$1\" = \"--help\" ]; then printf '      --no-daemon\\n          Run without the shared background server\\n'; exit 0; fi\n\
         f=\"{log}/$n-$(/bin/date +%s)-$$\"\n\
         printf 'K2_CELL=%s\\nK2_SIDECAR_NAME=%s\\n' \"$K2_CELL\" \"$K2_SIDECAR_NAME\" > \"$f.env\"\n\
         printf '%s\\0' \"$@\" > \"$f.tmp\"\n\
         /bin/mv \"$f.tmp\" \"$f.argv\"\n\
         exec /bin/cat\n",
        log = shim_log.display()
    );
    for name in HARNESS_SHIMS {
        let p = shim_dir.join(name);
        if *name == "cat" {
            std::fs::write(&p, "#!/bin/sh\nexec /bin/cat\n").expect("cat shim");
        } else {
            std::fs::write(&p, &script).expect("shim");
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod shim");
    }
    let env = TestEnv {
        prev_home: std::env::var_os("HOME"),
        prev_shim: std::env::var_os("K2_TEST_AGENT_SHIM_DIR"),
        home: home.clone(),
        shim_log,
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

fn req(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .expect("read timeout");
    let raw_req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None if method == "POST" => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
        ),
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    stream.write_all(raw_req.as_bytes()).expect("write request");
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset) => break,
            Err(e) => panic!(
                "{method} {path_and_query}: read: {e:?} after {:?}",
                String::from_utf8_lossy(&raw)
            ),
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status in {text:?}"));
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Resp { status, body }
}

fn json(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn post(port: u16, path: &str, token: &str, body: &Value) -> Resp {
    req(
        port,
        "POST",
        &format!("{path}?token={}", enc(token)),
        Some(&body.to_string()),
    )
}

fn error_code(r: &Resp) -> String {
    json(&r.body)["error"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("refusal must carry error.code: {}", r.body))
        .to_string()
}

fn provision_login(port: u16, username: &str, role: &str) -> String {
    let r = req(
        port,
        "POST",
        &format!("/cli/users/add?token={OWNER}"),
        Some(&format!(
            r#"{{"username":"{username}","password":"password123"}}"#
        )),
    );
    assert_eq!(r.status, 200, "users/add({username}); {}", r.body);
    if role != "member" {
        let r = req(
            port,
            "POST",
            &format!("/cli/users/set-role?token={OWNER}"),
            Some(&format!(r#"{{"username":"{username}","role":"{role}"}}"#)),
        );
        assert_eq!(r.status, 200, "set-role; {}", r.body);
    }
    let r = req(
        port,
        "POST",
        "/cli/auth/login",
        Some(&format!(
            r#"{{"username":"{username}","password":"password123"}}"#
        )),
    );
    assert_eq!(r.status, 200, "login {username}; {}", r.body);
    json(&r.body)["token"]
        .as_str()
        .expect("login token")
        .to_string()
}

// ── workspace + DB helpers ───────────────────────────────────────────

struct Ws {
    id: String,
    path: PathBuf,
    handle: String,
    workspace_id: String,
}

fn seed_ws(env: &TestEnv, handle: &str) -> Ws {
    let path = env.home.join(format!("ws-{handle}"));
    std::fs::create_dir_all(&path).expect("workspace dir");
    let id = uuid::Uuid::new_v4().to_string();
    let workspace_id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path.to_string_lossy()],
    )
    .expect("seed project");
    conn.execute(
        "INSERT INTO workspaces (id, project_id, name) VALUES (?1, ?2, 'main')",
        params![workspace_id, id],
    )
    .expect("seed workspaces row");
    Ws {
        id,
        path,
        handle: handle.to_string(),
        workspace_id,
    }
}

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

#[derive(Debug, Clone)]
struct TabRow {
    session_id: Option<String>,
    args: Vec<String>,
    created_by: Option<String>,
    brief_path: Option<String>,
}

fn tab_row(ws: &Ws, pg: &str) -> Option<TabRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT session_id, args_json, created_by, brief_path FROM workspace_tab_sessions \
         WHERE project_id = ?1 AND pane_group_id = ?2",
        params![ws.id, pg],
        |r| {
            let args: Option<String> = r.get(1)?;
            Ok(TabRow {
                session_id: r.get(0)?,
                args: args
                    .and_then(|a| serde_json::from_str(&a).ok())
                    .unwrap_or_default(),
                created_by: r.get(2)?,
                brief_path: r.get(3)?,
            })
        },
    )
    .ok()
}

fn live(agent: &str) -> bool {
    k2_daemon::v2_session_map::lookup_by_agent_name(agent).is_some_and(|s| s.is_child_alive())
}

/// Simulate a daemon restart for one session: the map entry and process go
/// away; every durable row stays.
fn restart_session(agent: &str) {
    let s = k2_daemon::v2_session_map::unregister(agent)
        .unwrap_or_else(|| panic!("{agent} was not live to restart"));
    s.kill();
}

// ── shim runs ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Run {
    file: PathBuf,
    argv: Vec<String>,
    env: HashMap<String, String>,
}

fn runs(env: &TestEnv, name: &str) -> Vec<Run> {
    let mut out: Vec<Run> = std::fs::read_dir(&env.shim_log)
        .expect("shim log dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("argv")
                && p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|f| f.starts_with(&format!("{name}-")))
        })
        .map(|file| {
            let raw = std::fs::read(&file).expect("argv file");
            let argv = raw
                .split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect::<Vec<_>>();
            let env_text = std::fs::read_to_string(file.with_extension("env")).unwrap_or_default();
            let env = env_text
                .lines()
                .filter_map(|l| l.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            Run { file, argv, env }
        })
        .collect();
    out.sort_by_key(|r| {
        std::fs::metadata(&r.file)
            .and_then(|m| m.modified())
            .expect("mtime")
    });
    out
}

fn wait_run(env: &TestEnv, name: &str, before: usize) -> Run {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let all = runs(env, name);
        if all.len() > before {
            return all.last().cloned().expect("a run");
        }
        assert!(
            Instant::now() < deadline,
            "no new {name} run within 20 s (had {before})"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn has_pair(argv: &[String], flag: &str, value: &str) -> bool {
    argv.windows(2).any(|w| w[0] == flag && w[1] == value)
}

// ── sidecar route helpers ────────────────────────────────────────────

fn new_sidecar(
    port: u16,
    token: &str,
    ws: &Ws,
    name: &str,
    harness: &str,
    brief: Option<&str>,
) -> Resp {
    let mut body = serde_json::json!({ "workspace": ws.handle, "name": name, "harness": harness });
    if let Some(b) = brief {
        body["brief"] = Value::String(b.to_string());
    }
    post(port, "/cli/sidecar/new", token, &body)
}

fn stop_sidecar(port: u16, token: &str, ws: &Ws, name: &str, force: bool) -> Resp {
    post(
        port,
        "/cli/sidecar/stop",
        token,
        &serde_json::json!({ "workspace": ws.handle, "name": name, "force": force }),
    )
}

fn list_sidecars(port: u16, token: &str, ws: &Ws) -> Resp {
    req(
        port,
        "GET",
        &format!(
            "/cli/sidecar/list?token={}&workspace={}",
            enc(token),
            enc(&ws.handle)
        ),
        None,
    )
}

fn ok_new(r: &Resp) -> Value {
    assert_eq!(r.status, 200, "sidecar new: {}", r.body);
    json(&r.body)
}

fn layout(ws: &Ws) -> Value {
    let raw = k2_core::db_ops::workspace_layout_load(&ws.id, &ws.workspace_id)
        .expect("layout load")
        .unwrap_or_else(|| panic!("no saved layout for {}", ws.handle));
    serde_json::from_str(&raw).expect("layout JSON")
}

fn layout_tab<'a>(layout: &'a Value, pg: &str) -> Option<&'a Value> {
    layout["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|t| t["paneGroups"].get(pg).is_some()))
}

fn audit_lines(env: &TestEnv) -> Vec<Value> {
    std::fs::read_to_string(env.home.join(".k2").join("auth-audit.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).expect("audit line JSON"))
        .collect()
}

fn mint(session_id: &SessionId, workspace: &str, address: &str) -> String {
    k2_daemon::session_token::mint_session_token(
        session_id,
        &session_id.to_string(),
        HookPrincipal {
            workspace_uuid: workspace.to_string(),
            agent_address: address.to_string(),
        },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

/// Spawn a live session under `key` (the canonical slot when `key` is the
/// project id) and return a passport bound to it.
fn live_session_passport(ws: &Ws, key: &str, command: &str, args: &[&str]) -> String {
    live_session_passport_and_id(ws, key, command, args).0
}

fn live_session_passport_and_id(
    ws: &Ws,
    key: &str,
    command: &str,
    args: &[&str],
) -> (String, SessionId) {
    let out = spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: key.to_string(),
        project_id: Some(ws.id.clone()),
        cwd: ws.path.to_string_lossy().into_owned(),
        command: Some(command.to_string()),
        args: Some(args.iter().map(|s| s.to_string()).collect()),
        cols: 80,
        rows: 24,
        canonical_key: Some(key.to_string()),
        env: HashMap::new(),
        launch_prompt: None,
        label: None,
    })
    .expect("spawn caller session");
    (mint(&out.session_id, &ws.id, key), out.session_id)
}

/// One raw request over a cell's own socket (one request per connection).
fn uds(sock: &Path, raw: &str) -> Resp {
    let mut stream = std::os::unix::net::UnixStream::connect(sock)
        .unwrap_or_else(|e| panic!("connect cell socket {}: {e}", sock.display()));
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .expect("timeout");
    stream.write_all(raw.as_bytes()).expect("write");
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status over UDS: {text:?}"));
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Resp { status, body }
}

fn uds_json(method: &str, path: &str, bearer: &str, body: &str) -> String {
    format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn plant_claude_transcript(env: &TestEnv, ws: &Ws, cid: &str) {
    let hash = k2_core::chat_history::claude_project_hash(&ws.path.to_string_lossy());
    let dir = env.home.join(".claude").join("projects").join(hash);
    std::fs::create_dir_all(&dir).expect("claude projects dir");
    std::fs::write(dir.join(format!("{cid}.jsonl")), "{\"type\":\"user\"}\n")
        .expect("claude transcript");
}

fn plant_grok_transcript(env: &TestEnv, ws: &Ws, cid: &str) {
    let dir = env
        .home
        .join(".grok")
        .join("sessions")
        .join("%2Ffixture")
        .join(cid);
    std::fs::create_dir_all(&dir).expect("grok session dir");
    let summary = serde_json::json!({
        "info": { "id": cid, "cwd": ws.path.to_string_lossy() },
        "last_active_at": "2026-10-07T00:00:00Z",
        "updated_at": "2026-10-07T00:00:00Z",
    });
    std::fs::write(dir.join("summary.json"), summary.to_string()).expect("grok summary");
}

fn plant_gemini_transcript(env: &TestEnv, cid: &str) {
    let dir = env
        .home
        .join(".gemini")
        .join("tmp")
        .join("fixture-slug")
        .join("chats");
    std::fs::create_dir_all(&dir).expect("gemini chats dir");
    std::fs::write(
        dir.join(format!("session-2026-10-07T00-00-{}.jsonl", &cid[..8])),
        format!("{{\"sessionId\":\"{cid}\",\"startTime\":\"2026-10-07T00:00:00Z\"}}\n"),
    )
    .expect("gemini chat file");
}

fn plant_codex_rollout(env: &TestEnv, ws: &Ws, id: &str, marker: &str) {
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
        "payload": { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": marker }] }
    });
    std::fs::write(
        dir.join(format!(
            "rollout-{}-{id}.jsonl",
            now.format("%Y-%m-%dT%H-%M-%S")
        )),
        format!("{meta}\n{user}\n"),
    )
    .expect("codex rollout");
}

fn plant_hermes_session(env: &TestEnv, ws: &Ws, id: &str, marker: &str) {
    let dir = env.home.join(".hermes");
    std::fs::create_dir_all(&dir).expect("hermes dir");
    let conn = rusqlite::Connection::open(dir.join("state.db")).expect("hermes db");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
             id TEXT PRIMARY KEY, source TEXT NOT NULL, parent_session_id TEXT,
             started_at REAL NOT NULL, ended_at REAL, end_reason TEXT,
             message_count INTEGER DEFAULT 0, title TEXT, cwd TEXT, git_branch TEXT,
             git_repo_root TEXT, archived INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS messages (
             id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL REFERENCES sessions(id),
             role TEXT NOT NULL, content TEXT, tool_calls TEXT, timestamp REAL NOT NULL);",
    )
    .expect("hermes schema");
    let now = chrono::Utc::now().timestamp() as f64;
    conn.execute(
        "INSERT INTO sessions (id, source, started_at, cwd, archived) VALUES (?1, 'cli', ?2, ?3, 0)",
        params![id, now, ws.path.to_string_lossy()],
    )
    .expect("hermes session row");
    conn.execute(
        "INSERT INTO messages (session_id, role, content, timestamp) VALUES (?1, 'user', ?2, ?3)",
        params![id, marker, now],
    )
    .expect("hermes message row");
}

fn wait_adopted(ws: &Ws, pg: &str, want: &str) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        let row = tab_row(ws, pg).expect("tab row");
        if row.session_id.as_deref() == Some(want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "conversation {want} was not adopted for {pg} within 25 s (row: {row:?})"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The app's restore visit after a restart: an empty-command v2 spawn of
/// the tab key.
fn app_visit(port: u16, ws: &Ws, pg: &str) -> Value {
    let r = req(
        port,
        "POST",
        &format!("/cli/sessions/v2/spawn?token={OWNER}"),
        Some(
            &serde_json::json!({
                "agent_name": format!("tab-{pg}"),
                "cwd": ws.path.to_string_lossy(),
                "cols": 120,
                "rows": 38,
            })
            .to_string(),
        ),
    );
    assert_eq!(r.status, 200, "v2 spawn visit: {}", r.body);
    json(&r.body)
}

// ── T1: owner new / list / stop / resume / msg wake (claude) ──────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_new_list_stop_resume_and_msg_wake() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let ws = seed_ws(&env, &unique("k"));
    let mut events = k2_daemon::session_events::subscribe();

    let marker = format!("SECRET-BRIEF-MARKER-{}", uuid::Uuid::new_v4());
    let brief = format!("# Gardens\nBuild the gardens.\n{marker}\n");
    let before = runs(&env, "claude").len();
    let v = ok_new(&new_sidecar(
        d.port,
        OWNER,
        &ws,
        "Gardens",
        "claude",
        Some(&brief),
    ));
    let address = format!("{}/gardens", ws.handle);
    assert_eq!(v["address"], address.as_str());
    assert_eq!(v["handle"], "gardens");
    assert_eq!(v["name"], "Gardens");
    assert_eq!(v["harness"], "claude");
    assert_eq!(v["resumed"], false);
    assert_eq!(v["brief"]["path"], ".k2/sidecars/gardens/BRIEF.md");
    assert_eq!(v["brief"]["bytes"].as_u64(), Some(brief.len() as u64));
    assert_eq!(v["brief"]["delivered"], "launch_param");
    assert_eq!(v["createdBy"], "owner");
    let pg = v["paneGroupId"].as_str().expect("paneGroupId").to_string();
    let cid = v["conversationId"]
        .as_str()
        .expect("conversationId")
        .to_string();
    let agent = format!("tab-{pg}");
    assert!(live(&agent), "map key {agent} must be live");

    // The harness argv: premint, pointer last, never the brief text.
    let run = wait_run(&env, "claude", before);
    assert!(
        has_pair(&run.argv, "--session-id", &cid),
        "premint missing: {:?}",
        run.argv
    );
    let last = run.argv.last().expect("argv");
    assert!(
        last.starts_with(&format!("[k2 sidecar] You are {address},")),
        "pointer last: {last:?}"
    );
    assert!(last.contains(".k2/sidecars/gardens/BRIEF.md"), "{last}");
    assert!(
        !run.argv.iter().any(|a| a.contains(&marker)),
        "brief text leaked into argv"
    );
    assert_eq!(run.env.get("K2_CELL").map(String::as_str), Some("sidecar"));
    assert_eq!(
        run.env.get("K2_SIDECAR_NAME").map(String::as_str),
        Some("gardens")
    );

    // BRIEF.md: content, 0600, .gitignore.
    let brief_path = ws.path.join(".k2/sidecars/gardens/BRIEF.md");
    assert_eq!(
        std::fs::read_to_string(&brief_path).expect("BRIEF.md"),
        brief
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&brief_path)
            .expect("meta")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_to_string(ws.path.join(".k2/sidecars/.gitignore")).expect("gitignore"),
        "*\n"
    );

    // Tab row + handle + Chats name.
    let row = tab_row(&ws, &pg).expect("tab row");
    assert_eq!(row.created_by.as_deref(), Some("owner"));
    assert_eq!(
        row.brief_path.as_deref(),
        Some(".k2/sidecars/gardens/BRIEF.md")
    );
    assert_eq!(row.session_id.as_deref(), Some(cid.as_str()));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(
            k2_core::workspace_session_handles::resolve_handle(&conn, &ws.id, "gardens")
                .expect("handle"),
            cid
        );
        assert_eq!(
            k2_core::workspace_session_handles::custom_name_for_session_id(&conn, &cid)
                .expect("name"),
            Some("Gardens".to_string())
        );
    }

    // Layout: adopted-<pg>, titled, locked (SC34).
    let lay = layout(&ws);
    let tab = layout_tab(&lay, &pg).expect("layout tab for the sidecar");
    assert_eq!(tab["id"], format!("adopted-{pg}"));
    assert_eq!(tab["title"], "Gardens");
    assert_eq!(tab["locked"], true);

    // session_added carries the label (SC33).
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_label = false;
    while Instant::now() < deadline && !saw_label {
        match events.try_recv() {
            Ok(k2_daemon::session_events::SessionEvent::SessionAdded {
                agent_name,
                label,
                label_locked,
                ..
            }) if agent_name == agent => {
                assert_eq!(label.as_deref(), Some("Gardens"));
                assert!(label_locked, "sidecar label must be locked");
                saw_label = true;
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    assert!(saw_label, "no session_added frame for {agent}");

    // The brief never reaches the audit log or any daemon-home file.
    for entry in walk(&env.home.join(".k2")) {
        let text = std::fs::read(&entry).unwrap_or_default();
        assert!(
            !String::from_utf8_lossy(&text).contains(&marker),
            "brief marker found in {}",
            entry.display()
        );
    }
    let audit = audit_lines(&env);
    let new_line = audit
        .iter()
        .find(|l| l["event"] == "sidecar-new")
        .unwrap_or_else(|| panic!("no sidecar-new audit line: {audit:?}"));
    assert_eq!(new_line["user"], "owner");
    assert_eq!(new_line["client"], "cli");
    let outcome = new_line["outcome"].as_str().expect("outcome");
    assert!(outcome.starts_with(&format!("ok {address}")), "{outcome}");
    assert!(
        outcome.contains(&format!("brief_bytes={}", brief.len())),
        "{outcome}"
    );

    // list: live.
    let r = list_sidecars(d.port, OWNER, &ws);
    assert_eq!(r.status, 200, "{}", r.body);
    let l = json(&r.body);
    assert_eq!(l["cap"], 8);
    assert_eq!(l["live"], 1);
    assert_eq!(l["agentsCanManage"], false);
    let rows = l["sidecars"].as_array().expect("sidecars");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["address"], address.as_str());
    assert_eq!(rows[0]["state"], "live");
    assert_eq!(rows[0]["attached"], false);
    assert_eq!(rows[0]["createdBy"], "owner");
    assert_eq!(rows[0]["harness"], "claude");

    // Collision while live.
    let r = new_sidecar(d.port, OWNER, &ws, "Gardens", "claude", None);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (409, "sidecar_live"),
        "{}",
        r.body
    );

    // stop: process gone, row/handle kept, layout tab gone, event emitted.
    let mut events = k2_daemon::session_events::subscribe();
    let r = stop_sidecar(d.port, OWNER, &ws, "gardens", false);
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(json(&r.body)["stopped"], true);
    assert!(!live(&agent), "stop must end the process");
    assert!(tab_row(&ws, &pg).is_some(), "stop keeps the tab row");
    assert!(
        layout_tab(&layout(&ws), &pg).is_none(),
        "stop removes the layout tab"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_removed = false;
    while Instant::now() < deadline && !saw_removed {
        match events.try_recv() {
            Ok(k2_daemon::session_events::SessionEvent::SessionRemoved { agent_name, .. })
                if agent_name == agent =>
            {
                saw_removed = true
            }
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    assert!(saw_removed, "stop must emit session_removed");
    let r = stop_sidecar(d.port, OWNER, &ws, "gardens", false);
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(json(&r.body)["stopped"], false);
    assert_eq!(json(&r.body)["state"], "asleep");
    let l = json(&list_sidecars(d.port, OWNER, &ws).body);
    assert_eq!(l["sidecars"][0]["state"], "asleep");

    // SC48: `k2 msg` wakes the stopped sidecar with --resume, not the
    // stored --session-id premint (the tab row still holds it).
    assert!(
        has_pair(&tab_row(&ws, &pg).expect("row").args, "--session-id", &cid),
        "precondition: the stored args hold the premint"
    );
    plant_claude_transcript(&env, &ws, &cid);
    let before = runs(&env, "claude").len();
    let r = req(
        d.port,
        "POST",
        &format!(
            "/cli/workspace/msg?token={OWNER}&workspace={}&text=hi%20there&from=k2",
            enc(&address)
        ),
        None,
    );
    assert_eq!(r.status, 200, "msg wake: {}", r.body);
    assert_eq!(json(&r.body)["success"], true, "{}", r.body);
    let run = wait_run(&env, "claude", before);
    assert!(
        has_pair(&run.argv, "--resume", &cid),
        "msg wake must resume: {:?}",
        run.argv
    );
    assert!(
        !run.argv.iter().any(|a| a == "--session-id"),
        "msg wake replayed the premint: {:?}",
        run.argv
    );
    assert!(
        !run.argv.iter().any(|a| a.contains("Your brief is")),
        "brief pointer re-sent on wake"
    );
    let woke = k2_daemon::v2_session_map::lookup_by_agent_name(&agent).expect("woken session");
    assert_eq!(
        woke.label(),
        "Gardens",
        "SC30: a woken sidecar keeps its own label"
    );

    // Restart (the daemon goes away) → list shows asleep → msg wakes it again
    // with --resume; the pointer is not re-sent.
    restart_session(&agent);
    let l = json(&list_sidecars(d.port, OWNER, &ws).body);
    assert_eq!(l["sidecars"][0]["state"], "asleep");
    let before = runs(&env, "claude").len();
    let r = req(
        d.port,
        "POST",
        &format!(
            "/cli/workspace/msg?token={OWNER}&workspace={}&text=again&from=k2",
            enc(&address)
        ),
        None,
    );
    assert_eq!(r.status, 200, "msg wake after restart: {}", r.body);
    let run = wait_run(&env, "claude", before);
    assert!(has_pair(&run.argv, "--resume", &cid), "{:?}", run.argv);
    assert!(!run.argv.iter().any(|a| a.contains("Your brief is")));

    // new on the stopped name resumes: same address, --resume, new brief.
    let r = stop_sidecar(d.port, OWNER, &ws, "Gardens", false);
    assert_eq!(r.status, 200, "{}", r.body);
    let before = runs(&env, "claude").len();
    let v = ok_new(&new_sidecar(
        d.port,
        OWNER,
        &ws,
        "Gardens",
        "claude",
        Some("# v2 brief\n"),
    ));
    assert_eq!(v["resumed"], true);
    assert_eq!(v["address"], address.as_str());
    assert_eq!(v["paneGroupId"], pg.as_str());
    assert_eq!(v["conversationId"], cid.as_str());
    let run = wait_run(&env, "claude", before);
    assert!(has_pair(&run.argv, "--resume", &cid), "{:?}", run.argv);
    assert!(
        !run.argv.iter().any(|a| a == "--session-id"),
        "{:?}",
        run.argv
    );
    assert!(
        run.argv
            .last()
            .expect("argv")
            .contains("your brief changed"),
        "{:?}",
        run.argv.last()
    );
    assert_eq!(
        std::fs::read_to_string(&brief_path).expect("BRIEF.md v2"),
        "# v2 brief\n"
    );
    assert!(audit_lines(&env)
        .iter()
        .any(|l| l["event"] == "sidecar-resume"));
    assert!(audit_lines(&env)
        .iter()
        .any(|l| l["event"] == "sidecar-stop"));

    // A name used by a different harness is name_taken.
    stop_sidecar(d.port, OWNER, &ws, "gardens", false);
    let r = new_sidecar(d.port, OWNER, &ws, "Gardens", "grok", None);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (409, "name_taken"),
        "{}",
        r.body
    );
    // Name rules.
    let r = new_sidecar(d.port, OWNER, &ws, "Gardens & Apps", "claude", None);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (400, "bad_name"),
        "{}",
        r.body
    );
    assert!(json(&r.body)["error"]["hint"]
        .as_str()
        .expect("hint")
        .contains("gardens-and-apps"));
    let r = new_sidecar(d.port, OWNER, &ws, "5", "claude", None);
    assert_eq!((r.status, error_code(&r).as_str()), (400, "bad_name"));
    // Unknown / unsupported presets.
    let r = new_sidecar(d.port, OWNER, &ws, "x1", "no-such-preset", None);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (400, "harness_unknown")
    );
    let r = new_sidecar(d.port, OWNER, &ws, "x2", "aider", None);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (400, "harness_not_supported"),
        "{}",
        r.body
    );
    // Brief over 64 KiB and a NUL byte.
    let r = new_sidecar(
        d.port,
        OWNER,
        &ws,
        "big",
        "claude",
        Some(&"x".repeat(64 * 1024 + 1)),
    );
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (413, "brief_too_large"),
        "{}",
        r.body
    );
    assert!(
        !ws.path.join(".k2/sidecars/big").exists(),
        "an oversized brief must not be written"
    );
    let r = new_sidecar(d.port, OWNER, &ws, "nul", "claude", Some("a\u{0}b"));
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (400, "brief_not_text"),
        "{}",
        r.body
    );
    // Exactly 64 KiB is fine.
    let v = ok_new(&new_sidecar(
        d.port,
        OWNER,
        &ws,
        "edge",
        "claude",
        Some(&"y".repeat(64 * 1024)),
    ));
    assert_eq!(v["brief"]["bytes"].as_u64(), Some(64 * 1024));
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

// ── per-harness resume after a restart (Big 7) ───────────────────────

/// Open a sidecar with `harness`, give the harness its on-disk
/// conversation where it needs one, restart, visit like the app, and
/// return (first run, resumed run, conversation id).
fn roundtrip(env: &TestEnv, port: u16, harness: &str, shim: &str) -> (Run, Run, String, Ws) {
    let ws = seed_ws(env, &unique(&format!("h{}", &harness[..2])));
    let before = runs(env, shim).len();
    let v = ok_new(&new_sidecar(
        port,
        OWNER,
        &ws,
        "Worker",
        harness,
        Some("# brief\nwork\n"),
    ));
    assert_eq!(v["harness"].as_str().expect("harness"), harness);
    let pg = v["paneGroupId"].as_str().expect("pg").to_string();
    let address = v["address"].as_str().expect("address").to_string();
    let first = if harness == "hermes" {
        wait_run(env, shim, before)
    } else {
        wait_run(env, shim, before)
    };
    let cid = match harness {
        "codex" | "hermes" => {
            assert!(
                v["conversationId"].is_null(),
                "{harness} mints its own id: {v}"
            );
            let id = format!("{harness}-conv-{}", uuid::Uuid::new_v4());
            let marker = k2_core::sidecar::pointer_message(
                &address,
                &ws.handle,
                "owner",
                ".k2/sidecars/worker/BRIEF.md",
            );
            if harness == "codex" {
                plant_codex_rollout(env, &ws, &id, &marker);
            } else {
                plant_hermes_session(env, &ws, &id, &marker);
            }
            wait_adopted(&ws, &pg, &id);
            id
        }
        _ => v["conversationId"]
            .as_str()
            .expect("conversationId")
            .to_string(),
    };
    match harness {
        "claude" => plant_claude_transcript(env, &ws, &cid),
        "grok" => plant_grok_transcript(env, &ws, &cid),
        "gemini" => plant_gemini_transcript(env, &cid),
        _ => {}
    }
    // The handle keeps working after adoption.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        assert_eq!(
            k2_core::workspace_session_handles::resolve_handle(&conn, &ws.id, "worker")
                .expect("handle"),
            cid
        );
    }
    let agent = format!("tab-{pg}");
    restart_session(&agent);
    let before = runs(env, shim).len();
    let visit = app_visit(port, &ws, &pg);
    assert_eq!(visit["reused"], false, "{visit}");
    let resumed = wait_run(env, shim, before);
    let live_session =
        k2_daemon::v2_session_map::lookup_by_agent_name(&agent).expect("live after visit");
    assert_eq!(
        live_session
            .program
            .as_deref()
            .map(|p| p.rsplit('/').next().unwrap_or(p)),
        Some(shim),
        "{harness} must come back as its own CLI, never a shell"
    );
    assert!(
        !resumed.argv.iter().any(|a| a.contains("Your brief is")),
        "{harness}: the brief pointer must not be re-sent on resume: {:?}",
        resumed.argv
    );
    (first, resumed, cid, ws)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_after_restart_claude() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let (first, resumed, cid, _) = roundtrip(&env, d.port, "claude", "claude");
    assert!(
        has_pair(&first.argv, "--session-id", &cid),
        "{:?}",
        first.argv
    );
    assert!(
        has_pair(&resumed.argv, "--resume", &cid),
        "{:?}",
        resumed.argv
    );
    assert!(
        !resumed.argv.iter().any(|a| a == "--session-id"),
        "{:?}",
        resumed.argv
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_after_restart_grok() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let (first, resumed, cid, _) = roundtrip(&env, d.port, "grok", "grok");
    assert!(
        has_pair(&first.argv, "--session-id", &cid),
        "{:?}",
        first.argv
    );
    assert!(
        has_pair(&resumed.argv, "--resume", &cid),
        "{:?}",
        resumed.argv
    );
    assert!(
        !resumed.argv.iter().any(|a| a == "--session-id"),
        "{:?}",
        resumed.argv
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_after_restart_gemini() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let (first, resumed, cid, _) = roundtrip(&env, d.port, "gemini", "gemini");
    assert!(
        has_pair(&first.argv, "--session-id", &cid),
        "{:?}",
        first.argv
    );
    assert!(
        has_pair(&resumed.argv, "--resume", &cid),
        "{:?}",
        resumed.argv
    );
    assert!(
        !resumed.argv.iter().any(|a| a == "--session-id"),
        "{:?}",
        resumed.argv
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_after_restart_pi() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let (first, resumed, cid, ws) = roundtrip(&env, d.port, "pi", "pi");
    let dir = k2_core::sidecar::pi_sessions_dir(&ws.path.to_string_lossy()).expect("pi dir");
    assert!(
        Path::new(&cid).starts_with(&dir),
        "pi session file {cid} must live in {}",
        dir.display()
    );
    assert!(cid.ends_with(".jsonl"), "{cid}");
    assert!(has_pair(&first.argv, "--session", &cid), "{:?}", first.argv);
    assert!(
        has_pair(&resumed.argv, "--session", &cid),
        "{:?}",
        resumed.argv
    );
    assert_eq!(
        resumed.argv.iter().filter(|a| *a == "--session").count(),
        1,
        "{:?}",
        resumed.argv
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_after_restart_cursor_agent() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let (first, resumed, cid, _) = roundtrip(&env, d.port, "cursor", "cursor-agent");
    assert!(uuid::Uuid::parse_str(&cid).is_ok(), "create-chat id: {cid}");
    assert!(has_pair(&first.argv, "--resume", &cid), "{:?}", first.argv);
    assert!(
        has_pair(&resumed.argv, "--resume", &cid),
        "{:?}",
        resumed.argv
    );
    assert_eq!(
        resumed.argv.iter().filter(|a| *a == "--resume").count(),
        1,
        "{:?}",
        resumed.argv
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_after_restart_codex() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let (first, resumed, cid, _) = roundtrip(&env, d.port, "codex", "codex");
    assert!(
        !first.argv.iter().any(|a| a == "resume"),
        "{:?}",
        first.argv
    );
    // Codex 0.155+: each sidecar runs its own app-server, so its tool
    // commands carry THIS sidecar's passport (not a shared server's).
    assert_eq!(first.argv.first().map(String::as_str), Some("--no-daemon"), "{:?}", first.argv);
    assert_eq!(resumed.argv.first().map(String::as_str), Some("--no-daemon"), "{:?}", resumed.argv);
    assert!(
        first.argv.last().expect("argv").contains("Your brief is"),
        "{:?}",
        first.argv
    );
    assert!(
        has_pair(&resumed.argv, "resume", &cid),
        "{:?}",
        resumed.argv
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_after_restart_hermes() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let (first, resumed, cid, _) = roundtrip(&env, d.port, "hermes", "hermes");
    // hermes has no first-turn argument: the pointer is pasted after start.
    assert!(
        !first.argv.iter().any(|a| a.contains("Your brief is")),
        "{:?}",
        first.argv
    );
    assert!(
        has_pair(&resumed.argv, "--resume", &cid),
        "{:?}",
        resumed.argv
    );
}

/// codex before its id is discovered: a restart brings back a FRESH codex
/// that re-reads BRIEF.md (never a shell), and watches for the new id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn codex_without_discovery_restarts_fresh_and_rereads_the_brief() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let ws = seed_ws(&env, &unique("cx"));
    let before = runs(&env, "codex").len();
    let v = ok_new(&new_sidecar(
        d.port,
        OWNER,
        &ws,
        "Scout",
        "codex",
        Some("# scout\n"),
    ));
    let pg = v["paneGroupId"].as_str().expect("pg").to_string();
    wait_run(&env, "codex", before);
    restart_session(&format!("tab-{pg}"));
    let before = runs(&env, "codex").len();
    app_visit(d.port, &ws, &pg);
    let run = wait_run(&env, "codex", before);
    assert!(
        !run.argv.iter().any(|a| a == "resume"),
        "nothing to resume yet: {:?}",
        run.argv
    );
    let last = run.argv.last().expect("argv");
    assert!(
        last.contains("fresh start") && last.contains(".k2/sidecars/scout/BRIEF.md"),
        "{last}"
    );
    assert!(
        k2_daemon::v2_session_map::lookup_by_agent_name(&format!("tab-{pg}"))
            .and_then(|s| s.program.clone())
            .is_some_and(|p| p.ends_with("codex")),
        "codex must come back as codex"
    );
    // The new conversation is adopted once codex writes it.
    let id = format!("codex-conv-{}", uuid::Uuid::new_v4());
    plant_codex_rollout(
        &env,
        &ws,
        &id,
        &format!("[k2 sidecar] You are {}/scout,", ws.handle),
    );
    wait_adopted(&ws, &pg, &id);
}

// ── agent gate (§7.3) ────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_permission_refusals() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let ws = seed_ws(&env, &unique("ag"));
    let other = seed_ws(&env, &unique("ot"));

    // The main session runs claude with skip-permissions (as the preset).
    let (canonical, canonical_sid) =
        live_session_passport_and_id(&ws, &ws.id, "claude", &["--dangerously-skip-permissions"]);

    // Switch off → gated, with the hint naming the switch.
    let r = new_sidecar(d.port, &canonical, &ws, "helper", "claude", None);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (403, "gated"),
        "{}",
        r.body
    );
    assert!(json(&r.body)["error"]["hint"]
        .as_str()
        .expect("hint")
        .contains("Allow hiring and managing agents"));
    assert!(audit_lines(&env).iter().any(|l| l["event"] == "sidecar-new"
        && l["outcome"] == "refused:gated"
        && l["user"] == format!("agent:{}", ws.handle)));

    // A passport may not flip the switch itself.
    let r = post(
        d.port,
        "/cli/agent-access/set",
        &canonical,
        &serde_json::json!({ "workspace": ws.handle, "toggle": "agents_manage", "value": true }),
    );
    assert_eq!(r.status, 403, "{}", r.body);
    assert!(!k2_core::workspace::settings::agents_can_manage_agents(
        &ws.id
    ));

    // The owner flips it (and the list reports it).
    let r = post(
        d.port,
        "/cli/agent-access/set",
        OWNER,
        &serde_json::json!({ "workspace": ws.handle, "toggle": "agents_manage", "value": true }),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(k2_core::workspace::settings::agents_can_manage_agents(
        &ws.id
    ));
    assert!(audit_lines(&env)
        .iter()
        .any(|l| l["event"] == "agent-access-set"));
    let r = post(
        d.port,
        "/cli/agent-access/set",
        OWNER,
        &serde_json::json!({ "workspace": ws.handle, "toggle": "something_else", "value": true }),
    );
    assert_eq!(r.status, 400, "{}", r.body);

    // On + canonical → 200; created_by is the agent.
    let v = ok_new(&new_sidecar(
        d.port,
        &canonical,
        &ws,
        "helper",
        "claude",
        Some("# help\n"),
    ));
    assert_eq!(v["createdBy"], format!("{} (agent)", ws.handle));
    let helper_pg = v["paneGroupId"].as_str().expect("pg").to_string();
    // The same over the main session's own cell socket (`k2` in a K2
    // terminal prefers it): list, new, 405 on GET, 413 on an oversized body.
    let sock = k2_daemon::cell_uds::cell_socket_path(&canonical_sid);
    let r = uds(
        &sock,
        &format!(
            "GET /cli/sidecar/list?workspace={} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {canonical}\r\n\r\n",
            enc(&ws.handle)
        ),
    );
    assert_eq!(r.status, 200, "UDS list: {}", r.body);
    assert_eq!(json(&r.body)["agentsCanManage"], true);
    let r = uds(
        &sock,
        &uds_json(
            "POST",
            "/cli/sidecar/new",
            &canonical,
            &serde_json::json!({ "name": "uds-helper", "harness": "claude" }).to_string(),
        ),
    );
    assert_eq!(r.status, 200, "UDS new: {}", r.body);
    assert_eq!(
        json(&r.body)["address"],
        format!("{}/uds-helper", ws.handle)
    );
    let r = uds(
        &sock,
        &format!("GET /cli/sidecar/new HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {canonical}\r\n\r\n"),
    );
    assert_eq!(r.status, 405, "UDS GET new: {}", r.body);
    let big = serde_json::json!({ "name": "uds-big", "harness": "claude", "brief": "q".repeat(100 * 1024) })
        .to_string();
    let r = uds(
        &sock,
        &uds_json("POST", "/cli/sidecar/new", &canonical, &big),
    );
    assert_eq!(r.status, 413, "UDS oversized: {}", r.body);
    assert!(!ws.path.join(".k2/sidecars/uds-big").exists());

    // A passport naming another workspace → forbidden.
    let r = new_sidecar(d.port, &canonical, &other, "x", "claude", None);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (403, "forbidden"),
        "{}",
        r.body
    );
    // The sidecar's own passport → forbidden (sidecars cannot make sidecars).
    let helper_session =
        k2_daemon::v2_session_map::lookup_by_agent_name(&format!("tab-{helper_pg}"))
            .expect("helper live");
    let sidecar_passport = mint(
        &helper_session.session_id,
        &ws.id,
        &format!("tab-{helper_pg}"),
    );
    let r = new_sidecar(d.port, &sidecar_passport, &ws, "nested", "claude", None);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (403, "forbidden"),
        "{}",
        r.body
    );
    // A sidecar may list.
    let r = list_sidecars(d.port, &sidecar_passport, &ws);
    assert_eq!(r.status, 200, "{}", r.body);

    // A human-made sidecar the agent may not stop; its own it may; never --force.
    ok_new(&new_sidecar(d.port, OWNER, &ws, "human", "claude", None));
    let r = stop_sidecar(d.port, &canonical, &ws, "human", false);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (403, "forbidden"),
        "{}",
        r.body
    );
    let r = stop_sidecar(d.port, &canonical, &ws, "helper", true);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (403, "forbidden"),
        "{}",
        r.body
    );
    let r = stop_sidecar(d.port, &canonical, &ws, "helper", false);
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(json(&r.body)["stopped"], true);

    // More power than itself: a main session without skip flags may not open
    // a preset that skips approvals.
    let ws2 = seed_ws(&env, &unique("pw"));
    k2_core::workspace::settings::set_agents_can_manage_agents(&ws2.id, true).expect("switch on");
    let careful = live_session_passport(&ws2, &ws2.id, "claude", &[]);
    let r = new_sidecar(d.port, &careful, &ws2, "risky", "claude", None);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (403, "forbidden"),
        "{}",
        r.body
    );
    assert!(json(&r.body)["error"]["hint"]
        .as_str()
        .expect("hint")
        .contains("more power"));
    // Pi's preset carries no skip flag: allowed.
    ok_new(&new_sidecar(d.port, &careful, &ws2, "gentle", "pi", None));

    // A plain shell tab is a human typing: treated as the owner, even with
    // the switch off.
    let ws3 = seed_ws(&env, &unique("sh"));
    let shell = live_session_passport(&ws3, &format!("tab-{}", uuid::Uuid::new_v4()), "cat", &[]);
    let v = ok_new(&new_sidecar(d.port, &shell, &ws3, "byhand", "claude", None));
    assert_eq!(v["createdBy"], format!("{} (shell)", ws3.handle));

    // An API host-session passport → wrong_credential.
    let api = mint(&SessionId::new(), &ws.id, "api-owner-cell");
    let r = new_sidecar(d.port, &api, &ws, "api", "claude", None);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (403, "wrong_credential"),
        "{}",
        r.body
    );
    // An app pass / API key shape → wrong_credential.
    let r = new_sidecar(d.port, "k2sk_notarealkey", &ws, "api", "claude", None);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (403, "wrong_credential"),
        "{}",
        r.body
    );
    // Garbage → forbidden.
    let r = new_sidecar(d.port, "garbage", &ws, "api", "claude", None);
    assert_eq!(r.status, 403, "{}", r.body);
}

// ── HTTP shape: Member, 405, 413 + close, cap ────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_shape_member_405_413_and_cap() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let ws = seed_ws(&env, &unique("hs"));

    // Member login: new → role_required; list → 200. Admin login: new → 200.
    let member = provision_login(d.port, &unique("m"), "member");
    let r = new_sidecar(d.port, &member, &ws, "m1", "claude", None);
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(json(&r.body)["error"], "role_required", "{}", r.body);
    let r = list_sidecars(d.port, &member, &ws);
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(
        d.port,
        "/cli/agent-access/set",
        &member,
        &serde_json::json!({ "workspace": ws.handle, "toggle": "agents_manage", "value": true }),
    );
    assert_eq!(r.status, 403, "{}", r.body);
    let admin_user = unique("a");
    let admin = provision_login(d.port, &admin_user, "admin");
    let v = ok_new(&new_sidecar(d.port, &admin, &ws, "a1", "claude", None));
    assert_eq!(v["createdBy"], admin_user.as_str());

    // GET on a POST route → 405 (central + require_post).
    for path in [
        "/cli/sidecar/new",
        "/cli/sidecar/stop",
        "/cli/agent-access/set",
    ] {
        let r = req(d.port, "GET", &format!("{path}?token={OWNER}"), None);
        assert_eq!(r.status, 405, "GET {path}: {}", r.body);
    }

    // 100 KiB body → 413 and the connection closes (the rest is never parsed
    // as the next request).
    {
        let mut stream = StdTcpStream::connect(("127.0.0.1", d.port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .expect("timeout");
        let body = format!(
            r#"{{"workspace":"{}","name":"huge","harness":"claude","brief":"{}"}}"#,
            ws.handle,
            "z".repeat(100 * 1024)
        );
        let head = format!(
            "POST /cli/sidecar/new?token={OWNER} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).expect("head");
        let _ = stream.write_all(body.as_bytes());
        let mut raw = Vec::new();
        let mut chunk = [0u8; 4096];
        let closed = loop {
            match stream.read(&mut chunk) {
                Ok(0) => break true,
                Ok(n) => raw.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break true,
                Err(e) => panic!("413 read: {e:?} after {:?}", String::from_utf8_lossy(&raw)),
            }
        };
        let text = String::from_utf8_lossy(&raw);
        assert!(text.starts_with("HTTP/1.1 413"), "{text}");
        assert!(closed, "the socket must close after 413");
        assert!(!ws.path.join(".k2/sidecars/huge").exists());
    }

    // Cap: K2_SIDECAR_CAP=2 → the third live sidecar is refused; app-made
    // (and admin-made) ones count.
    std::env::set_var("K2_SIDECAR_CAP", "2");
    let second = new_sidecar(d.port, OWNER, &ws, "c2", "claude", None);
    let third = new_sidecar(d.port, OWNER, &ws, "c3", "claude", None);
    std::env::remove_var("K2_SIDECAR_CAP");
    assert_eq!(second.status, 200, "{}", second.body);
    assert_eq!(
        (third.status, error_code(&third).as_str()),
        (409, "sidecar_cap"),
        "{}",
        third.body
    );
    // Default cap is 8.
    assert_eq!(k2_core::sidecar::cap(), 8);
    for i in 3..=8 {
        ok_new(&new_sidecar(
            d.port,
            OWNER,
            &ws,
            &format!("d{i}"),
            "claude",
            None,
        ));
    }
    let r = new_sidecar(d.port, OWNER, &ws, "d9", "claude", None);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (409, "sidecar_cap"),
        "{}",
        r.body
    );
    // Stop is how a slot comes back.
    assert_eq!(stop_sidecar(d.port, OWNER, &ws, "d8", false).status, 200);
    ok_new(&new_sidecar(d.port, OWNER, &ws, "d9", "claude", None));

    // Unknown workspace → 404; a missing harness binary → harness_not_installed.
    let ghost = Ws {
        id: String::new(),
        path: PathBuf::new(),
        handle: "no-such-ws".into(),
        workspace_id: String::new(),
    };
    let r = new_sidecar(d.port, OWNER, &ghost, "g", "claude", None);
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (404, "workspace_not_found"),
        "{}",
        r.body
    );
    // The binary must resolve where spawns look (here: the shim dir).
    let shim = env.home.join("shim").join("hermes");
    let parked = env.home.join("hermes.parked");
    std::fs::rename(&shim, &parked).expect("park hermes shim");
    let ws2 = seed_ws(&env, &unique("mi"));
    let r = new_sidecar(d.port, OWNER, &ws2, "missing", "hermes", None);
    std::fs::rename(&parked, &shim).expect("restore hermes shim");
    assert_eq!(
        (r.status, error_code(&r).as_str()),
        (409, "harness_not_installed"),
        "{}",
        r.body
    );
}

// ── T4: route table ──────────────────────────────────────────────────

#[test]
fn route_table_and_agent_verbs() {
    use k2_daemon::routes::route_policy::{lookup, Floor};
    let new = lookup("/cli/sidecar/new").expect("new row");
    assert_eq!((new.get, new.post), (None, Some(Floor::Admin)));
    let stop = lookup("/cli/sidecar/stop").expect("stop row");
    assert_eq!((stop.get, stop.post), (None, Some(Floor::Admin)));
    let list = lookup("/cli/sidecar/list").expect("list row");
    assert_eq!((list.get, list.post), (Some(Floor::Member), None));
    let access = lookup("/cli/agent-access/set").expect("access row");
    assert_eq!((access.get, access.post), (None, Some(Floor::Admin)));
    for p in ["/cli/sidecar/new", "/cli/sidecar/list", "/cli/sidecar/stop"] {
        assert!(
            k2_daemon::session_token::is_agent_verb(p),
            "{p} must be an agent verb"
        );
    }
    assert!(!k2_daemon::session_token::is_agent_verb(
        "/cli/agent-access/set"
    ));
}

// ── Thread survives a tab rename (prd-thread-survives-tab-rename-v1) ──

fn grid_contains(agent: &str, needle: &str) -> bool {
    k2_daemon::v2_session_map::lookup_by_agent_name(agent)
        .map(|s| s.visible_text_rows().join("\n").contains(needle))
        .unwrap_or(false)
}

/// Headless smoke as a test (S7, TR8, TR11, Q8): a live `k2 sidecar`
/// renamed from Reviewer to Critic. The live PTY gets the one notice line
/// and its label (locked), the old addresses keep posting into the same
/// Thread with `movedFrom`, and `k2 sidecar new reviewer` is refused
/// without touching the renamed sidecar's brief.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rename_keeps_old_addresses_notifies_the_live_chat_and_reserves_the_name() {
    let env = setup();
    let d = futures_block(test_harness::start(OWNER));
    let ws = seed_ws(&env, &unique("rn"));
    let brief = "# Reviewer\nReview the tests.\n";
    let v = ok_new(&new_sidecar(d.port, OWNER, &ws, "Reviewer", "claude", Some(brief)));
    let cid = v["conversationId"].as_str().expect("conversationId").to_string();
    let pg = v["paneGroupId"].as_str().expect("paneGroupId").to_string();
    let agent = format!("tab-{pg}");
    assert!(live(&agent), "{agent} live");
    let old_name = format!("{}/reviewer", ws.handle);
    let ordinal_addr = format!("{}/1", ws.handle);
    let new_name = format!("{}/critic", ws.handle);

    let first = post(
        d.port,
        "/cli/thread/post",
        OWNER,
        &serde_json::json!({ "addr": ordinal_addr, "text": "before", "from": "k2" }),
    );
    assert_eq!(first.status, 200, "{}", first.body);

    let renamed = post(
        d.port,
        "/cli/chat/rename",
        OWNER,
        &serde_json::json!({
            "provider": "claude",
            "session_id": cid,
            "custom_name": "Critic",
            "project_path": ws.path.to_string_lossy(),
        }),
    );
    assert_eq!(renamed.status, 200, "{}", renamed.body);
    let rj = json(&renamed.body);
    assert_eq!(rj["address"], new_name.as_str(), "{rj}");
    assert_eq!(rj["previousAddress"], old_name.as_str(), "{rj}");

    // TR8: the live session gets exactly the notice line (the shim is cat).
    // Grid rows wrap at the PTY width; match a short, unwrapped prefix.
    let notice = "[k2] This chat's address is now";
    let deadline = Instant::now() + Duration::from_secs(10);
    while !grid_contains(&agent, &notice) {
        assert!(Instant::now() < deadline, "the live sidecar never got the rename notice");
        std::thread::sleep(Duration::from_millis(100));
    }
    // Q8: the daemon label follows the rename, locked.
    let session = k2_daemon::v2_session_map::lookup_by_agent_name(&agent).expect("live session");
    assert_eq!(session.label(), "Critic");
    assert!(
        format!("{:?}", session.label_source()).contains("Locked"),
        "a reconnect keeps the new name: {:?}",
        session.label_source()
    );

    for (addr, text) in [(&old_name, "via-old-name"), (&ordinal_addr, "via-ordinal")] {
        let r = post(
            d.port,
            "/cli/thread/post",
            OWNER,
            &serde_json::json!({ "addr": addr, "text": text, "from": "k2" }),
        );
        assert_eq!(r.status, 200, "post to {addr}: {}", r.body);
        let j = json(&r.body);
        assert_eq!(j["conversation_id"], cid.as_str(), "{j}");
        assert_eq!(j["addr"], new_name.as_str(), "{j}");
        assert_eq!(j["movedFrom"], addr.as_str(), "{j}");
    }
    let thread = req(
        d.port,
        "GET",
        &format!("/cli/thread?token={}&addr={}&limit=0", enc(OWNER), enc(&new_name)),
        None,
    );
    assert_eq!(thread.status, 200, "{}", thread.body);
    let bodies: Vec<String> = json(&thread.body)["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| i["doc"]["body"].as_str().expect("body").to_string())
        .collect();
    assert_eq!(bodies, ["before", "via-old-name", "via-ordinal"]);

    // Q7/TR11: a new sidecar may not take the retired name; the renamed
    // sidecar's brief stays its own.
    let brief_path = ws.path.join(".k2/sidecars/reviewer/BRIEF.md");
    let brief_before = std::fs::read_to_string(&brief_path).expect("brief");
    let refused = new_sidecar(d.port, OWNER, &ws, "reviewer", "claude", Some("# stranger\n"));
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(error_code(&refused), "name_reserved");
    assert!(refused.body.contains("k2 sidecar new critic"), "{}", refused.body);
    assert_eq!(std::fs::read_to_string(&brief_path).expect("brief"), brief_before);
}
