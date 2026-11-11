//! The orchestrator seat: the one row that says which workspace is
//! **Kessel**, K2's own agent, and which is the person's **Chief of Staff**,
//! Kessel's first hire (prd-setup-checklist-v1 §10; the daemon–app contract
//! is `kessel-first-run-contract.md`, migration 0293).
//!
//! - **One row** (`id = 1`). Written only by the owner or through the
//!   `/cli/first-run/*` routes (`k2-daemon` `first_run_routes`).
//! - **The name** ("Kessel" by default, [`DEFAULT_NAME`]) lives in
//!   `display_name`. [`record_kessel`] always writes it; the SQL default in
//!   0293 is still the old name and is never relied on (a landed migration
//!   isn't edited).
//! - **Stale ids:** an id whose `projects` row is gone counts as empty, so
//!   deleting Kessel's workspace clears the seat and the offer comes back. The
//!   readers check; nothing rewrites the row behind the person's back.

use rusqlite::{params, Connection, OptionalExtension};

/// The seat's default name. The SQL default in 0293 is the old name; the
/// code always writes `display_name` itself, so that default never shows.
pub const DEFAULT_NAME: &str = "Kessel";

/// Kessel's default handle and folder name under the hire root.
pub const DEFAULT_HANDLE: &str = "kessel";

/// The one fallback when another workspace already holds [`DEFAULT_HANDLE`].
/// There is no third: with both held, making Kessel needs a chosen handle.
pub const FALLBACK_HANDLE: &str = "kessel5";

/// True when a registered workspace holds `handle`, as its handle or one of
/// its aliases (any case). A retired handle (a deleted workspace's) isn't
/// held: no agent answers to it.
pub fn handle_held(conn: &Connection, handle: &str) -> bool {
    let n: i64 = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM projects WHERE handle = ?1 COLLATE NOCASE) \
             + (SELECT COUNT(*) FROM project_handle_aliases a JOIN projects p ON p.id = a.project_id \
                WHERE a.alias = ?1 COLLATE NOCASE)",
            params![handle],
            |r| r.get(0),
        )
        .unwrap_or(0);
    n > 0
}

/// The handle a new Kessel gets when none is chosen: [`DEFAULT_HANDLE`],
/// else [`FALLBACK_HANDLE`]; `None` when another workspace holds both.
pub fn default_handle(conn: &Connection) -> Option<&'static str> {
    [DEFAULT_HANDLE, FALLBACK_HANDLE].into_iter().find(|h| !handle_held(conn, h))
}

/// A handle sent for Kessel that means "the default": none, `kessel`, or
/// the handle [`default_handle`] would pick (what the sheet prefilled).
pub fn is_default_handle(conn: &Connection, handle: Option<&str>) -> bool {
    match handle.map(str::trim).filter(|h| !h.is_empty()) {
        None => true,
        Some(h) => h.eq_ignore_ascii_case(DEFAULT_HANDLE) || default_handle(conn).is_some_and(|d| h.eq_ignore_ascii_case(d)),
    }
}

/// The row as stored. Ids may be stale; use [`live_kessel`] / [`live_chief_of_staff`].
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
    /// Kessel: the seat's `display_name`; the Chief of Staff: its workspace name.
    pub name: String,
    pub path: String,
}

/// The stored row, or `None` before Kessel was ever made.
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

/// Kessel, when the seat names a workspace that still exists.
pub fn live_kessel(conn: &Connection) -> rusqlite::Result<Option<Live>> {
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

/// Seat Kessel: `workspace_id`, its name, who made it and when. Replaces a
/// stale Kessel (the routes refuse while a live one sits there). A Chief of
/// Staff already recorded stays.
pub fn record_kessel(conn: &Connection, workspace_id: &str, name: &str, created_by: &str, now: i64) -> rusqlite::Result<()> {
    let name = if name.trim().is_empty() { DEFAULT_NAME } else { name.trim() };
    conn.execute(
        "INSERT INTO orchestrator (id, workspace_id, display_name, created_by, created_at) VALUES (1, ?1, ?2, ?3, ?4) \
         ON CONFLICT(id) DO UPDATE SET workspace_id = excluded.workspace_id, display_name = excluded.display_name, \
         created_by = excluded.created_by, created_at = excluded.created_at",
        params![workspace_id, name, created_by, now],
    )?;
    Ok(())
}

/// Record Kessel's first hire. The seat must exist (Kessel made it).
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
    fn an_empty_seat_has_no_kessel_and_the_name_defaults_to_kessel() {
        let conn = db();
        assert_eq!(load(&conn).expect("load"), None);
        assert_eq!(live_kessel(&conn).expect("kessel"), None);
        assert_eq!((DEFAULT_NAME, DEFAULT_HANDLE), ("Kessel", "kessel"));
        // A blank name is written as the default, never left to 0293's SQL default.
        project_row(&conn, "w-kessel");
        record_kessel(&conn, "w-kessel", "  ", "owner", 1).expect("seat");
        assert_eq!(load(&conn).expect("load").expect("row").display_name, "Kessel");
        assert!(conn.execute("INSERT INTO orchestrator (id) VALUES (2)", []).is_err(), "one row only");
    }

    #[test]
    fn the_default_handle_falls_back_to_kessel5_once_and_never_a_third() {
        let conn = db();
        assert_eq!(default_handle(&conn), Some("kessel"));
        assert!(is_default_handle(&conn, None) && is_default_handle(&conn, Some(" Kessel ")));
        assert!(!is_default_handle(&conn, Some("kessel5")), "kessel is free: kessel5 is a choice");
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES ('w-a', 'A', '/tmp/ha', 'kessel')", []).expect("a");
        assert!(handle_held(&conn, "KESSEL"));
        assert_eq!(default_handle(&conn), Some("kessel5"));
        assert!(is_default_handle(&conn, Some("kessel5")), "the prefilled fallback is still the default");
        assert!(!is_default_handle(&conn, Some("ops")));
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES ('w-b', 'B', '/tmp/hb', 'b')", []).expect("b");
        conn.execute("INSERT INTO project_handle_aliases (project_id, alias) VALUES ('w-b', 'kessel5')", []).expect("alias");
        assert_eq!(default_handle(&conn), None, "both held: no third name");
        // A deleted workspace's handle is retired, not held.
        conn.execute("DELETE FROM projects WHERE id = 'w-a'", []).expect("delete");
        assert!(!handle_held(&conn, "kessel"));
        assert_eq!(default_handle(&conn), Some("kessel"));
    }

    #[test]
    fn deleting_kessels_workspace_clears_the_seat_and_its_chief_of_staff_likewise() {
        let conn = db();
        project_row(&conn, "w-kessel");
        project_row(&conn, "w-cos");
        record_kessel(&conn, "w-kessel", "Kessel", "owner", 100).expect("seat");
        assert_eq!(record_chief_of_staff(&conn, "w-cos").expect("cos"), 1);
        let kessel = live_kessel(&conn).expect("kessel").expect("live");
        assert_eq!((kessel.workspace_id.as_str(), kessel.name.as_str(), kessel.path.as_str()), ("w-kessel", "Kessel", "/tmp/seat-w-kessel"));
        assert_eq!(live_chief_of_staff(&conn).expect("cos").expect("live").workspace_id, "w-cos");

        conn.execute("DELETE FROM projects WHERE id = 'w-cos'", []).expect("delete");
        assert_eq!(live_chief_of_staff(&conn).expect("cos"), None, "a stale Chief of Staff counts as none");
        conn.execute("DELETE FROM projects WHERE id = 'w-kessel'", []).expect("delete");
        assert_eq!(live_kessel(&conn).expect("kessel"), None, "a stale Kessel counts as none");
        assert_eq!(load(&conn).expect("load").expect("row").workspace_id.as_deref(), Some("w-kessel"), "the row itself is kept");

        // A new Kessel takes the seat over the stale id.
        project_row(&conn, "w-kesselb");
        record_kessel(&conn, "w-kesselb", "Kay", "login:julie", 200).expect("reseat");
        let seat = load(&conn).expect("load").expect("row");
        assert_eq!((seat.workspace_id.as_deref(), seat.display_name.as_str()), (Some("w-kesselb"), "Kay"));
        assert_eq!((seat.created_by.as_deref(), seat.created_at), (Some("login:julie"), Some(200)));
    }
}
