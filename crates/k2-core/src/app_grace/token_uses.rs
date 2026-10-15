//! Owner-token calls made by a published service (`app_owner_token_uses`).
//!
//! The daemon attributes a loopback request that carries the owner token
//! to the published service whose process made it (Linux: socket inode →
//! process → its service's session). One row per (service, route) with a
//! count, so the 0.46.0 warnings can say "your BFF called
//! `/cli/skin/users/rooms` with the owner token 41 times" before 0.46.1
//! takes the token out of reach. `refused` counts calls refused because
//! the service runs in clean mode.

use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUse {
    pub route: String,
    pub count: i64,
    pub refused: i64,
    pub first_at: i64,
    pub last_at: i64,
}

/// Routes are stored as their path only (never a query string, which may
/// carry the token itself), capped at 200 characters.
pub fn clean_route(path: &str) -> String {
    let p = path.split('?').next().unwrap_or("");
    p.chars().take(200).collect()
}

pub fn record(conn: &Connection, service_id: &str, path: &str, refused: bool, now: i64) -> rusqlite::Result<()> {
    let route = clean_route(path);
    conn.execute(
        "INSERT INTO app_owner_token_uses (service_id, route, count, refused, first_at, last_at) VALUES (?1, ?2, 1, ?3, ?4, ?4)
         ON CONFLICT(service_id, route) DO UPDATE SET count = count + 1, refused = refused + ?3, last_at = ?4",
        params![service_id, route, refused as i64, now],
    )?;
    Ok(())
}

pub fn list(conn: &Connection, service_id: &str) -> rusqlite::Result<Vec<TokenUse>> {
    let mut st = conn.prepare(
        "SELECT route, count, refused, first_at, last_at FROM app_owner_token_uses WHERE service_id = ?1 ORDER BY last_at DESC LIMIT 100",
    )?;
    let rows = st.query_map(params![service_id], |r| {
        Ok(TokenUse { route: r.get(0)?, count: r.get(1)?, refused: r.get(2)?, first_at: r.get(3)?, last_at: r.get(4)? })
    })?;
    rows.collect()
}

/// Forget a service's rows (it was removed, or the owner cleared them).
pub fn clear(conn: &Connection, service_id: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM app_owner_token_uses WHERE service_id = ?1", params![service_id])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_count_per_route_and_never_keep_the_query() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE app_owner_token_uses (service_id TEXT NOT NULL, route TEXT NOT NULL, count INTEGER NOT NULL DEFAULT 0,
             refused INTEGER NOT NULL DEFAULT 0, first_at INTEGER NOT NULL, last_at INTEGER NOT NULL, PRIMARY KEY (service_id, route));",
        )
        .unwrap();
        record(&c, "s1", "/cli/skin/users/rooms?token=SECRET", false, 10).unwrap();
        record(&c, "s1", "/cli/skin/users/rooms", true, 20).unwrap();
        record(&c, "s1", "/cli/thread", false, 15).unwrap();
        let v = list(&c, "s1").unwrap();
        assert_eq!(v.len(), 2);
        let rooms = v.iter().find(|u| u.route == "/cli/skin/users/rooms").unwrap();
        assert_eq!((rooms.count, rooms.refused, rooms.first_at, rooms.last_at), (2, 1, 10, 20));
        assert!(v.iter().all(|u| !u.route.contains("SECRET")), "the query (and any token in it) is never stored");
        assert_eq!(clear(&c, "s1").unwrap(), 2);
    }
}
