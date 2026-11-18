//! Signed-in guests per App right now (0.46.16): the owner's "who would a
//! restart sign out" read (`GET /cli/app/sessions/live`). Counts only.

use rusqlite::params;

use crate::skin::{with_conn, HOST_APP_ID};

/// Signed-in guests per App right now (0.46.16, the owner's "who would a
/// restart sign out" read): distinct people with a login session that is
/// neither revoked nor expired at `now`, by the App it is bound to (`host`
/// for the box login and a pre-0.45.5 row). Counts only, no names.
pub fn signed_in_guests_by_app(now: i64) -> Result<Vec<(String, u64)>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT COALESCE(NULLIF(app_service_id, ''), ?2) AS app, COUNT(DISTINCT principal_id)
                 FROM tokens
                 WHERE session = 1 AND revoked_at IS NULL AND principal_id IS NOT NULL
                   AND (expires_at IS NULL OR expires_at > ?1)
                 GROUP BY app ORDER BY app",
            )
            .map_err(|e| format!("skin signed-in count: {e}"))?;
        let rows = stmt
            .query_map(params![now, HOST_APP_ID], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| format!("skin signed-in count: {e}"))?;
        rows.map(|r| r.map(|(a, n)| (a, n.max(0) as u64)))
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| format!("skin signed-in count: {e}"))
    })
}
