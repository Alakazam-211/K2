//! Held App requests (`app_widenings`, 0168; prd-k2-app-runtime-v1 §5.6).
//!
//! Owners widen, agents propose. One table and one flow for every kind of
//! held change:
//!
//! - 0.46.0 grace kinds: `config` (an S13 widening), `publish_run` (a new
//!   `--cmd` service when the cross-over switches are enforced), `app_key`
//!   (an App-scoped key mint), `clean_mode` (turning clean mode off);
//! - S15 egress kinds: `egress` (new hosts on an App's allowlist) and
//!   `egress_mode` (`broad`, the open path's).
//!
//! A request is `pending` until the server's owner approves it in person
//! (a fresh vault confirm, action `app-approve`, bound to the request id;
//! never a passport), an Owner/Admin human denies it, its requester
//! withdraws it, or seven days pass (`expired`). An approved request whose
//! change then fails to apply is `failed` with the error in `result`.
//!
//! Egress kinds carry their own side effects (allow the pending hosts, set
//! the mode, or release the pending rows): [`decide`] and [`expire_due`]
//! run them through `deploy::egress`, so every path that closes a request
//! keeps the host list consistent. The grace kinds' changes are applied by
//! the daemon after approval (it owns the services and the vault).

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

pub const KIND_CONFIG: &str = "config";
pub const KIND_PUBLISH_RUN: &str = "publish_run";
pub const KIND_APP_KEY: &str = "app_key";
pub const KIND_CLEAN_MODE: &str = "clean_mode";
pub const KIND_EGRESS: &str = "egress";
pub const KIND_EGRESS_MODE: &str = "egress_mode";

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_APPROVED: &str = "approved";
pub const STATUS_DENIED: &str = "denied";
pub const STATUS_WITHDRAWN: &str = "withdrawn";
pub const STATUS_EXPIRED: &str = "expired";
pub const STATUS_FAILED: &str = "failed";

/// Seven days (§5.6 step 6).
pub const TTL_SECS: i64 = 7 * 24 * 3600;

/// One held request. `app_id` is the runtime App (NULL for a `--cmd`
/// service that has no runtime App row); `service_id` the published
/// service; `workspace_id` the workspace that owns the App (a passport sees
/// only its own workspace's requests).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRequest {
    pub id: String,
    pub kind: String,
    pub app_id: Option<String>,
    pub service_id: Option<String>,
    pub app_label: String,
    pub workspace_id: String,
    /// What approving applies (grace kinds).
    pub body: Value,
    /// The change in plain words (a list of lines) or, for egress kinds,
    /// `{egress: {add: [...]}}` / `{egressMode: "broad"}`.
    pub diff: Value,
    pub summary: String,
    /// Why it was held (the classifier's words).
    pub reasons: Vec<String>,
    /// The requester's own why (`--reason`).
    pub reason: Option<String>,
    pub deployment_id: Option<String>,
    pub requested_by: String,
    pub requester_workspace: Option<String>,
    #[serde(skip)]
    pub requester_session: Option<String>,
    pub requested_at: i64,
    pub expires_at: i64,
    pub status: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<i64>,
    /// How the owner confirmed (Touch ID, a password).
    pub confirm_via: Option<String>,
    pub decision_reason: Option<String>,
    /// What applying an approved request did (or its error).
    pub result: Option<Value>,
}

/// What a caller proposes.
pub struct NewRequest<'a> {
    pub kind: &'a str,
    pub app_id: Option<&'a str>,
    pub service_id: Option<&'a str>,
    pub app_label: &'a str,
    pub workspace_id: &'a str,
    pub body: Value,
    pub diff: Value,
    pub summary: String,
    pub reasons: Vec<String>,
    pub reason: Option<&'a str>,
    pub deployment_id: Option<&'a str>,
    pub requested_by: &'a str,
    pub requester_workspace: Option<&'a str>,
    pub requester_session: Option<&'a str>,
}

const COLS: &str = "id, kind, app_id, service_id, app_label, workspace_id, body_json, diff_json, summary, reasons_json, reason, \
                    deployment_id, requested_by, requester_workspace, requester_session, requested_at, expires_at, status, \
                    decided_by, decided_at, confirm_via, decision_reason, result_json";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AppRequest> {
    let parse = |s: String| serde_json::from_str::<Value>(&s).unwrap_or(Value::Null);
    Ok(AppRequest {
        id: r.get(0)?,
        kind: r.get(1)?,
        app_id: r.get(2)?,
        service_id: r.get(3)?,
        app_label: r.get(4)?,
        workspace_id: r.get(5)?,
        body: parse(r.get(6)?),
        diff: parse(r.get(7)?),
        summary: r.get(8)?,
        reasons: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default(),
        reason: r.get(10)?,
        deployment_id: r.get(11)?,
        requested_by: r.get(12)?,
        requester_workspace: r.get(13)?,
        requester_session: r.get(14)?,
        requested_at: r.get(15)?,
        expires_at: r.get(16)?,
        status: r.get(17)?,
        decided_by: r.get(18)?,
        decided_at: r.get(19)?,
        confirm_via: r.get(20)?,
        decision_reason: r.get(21)?,
        result: r.get::<_, Option<String>>(22)?.map(parse),
    })
}

pub fn new_id() -> String {
    format!("w_{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

/// Store a new pending request as is.
pub fn insert(conn: &Connection, n: &NewRequest<'_>, now: i64) -> rusqlite::Result<AppRequest> {
    let id = new_id();
    conn.execute(
        &format!(
            "INSERT INTO app_widenings ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, \
             'pending', NULL, NULL, NULL, NULL, NULL)"
        ),
        params![
            id,
            n.kind,
            n.app_id,
            n.service_id,
            n.app_label,
            n.workspace_id,
            n.body.to_string(),
            n.diff.to_string(),
            n.summary,
            serde_json::to_string(&n.reasons).unwrap_or_else(|_| "[]".into()),
            n.reason,
            n.deployment_id,
            n.requested_by,
            n.requester_workspace,
            n.requester_session,
            now,
            now + TTL_SECS,
        ],
    )?;
    get(conn, &id).map(|o| o.expect("row just inserted"))
}

/// Store a held request. A pending request with the same kind, App and
/// body from the same requester is returned instead of a duplicate
/// (agents retry; the owner sees one card).
pub fn create(conn: &Connection, n: &NewRequest<'_>, now: i64) -> rusqlite::Result<AppRequest> {
    expire_due(conn, now)?;
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM app_widenings WHERE kind = ?1 AND workspace_id = ?2 AND app_label = ?3 AND body_json = ?4 \
             AND requested_by = ?5 AND status = 'pending'",
            params![n.kind, n.workspace_id, n.app_label, n.body.to_string(), n.requested_by],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return get(conn, &id).map(|o| o.expect("row just found"));
    }
    insert(conn, n, now)
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<AppRequest>> {
    conn.query_row(&format!("SELECT {COLS} FROM app_widenings WHERE id = ?1"), params![id.trim()], row).optional()
}

/// A request by id or a unique id prefix (at least 6 characters).
pub fn find(conn: &Connection, token: &str) -> rusqlite::Result<Option<AppRequest>> {
    let t = token.trim();
    if let Some(r) = get(conn, t)? {
        return Ok(Some(r));
    }
    if t.len() < 6 || !t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Ok(None);
    }
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM app_widenings WHERE id LIKE ?1 || '%' LIMIT 2"))?;
    let rows = st.query_map(params![t], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(if rows.len() == 1 { rows.into_iter().next() } else { None })
}

/// Which requests [`list`] returns. `None` = any.
#[derive(Debug, Default, Clone, Copy)]
pub struct Filter<'a> {
    pub app_id: Option<&'a str>,
    pub service_id: Option<&'a str>,
    /// The App's workspace (a passport's own).
    pub workspace: Option<&'a str>,
    pub status: Option<&'a str>,
}

/// Requests, newest first.
pub fn list(conn: &Connection, f: &Filter<'_>, limit: i64) -> rusqlite::Result<Vec<AppRequest>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM app_widenings WHERE (?1 IS NULL OR app_id = ?1) AND (?2 IS NULL OR service_id = ?2) \
         AND (?3 IS NULL OR workspace_id = ?3) AND (?4 IS NULL OR status = ?4) \
         ORDER BY requested_at DESC, id DESC LIMIT ?5"
    ))?;
    let rows = st.query_map(params![f.app_id, f.service_id, f.workspace, f.status, limit.clamp(1, 1000)], row)?;
    rows.collect()
}

/// The newest pending request of `kind` on a runtime App.
pub fn pending_of_kind(conn: &Connection, app_id: &str, kind: &str) -> rusqlite::Result<Option<AppRequest>> {
    conn.query_row(
        &format!("SELECT {COLS} FROM app_widenings WHERE app_id = ?1 AND kind = ?2 AND status = 'pending' ORDER BY requested_at DESC LIMIT 1"),
        params![app_id, kind],
        row,
    )
    .optional()
}

/// Replace a pending request's diff and summary (an egress request that
/// grows); `reason` replaces the requester's why when given.
pub fn update_diff(conn: &Connection, id: &str, diff: &Value, summary: &str, reason: Option<&str>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE app_widenings SET diff_json = ?2, summary = ?3, reason = COALESCE(?4, reason) WHERE id = ?1 AND status = 'pending'",
        params![id, diff.to_string(), summary, reason],
    )?;
    Ok(())
}

/// Egress kinds keep the host list in step with the request.
fn closed(conn: &Connection, r: &AppRequest, status: &str, by: &str) -> rusqlite::Result<()> {
    if r.kind == KIND_EGRESS || r.kind == KIND_EGRESS_MODE {
        crate::deploy::egress::on_closed(conn, r, status, by)?;
    }
    Ok(())
}

/// Close a pending request (`approved`, `denied`, `withdrawn`). `Err`
/// codes: `request_not_found`, `not_pending`, `request_expired` (an
/// approval after the seven days: the request is marked expired).
pub fn decide(conn: &Connection, id: &str, status: &str, by: &str, via: Option<&str>, reason: Option<&str>, now: i64) -> Result<AppRequest, &'static str> {
    let r = get(conn, id).map_err(|_| "request_not_found")?.ok_or("request_not_found")?;
    if r.status != STATUS_PENDING {
        return Err("not_pending");
    }
    if r.expires_at <= now {
        expire_one(conn, &r, now).map_err(|_| "request_not_found")?;
        return Err(if status == STATUS_APPROVED { "request_expired" } else { "not_pending" });
    }
    let n = conn
        .execute(
            "UPDATE app_widenings SET status = ?2, decided_by = ?3, decided_at = ?4, confirm_via = ?5, decision_reason = ?6 \
             WHERE id = ?1 AND status = 'pending'",
            params![id, status, by, now, via, reason],
        )
        .map_err(|_| "request_not_found")?;
    if n == 0 {
        return Err("not_pending");
    }
    closed(conn, &r, status, by).map_err(|_| "request_not_found")?;
    get(conn, id).map_err(|_| "request_not_found")?.ok_or("request_not_found")
}

/// Record the outcome of applying an approved request (`approved` with
/// what it did, or `failed` with the error).
pub fn set_result(conn: &Connection, id: &str, status: &str, result: &Value) -> rusqlite::Result<()> {
    conn.execute("UPDATE app_widenings SET status = ?2, result_json = ?3 WHERE id = ?1", params![id, status, result.to_string()])?;
    Ok(())
}

fn expire_one(conn: &Connection, r: &AppRequest, now: i64) -> rusqlite::Result<()> {
    let n = conn.execute(
        "UPDATE app_widenings SET status = 'expired', decided_by = 'system', decided_at = ?2 WHERE id = ?1 AND status = 'pending'",
        params![r.id, now],
    )?;
    if n > 0 {
        closed(conn, r, STATUS_EXPIRED, "system")?;
    }
    Ok(())
}

/// Expire every pending request past its seven days; returns them (the
/// caller tells their requesters).
pub fn expire_due(conn: &Connection, now: i64) -> rusqlite::Result<Vec<AppRequest>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM app_widenings WHERE status = 'pending' AND expires_at <= ?1"))?;
    let due = st.query_map(params![now], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    for r in &due {
        expire_one(conn, r, now)?;
    }
    Ok(due)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        // The `apps` table the requests point at (0165), minimal.
        c.execute_batch("CREATE TABLE apps (app_id TEXT PRIMARY KEY);").unwrap();
        let sql = include_str!("../../drizzle_sql/0168_app_grace.sql");
        // Only the new tables (the ALTERs need the full schema).
        for stmt in sql.split("--> statement-breakpoint") {
            if stmt.contains("CREATE TABLE") || stmt.contains("CREATE INDEX IF NOT EXISTS app_widenings") {
                c.execute_batch(stmt).unwrap();
            }
        }
        c
    }

    fn new<'a>(kind: &'a str, label: &'a str, body: &Value, by: &'a str) -> NewRequest<'a> {
        NewRequest {
            kind,
            app_id: None,
            service_id: Some("svc-1"),
            app_label: label,
            workspace_id: "ws-1",
            body: body.clone(),
            diff: serde_json::json!(["+ memory 2G"]),
            summary: format!("{kind} for {label}"),
            reasons: vec!["a widening only the owner makes".to_string()],
            reason: None,
            deployment_id: None,
            requested_by: by,
            requester_workspace: Some("ws-1"),
            requester_session: None,
        }
    }

    #[test]
    fn held_requests_dedupe_decide_once_and_expire() {
        let c = conn();
        let body = serde_json::json!({"app": "jobs", "ops": [{"memory": 2147483648i64}]});
        let a = create(&c, &new(KIND_CONFIG, "jobs", &body, "agent:nsi"), 100).unwrap();
        let b = create(&c, &new(KIND_CONFIG, "jobs", &body, "agent:nsi"), 101).unwrap();
        assert_eq!(a.id, b.id, "the same proposal from the same agent is one request");
        assert!(a.id.starts_with("w_") && a.id.len() == 14, "{}", a.id);
        assert_eq!(a.status, STATUS_PENDING);
        assert_eq!(a.expires_at, 100 + TTL_SECS);
        let mine = Filter { workspace: Some("ws-1"), status: Some(STATUS_PENDING), ..Default::default() };
        assert_eq!(list(&c, &mine, 50).unwrap().len(), 1);
        let theirs = Filter { workspace: Some("ws-2"), ..Default::default() };
        assert!(list(&c, &theirs, 50).unwrap().is_empty(), "another workspace sees nothing");
        assert_eq!(find(&c, &a.id[..8]).unwrap().unwrap().id, a.id, "a unique prefix finds it");
        assert!(find(&c, "w_").unwrap().is_none(), "too short a prefix");
        let d = decide(&c, &a.id, STATUS_DENIED, "owner", None, Some("no"), 103).unwrap();
        assert_eq!((d.status.as_str(), d.decision_reason.as_deref()), (STATUS_DENIED, Some("no")));
        assert_eq!(decide(&c, &a.id, STATUS_APPROVED, "owner", None, None, 104).unwrap_err(), "not_pending", "decided once");
        // A failed apply is recorded on the request.
        let f = create(&c, &new(KIND_CLEAN_MODE, "jobs", &serde_json::json!({"on": false}), "agent:nsi"), 110).unwrap();
        decide(&c, &f.id, STATUS_APPROVED, "owner", Some("local-auth"), None, 111).unwrap();
        set_result(&c, &f.id, STATUS_FAILED, &serde_json::json!({"code": "app_not_found"})).unwrap();
        let f = get(&c, &f.id).unwrap().unwrap();
        assert_eq!((f.status.as_str(), f.confirm_via.as_deref()), (STATUS_FAILED, Some("local-auth")));
        assert_eq!(f.result.unwrap()["code"], "app_not_found");
        // Expiry after seven days; approving an expired request fails.
        let e = create(&c, &new(KIND_APP_KEY, "other", &serde_json::json!({}), "agent:nsi"), 200).unwrap();
        assert_eq!(decide(&c, &e.id, STATUS_APPROVED, "owner", None, None, 200 + TTL_SECS).unwrap_err(), "request_expired");
        assert_eq!(get(&c, &e.id).unwrap().unwrap().status, STATUS_EXPIRED);
        let g = create(&c, &new(KIND_APP_KEY, "third", &serde_json::json!({}), "agent:nsi"), 300).unwrap();
        let gone = expire_due(&c, 300 + TTL_SECS).unwrap();
        assert_eq!(gone.iter().map(|r| r.id.clone()).collect::<Vec<_>>(), vec![g.id.clone()]);
        assert!(expire_due(&c, 300 + TTL_SECS).unwrap().is_empty());
        assert_eq!(decide(&c, &g.id, STATUS_WITHDRAWN, "agent:nsi", None, None, 300 + TTL_SECS + 1).unwrap_err(), "not_pending");
    }
}
