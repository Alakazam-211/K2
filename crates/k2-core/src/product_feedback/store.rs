//! The outbox (`product_feedback_outbox`, migration 0285) and a person's
//! remembered diagnostics choice (`product_feedback_prefs`).
//!
//! States: `queued` → `sending` → `sent`; a retryable failure goes back to
//! `queued` with a later `next_attempt_at`; a reject the intake understood
//! is `refused` (never retried); 7 days of retrying is `failed` (Retry puts
//! it back); a person can cancel a `queued` row. `sending` rows found at
//! boot go back to `queued`: the intake's idempotency key makes a resend
//! harmless. Times are milliseconds.

use rusqlite::{params, Connection, OptionalExtension, Row as SqlRow};
use serde::Serialize;

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
}

const COLS: &str = "id, created_at, queued_at, updated_at, sender_kind, sender, client, category, body, \
     contact_email, meta_json, state, attempts, next_attempt_at, last_error, remote_id, sent_at, diagnostics_json";

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
    })
}

/// Store a new `queued` row, due now.
pub fn insert(conn: &Connection, n: &NewRow, now: i64) -> rusqlite::Result<Row> {
    conn.execute(
        "INSERT INTO product_feedback_outbox (id, created_at, queued_at, updated_at, sender_kind, sender, client, \
         category, body, contact_email, meta_json, state, attempts, next_attempt_at, diagnostics_json) \
         VALUES (?1, ?2, ?2, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'queued', 0, ?2, ?10)",
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
            n.diagnostics_json
        ],
    )?;
    get(conn, &n.id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
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

/// `sending` → `sent`, keeping the intake's id.
pub fn mark_sent(conn: &Connection, id: &str, remote_id: &str, now: i64) -> rusqlite::Result<bool> {
    set_state(
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

/// The intake understood and rejected it: never retried.
pub fn mark_refused(conn: &Connection, id: &str, error: &str, now: i64) -> rusqlite::Result<bool> {
    set_state(
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

/// Cancel: `queued` → `cancelled`.
pub fn cancel(conn: &Connection, id: &str, now: i64) -> rusqlite::Result<bool> {
    set_state(conn, id, &["queued"], "state = 'cancelled', updated_at = ?2", &[&now])
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

/// Delete final rows older than [`KEEP_FINAL_MS`]; clear the diagnostics
/// of rows sent more than [`KEEP_SENT_DIAGNOSTICS_MS`] ago. Returns
/// `(deleted, cleared)`.
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
            AgentLimit::PerMinute => format!("An agent may send feedback to K2 {AGENT_PER_MINUTE} time a minute. Try again in a minute."),
            AgentLimit::PerDay => format!("An agent may send feedback to K2 {AGENT_PER_DAY} times a day. Try again tomorrow, or ask your human to send it."),
            AgentLimit::BoxPerDay => format!(
                "Agents on this server have sent {AGENTS_PER_DAY} feedback messages to K2 today. Try again tomorrow, or ask your human to send it."
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
        }
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
