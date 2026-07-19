//! Remote Session Layer 0 — master switch + denial/event audit.
//!
//! Fail-closed hard wall independent of grants:
//! - `remote_sessions_enabled` (app_settings, default OFF) is the Layer 0 gate
//! - When OFF, drive attempts are denied with `REMOTE_SESSIONS_DISABLED`
//! - Every denial is persisted in `remote_session_events` for owner visibility
//!
//! Stage 1 ships empty grant tables + event write/list. Stage 2 mints grants.

use rusqlite::params;

/// Wire / DB code for "master switch is OFF".
pub const CODE_REMOTE_SESSIONS_DISABLED: &str = "REMOTE_SESSIONS_DISABLED";
/// Wire / DB code for "switch is ON but no grant covers this principal".
pub const CODE_NO_GRANT: &str = "NO_GRANT";
/// Event kind written for access denials.
pub const KIND_DENIAL: &str = "denial";

/// One `remote_session_events` row (camelCase wire shape for status).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSessionEvent {
    pub id: String,
    pub grant_id: Option<String>,
    pub principal_label: String,
    pub kind: String,
    pub code: Option<String>,
    pub payload: Option<String>,
    pub created_at: i64,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Layer 0 gate: true only when the owner opted this host into remote sessions.
pub fn is_enabled() -> bool {
    crate::app_settings::load().remote_sessions_enabled
}

/// Persist the Layer 0 master switch via app_settings deep-merge.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    crate::app_settings::update(serde_json::json!({
        "remoteSessionsEnabled": enabled
    }))
    .map(|_| ())
    .map_err(|e| format!("remote_sessions set_enabled: {e}"))
}

/// Insert an audit event (denial, grant-use, etc.).
pub fn record_event(
    grant_id: Option<&str>,
    principal_label: &str,
    kind: &str,
    code: Option<&str>,
    payload: Option<&str>,
) -> Result<RemoteSessionEvent, String> {
    let label = principal_label.trim();
    if label.is_empty() {
        return Err("principal_label must not be empty".to_string());
    }
    let kind = kind.trim();
    if kind.is_empty() {
        return Err("kind must not be empty".to_string());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO remote_session_events \
         (id, grant_id, principal_label, kind, code, payload, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            grant_id,
            label,
            kind,
            code,
            payload,
            created_at,
        ],
    )
    .map_err(|e| format!("remote_session_events insert failed: {e}"))?;
    Ok(RemoteSessionEvent {
        id,
        grant_id: grant_id.map(|s| s.to_string()),
        principal_label: label.to_string(),
        kind: kind.to_string(),
        code: code.map(|s| s.to_string()),
        payload: payload.map(|s| s.to_string()),
        created_at,
    })
}

/// Convenience: record a denial with kind=`denial`.
pub fn record_denial(
    principal_label: &str,
    code: &str,
    payload: Option<&str>,
) -> Result<RemoteSessionEvent, String> {
    record_event(None, principal_label, KIND_DENIAL, Some(code), payload)
}

/// Recent denial events (newest first), capped at `limit` (clamped 1..=200).
pub fn list_recent_denials(limit: usize) -> Result<Vec<RemoteSessionEvent>, String> {
    let limit = limit.clamp(1, 200) as i64;
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, grant_id, principal_label, kind, code, payload, created_at \
             FROM remote_session_events \
             WHERE kind = ?1 \
             ORDER BY created_at DESC \
             LIMIT ?2",
        )
        .map_err(|e| format!("remote_session_events prepare: {e}"))?;
    let rows = stmt
        .query_map(params![KIND_DENIAL, limit], |row| {
            Ok(RemoteSessionEvent {
                id: row.get(0)?,
                grant_id: row.get(1)?,
                principal_label: row.get(2)?,
                kind: row.get(3)?,
                code: row.get(4)?,
                payload: row.get(5)?,
                created_at: row.get(6)?,
            })
        })
        .map_err(|e| format!("remote_session_events query: {e}"))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| format!("remote_session_events row: {e}"))?);
    }
    Ok(out)
}

/// Active (non-revoked, non-expired) grant count — Stage 1 always 0 until mint.
pub fn active_grant_count() -> Result<usize, String> {
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM remote_session_grants \
             WHERE revoked_at IS NULL AND expires_at > ?1",
            params![now],
            |row| row.get(0),
        )
        .map_err(|e| format!("remote_session_grants count: {e}"))?;
    Ok(n as usize)
}
