//! Lease-watchdog wedge fix (heartbeat reliability overhaul).
//!
//! Pre-overhaul, a `smart_launch` that outlived the per-spawn timeout
//! in `run_candidates_bounded` was abandoned WITHOUT releasing the
//! row's `in_flight_started_at` — under the default
//! `concurrency_policy='forbid'` the heartbeat was wedged until the
//! next daemon restart's boot sweep (misfire study §1.6.2; the code
//! comment promised "P5.5's watchdog," which was never built).
//!
//! This test drives the timeout path through the REAL fan-out
//! (`run_candidates_bounded_with`) with an injected launcher that
//! acquires the lease exactly like `smart_launch` does and then hangs.
//! With the env-shrunk deadline + grace, the watchdog must:
//!
//!   1. return the tick promptly (no fire recorded),
//!   2. force-release the stale lease WITHOUT a daemon restart,
//!   3. record the hang as a failed fire-attempt (error audit row +
//!      consecutive_failures bump).

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::db::init_for_tests;
use k2_core::db::schema::AgentHeartbeat;
use k2_daemon::triage::run_candidates_bounded_with;

/// Serialize tests — shared DB + env vars are process globals.
static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn setup_project(workspace_id: &str) -> PathBuf {
    let project_path = std::env::temp_dir().join(format!(
        "k2-hb-watchdog-test-{}-{}-{}",
        workspace_id,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&project_path);
    std::fs::create_dir_all(&project_path).unwrap();

    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name, agent_mode) \
         VALUES (?1, ?2, ?3, 'manager')",
        rusqlite::params![
            workspace_id,
            project_path.to_string_lossy().as_ref(),
            "hb-watchdog-test",
        ],
    )
    .unwrap();
    project_path
}

/// Env guard: set the deadline/grace overrides and restore on drop so
/// other tests in the process see production defaults.
struct EnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    fn set(pairs: &[(&'static str, &str)]) -> Self {
        let saved = pairs
            .iter()
            .map(|(k, v)| {
                let prior = std::env::var(k).ok();
                std::env::set_var(k, v);
                (*k, prior)
            })
            .collect();
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, prior) in &self.saved {
            match prior {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hung_spawn_lease_is_released_by_watchdog_without_restart() {
    let _g = lock();
    init_for_tests();
    let _env = EnvGuard::set(&[
        ("K2_HEARTBEAT_SPAWN_DEADLINE_SECS", "1"),
        ("K2_HEARTBEAT_LEASE_WATCHDOG_SECS", "1"),
    ]);

    let workspace_id = "hb-watchdog-ws-1";
    let project = setup_project(workspace_id);
    let project_path = project.to_string_lossy().into_owned();

    // Primary AGENT.md so `resolve_agent_name` resolves (same fixture
    // the other heartbeat integration tests write).
    {
        let dir = project.join(".k2so/agents/manager");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("AGENT.md"),
            "---\nname: manager\ntype: manager\n---\n# manager\n",
        )
        .unwrap();
    }

    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        AgentHeartbeat::insert(
            &conn,
            "watchdog-hb-id",
            workspace_id,
            "hung-hb",
            "hourly",
            r#"{"every_seconds":3600}"#,
            "WAKEUP.md",
            true,
        )
        .expect("seed heartbeat");
    }

    let candidates = vec![k2_core::heartbeats::HeartbeatFireCandidate {
        name: "hung-hb".to_string(),
        agent_name: "manager".to_string(),
        wakeup_path_abs: project.join("WAKEUP.md").to_string_lossy().into_owned(),
        wakeup_path_rel: "WAKEUP.md".to_string(),
        catchup_of: None,
    }];

    // Launcher that does exactly what smart_launch does first —
    // acquire the in-flight lease — and then hangs well past
    // deadline + grace, like a hung PTY allocate. Called directly:
    // `run_candidates_bounded_with` steps into async via
    // block_in_place, which needs a multi-thread runtime worker (the
    // same context the production HTTP handler provides).
    let ws = workspace_id.to_string();
    let fired = run_candidates_bounded_with(&project_path, candidates, move |_pp, cand| {
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let won = AgentHeartbeat::try_acquire_heartbeat(&conn, &ws, &cand.name)
                .expect("acquire lease");
            assert!(won, "test launcher must win the lease");
        }
        std::thread::sleep(Duration::from_secs(20));
        serde_json::json!({ "success": false, "decision": "hung" })
    });

    assert!(fired.is_empty(), "a timed-out spawn must not report a fire");

    // The lease is still held right after the timeout returned (the
    // watchdog's grace hasn't elapsed) — this is the pre-overhaul
    // wedge state.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row = AgentHeartbeat::get_by_name(&conn, workspace_id, "hung-hb")
            .unwrap()
            .expect("row exists");
        assert!(
            row.in_flight_started_at.is_some(),
            "lease should still be held immediately after the timeout",
        );
    }

    // Wait past deadline(1s) + grace(1s) + scheduling slack, then the
    // watchdog must have force-released the lease and recorded the
    // failed attempt — all WITHOUT a daemon restart.
    let mut released = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let db = k2_core::db::shared();
        let conn = db.lock();
        let row = AgentHeartbeat::get_by_name(&conn, workspace_id, "hung-hb")
            .unwrap()
            .expect("row exists");
        if row.in_flight_started_at.is_none() {
            released = true;
            assert_eq!(
                row.consecutive_failures, 1,
                "the watchdog release must count as a failed fire-attempt",
            );
            assert!(
                row.next_retry_at.is_some(),
                "the failed attempt must schedule a backoff window",
            );
            break;
        }
    }
    assert!(
        released,
        "watchdog must force-release the hung spawn's lease within its grace period",
    );

    // The hang is visible in the audit trail as an error row.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let fires = k2_core::db::schema::HeartbeatFire::list_by_schedule_name(
            &conn,
            workspace_id,
            "hung-hb",
            10,
        )
        .expect("list fires");
        assert!(
            fires.iter().any(|f| {
                f.decision == "error"
                    && f.reason
                        .as_deref()
                        .unwrap_or("")
                        .contains("force-released by the watchdog")
            }),
            "watchdog release must write an error audit row; got {:?}",
            fires
                .iter()
                .map(|f| (f.decision.clone(), f.reason.clone()))
                .collect::<Vec<_>>(),
        );
    }

    let _ = std::fs::remove_dir_all(&project);
}
