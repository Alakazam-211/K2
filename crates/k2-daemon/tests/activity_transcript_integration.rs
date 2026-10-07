//! prd-daemon-activity-and-thread-working-v1 S3: transcript and screen
//! evidence for the activity store (DA27, DA28 / A13, A18, A19).
//!
//! - T-S3d: a Claude session with no hooks runs on its transcript both
//!   ways, and the row says `evidence: transcript`. Its file is adopted
//!   (no premint) and stamped into the tab's row.
//! - T-S3b (daemon): a Codex tab's rollout drives working → idle.
//! - T-S3g: two fresh Codex tabs in one cwd adopt distinct rollouts, and
//!   nothing re-locates per tick (resolve scans are counted).
//! - T-S3f: a Chat view subscriber and an activity follower on the same
//!   file both see every turn.
//! - T-S3h: a Hermes session with its interrupt marker on screen is
//!   working (`evidence: screen`); a cursor-agent session showing prose
//!   ("waiting for") is not.
//!
//! No agent CLI runs: transcripts are synthetic files under a temp HOME,
//! and the PTYs are `exec cat` shims named after the harness.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession};
use k2_daemon::activity_store::{self, SessionFacts};
use k2_daemon::activity_transcript::{self, ConversationSource};
use k2_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

/// Serializes the tests, points `$HOME` (and the agent shim dir) at temp
/// dirs, and refuses to run where production is reachable.
struct Env {
    _g: std::sync::MutexGuard<'static, ()>,
    home: PathBuf,
    prev_home: Option<std::ffi::OsString>,
    prev_shim: Option<std::ffi::OsString>,
}

impl Env {
    fn new(tag: &str) -> Self {
        let g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        k2_core::test_isolation::assert_no_prod_env();
        let home = tmp_dir(&format!("home-{tag}"));
        let prev_home = std::env::var_os("HOME");
        let prev_shim = std::env::var_os("K2_TEST_AGENT_SHIM_DIR");
        std::env::set_var("HOME", &home);
        let shims = home.join("shims");
        std::fs::create_dir_all(&shims).expect("shim dir");
        for name in ["claude", "codex", "hermes", "cursor-agent"] {
            let shim = shims.join(name);
            std::fs::write(&shim, "#!/bin/sh\nexec cat\n").expect("shim");
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shims);
        k2_core::test_isolation::assert_isolated_from_prod();
        let _db = k2_core::db::init_for_tests();
        activity_store::clear_for_tests();
        Self { _g: g, home, prev_home, prev_shim }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        activity_store::clear_for_tests();
        match self.prev_home.take() {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match self.prev_shim.take() {
            Some(v) => std::env::set_var("K2_TEST_AGENT_SHIM_DIR", v),
            None => std::env::remove_var("K2_TEST_AGENT_SHIM_DIR"),
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "k2-act3-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos()
    ));
    std::fs::create_dir_all(&p).expect("tmp dir");
    std::fs::canonicalize(&p).expect("canonical tmp dir")
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn seed_project(id: &str, path: &Path) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name, color, agent_mode, pinned, tab_order) \
         VALUES (?1, ?2, 'transcript-test', '#123456', 'off', 0, 0)",
        rusqlite::params![id, path.to_string_lossy().as_ref()],
    )
    .expect("insert project");
}

fn seed_tab_row(project_id: &str, pane_group_id: &str, cwd: &Path) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = k2_core::db::schema::WorkspaceTabSession {
        project_id: project_id.to_string(),
        pane_group_id: pane_group_id.to_string(),
        agent_name: format!("tab-{pane_group_id}"),
        session_id: None,
        command: Some("claude".into()),
        args_json: None,
        cwd: Some(cwd.to_string_lossy().into_owned()),
        last_seen_at: 0,
        pinned_cols: None,
        pinned_rows: None,
        pinned_set_by: None,
    };
    k2_core::db::schema::WorkspaceTabSession::upsert(&conn, &row).expect("tab row");
}

fn tab_session_id(project_id: &str, pane_group_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceTabSession::get(&conn, project_id, pane_group_id)
        .expect("query tab row")
        .expect("tab row exists")
        .session_id
}

fn register(sid: &str, agent: &str, program: &str, cwd: &Path) {
    activity_store::register(SessionFacts {
        session_id: sid.to_string(),
        agent_name: agent.to_string(),
        cwd: Some(cwd.to_string_lossy().into_owned()),
        program: Some(program.to_string()),
    });
}

fn append(path: &Path, lines: &[&str]) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("open transcript");
    for l in lines {
        writeln!(f, "{l}").expect("append");
    }
}

fn row(sid: &str) -> serde_json::Value {
    activity_store::row_json(sid).expect("row exists")
}

/// Poll every follower until `done(row)` or 3 s pass (the follower reads
/// on its own cadence, so a few passes may be needed).
fn poll_until(sid: &str, what: &str, done: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        activity_transcript::poll_all_once(now_ms());
        let r = row(sid);
        if done(&r) {
            return r;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}: {r}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn claude_dir(home: &Path, cwd: &Path) -> PathBuf {
    let dir = home
        .join(".claude")
        .join("projects")
        .join(k2_core::chat_history::claude_project_hash(&cwd.to_string_lossy()));
    std::fs::create_dir_all(&dir).expect("claude project dir");
    dir
}

fn codex_day_dir(home: &Path) -> PathBuf {
    let day = chrono::Local::now().format("%Y/%m/%d").to_string();
    let dir = home.join(".codex").join("sessions").join(day);
    std::fs::create_dir_all(&dir).expect("codex day dir");
    dir
}

fn codex_rollout(dir: &Path, id: &str, cwd: &Path, at_ms: i64) -> PathBuf {
    let path = dir.join(format!("rollout-synthetic-{id}.jsonl"));
    let ts = chrono::DateTime::from_timestamp_millis(at_ms).expect("ts").to_rfc3339();
    let header = serde_json::json!({
        "timestamp": ts,
        "type": "session_meta",
        "payload": {"id": id, "timestamp": ts, "cwd": cwd.to_string_lossy(), "originator": "codex_cli_rs"}
    });
    append(&path, &[&header.to_string()]);
    path
}

const CLAUDE_TURN: &[&str] = &[
    r#"{"type":"user","timestamp":"2026-09-21T14:13:20.000Z","message":{"role":"user","content":"Synthetic prompt."}}"#,
    r#"{"type":"assistant","timestamp":"2026-09-21T14:13:21.000Z","message":{"id":"m1","role":"assistant","stop_reason":"tool_use","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"cargo build"}}]}}"#,
];
const CLAUDE_END: &[&str] = &[
    r#"{"type":"user","timestamp":"2026-09-21T14:13:25.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"synthetic output"}]}}"#,
    r#"{"type":"assistant","timestamp":"2026-09-21T14:13:26.000Z","message":{"id":"m2","role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"Synthetic answer."}]}}"#,
];

/// T-S3d.
#[test]
fn claude_without_hooks_runs_on_its_adopted_transcript() {
    let env = Env::new("d");
    let ws = tmp_dir("ws-d");
    let pid = format!("act3-d-{}", uuid::Uuid::new_v4());
    seed_project(&pid, &ws);
    seed_tab_row(&pid, "claude-d", &ws);
    let sid = uuid::Uuid::new_v4().to_string();
    register(&sid, "tab-claude-d", "claude", &ws);
    assert_eq!(row(&sid)["reason"], "unconfirmed");

    // A fresh tab (no premint, no hooks): the transcript appears after
    // the spawn and is adopted.
    let conv = uuid::Uuid::new_v4().to_string();
    let path = claude_dir(&env.home, &ws).join(format!("{conv}.jsonl"));
    append(&path, CLAUDE_TURN);
    let r = poll_until(&sid, "working from the transcript", |r| r["display"] == "working");
    assert_eq!(r["evidenceSource"], "transcript");
    assert_eq!(r["lead"]["state"], "working");
    assert_eq!(
        activity_transcript::conversation_of(&sid),
        Some((conv.clone(), ConversationSource::Adopted))
    );
    assert_eq!(tab_session_id(&pid, "claude-d").as_deref(), Some(conv.as_str()), "A18: the adoption is stamped");

    append(&path, CLAUDE_END);
    let r = poll_until(&sid, "idle from end_turn", |r| r["display"] == "idle");
    assert_eq!(r["reason"], "transcript_turn_end");
    assert_eq!(r["evidenceSource"], "transcript");
    // DA27: `k2 hooks status` lists the hookless session by its evidence.
    let status = k2_daemon::hook_ingest::status_json();
    let entry = &status["sessions"][sid.as_str()];
    assert_eq!(entry["evidence"], "transcript", "{status}");
    assert_eq!(entry["conversationId"], conv.as_str());
    activity_store::unregister(&sid);
    assert!(activity_transcript::conversation_of(&sid).is_none(), "unregister stops the follower");
}

/// T-S3g + T-S3b (daemon).
#[test]
fn two_fresh_codex_tabs_adopt_distinct_rollouts_without_relocating() {
    let env = Env::new("g");
    let ws = tmp_dir("ws-g");
    let pid = format!("act3-g-{}", uuid::Uuid::new_v4());
    seed_project(&pid, &ws);
    let (a, b) = (uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string());
    register(&a, "tab-codex-a", "codex", &ws);
    std::thread::sleep(Duration::from_millis(5));
    register(&b, "tab-codex-b", "codex", &ws);

    let day = codex_day_dir(&env.home);
    let t = now_ms();
    let ra = codex_rollout(&day, "conv-codex-a", &ws, t);
    let rb = codex_rollout(&day, "conv-codex-b", &ws, t + 10);
    // Another cwd's rollout is never adopted.
    let other = tmp_dir("ws-other");
    codex_rollout(&day, "conv-codex-other", &other, t + 5);

    activity_transcript::poll_all_once(now_ms());
    let ca = activity_transcript::conversation_of(&a).expect("A adopted");
    let cb = activity_transcript::conversation_of(&b).expect("B adopted");
    assert_eq!(ca, ("conv-codex-a".to_string(), ConversationSource::Adopted));
    assert_eq!(cb, ("conv-codex-b".to_string(), ConversationSource::Adopted));

    // A's rollout drives A only.
    append(&ra, &[
        r#"{"timestamp":"2026-09-21T14:13:21.000Z","type":"event_msg","payload":{"type":"task_started"}}"#,
        r#"{"timestamp":"2026-09-21T14:13:22.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo test\"}","call_id":"c1"}}"#,
    ]);
    let r = poll_until(&a, "A working", |r| r["display"] == "working");
    assert_eq!(r["evidenceSource"], "transcript");
    assert_ne!(row(&b)["display"], "working", "B's row is untouched by A's rollout");
    append(&ra, &[r#"{"timestamp":"2026-09-21T14:13:29.000Z","type":"event_msg","payload":{"type":"task_complete"}}"#]);
    let r = poll_until(&a, "A idle", |r| r["display"] == "idle");
    assert_eq!(r["reason"], "transcript_turn_end");
    append(&rb, &[r#"{"timestamp":"2026-09-21T14:13:21.000Z","type":"event_msg","payload":{"type":"task_started"}}"#]);
    poll_until(&b, "B working", |r| r["display"] == "working");

    // No per-tick re-locate: a resolved follower never scans again.
    let scans = activity_transcript::resolve_scans();
    let mut at = now_ms();
    for _ in 0..15 {
        at += 200;
        activity_transcript::poll_all_once(at);
    }
    assert_eq!(activity_transcript::resolve_scans(), scans, "resolved followers re-located");

    // A miss backs off (5 s, doubling), never per tick.
    let lonely = tmp_dir("ws-lonely");
    let c = uuid::Uuid::new_v4().to_string();
    register(&c, "tab-codex-c", "codex", &lonely);
    let start = now_ms();
    activity_transcript::poll_all_once(start);
    let after_first = activity_transcript::resolve_scans();
    for step in 1..=20 {
        activity_transcript::poll_all_once(start + step * 200);
    }
    assert_eq!(activity_transcript::resolve_scans(), after_first, "a miss re-scanned inside its backoff");
    activity_transcript::poll_all_once(start + 5_000);
    assert_eq!(activity_transcript::resolve_scans(), after_first + 1);
    for id in [&a, &b, &c] {
        activity_store::unregister(id);
    }
}

/// 0.45.0 integration (k2 sidecar × A18): a sidecar tab still waiting for
/// its conversation (`adopt_since` set) is adopted by the sidecar's own
/// watch, which also moves the pane-keyed Chats name. The follower may
/// follow the rollout for activity, but it never stamps that row.
#[test]
fn follower_leaves_a_pending_sidecar_row_to_the_sidecar_adoption() {
    let env = Env::new("s");
    let ws = tmp_dir("ws-s");
    let pid = format!("act3-s-{}", uuid::Uuid::new_v4());
    seed_project(&pid, &ws);
    seed_tab_row(&pid, "codex-s", &ws);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::sidecar::stamp_meta(&conn, &pid, "codex-s", Some("owner"), None, Some(1))
            .expect("stamp sidecar meta");
    }
    let sid = uuid::Uuid::new_v4().to_string();
    register(&sid, "tab-codex-s", "codex", &ws);

    let day = codex_day_dir(&env.home);
    codex_rollout(&day, "conv-codex-s", &ws, now_ms());
    activity_transcript::poll_all_once(now_ms());

    assert_eq!(
        activity_transcript::conversation_of(&sid),
        Some(("conv-codex-s".to_string(), ConversationSource::Adopted)),
        "the follower still follows the rollout for activity"
    );
    assert_eq!(tab_session_id(&pid, "codex-s"), None, "a pending sidecar row is not stamped by the follower");
    let meta = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::sidecar::meta(&conn, &pid, "codex-s").expect("sidecar meta")
    };
    assert_eq!(meta.adopt_since, Some(1), "the sidecar's adoption is still pending");
    activity_store::unregister(&sid);
}

/// T-S3f: the Chat view tail and the activity follower share the file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_view_and_activity_follower_both_see_every_turn() {
    let env = Env::new("f");
    let ws = tmp_dir("ws-f");
    let ws_s = ws.to_string_lossy().into_owned();
    let conv = uuid::Uuid::new_v4().to_string();
    let path = claude_dir(&env.home, &ws).join(format!("{conv}.jsonl"));
    append(&path, &[r#"{"type":"user","uuid":"u0","timestamp":"2026-09-21T14:13:00.000Z","message":{"role":"user","content":"Earlier synthetic prompt."}}"#]);

    let sid = uuid::Uuid::new_v4().to_string();
    register(&sid, "tab-claude-f", "claude", &ws);
    // The owner's hook claim names the conversation (no hooks after that).
    activity_transcript::note_conversation(&sid, &conv);
    let mut signals = activity_transcript::subscribe();
    activity_transcript::poll_all_once(now_ms());
    assert_eq!(activity_transcript::conversation_of(&sid), Some((conv.clone(), ConversationSource::Hook)));

    let mut chat = k2_daemon::chat_overlay_ws::subscribe_chat_tail("claude", &conv, &ws_s).expect("chat tail");
    let mut chat_turns: Vec<String> = Vec::new();
    let mut act_signals = 0usize;
    let turns = 6;
    for i in 0..turns {
        append(&path, &[&format!(
            r#"{{"type":"user","uuid":"u{i}x","timestamp":"2026-09-21T14:14:0{i}.000Z","message":{{"role":"user","content":"Synthetic prompt {i}."}}}}"#
        )]);
        activity_transcript::poll_all_once(now_ms());
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    // The file was born just before the spawn (inside the adoption
    // slack), so the activity follower reads it from its first record:
    // `turns` appended prompts plus the seed one.
    let want_signals = turns + 1;
    while (chat_turns.iter().filter(|id| id.ends_with('x')).count() < turns || act_signals < want_signals)
        && tokio::time::Instant::now() < deadline
    {
        activity_transcript::poll_all_once(now_ms());
        while let Ok(ev) = signals.try_recv() {
            if ev.session_id == sid {
                act_signals += 1;
            }
        }
        if let Ok(Some(frame)) = tokio::time::timeout(Duration::from_millis(100), chat.rx.recv()).await {
            let v: serde_json::Value = serde_json::from_str(&frame).expect("frame json");
            if v["kind"] == "turn" {
                chat_turns.push(v["turn"]["id"].as_str().expect("turn id").to_string());
            }
        }
    }
    let new_chat: Vec<&String> = chat_turns.iter().filter(|id| id.ends_with('x')).collect();
    assert_eq!(new_chat.len(), turns, "chat view saw {chat_turns:?}");
    assert!(chat_turns.contains(&"u0".to_string()), "the Chat view reads from the start");
    assert_eq!(act_signals, want_signals, "the activity follower saw one turn start per record, each once");
    assert_eq!(row(&sid)["evidenceSource"], "transcript");
    drop(chat);
    activity_store::unregister(&sid);
}

fn spawn_pty(program: &str, cwd: &Path, key: &str) -> Arc<DaemonPtySession> {
    let cfg = DaemonPtyConfig {
        cols: 80,
        rows: 24,
        cwd: Some(cwd.to_path_buf()),
        program: Some(program.to_string()),
        ..DaemonPtyConfig::default()
    };
    let session = DaemonPtySession::spawn(cfg).expect("spawn shim PTY");
    v2_session_map::register(key.to_string(), Arc::clone(&session));
    session
}

/// T-S3h.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hermes_marker_is_working_and_prose_is_not() {
    let _env = Env::new("h");
    let ws = tmp_dir("ws-h");
    let hermes = spawn_pty("hermes", &ws, "tab-hermes-h");
    let cursor = spawn_pty("cursor-agent", &ws, "tab-cursor-h");
    let (hs, cs) = (hermes.session_id.to_string(), cursor.session_id.to_string());
    assert_eq!(row(&hs)["harness"], "hermes");
    assert_eq!(row(&cs)["harness"], "cursor");

    // `cat` echoes what it reads: the grid shows the text.
    hermes.write(&b"msg=interrupt . /queue . Ctrl+C cancel\n"[..]);
    cursor.write(&b"I'm waiting for the build to finish.\n"[..]);
    let r = poll_until(&hs, "hermes working from its marker", |r| r["display"] == "working");
    assert_eq!(r["evidenceSource"], "screen");
    // Give the prose the same chances: it never counts.
    for _ in 0..5 {
        activity_transcript::poll_all_once(now_ms() + 1_100);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let c = row(&cs);
    assert_eq!(c["display"], "idle");
    assert_eq!(c["reason"], "unconfirmed", "prose is not evidence: {c}");

    for (key, s) in [("tab-hermes-h", &hermes), ("tab-cursor-h", &cursor)] {
        v2_session_map::unregister(key);
        s.kill();
    }
}
