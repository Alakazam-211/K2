//! The orchestrator seat: the one row that says which workspace is **K5**,
//! K2's own agent, and which is the person's **Chief of Staff**, K5's first
//! hire (prd-setup-checklist-v1 §10; the daemon–app contract is
//! `k5-first-run-contract.md`, migration 0293).
//!
//! - **One row** (`id = 1`). Written only by the owner or through the
//!   `/cli/first-run/*` routes (`k2-daemon` `first_run_routes`).
//! - **The name** "K5" lives in `display_name` and nowhere else: the SQL
//!   default and [`DEFAULT_NAME`] are the same word.
//! - **Stale ids:** an id whose `projects` row is gone counts as empty, so
//!   deleting K5's workspace clears the seat and the offer comes back. The
//!   readers check; nothing rewrites the row behind the person's back.

use rusqlite::{params, Connection, OptionalExtension};

/// The seat's default name (the SQL default in 0293 is the same).
pub const DEFAULT_NAME: &str = "K5";

/// K5's default handle and folder name under the hire root.
pub const DEFAULT_HANDLE: &str = "k5";

/// The row as stored. Ids may be stale; use [`live_k5`] / [`live_chief_of_staff`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seat {
    pub workspace_id: Option<String>,
    pub display_name: String,
    pub chief_of_staff_id: Option<String>,
    pub created_by: Option<String>,
    pub created_at: Option<i64>,
    pub opt_out: bool,
}

/// A live workspace the seat names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Live {
    pub workspace_id: String,
    /// K5: the seat's `display_name`; the Chief of Staff: its workspace name.
    pub name: String,
    pub path: String,
}

/// The stored row, or `None` before K5 was ever made.
pub fn load(conn: &Connection) -> rusqlite::Result<Option<Seat>> {
    conn.query_row(
        "SELECT workspace_id, display_name, chief_of_staff_id, created_by, created_at, opt_out FROM orchestrator WHERE id = 1",
        [],
        |r| {
            Ok(Seat {
                workspace_id: r.get(0)?,
                display_name: r.get(1)?,
                chief_of_staff_id: r.get(2)?,
                created_by: r.get(3)?,
                created_at: r.get(4)?,
                opt_out: r.get::<_, i64>(5)? != 0,
            })
        },
    )
    .optional()
}

/// `(name, path)` of a registered workspace, `None` when its row is gone.
fn project(conn: &Connection, id: &str) -> rusqlite::Result<Option<(String, String)>> {
    conn.query_row("SELECT name, path FROM projects WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
}

/// K5, when the seat names a workspace that still exists.
pub fn live_k5(conn: &Connection) -> rusqlite::Result<Option<Live>> {
    let Some(seat) = load(conn)? else { return Ok(None) };
    let Some(id) = seat.workspace_id.filter(|s| !s.trim().is_empty()) else { return Ok(None) };
    Ok(project(conn, &id)?.map(|(_, path)| Live { workspace_id: id, name: seat.display_name, path }))
}

/// The Chief of Staff, when the seat names a workspace that still exists.
pub fn live_chief_of_staff(conn: &Connection) -> rusqlite::Result<Option<Live>> {
    let Some(seat) = load(conn)? else { return Ok(None) };
    let Some(id) = seat.chief_of_staff_id.filter(|s| !s.trim().is_empty()) else { return Ok(None) };
    Ok(project(conn, &id)?.map(|(name, path)| Live { workspace_id: id, name, path }))
}

/// Seat K5: `workspace_id`, its name, who made it and when. Replaces a
/// stale K5 (the routes refuse while a live one sits there). A Chief of
/// Staff already recorded stays.
pub fn record_k5(conn: &Connection, workspace_id: &str, name: &str, created_by: &str, now: i64) -> rusqlite::Result<()> {
    let name = if name.trim().is_empty() { DEFAULT_NAME } else { name.trim() };
    conn.execute(
        "INSERT INTO orchestrator (id, workspace_id, display_name, created_by, created_at) VALUES (1, ?1, ?2, ?3, ?4) \
         ON CONFLICT(id) DO UPDATE SET workspace_id = excluded.workspace_id, display_name = excluded.display_name, \
         created_by = excluded.created_by, created_at = excluded.created_at",
        params![workspace_id, name, created_by, now],
    )?;
    Ok(())
}

/// Record K5's first hire. The seat must exist (K5 made it).
pub fn record_chief_of_staff(conn: &Connection, workspace_id: &str) -> rusqlite::Result<usize> {
    conn.execute("UPDATE orchestrator SET chief_of_staff_id = ?1 WHERE id = 1", params![workspace_id])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open(":memory:").expect("open");
        crate::db::run_migrations(&conn).expect("migrate");
        conn
    }

    fn project_row(conn: &Connection, id: &str) {
        conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, ?1, ?2)", params![id, format!("/tmp/seat-{id}")])
            .expect("project");
    }

    #[test]
    fn an_empty_seat_has_no_k5_and_the_name_defaults_to_k5() {
        let conn = db();
        assert_eq!(load(&conn).expect("load"), None);
        assert_eq!(live_k5(&conn).expect("k5"), None);
        conn.execute("INSERT INTO orchestrator (id) VALUES (1)", []).expect("bare row");
        assert_eq!(load(&conn).expect("load").expect("row").display_name, DEFAULT_NAME, "the SQL default");
        assert!(conn.execute("INSERT INTO orchestrator (id) VALUES (2)", []).is_err(), "one row only");
    }

    #[test]
    fn deleting_k5s_workspace_clears_the_seat_and_its_chief_of_staff_likewise() {
        let conn = db();
        project_row(&conn, "w-k5");
        project_row(&conn, "w-cos");
        record_k5(&conn, "w-k5", "K5", "owner", 100).expect("seat");
        assert_eq!(record_chief_of_staff(&conn, "w-cos").expect("cos"), 1);
        let k5 = live_k5(&conn).expect("k5").expect("live");
        assert_eq!((k5.workspace_id.as_str(), k5.name.as_str(), k5.path.as_str()), ("w-k5", "K5", "/tmp/seat-w-k5"));
        assert_eq!(live_chief_of_staff(&conn).expect("cos").expect("live").workspace_id, "w-cos");

        conn.execute("DELETE FROM projects WHERE id = 'w-cos'", []).expect("delete");
        assert_eq!(live_chief_of_staff(&conn).expect("cos"), None, "a stale Chief of Staff counts as none");
        conn.execute("DELETE FROM projects WHERE id = 'w-k5'", []).expect("delete");
        assert_eq!(live_k5(&conn).expect("k5"), None, "a stale K5 counts as none");
        assert_eq!(load(&conn).expect("load").expect("row").workspace_id.as_deref(), Some("w-k5"), "the row itself is kept");

        // A new K5 takes the seat over the stale id.
        project_row(&conn, "w-k5b");
        record_k5(&conn, "w-k5b", "Kay", "login:julie", 200).expect("reseat");
        let seat = load(&conn).expect("load").expect("row");
        assert_eq!((seat.workspace_id.as_deref(), seat.display_name.as_str()), (Some("w-k5b"), "Kay"));
        assert_eq!((seat.created_by.as_deref(), seat.created_at), (Some("login:julie"), Some(200)));
    }
}
