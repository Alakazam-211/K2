//! Home chat pushes' own rows in skin.db (prd-home-chat-pushes-v1 §7;
//! vs-live HC-2, HC-20). Idempotent DDL in the `ensure_schema` family that
//! `skin::open_db` runs (no k2.db migration):
//!
//! - `home_chat_feed_cursor`: where each box person's Sew feed poller has
//!   read to. Written only after a page's pushes went (a crash repeats a
//!   push, never loses one), and forgotten whenever Sew's data goes back
//!   under K2 (`home_chat::sew_data_reset`): a restored Sew's seqs are lower,
//!   and a cursor above its head would silently push nothing.
//! - `home_chat_push_prefs`: each person's own choices, "Show message text"
//!   (`previews`, default on) and "Also notify me for agents' posts"
//!   (`agents`, default off, D4), and whether K2 already set their K5 Space
//!   default to @mentions once (`space_default_set`, HC-20: never again,
//!   never over a choice). Keyed by the person's external id
//!   (`k2:owner`, `k2:connect:<uuid>`).
//!
//! Ids, numbers and switches only: never message text.

use rusqlite::{params, Connection, OptionalExtension};

/// Idempotent DDL, called from `skin::open_db`.
pub fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS home_chat_feed_cursor (
            ext        TEXT PRIMARY KEY NOT NULL,
            cursor     INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS home_chat_push_prefs (
            principal         TEXT PRIMARY KEY NOT NULL,
            previews          INTEGER NOT NULL DEFAULT 1,
            agents            INTEGER NOT NULL DEFAULT 0,
            space_default_set INTEGER NOT NULL DEFAULT 0,
            updated_at        INTEGER NOT NULL,
            updated_by        TEXT NOT NULL DEFAULT ''
         );",
    )
    .map_err(|e| format!("skin.db home chat push schema: {e}"))
}

/// `ext`'s stored feed cursor (`None` = never read here, or forgotten).
pub fn cursor(conn: &Connection, ext: &str) -> Result<Option<u64>, String> {
    conn.query_row("SELECT cursor FROM home_chat_feed_cursor WHERE ext = ?1", params![ext], |r| r.get::<_, i64>(0))
        .optional()
        .map(|o| o.map(|c| c.max(0) as u64))
        .map_err(|e| format!("feed cursor read: {e}"))
}

/// Store `ext`'s feed cursor.
pub fn set_cursor(conn: &Connection, ext: &str, cursor: u64, now: i64) -> Result<(), String> {
    let c = i64::try_from(cursor).map_err(|_| format!("feed cursor {cursor} is out of range"))?;
    conn.execute(
        "INSERT INTO home_chat_feed_cursor (ext, cursor, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(ext) DO UPDATE SET cursor = excluded.cursor, updated_at = excluded.updated_at",
        params![ext, c, now],
    )
    .map(|_| ())
    .map_err(|e| format!("feed cursor write: {e}"))
}

/// Forget every stored cursor (Sew's data was reset). Returns how many.
pub fn forget_cursors(conn: &Connection) -> Result<usize, String> {
    conn.execute("DELETE FROM home_chat_feed_cursor", []).map_err(|e| format!("feed cursor reset: {e}"))
}

/// One person's own push choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushPrefs {
    /// "Show message text" (D6): off = no text in the push at all, seal
    /// included.
    pub previews: bool,
    /// "Also notify me for agents' posts" (D4).
    pub agents: bool,
    /// K2 set the K5 Space default to @mentions once (HC-20).
    pub space_default_set: bool,
}

impl Default for PushPrefs {
    fn default() -> Self {
        Self { previews: true, agents: false, space_default_set: false }
    }
}

/// `principal`'s choices (the defaults when nothing is stored).
pub fn prefs(conn: &Connection, principal: &str) -> Result<PushPrefs, String> {
    conn.query_row(
        "SELECT previews, agents, space_default_set FROM home_chat_push_prefs WHERE principal = ?1",
        params![principal],
        |r| Ok(PushPrefs { previews: r.get::<_, i64>(0)? != 0, agents: r.get::<_, i64>(1)? != 0, space_default_set: r.get::<_, i64>(2)? != 0 }),
    )
    .optional()
    .map(Option::unwrap_or_default)
    .map_err(|e| format!("push prefs read: {e}"))
}

/// Change `principal`'s own choices (`None` keeps one). Returns them after.
pub fn set_prefs(conn: &Connection, principal: &str, previews: Option<bool>, agents: Option<bool>, by: &str, now: i64) -> Result<PushPrefs, String> {
    let was = prefs(conn, principal)?;
    let next = PushPrefs { previews: previews.unwrap_or(was.previews), agents: agents.unwrap_or(was.agents), ..was };
    write(conn, principal, next, by, now)?;
    Ok(next)
}

/// Record that K2 set `principal`'s K5 Space default (or found it chosen).
pub fn mark_space_default_set(conn: &Connection, principal: &str, now: i64) -> Result<(), String> {
    let was = prefs(conn, principal)?;
    write(conn, principal, PushPrefs { space_default_set: true, ..was }, "k2", now)
}

fn write(conn: &Connection, principal: &str, p: PushPrefs, by: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO home_chat_push_prefs (principal, previews, agents, space_default_set, updated_at, updated_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(principal) DO UPDATE SET previews = excluded.previews, agents = excluded.agents,
           space_default_set = excluded.space_default_set, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
        params![principal, p.previews as i64, p.agents as i64, p.space_default_set as i64, now, by],
    )
    .map(|_| ())
    .map_err(|e| format!("push prefs write: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        ensure_schema(&c).unwrap();
        ensure_schema(&c).expect("idempotent");
        c
    }

    /// HC-2: a cursor is stored per person, moves, and is forgotten on a Sew
    /// data reset.
    #[test]
    fn the_feed_cursor_is_stored_and_forgotten() {
        let c = db();
        assert_eq!(cursor(&c, "k2:owner").unwrap(), None, "never read");
        set_cursor(&c, "k2:owner", 41, 1).unwrap();
        set_cursor(&c, "k2:owner", 57, 2).unwrap();
        set_cursor(&c, "k2:connect:a", 3, 2).unwrap();
        assert_eq!(cursor(&c, "k2:owner").unwrap(), Some(57));
        assert_eq!(cursor(&c, "k2:connect:a").unwrap(), Some(3));
        assert_eq!(forget_cursors(&c).unwrap(), 2);
        assert_eq!(cursor(&c, "k2:owner").unwrap(), None, "forgotten");
        assert!(set_cursor(&c, "k2:owner", u64::MAX, 3).is_err(), "out of range is loud");
    }

    /// D4, D6, HC-20: previews default on, agents off; a change keeps the
    /// other; the Space default mark never resets a choice.
    #[test]
    fn push_prefs_default_and_change() {
        let c = db();
        assert_eq!(prefs(&c, "k2:owner").unwrap(), PushPrefs { previews: true, agents: false, space_default_set: false });
        let p = set_prefs(&c, "k2:owner", None, Some(true), "owner", 1).unwrap();
        assert_eq!(p, PushPrefs { previews: true, agents: true, space_default_set: false });
        let p = set_prefs(&c, "k2:owner", Some(false), None, "owner", 2).unwrap();
        assert_eq!(p, PushPrefs { previews: false, agents: true, space_default_set: false });
        mark_space_default_set(&c, "k2:owner", 3).unwrap();
        assert_eq!(prefs(&c, "k2:owner").unwrap(), PushPrefs { previews: false, agents: true, space_default_set: true });
        assert_eq!(prefs(&c, "k2:connect:b").unwrap(), PushPrefs::default(), "another person: defaults");
    }
}
