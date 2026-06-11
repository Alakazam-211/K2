//! H6 of Phase 4 integration tests — `handle_scheduler_fire`
//! dispatches wakes via the Session Stream pipeline when the
//! project has `use_session_stream='on'`.
//!
//! Post-H7 note: the destructive fire path moved from
//! `handle_triage` (which is now read-only) to
//! `handle_scheduler_fire`. URL: `/cli/scheduler-tick`. Tests
//! below exercise the destructive handler directly.
//!
//! The triage handler depends on a lot of real wiring
//! (scheduler_tick, heartbeat candidates, AGENT.md, session
//! locks). These tests exercise the dispatch point specifically:
//!
//! - Flag ON + scheduler returns launchable agent → daemon
//!   `session_map` gains an entry under that agent.
//! - Flag OFF + same setup → daemon `session_map` stays empty
//!   (legacy `spawn_wake_headless` path owns the PTY).
//! - Response JSON always carries `{count, launched, heartbeats}`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;

use k2_core::db::init_for_tests;

use k2_daemon::session_lookup;
use k2_daemon::triage;
use k2_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn tmp_project_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "k2so-h6-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn seed_project(path: &str, use_session_stream: &str) -> String {
    let id = format!("proj-{}", k2_core::session::SessionId::new());
    let db = k2_core::db::shared();
    let conn = db.lock();
    // `heartbeat_mode` must be something other than 'off' (its
    // schema default) for scheduler_tick to evaluate agents —
    // 'heartbeat' means "fire whenever work is ready," with no
    // schedule window gating.
    conn.execute(
        "INSERT OR REPLACE INTO projects \
         (id, path, name, color, agent_mode, pinned, tab_order, \
          heartbeat_mode, use_session_stream) \
         VALUES (?1, ?2, ?3, '#123456', 'manager', 0, 0, 'heartbeat', ?4)",
        rusqlite::params![id, path, "triage-test", use_session_stream],
    )
    .unwrap();
    id
}

fn clear_projects() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "DELETE FROM projects WHERE id NOT IN ('_orphan', '_broadcast')",
        [],
    );
    let _ = conn.execute("DELETE FROM agent_sessions", []);
    let _ = conn.execute("DELETE FROM workspace_heartbeats", []);
}

fn drain_session_map() {
    // Post-0.39.0: only the v2 map remains.
    for (name, _) in v2_session_map::snapshot() {
        v2_session_map::unregister(&name);
    }
}

fn write_agent_md(project: &Path, name: &str, agent_type: &str) {
    let dir = project.join(".k2so/agents").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("AGENT.md"),
        format!(
            "---\nname: {name}\nrole: triage test\ntype: {ty}\n---\n\nAgent body.\n",
            name = name, ty = agent_type
        ),
    )
    .unwrap();
    // WAKEUP.md must exist for `compose_wake_prompt_for_agent` to
    // return Some. Agent-type `k2so` uses the shipped template by
    // default — but we supply an explicit WAKEUP.md so nothing is
    // silently template-derived.
    std::fs::write(
        dir.join("WAKEUP.md"),
        "# Wake\n\nDo the thing.\n",
    )
    .unwrap();
}

/// Give the scheduler a reason to mark the agent launchable:
/// write an `active/` work item that hasn't been picked up.
/// Scheduler_tick looks at file state under `.k2so/agents/<name>/
/// work/active/` and reports agents that have idle work + no
/// active session.
fn seed_inbox_work(project: &Path, agent: &str, slug: &str) {
    let dir = project.join(".k2so/agents").join(agent).join("work/inbox");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{slug}.md")),
        "---\n\
         title: Triage test\n\
         priority: high\n\
         type: task\n\
         source: manual\n\
         assigned_by: test\n\
         created: 2026-04-20\n\
         ---\n\
         Body.\n",
    )
    .unwrap();
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn scheduler_fire_returns_json_shape_even_when_nothing_to_launch() {
    let _g = lock();
    init_for_tests();
    clear_projects();
    drain_session_map();

    let proj = tmp_project_dir("empty");
    let proj_str = proj.to_string_lossy().into_owned();
    seed_project(&proj_str, "on");

    let body = triage::handle_scheduler_fire(&proj_str);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["count"].is_number());
    assert!(v["launched"].is_array());
    assert!(v["heartbeats"].is_array());
    assert_eq!(v["count"].as_u64(), Some(0));

    clear_projects();
}

/// `handle_triage` post-H7 rework: read-only plain-text summary
/// regardless of `use_session_stream` setting. No spawning.
#[tokio::test(flavor = "current_thread")]
async fn triage_summary_is_readonly_and_plaintext() {
    let _g = lock();
    init_for_tests();
    clear_projects();
    drain_session_map();

    let proj = tmp_project_dir("readonly");
    let proj_str = proj.to_string_lossy().into_owned();
    seed_project(&proj_str, "on");
    write_agent_md(&proj, "readonly-probe", "agent-template");
    seed_inbox_work(&proj, "readonly-probe", "probe-task");

    let body = triage::handle_triage(&proj_str);
    assert!(
        body.contains("readonly-probe"),
        "summary missing agent name: {body}"
    );
    assert!(
        body.contains("high") || body.contains("Triage test"),
        "summary missing work-item details: {body}"
    );
    // No spawning happened → neither map has the agent.
    assert!(
        session_lookup::lookup_any("readonly-probe").is_none(),
        "read-only triage should NOT spawn; lookup found a leaked session"
    );

    clear_projects();
}

#[tokio::test(flavor = "current_thread")]
async fn triage_with_flag_on_spawns_via_session_stream() {
    let _g = lock();
    init_for_tests();
    clear_projects();
    drain_session_map();

    let proj = tmp_project_dir("on");
    let proj_str = proj.to_string_lossy().into_owned();
    let project_id = seed_project(&proj_str, "on");
    // Agent type `k2so` has a shipped wakeup template so
    // `compose_wake_prompt_for_agent` returns Some without a
    // separate WAKEUP.md. We still scaffold one above.
    write_agent_md(&proj, "runner", "k2so");
    seed_inbox_work(&proj, "runner", "do-thing");

    let _body = triage::handle_scheduler_fire(&proj_str);

    // **0.37.5 canonicalization:** scheduler-driven spawns
    // (v2 path → spawn_agent_session_v2_blocking) register under
    // bare `<project_id>` (no agent suffix). Pre-0.37.5 it was
    // `<project_id>:<agent_name>`; the suffix was vestigial
    // post-unification (one agent per workspace).
    let canonical_key = project_id.to_string();
    assert!(
        session_lookup::lookup_any(&canonical_key).is_some(),
        "expected '{canonical_key}' in a daemon session map under flag-on scheduler fire"
    );

    drain_session_map();
    clear_projects();
}

#[tokio::test(flavor = "current_thread")]
async fn triage_with_flag_off_does_not_land_in_session_map() {
    let _g = lock();
    init_for_tests();
    clear_projects();
    drain_session_map();

    let proj = tmp_project_dir("off");
    let proj_str = proj.to_string_lossy().into_owned();
    seed_project(&proj_str, "off");
    write_agent_md(&proj, "legacy", "k2so");
    seed_inbox_work(&proj, "legacy", "legacy-task");

    let _body = triage::handle_scheduler_fire(&proj_str);

    // Flag-off path uses `spawn_wake_headless` which owns the PTY
    // through the legacy TerminalManager — no entry should appear
    // in any daemon session map.
    assert!(
        session_lookup::lookup_any("legacy").is_none(),
        "flag-off triage leaked into daemon session map"
    );

    // Best-effort cleanup: the legacy path DID spawn a PTY into
    // the global TerminalManager. Kill it via the core helper so
    // it doesn't linger for the next test.
    let _ = k2_core::terminal::shared();

    drain_session_map();
    clear_projects();
}
