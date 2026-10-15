//! Held App requests (`app_requests`, 0168; prd-k2-app-runtime-v1 §5.6).
//!
//! Owners widen, agents propose. A change an agent (or a non-owner login)
//! may not make at once is stored here, held, with a plain-words diff.
//! Only the owner approves, with a fresh confirm (vault action
//! `app-approve`, bound to the request id); an Admin may deny; the
//! requesting workspace may withdraw its own. Held requests expire after
//! seven days. The daemon applies the stored body on approve.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

pub const KIND_CONFIG: &str = "config";
pub const KIND_PUBLISH_RUN: &str = "publish_run";
pub const KIND_APP_KEY: &str = "app_key";
pub const KIND_CLEAN_MODE: &str = "clean_mode";

pub const STATE_HELD: &str = "held";
pub const STATE_APPROVED: &str = "approved";
pub const STATE_DENIED: &str = "denied";
pub const STATE_WITHDRAWN: &str = "withdrawn";
pub const STATE_EXPIRED: &str = "expired";
pub const STATE_FAILED: &str = "failed";

/// Seven days.
pub const TTL_SECS: i64 = 7 * 24 * 3600;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRequest {
    pub id: String,
    pub kind: String,
    pub service_id: Option<String>,
    pub app_label: String,
    pub workspace_id: String,
    pub body: Value,
    pub diff: Value,
    pub reasons: Vec<String>,
    pub requested_by: String,
    pub state: String,
    pub result: Option<Value>,
    pub decided_by: Option<String>,
    pub decided_via: Option<String>,
    pub decided_at: Option<i64>,
    pub reason: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

/// What a caller proposes.
pub struct NewRequest<'a> {
    pub kind: &'a str,
    pub service_id: Option<&'a str>,
    pub app_label: &'a str,
    pub workspace_id: &'a str,
    pub body: &'a Value,
    pub diff: &'a Value,
    pub reasons: &'a [String],
    pub requested_by: &'a str,
}

const COLS: &str = "id, kind, service_id, app_label, workspace_id, body_json, diff_json, reasons_json, requested_by, state, \
                    result_json, decided_by, decided_via, decided_at, reason, created_at, expires_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AppRequest> {
    let parse = |s: String| serde_json::from_str::<Value>(&s).unwrap_or(Value::Null);
    Ok(AppRequest {
        id: r.get(0)?,
        kind: r.get(1)?,
        service_id: r.get(2)?,
        app_label: r.get(3)?,
        workspace_id: r.get(4)?,
        body: parse(r.get(5)?),
        diff: parse(r.get(6)?),
        reasons: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
        requested_by: r.get(8)?,
        state: r.get(9)?,
        result: r.get::<_, Option<String>>(10)?.map(parse),
        decided_by: r.get(11)?,
        decided_via: r.get(12)?,
        decided_at: r.get(13)?,
        reason: r.get(14)?,
        created_at: r.get(15)?,
        expires_at: r.get(16)?,
    })
}

/// Store a held request. A held request with the same kind, App and body
/// from the same requester is returned instead of a duplicate (agents
/// retry; the owner sees one card).
pub fn create(conn: &Connection, n: &NewRequest<'_>, now: i64) -> rusqlite::Result<AppRequest> {
    expire_due(conn, now)?;
    let body = serde_json::to_string(n.body).unwrap_or_else(|_| "{}".into());
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM app_requests WHERE kind = ?1 AND workspace_id = ?2 AND app_label = ?3 AND body_json = ?4 \
             AND requested_by = ?5 AND state = 'held'",
            params![n.kind, n.workspace_id, n.app_label, body, n.requested_by],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return get(conn, &id).map(|o| o.expect("row just found"));
    }
    let id = format!("ar_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    conn.execute(
        &format!(
            "INSERT INTO app_requests ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'held', NULL, NULL, NULL, NULL, NULL, ?10, ?11)"
        ),
        params![
            id,
            n.kind,
            n.service_id,
            n.app_label,
            n.workspace_id,
            body,
            serde_json::to_string(n.diff).unwrap_or_else(|_| "{}".into()),
            serde_json::to_string(n.reasons).unwrap_or_else(|_| "[]".into()),
            n.requested_by,
            now,
            now + TTL_SECS,
        ],
    )?;
    get(conn, &id).map(|o| o.expect("row just inserted"))
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<AppRequest>> {
    conn.query_row(&format!("SELECT {COLS} FROM app_requests WHERE id = ?1"), params![id.trim()], row).optional()
}

/// Mark every held request past its expiry `expired`.
pub fn expire_due(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE app_requests SET state = 'expired', decided_at = ?1, decided_by = 'system' WHERE state = 'held' AND expires_at <= ?1",
        params![now],
    )
}

/// Requests, newest first. `workspace` narrows to one workspace (a
/// passport's own); `all = false` shows only held ones.
pub fn list(conn: &Connection, workspace: Option<&str>, service: Option<&str>, all: bool, now: i64) -> rusqlite::Result<Vec<AppRequest>> {
    expire_due(conn, now)?;
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM app_requests WHERE (?1 IS NULL OR workspace_id = ?1) AND (?2 IS NULL OR service_id = ?2) \
         AND (?3 = 1 OR state = 'held') ORDER BY created_at DESC, id DESC LIMIT 200"
    ))?;
    let rows = st.query_map(params![workspace, service, all as i64], row)?;
    rows.collect()
}

/// Move a held request to `state`. `Err("not_held")` when it was already
/// decided (or expired) — the caller answers 409.
pub fn decide(
    conn: &Connection,
    id: &str,
    state: &str,
    by: &str,
    via: Option<&str>,
    reason: Option<&str>,
    result: Option<&Value>,
    now: i64,
) -> Result<AppRequest, &'static str> {
    expire_due(conn, now).map_err(|_| "db")?;
    let n = conn
        .execute(
            "UPDATE app_requests SET state = ?2, decided_by = ?3, decided_via = ?4, reason = ?5, result_json = ?6, decided_at = ?7 \
             WHERE id = ?1 AND state = 'held'",
            params![id, state, by, via, reason, result.map(|r| r.to_string()), now],
        )
        .map_err(|_| "db")?;
    if n == 0 {
        return Err("not_held");
    }
    get(conn, id).map_err(|_| "db")?.ok_or("not_held")
}

/// Record the outcome of applying an approved request.
pub fn set_result(conn: &Connection, id: &str, state: &str, result: &Value) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE app_requests SET state = ?2, result_json = ?3 WHERE id = ?1",
        params![id, state, result.to_string()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        let sql = include_str!("../../drizzle_sql/0168_app_grace.sql");
        // Only the two new tables (the ALTERs need the full schema).
        for stmt in sql.split("--> statement-breakpoint") {
            if stmt.contains("CREATE TABLE") || stmt.contains("app_requests_ws_state") {
                c.execute_batch(stmt).unwrap();
            }
        }
        c
    }

    #[test]
    fn held_requests_dedupe_decide_once_and_expire() {
        let c = conn();
        let body = serde_json::json!({"egress": {"add": ["api.stripe.com:443"]}});
        let diff = serde_json::json!({"egress": "+ api.stripe.com:443"});
        let reasons = vec!["new egress host".to_string()];
        let n = NewRequest {
            kind: KIND_CONFIG,
            service_id: Some("svc-1"),
            app_label: "jobs",
            workspace_id: "ws-1",
            body: &body,
            diff: &diff,
            reasons: &reasons,
            requested_by: "agent:nsi",
        };
        let a = create(&c, &n, 100).unwrap();
        let b = create(&c, &n, 101).unwrap();
        assert_eq!(a.id, b.id, "the same proposal from the same agent is one request");
        assert_eq!(a.state, STATE_HELD);
        assert_eq!(list(&c, Some("ws-1"), None, false, 102).unwrap().len(), 1);
        assert!(list(&c, Some("ws-2"), None, false, 102).unwrap().is_empty(), "another workspace sees nothing");
        let d = decide(&c, &a.id, STATE_DENIED, "owner", None, Some("no"), None, 103).unwrap();
        assert_eq!(d.state, STATE_DENIED);
        assert_eq!(decide(&c, &a.id, STATE_APPROVED, "owner", None, None, None, 104).unwrap_err(), "not_held", "decided once");
        let e = create(&c, &NewRequest { app_label: "other", ..n }, 200).unwrap();
        assert_eq!(expire_due(&c, 200 + TTL_SECS).unwrap(), 1);
        assert_eq!(get(&c, &e.id).unwrap().unwrap().state, STATE_EXPIRED);
        assert_eq!(decide(&c, &e.id, STATE_APPROVED, "owner", None, None, None, 200 + TTL_SECS + 1).unwrap_err(), "not_held");
    }
}
