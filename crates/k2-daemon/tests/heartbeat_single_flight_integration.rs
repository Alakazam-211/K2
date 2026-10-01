//! Heartbeat S2 — one fire per slot however many tickers run (HB3).
//!
//! - HB13: a second due scan for a project already scanning returns
//!   `{"skipped":"tick_in_progress"}` at once.
//! - HB14: the lease re-checks `last_fired` against what the evaluator
//!   read, so a daemon tick and a leftover OS tick that both judged the
//!   same slot due fire it once — even when the second acquires after
//!   the first released.
//! - W2: a fan-out with a due heartbeat takes exactly one keep-awake hold
//!   and releases it (counting fake, no real power calls).

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use k2_core::db::init_for_tests;
use k2_core::db::schema::{AgentHeartbeat, LeaseCheck, LeaseOutcome};
use k2_daemon::power::fake::FakePowerOs;
use k2_daemon::triage::{handle_scheduler_fire, run_candidates_bounded_with, ScanGuard};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A workspace with a manager agent, one hourly heartbeat `hb` that is
/// due (last fired 2 h ago), and a non-empty WAKEUP.md.
fn setup(workspace_id: &str) -> (PathBuf, String) {
    init_for_tests();
    let project = std::env::temp_dir().join(format!(
        "k2-hb-single-flight-{workspace_id}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let agent_dir = project.join(".k2so/agents/manager");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");
    std::fs::write(
        agent_dir.join("AGENT.md"),
        "---\nname: manager\ntype: manager\n---\n# manager\n",
    )
    .expect("AGENT.md");
    std::fs::write(project.join("WAKEUP.md"), "---\nname: hb\n---\nCheck the inbox.\n")
        .expect("WAKEUP.md");
    let last = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name, agent_mode) VALUES (?1, ?2, 'hb-sf', 'manager')",
        rusqlite::params![workspace_id, project.to_string_lossy().as_ref()],
    )
    .expect("project");
    AgentHeartbeat::insert(
        &conn,
        &format!("{workspace_id}-hb"),
        workspace_id,
        "hb",
        "hourly",
        r#"{"every_seconds":3600}"#,
        "WAKEUP.md",
        true,
    )
    .expect("heartbeat");
    conn.execute(
        "UPDATE workspace_heartbeats SET last_fired = ?1 WHERE project_id = ?2",
        rusqlite::params![last, workspace_id],
    )
    .expect("last_fired");
    (project, last)
}

fn fires(workspace_id: &str, decision: &str) -> usize {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM heartbeat_fires WHERE project_id = ?1 AND decision = ?2",
        rusqlite::params![workspace_id, decision],
        |r| r.get::<_, i64>(0),
    )
    .expect("count fires") as usize
}

/// A launcher that does what `smart_launch_scheduled` does at the lease
/// (HB14 CAS) and then "fires": stamps last_fired and releases.
fn counting_launcher(
    ws: String,
    launches: Arc<AtomicUsize>,
) -> impl Fn(&str, &k2_core::heartbeats::HeartbeatFireCandidate) -> serde_json::Value + Send + Sync + Clone + 'static
{
    move |_pp, cand| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let outcome = AgentHeartbeat::try_acquire_heartbeat_checked(
            &conn,
            &ws,
            &cand.name,
            LeaseCheck::Scheduled(cand.evaluated_last_fired.as_deref()),
        )
        .expect("lease claim");
        if outcome != LeaseOutcome::Acquired {
            return serde_json::json!({ "success": false, "decision": format!("{outcome:?}") });
        }
        launches.fetch_add(1, Ordering::SeqCst);
        AgentHeartbeat::stamp_fired_and_release(&conn, &ws, &cand.name).expect("stamp");
        serde_json::json!({ "success": true, "decision": "fired" })
    }
}

/// HB3 + HB14: two ticks evaluate the same due slot (same `last_fired`).
/// The first fires and stamps; the second acquires AFTER the release and
/// must still not fire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_ticks_on_the_same_slot_fire_once() {
    let _g = lock();
    let ws = "hb-sf-ws-two-ticks";
    let (project, _last) = setup(ws);
    k2_daemon::power::install_os(Arc::new(FakePowerOs::ready()));
    let path = project.to_string_lossy().into_owned();

    // Both tickers evaluate before either fires.
    let tick_a = k2_core::heartbeats::k2so_agents_heartbeat_tick(&path);
    let tick_b = k2_core::heartbeats::k2so_agents_heartbeat_tick(&path);
    assert_eq!(tick_a.len(), 1, "precondition: the slot is due");
    assert_eq!(tick_b.len(), 1, "precondition: the second ticker sees it due too");

    let launches = Arc::new(AtomicUsize::new(0));
    let fired_a = run_candidates_bounded_with(&path, tick_a, counting_launcher(ws.into(), launches.clone()));
    let fired_b = run_candidates_bounded_with(&path, tick_b, counting_launcher(ws.into(), launches.clone()));
    assert_eq!(fired_a, vec!["hb".to_string()]);
    assert!(fired_b.is_empty(), "the second tick must not fire the same slot, got {fired_b:?}");
    assert_eq!(launches.load(Ordering::SeqCst), 1, "exactly one launch per slot");
    let _ = std::fs::remove_dir_all(&project);
}

/// HB14 at the lease, directly: read X, another fire stamps Y, a claim
/// expecting X fails with AlreadyFired; Manual is unaffected.
#[test]
fn lease_claim_rechecks_last_fired() {
    let _g = lock();
    let ws = "hb-sf-ws-cas";
    let (project, last) = setup(ws);
    let db = k2_core::db::shared();
    let conn = db.lock();
    // Another tick fires (stamps Y).
    AgentHeartbeat::stamp_fired_and_release(&conn, ws, "hb").expect("stamp Y");
    assert_eq!(
        AgentHeartbeat::try_acquire_heartbeat_checked(&conn, ws, "hb", LeaseCheck::Scheduled(Some(&last)))
            .expect("claim"),
        LeaseOutcome::AlreadyFired,
    );
    let row = AgentHeartbeat::get_by_name(&conn, ws, "hb").expect("query").expect("row");
    assert!(row.in_flight_started_at.is_none(), "a refused claim must not leave a lease");
    // Expecting the current value wins.
    let current = row.last_fired.clone();
    assert_eq!(
        AgentHeartbeat::try_acquire_heartbeat_checked(&conn, ws, "hb", LeaseCheck::Scheduled(current.as_deref()))
            .expect("claim"),
        LeaseOutcome::Acquired,
    );
    AgentHeartbeat::release_heartbeat_lease(&conn, ws, "hb").expect("release");
    // Manual Launch is exempt (no due re-check).
    assert!(AgentHeartbeat::try_acquire_heartbeat(&conn, ws, "hb").expect("manual claim"));
    AgentHeartbeat::release_heartbeat_lease(&conn, ws, "hb").expect("release");
    drop(conn);
    let _ = std::fs::remove_dir_all(&project);
}

/// HB14 through the real scheduled launch: a stale expectation returns
/// `skipped_already_fired`, writes no audit row and spawns nothing.
#[test]
fn scheduled_launch_with_stale_last_fired_is_a_quiet_skip() {
    let _g = lock();
    let ws = "hb-sf-ws-real-launch";
    let (project, _last) = setup(ws);
    let path = project.to_string_lossy().into_owned();
    let before = fires(ws, "skipped_locked");
    let v = k2_daemon::heartbeat_launch::smart_launch_scheduled(
        &path,
        "hb",
        None,
        Some("2000-01-01T00:00:00+00:00"),
    );
    assert_eq!(v["decision"], "skipped_already_fired", "{v}");
    assert_eq!(v["success"], false);
    assert_eq!(fires(ws, "skipped_locked"), before, "a lost race is not an audit event");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = AgentHeartbeat::get_by_name(&conn, ws, "hb").expect("query").expect("row");
    assert!(row.in_flight_started_at.is_none());
    drop(conn);
    let _ = std::fs::remove_dir_all(&project);
}

/// HB13: while a scan for the project runs, another returns at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_scan_of_a_busy_project_is_skipped() {
    let _g = lock();
    let ws = "hb-sf-ws-busy";
    let (project, _last) = setup(ws);
    let path = project.to_string_lossy().into_owned();

    let held = ScanGuard::try_begin(&path).expect("first scan takes the slot");
    assert!(ScanGuard::try_begin(&path).is_none(), "a second guard must be refused");
    let p2 = path.clone();
    let v: serde_json::Value = serde_json::from_str(
        &tokio::task::spawn_blocking(move || handle_scheduler_fire(&p2)).await.expect("join"),
    )
    .expect("json");
    assert_eq!(v["skipped"], "tick_in_progress", "{v}");
    assert_eq!(v["count"], 0);
    drop(held);
    assert!(ScanGuard::try_begin(&path).is_some(), "the slot frees on drop");
    let _ = std::fs::remove_dir_all(&project);
}

/// W2 / T-W2: one due heartbeat → exactly one keep-awake hold and one
/// release around the fan-out; a panicking launcher releases too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fan_out_holds_the_machine_awake_once_and_releases() {
    let _g = lock();
    let ws = "hb-sf-ws-awake";
    let (project, _last) = setup(ws);
    let path = project.to_string_lossy().into_owned();
    let fake = Arc::new(FakePowerOs::ready());
    k2_daemon::power::install_os(fake.clone());

    let cands = k2_core::heartbeats::k2so_agents_heartbeat_tick(&path);
    assert_eq!(cands.len(), 1);
    let launches = Arc::new(AtomicUsize::new(0));
    let fired = run_candidates_bounded_with(&path, cands, counting_launcher(ws.into(), launches));
    assert_eq!(fired.len(), 1);
    assert_eq!(fake.holds_taken(), 1, "one OS hold for the fan-out");
    assert_eq!(fake.holds_released(), 1, "released when the fan-out ends");

    let cand = k2_core::heartbeats::HeartbeatFireCandidate {
        name: "hb".into(),
        agent_name: "manager".into(),
        wakeup_path_abs: project.join("WAKEUP.md").to_string_lossy().into_owned(),
        wakeup_path_rel: "WAKEUP.md".into(),
        catchup_of: None,
        evaluated_last_fired: None,
    };
    let fired = run_candidates_bounded_with(&path, vec![cand], |_pp, _c| -> serde_json::Value {
        panic!("simulated launch panic")
    });
    assert!(fired.is_empty());
    assert_eq!(fake.holds_taken(), 2);
    assert_eq!(fake.holds_released(), 2, "a panicking launch still releases the hold");
    let _ = std::fs::remove_dir_all(&project);
}
