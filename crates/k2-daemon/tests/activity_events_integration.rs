//! prd-daemon-activity-and-thread-working-v1 S4: `activity_changed`, the
//! snapshot route, lag closes, compat events and the server readers.
//!
//! - T-S4a: `seq` is gap-free across 10,000 rapid changes from 8
//!   sessions, and each row's last state goes out (coalesced).
//! - T-S4b / T-S4j: a lagged `/cli/sessions/events` subscriber is closed
//!   with 4008; after it reconnects, the snapshot equals the store.
//! - T-S4d: removing a session whose lead was working emits
//!   `agent_status_changed{stop}` and `session_activity_changed{idle}`.
//! - T-S4e / T-S4i: the presence summary, Keep awake and the snapshot
//!   agree for the same rows; Keep awake holds through a 40-minute
//!   `unverifiable` and lets a cron-only row go.
//! - T-S4g: `/v1` busy is `working` or `waiting`, never `monitoring`.
//! - The snapshot route: GET only, owner or login, app passes refused,
//!   `?workspace=` narrows it. Q14: the daemon touches Active on lead →
//!   working.
//!
//! No agent CLI runs: the PTYs are `cat`, the hook envelopes synthetic.
//! ISOLATION: `$HOME`, the shared DB, the v2 map, the activity store and
//! the session-events bus are process-wide, so every test here holds one
//! lock.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use k2_core::activity::Evidence;
use k2_core::agent_hooks::envelope::{self, HookEnvelope, HookHeaders, HookSource};
use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession};
use k2_daemon::activity_store::{self, SessionFacts};
use k2_daemon::session_events::{self, SessionEvent};
use k2_daemon::{activity_events, events, test_harness, v2_session_map};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

const OWNER_TOKEN: &str = "owner-token-activity-events-s4";

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    let g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    k2_core::test_isolation::assert_no_prod_env();
    g
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "k2-actev-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos()
    ));
    std::fs::create_dir_all(&p).expect("tmp dir");
    p
}

/// Temp `HOME` + in-memory DB + empty v2 map and store, restored on drop.
struct TempHome {
    prev: Option<std::ffi::OsString>,
    dir: PathBuf,
}

impl TempHome {
    fn new() -> Self {
        let dir = tmp_dir("home");
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", &dir);
        let _ = k2_core::db::init_for_tests();
        v2_session_map::clear_for_tests();
        activity_store::clear_for_tests();
        Self { prev, dir }
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        v2_session_map::clear_for_tests();
        activity_store::clear_for_tests();
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn seed_project(id: &str, path: &Path, last_interaction_at: i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects \
         (id, path, name, color, agent_mode, pinned, tab_order, manually_active, last_interaction_at) \
         VALUES (?1, ?2, ?3, '#123456', 'off', 0, 0, 0, ?4)",
        rusqlite::params![id, path.to_string_lossy().as_ref(), format!("act-{id}"), last_interaction_at],
    )
    .expect("seed project");
}

fn last_interaction_at(id: &str) -> Option<i64> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT last_interaction_at FROM projects WHERE id = ?1", rusqlite::params![id], |r| {
        r.get::<_, Option<i64>>(0)
    })
    .expect("project row")
}

fn hook(pane: &str, body: &str) -> HookEnvelope {
    envelope::parse(
        &HookHeaders {
            pane: pane.to_string(),
            agent_pid: Some(4242),
            source: HookSource::Claude,
            hook_version: Some(2),
            cli_version: Some("2.1.292".into()),
            truncated: false,
            event_hint: None,
        },
        body.as_bytes(),
    )
    .expect("synthetic hook parses")
}

fn apply(sid: &str, body: &str, at: i64) {
    activity_store::apply_at(sid, Evidence::Hook(&hook(sid, body)), at);
}

fn prompt(sid: &str, p: &str, at: i64) {
    apply(sid, &format!(r#"{{"hook_event_name":"UserPromptSubmit","prompt_id":"{p}"}}"#), at);
}

fn stop(sid: &str, p: &str, tasks: &str, crons: &str, at: i64) {
    apply(
        sid,
        &format!(
            r#"{{"hook_event_name":"Stop","prompt_id":"{p}","background_tasks":{tasks},"session_crons":{crons}}}"#
        ),
        at,
    );
}

fn register_row(sid: &str, agent: &str, cwd: &Path) {
    activity_store::register(SessionFacts {
        session_id: sid.to_string(),
        agent_name: agent.to_string(),
        cwd: Some(cwd.to_string_lossy().into_owned()),
        program: Some("claude".into()),
    });
}

fn spawn_cat(cwd: &Path, key: &str) -> Arc<DaemonPtySession> {
    let cfg = DaemonPtyConfig {
        cols: 80,
        rows: 24,
        cwd: Some(cwd.to_path_buf()),
        program: Some("cat".to_string()),
        ..DaemonPtyConfig::default()
    };
    let session = DaemonPtySession::spawn(cfg).expect("spawn cat PTY");
    v2_session_map::register(key.to_string(), Arc::clone(&session));
    session
}

/// Every `activity_changed` frame (and the compat frames) a bus
/// subscriber sees, collected on a plain thread so the bus never lags it.
struct BusTap {
    frames: Arc<StdMutex<Vec<SessionEvent>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl BusTap {
    fn start() -> Self {
        let frames = Arc::new(StdMutex::new(Vec::new()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut rx = session_events::subscribe();
        let (f, s) = (Arc::clone(&frames), Arc::clone(&stop));
        std::thread::spawn(move || {
            use tokio::sync::broadcast::error::TryRecvError;
            while !s.load(std::sync::atomic::Ordering::Relaxed) {
                match rx.try_recv() {
                    Ok(ev) => {
                        if matches!(
                            ev,
                            SessionEvent::ActivityChanged { .. }
                                | SessionEvent::SessionActivityChanged { .. }
                                | SessionEvent::AgentStatusChanged { .. }
                        ) {
                            f.lock().expect("tap").push(ev);
                        }
                    }
                    Err(TryRecvError::Empty) => std::thread::yield_now(),
                    Err(TryRecvError::Lagged(n)) => panic!("the test tap itself lagged {n} frames"),
                    Err(TryRecvError::Closed) => return,
                }
            }
        });
        Self { frames, stop }
    }

    fn snapshot(&self) -> Vec<SessionEvent> {
        self.frames.lock().expect("tap").clone()
    }

    /// Wait until `pred` holds over the frames so far.
    fn wait_for(&self, what: &str, pred: impl Fn(&[SessionEvent]) -> bool) -> Vec<SessionEvent> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let frames = self.snapshot();
            if pred(&frames) {
                return frames;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}; frames: {frames:#?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Wait until no new frame arrived for `quiet`.
    fn settle(&self, quiet: Duration) -> Vec<SessionEvent> {
        let mut last = usize::MAX;
        let mut since = Instant::now();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let n = self.snapshot().len();
            if n != last {
                last = n;
                since = Instant::now();
            } else if since.elapsed() >= quiet {
                return self.snapshot();
            }
            assert!(Instant::now() < deadline, "the bus never went quiet");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for BusTap {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn activity_frames(frames: &[SessionEvent]) -> Vec<(u64, Option<serde_json::Value>, Option<String>, Option<serde_json::Value>)> {
    frames
        .iter()
        .filter_map(|ev| match ev {
            SessionEvent::ActivityChanged { seq, row, removed, workspace, .. } => {
                Some((*seq, row.clone(), removed.clone(), workspace.clone()))
            }
            _ => None,
        })
        .collect()
}

struct NoSink;

impl k2_core::agent_hooks::AgentHookEventSink for NoSink {
    fn emit(&self, _event: k2_core::agent_hooks::HookEvent, _payload: serde_json::Value) {}
}

/// T-S4a.
#[test]
fn seq_is_gap_free_and_every_rows_last_state_goes_out() {
    let _g = lock();
    let _home = TempHome::new();
    activity_events::spawn();
    // No hook sink: another test may have installed the daemon's, and
    // 10,000 compat lifecycle frames would only flood this test's tap.
    k2_core::agent_hooks::set_sink(Box::new(NoSink));
    let dir = tmp_dir("a");
    seed_project("act-a-pid", &dir, 0);
    let sids: Vec<String> = (0..8).map(|_| uuid::Uuid::new_v4().to_string()).collect();
    for (i, sid) in sids.iter().enumerate() {
        register_row(sid, &format!("tab-a{i}"), &dir);
    }
    let tap = BusTap::start();

    // 10,000 changes: each session alternates working / idle.
    let t0 = now_ms();
    for n in 0..10_000usize {
        let sid = &sids[n % sids.len()];
        let turn = n / sids.len();
        let p = format!("p{turn}");
        if turn % 2 == 0 {
            prompt(sid, &p, t0 + n as i64);
        } else {
            stop(sid, &format!("p{}", turn - 1), "[]", "[]", t0 + n as i64);
        }
    }
    let frames = tap.settle(Duration::from_millis(500));
    let act = activity_frames(&frames);
    assert!(!act.is_empty(), "no activity_changed frames");
    for w in act.windows(2) {
        assert_eq!(w[1].0, w[0].0 + 1, "seq gap: {} then {}", w[0].0, w[1].0);
    }
    assert!(
        act.len() < 10_000,
        "the coalescer sent {} frames for 10,000 changes",
        act.len()
    );
    // Each row's LAST frame is its final state in the store.
    for sid in &sids {
        let last = act
            .iter()
            .rev()
            .find_map(|(_, row, _, _)| row.as_ref().filter(|r| r["sessionId"] == sid.as_str()))
            .unwrap_or_else(|| panic!("no frame for {sid}"));
        let store = activity_store::row_json(sid).expect("row");
        assert_eq!(last, &store, "{sid}: the last frame is not the final state");
    }
    // The workspace rollup in the newest frame matches the snapshot's.
    let snap = activity_events::snapshot(None);
    let newest = act.last().expect("frame");
    assert_eq!(newest.0, snap["seq"].as_u64().expect("seq"), "snapshot seq = the last frame's seq");
    let ws = snap["workspaces"]
        .as_array()
        .expect("workspaces")
        .iter()
        .find(|w| w["projectId"] == "act-a-pid")
        .expect("rollup for the workspace");
    assert_eq!(newest.3.as_ref().expect("frame rollup"), ws);
    assert_eq!(ws["counts"]["idle"].as_u64().expect("idle count") + ws["counts"]["working"].as_u64().expect("working"), 8);
    for sid in &sids {
        activity_store::unregister(sid);
    }
}

/// T-S4d (+ RL5 compat from the store, not the title observer).
#[test]
fn removing_a_working_session_closes_out_older_clients() {
    let _g = lock();
    let _home = TempHome::new();
    activity_events::spawn();
    // The daemon's own sink (main.rs installs it at boot), so the store's
    // compat `agent:lifecycle` reaches the bus as `agent_status_changed`.
    let (tx, _rx) = tokio::sync::broadcast::channel::<events::WireEvent>(64);
    k2_core::agent_hooks::set_sink(Box::new(events::DaemonBroadcastSink::new(tx)));
    let dir = tmp_dir("d");
    seed_project("act-d-pid", &dir, 0);
    let sid = uuid::Uuid::new_v4().to_string();
    register_row(&sid, "tab-d", &dir);
    let tap = BusTap::start();

    prompt(&sid, "p1", now_ms());
    tap.wait_for("the working frame", |f| {
        activity_frames(f).iter().any(|(_, row, _, _)| {
            row.as_ref().is_some_and(|r| r["sessionId"] == sid.as_str() && r["display"] == "working")
        })
    });
    activity_store::unregister(&sid);
    let frames = tap.wait_for("the removal frames", |f| {
        let removed = activity_frames(f).iter().any(|(_, _, rm, _)| rm.as_deref() == Some(sid.as_str()));
        let idle = f.iter().any(|ev| {
            matches!(ev, SessionEvent::SessionActivityChanged { agent_name, status, .. }
                if agent_name == "tab-d" && status == "idle")
        });
        removed && idle
    });
    let lifecycle: Vec<&str> = frames
        .iter()
        .filter_map(|ev| match ev {
            SessionEvent::AgentStatusChanged { pane_id, status, .. } if *pane_id == sid => Some(status.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(lifecycle, vec!["start", "stop"], "agent_status_changed: {lifecycle:?}");
    let compat: Vec<&str> = frames
        .iter()
        .filter_map(|ev| match ev {
            SessionEvent::SessionActivityChanged { agent_name, status, workspace_path, .. } if agent_name == "tab-d" => {
                assert_eq!(workspace_path, &dir.to_string_lossy(), "compat carries the workspace path");
                Some(status.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(compat, vec!["working", "idle"], "session_activity_changed: {compat:?}");
    // The removal frame carries the workspace's rollup without the row.
    let (_, row, _, ws) = activity_frames(&frames)
        .into_iter()
        .find(|(_, _, rm, _)| rm.as_deref() == Some(sid.as_str()))
        .expect("removal frame");
    assert!(row.is_none());
    let ws = ws.expect("rollup on removal");
    assert_eq!(ws["projectId"], "act-d-pid");
    assert_eq!(ws["display"], "idle");
    assert_eq!(ws["counts"]["working"], 0);
}

// ── HTTP + WS through the in-process daemon ──────────────────────────

async fn http(port: u16, method: &str, path_and_query: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.expect("connect");
    let req = format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\n\r\n");
    stream.write_all(req.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let text = String::from_utf8_lossy(&raw).to_string();
        if let Some((headers, body)) = text.split_once("\r\n\r\n") {
            let clen = headers.lines().find_map(|l| {
                l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())
            });
            if let Some(n) = clen {
                if body.len() >= n {
                    let status = headers
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .and_then(|s| s.parse().ok())
                        .expect("status line");
                    return (status, body[..n].to_string());
                }
            }
        }
        let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut chunk))
            .await
            .expect("response in time")
            .expect("read");
        assert!(n > 0, "connection closed mid-response: {text}");
        raw.extend_from_slice(&chunk[..n]);
    }
}

async fn snapshot(port: u16, query: &str) -> serde_json::Value {
    let (status, body) = http(port, "GET", &format!("/cli/activity/snapshot?token={OWNER_TOKEN}{query}")).await;
    assert_eq!(status, 200, "snapshot: {body}");
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("snapshot JSON ({e}): {body}"))
}

async fn open_events(port: u16) -> (tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, serde_json::Value) {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?path=&token={OWNER_TOKEN}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("ws connect");
    let hello = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("hello in time")
        .expect("hello frame")
        .expect("hello ok");
    let Message::Text(text) = hello else { panic!("hello is text: {hello:?}") };
    let hello: serde_json::Value = serde_json::from_str(&text).expect("hello JSON");
    assert_eq!(hello["kind"], "hello");
    (ws, hello)
}

fn sorted_rows(v: &[serde_json::Value]) -> BTreeMap<String, serde_json::Value> {
    v.iter().map(|r| (r["sessionId"].as_str().expect("sessionId").to_string(), r.clone())).collect()
}

/// T-S4b / T-S4j. A current-thread runtime: while this test floods the
/// bus without yielding, the subscriber's task can't drain, so it lags
/// deterministically.
#[tokio::test(flavor = "current_thread")]
async fn a_lagged_events_subscriber_is_closed_4008_and_the_snapshot_equals_the_store() {
    let _g = lock();
    let _home = TempHome::new();
    let d = test_harness::start(OWNER_TOKEN).await;
    let dir = tmp_dir("b");
    seed_project("act-b-pid", &dir, 0);
    let sid = uuid::Uuid::new_v4().to_string();
    register_row(&sid, "tab-b", &dir);
    prompt(&sid, "p1", now_ms());

    let (mut ws, hello) = open_events(d.port).await;
    assert_eq!(hello["instance_id"].as_str().expect("instance_id").len(), 16);
    // 300+ frames past the bus capacity (256), sent without yielding.
    for _ in 0..(session_events::EVENT_CHANNEL_CAP + 300) {
        let _ = session_events::emit(SessionEvent::TokenUsageChanged {});
    }
    let close = loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("a frame in time")
            .expect("socket open")
            .expect("frame ok");
        if let Message::Close(frame) = msg {
            break frame;
        }
    };
    let frame = close.expect("a close frame with a code");
    assert_eq!(u16::from(frame.code), 4008, "lagged close code; got {frame:?}");
    assert_eq!(u16::from(frame.code), k2_daemon::session_events_ws::LAGGED_CLOSE_CODE);

    // The client reconnects and pulls the snapshot: it equals the store.
    let (mut ws2, hello2) = open_events(d.port).await;
    assert_eq!(hello2["instance_id"], hello["instance_id"], "same daemon, same instance id");
    let snap = snapshot(d.port, "").await;
    assert_eq!(snap["instanceId"], hello["instance_id"]);
    assert_eq!(snap["staleAfterSecs"], 1800);
    assert!(snap["serverNow"].as_i64().expect("serverNow") > 0);
    let rows = snap["rows"].as_array().expect("rows");
    assert_eq!(sorted_rows(rows), sorted_rows(&activity_store::rows_json()));
    assert_eq!(sorted_rows(rows)[&sid]["display"], "working");
    let _ = ws2.send(Message::Close(None)).await;
    activity_store::unregister(&sid);
}

/// The snapshot route's gates and the `?workspace=` filter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn snapshot_route_is_a_get_for_owner_or_login_and_filters_by_workspace() {
    let _g = lock();
    let _home = TempHome::new();
    let d = test_harness::start(OWNER_TOKEN).await;
    let (one, two) = (tmp_dir("s1"), tmp_dir("s2"));
    seed_project("act-s1-pid", &one, 0);
    seed_project("act-s2-pid", &two, 0);
    let (a, b) = (uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string());
    register_row(&a, "tab-s1", &one);
    register_row(&b, "tab-s2", &two);
    prompt(&a, "p1", now_ms());

    let (st, body) = http(d.port, "POST", &format!("/cli/activity/snapshot?token={OWNER_TOKEN}")).await;
    assert_eq!(st, 405, "POST: {body}");
    let (st, body) = http(d.port, "GET", "/cli/activity/snapshot").await;
    assert_eq!(st, 403, "no token: {body}");
    let (st, body) = http(d.port, "GET", "/cli/activity/snapshot?token=k2skn_not-a-login").await;
    assert_eq!(st, 403, "an app pass gets no owner snapshot: {body}");

    let all = snapshot(d.port, "").await;
    let ids: Vec<&str> = all["rows"].as_array().expect("rows").iter().map(|r| r["sessionId"].as_str().expect("id")).collect();
    assert!(ids.contains(&a.as_str()) && ids.contains(&b.as_str()), "{all}");

    for q in ["act-s1-pid".to_string(), one.to_string_lossy().into_owned()] {
        let only = snapshot(d.port, &format!("&workspace={q}")).await;
        let rows = only["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), 1, "{q}: {only}");
        assert_eq!(rows[0]["sessionId"], a.as_str());
        let ws = only["workspaces"].as_array().expect("workspaces");
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0]["projectId"], "act-s1-pid");
        assert_eq!(ws[0]["display"], "working");
    }
    let quiet = snapshot(d.port, "&workspace=act-s2-pid").await;
    assert_eq!(quiet["workspaces"][0]["display"], "idle");
    assert_eq!(quiet["workspaces"][0]["counts"]["idle"], 1);
    let (st, body) = http(d.port, "GET", &format!("/cli/activity/snapshot?token={OWNER_TOKEN}&workspace=no-such-ws")).await;
    assert_eq!(st, 400, "unknown workspace: {body}");
    activity_store::unregister(&a);
    activity_store::unregister(&b);
}

fn summary_activity(v: &serde_json::Value) -> BTreeMap<String, (String, String)> {
    v["agentActivity"]
        .as_array()
        .expect("agentActivity")
        .iter()
        .map(|a| {
            (
                a["workspaceId"].as_str().expect("id").to_string(),
                (a["status"].as_str().expect("status").to_string(), a["display"].as_str().expect("display").to_string()),
            )
        })
        .collect()
}

/// T-S4e / T-S4i / T-S4g / Q14.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presence_keep_awake_v1_busy_and_the_snapshot_agree() {
    let _g = lock();
    let _home = TempHome::new();
    let d = test_harness::start(OWNER_TOKEN).await;
    let now = now_ms();
    let forty_min_ago = now - 40 * 60 * 1000;
    let names = ["work", "mon", "cron", "stale", "idle"];
    let mut sessions = Vec::new();
    for name in names {
        let dir = tmp_dir(name);
        let session = spawn_cat(&dir, &format!("tab-agree-{name}"));
        let cwd = session.cwd.as_ref().expect("cwd").clone();
        // Registered after the PTY: the store resolved no project, the
        // presence fold resolves it at read time (longest prefix).
        seed_project(&format!("agree-{name}"), &cwd, 1);
        sessions.push((name, session.session_id.to_string(), cwd, session));
    }
    let sid = |n: &str| sessions.iter().find(|s| s.0 == n).expect("session").1.clone();

    // Q14: the daemon touches Active on lead → working (no client).
    prompt(&sid("work"), "p1", now);
    prompt(&sid("mon"), "p1", now);
    stop(&sid("mon"), "p1", r#"[{"id":"bash_1","type":"shell","status":"running"}]"#, "[]", now + 1);
    prompt(&sid("cron"), "p1", now);
    stop(&sid("cron"), "p1", "[]", r#"[{"id":"cron-1"}]"#, now + 1);
    prompt(&sid("stale"), "p1", forty_min_ago);
    activity_store::tick_all(now_ms());

    let snap = snapshot(d.port, "").await;
    let rows = sorted_rows(snap["rows"].as_array().expect("rows"));
    let display = |n: &str| rows[&sid(n)]["display"].as_str().expect("display").to_string();
    assert_eq!(display("work"), "working");
    assert_eq!(display("mon"), "monitoring");
    assert_eq!(display("cron"), "monitoring");
    assert_eq!(rows[&sid("cron")]["reason"], "crons_scheduled");
    assert_eq!(display("stale"), "unverifiable");
    assert_eq!(display("idle"), "idle");

    // Presence summary: the old words plus the same display.
    let (st, body) = http(d.port, "GET", &format!("/cli/presence/summary?token={OWNER_TOKEN}")).await;
    assert_eq!(st, 200, "{body}");
    let summary: serde_json::Value = serde_json::from_str(&body).expect("summary JSON");
    let act = summary_activity(&summary);
    let want = |s: &str, d: &str| (s.to_string(), d.to_string());
    assert_eq!(act.get("agree-work"), Some(&want("working", "working")), "{summary}");
    assert_eq!(act.get("agree-mon"), Some(&want("working", "monitoring")));
    assert_eq!(act.get("agree-cron"), Some(&want("working", "monitoring")));
    assert_eq!(act.get("agree-stale"), Some(&want("idle", "unverifiable")));
    assert_eq!(act.get("agree-idle"), None, "idle workspaces stay out");
    for name in names {
        if let Some((_, d)) = act.get(&format!("agree-{name}")) {
            assert_eq!(d, &display(name), "{name}: presence and snapshot disagree");
        }
    }

    // Keep awake re-reads the same rows (A32): working and plain
    // monitoring hold; a cron-only row doesn't; a 40-minute unverifiable
    // still holds (2 h clock from its last evidence).
    let awake: BTreeMap<String, bool> = activity_store::keep_awake_view().into_iter().map(|(s, h, _)| (s, h)).collect();
    assert!(awake[&sid("work")]);
    assert!(awake[&sid("mon")]);
    assert!(!awake[&sid("cron")], "a /loop cron must not hold the Mac awake");
    assert!(awake[&sid("stale")], "unverifiable holds up to 2 h");
    assert!(!awake[&sid("idle")]);

    // `/v1` busy (A31): working refuses, monitoring / unverifiable accept.
    let path = |n: &str| sessions.iter().find(|s| s.0 == n).expect("session").2.to_string_lossy().into_owned();
    assert_eq!(k2_daemon::v1_ws_message::busy_status_for_path(&path("work")), Some("working"));
    assert_eq!(k2_daemon::v1_ws_message::busy_status_for_path(&path("mon")), None);
    assert_eq!(k2_daemon::v1_ws_message::busy_status_for_path(&path("stale")), None);
    assert_eq!(k2_daemon::v1_ws_message::busy_status_for_path(&path("idle")), None);

    // ops/overview reports the same rows in the old words + display.
    let (st, body) = http(d.port, "GET", &format!("/cli/ops/overview?token={OWNER_TOKEN}")).await;
    assert_eq!(st, 200, "{body}");
    let overview: serde_json::Value = serde_json::from_str(&body).expect("overview JSON");
    for (name, word, disp) in [("work", "working", "working"), ("mon", "working", "monitoring"), ("stale", "idle", "unverifiable")] {
        let e = overview
            .as_array()
            .expect("array")
            .iter()
            .find(|e| e["sessionId"] == sid(name).as_str())
            .unwrap_or_else(|| panic!("{name} in overview: {overview}"));
        assert_eq!((e["agentStatus"].as_str(), e["display"].as_str()), (Some(word), Some(disp)), "{name}");
    }
    let idle = overview.as_array().expect("array").iter().find(|e| e["sessionId"] == sid("idle").as_str()).expect("idle");
    assert!(idle["agentStatus"].is_null() && idle["display"].is_null(), "an unconfirmed row says nothing: {idle}");

    for (name, _, _, session) in &sessions {
        session.kill();
        drop(v2_session_map::unregister(&format!("tab-agree-{name}")));
    }
}

/// RL2: a newly registered session is announced (idle, unconfirmed), so
/// clients' rollup counts include it before any evidence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_registered_session_is_announced_before_its_first_evidence() {
    let _g = lock();
    let _home = TempHome::new();
    activity_events::spawn();
    let dir = tmp_dir("reg");
    seed_project("act-reg-pid", &dir, 0);
    let tap = BusTap::start();
    let session = spawn_cat(&dir, "tab-act-reg");
    let sid = session.session_id.to_string();
    let frames = tap.wait_for("the registration frame", |f| {
        activity_frames(f).iter().any(|(_, row, _, _)| row.as_ref().is_some_and(|r| r["sessionId"] == sid.as_str()))
    });
    let (_, row, _, ws) = activity_frames(&frames)
        .into_iter()
        .find(|(_, row, _, _)| row.as_ref().is_some_and(|r| r["sessionId"] == sid.as_str()))
        .expect("frame");
    let row = row.expect("row");
    assert_eq!((row["display"].as_str(), row["reason"].as_str()), (Some("idle"), Some("unconfirmed")));
    assert_eq!(row["confirmed"], false);
    let ws = ws.expect("rollup");
    assert_eq!(ws["projectId"], "act-reg-pid");
    assert_eq!(ws["counts"]["idle"], 1);
    // No compat word for a row that never left idle.
    assert!(!frames.iter().any(|ev| matches!(ev, SessionEvent::SessionActivityChanged { agent_name, .. } if agent_name == "tab-act-reg")));
    session.kill();
    drop(v2_session_map::unregister("tab-act-reg"));
}

/// Q14 / RL12: lead → working touches the workspace's Active clock in the
/// daemon itself, debounced.
#[test]
fn lead_working_touches_active_in_the_daemon() {
    let _g = lock();
    let _home = TempHome::new();
    let dir = tmp_dir("q14");
    seed_project("act-q14-pid", &dir, 1);
    let sid = uuid::Uuid::new_v4().to_string();
    register_row(&sid, "tab-q14", &dir);
    assert_eq!(last_interaction_at("act-q14-pid"), Some(1));
    prompt(&sid, "p1", now_ms());
    let touched = last_interaction_at("act-q14-pid").expect("touched");
    assert!(touched > 1, "lead → working touches Active");
    activity_store::unregister(&sid);
}
