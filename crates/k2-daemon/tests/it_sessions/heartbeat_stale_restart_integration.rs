//! 0.46.9 — a heartbeat never keeps waking a stuck session (field report
//! 2026-11-05: wakes went into a terminal stuck on the weekly LLM limit).
//!
//! Real fires through `smart_launch`, with the agent replaced by `cat`
//! (`K2SO_WAKE_HEADLESS_TEST_COMMAND` + a shim dir, never a real LLM CLI).
//! Claude's transcripts are read from a temp `CLAUDE_CONFIG_DIR`, where
//! the tests write the `<synthetic>` limit turn the CLI would have.
//!
//! - a live heartbeat session whose last turn was a weekly-limit error is
//!   replaced (killed, new spawn) and the fire record says
//!   "restarted: weekly limit";
//! - a replacement that dies at once is a failed fire, not "fired";
//! - a fire that reuses the live session says "delivered into live
//!   session" and never saves a made-up conversation id;
//! - `restart_session` stops the live session and clears the saved id.

use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;

use k2_core::db::init_for_tests;
use k2_core::db::schema::{AgentHeartbeat, HeartbeatFire};
use k2_core::session::SessionId;

use k2_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

/// This binary's lock, under a stub `$HOME` (never the real one: these tests
/// mint passports into `~/.k2/hook-sessions.json`). The env lock first, then
/// the module lock (`k2_core::test_env::serial`'s order).
fn lock() -> (k2_core::test_env::StubHome, k2_core::test_env::SerialGuard) {
    let home = k2_core::test_env::StubHome::new("heartbeat-stale-restart");
    (home, k2_core::test_env::serial(&TEST_LOCK))
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn setup_project(workspace_id: &str) -> PathBuf {
    let project_path = std::env::temp_dir().join(format!("k2-hb-stale-{workspace_id}-{}-{}", std::process::id(), nanos()));
    let _ = std::fs::remove_dir_all(&project_path);
    std::fs::create_dir_all(&project_path).unwrap();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name, agent_mode, agent_enabled) \
         VALUES (?1, ?2, ?3, 'manager', 1)",
        rusqlite::params![workspace_id, project_path.to_string_lossy().as_ref(), "hb-stale"],
    )
    .unwrap();
    project_path
}

fn write_primary_agent(project: &Path, name: &str) {
    let dir = project.join(".k2so/agents").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("AGENT.md"), format!("---\nname: {name}\ntype: manager\n---\n# {name}\n")).unwrap();
}

fn seed_heartbeat(project: &Path, workspace_id: &str, name: &str) {
    let rel = format!(".k2/heartbeats/{name}/WAKEUP.md");
    let abs = project.join(&rel);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(&abs, "Check the queue and report.\n").unwrap();
    let db = k2_core::db::shared();
    let conn = db.lock();
    AgentHeartbeat::insert(&conn, &format!("{workspace_id}-{name}-id"), workspace_id, name, "hourly", "{}", &rel, true)
        .expect("seed heartbeat");
    AgentHeartbeat::set_use_workspace_session(&conn, workspace_id, name, false).expect("own session lane");
}

fn row(workspace_id: &str, name: &str) -> AgentHeartbeat {
    let db = k2_core::db::shared();
    let conn = db.lock();
    AgentHeartbeat::get_by_name(&conn, workspace_id, name).expect("query").expect("row")
}

fn latest_fire(workspace_id: &str, name: &str) -> HeartbeatFire {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut fires = HeartbeatFire::list_by_schedule_name(&conn, workspace_id, name, 5).expect("fires");
    assert!(!fires.is_empty(), "an audit row must be written");
    fires.remove(0)
}

/// The `<synthetic>` turn Claude Code writes when the token is out.
fn write_limit_transcript(claude_home: &Path, conversation: &str) {
    let dir = claude_home.join("projects").join("-tmp-acme");
    std::fs::create_dir_all(&dir).unwrap();
    let user = serde_json::json!({"type": "user", "message": {"role": "user", "content": "Check the queue and report."}});
    let err = serde_json::json!({
        "type": "assistant", "isApiErrorMessage": true,
        "message": {"model": "<synthetic>", "role": "assistant",
                    "content": [{"type": "text", "text": "You've hit your weekly limit · resets Nov 9, 10am"}]}
    });
    std::fs::write(dir.join(format!("{conversation}.jsonl")), format!("{user}\n{err}\n")).unwrap();
}

/// Shim + env guard. The shim runs `cat`, or exits at once while
/// `<shim_dir>/die` exists (a replacement that fails straight away).
struct Env {
    prior: Vec<(&'static str, Option<std::ffi::OsString>)>,
    dir: PathBuf,
    claude_home: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("k2-hb-stale-shim-{}-{}", std::process::id(), nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let die = dir.join("die");
        k2_core::test_env::write_executable(
            &dir.join("cat"),
            &format!("#!/bin/sh\nif [ -f '{}' ]; then exit 3; fi\nexec cat\n", die.display()),
        );
        let claude_home = dir.join("claude-home");
        std::fs::create_dir_all(claude_home.join("projects")).unwrap();
        let keys = ["K2SO_WAKE_HEADLESS_TEST_COMMAND", "K2_TEST_AGENT_SHIM_DIR", "CLAUDE_CONFIG_DIR"];
        let prior = keys.iter().map(|k| (*k, std::env::var_os(k))).collect();
        std::env::set_var("K2SO_WAKE_HEADLESS_TEST_COMMAND", "cat");
        std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &dir);
        std::env::set_var("CLAUDE_CONFIG_DIR", &claude_home);
        Env { prior, dir, claude_home }
    }

    fn die_on_next_spawn(&self) {
        std::fs::write(self.dir.join("die"), "1").unwrap();
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for (k, v) in self.prior.drain(..) {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn alive(terminal_id: &str) -> bool {
    let sid = SessionId::parse(terminal_id).expect("a session id");
    v2_session_map::lookup_by_session_id(&sid).is_some_and(|s| s.is_child_alive())
}

/// First fire: a fresh session; returns (terminal id, saved conversation).
fn first_fire(project_path: &str, ws: &str, name: &str) -> (String, String) {
    let r = k2_daemon::heartbeat_launch::smart_launch(project_path, name);
    assert_eq!(r["success"], true, "first fire: {r}");
    assert_eq!(r["branch"], "fresh_fire", "{r}");
    assert_eq!(r["outcome"], "delivered", "{r}");
    let t1 = r["terminalId"].as_str().expect("terminalId").to_string();
    let f = latest_fire(ws, name);
    assert_eq!(f.decision, "fired", "{f:?}");
    assert!(f.reason.as_deref().unwrap_or("").contains("delivered (no saved session)"), "{f:?}");
    let conv = row(ws, name).last_session_id.expect("a new spawn saves its conversation");
    (t1, conv)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_session_stuck_on_the_weekly_limit_is_replaced() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let env = Env::new();
    let ws = "hb-stale-ws-limit";
    let project = setup_project(ws);
    write_primary_agent(&project, "manager");
    seed_heartbeat(&project, ws, "default");
    let path = project.to_string_lossy().into_owned();

    let (t1, conv) = first_fire(&path, ws, "default");
    assert!(alive(&t1));

    // The CLI in that session answered the last wake with the limit.
    write_limit_transcript(&env.claude_home, &conv);

    let r = k2_daemon::heartbeat_launch::smart_launch(&path, "default");
    assert_eq!(r["success"], true, "the restart fire succeeds: {r}");
    assert_eq!(r["branch"], "restarted", "{r}");
    assert_eq!(r["outcome"], "restarted: weekly limit", "{r}");
    assert_eq!(r["restartReason"], "weekly_limit", "{r}");
    let t2 = r["terminalId"].as_str().expect("terminalId").to_string();
    assert_ne!(t2, t1, "a new session, never the stuck one");
    assert!(!alive(&t1), "the stuck session was killed");
    assert!(alive(&t2), "the new session runs");
    let key = format!("{ws}:hb:default");
    let under_key = v2_session_map::lookup_by_agent_name(&key).expect("a session under the heartbeat's key");
    assert_eq!(under_key.session_id.to_string(), t2);

    let f = latest_fire(ws, "default");
    assert_eq!(f.decision, "fired", "{f:?}");
    let reason = f.reason.unwrap_or_default();
    assert!(reason.contains("restarted: weekly limit"), "{reason}");
    assert!(reason.contains("weekly-limit error"), "{reason}");
    let hb = row(ws, "default");
    assert_eq!(hb.active_terminal_id.as_deref(), Some(t2.as_str()));
    assert_ne!(hb.last_session_id.as_deref(), Some(conv.as_str()), "the stuck conversation is not kept");

    // The agent can see it in its activity feed.
    let feed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::ActivityFeedEntry::list_by_project(&conn, ws, 20, 0).expect("feed")
    };
    let line = feed
        .iter()
        .find(|e| e.event_type == "heartbeat.restarted")
        .unwrap_or_else(|| panic!("no heartbeat.restarted line: {feed:?}"));
    assert!(line.summary.as_deref().unwrap_or("").contains("restarted: weekly limit"), "{line:?}");

    // The next fire is healthy: it goes into the new session.
    let r = k2_daemon::heartbeat_launch::smart_launch(&path, "default");
    assert_eq!(r["success"], true, "{r}");
    assert_eq!(r["branch"], "injected", "{r}");
    assert_eq!(r["targetSessionId"], t2.as_str(), "{r}");

    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replacement_that_dies_at_once_is_a_failed_fire() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let env = Env::new();
    let ws = "hb-stale-ws-dies";
    let project = setup_project(ws);
    write_primary_agent(&project, "manager");
    seed_heartbeat(&project, ws, "default");
    let path = project.to_string_lossy().into_owned();

    let (t1, conv) = first_fire(&path, ws, "default");
    write_limit_transcript(&env.claude_home, &conv);
    env.die_on_next_spawn();

    let r = k2_daemon::heartbeat_launch::smart_launch(&path, "default");
    assert_eq!(r["success"], false, "never 'fired': {r}");
    assert!(!alive(&t1), "the stuck session was still stopped");
    let f = latest_fire(ws, "default");
    assert_eq!(f.decision, "error", "{f:?}");
    let reason = f.reason.unwrap_or_default();
    assert!(reason.starts_with("restarted: weekly limit"), "the record says it restarted: {reason}");
    assert_eq!(row(ws, "default").consecutive_failures, 1, "it counts as a failure");

    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reuse_is_recorded_truthfully_and_saves_no_made_up_id() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _env = Env::new();
    let ws = "hb-stale-ws-reuse";
    let project = setup_project(ws);
    write_primary_agent(&project, "manager");
    seed_heartbeat(&project, ws, "default");
    let path = project.to_string_lossy().into_owned();

    let (t1, _conv) = first_fire(&path, ws, "default");

    // The field state: the saved id and the stamp are gone, the session
    // still runs under the heartbeat's key.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        AgentHeartbeat::clear_session_id(&conn, ws, "default").unwrap();
        AgentHeartbeat::clear_active_terminal_id(&conn, ws, "default").unwrap();
    }

    let r = k2_daemon::heartbeat_launch::smart_launch(&path, "default");
    assert_eq!(r["success"], true, "{r}");
    assert_eq!(r["branch"], "reused_live", "{r}");
    assert_eq!(r["outcome"], "delivered into live session", "{r}");
    assert_eq!(r["reused"], true, "{r}");
    assert_eq!(r["terminalId"], t1.as_str(), "{r}");
    let f = latest_fire(ws, "default");
    assert_eq!(f.decision, "fired", "{f:?}");
    let reason = f.reason.unwrap_or_default();
    assert!(reason.contains("delivered into live session (no saved session)"), "{reason}");
    assert!(!reason.contains("fresh fire"), "a reuse is not called a fresh fire: {reason}");
    let hb = row(ws, "default");
    assert_eq!(hb.last_session_id, None, "no made-up conversation id is saved on a reuse");
    assert_eq!(hb.active_terminal_id.as_deref(), Some(t1.as_str()));

    v2_session_map::clear_for_tests();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_session_stops_the_live_session_and_clears_the_saved_id() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _env = Env::new();
    let ws = "hb-stale-ws-restart";
    let project = setup_project(ws);
    write_primary_agent(&project, "manager");
    seed_heartbeat(&project, ws, "default");
    let path = project.to_string_lossy().into_owned();

    let (t1, conv) = first_fire(&path, ws, "default");
    let (v, history) = k2_daemon::heartbeat_launch::restart_session(&path, "hb:default").expect("restart");
    assert_eq!(v["stopped"], true, "{v}");
    assert_eq!(v["terminalId"], t1.as_str(), "{v}");
    assert_eq!(v["clearedSessionId"], conv.as_str(), "{v}");
    assert!(history.contains(&t1), "{history}");
    assert!(!alive(&t1), "killed");
    assert!(v2_session_map::lookup_by_agent_name(&format!("{ws}:hb:default")).is_none(), "unregistered");
    let hb = row(ws, "default");
    assert_eq!(hb.last_session_id, None);
    assert_eq!(hb.active_terminal_id, None);

    // Not a heartbeat name, and a name with no heartbeat: refused.
    for bad in ["sales/default", "tab-1:x", "hb:"] {
        let e = k2_daemon::heartbeat_launch::restart_session(&path, bad).expect_err(bad);
        assert!(e.starts_with("not_a_heartbeat"), "{bad}: {e}");
    }
    let e = k2_daemon::heartbeat_launch::restart_session(&path, "nope").expect_err("unknown");
    assert!(k2_core::heartbeats::is_no_such_heartbeat(&e), "{e}");

    // The next fire starts a new session.
    let r = k2_daemon::heartbeat_launch::smart_launch(&path, "default");
    assert_eq!(r["success"], true, "{r}");
    assert_eq!(r["branch"], "fresh_fire", "{r}");
    assert_ne!(r["terminalId"], t1.as_str(), "{r}");

    v2_session_map::clear_for_tests();
}
