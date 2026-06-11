//! `correct_auto_pin_filter_0_39_1` — unpin manager/coordinator/pod
//! workspaces over-pinned by the buggy 0.39.0 ship.
//!
//! 0.39.0 shipped [`super::auto_pin_existing_agents_0_39_0`] with an
//! over-broad filter that pinned `('agent', 'custom', 'manager',
//! 'coordinator', 'pod')` workspaces. The pre-0.39.0 sidebar only
//! auto-promoted `agent` (K2SO Agent) and `custom` (Custom Agent) —
//! manager-mode workspaces never appeared in the auto-promoted Agents
//! section. The over-pin surprised users by surfacing their manager
//! workspaces at the top of the Pinned list.
//!
//! This corrective migration unpins every `manager` / `coordinator` /
//! `pod` workspace that's currently pinned. The 0.39.0 migration
//! source has also been narrowed to only `('agent', 'custom')` so
//! users installing 0.39.1 fresh (skipping 0.39.0) get the right
//! behavior on first launch.
//!
//! **Trade-off acknowledged**: this also unpins any manager-mode
//! workspace a user manually pinned pre-0.39.0. We can't distinguish
//! "buggy migration pinned this" from "user manually pinned this," so
//! we unpin all manager-family rows and trust users to re-pin
//! intentional ones. The blast radius is small (very few users had
//! managers pinned pre-0.39.0; managers don't auto-promote and the
//! UI for pinning a manager is the same right-click as any
//! workspace).
//!
//! Gated by `code_migrations` (one-shot per local DB).

use rusqlite::Connection;

/// Stable id stored in the `code_migrations` table once this migration
/// completes for the local DB.
pub const MIGRATION_ID: &str = "correct_auto_pin_filter_0_39_1";

/// Outcome of running the migration. Carries a count of workspaces
/// whose `pinned` column flipped from 1 to 0.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CorrectAutoPinOutcome {
    /// Number of project rows where `pinned` flipped to 0. Idempotent
    /// re-runs return 0.
    pub unpinned_count: usize,
}

/// Run the corrective migration. Unpins every workspace currently
/// pinned with `agent_mode` in the manager family (manager /
/// coordinator / pod). Pure SQL; no filesystem work.
pub fn run(conn: &Connection) -> Result<CorrectAutoPinOutcome, rusqlite::Error> {
    let changed = conn.execute(
        "UPDATE projects \
         SET pinned = 0 \
         WHERE pinned = 1 \
           AND agent_mode IN ('manager', 'coordinator', 'pod')",
        [],
    )?;
    Ok(CorrectAutoPinOutcome { unpinned_count: changed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn make_test_db() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                pinned INTEGER NOT NULL DEFAULT 0,
                agent_mode TEXT NOT NULL DEFAULT 'off'
            );",
        )
        .expect("create projects table");
        conn
    }

    fn insert_project(conn: &Connection, id: &str, pinned: i64, agent_mode: &str) {
        conn.execute(
            "INSERT INTO projects (id, pinned, agent_mode) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, pinned, agent_mode],
        )
        .expect("insert project");
    }

    #[test]
    fn empty_db_returns_zero() {
        let conn = make_test_db();
        let outcome = run(&conn).expect("run migration");
        assert_eq!(outcome.unpinned_count, 0);
    }

    #[test]
    fn pinned_manager_gets_unpinned() {
        let conn = make_test_db();
        insert_project(&conn, "ws-manager", 1, "manager");
        let outcome = run(&conn).expect("run migration");
        assert_eq!(outcome.unpinned_count, 1);
        let still_pinned: i64 = conn
            .query_row(
                "SELECT pinned FROM projects WHERE id = 'ws-manager'",
                [],
                |row| row.get(0),
            )
            .expect("read pinned");
        assert_eq!(still_pinned, 0);
    }

    #[test]
    fn pinned_coordinator_gets_unpinned() {
        let conn = make_test_db();
        insert_project(&conn, "ws-coord", 1, "coordinator");
        let outcome = run(&conn).expect("run migration");
        assert_eq!(outcome.unpinned_count, 1);
    }

    #[test]
    fn pinned_pod_gets_unpinned() {
        let conn = make_test_db();
        insert_project(&conn, "ws-pod", 1, "pod");
        let outcome = run(&conn).expect("run migration");
        assert_eq!(outcome.unpinned_count, 1);
    }

    #[test]
    fn pinned_agent_stays_pinned() {
        let conn = make_test_db();
        insert_project(&conn, "ws-agent", 1, "agent");
        let outcome = run(&conn).expect("run migration");
        assert_eq!(outcome.unpinned_count, 0);
        let still_pinned: i64 = conn
            .query_row(
                "SELECT pinned FROM projects WHERE id = 'ws-agent'",
                [],
                |row| row.get(0),
            )
            .expect("read pinned");
        assert_eq!(still_pinned, 1);
    }

    #[test]
    fn pinned_custom_stays_pinned() {
        let conn = make_test_db();
        insert_project(&conn, "ws-custom", 1, "custom");
        let outcome = run(&conn).expect("run migration");
        assert_eq!(outcome.unpinned_count, 0);
    }

    #[test]
    fn unpinned_manager_is_no_op() {
        let conn = make_test_db();
        insert_project(&conn, "ws-manager", 0, "manager");
        let outcome = run(&conn).expect("run migration");
        assert_eq!(outcome.unpinned_count, 0);
    }

    #[test]
    fn second_run_is_idempotent() {
        let conn = make_test_db();
        insert_project(&conn, "ws-manager", 1, "manager");
        let first = run(&conn).expect("first run");
        assert_eq!(first.unpinned_count, 1);
        let second = run(&conn).expect("second run");
        assert_eq!(second.unpinned_count, 0);
    }

    #[test]
    fn mixed_only_manager_family_unpinned() {
        let conn = make_test_db();
        insert_project(&conn, "agent1", 1, "agent");
        insert_project(&conn, "custom1", 1, "custom");
        insert_project(&conn, "manager1", 1, "manager");
        insert_project(&conn, "coord1", 1, "coordinator");
        insert_project(&conn, "pod1", 1, "pod");
        insert_project(&conn, "off1", 0, "off");
        let outcome = run(&conn).expect("run migration");
        assert_eq!(outcome.unpinned_count, 3); // manager1 + coord1 + pod1
        // agent1 + custom1 stay pinned
        let still_pinned: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE pinned = 1",
                [],
                |row| row.get(0),
            )
            .expect("count pinned");
        assert_eq!(still_pinned, 2);
    }
}
