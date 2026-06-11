//! `auto_pin_existing_agents_0_39_0` — pin existing agent-mode
//! workspaces on upgrade to 0.39.0.
//!
//! Pre-0.39.0 the sidebar auto-promoted every workspace with
//! `agent_mode in ('agent', 'custom', 'manager', 'coordinator', 'pod')`
//! to a dedicated "Agents & Pinned" section above the user's manually
//! pinned workspaces. That auto-promotion was retired in 0.39.0 —
//! agent-mode workspaces now flow through the same Pinned / focus
//! group / ungrouped lists as any other workspace.
//!
//! Without a migration, the visible effect would be: every workspace
//! that previously appeared in the auto-promoted Agents section
//! *disappears* from the top of the user's nav on first 0.39.0 launch
//! (they're still there, just in the ungrouped section or their focus
//! group — but the user doesn't know that). This migration prevents
//! that surprise by flipping `pinned = 1` for every agent-mode
//! workspace that wasn't already pinned, so they keep appearing in
//! the Pinned section.
//!
//! Users who DON'T want agent-mode workspaces in their Pinned section
//! can unpin via the existing UI affordance (right-click → Unpin).
//! Future workspaces switched into agent mode (post-0.39.0) do NOT
//! auto-pin — they flow through the normal sections like any other
//! workspace.
//!
//! Gated by the `code_migrations` table (one-shot per local DB).

use rusqlite::Connection;

/// Stable id stored in the `code_migrations` table once this migration
/// completes for the local DB. Used by the daemon-side boot caller;
/// exposed as a `pub const` so call sites + tests agree.
pub const MIGRATION_ID: &str = "auto_pin_existing_agents_0_39_0";

/// Outcome of running the migration. Carries a count of workspaces
/// whose `pinned` column flipped from 0 to 1.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutoPinOutcome {
    /// Number of project rows where `pinned` flipped to 1. Idempotent
    /// re-runs return 0.
    pub pinned_count: usize,
}

/// Run the migration against the given DB connection. Pure SQL — no
/// filesystem work. Idempotent: workspaces already pinned (or not in
/// an agent mode) are untouched. Returns the count of newly-pinned
/// rows.
///
/// The agent-mode set matches the pre-0.39.0 auto-promote filter in
/// the sidebar: any workspace whose `agent_mode` was one of `agent`,
/// `custom`, `manager`, `coordinator`, or `pod` was rendered in the
/// dedicated Agents section. Those are the workspaces that need to
/// be pinned manually now that the auto-promotion is gone.
pub fn run(conn: &Connection) -> Result<AutoPinOutcome, rusqlite::Error> {
    // 0.39.1 fix: original 0.39.0 ship included manager / coordinator
    // / pod in the filter, but the pre-0.39.0 sidebar only auto-
    // promoted `agent` (K2SO Agent) and `custom` (Custom Agent) — not
    // manager-mode workspaces. Filter narrowed to match the original
    // auto-promote scope. See correct_auto_pin_filter_0_39_1 for the
    // corrective migration that unpins manager/coordinator/pod for
    // users who already ran the over-broad 0.39.0 version.
    let changed = conn.execute(
        "UPDATE projects \
         SET pinned = 1 \
         WHERE pinned = 0 \
           AND agent_mode IN ('agent', 'custom')",
        [],
    )?;
    Ok(AutoPinOutcome { pinned_count: changed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn make_test_db() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        // Minimal projects schema — just the columns the migration touches.
        // Real schema has many more columns; the migration only reads
        // `pinned` and `agent_mode` and writes `pinned`.
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

    fn pinned_count_for(conn: &Connection, agent_mode: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM projects WHERE pinned = 1 AND agent_mode = ?1",
            rusqlite::params![agent_mode],
            |row| row.get(0),
        )
        .expect("count pinned")
    }

    #[test]
    fn empty_db_returns_zero() {
        let conn = make_test_db();
        let outcome = run(&conn).expect("run migration");
        assert_eq!(outcome.pinned_count, 0);
    }

    #[test]
    fn off_workspaces_are_not_pinned() {
        let conn = make_test_db();
        insert_project(&conn, "ws-off", 0, "off");
        let outcome = run(&conn).expect("run migration");
        assert_eq!(outcome.pinned_count, 0);
        assert_eq!(pinned_count_for(&conn, "off"), 0);
    }

    #[test]
    fn agent_workspace_gets_pinned() {
        let conn = make_test_db();
        insert_project(&conn, "ws-agent", 0, "agent");
        let outcome = run(&conn).expect("run migration");
        assert_eq!(outcome.pinned_count, 1);
        assert_eq!(pinned_count_for(&conn, "agent"), 1);
    }

    #[test]
    fn custom_workspace_gets_pinned() {
        let conn = make_test_db();
        insert_project(&conn, "ws-custom", 0, "custom");
        let outcome = run(&conn).expect("run migration");
        assert_eq!(outcome.pinned_count, 1);
        assert_eq!(pinned_count_for(&conn, "custom"), 1);
    }

    #[test]
    fn manager_coordinator_pod_workspaces_are_NOT_pinned() {
        // 0.39.1 narrowed the filter — manager-mode workspaces never
        // auto-promoted pre-0.39.0 so they shouldn't be pinned by
        // this migration. (See correct_auto_pin_filter_0_39_1 for the
        // corrective migration that fixes the buggy 0.39.0 ship.)
        let conn = make_test_db();
        insert_project(&conn, "ws-manager", 0, "manager");
        insert_project(&conn, "ws-coordinator", 0, "coordinator");
        insert_project(&conn, "ws-pod", 0, "pod");
        let outcome = run(&conn).expect("run migration");
        assert_eq!(outcome.pinned_count, 0);
    }

    #[test]
    fn already_pinned_agent_is_not_double_counted() {
        let conn = make_test_db();
        insert_project(&conn, "ws-already-pinned", 1, "agent");
        let outcome = run(&conn).expect("run migration");
        // Idempotent: 0 rows changed because the row was already pinned.
        assert_eq!(outcome.pinned_count, 0);
        assert_eq!(pinned_count_for(&conn, "agent"), 1);
    }

    #[test]
    fn mixed_modes_pins_only_agent_and_custom() {
        let conn = make_test_db();
        insert_project(&conn, "off1", 0, "off");
        insert_project(&conn, "off2", 0, "off");
        insert_project(&conn, "agent1", 0, "agent");
        insert_project(&conn, "custom1", 0, "custom");
        insert_project(&conn, "manager1", 0, "manager");
        let outcome = run(&conn).expect("run migration");
        // Only agent1 + custom1 — manager1 stays unpinned (0.39.1 narrowed filter)
        assert_eq!(outcome.pinned_count, 2);
        // off + manager workspaces stay unpinned
        let unpinned: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE pinned = 0",
                [],
                |row| row.get(0),
            )
            .expect("count unpinned");
        assert_eq!(unpinned, 3); // off1, off2, manager1
    }

    #[test]
    fn second_run_is_idempotent_no_op() {
        let conn = make_test_db();
        insert_project(&conn, "agent1", 0, "agent");
        insert_project(&conn, "custom1", 0, "custom");
        let first = run(&conn).expect("run migration first time");
        assert_eq!(first.pinned_count, 2);
        let second = run(&conn).expect("run migration second time");
        assert_eq!(second.pinned_count, 0);
    }
}
