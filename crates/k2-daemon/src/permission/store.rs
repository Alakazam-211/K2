//! A1 — `permission_requests` / `permission_notes` reads and writes
//! (migration 0218), incl. the `pending → applying` compare-and-set (§6.2,
//! C12).
//!
//! Every write is one SQL statement under the shared connection's lock, so
//! a compare-and-set is atomic: two Allows at once → exactly one row
//! changes, the loser gets `request_not_pending` before any apply.

use k2_core::permission::card::Note;
use k2_core::permission::request::{PermissionRequest, RequestId, RequestState};
use rusqlite::{params, Connection, OptionalExtension, Row};

const COLS: &str = "id, workspace_id, session_id, conversation_id, addr, card_id, action, params_json, params_hash, \
     text_hash, why, state, floor, held_ref, batch_id, ask_number, asked_for, created_at, expires_at, answered_at, \
     answered_by, answered_via, reason, result_message, updated_why, updated_at";

fn from_row(r: &Row<'_>) -> rusqlite::Result<PermissionRequest> {
    let id: String = r.get(0)?;
    let state: String = r.get(11)?;
    let params_json: String = r.get(7)?;
    Ok(PermissionRequest {
        // Stored ids are always minted by `RequestId::new` (or parsed).
        id: RequestId::parse(&id).unwrap_or_else(|| panic!("permission_requests holds a bad id {id:?}")),
        workspace_id: r.get(1)?,
        session_id: r.get(2)?,
        conversation_id: r.get(3)?,
        addr: r.get(4)?,
        card_id: r.get(5)?,
        action: r.get(6)?,
        params: serde_json::from_str(&params_json).unwrap_or(serde_json::Value::Object(Default::default())),
        params_hash: r.get(8)?,
        text_hash: r.get(9)?,
        why: r.get(10)?,
        state: RequestState::parse(&state).unwrap_or(RequestState::Failed),
        floor: r.get(12)?,
        held_ref: r.get(13)?,
        batch_id: r.get(14)?,
        ask_number: r.get(15)?,
        asked_for: r.get(16)?,
        created_at: r.get(17)?,
        expires_at: r.get(18)?,
        answered_at: r.get(19)?,
        answered_by: r.get(20)?,
        answered_via: r.get(21)?,
        reason: r.get(22)?,
        result_message: r.get(23)?,
        updated_why: r.get(24)?,
        updated_at: r.get(25)?,
    })
}

fn with_conn<T>(f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    f(&conn).map_err(|e| format!("permission_requests: {e}"))
}

pub(crate) fn insert(r: &PermissionRequest) -> Result<(), String> {
    with_conn(|c| {
        c.execute(
            &format!("INSERT INTO permission_requests ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)"),
            params![
                r.id.as_str(),
                r.workspace_id,
                r.session_id,
                r.conversation_id,
                r.addr,
                r.card_id,
                r.action,
                r.params.to_string(),
                r.params_hash,
                r.text_hash,
                r.why,
                r.state.as_str(),
                r.floor,
                r.held_ref,
                r.batch_id,
                r.ask_number,
                r.asked_for,
                r.created_at,
                r.expires_at,
                r.answered_at,
                r.answered_by,
                r.answered_via,
                r.reason,
                r.result_message,
                r.updated_why,
                r.updated_at,
            ],
        )
        .map(|_| ())
    })
}

pub(crate) fn get(id: &str) -> Result<Option<PermissionRequest>, String> {
    with_conn(|c| c.query_row(&format!("SELECT {COLS} FROM permission_requests WHERE id = ?1"), params![id], from_row).optional())
}

/// By `pr_…` or a held request's own id (`ar_22`), newest first.
pub(crate) fn get_by_display_id(id: &str) -> Result<Option<PermissionRequest>, String> {
    if let Some(r) = get(id)? {
        return Ok(Some(r));
    }
    with_conn(|c| {
        c.query_row(
            &format!("SELECT {COLS} FROM permission_requests WHERE held_ref = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1"),
            params![id],
            from_row,
        )
        .optional()
    })
}

/// The requests a card carries (one, or a batch's items), oldest first.
pub(crate) fn for_card(conversation_id: &str, card_id: &str) -> Result<Vec<PermissionRequest>, String> {
    with_conn(|c| {
        let mut st = c.prepare(&format!(
            "SELECT {COLS} FROM permission_requests WHERE conversation_id = ?1 AND card_id = ?2 ORDER BY created_at, rowid"
        ))?;
        let rows = st.query_map(params![conversation_id, card_id], from_row)?;
        rows.collect()
    })
}

/// Requests of one workspace, newest first (at most `limit`).
pub(crate) fn for_workspace(workspace_id: &str, limit: usize) -> Result<Vec<PermissionRequest>, String> {
    with_conn(|c| {
        let mut st = c.prepare(&format!(
            "SELECT {COLS} FROM permission_requests WHERE workspace_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2"
        ))?;
        let rows = st.query_map(params![workspace_id, limit as i64], from_row)?;
        rows.collect()
    })
}

/// Every answerable request (pending or already-done, never answered),
/// optionally for one workspace.
pub(crate) fn open(workspace_id: Option<&str>) -> Result<Vec<PermissionRequest>, String> {
    with_conn(|c| {
        let sql = format!(
            "SELECT {COLS} FROM permission_requests WHERE state IN ('pending', 'already_done') AND answered_at IS NULL \
             AND (?1 IS NULL OR workspace_id = ?1) ORDER BY created_at, rowid"
        );
        let mut st = c.prepare(&sql)?;
        let rows = st.query_map(params![workspace_id], from_row)?;
        rows.collect()
    })
}

/// Answerable requests whose time ran out (`expires_at <= now`).
pub(crate) fn open_due(now: i64) -> Result<Vec<PermissionRequest>, String> {
    with_conn(|c| {
        let mut st = c.prepare(&format!(
            "SELECT {COLS} FROM permission_requests WHERE state IN ('pending', 'already_done') AND answered_at IS NULL \
             AND expires_at <= ?1 ORDER BY created_at, rowid"
        ))?;
        let rows = st.query_map(params![now], from_row)?;
        rows.collect()
    })
}

/// The open request for the same (workspace, action, values), if any.
pub(crate) fn open_same(workspace_id: &str, action: &str, params_hash: &str) -> Result<Option<PermissionRequest>, String> {
    with_conn(|c| {
        c.query_row(
            &format!(
                "SELECT {COLS} FROM permission_requests WHERE workspace_id = ?1 AND action = ?2 AND params_hash = ?3 \
                 AND state IN ('pending', 'already_done', 'applying') AND answered_at IS NULL ORDER BY created_at DESC LIMIT 1"
            ),
            params![workspace_id, action, params_hash],
            from_row,
        )
        .optional()
    })
}

/// **The compare-and-set** (§6.2, C12): move an answerable request to
/// `to` in one statement. `true` only for the one caller that won.
pub(crate) fn cas_open_to(id: &str, to: RequestState) -> Result<bool, String> {
    with_conn(|c| {
        c.execute(
            "UPDATE permission_requests SET state = ?2 WHERE id = ?1 AND state IN ('pending', 'already_done') AND answered_at IS NULL",
            params![id, to.as_str()],
        )
    })
    .map(|n| n == 1)
}

/// Put a request that won the compare-and-set back to `to` (an answerable
/// state) when nothing was decided: a missing or wrong confirm, or a
/// `Close` that no longer fits. Only from `applying`.
pub(crate) fn revert_applying(id: &str, to: RequestState) -> Result<(), String> {
    with_conn(|c| {
        c.execute(
            "UPDATE permission_requests SET state = ?2 WHERE id = ?1 AND state = 'applying'",
            params![id, to.as_str()],
        )
        .map(|_| ())
    })
}

/// The decision, once: state, who, how, when, reason and result message.
pub(crate) struct Decided<'a> {
    pub state: RequestState,
    pub by: Option<&'a str>,
    pub via: Option<&'a str>,
    pub reason: Option<&'a str>,
    pub message: Option<&'a str>,
    pub at: i64,
}

/// Record the decision on a request this caller owns (it won the
/// compare-and-set, or it is the daemon closing an open one).
pub(crate) fn decide(id: &str, d: &Decided<'_>) -> Result<(), String> {
    with_conn(|c| {
        c.execute(
            "UPDATE permission_requests SET state = ?2, answered_by = ?3, answered_via = ?4, reason = ?5, \
             result_message = ?6, answered_at = ?7 WHERE id = ?1",
            params![id, d.state.as_str(), d.by, d.via, d.reason, d.message, d.at],
        )
        .map(|_| ())
    })
}

/// Where the card is (posted, or moved onto a batch card).
pub(crate) fn set_card(id: &str, conversation_id: &str, card_id: &str, batch_id: Option<&str>, expires_at: i64) -> Result<(), String> {
    with_conn(|c| {
        c.execute(
            "UPDATE permission_requests SET conversation_id = ?2, card_id = ?3, batch_id = ?4, expires_at = ?5 WHERE id = ?1",
            params![id, conversation_id, card_id, batch_id, expires_at],
        )
        .map(|_| ())
    })
}

/// A pending request's state between `pending` and `already_done` (the
/// render-time and sweep re-check, §4.2, §6.2).
pub(crate) fn set_open_state(id: &str, to: RequestState) -> Result<bool, String> {
    debug_assert!(to.is_open());
    with_conn(|c| {
        c.execute(
            "UPDATE permission_requests SET state = ?2 WHERE id = ?1 AND state IN ('pending', 'already_done') AND answered_at IS NULL",
            params![id, to.as_str()],
        )
    })
    .map(|n| n == 1)
}

/// The one `update` (§3.2): a denied request (denied after `since`, never
/// updated) is open again with the agent's new `why` and a new expiry.
/// `true` only for the one caller that reopened it.
pub(crate) fn reopen_denied(id: &str, why: &str, at: i64, since: i64, expires_at: i64) -> Result<bool, String> {
    with_conn(|c| {
        c.execute(
            "UPDATE permission_requests SET state = 'pending', updated_why = ?2, updated_at = ?3, expires_at = ?5, \
             answered_at = NULL, answered_by = NULL, answered_via = NULL, reason = NULL, result_message = NULL, \
             batch_id = NULL WHERE id = ?1 AND state = 'denied' AND updated_at IS NULL AND answered_at > ?4",
            params![id, why, at, since, expires_at],
        )
    })
    .map(|n| n == 1)
}

// ── counts for the limits (SQL, so they survive a restart; §3.2, C20) ──

pub(crate) fn count_open_for_workspace(workspace_id: &str) -> Result<usize, String> {
    count("SELECT COUNT(*) FROM permission_requests WHERE workspace_id = ?1 AND state IN ('pending', 'already_done', 'applying') AND answered_at IS NULL", &[&workspace_id])
}

pub(crate) fn count_open_on_server() -> Result<usize, String> {
    count("SELECT COUNT(*) FROM permission_requests WHERE state IN ('pending', 'already_done', 'applying') AND answered_at IS NULL", &[])
}

pub(crate) fn count_asks_since(workspace_id: &str, since: i64) -> Result<usize, String> {
    count("SELECT COUNT(*) FROM permission_requests WHERE workspace_id = ?1 AND created_at > ?2", &[&workspace_id, &since])
}

pub(crate) fn count_same_asks_since(workspace_id: &str, action: &str, params_hash: &str, since: i64) -> Result<usize, String> {
    count(
        "SELECT COUNT(*) FROM permission_requests WHERE workspace_id = ?1 AND action = ?2 AND params_hash = ?3 AND created_at > ?4",
        &[&workspace_id, &action, &params_hash, &since],
    )
}

/// The newest denial of the same (workspace, action, values) since `since`.
pub(crate) fn last_denial_since(workspace_id: &str, action: &str, params_hash: &str, since: i64) -> Result<Option<PermissionRequest>, String> {
    with_conn(|c| {
        c.query_row(
            &format!(
                "SELECT {COLS} FROM permission_requests WHERE workspace_id = ?1 AND action = ?2 AND params_hash = ?3 \
                 AND state = 'denied' AND answered_at > ?4 ORDER BY answered_at DESC LIMIT 1"
            ),
            params![workspace_id, action, params_hash, since],
            from_row,
        )
        .optional()
    })
}

fn count(sql: &str, args: &[&dyn rusqlite::ToSql]) -> Result<usize, String> {
    with_conn(|c| c.query_row(sql, args, |r| r.get::<_, i64>(0))).map(|n| n as usize)
}

// ── notes (one per person per request; written by A2's note route) ──

/// The notes on a request from people (not guests), oldest first.
pub(crate) fn notes(request_id: &str) -> Result<Vec<Note>, String> {
    with_conn(|c| {
        let mut st = c.prepare(
            "SELECT author, author_role, text, updated_at FROM permission_notes \
             WHERE request_id = ?1 AND author_kind != 'guest' ORDER BY created_at, author",
        )?;
        let rows = st.query_map(params![request_id], |r| {
            Ok(Note { by: r.get(0)?, role: r.get(1)?, text: r.get(2)?, at: r.get(3)? })
        })?;
        rows.collect()
    })
}

/// Write (or replace) a person's note; `""` deletes it (§4.3, §17.5).
/// For A2's note route.
pub(crate) fn put_note(request_id: &str, author: &str, author_kind: &str, author_role: &str, text: &str, at: i64) -> Result<(), String> {
    with_conn(|c| {
        if text.is_empty() {
            c.execute("DELETE FROM permission_notes WHERE request_id = ?1 AND author = ?2", params![request_id, author])?;
            return Ok(());
        }
        c.execute(
            "INSERT INTO permission_notes (request_id, author, author_kind, author_role, text, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6) \
             ON CONFLICT(request_id, author) DO UPDATE SET text = excluded.text, author_role = excluded.author_role, \
             updated_at = excluded.updated_at",
            params![request_id, author, author_kind, author_role, text, at],
        )
        .map(|_| ())
    })
}
