//! T3 (heartbeat S2, HB12) — a headless daemon with NO external ticker
//! fires a due heartbeat on its own.
//!
//! Spawns the real `k2-daemon` binary under a temp HOME with
//! `K2_HEARTBEAT_NO_SELF_HEAL=1` (no OS job, no power calls),
//! `K2_TEST_AGENT_SHIM_DIR` (an `exec cat` shim, so no real agent can
//! start) and `K2SO_WAKE_HEADLESS_TEST_COMMAND=cat`. Nothing calls
//! `/cli/scheduler-tick` or `/cli/heartbeat/active-projects`. After the
//! daemon is up, the test writes a workspace + a 60 s heartbeat straight
//! into the daemon's DB with `created_at = now`, so the boot scan (3 s
//! after boot) finds it NOT due and only the daemon's own 60 s loop can
//! fire it. Asserts a `fired` row within 150 s, that the daemon stamped
//! `last_daemon_tick_at`, and that no OS tick was ever recorded.
//!
//! Before S2 this failed: the daemon never ticked by itself.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "k2-hb-t3-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}

/// The daemon's DB once it has migrated through S2's 0123.
fn wait_for_db(k2_dir: &Path, deadline: Instant) -> PathBuf {
    loop {
        let path = k2_core::db::resolve_home_db_path(k2_dir);
        if path.exists() {
            if let Ok(conn) = rusqlite::Connection::open(&path) {
                let migrated: rusqlite::Result<i64> = conn.query_row(
                    "SELECT COUNT(*) FROM _migrations WHERE name = '0123_heartbeat_schedule_anchor'",
                    [],
                    |r| r.get(0),
                );
                if migrated == Ok(1) && k2_dir.join("heartbeat.port").exists() {
                    return path;
                }
            }
        }
        assert!(Instant::now() < deadline, "daemon did not create and migrate its DB in time");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn open(db: &Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(db).expect("open daemon DB");
    conn.busy_timeout(Duration::from_secs(10)).expect("busy timeout");
    conn
}

#[test]
fn headless_daemon_fires_with_no_external_ticker_within_150s() {
    let home = scratch("home");
    let k2_dir = home.join(".k2");
    std::fs::create_dir_all(&k2_dir).expect(".k2");
    let shim_dir = home.join("agent-shim");
    std::fs::create_dir_all(&shim_dir).expect("shim dir");
    let shim = shim_dir.join("cat");
    std::fs::write(&shim, "#!/bin/sh\nexec /bin/cat\n").expect("shim");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod shim");
    }

    let _daemon = KillOnDrop(
        Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
            .env("HOME", &home)
            .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
            .env("K2_TEST_AGENT_SHIM_DIR", &shim_dir)
            .env("K2SO_WAKE_HEADLESS_TEST_COMMAND", "cat")
            .env("K2SO_WATCHDOG_DISABLED", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn daemon"),
    );

    let db = wait_for_db(&k2_dir, Instant::now() + Duration::from_secs(60));

    // A workspace with a manager agent and a 60 s heartbeat created NOW.
    let project = scratch("ws");
    let agent_dir = project.join(".k2so/agents/manager");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");
    std::fs::write(
        agent_dir.join("AGENT.md"),
        "---\nname: manager\ntype: manager\n---\n# manager\n",
    )
    .expect("AGENT.md");
    std::fs::write(project.join("WAKEUP.md"), "---\nname: t3\n---\nSay hello.\n").expect("WAKEUP.md");
    let ws_id = "hb-t3-ws";
    let created = {
        let conn = open(&db);
        conn.execute(
            // agent_enabled = 1 with the mode, as every production
            // mode-setting path writes them (pinned delivery checks it).
            "INSERT INTO projects (id, path, name, agent_mode, agent_enabled) \
             VALUES (?1, ?2, 't3', 'manager', 1)",
            rusqlite::params![ws_id, project.to_string_lossy().as_ref()],
        )
        .expect("insert project");
        conn.execute(
            "INSERT INTO workspace_heartbeats \
             (id, project_id, name, frequency, spec_json, wakeup_path, enabled, created_at, use_workspace_session) \
             VALUES ('hb-t3', ?1, 't3', 'hourly', '{\"every_seconds\":60}', 'WAKEUP.md', 1, unixepoch(), 1)",
            rusqlite::params![ws_id],
        )
        .expect("insert heartbeat");
        Instant::now()
    };

    // Poll the fire history. Nothing outside the daemon ticks it.
    let deadline = created + Duration::from_secs(150);
    let fired_after = loop {
        let conn = open(&db);
        let fired: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM heartbeat_fires \
                 WHERE project_id = ?1 AND schedule_name = 't3' AND decision IN ('fired', 'fired_catchup')",
                rusqlite::params![ws_id],
                |r| r.get(0),
            )
            .expect("count fires");
        if fired >= 1 {
            break created.elapsed();
        }
        if Instant::now() >= deadline {
            let rows: Vec<(String, Option<String>)> = conn
                .prepare("SELECT decision, reason FROM heartbeat_fires WHERE project_id = ?1")
                .expect("prepare")
                .query_map(rusqlite::params![ws_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .expect("query")
                .collect::<rusqlite::Result<_>>()
                .expect("rows");
            panic!("no fire within 150 s of creating a 60 s heartbeat; fire rows: {rows:?}");
        }
        drop(conn);
        std::thread::sleep(Duration::from_secs(2));
    };
    assert!(
        fired_after >= Duration::from_secs(55),
        "the first fire must wait for the first slot (created + 60 s), fired after {fired_after:?}",
    );

    let conn = open(&db);
    let meta = |key: &str| -> Option<String> {
        conn.query_row("SELECT value FROM scheduler_meta WHERE key = ?1", rusqlite::params![key], |r| r.get(0))
            .ok()
    };
    assert!(meta("last_daemon_tick_at").is_some(), "the daemon must stamp its own ticks (HB9)");
    assert_eq!(meta("last_os_tick_at"), None, "no external ticker ran in this test");
    drop(conn);

    let _ = std::fs::remove_dir_all(&project);
    let _ = std::fs::remove_dir_all(&home);
}
