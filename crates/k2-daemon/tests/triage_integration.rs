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
    // Hermetic: a temp HOME, and agent CLIs resolve only to `exec cat`
    // shims (test builds refuse a real `claude`; the workspace default
    // agent is `claude`).
    let _home = k2_core::test_env::TempHome::new();
    let _agents = k2_core::test_env::AgentShim::install();
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
    // Hermetic: a temp HOME, and agent CLIs resolve only to `exec cat`
    // shims (test builds refuse a real `claude`; the workspace default
    // agent is `claude`).
    let _home = k2_core::test_env::TempHome::new();
    let _agents = k2_core::test_env::AgentShim::install();
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
    // Hermetic: a temp HOME, and agent CLIs resolve only to `exec cat`
    // shims (test builds refuse a real `claude`; the workspace default
    // agent is `claude`).
    let _home = k2_core::test_env::TempHome::new();
    let _agents = k2_core::test_env::AgentShim::install();
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
    // Hermetic: a temp HOME, and agent CLIs resolve only to `exec cat`
    // shims (test builds refuse a real `claude`; the workspace default
    // agent is `claude`).
    let _home = k2_core::test_env::TempHome::new();
    let _agents = k2_core::test_env::AgentShim::install();
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

// ─────────────────────────────────────────────────────────────────────
// HB9 — tick stamps name their source (heartbeat S1, T-S1d)
// ─────────────────────────────────────────────────────────────────────

fn meta(key: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::SchedulerMeta::get(&conn, key)
}

fn set_meta(key: &str, value: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::SchedulerMeta::set(&conn, key, value).expect("set scheduler_meta");
}

/// A daemon scan (boot / wall-clock jump) moves `last_daemon_tick_at`
/// and never `last_os_tick_at`. The HTTP active-projects route (what
/// `heartbeat.sh` calls) moves `last_os_tick_at` and never the daemon
/// key. Pre-S1 both wrote one `last_tick_at`, so a daemon restart hid
/// a dead OS job.
#[tokio::test(flavor = "current_thread")]
async fn tick_stamps_name_their_source() {
    use k2_core::db::schema::SchedulerMeta;
    let _g = lock();
    // Hermetic: a temp HOME, and agent CLIs resolve only to `exec cat`
    // shims (test builds refuse a real `claude`; the workspace default
    // agent is `claude`).
    let _home = k2_core::test_env::TempHome::new();
    let _agents = k2_core::test_env::AgentShim::install();
    init_for_tests();

    let old = "2000-01-01T00:00:00+00:00";
    set_meta(SchedulerMeta::LAST_OS_TICK_AT, old);
    set_meta(SchedulerMeta::LAST_DAEMON_TICK_AT, old);
    set_meta(SchedulerMeta::LAST_TICK_AT, old);

    // Boot / jump scan.
    let _paths = triage::daemon_scan_project_paths();
    assert_eq!(
        meta(SchedulerMeta::LAST_OS_TICK_AT).as_deref(),
        Some(old),
        "a daemon scan must not stamp the OS tick key"
    );
    let daemon_tick = meta(SchedulerMeta::LAST_DAEMON_TICK_AT).expect("daemon tick stamped");
    assert_ne!(daemon_tick, old, "a daemon scan must stamp last_daemon_tick_at");
    assert_eq!(
        meta(SchedulerMeta::LAST_TICK_AT).as_deref(),
        Some(daemon_tick.as_str()),
        "last_tick_at stays the newest tick of any kind"
    );

    // OS tick: the HTTP route heartbeat.sh calls.
    set_meta(SchedulerMeta::LAST_DAEMON_TICK_AT, old);
    let _list = triage::handle_active_projects();
    assert_eq!(
        meta(SchedulerMeta::LAST_DAEMON_TICK_AT).as_deref(),
        Some(old),
        "an OS tick must not stamp the daemon tick key"
    );
    let os_tick = meta(SchedulerMeta::LAST_OS_TICK_AT).expect("os tick stamped");
    assert_ne!(os_tick, old, "active-projects must stamp last_os_tick_at");
    assert_eq!(meta(SchedulerMeta::LAST_TICK_AT).as_deref(), Some(os_tick.as_str()));

    // The non-stamping list stamps nothing.
    set_meta(SchedulerMeta::LAST_OS_TICK_AT, old);
    set_meta(SchedulerMeta::LAST_DAEMON_TICK_AT, old);
    let _ = triage::active_project_paths();
    assert_eq!(meta(SchedulerMeta::LAST_OS_TICK_AT).as_deref(), Some(old));
    assert_eq!(meta(SchedulerMeta::LAST_DAEMON_TICK_AT).as_deref(), Some(old));
}

// ─────────────────────────────────────────────────────────────────────
// Heartbeat S3 — stored wait reasons + the overdue watchdog (T5, HB19, HB22)
// ─────────────────────────────────────────────────────────────────────

/// Seed one hourly heartbeat (every 60 s) with a real WAKEUP.md body.
/// `created_ago_secs` back-dates `created_at`, which (never fired) is
/// the reference its first slot counts from.
fn seed_heartbeat(project: &Path, project_id: &str, name: &str, created_ago_secs: i64) {
    let rel = format!(".k2/heartbeats/{name}/WAKEUP.md");
    let abs = project.join(&rel);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(&abs, "---\ndescription:\n---\n\ncheck the inbox\n").unwrap();
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::AgentHeartbeat::insert(
        &conn,
        &uuid::Uuid::new_v4().to_string(),
        project_id,
        name,
        "hourly",
        r#"{"every_seconds":60}"#,
        &rel,
        true,
    )
    .unwrap();
    conn.execute(
        "UPDATE workspace_heartbeats SET created_at = unixepoch() - ?1 \
         WHERE project_id = ?2 AND name = ?3",
        rusqlite::params![created_ago_secs, project_id, name],
    )
    .unwrap();
}

fn hb_row(project_id: &str, name: &str) -> k2_core::db::schema::AgentHeartbeat {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::AgentHeartbeat::get_by_name(&conn, project_id, name)
        .unwrap()
        .expect("heartbeat row")
}

fn overdue_rows(project_id: &str, name: &str) -> Vec<k2_core::db::schema::HeartbeatFire> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::HeartbeatFire::list_by_schedule_name(&conn, project_id, name, 50)
        .unwrap()
        .into_iter()
        .filter(|f| f.decision == "overdue")
        .collect()
}

/// T5 — a workspace with no resolvable agent name. Pre-S3 the due scan
/// skipped every heartbeat there silently, with no audit (research S4).
/// Now one tick stores `no_agent`; once the slot is >120 s past, two
/// passes write exactly ONE `overdue` row, naming the `no_agent` gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_agent_is_named_and_overdue_audits_once_per_episode() {
    let _g = lock();
    // Hermetic: a temp HOME, and agent CLIs resolve only to `exec cat`
    // shims (test builds refuse a real `claude`; the workspace default
    // agent is `claude`).
    let _home = k2_core::test_env::TempHome::new();
    let _agents = k2_core::test_env::AgentShim::install();
    init_for_tests();
    clear_projects();
    drain_session_map();

    let proj = tmp_project_dir("s3-no-agent");
    let proj_str = proj.to_string_lossy().into_owned();
    // agent_enabled = 0 and no AGENT.md → resolve_agent_name is None.
    let pid = seed_project(&proj_str, "on");
    seed_heartbeat(&proj, &pid, "nag", 10);

    let body = triage::handle_scheduler_fire(&proj_str);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["heartbeats"], serde_json::json!([]), "no agent → nothing fires");
    let hb = hb_row(&pid, "nag");
    assert_eq!(hb.wait_reason.as_deref(), Some("no_agent"), "one tick names the gate");
    assert!(hb.next_fire_at.is_some(), "the slot is still stored");
    assert!(overdue_rows(&pid, "nag").is_empty(), "not overdue yet");

    // The slot is now 340 s past.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE workspace_heartbeats SET created_at = unixepoch() - 400 \
             WHERE project_id = ?1 AND name = 'nag'",
            rusqlite::params![pid],
        )
        .unwrap();
    }
    k2_daemon::heartbeat_wait::run_pass();
    k2_daemon::heartbeat_wait::run_pass();
    let _ = triage::handle_scheduler_fire(&proj_str);

    let rows = overdue_rows(&pid, "nag");
    assert_eq!(rows.len(), 1, "exactly one overdue row per episode: {rows:?}");
    let reason = rows[0].reason.as_deref().expect("overdue row carries a reason");
    assert!(reason.contains("no_agent"), "the audit names the gate: {reason}");
    let hb = hb_row(&pid, "nag");
    assert_eq!(
        hb.wait_reason.as_deref(),
        Some("no_agent"),
        "a more specific reason is kept, not replaced by overdue"
    );
    assert!(hb.overdue_noted_at.is_some(), "the episode is open");

    clear_projects();
}

/// HB22 + HB4 — an agent-ready row whose slot passed while nothing
/// ticked: the wait pass stores `overdue`, its detail and the one audit
/// row name the `no_ticks` gate; a fire closes the episode.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overdue_without_ticks_names_no_ticks_and_a_fire_closes_the_episode() {
    use k2_core::db::schema::SchedulerMeta;
    let _g = lock();
    // Hermetic: a temp HOME, and agent CLIs resolve only to `exec cat`
    // shims (test builds refuse a real `claude`; the workspace default
    // agent is `claude`).
    let _home = k2_core::test_env::TempHome::new();
    let _agents = k2_core::test_env::AgentShim::install();
    init_for_tests();
    clear_projects();
    drain_session_map();

    let proj = tmp_project_dir("s3-no-ticks");
    let proj_str = proj.to_string_lossy().into_owned();
    let pid = seed_project(&proj_str, "on");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE projects SET agent_enabled = 1 WHERE id = ?1", rusqlite::params![pid])
            .unwrap();
    }
    seed_heartbeat(&proj, &pid, "late", 400);
    let old = "2000-01-01T00:00:00+00:00";
    set_meta(SchedulerMeta::LAST_OS_TICK_AT, old);
    set_meta(SchedulerMeta::LAST_DAEMON_TICK_AT, old);

    k2_daemon::heartbeat_wait::run_pass();
    k2_daemon::heartbeat_wait::run_pass();

    let hb = hb_row(&pid, "late");
    assert_eq!(hb.wait_reason.as_deref(), Some("overdue"), "row: {hb:?}");
    let detail = hb.wait_detail.as_deref().expect("overdue detail");
    assert!(detail.starts_with("no_ticks"), "detail names the gate: {detail}");
    let rows = overdue_rows(&pid, "late");
    assert_eq!(rows.len(), 1, "one overdue row: {rows:?}");
    assert!(rows[0].reason.as_deref().expect("reason").contains("no_ticks"));

    // The heartbeat fires (stamp as the launcher does on success).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::AgentHeartbeat::stamp_fired_and_release(&conn, &pid, "late")
            .unwrap();
    }
    k2_daemon::heartbeat_wait::run_pass();
    let hb = hb_row(&pid, "late");
    assert_eq!(hb.wait_reason.as_deref(), Some("scheduled"));
    assert_eq!(hb.overdue_noted_at, None, "a fire closes the episode");
    assert_eq!(overdue_rows(&pid, "late").len(), 1, "no new overdue row");

    clear_projects();
}
