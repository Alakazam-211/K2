//! Compute package requests on the controller (0.46.9,
//! prd-compute-self-serve-environments-v1 §6, §13; migration 0271).
//!
//! Three origins share one table:
//! - `own`: this server's agent asked for a node this server owns. The
//!   owner (or an Admin) decides here, on a card or in Settings → Compute →
//!   Approvals; an approval sends `packages.allow {packages, install,
//!   request}` to the node, where root's guards run.
//! - `sent`: this server asked another server's node. The node relays it
//!   to its owner; the outcome comes back as `pkg_request_done`.
//! - `fwd`: another server asked for a node this server owns (relayed by
//!   the node). Decided here like `own`.
//!
//! A batch (one card) is every row filed together; deciding the batch
//! decides each pending row in it.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::proto::pkgpolicy::{self, DrySummary};

/// Origins.
pub const OWN: &str = "own";
pub const SENT: &str = "sent";
pub const FWD: &str = "fwd";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PkgRow {
    pub id: String,
    pub batch_id: String,
    pub origin: String,
    pub node_id: String,
    pub node_request: Option<String>,
    pub server: String,
    pub workspace_id: Option<String>,
    pub session_id: Option<String>,
    pub job_id: Option<String>,
    pub asked_by: String,
    pub why: Option<String>,
    pub names: Vec<String>,
    pub natives: serde_json::Value,
    pub dry: Option<DrySummary>,
    pub host: Option<String>,
    pub state: String,
    pub add_to_list: bool,
    pub decided_by: Option<String>,
    pub decided_at: Option<i64>,
    pub note: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

const COLS: &str = "id, batch_id, origin, node_id, node_request, server, workspace_id, session_id, job_id, asked_by, why, names_json, natives_json, dry_json, host, state, add_to_list, decided_by, decided_at, note, created_at, expires_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PkgRow> {
    let names: String = r.get(11)?;
    let natives: String = r.get(12)?;
    let dry: Option<String> = r.get(13)?;
    Ok(PkgRow {
        id: r.get(0)?,
        batch_id: r.get(1)?,
        origin: r.get(2)?,
        node_id: r.get(3)?,
        node_request: r.get(4)?,
        server: r.get(5)?,
        workspace_id: r.get(6)?,
        session_id: r.get(7)?,
        job_id: r.get(8)?,
        asked_by: r.get(9)?,
        why: r.get(10)?,
        names: serde_json::from_str(&names).unwrap_or_default(),
        natives: serde_json::from_str(&natives).unwrap_or(serde_json::Value::Null),
        dry: dry.and_then(|d| serde_json::from_str(&d).ok()),
        host: r.get(14)?,
        state: r.get(15)?,
        add_to_list: r.get::<_, i64>(16)? != 0,
        decided_by: r.get(17)?,
        decided_at: r.get(18)?,
        note: r.get(19)?,
        created_at: r.get(20)?,
        expires_at: r.get(21)?,
    })
}

/// A new id (`cpr_…`).
pub fn new_id() -> String {
    format!("cpr_{}", uuid::Uuid::new_v4().simple().to_string().chars().take(16).collect::<String>())
}

/// What a new row says.
#[derive(Debug, Clone, Default)]
pub struct NewRow<'a> {
    pub batch_id: &'a str,
    pub origin: &'a str,
    pub node_id: &'a str,
    pub node_request: Option<&'a str>,
    pub server: &'a str,
    pub workspace_id: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub job_id: Option<&'a str>,
    pub asked_by: &'a str,
    pub why: Option<&'a str>,
    pub names: &'a [String],
    pub natives: serde_json::Value,
    pub dry: Option<&'a DrySummary>,
    pub host: Option<&'a str>,
    pub now: i64,
    pub expires_at: Option<i64>,
}

/// Open (pending) rows for one node from one workspace (the 10-per-server
/// cap is the node's; this one keeps one workspace from flooding the owner).
pub fn open_count(conn: &Connection, node_id: &str, workspace_id: Option<&str>) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM compute_pkg_requests WHERE node_id = ?1 AND state = 'pending' AND (?2 IS NULL OR workspace_id = ?2)",
        params![node_id, workspace_id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

pub fn insert(conn: &Connection, n: &NewRow<'_>) -> Result<PkgRow, String> {
    let id = new_id();
    conn.execute(
        "INSERT INTO compute_pkg_requests (id, batch_id, origin, node_id, node_request, server, workspace_id, session_id, job_id, asked_by, why, \
         names_json, natives_json, dry_json, host, state, created_at, expires_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 'pending', ?16, ?17)",
        params![
            id,
            n.batch_id,
            n.origin,
            n.node_id,
            n.node_request,
            n.server,
            n.workspace_id,
            n.session_id,
            n.job_id,
            n.asked_by,
            n.why,
            serde_json::to_string(n.names).unwrap_or_else(|_| "[]".into()),
            n.natives.to_string(),
            n.dry.map(|d| serde_json::to_string(d).unwrap_or_default()),
            n.host,
            n.now,
            n.expires_at.unwrap_or(n.now + pkgpolicy::REQUEST_TTL_SECS),
        ],
    )
    .map_err(|e| e.to_string())?;
    get(conn, &id).ok_or_else(|| "the request row vanished".to_string())
}

pub fn get(conn: &Connection, id: &str) -> Option<PkgRow> {
    conn.query_row(&format!("SELECT {COLS} FROM compute_pkg_requests WHERE id = ?1"), params![id], row).optional().ok().flatten()
}

/// Every row of a batch (or the one row when `id` is a row id).
pub fn batch(conn: &Connection, id: &str) -> Vec<PkgRow> {
    let mut st = match conn.prepare(&format!("SELECT {COLS} FROM compute_pkg_requests WHERE batch_id = ?1 OR id = ?1 ORDER BY created_at, id")) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    st.query_map(params![id], row).map(|it| it.flatten().collect()).unwrap_or_default()
}

/// A row by the node's id for it, for one node.
pub fn by_node_request(conn: &Connection, node_id: &str, node_request: &str) -> Option<PkgRow> {
    conn.query_row(
        &format!("SELECT {COLS} FROM compute_pkg_requests WHERE node_id = ?1 AND node_request = ?2"),
        params![node_id, node_request],
        row,
    )
    .optional()
    .ok()
    .flatten()
}

/// Filters for [`list`].
#[derive(Debug, Clone, Default)]
pub struct Filter<'a> {
    pub node_id: Option<&'a str>,
    pub workspace_id: Option<&'a str>,
    /// `None` = every state.
    pub state: Option<&'a str>,
    /// Rows this server's people decide (`own` + `fwd`).
    pub decidable_only: bool,
    pub limit: i64,
}

pub fn list(conn: &Connection, f: &Filter<'_>) -> Vec<PkgRow> {
    let sql = format!(
        "SELECT {COLS} FROM compute_pkg_requests WHERE (?1 IS NULL OR node_id = ?1) AND (?2 IS NULL OR workspace_id = ?2) \
         AND (?3 IS NULL OR state = ?3) AND (?4 = 0 OR origin IN ('own','fwd')) ORDER BY created_at DESC, id LIMIT ?5"
    );
    let mut st = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    st.query_map(params![f.node_id, f.workspace_id, f.state, i64::from(f.decidable_only), f.limit.clamp(1, 500)], row)
        .map(|it| it.flatten().collect())
        .unwrap_or_default()
}

/// Move a row to `state` (from pending or approved only).
pub fn set_state(conn: &Connection, id: &str, state: &str, by: Option<&str>, note: Option<&str>, add: Option<bool>, now: i64) -> bool {
    conn.execute(
        "UPDATE compute_pkg_requests SET state = ?2, decided_by = COALESCE(?3, decided_by), decided_at = COALESCE(decided_at, ?4), \
         note = COALESCE(?5, note), add_to_list = COALESCE(?6, add_to_list) WHERE id = ?1 AND state IN ('pending','approved')",
        params![id, state, by, now, note, add.map(i64::from)],
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// The node's id and its dry run, when the node answers.
pub fn set_node_info(conn: &Connection, id: &str, node_request: &str, dry: Option<&DrySummary>, host: Option<&str>) {
    let _ = conn.execute(
        "UPDATE compute_pkg_requests SET node_request = ?2, dry_json = COALESCE(?3, dry_json), host = COALESCE(?4, host) WHERE id = ?1",
        params![id, node_request, dry.map(|d| serde_json::to_string(d).unwrap_or_default()), host],
    );
}

/// Pending rows past their end: expired. Returns them (for the agents).
pub fn expire(conn: &Connection, now: i64) -> Vec<PkgRow> {
    let rows = list(conn, &Filter { state: Some(pkgpolicy::state::PENDING), limit: 500, ..Default::default() });
    rows.into_iter().filter(|r| r.expires_at <= now && set_state(conn, &r.id, pkgpolicy::state::EXPIRED, None, Some("nobody answered within 24 h"), None, now)).collect()
}

/// The card's and Settings' one line: "Install 3 packages on k2n11
/// (Ubuntu 24.04) for acme.k2.dev's agent tests: a, b, c (42 with
/// dependencies, 180 MB)".
pub fn summary(rows: &[PkgRow], node_names: &dyn Fn(&str) -> String) -> String {
    let mut names: Vec<String> = rows.iter().flat_map(|r| r.names.clone()).collect();
    names.sort();
    names.dedup();
    let mut nodes: Vec<String> = rows.iter().map(|r| {
        let n = node_names(&r.node_id);
        match &r.host {
            Some(h) => format!("{n} ({h})"),
            None => n,
        }
    }).collect();
    nodes.dedup();
    let who = rows.first().map(|r| if r.server == "own" { format!("agent {}", r.asked_by) } else { format!("{}'s {}", r.server, r.asked_by) }).unwrap_or_default();
    let deps: usize = rows.iter().filter_map(|r| r.dry.as_ref()).map(|d| d.installs.len()).sum();
    let mb: u64 = rows.iter().filter_map(|r| r.dry.as_ref().and_then(|d| d.size_mb)).sum();
    let extra = if deps > 0 { format!(" ({deps} packages with dependencies{})", if mb > 0 { format!(", {mb} MB") } else { String::new() }) } else { String::new() };
    format!(
        "Install {} package{} on {} for {who}: {}{extra}",
        names.len(),
        if names.len() == 1 { "" } else { "s" },
        nodes.join(", "),
        names.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../drizzle_sql/0271_compute_pkg_requests.sql")).unwrap();
        conn
    }

    #[test]
    fn rows_batch_decide_and_expire() {
        let conn = db();
        let names = vec!["libgtk-3-dev".to_string(), "libssl-dev".to_string()];
        let dry = DrySummary { installs: vec!["a".into(), "b".into(), "c".into()], size_mb: Some(180), ..Default::default() };
        let a = insert(&conn, &NewRow { batch_id: "cpr_b1", origin: OWN, node_id: "n1", server: "own", asked_by: "tests", names: &names, dry: Some(&dry), host: Some("Ubuntu 24.04.1 LTS"), now: 100, ..Default::default() }).unwrap();
        let b = insert(&conn, &NewRow { batch_id: "cpr_b1", origin: OWN, node_id: "n2", server: "own", asked_by: "tests", names: &names, now: 100, ..Default::default() }).unwrap();
        assert_eq!(batch(&conn, "cpr_b1").len(), 2);
        assert_eq!(batch(&conn, &a.id).len(), 1, "a row id names just the row");
        assert_eq!(open_count(&conn, "n1", None), 1);
        let s = summary(&batch(&conn, "cpr_b1"), &|id| id.to_uppercase());
        assert_eq!(s, "Install 2 packages on N1 (Ubuntu 24.04.1 LTS), N2 for agent tests: libgtk-3-dev, libssl-dev (3 packages with dependencies, 180 MB)");
        assert!(set_state(&conn, &a.id, "approved", Some("owner"), None, Some(true), 150));
        assert!(set_state(&conn, &a.id, "installed", None, None, None, 160));
        assert!(!set_state(&conn, &a.id, "denied", Some("x"), None, None, 170), "a decided row stays decided");
        let gone = expire(&conn, 100 + pkgpolicy::REQUEST_TTL_SECS + 1);
        assert_eq!(gone.iter().map(|r| r.id.clone()).collect::<Vec<_>>(), vec![b.id.clone()]);
        assert_eq!(get(&conn, &b.id).unwrap().state, "expired");
        let r = get(&conn, &a.id).unwrap();
        assert!(r.add_to_list && r.decided_by.as_deref() == Some("owner") && r.state == "installed");
        set_node_info(&conn, &a.id, "npr_1", None, None);
        assert_eq!(by_node_request(&conn, "n1", "npr_1").unwrap().id, a.id);
    }
}
