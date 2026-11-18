//! Read-only calendar links, S1 storage (prd-google-calendar-link-v1
//! §3.2/§3.3, migration 0316): which calendars a `calendar-read` link
//! exposes, and each workspace's "omit event descriptions" choice.
//!
//! **Calendars per link.** A link stored `calendar-read` exposes its
//! primary calendar (always) plus the calendars the owner opted in here.
//! Reads (`list`, `events`, `show`) on any other calendar answer the masked
//! `not_found`. A link stored `calendar` (0.46.1, read-write) keeps every
//! listed calendar readable and never reads this table (GC-23).
//!
//! **Omit descriptions.** Per workspace (the primary has no grant row, so
//! it's keyed by project). When on, `show`, `event` and `events --full`
//! answer `description: null` with `redacted: ["description"]`; the meeting
//! link and meeting id are still read out of the description (Q5, yes), and
//! nothing else from it leaves.
//!
//! Both go with their link: `inbox_id` cascades from
//! `mail_external_inboxes`, and [`purge_link`] runs in the unlink code too.

use k2_core::db::shared;

/// The account's own calendar: always on for a `calendar-read` link.
pub const PRIMARY: &str = "primary";

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// One opted-in calendar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptedIn {
    pub calendar_id: String,
    pub name: Option<String>,
    pub added_by: String,
    pub added_at: i64,
}

/// The calendars the owner opted in on `inbox_id` (the primary isn't
/// listed: it's always on).
pub fn opted_in(inbox_id: &str) -> Result<Vec<OptedIn>, String> {
    let db = shared();
    let conn = db.lock();
    let mut st = conn
        .prepare(
            "SELECT calendar_id, name, added_by, added_at FROM calendar_link_calendars \
             WHERE inbox_source = 'linked' AND inbox_id = ?1 ORDER BY added_at, calendar_id",
        )
        .map_err(|e| format!("calendar opt-ins: {e}"))?;
    let rows = st
        .query_map(rusqlite::params![inbox_id], |r| {
            Ok(OptedIn { calendar_id: r.get(0)?, name: r.get(1)?, added_by: r.get(2)?, added_at: r.get(3)? })
        })
        .map_err(|e| format!("calendar opt-ins: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("calendar opt-ins: {e}"))
}

/// Opt a calendar in. `Ok(false)` = it already was.
pub fn add_calendar(inbox_id: &str, calendar_id: &str, name: Option<&str>, added_by: &str) -> Result<bool, String> {
    let db = shared();
    let conn = db.lock();
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO calendar_link_calendars \
             (inbox_source, inbox_id, calendar_id, name, added_by, added_at) \
             VALUES ('linked', ?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![inbox_id, calendar_id, name, added_by, now_secs()],
        )
        .map_err(|e| format!("calendar opt-in: {e}"))?;
    Ok(n > 0)
}

/// Opt a calendar out. `Ok(false)` = it wasn't opted in.
pub fn remove_calendar(inbox_id: &str, calendar_id: &str) -> Result<bool, String> {
    let db = shared();
    let conn = db.lock();
    let n = conn
        .execute(
            "DELETE FROM calendar_link_calendars \
             WHERE inbox_source = 'linked' AND inbox_id = ?1 AND calendar_id = ?2",
            rusqlite::params![inbox_id, calendar_id],
        )
        .map_err(|e| format!("calendar opt-out: {e}"))?;
    Ok(n > 0)
}

/// Does `project_id`'s grant on `inbox_id` omit descriptions? A read
/// failure answers `Err` (the caller refuses rather than show them).
pub fn omit_description(inbox_id: &str, project_id: &str) -> Result<bool, String> {
    let db = shared();
    let conn = db.lock();
    omit_description_conn(&conn, inbox_id, project_id)
}

/// [`omit_description`] under a held connection.
pub fn omit_description_conn(conn: &rusqlite::Connection, inbox_id: &str, project_id: &str) -> Result<bool, String> {
    match conn.query_row(
        "SELECT omit_description FROM calendar_grant_prefs \
         WHERE inbox_source = 'linked' AND inbox_id = ?1 AND project_id = ?2",
        rusqlite::params![inbox_id, project_id],
        |r| r.get::<_, i64>(0),
    ) {
        Ok(v) => Ok(v != 0),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(e) => Err(format!("calendar privacy: {e}")),
    }
}

/// Set `project_id`'s omit-descriptions choice on `inbox_id`. `Ok(true)` =
/// it changed.
pub fn set_omit_description(inbox_id: &str, project_id: &str, on: bool, set_by: &str) -> Result<bool, String> {
    let db = shared();
    let conn = db.lock();
    let before = omit_description_conn(&conn, inbox_id, project_id)?;
    conn.execute(
        "INSERT INTO calendar_grant_prefs \
         (inbox_source, inbox_id, project_id, omit_description, set_by, set_at) \
         VALUES ('linked', ?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT (inbox_source, inbox_id, project_id) DO UPDATE SET \
         omit_description = excluded.omit_description, set_by = excluded.set_by, set_at = excluded.set_at",
        rusqlite::params![inbox_id, project_id, on as i64, set_by, now_secs()],
    )
    .map_err(|e| format!("calendar privacy: {e}"))?;
    Ok(before != on)
}

/// Drop a workspace's pref on a link (its grant was revoked).
pub fn drop_pref(conn: &rusqlite::Connection, inbox_id: &str, project_id: &str) {
    if let Err(e) = conn.execute(
        "DELETE FROM calendar_grant_prefs WHERE inbox_source = 'linked' AND inbox_id = ?1 AND project_id = ?2",
        rusqlite::params![inbox_id, project_id],
    ) {
        k2_core::log_debug!("[mail/calendar_link] dropping a calendar pref: {e}");
    }
}

/// Everything 0316 holds for a link (unlink). The FK cascade does the
/// same; this keeps unlink correct on its own.
pub fn purge_link(conn: &rusqlite::Connection, inbox_id: &str) -> Result<(), String> {
    for t in ["calendar_link_calendars", "calendar_grant_prefs"] {
        conn.execute(
            &format!("DELETE FROM {t} WHERE inbox_source = 'linked' AND inbox_id = ?1"),
            rusqlite::params![inbox_id],
        )
        .map_err(|e| format!("unlink {t}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed() -> (String, String, String) {
        let project = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        let inbox = uuid::Uuid::new_v4().to_string();
        let db = shared();
        let conn = db.lock();
        for p in [&project, &other] {
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?1, ?2)",
                rusqlite::params![p, format!("/tmp/cal-link-{p}")],
            )
            .expect("project");
        }
        conn.execute(
            "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, kind, host, port, \
             username, created_at, primary_level, auth_kind, provider, features) \
             VALUES (?1, ?2, ?3, 'imap', 'imap.gmail.com', 993, ?3, 1, 'draft', 'oauth', 'gmail', 'calendar-read')",
            rusqlite::params![inbox, project, format!("p-{}@gmail.com", &inbox[..8])],
        )
        .expect("link");
        (project, other, inbox)
    }

    #[test]
    fn opt_ins_add_list_remove() {
        let (_p, _o, inbox) = seed();
        assert!(opted_in(&inbox).unwrap().is_empty(), "only the primary until the owner adds one");
        assert!(add_calendar(&inbox, "team@group.calendar.google.com", Some("Consults"), "owner-token").unwrap());
        assert!(!add_calendar(&inbox, "team@group.calendar.google.com", Some("Consults"), "owner-token").unwrap());
        let list = opted_in(&inbox).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].calendar_id, "team@group.calendar.google.com");
        assert_eq!(list[0].name.as_deref(), Some("Consults"));
        assert!(remove_calendar(&inbox, "team@group.calendar.google.com").unwrap());
        assert!(!remove_calendar(&inbox, "team@group.calendar.google.com").unwrap());
        assert!(opted_in(&inbox).unwrap().is_empty());
    }

    #[test]
    fn omit_description_is_per_workspace() {
        let (p, o, inbox) = seed();
        assert!(!omit_description(&inbox, &p).unwrap(), "descriptions show by default");
        assert!(set_omit_description(&inbox, &p, true, "owner-token").unwrap());
        assert!(!set_omit_description(&inbox, &p, true, "owner-token").unwrap(), "no change");
        assert!(omit_description(&inbox, &p).unwrap());
        assert!(!omit_description(&inbox, &o).unwrap(), "another workspace is untouched");
        assert!(set_omit_description(&inbox, &p, false, "user:ana").unwrap());
        assert!(!omit_description(&inbox, &p).unwrap());
    }

    /// The coordinator's check: unlinking (deleting the inbox row) leaves
    /// nothing behind, by the FK cascade and by `purge_link`; a deleted
    /// workspace drops its own pref.
    #[test]
    fn unlink_and_workspace_delete_leave_nothing() {
        let count = |inbox: &str| -> i64 {
            let db = shared();
            let conn = db.lock();
            let a: i64 = conn
                .query_row("SELECT COUNT(*) FROM calendar_link_calendars WHERE inbox_id = ?1", [inbox], |r| r.get(0))
                .unwrap();
            let b: i64 = conn
                .query_row("SELECT COUNT(*) FROM calendar_grant_prefs WHERE inbox_id = ?1", [inbox], |r| r.get(0))
                .unwrap();
            a + b
        };
        // Cascade: delete the link row only.
        let (p, o, inbox) = seed();
        add_calendar(&inbox, "c1", None, "owner-token").unwrap();
        set_omit_description(&inbox, &p, true, "owner-token").unwrap();
        set_omit_description(&inbox, &o, true, "owner-token").unwrap();
        assert_eq!(count(&inbox), 3);
        {
            let db = shared();
            let conn = db.lock();
            conn.execute("DELETE FROM projects WHERE id = ?1", [&o]).unwrap();
        }
        assert_eq!(count(&inbox), 2, "the deleted workspace's pref went with it");
        {
            let db = shared();
            let conn = db.lock();
            conn.execute("DELETE FROM mail_external_inboxes WHERE id = ?1", [&inbox]).unwrap();
        }
        assert_eq!(count(&inbox), 0, "the FK cascade cleans a deleted link");
        // purge_link on its own.
        let (p, _o, inbox) = seed();
        add_calendar(&inbox, "c1", None, "owner-token").unwrap();
        set_omit_description(&inbox, &p, true, "owner-token").unwrap();
        {
            let db = shared();
            let conn = db.lock();
            purge_link(&conn, &inbox).unwrap();
        }
        assert_eq!(count(&inbox), 0);
    }
}
