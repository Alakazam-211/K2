//! prd-daemon-activity-and-thread-working-v1 S6: the Thread working
//! strip, daemon side, end to end through the real dispatcher.
//!
//! - T-S6a: a compose post → `delivering` → bound through the transcript
//!   `queued_command` (A22) → `tool` ("Running `cargo test`") →
//!   `thinking` 1.5 s after the tool result → subagent counts → the agent's
//!   Thread reply ends it `reply`. Nothing but the two messages is written
//!   to the Thread store.
//! - T-S6b: a nested `claude -p` (a foreign pid) stamped prompt never binds.
//! - T-S6d: the catch-up route returns the live turn mid-turn, `null`
//!   after; app passes 403; POST 405.
//! - T-S6e: an app guest on the same conversation receives zero activity
//!   frames.
//! - T-S6f: a second compose supersedes the first turn.
//! - Q7: an early reply while the lead keeps working brings the strip back
//!   3 s later with the same `startedAt`.
//! - Q8 / T-S6h: a turn that ends with no reply (the lead's Stop on the
//!   running turn the inject was folded into) sends its end and leaves no
//!   trace.
//! - T-S6j: 1,500 activity frames on one conversation close no Thread
//!   subscriber.
//!
//! ISOLATION: temp HOME, an `exec cat` shim as `claude` (no agent CLI ever
//! starts), synthetic transcript lines and hook payloads only.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use k2_core::db::schema::WorkspaceSession;
use k2_core::session::SessionId;
use k2_daemon::{activity_store, activity_transcript, overlay_ws, test_harness, thread_activity, v2_session_map};
use rusqlite::params;
use serde_json::Value as J;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

const OWNER_TOKEN: &str = "owner-token-thread-activity";

static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> J {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", self.body))
    }
}

fn raw_request(port: u16, head: &str, body: &[u8]) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(20))).expect("timeout");
    s.write_all(head.as_bytes()).expect("write head");
    s.write_all(body).expect("write body");
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

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> Resp {
    let b = body.unwrap_or("");
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        b.len()
    );
    raw_request(port, &head, b.as_bytes())
}

fn post_ok(port: u16, token: &str, route: &str, body: J) -> J {
    let r = http(port, "POST", &format!("{route}?token={token}"), Some(&body.to_string()));
    assert_eq!(r.status, 200, "{route}: {}", r.body);
    let v = r.json();
    assert_eq!(v["ok"], true, "{route}: {}", r.body);
    v
}

/// One synthetic hook payload, as `notify.sh` would forward it.
fn hook(port: u16, pane: &str, pid: i32, body: J) {
    let b = body.to_string();
    let head = format!(
        "POST /hook/event HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {OWNER_TOKEN}\r\n\
         X-K2-Pane: {pane}\r\nX-K2-Agent-Pid: {pid}\r\nX-K2-Hook-Source: claude\r\nX-K2-Hook-Version: 2\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        b.len()
    );
    let r = raw_request(port, &head, b.as_bytes());
    assert_eq!(r.status, 204, "hook/event: {}", r.body);
}

fn catch_up(port: u16, token: &str, addr: &str) -> Resp {
    http(port, "GET", &format!("/cli/thread/activity?token={token}&addr={addr}"), None)
}

fn thread_len(conv: &str) -> usize {
    k2_core::overlay::read_thread(conv, 0).expect("read thread").len()
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Serialized; temp HOME + `exec cat` shim as `claude`; refuses to run
/// where production is reachable.
fn with_temp_home<F: FnOnce(&Path)>(f: F) {
    struct Restore {
        _g: std::sync::MutexGuard<'static, ()>,
        prev_home: Option<std::ffi::OsString>,
        prev_shim: Option<std::ffi::OsString>,
        tmp: PathBuf,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            thread_activity::clear_for_tests();
            match self.prev_home.take() {
                Some(p) => std::env::set_var("HOME", p),
                None => std::env::remove_var("HOME"),
            }
            match self.prev_shim.take() {
                Some(p) => std::env::set_var("K2_TEST_AGENT_SHIM_DIR", p),
                None => std::env::remove_var("K2_TEST_AGENT_SHIM_DIR"),
            }
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
    let g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    let tmp = std::env::temp_dir().join(format!("k2-thread-act-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    let shim_dir = tmp.join("shim");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    let shim = shim_dir.join("claude");
    std::fs::write(&shim, "#!/bin/sh\nexec cat\n").expect("shim");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let tmp = std::fs::canonicalize(&tmp).expect("canonical tmp");
    let _restore = Restore {
        _g: g,
        prev_home: std::env::var_os("HOME"),
        prev_shim: std::env::var_os("K2_TEST_AGENT_SHIM_DIR"),
        tmp: tmp.clone(),
    };
    std::env::set_var("HOME", &tmp);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", tmp.join("shim"));
    k2_core::test_isolation::assert_isolated_from_prod();
    f(&tmp);
}

/// A workspace (a real folder under HOME) with a pinned Chat.
struct Room {
    handle: String,
    project_id: String,
    path: PathBuf,
}

fn seed_room(home: &Path, handle: &str) -> Room {
    let path = home.join("ws").join(handle);
    std::fs::create_dir_all(&path).expect("workspace dir");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, path.to_string_lossy()],
    )
    .expect("seed project");
    let conv = uuid::Uuid::new_v4().to_string();
    WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &id, None, Some(&conv), "claude", "system", "running")
        .expect("pin");
    Room { handle: handle.to_string(), project_id: id, path }
}

/// The room's agent, after a warm-up compose woke its pinned Chat (the
/// shim): its conversation, v2 session id and PTY child pid (the hook
/// sender the owner check accepts).
struct Agent {
    conv: String,
    sid: String,
    pid: i32,
}

fn warm_up(port: u16, room: &Room) -> Agent {
    post_ok(port, OWNER_TOKEN, "/cli/thread/post", serde_json::json!({
        "addr": room.handle, "text": "warm-up", "via": "compose"
    }));
    let r = http(port, "GET", &format!("/cli/thread?token={OWNER_TOKEN}&addr={}&limit=1", room.handle), None);
    assert_eq!(r.status, 200, "{}", r.body);
    let conv = r.json()["conversation_id"].as_str().expect("conversation_id").to_string();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let sid = loop {
        let found = activity_store::rows_json()
            .into_iter()
            .find(|row| row["projectId"] == room.project_id.as_str() && row["harness"] == "claude");
        if let Some(row) = found {
            break row["sessionId"].as_str().expect("sessionId").to_string();
        }
        assert!(std::time::Instant::now() < deadline, "the pinned Chat never registered a row");
        std::thread::sleep(Duration::from_millis(50));
    };
    let pid = v2_session_map::lookup_by_session_id(&SessionId::parse(&sid).expect("session id"))
        .expect("live session")
        .child_pid()
        .expect("child pid");
    // The warm-up's own turn is not under test.
    thread_activity::clear_for_tests();
    Agent { conv, sid, pid }
}

/// The agent's Claude transcript (`~/.claude/projects/<slug>/<conv>.jsonl`),
/// created now and followed once the owner's first hook names it.
fn transcript(home: &Path, room: &Room, agent: &Agent) -> PathBuf {
    let dir = home
        .join(".claude")
        .join("projects")
        .join(k2_core::chat_history::claude_project_hash(&room.path.to_string_lossy()));
    std::fs::create_dir_all(&dir).expect("claude project dir");
    let path = dir.join(format!("{}.jsonl", agent.conv));
    append(&path, &[r#"{"type":"user","timestamp":"2026-09-21T14:13:20.000Z","message":{"role":"user","content":"Synthetic earlier prompt."}}"#]);
    path
}

fn append(path: &Path, lines: &[&str]) {
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("open transcript");
    for l in lines {
        writeln!(f, "{l}").expect("append");
    }
}

fn queued_line(addr: &str, text: &str) -> String {
    serde_json::json!({
        "type": "attachment",
        "timestamp": "2026-09-21T14:13:30.000Z",
        "attachment": { "type": "queued_command", "prompt": format!("[from owner] [thread:{addr}] {text}") }
    })
    .to_string()
}

/// The owner's prompt opens a turn and names the conversation, so the
/// follower resolves the transcript (A18: resolve on claim).
fn owner_prompt(port: u16, agent: &Agent, prompt_id: &str) {
    hook(port, &agent.sid, agent.pid, serde_json::json!({
        "hook_event_name": "UserPromptSubmit", "session_id": agent.conv,
        "prompt_id": prompt_id, "prompt": "Synthetic earlier prompt."
    }));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while activity_transcript::conversation_of(&agent.sid).map(|(c, _)| c) != Some(agent.conv.clone()) {
        assert!(std::time::Instant::now() < deadline, "the follower never named the conversation");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn owner_stop(port: u16, agent: &Agent, prompt_id: &str) {
    hook(port, &agent.sid, agent.pid, serde_json::json!({
        "hook_event_name": "Stop", "session_id": agent.conv, "prompt_id": prompt_id,
        "stop_hook_active": false, "background_tasks": [], "session_crons": []
    }));
}

async fn open_ws(port: u16, conv: &str, token: &str) -> Ws {
    let url = format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={conv}&token={token}");
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .unwrap_or_else(|e| panic!("overlay WS must open for {conv}: {e}"));
    ws
}

/// The next Text frame within `ms` (Pings skipped), or `None`.
async fn next_frame_within(ws: &mut Ws, ms: u64) -> Option<J> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        match timeout(left, ws.next()).await {
            Err(_) => return None,
            Ok(Some(Ok(Message::Text(t)))) => {
                return Some(serde_json::from_str(&t).unwrap_or_else(|e| panic!("frame JSON ({e}): {t}")))
            }
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
            Ok(other) => panic!("overlay WS ended or failed: {other:?}"),
        }
    }
}

/// The first `activity` frame for `turn` that satisfies `want`, within
/// 6 s. Thread frames and other turns' frames are skipped.
async fn activity_until(ws: &mut Ws, turn: &str, what: &str, want: impl Fn(&J) -> bool) -> J {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let f = next_frame_within(ws, left.as_millis() as u64)
            .await
            .unwrap_or_else(|| panic!("timed out waiting for {what}; activity frames seen: {seen:#?}"));
        if f["collection"] != "activity" || f["id"] != turn {
            continue;
        }
        assert!(f.get("doc").is_none(), "activity frames carry no doc: {f}");
        assert_eq!(f["seq"], 0, "activity frames have no Thread seq: {f}");
        let a = f["activity"].clone();
        assert_eq!(a["turnId"], turn, "{a}");
        if want(&a) {
            return a;
        }
        seen.push(a);
    }
}

/// No activity frame for `turn` within `ms`.
async fn no_activity(ws: &mut Ws, turn: &str, ms: u64, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Some(f) = next_frame_within(ws, left.as_millis() as u64).await else { return };
        assert!(
            !(f["collection"] == "activity" && f["id"] == turn),
            "{what}: unexpected activity frame {f}"
        );
    }
}

fn mint_guest(port: u16, name: &str, room: &str) -> String {
    let r = http(port, "POST", &format!("/cli/skin/users?token={OWNER_TOKEN}"), Some(&format!(r#"{{"username":"{name}"}}"#)));
    assert_eq!(r.status, 200, "skin user add: {}", r.body);
    let r = http(
        port,
        "POST",
        &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"name":"{name}","caps":["thread:read","thread:post","activity:read"],"rooms":["{room}"]}}"#)),
    );
    assert_eq!(r.status, 200, "mint: {}", r.body);
    let tok = r.json()["token"].as_str().expect("token").to_string();
    assert!(tok.starts_with("k2skn_"), "{tok}");
    tok
}

/// T-S6a + T-S6d + T-S6e.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_compose_turn_streams_its_steps_and_ends_on_the_reply() {
    with_temp_home(|home| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let tag = &uuid::Uuid::new_v4().to_string()[..8];
        let room = seed_room(home, &format!("tact{tag}"));
        let agent = warm_up(port, &room);
        let guest = mint_guest(port, &format!("guest{tag}"), &room.handle);
        let file = transcript(home, &room, &agent);
        // The agent is mid-turn on an earlier prompt.
        owner_prompt(port, &agent, "p0");

        futures_block(async {
            let mut a = open_ws(port, &agent.conv, OWNER_TOKEN).await;
            let mut g = open_ws(port, &agent.conv, &guest).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            let before = thread_len(&agent.conv);

            let sent_at = now_ms();
            let compose = post_ok(port, OWNER_TOKEN, "/cli/thread/post", serde_json::json!({
                "addr": room.handle, "text": "run the tests", "via": "compose"
            }));
            let turn = compose["id"].as_str().expect("id").to_string();
            let first = activity_until(&mut a, &turn, "the delivering frame", |_| true).await;
            assert_eq!(first["phase"], "delivering", "{first}");
            assert_eq!(first["state"], "working");
            assert_eq!(first["end"], J::Null);
            let started = first["startedAt"].as_i64().expect("startedAt");
            assert!(started >= sent_at - 1_000 && started <= now_ms(), "startedAt is the post's receipt time: {first}");
            assert_eq!(first["since"], started);
            assert!(first["serverNow"].as_i64().is_some() && first["rev"].as_u64().is_some(), "{first}");

            // T-S6d: the catch-up mid-turn is the live turn.
            let r = catch_up(port, OWNER_TOKEN, &room.handle);
            assert_eq!(r.status, 200, "{}", r.body);
            let caught = r.json();
            assert_eq!(caught["conversation_id"], agent.conv.as_str());
            assert_eq!(caught["turn"]["turnId"], turn.as_str(), "{caught}");
            assert_eq!(caught["turn"]["startedAt"], started);
            assert_eq!(caught["turn"]["end"], J::Null);

            // A22: the inject reaches Claude mid-turn as a queued command.
            append(&file, &[&queued_line(&room.handle, "run the tests")]);
            tokio::time::sleep(Duration::from_millis(600)).await;

            hook(port, &agent.sid, agent.pid, serde_json::json!({
                "hook_event_name": "PreToolUse", "session_id": agent.conv, "prompt_id": "p0",
                "tool_name": "Bash", "tool_use_id": "toolu_s6a", "tool_input": {"command": "cargo test\nsecond line"}
            }));
            let tool = activity_until(&mut a, &turn, "the tool step", |a| a["phase"] == "tool").await;
            assert_eq!(tool["line"], "Running `cargo test`", "{tool}");
            assert_eq!(tool["tally"]["cmd"], 1, "{tool}");
            assert_eq!(tool["state"], "working");

            let post_at = now_ms();
            hook(port, &agent.sid, agent.pid, serde_json::json!({
                "hook_event_name": "PostToolUse", "session_id": agent.conv, "prompt_id": "p0",
                "tool_name": "Bash", "tool_use_id": "toolu_s6a", "tool_response": {"stdout": "synthetic"}
            }));
            let thinking = activity_until(&mut a, &turn, "thinking after 1.5 s", |a| a["phase"] == "thinking").await;
            let since = thinking["phaseSince"].as_i64().expect("phaseSince");
            assert!(since >= post_at - 50, "thinking counts from the tool result: {thinking}");
            assert!(thinking["serverNow"].as_i64().expect("serverNow") >= since + 1_400, "{thinking}");
            assert_eq!(thinking["line"], J::Null);

            // TW7: subagent counts, and the ones done in this turn.
            hook(port, &agent.sid, agent.pid, serde_json::json!({
                "hook_event_name": "SubagentStart", "session_id": agent.conv, "prompt_id": "p0",
                "agent_id": "agent-s6a", "agent_type": "general-purpose"
            }));
            activity_until(&mut a, &turn, "a running subagent", |a| a["subagents"] == 1).await;
            hook(port, &agent.sid, agent.pid, serde_json::json!({
                "hook_event_name": "SubagentStop", "session_id": agent.conv, "prompt_id": "p0",
                "agent_id": "agent-s6a", "agent_type": "general-purpose", "background_tasks": []
            }));
            let done = activity_until(&mut a, &turn, "the subagent done", |a| a["subagentsDone"] == 1).await;
            assert_eq!(done["subagents"], 0, "{done}");

            // The agent answers in Thread: the strip ends with the reply.
            let reply = post_ok(port, OWNER_TOKEN, "/cli/thread/post", serde_json::json!({
                "addr": room.handle, "text": "Fixed 2 failing tests.", "from": room.handle
            }));
            assert_eq!(reply["via"], "thread");
            let end = activity_until(&mut a, &turn, "the reply end", |a| !a["end"].is_null()).await;
            assert_eq!(end["end"]["reason"], "reply", "{end}");
            assert_eq!(end["state"], "idle");
            assert!(catch_up(port, OWNER_TOKEN, &room.handle).json()["turn"].is_null(), "hidden after the reply");

            // The lead finishes inside the Q7 window: nothing comes back.
            owner_stop(port, &agent, "p0");
            no_activity(&mut a, &turn, 3_600, "after the turn finished").await;
            assert!(catch_up(port, OWNER_TOKEN, &room.handle).json()["turn"].is_null());

            // Only the two messages were stored (TW5: activity never is).
            assert_eq!(thread_len(&agent.conv), before + 2, "activity frames must not be stored");

            // T-S6e: the app guest saw the Thread, never the strip.
            let mut thread_ids = Vec::new();
            while let Some(f) = next_frame_within(&mut g, 300).await {
                assert_ne!(f["collection"], "activity", "an app guest must never get activity frames: {f}");
                if f["collection"] == "thread" {
                    thread_ids.push(f["id"].as_str().unwrap_or_default().to_string());
                }
            }
            assert!(thread_ids.contains(&turn), "the guest socket was live on this conversation: {thread_ids:?}");
            assert!(thread_ids.iter().any(|id| reply["id"] == id.as_str()), "{thread_ids:?}");

            // The catch-up route: app passes 403, POST 405, bad addrs.
            let r = catch_up(port, &guest, &room.handle);
            assert_eq!(r.status, 403, "app pass: {}", r.body);
            let r = http(port, "POST", &format!("/cli/thread/activity?token={OWNER_TOKEN}&addr={}", room.handle), Some("{}"));
            assert_eq!(r.status, 405, "POST: {}", r.body);
            let r = http(port, "GET", &format!("/cli/thread/activity?token={OWNER_TOKEN}"), None);
            assert_eq!(r.status, 400, "missing addr: {}", r.body);
            let r = catch_up(port, OWNER_TOKEN, &format!("nope{tag}"));
            assert_eq!(r.status, 404, "unknown addr: {}", r.body);
            let r = http(port, "GET", &format!("/cli/thread/activity?addr={}", room.handle), None);
            assert_eq!(r.status, 403, "no token: {}", r.body);
        });
    });
}

/// Q7, T-S6f, Q8 + T-S6h, T-S6b.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn early_replies_supersedes_silent_ends_and_foreign_stamps() {
    with_temp_home(|home| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let tag = &uuid::Uuid::new_v4().to_string()[..8];
        let room = seed_room(home, &format!("tq{tag}"));
        let agent = warm_up(port, &room);
        let file = transcript(home, &room, &agent);
        owner_prompt(port, &agent, "q7");

        futures_block(async {
            let mut a = open_ws(port, &agent.conv, OWNER_TOKEN).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            let compose = |text: &str| -> String {
                post_ok(port, OWNER_TOKEN, "/cli/thread/post", serde_json::json!({
                    "addr": room.handle, "text": text, "via": "compose"
                }))["id"]
                    .as_str()
                    .expect("id")
                    .to_string()
            };

            // Q7: "on it", then the lead keeps working.
            let turn = compose("can you check the build");
            let first = activity_until(&mut a, &turn, "the Q7 turn", |_| true).await;
            let started = first["startedAt"].as_i64().expect("startedAt");
            append(&file, &[&queued_line(&room.handle, "can you check the build")]);
            tokio::time::sleep(Duration::from_millis(500)).await;
            post_ok(port, OWNER_TOKEN, "/cli/thread/post", serde_json::json!({
                "addr": room.handle, "text": "On it.", "from": room.handle
            }));
            let end = activity_until(&mut a, &turn, "the early reply", |a| !a["end"].is_null()).await;
            assert_eq!(end["end"]["reason"], "reply");
            let replied_at = now_ms();
            let back = activity_until(&mut a, &turn, "the strip coming back", |a| a["end"].is_null()).await;
            assert!(now_ms() - replied_at >= 2_800, "back after about 3 s, not sooner");
            assert_eq!(back["startedAt"], started, "the same startedAt: {back}");
            assert_eq!(catch_up(port, OWNER_TOKEN, &room.handle).json()["turn"]["turnId"], turn.as_str());
            owner_stop(port, &agent, "q7");
            let end = activity_until(&mut a, &turn, "the done end", |a| !a["end"].is_null()).await;
            assert_eq!(end["end"]["reason"], "done", "{end}");
            assert_eq!(end["end"]["detail"], J::Null, "bound, not unbound: {end}");

            // T-S6f: a second compose supersedes the first.
            hook(port, &agent.sid, agent.pid, serde_json::json!({
                "hook_event_name": "UserPromptSubmit", "session_id": agent.conv,
                "prompt_id": "q8", "prompt": "Synthetic prompt."
            }));
            let old = compose("first thought");
            let new = compose("actually, run the linter");
            let gone = activity_until(&mut a, &old, "the superseded end", |a| !a["end"].is_null()).await;
            assert_eq!(gone["end"]["reason"], "superseded");
            assert_eq!(gone["state"], "stopped");

            // Q8 + T-S6h: folded into the running turn, ended by its Stop,
            // no reply: an end frame and then nothing.
            let before = thread_len(&agent.conv);
            append(&file, &[&queued_line(&room.handle, "actually, run the linter")]);
            tokio::time::sleep(Duration::from_millis(500)).await;
            owner_stop(port, &agent, "q8");
            let end = activity_until(&mut a, &new, "the silent end", |a| !a["end"].is_null()).await;
            assert_eq!(end["end"]["reason"], "done", "{end}");
            assert_eq!(end["end"]["detail"], J::Null, "bound by the queued command: {end}");
            assert!(catch_up(port, OWNER_TOKEN, &room.handle).json()["turn"].is_null());
            no_activity(&mut a, &new, 3_500, "after a silent end").await;
            assert_eq!(thread_len(&agent.conv), before, "Q8: no trace in the Thread");

            // T-S6b: a nested `claude -p` (not the pane's owner) with a
            // stamped prompt never binds: the turn stays unbound and ends
            // only as `unbound_turn_end`.
            hook(port, &agent.sid, agent.pid, serde_json::json!({
                "hook_event_name": "UserPromptSubmit", "session_id": agent.conv,
                "prompt_id": "b0", "prompt": "Synthetic prompt."
            }));
            let turn = compose("ping");
            activity_until(&mut a, &turn, "the T-S6b turn", |_| true).await;
            hook(port, &agent.sid, 1, serde_json::json!({
                "hook_event_name": "UserPromptSubmit", "session_id": "nested-run",
                "prompt_id": "nested", "prompt": format!("[from owner] [thread:{}] ping", room.handle)
            }));
            let status = k2_daemon::hook_ingest::status_json();
            assert!(status["sessions"][agent.sid.as_str()]["counters"]["foreign"].as_u64().unwrap_or(0) >= 1, "{status}");
            tokio::time::sleep(Duration::from_millis(2_200)).await;
            let stopped_at = now_ms();
            owner_stop(port, &agent, "b0");
            let end = activity_until(&mut a, &turn, "the unbound end", |a| !a["end"].is_null()).await;
            assert_eq!(end["end"]["reason"], "done", "{end}");
            assert_eq!(end["end"]["detail"], "unbound_turn_end", "a foreign stamp must not bind: {end}");
            assert!(now_ms() - stopped_at >= 1_300, "the unbound end waits out its grace");
        });
    });
}

/// T-S6j: strip traffic never closes a Thread subscriber, on its own
/// conversation or another.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flood_of_activity_frames_closes_no_thread_socket() {
    with_temp_home(|home| {
        let daemon = futures_block(test_harness::start(OWNER_TOKEN));
        let port = daemon.port;
        let tag = &uuid::Uuid::new_v4().to_string()[..8];
        let busy = seed_room(home, &format!("tbusy{tag}"));
        let quiet = seed_room(home, &format!("tquiet{tag}"));
        let busy_conv = {
            let r = http(port, "GET", &format!("/cli/thread?token={OWNER_TOKEN}&addr={}&limit=1", busy.handle), None);
            r.json()["conversation_id"].as_str().expect("conversation").to_string()
        };
        let quiet_conv = {
            let r = http(port, "GET", &format!("/cli/thread?token={OWNER_TOKEN}&addr={}&limit=1", quiet.handle), None);
            r.json()["conversation_id"].as_str().expect("conversation").to_string()
        };
        futures_block(async {
            let mut on_busy = open_ws(port, &busy_conv, OWNER_TOKEN).await;
            let mut on_quiet = open_ws(port, &quiet_conv, OWNER_TOKEN).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            for i in 0..1_500 {
                overlay_ws::publish_activity(overlay_ws::OverlayFrame {
                    collection: "activity".into(),
                    seq: 0,
                    id: "flood-turn".into(),
                    doc: None,
                    activity: Some(serde_json::json!({ "turnId": "flood-turn", "rev": i })),
                    conversation_id: Some(busy_conv.clone()),
                });
            }
            // Both sockets still deliver the next Thread message.
            for (ws, room, conv) in [(&mut on_busy, &busy, &busy_conv), (&mut on_quiet, &quiet, &quiet_conv)] {
                let posted = post_ok(port, OWNER_TOKEN, "/cli/thread/post", serde_json::json!({
                    "addr": room.handle, "text": "still here", "from": room.handle
                }));
                assert_eq!(posted["conversation_id"], conv.as_str());
                let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                loop {
                    let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                    let f = next_frame_within(ws, left.as_millis() as u64)
                        .await
                        .unwrap_or_else(|| panic!("no Thread frame on {conv} after the flood"));
                    if f["collection"] == "thread" && f["id"] == posted["id"] {
                        break;
                    }
                    assert_eq!(f["collection"], "activity", "only the flood comes first: {f}");
                    assert_eq!(conv, &busy_conv, "the quiet conversation never sees the busy one's strip: {f}");
                }
            }
        });
    });
}
