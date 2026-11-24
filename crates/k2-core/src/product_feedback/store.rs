//! The outbox (`product_feedback_outbox`, migration 0285), a person's
//! remembered diagnostics choice (`product_feedback_prefs`) and the
//! screenshot a person attached (`product_feedback_screenshots` and
//! `screenshot_meta`, migration 0289, prd-feedback-screenshot-v1 §5.4).
//!
//! A row and its picture are written in one transaction. The picture is
//! deleted in the same transaction as the move to `sent` (the intake
//! confirmed it), `refused` or `cancelled`; a retry or a `failed` row keeps
//! it, so the next attempt sends the same bytes. `screenshot_meta` stays.
//!
//! States: `queued` → `sending` → `sent`; a retryable failure goes back to
//! `queued` with a later `next_attempt_at`; a reject the intake understood
//! is `refused` (never retried); 7 days of retrying is `failed` (Retry puts
//! it back, and one that failed because the service couldn't be reached
//! goes back by itself once the service answers: [`requeue_unreachable_failed`]);
//! a person can cancel a `queued` row. `sending` rows found at
//! boot go back to `queued`: the intake's idempotency key makes a resend
//! harmless. Times are milliseconds.

use rusqlite::{params, Connection, OptionalExtension, Row as SqlRow};
use serde::Serialize;
use crate::brand::PRODUCT_NAME;

/// How long a row keeps retrying before it is `failed` (PF16).
pub const GIVE_UP_AFTER_MS: i64 = 7 * 86_400_000;
/// Final rows are deleted this long after their last change (PF15).
pub const KEEP_FINAL_MS: i64 = 90 * 86_400_000;
/// The sent diagnostics are cleared this long after `sent` (PF15).
pub const KEEP_SENT_DIAGNOSTICS_MS: i64 = 7 * 86_400_000;
/// Agent limits on this box (PF8): per workspace, per minute and per day;
/// all agents together, per day.
pub const AGENT_PER_MINUTE: i64 = 1;
pub const AGENT_PER_DAY: i64 = 3;
pub const AGENTS_PER_DAY: i64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Queued,
    Sending,
    Sent,
    Failed,
    Refused,
    Cancelled,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Sending => "sending",
            State::Sent => "sent",
            State::Failed => "failed",
            State::Refused => "refused",
            State::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<State> {
        Some(match s {
            "queued" => State::Queued,
            "sending" => State::Sending,
            "sent" => State::Sent,
            "failed" => State::Failed,
            "refused" => State::Refused,
            "cancelled" => State::Cancelled,
            _ => return None,
        })
    }

    pub fn is_final(self) -> bool {
        matches!(self, State::Sent | State::Failed | State::Refused | State::Cancelled)
    }
}

/// One outbox row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub created_at: i64,
    pub queued_at: i64,
    pub updated_at: i64,
    pub sender_kind: String,
    pub sender: String,
    pub client: String,
    pub category: String,
    pub body: String,
    pub contact_email: Option<String>,
    pub meta_json: String,
    pub state: State,
    pub attempts: i64,
    pub next_attempt_at: i64,
    pub last_error: Option<String>,
    pub remote_id: Option<String>,
    pub sent_at: Option<i64>,
    pub diagnostics_json: Option<String>,
    /// [`super::screenshot::Meta`] as JSON, when a person attached one.
    pub screenshot_meta: Option<String>,
}

/// A picture to store with a new row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewShot {
    pub png: Vec<u8>,
    pub sha256: String,
    /// [`super::screenshot::Meta`] as JSON.
    pub meta_json: String,
}

/// A row's stored picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredShot {
    pub png: Vec<u8>,
    pub sha256: String,
    pub created_at: i64,
}

/// What a `send` stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRow {
    pub id: String,
    pub sender_kind: String,
    pub sender: String,
    pub client: String,
    pub category: String,
    pub body: String,
    pub contact_email: Option<String>,
    pub meta_json: String,
    pub diagnostics_json: Option<String>,
    pub screenshot: Option<NewShot>,
}

const COLS: &str = "id, created_at, queued_at, updated_at, sender_kind, sender, client, category, body, \
     contact_email, meta_json, state, attempts, next_attempt_at, last_error, remote_id, sent_at, diagnostics_json, \
     screenshot_meta";

fn from_row(r: &SqlRow<'_>) -> rusqlite::Result<Row> {
    let state: String = r.get(11)?;
    Ok(Row {
        id: r.get(0)?,
        created_at: r.get(1)?,
        queued_at: r.get(2)?,
        updated_at: r.get(3)?,
        sender_kind: r.get(4)?,
        sender: r.get(5)?,
        client: r.get(6)?,
        category: r.get(7)?,
        body: r.get(8)?,
        contact_email: r.get(9)?,
        meta_json: r.get(10)?,
        state: State::parse(&state).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, format!("unknown state {state}").into())
        })?,
        attempts: r.get(12)?,
        next_attempt_at: r.get(13)?,
        last_error: r.get(14)?,
        remote_id: r.get(15)?,
        sent_at: r.get(16)?,
        diagnostics_json: r.get(17)?,
        screenshot_meta: r.get(18)?,
    })
}

/// Run `f` inside a savepoint: all of it, or none of it. Works inside or
/// outside a transaction the caller holds.
fn atomically<T>(conn: &Connection, f: impl FnOnce() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    conn.execute_batch("SAVEPOINT product_feedback_store")?;
    match f() {
        Ok(v) => {
            conn.execute_batch("RELEASE product_feedback_store")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO product_feedback_store; RELEASE product_feedback_store");
            Err(e)
        }
    }
}

/// Store a new `queued` row, due now, and its picture: both or neither.
pub fn insert(conn: &Connection, n: &NewRow, now: i64) -> rusqlite::Result<Row> {
    atomically(conn, || {
        conn.execute(
            "INSERT INTO product_feedback_outbox (id, created_at, queued_at, updated_at, sender_kind, sender, client, \
             category, body, contact_email, meta_json, state, attempts, next_attempt_at, diagnostics_json, screenshot_meta) \
             VALUES (?1, ?2, ?2, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'queued', 0, ?2, ?10, ?11)",
            params![
                n.id,
                now,
                n.sender_kind,
                n.sender,
                n.client,
                n.category,
                n.body,
                n.contact_email,
                n.meta_json,
                n.diagnostics_json,
                n.screenshot.as_ref().map(|s| s.meta_json.as_str())
            ],
        )?;
        if let Some(s) = &n.screenshot {
            conn.execute(
                "INSERT INTO product_feedback_screenshots (feedback_id, png, sha256, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![n.id, s.png, s.sha256, now],
            )?;
        }
        Ok(())
    })?;
    get(conn, &n.id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// The row's picture, while it is still on this box.
pub fn screenshot(conn: &Connection, id: &str) -> rusqlite::Result<Option<StoredShot>> {
    conn.query_row(
        "SELECT png, sha256, created_at FROM product_feedback_screenshots WHERE feedback_id = ?1",
        params![id],
        |r| Ok(StoredShot { png: r.get(0)?, sha256: r.get(1)?, created_at: r.get(2)? }),
    )
    .optional()
}

/// Whether the row's picture is still on this box (no bytes read).
pub fn has_screenshot(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM product_feedback_screenshots WHERE feedback_id = ?1", params![id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn purge_screenshot(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM product_feedback_screenshots WHERE feedback_id = ?1", params![id])?;
    Ok(())
}

/// A state change that ends the picture's life on the box: the change and
/// the delete in one savepoint.
fn set_state_and_purge(
    conn: &Connection,
    id: &str,
    from: &[&str],
    sql_set: &str,
    p: &[&dyn rusqlite::ToSql],
) -> rusqlite::Result<bool> {
    atomically(conn, || {
        let changed = set_state(conn, id, from, sql_set, p)?;
        if changed {
            purge_screenshot(conn, id)?;
        }
        Ok(changed)
    })
}

/// Pictures whose row is gone or final (`sent`, `refused`, `cancelled`):
/// the prune's backstop. Returns how many went.
pub fn prune_screenshots(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM product_feedback_screenshots WHERE feedback_id NOT IN \
         (SELECT id FROM product_feedback_outbox WHERE state IN ('queued', 'sending', 'failed'))",
        [],
    )
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<Row>> {
    conn.query_row(&format!("SELECT {COLS} FROM product_feedback_outbox WHERE id = ?1"), params![id], from_row)
        .optional()
}

/// Rows, newest first. `sender` = only that sender's; `state` = only that state.
pub fn list(conn: &Connection, sender: Option<&str>, state: Option<State>, limit: i64) -> rusqlite::Result<Vec<Row>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM product_feedback_outbox \
         WHERE (?1 IS NULL OR sender = ?1) AND (?2 IS NULL OR state = ?2) \
         ORDER BY created_at DESC, id DESC LIMIT ?3"
    ))?;
    let rows = stmt.query_map(params![sender, state.map(State::as_str), limit], from_row)?;
    rows.collect()
}

/// `queued` rows due at `now`, oldest first.
pub fn due(conn: &Connection, now: i64, limit: i64) -> rusqlite::Result<Vec<Row>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM product_feedback_outbox WHERE state = 'queued' AND next_attempt_at <= ?1 \
         ORDER BY created_at ASC, id ASC LIMIT ?2"
    ))?;
    let rows = stmt.query_map(params![now, limit], from_row)?;
    rows.collect()
}

/// How many rows wait to go (`queued` or `sending`).
pub fn pending_count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM product_feedback_outbox WHERE state IN ('queued', 'sending')",
        [],
        |r| r.get(0),
    )
}

fn set_state(conn: &Connection, id: &str, from: &[&str], sql_set: &str, p: &[&dyn rusqlite::ToSql]) -> rusqlite::Result<bool> {
    let from_list = from.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(", ");
    let sql = format!("UPDATE product_feedback_outbox SET {sql_set} WHERE id = ?1 AND state IN ({from_list})");
    let mut all: Vec<&dyn rusqlite::ToSql> = vec![&id];
    all.extend_from_slice(p);
    Ok(conn.execute(&sql, all.as_slice())? == 1)
}

/// `queued` → `sending` (the drain claims it).
pub fn mark_sending(conn: &Connection, id: &str, now: i64) -> rusqlite::Result<bool> {
    set_state(conn, id, &["queued"], "state = 'sending', updated_at = ?2", &[&now])
}

/// `sending` → `sent`, keeping the intake's id. The intake confirmed it,
/// so the picture goes from the box.
pub fn mark_sent(conn: &Connection, id: &str, remote_id: &str, now: i64) -> rusqlite::Result<bool> {
    set_state_and_purge(
        conn,
        id,
        &["sending"],
        "state = 'sent', remote_id = ?2, sent_at = ?3, updated_at = ?3, last_error = NULL, attempts = attempts + 1",
        &[&remote_id, &now],
    )
}

/// Back to `queued` after a retryable failure, due at `next_at`.
pub fn mark_retry(conn: &Connection, id: &str, next_at: i64, error: &str, now: i64) -> rusqlite::Result<bool> {
    set_state(
        conn,
        id,
        &["sending", "queued"],
        "state = 'queued', next_attempt_at = ?2, last_error = ?3, updated_at = ?4, attempts = attempts + 1",
        &[&next_at, &error, &now],
    )
}

/// Held without an attempt (no Connect, air-gap, turned off): stays
/// `queued`, the reason shown, nothing counted.
pub fn mark_held(conn: &Connection, id: &str, next_at: i64, error: &str, now: i64) -> rusqlite::Result<bool> {
    set_state(
        conn,
        id,
        &["sending", "queued"],
        "state = 'queued', next_attempt_at = ?2, last_error = ?3, updated_at = ?4",
        &[&next_at, &error, &now],
    )
}

/// The intake understood and rejected it: never retried, so the picture
/// goes.
pub fn mark_refused(conn: &Connection, id: &str, error: &str, now: i64) -> rusqlite::Result<bool> {
    set_state_and_purge(
        conn,
        id,
        &["sending", "queued"],
        "state = 'refused', last_error = ?2, updated_at = ?3, attempts = attempts + 1",
        &[&error, &now],
    )
}

/// Retry: `failed` → `queued`, due now, a fresh 7-day window.
pub fn retry(conn: &Connection, id: &str, now: i64) -> rusqlite::Result<bool> {
    set_state(
        conn,
        id,
        &["failed"],
        "state = 'queued', queued_at = ?2, next_attempt_at = ?2, updated_at = ?2, attempts = 0",
        &[&now],
    )
}

/// Cancel: `queued` → `cancelled`; the picture goes.
pub fn cancel(conn: &Connection, id: &str, now: i64) -> rusqlite::Result<bool> {
    set_state_and_purge(conn, id, &["queued"], "state = 'cancelled', updated_at = ?2", &[&now])
}

/// Boot: every `sending` row goes back to `queued`, due now.
pub fn recover_at_boot(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE product_feedback_outbox SET state = 'queued', next_attempt_at = ?1, updated_at = ?1 WHERE state = 'sending'",
        params![now],
    )
}

/// `queued` rows that have retried for [`GIVE_UP_AFTER_MS`] become
/// `failed`. Returns their ids.
pub fn give_up_stale(conn: &Connection, now: i64) -> rusqlite::Result<Vec<String>> {
    let cutoff = now - GIVE_UP_AFTER_MS;
    let ids: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT id FROM product_feedback_outbox WHERE state = 'queued' AND queued_at < ?1 AND attempts > 0",
        )?;
        let rows = stmt.query_map(params![cutoff], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for id in &ids {
        conn.execute(
            "UPDATE product_feedback_outbox SET state = 'failed', updated_at = ?2, \
             last_error = COALESCE(last_error, 'not sent within 7 days') WHERE id = ?1 AND state = 'queued'",
            params![id, now],
        )?;
    }
    Ok(ids)
}

/// A `failed` row whose last reason is one of these codes goes again by
/// itself once the service answers: the service or the relay couldn't be
/// reached, was paused or asked to wait. Anything else (a person's
/// switch, air-gap, no K2 Connect pairing, a rejected token) stays
/// `failed` until someone presses Retry.
pub const RESEND_BY_ITSELF: &[&str] = &[
    super::wire::E_UNREACHABLE,
    super::wire::E_RELAY_NOT_READY,
    super::wire::E_PAUSED,
    super::wire::E_RATE_LIMITED,
];
/// Only rows created this recently go again by themselves (the same 90
/// days a final row is kept).
pub const RESEND_MAX_AGE_MS: i64 = KEEP_FINAL_MS;

/// Whether a `failed` row with this `last_error` goes again by itself.
pub fn resends_by_itself(last_error: Option<&str>) -> bool {
    // Stored as `<code>: <detail>`.
    last_error
        .and_then(|e| e.split_once(": "))
        .is_some_and(|(code, _)| RESEND_BY_ITSELF.contains(&code))
}

/// `failed` rows that [`resends_by_itself`], created within
/// [`RESEND_MAX_AGE_MS`] of `now`, oldest first.
fn resendable_failed(conn: &Connection, now: i64) -> rusqlite::Result<Vec<Row>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM product_feedback_outbox WHERE state = 'failed' AND created_at >= ?1 \
         ORDER BY created_at ASC, id ASC"
    ))?;
    let rows = stmt.query_map(params![now - RESEND_MAX_AGE_MS], from_row)?;
    let mut out = Vec::new();
    for r in rows {
        let r = r?;
        if resends_by_itself(r.last_error.as_deref()) {
            out.push(r);
        }
    }
    Ok(out)
}

/// The service answered: every `failed` row that [`resends_by_itself`] and
/// was created within [`RESEND_MAX_AGE_MS`] goes back to `queued`, due now,
/// with a fresh 7-day window (as Retry does). Returns their ids, oldest
/// first. They then go through the drain like any queued row.
pub fn requeue_unreachable_failed(conn: &Connection, now: i64) -> rusqlite::Result<Vec<String>> {
    atomically(conn, || {
        let mut ids = Vec::new();
        for r in resendable_failed(conn, now)? {
            if retry(conn, &r.id, now)? {
                ids.push(r.id);
            }
        }
        Ok(ids)
    })
}

/// The `last_error` a row was left with when the intake refused a box token
/// it had already seen (`401 replayed`): before 0.46.18 the drain reused one
/// token for a whole batch, and every post after the first was refused.
pub const REPLAYED_ERROR: &str = "token_rejected: replayed";

/// `failed` rows left by [`REPLAYED_ERROR`] (created within
/// [`RESEND_MAX_AGE_MS`]) go back to `queued`, due now, with a fresh 7-day
/// window, oldest first. Returns their ids. The daemon runs this once per
/// server (a code migration), at boot.
pub fn requeue_replayed_failed(conn: &Connection, now: i64) -> rusqlite::Result<Vec<String>> {
    atomically(conn, || {
        let ids: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT id FROM product_feedback_outbox WHERE state = 'failed' AND last_error = ?1 AND created_at >= ?2 \
                 ORDER BY created_at ASC, id ASC",
            )?;
            let rows = stmt.query_map(params![REPLAYED_ERROR, now - RESEND_MAX_AGE_MS], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut out = Vec::new();
        for id in ids {
            if retry(conn, &id, now)? {
                out.push(id);
            }
        }
        Ok(out)
    })
}

/// Whether any `failed` row would go again by itself: only then is the
/// intake's health check worth asking.
pub fn has_resendable_failed(conn: &Connection, now: i64) -> rusqlite::Result<bool> {
    Ok(!resendable_failed(conn, now)?.is_empty())
}

/// Delete final rows older than [`KEEP_FINAL_MS`]; clear the diagnostics
/// of rows sent more than [`KEEP_SENT_DIAGNOSTICS_MS`] ago; then
/// [`prune_screenshots`]. Returns `(deleted, cleared)`.
pub fn prune(conn: &Connection, now: i64) -> rusqlite::Result<(usize, usize)> {
    let deleted = conn.execute(
        "DELETE FROM product_feedback_outbox WHERE state IN ('sent', 'failed', 'refused', 'cancelled') AND updated_at < ?1",
        params![now - KEEP_FINAL_MS],
    )?;
    let cleared = conn.execute(
        "UPDATE product_feedback_outbox SET diagnostics_json = NULL \
         WHERE state = 'sent' AND diagnostics_json IS NOT NULL AND sent_at < ?1",
        params![now - KEEP_SENT_DIAGNOSTICS_MS],
    )?;
    prune_screenshots(conn)?;
    Ok((deleted, cleared))
}

/// Why an agent may not send now (PF8), or `None`. Counts every row the
/// agent stored except cancelled ones, so a send that was refused still
/// counts toward the day.
pub fn agent_limit(conn: &Connection, sender: &str, now: i64) -> rusqlite::Result<Option<AgentLimit>> {
    let count = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> rusqlite::Result<i64> { conn.query_row(sql, p, |r| r.get(0)) };
    let minute = now - 60_000;
    let day = now - 86_400_000;
    let mine_minute = count(
        "SELECT COUNT(*) FROM product_feedback_outbox WHERE sender = ?1 AND sender_kind = 'agent' AND state != 'cancelled' AND created_at > ?2",
        &[&sender, &minute],
    )?;
    if mine_minute >= AGENT_PER_MINUTE {
        return Ok(Some(AgentLimit::PerMinute));
    }
    let mine_day = count(
        "SELECT COUNT(*) FROM product_feedback_outbox WHERE sender = ?1 AND sender_kind = 'agent' AND state != 'cancelled' AND created_at > ?2",
        &[&sender, &day],
    )?;
    if mine_day >= AGENT_PER_DAY {
        return Ok(Some(AgentLimit::PerDay));
    }
    let all_day = count(
        "SELECT COUNT(*) FROM product_feedback_outbox WHERE sender_kind = 'agent' AND state != 'cancelled' AND created_at > ?1",
        &[&day],
    )?;
    if all_day >= AGENTS_PER_DAY {
        return Ok(Some(AgentLimit::BoxPerDay));
    }
    Ok(None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentLimit {
    PerMinute,
    PerDay,
    BoxPerDay,
}

impl AgentLimit {
    pub fn message(self) -> String {
        match self {
            AgentLimit::PerMinute => format!("An agent may send feedback to {PRODUCT_NAME} {AGENT_PER_MINUTE} time a minute. Try again in a minute."),
            AgentLimit::PerDay => format!("An agent may send feedback to {PRODUCT_NAME} {AGENT_PER_DAY} times a day. Try again tomorrow, or ask your human to send it."),
            AgentLimit::BoxPerDay => format!(
                "Agents on this server have sent {AGENTS_PER_DAY} feedback messages to {PRODUCT_NAME} today. Try again tomorrow, or ask your human to send it."
            ),
        }
    }
    pub fn retry_after_secs(self) -> u64 {
        match self {
            AgentLimit::PerMinute => 60,
            _ => 86_400,
        }
    }
}

/// A person's remembered "start with the diagnostics box ticked" (PF27).
pub fn remember_diagnostics(conn: &Connection, username: &str) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row(
            "SELECT remember_diagnostics FROM product_feedback_prefs WHERE username = ?1",
            params![username],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .is_some_and(|v| v != 0))
}

pub fn set_remember_diagnostics(conn: &Connection, username: &str, on: bool) -> rusqlite::Result<()> {
    if on {
        conn.execute(
            "INSERT INTO product_feedback_prefs (username, remember_diagnostics) VALUES (?1, 1) \
             ON CONFLICT(username) DO UPDATE SET remember_diagnostics = 1",
            params![username],
        )?;
    } else {
        conn.execute("DELETE FROM product_feedback_prefs WHERE username = ?1", params![username])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../drizzle_sql/0285_product_feedback.sql")).unwrap();
        c.execute_batch(include_str!("../../drizzle_sql/0289_product_feedback_screenshot.sql")).unwrap();
        c
    }

    fn new(id: &str, sender: &str, kind: &str) -> NewRow {
        NewRow {
            id: id.into(),
            sender_kind: kind.into(),
            sender: sender.into(),
            client: "cli".into(),
            category: "bug".into(),
            body: "the tab froze".into(),
            contact_email: None,
            meta_json: "{}".into(),
            diagnostics_json: None,
            screenshot: None,
        }
    }

    fn with_shot(id: &str, png: &[u8]) -> NewRow {
        let mut n = new(id, "login:alice", "person");
        n.screenshot = Some(NewShot {
            png: png.to_vec(),
            sha256: crate::product_feedback::screenshot::sha256_hex(png),
            meta_json: format!(r#"{{"bytes":{},"width":1,"height":1,"source":"window","capture":"page","sha256":"x"}}"#, png.len()),
        });
        n
    }

    fn shots(c: &Connection) -> i64 {
        c.query_row("SELECT COUNT(*) FROM product_feedback_screenshots", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn a_row_and_its_picture_are_written_together() {
        let c = db();
        let r = insert(&c, &with_shot("pf_s1", b"png-bytes"), 5).unwrap();
        assert!(r.screenshot_meta.as_deref().unwrap().contains(r#""capture":"page""#));
        let s = screenshot(&c, "pf_s1").unwrap().unwrap();
        assert_eq!((s.png.as_slice(), s.created_at), (&b"png-bytes"[..], 5));
        assert_eq!(s.sha256, crate::product_feedback::screenshot::sha256_hex(b"png-bytes"));
        assert!(has_screenshot(&c, "pf_s1").unwrap());
        // A row without one has neither.
        let r = insert(&c, &new("pf_s0", "owner", "person"), 5).unwrap();
        assert_eq!(r.screenshot_meta, None);
        assert!(!has_screenshot(&c, "pf_s0").unwrap());
    }

    #[test]
    fn a_failed_picture_write_leaves_no_row() {
        let c = db();
        c.execute_batch(
            "CREATE TRIGGER fail_shot BEFORE INSERT ON product_feedback_screenshots BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        )
        .unwrap();
        let e = insert(&c, &with_shot("pf_s2", b"png"), 1).unwrap_err();
        assert!(e.to_string().contains("injected"), "{e}");
        assert!(get(&c, "pf_s2").unwrap().is_none(), "no row without its picture");
        assert_eq!(shots(&c), 0);
        // And the connection is usable afterwards (the savepoint closed).
        c.execute_batch("DROP TRIGGER fail_shot").unwrap();
        insert(&c, &with_shot("pf_s2", b"png"), 2).unwrap();
        assert_eq!(shots(&c), 1);
    }

    #[test]
    fn sent_refused_and_cancelled_delete_the_picture_and_keep_the_meta() {
        let c = db();
        // Sent: only once the intake confirmed it.
        insert(&c, &with_shot("pf_t1", b"a"), 1).unwrap();
        mark_sending(&c, "pf_t1", 2).unwrap();
        assert!(mark_retry(&c, "pf_t1", 10, "service_unreachable: timeout", 3).unwrap());
        assert!(has_screenshot(&c, "pf_t1").unwrap(), "a failed attempt (or a timeout) keeps it for the next one");
        mark_sending(&c, "pf_t1", 11).unwrap();
        assert!(mark_sent(&c, "pf_t1", "remote-1", 12).unwrap());
        assert!(!has_screenshot(&c, "pf_t1").unwrap(), "sent: gone from the box");
        assert!(get(&c, "pf_t1").unwrap().unwrap().screenshot_meta.is_some(), "the meta stays for the Sent tab");
        // A no-op transition deletes nothing.
        insert(&c, &with_shot("pf_t2", b"b"), 1).unwrap();
        assert!(!mark_sent(&c, "pf_t2", "r", 2).unwrap(), "sent only from sending");
        assert!(has_screenshot(&c, "pf_t2").unwrap());
        // Refused.
        assert!(mark_refused(&c, "pf_t2", "bad_screenshot: no", 3).unwrap());
        assert!(!has_screenshot(&c, "pf_t2").unwrap());
        // Cancelled.
        insert(&c, &with_shot("pf_t3", b"c"), 1).unwrap();
        assert!(cancel(&c, "pf_t3", 2).unwrap());
        assert!(!has_screenshot(&c, "pf_t3").unwrap());
        assert!(get(&c, "pf_t3").unwrap().unwrap().screenshot_meta.is_some());
        assert_eq!(shots(&c), 0);
    }

    #[test]
    fn failed_keeps_the_picture_for_retry_and_prune_removes_orphans() {
        let c = db();
        insert(&c, &with_shot("pf_u1", b"a"), 0).unwrap();
        mark_retry(&c, "pf_u1", 10, "unreachable", 5).unwrap();
        assert_eq!(give_up_stale(&c, GIVE_UP_AFTER_MS + 1).unwrap(), vec!["pf_u1".to_string()]);
        assert!(has_screenshot(&c, "pf_u1").unwrap(), "failed keeps it");
        assert!(retry(&c, "pf_u1", GIVE_UP_AFTER_MS + 2).unwrap());
        assert_eq!(screenshot(&c, "pf_u1").unwrap().unwrap().png, b"a".to_vec(), "Retry resends the same bytes");
        // Orphans: a picture whose row is gone, or whose row is final.
        c.execute("INSERT INTO product_feedback_screenshots (feedback_id, png, sha256, created_at) VALUES ('pf_gone', x'00', 's', 0)", []).unwrap();
        insert(&c, &with_shot("pf_u2", b"b"), 0).unwrap();
        c.execute("UPDATE product_feedback_outbox SET state = 'sent' WHERE id = 'pf_u2'", []).unwrap();
        assert_eq!(prune_screenshots(&c).unwrap(), 2);
        assert!(has_screenshot(&c, "pf_u1").unwrap(), "a queued row's picture stays");
        // The 90-day prune of a failed row takes its picture too.
        give_up_stale(&c, 2 * GIVE_UP_AFTER_MS + 10).unwrap();
        c.execute("UPDATE product_feedback_outbox SET state = 'failed', updated_at = 0 WHERE id = 'pf_u1'", []).unwrap();
        prune(&c, KEEP_FINAL_MS + 1).unwrap();
        assert!(get(&c, "pf_u1").unwrap().is_none());
        assert_eq!(shots(&c), 0);
    }

    #[test]
    fn states_move_only_along_their_edges() {
        let c = db();
        let r = insert(&c, &new("pf_a", "owner", "person"), 1_000).unwrap();
        assert_eq!(r.state, State::Queued);
        assert_eq!(r.next_attempt_at, 1_000);
        assert!(!retry(&c, "pf_a", 2_000).unwrap(), "retry only from failed");
        assert!(!mark_sent(&c, "pf_a", "r1", 2_000).unwrap(), "sent only from sending");
        assert!(mark_sending(&c, "pf_a", 2_000).unwrap());
        assert!(!cancel(&c, "pf_a", 2_000).unwrap(), "cancel only while queued");
        assert!(mark_retry(&c, "pf_a", 32_000, "unreachable", 2_000).unwrap());
        let r = get(&c, "pf_a").unwrap().unwrap();
        assert_eq!((r.state, r.attempts, r.next_attempt_at), (State::Queued, 1, 32_000));
        assert_eq!(due(&c, 31_999, 10).unwrap().len(), 0, "not due before next_attempt_at");
        assert_eq!(due(&c, 32_000, 10).unwrap().len(), 1);
        assert!(mark_sending(&c, "pf_a", 33_000).unwrap());
        assert!(mark_sent(&c, "pf_a", "remote-1", 34_000).unwrap());
        let r = get(&c, "pf_a").unwrap().unwrap();
        assert_eq!((r.state, r.remote_id.as_deref(), r.sent_at, r.last_error), (State::Sent, Some("remote-1"), Some(34_000), None));
    }

    #[test]
    fn a_restart_mid_sending_goes_back_to_queued() {
        let c = db();
        insert(&c, &new("pf_b", "owner", "person"), 1_000).unwrap();
        mark_sending(&c, "pf_b", 2_000).unwrap();
        assert_eq!(recover_at_boot(&c, 5_000).unwrap(), 1);
        let r = get(&c, "pf_b").unwrap().unwrap();
        assert_eq!((r.state, r.next_attempt_at), (State::Queued, 5_000));
    }

    #[test]
    fn seven_days_of_retrying_is_failed_and_retry_restarts_the_window() {
        let c = db();
        insert(&c, &new("pf_c", "owner", "person"), 0).unwrap();
        // Never attempted (held): not given up.
        assert!(give_up_stale(&c, GIVE_UP_AFTER_MS + 1).unwrap().is_empty());
        mark_retry(&c, "pf_c", 10, "unreachable", 5).unwrap();
        assert!(give_up_stale(&c, GIVE_UP_AFTER_MS - 1).unwrap().is_empty());
        assert_eq!(give_up_stale(&c, GIVE_UP_AFTER_MS + 1).unwrap(), vec!["pf_c".to_string()]);
        let r = get(&c, "pf_c").unwrap().unwrap();
        assert_eq!((r.state, r.last_error.as_deref()), (State::Failed, Some("unreachable")));
        let now = GIVE_UP_AFTER_MS + 100;
        assert!(retry(&c, "pf_c", now).unwrap());
        let r = get(&c, "pf_c").unwrap().unwrap();
        assert_eq!((r.state, r.queued_at, r.attempts, r.next_attempt_at), (State::Queued, now, 0, now));
    }

    #[test]
    fn refused_is_final_and_cancel_only_from_queued() {
        let c = db();
        insert(&c, &new("pf_d", "owner", "person"), 0).unwrap();
        assert!(mark_refused(&c, "pf_d", "box_blocked", 1).unwrap());
        assert!(!retry(&c, "pf_d", 2).unwrap(), "a refused row is not retried");
        insert(&c, &new("pf_e", "owner", "person"), 0).unwrap();
        assert!(cancel(&c, "pf_e", 3).unwrap());
        assert_eq!(get(&c, "pf_e").unwrap().unwrap().state, State::Cancelled);
    }

    #[test]
    fn prune_drops_old_finals_and_clears_sent_diagnostics() {
        let c = db();
        let mut n = new("pf_f", "owner", "person");
        n.diagnostics_json = Some("[]".into());
        insert(&c, &n, 0).unwrap();
        mark_sending(&c, "pf_f", 0).unwrap();
        mark_sent(&c, "pf_f", "r", 0).unwrap();
        insert(&c, &new("pf_g", "owner", "person"), 0).unwrap();
        assert_eq!(prune(&c, KEEP_SENT_DIAGNOSTICS_MS + 1).unwrap(), (0, 1));
        assert_eq!(get(&c, "pf_f").unwrap().unwrap().diagnostics_json, None);
        assert_eq!(prune(&c, KEEP_FINAL_MS + 1).unwrap(), (1, 0));
        assert!(get(&c, "pf_f").unwrap().is_none());
        assert!(get(&c, "pf_g").unwrap().is_some(), "a queued row is never pruned");
    }

    #[test]
    fn agent_limits_per_minute_day_and_box() {
        let c = db();
        let t = 10 * 86_400_000;
        insert(&c, &new("pf_h1", "agent:w1", "agent"), t).unwrap();
        assert_eq!(agent_limit(&c, "agent:w1", t + 1_000).unwrap(), Some(AgentLimit::PerMinute));
        assert_eq!(agent_limit(&c, "agent:w1", t + 61_000).unwrap(), None);
        insert(&c, &new("pf_h2", "agent:w1", "agent"), t + 61_000).unwrap();
        insert(&c, &new("pf_h3", "agent:w1", "agent"), t + 122_000).unwrap();
        assert_eq!(agent_limit(&c, "agent:w1", t + 200_000).unwrap(), Some(AgentLimit::PerDay));
        // People never count toward an agent's limit.
        insert(&c, &new("pf_p", "owner", "person"), t).unwrap();
        // Nine more agents on the box, each once: the box limit after ten.
        for i in 0..7 {
            insert(&c, &new(&format!("pf_x{i}"), &format!("agent:o{i}"), "agent"), t + 300_000).unwrap();
        }
        assert_eq!(agent_limit(&c, "agent:new", t + 400_000).unwrap(), Some(AgentLimit::BoxPerDay));
        assert_eq!(agent_limit(&c, "agent:new", t + 86_400_000 + 400_000).unwrap(), None, "a day later");
    }

    /// A row that tried for 7 days and failed with `error`, created at
    /// `created`.
    fn failed_row(c: &Connection, id: &str, created: i64, error: &str) {
        insert(c, &new(id, "owner", "person"), created).unwrap();
        mark_sending(c, id, created + 1).unwrap();
        assert!(mark_retry(c, id, created + 2, error, created + 2).unwrap());
        let n = c
            .execute(
                "UPDATE product_feedback_outbox SET state = 'failed', updated_at = ?2 WHERE id = ?1 AND state = 'queued'",
                params![id, created + GIVE_UP_AFTER_MS + 1],
            )
            .unwrap();
        assert_eq!(n, 1, "{id} failed");
    }

    #[test]
    fn which_failed_reasons_go_again_by_themselves() {
        for e in [
            "service_unreachable: connection refused",
            "relay_not_ready: 404",
            "intake_paused: maintenance",
            "rate_limited: scope box",
        ] {
            assert!(resends_by_itself(Some(e)), "{e}");
        }
        for e in [
            "disabled: Sending feedback to K2 is turned off on this server",
            "airgap: Air-gap is on",
            "needs_connect: This server isn't paired with K2 Connect",
            "relay_refused: forbidden",
            "token_rejected: replayed",
            "bad_body: no",
            "service_unreachable",
            "not sent within 7 days",
            "",
        ] {
            assert!(!resends_by_itself(Some(e)), "{e:?}");
        }
        assert!(!resends_by_itself(None));
    }

    #[test]
    fn the_service_answering_requeues_unreachable_failed_rows_oldest_first() {
        let c = db();
        let day = 86_400_000;
        let now = 200 * day;
        // Inserted newest first, so the order below is the requeue's own.
        failed_row(&c, "pf_r3", now - 10 * day, "intake_paused: maintenance");
        failed_row(&c, "pf_r2", now - 20 * day, "relay_not_ready: 404");
        failed_row(&c, "pf_r1", now - 30 * day, "service_unreachable: connection refused");
        insert(&c, &with_shot("pf_r0", b"kept"), now - 40 * day).unwrap();
        c.execute(
            "UPDATE product_feedback_outbox SET state = 'failed', attempts = 9, last_error = 'rate_limited: wait' WHERE id = 'pf_r0'",
            [],
        )
        .unwrap();
        assert!(has_resendable_failed(&c, now).unwrap(), "the health check is worth asking");
        let ids = requeue_unreachable_failed(&c, now).unwrap();
        assert_eq!(ids, ["pf_r0", "pf_r1", "pf_r2", "pf_r3"].map(String::from).to_vec());
        for id in &ids {
            let r = get(&c, id).unwrap().unwrap();
            assert_eq!(
                (r.state, r.queued_at, r.next_attempt_at, r.attempts, r.updated_at),
                (State::Queued, now, now, 0, now),
                "{id}: queued, due now, a fresh 7-day window"
            );
        }
        assert!(has_screenshot(&c, "pf_r0").unwrap(), "the picture goes with it");
        let due_ids: Vec<String> = due(&c, now, 10).unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(due_ids, ids, "and they go oldest first");
        assert!(requeue_unreachable_failed(&c, now).unwrap().is_empty(), "once");
        assert!(!has_resendable_failed(&c, now).unwrap());
    }

    #[test]
    fn a_refused_or_a_persons_failed_row_stays_failed() {
        let c = db();
        let now = 200 * 86_400_000;
        failed_row(&c, "pf_k1", now - 86_400_000, "disabled: Sending feedback to K2 is turned off on this server");
        failed_row(&c, "pf_k2", now - 86_400_000, "airgap: Air-gap is on");
        failed_row(&c, "pf_k3", now - 86_400_000, "needs_connect: not paired");
        failed_row(&c, "pf_k4", now - 86_400_000, "token_rejected: replayed");
        failed_row(&c, "pf_k5", now - 86_400_000, "relay_refused: forbidden");
        insert(&c, &new("pf_k6", "owner", "person"), now - 86_400_000).unwrap();
        assert!(mark_refused(&c, "pf_k6", "bad_category: no", now - 1).unwrap());
        insert(&c, &new("pf_k7", "owner", "person"), now - 86_400_000).unwrap();
        assert!(cancel(&c, "pf_k7", now - 1).unwrap());
        assert!(!has_resendable_failed(&c, now).unwrap(), "no health check for these");
        assert!(requeue_unreachable_failed(&c, now).unwrap().is_empty());
        for id in ["pf_k1", "pf_k2", "pf_k3", "pf_k4", "pf_k5"] {
            assert_eq!(get(&c, id).unwrap().unwrap().state, State::Failed, "{id}");
        }
        assert_eq!(get(&c, "pf_k6").unwrap().unwrap().state, State::Refused);
        assert_eq!(get(&c, "pf_k7").unwrap().unwrap().state, State::Cancelled);
        // Retry still puts any failed one back by hand.
        assert!(retry(&c, "pf_k1", now).unwrap());
        assert_eq!(get(&c, "pf_k1").unwrap().unwrap().state, State::Queued);
    }

    #[test]
    fn only_rows_created_within_90_days_go_again_by_themselves() {
        let c = db();
        let now = 400 * 86_400_000;
        failed_row(&c, "pf_old", now - RESEND_MAX_AGE_MS - 1, "service_unreachable: timeout");
        failed_row(&c, "pf_edge", now - RESEND_MAX_AGE_MS, "service_unreachable: timeout");
        assert!(has_resendable_failed(&c, now).unwrap());
        assert_eq!(requeue_unreachable_failed(&c, now).unwrap(), vec!["pf_edge".to_string()]);
        assert_eq!(get(&c, "pf_old").unwrap().unwrap().state, State::Failed);
        assert!(!has_resendable_failed(&c, now).unwrap(), "the old one alone asks nothing");
    }

    /// 0.46.18: rows a reused box token left failed (`401 replayed`) go
    /// back to the queue; no other failure does, nor one older than 90 days.
    #[test]
    fn rows_failed_on_a_replayed_token_go_back_and_nothing_else() {
        assert_eq!(REPLAYED_ERROR, format!("{}: {}", super::super::wire::E_TOKEN, super::super::wire::E_REPLAYED), "what the drain stores");
        let c = db();
        let day = 86_400_000;
        let now = 200 * day;
        failed_row(&c, "pf_p2", now - 2 * day, REPLAYED_ERROR);
        failed_row(&c, "pf_p1", now - 3 * day, REPLAYED_ERROR);
        failed_row(&c, "pf_old", now - RESEND_MAX_AGE_MS - 1, REPLAYED_ERROR);
        failed_row(&c, "pf_bad", now - day, "token_rejected: bad_assertion");
        failed_row(&c, "pf_off", now - day, "disabled: Sending feedback to K2 is turned off on this server");
        failed_row(&c, "pf_net", now - day, "service_unreachable: connection refused");
        let ids = requeue_replayed_failed(&c, now).unwrap();
        assert_eq!(ids, ["pf_p1", "pf_p2"].map(String::from).to_vec(), "oldest first");
        for id in &ids {
            let r = get(&c, id).unwrap().unwrap();
            assert_eq!((r.state, r.queued_at, r.next_attempt_at, r.attempts), (State::Queued, now, now, 0), "{id}");
        }
        for id in ["pf_old", "pf_bad", "pf_off", "pf_net"] {
            assert_eq!(get(&c, id).unwrap().unwrap().state, State::Failed, "{id}");
        }
        assert!(requeue_replayed_failed(&c, now).unwrap().is_empty(), "nothing left");
    }

    #[test]
    fn remembered_choice_is_per_person() {
        let c = db();
        assert!(!remember_diagnostics(&c, "alice").unwrap());
        set_remember_diagnostics(&c, "alice", true).unwrap();
        assert!(remember_diagnostics(&c, "alice").unwrap());
        assert!(!remember_diagnostics(&c, "bob").unwrap());
        set_remember_diagnostics(&c, "alice", false).unwrap();
        assert!(!remember_diagnostics(&c, "alice").unwrap());
    }
}
