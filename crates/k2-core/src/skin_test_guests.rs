//! Test guests (0.46.4, NSI B): short-lived guests to test one App with.
//! A person makes them, as every guest (whether an App's own agent may is
//! parked for its own arc; the daemon route refuses agents). A test guest
//! is an App-only guest (one `kind=app` grant on that App, no box login) plus a
//! row here, and K2 holds it to the narrowest shape:
//!
//! - **one App, one door:** a session is minted for it only for its App
//!   and only while the login runs on that App's gateway link
//!   ([`with_link_login`]); the box login, another App, the browser helper
//!   and device sign-in (refresh tokens) all refuse it like a bad password;
//! - **short-lived:** it ends at `expires_at` (24 h by default, at most
//!   [`MAX_TTL_SECS`]); a session never outlives it (its expiry is capped
//!   at mint), and an expired one can't sign in;
//! - **few:** at most [`MAX_PER_APP`] live ones per App;
//! - **visible:** `k2 app user list` shows the `test` flag.
//!
//! Who may make one, the roles it may get, the audit and the Thread note
//! are the daemon route's (`apps::test_guests`). The table is additive in skin.db
//! (created at open, no numbered step), like `app_login_settings`.

use std::cell::RefCell;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::{now_secs, with_conn};

/// Default life of a test guest.
pub const DEFAULT_TTL_SECS: i64 = 24 * 3600;
/// Longest life a test guest may be given.
pub const MAX_TTL_SECS: i64 = 7 * 24 * 3600;
/// Shortest.
pub const MIN_TTL_SECS: i64 = 5 * 60;
/// Live test guests per App.
pub const MAX_PER_APP: usize = 5;

/// One test guest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestGuest {
    pub principal_id: String,
    /// The App's published service id (what its grant and sessions name).
    pub app: String,
    pub workspace_id: String,
    pub created_by: String,
    pub created_at: i64,
    pub expires_at: i64,
}

impl TestGuest {
    pub fn expired_at(&self, now: i64) -> bool {
        now >= self.expires_at
    }
}

/// skin.db DDL (run by the idempotent open block).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_test_guests (
            principal_id TEXT PRIMARY KEY REFERENCES principals(id) ON DELETE CASCADE,
            app_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            created_by TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_app_test_guests_app ON app_test_guests (app_id);",
    )
    .map_err(|e| format!("skin.db app_test_guests: {e}"))
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TestGuest> {
    Ok(TestGuest {
        principal_id: r.get(0)?,
        app: r.get(1)?,
        workspace_id: r.get(2)?,
        created_by: r.get(3)?,
        created_at: r.get(4)?,
        expires_at: r.get(5)?,
    })
}

const COLS: &str = "principal_id, app_id, workspace_id, created_by, created_at, expires_at";

/// The test-guest row of a principal on an open connection.
pub(crate) fn get_conn(conn: &Connection, principal_id: &str) -> Result<Option<TestGuest>, String> {
    conn.query_row(&format!("SELECT {COLS} FROM app_test_guests WHERE principal_id = ?1"), params![principal_id], row)
        .optional()
        .map_err(|e| format!("test guest lookup: {e}"))
}

/// The test-guest row of a principal (`None`: a real person).
pub fn get(principal_id: &str) -> Result<Option<TestGuest>, String> {
    with_conn(|conn| get_conn(conn, principal_id))
}

/// Every test guest (one App's, or all), oldest first.
pub fn list(app: Option<&str>) -> Result<Vec<TestGuest>, String> {
    with_conn(|conn| {
        let mut st = conn
            .prepare(&format!("SELECT {COLS} FROM app_test_guests WHERE (?1 IS NULL OR app_id = ?1) ORDER BY created_at, principal_id"))
            .map_err(|e| format!("test guest list: {e}"))?;
        let rows = st.query_map(params![app], row).map_err(|e| format!("test guest list: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("test guest list: {e}"))
    })
}

/// Live (unexpired) test guests of an App.
pub fn live_count(app: &str) -> Result<usize, String> {
    let now = now_secs();
    Ok(list(Some(app))?.into_iter().filter(|t| !t.expired_at(now)).count())
}

/// Mark `principal_id` a test guest of `app` until `expires_at`. The
/// principal and its App grant are the caller's (`add_principal_for_app`).
pub fn record(principal_id: &str, app: &str, workspace_id: &str, created_by: &str, expires_at: i64) -> Result<TestGuest, String> {
    let created_at = now_secs();
    with_conn(|conn| {
        conn.execute(
            &format!("INSERT INTO app_test_guests ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"),
            params![principal_id, app, workspace_id, created_by, created_at, expires_at],
        )
        .map_err(|e| format!("test guest insert: {e}"))?;
        Ok(())
    })?;
    Ok(TestGuest {
        principal_id: principal_id.to_string(),
        app: app.to_string(),
        workspace_id: workspace_id.to_string(),
        created_by: created_by.to_string(),
        created_at,
        expires_at,
    })
}

/// Usernames of test guests that expired (the daemon removes them).
pub fn expired_usernames(app: Option<&str>) -> Result<Vec<String>, String> {
    let now = now_secs();
    with_conn(|conn| {
        let mut st = conn
            .prepare(
                "SELECT p.username FROM app_test_guests t JOIN principals p ON p.id = t.principal_id \
                 WHERE t.expires_at <= ?1 AND (?2 IS NULL OR t.app_id = ?2) ORDER BY p.username",
            )
            .map_err(|e| format!("test guest sweep: {e}"))?;
        let rows = st.query_map(params![now, app], |r| r.get::<_, String>(0)).map_err(|e| format!("test guest sweep: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("test guest sweep: {e}"))
    })
}

thread_local! {
    /// The App whose gateway link the login on this thread came through.
    static LINK_LOGIN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Run a login (`f`) knowing it came through `app`'s gateway link (`None`
/// = any other door). Only inside this may a test guest of `app` get a
/// session. Always clears afterwards.
pub fn with_link_login<T>(app: Option<&str>, f: impl FnOnce() -> T) -> T {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            LINK_LOGIN.with(|c| *c.borrow_mut() = None);
        }
    }
    let _clear = Clear;
    LINK_LOGIN.with(|c| *c.borrow_mut() = app.map(str::to_string));
    f()
}

/// May a test guest get a session for `app` now? `Ok(expires_at)` = yes,
/// capped at that time; `Err` = refuse it like a bad password.
pub(crate) fn session_allowed(t: &TestGuest, app: &str, now: i64) -> Result<i64, &'static str> {
    if t.expired_at(now) {
        return Err("test guest expired");
    }
    if t.app != app {
        return Err("a test guest signs in to its own App only");
    }
    let on_link = LINK_LOGIN.with(|c| c.borrow().clone());
    if on_link.as_deref() != Some(t.app.as_str()) {
        return Err("a test guest signs in only through its App's gateway link");
    }
    Ok(t.expires_at)
}

/// `24h`, `90m`, `7d`, `3600` (seconds): the life asked for, clamped to
/// [`MIN_TTL_SECS`]…[`MAX_TTL_SECS`] (`Err` past the maximum: said, never
/// silently shortened).
pub fn parse_ttl(raw: Option<&str>) -> Result<i64, String> {
    let Some(t) = raw.map(str::trim).filter(|t| !t.is_empty()) else { return Ok(DEFAULT_TTL_SECS) };
    let secs = if let Ok(n) = t.parse::<i64>() {
        n
    } else {
        let (num, unit) = t.split_at(t.len() - 1);
        let n: i64 = num.parse().map_err(|_| format!("expires is 30m, 24h, 7d or seconds, not '{t}'"))?;
        n.saturating_mul(match unit {
            "s" => 1,
            "m" => 60,
            "h" => 3600,
            "d" => 86_400,
            _ => return Err(format!("expires is 30m, 24h, 7d or seconds, not '{t}'")),
        })
    };
    if secs > MAX_TTL_SECS {
        return Err(format!("a test guest lives at most 7 days ('{t}' is longer)"));
    }
    Ok(secs.max(MIN_TTL_SECS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_defaults_and_limits() {
        assert_eq!(parse_ttl(None).unwrap(), 24 * 3600);
        assert_eq!(parse_ttl(Some("2h")).unwrap(), 7200);
        assert_eq!(parse_ttl(Some("7d")).unwrap(), MAX_TTL_SECS);
        assert!(parse_ttl(Some("8d")).is_err(), "past 7 days is refused, never shortened");
        assert_eq!(parse_ttl(Some("1m")).unwrap(), MIN_TTL_SECS);
        assert!(parse_ttl(Some("soon")).is_err());
    }

    #[test]
    fn a_session_needs_its_app_its_link_and_time_left() {
        let t = TestGuest {
            principal_id: "p".into(),
            app: "svc-a".into(),
            workspace_id: "ws".into(),
            created_by: "agent:ws".into(),
            created_at: 100,
            expires_at: 1_000,
        };
        assert!(session_allowed(&t, "svc-a", 500).is_err(), "not on a link");
        with_link_login(Some("svc-b"), || assert!(session_allowed(&t, "svc-a", 500).is_err(), "another App's link"));
        with_link_login(Some("svc-a"), || {
            assert_eq!(session_allowed(&t, "svc-a", 500), Ok(1_000));
            assert!(session_allowed(&t, "host", 500).is_err(), "never the box login");
            assert!(session_allowed(&t, "svc-a", 1_000).is_err(), "expired");
        });
        assert!(session_allowed(&t, "svc-a", 500).is_err(), "cleared after the login");
    }
}
