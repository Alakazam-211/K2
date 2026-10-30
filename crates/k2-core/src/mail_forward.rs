//! Hosted mail forwards: the k2.db store (migration 0256,
//! prd-hostmail-external-forwarding-v1 §3.2, §3.6, §3.7; vs-live FW-A5,
//! FW-A6, FW-A8).
//!
//! One row per forward; at most one live (not `removed`) row per mailbox.
//! The daemon writes the mailbox's Sieve from the live row
//! (`mail::forward_sync`); nothing here talks to the mail server.
//!
//! ```text
//!   set (outside) ──► pending_verify ──(confirm link)──► active ◄──(resume)── paused
//!   set (same box) ─────────────────────────────────────► active ──(pause)───► paused
//!   any live state ──(unset / replaced / revoked before confirm)──► removed
//! ```
//!
//! [`transition`] is the only way a row changes state, and it refuses any
//! move the diagram doesn't have. Every change writes one
//! `mail_forward_events` row (the audit list).
//!
//! Outcomes ([`record_outcome`]) are per forwarded message, joined from the
//! mail server's webhook by queue id ([`link_queue`]); the counts feed the
//! caps, the auto-pause and the box brake. Nothing here logs an address.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value as J;

pub const ST_PENDING_VERIFY: &str = "pending_verify";
pub const ST_ACTIVE: &str = "active";
pub const ST_PAUSED: &str = "paused";
pub const ST_REMOVED: &str = "removed";

pub const KIND_INTERNAL: &str = "internal";
pub const KIND_EXTERNAL: &str = "external";

// ── pause reasons (`pause_reason`) ──────────────────────────────────
/// The destination said the mailbox doesn't exist / is disabled
/// (5.1.x, 5.2.1) [`DEST_GONE_IN_A_ROW`] times in a row.
pub const P_DEST_GONE: &str = "dest_gone";
/// More than [`REFUSED_PCT`]% refused for policy or spam (5.7.x) over 24 h
/// with at least [`REFUSED_MIN`] forwards.
pub const P_REFUSED: &str = "refused";
/// The destination pressed "stop" (its verified address was revoked).
pub const P_REVOKED: &str = "revoked";
/// [`DAILY_CAP`] forwards in the trailing 24 h.
pub const P_DAILY_CAP: &str = "daily_cap";
/// The box brake: the whole box's forwards are being refused.
pub const P_BOX_BRAKE: &str = "box_brake";
/// The mailbox's domain is `receive-only`.
pub const P_RECEIVE_ONLY: &str = "receive_only";
/// Air-gap is on.
pub const P_AIR_GAP: &str = "air_gap";
/// Someone paused it by hand.
pub const P_MANUAL: &str = "manual";

/// Pauses K2 lifts by itself when the cause goes away (the reconcile).
pub fn auto_lifts(reason: &str) -> bool {
    reason == P_RECEIVE_ONLY || reason == P_AIR_GAP
}

// ── events (`mail_forward_events.event`) ────────────────────────────
pub const E_SET: &str = "set";
pub const E_VERIFY_SENT: &str = "verify_sent";
pub const E_VERIFIED: &str = "verified";
pub const E_ACTIVATED: &str = "activated";
pub const E_PAUSED: &str = "paused";
pub const E_RESUMED: &str = "resumed";
pub const E_REMOVED: &str = "removed";
pub const E_REPLACED: &str = "replaced";
pub const E_DRIFT: &str = "drift";
pub const E_IMPORTED: &str = "imported";
pub const E_KEEP: &str = "keep_changed";

// ── outcomes (`mail_forward_outcomes.kind`) ─────────────────────────
pub const O_QUEUED: &str = "queued";
pub const O_DELIVERED: &str = "delivered";
pub const O_REFUSED_USER: &str = "refused_user";
pub const O_REFUSED_POLICY: &str = "refused_policy";
pub const O_FAILED: &str = "failed";

// ── limits (PRD §3.5, §3.6; vs-live Q6, Q7) ─────────────────────────
/// Per mailbox per hour (the mail server's throttle defers above it).
pub const HOURLY_CAP: i64 = 100;
/// Per mailbox per trailing 24 h (K2 pauses at it).
pub const DAILY_CAP: i64 = 500;
/// Per box per hour across all mailboxes (the mail server defers above it).
pub const BOX_HOURLY_CAP: i64 = 1000;
pub const WINDOW_SECS: i64 = 24 * 3600;
pub const DEST_GONE_IN_A_ROW: i64 = 3;
pub const REFUSED_PCT: i64 = 20;
pub const REFUSED_MIN: i64 = 10;
pub const BRAKE_PCT: i64 = 10;
pub const BRAKE_MIN: i64 = 50;

/// Retention.
pub const EVENTS_KEEP_SECS: i64 = 365 * 86_400;
pub const QUEUE_KEEP_SECS: i64 = 7 * 86_400;
pub const OUTCOMES_KEEP_SECS: i64 = 30 * 86_400;

fn err(what: &str) -> impl Fn(rusqlite::Error) -> String + '_ {
    move |e| format!("mail forwards {what}: {e}")
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Forward {
    pub id: String,
    /// `mail_addresses.id` of the forwarding mailbox.
    pub address_id: String,
    /// The forwarding mailbox's address.
    pub address: String,
    pub dest: String,
    /// [`KIND_INTERNAL`] | [`KIND_EXTERNAL`].
    pub dest_kind: String,
    /// `email_verified_addresses.id` (outside destinations).
    pub verified_address_id: Option<String>,
    pub keep: bool,
    pub state: String,
    pub set_by: String,
    pub pause_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub activated_at: Option<i64>,
    pub paused_at: Option<i64>,
    pub removed_at: Option<i64>,
}

impl Forward {
    pub fn is_external(&self) -> bool {
        self.dest_kind == KIND_EXTERNAL
    }
    pub fn is_live(&self) -> bool {
        self.state != ST_REMOVED
    }
    pub fn is_active(&self) -> bool {
        self.state == ST_ACTIVE
    }
}

const COLS: &str = "id, address_id, address, dest, dest_kind, verified_address_id, keep, state, set_by, pause_reason, \
                    created_at, updated_at, activated_at, paused_at, removed_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Forward> {
    Ok(Forward {
        id: r.get(0)?,
        address_id: r.get(1)?,
        address: r.get(2)?,
        dest: r.get(3)?,
        dest_kind: r.get(4)?,
        verified_address_id: r.get(5)?,
        keep: r.get::<_, i64>(6)? == 1,
        state: r.get(7)?,
        set_by: r.get(8)?,
        pause_reason: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        activated_at: r.get(12)?,
        paused_at: r.get(13)?,
        removed_at: r.get(14)?,
    })
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Forward>, String> {
    conn.query_row(&format!("SELECT {COLS} FROM mail_forwards WHERE id = ?1"), params![id], row)
        .optional()
        .map_err(err("get"))
}

/// The mailbox's live (not removed) forward.
pub fn live_for_address(conn: &Connection, address_id: &str) -> Result<Option<Forward>, String> {
    conn.query_row(
        &format!("SELECT {COLS} FROM mail_forwards WHERE address_id = ?1 AND state <> 'removed'"),
        params![address_id],
        row,
    )
    .optional()
    .map_err(err("live"))
}

/// The newest row of the mailbox (live or removed), for the panel.
pub fn latest_for_address(conn: &Connection, address_id: &str) -> Result<Option<Forward>, String> {
    conn.query_row(
        &format!(
            "SELECT {COLS} FROM mail_forwards WHERE address_id = ?1 \
             ORDER BY (state <> 'removed') DESC, updated_at DESC, id DESC LIMIT 1"
        ),
        params![address_id],
        row,
    )
    .optional()
    .map_err(err("latest"))
}

/// Live rows whose verified address is `verified_id` (the stop link).
pub fn live_for_verified(conn: &Connection, verified_id: &str) -> Result<Vec<Forward>, String> {
    let mut st = conn
        .prepare(&format!("SELECT {COLS} FROM mail_forwards WHERE verified_address_id = ?1 AND state <> 'removed' ORDER BY id"))
        .map_err(err("by verified"))?;
    let rows = st.query_map(params![verified_id], row).map_err(err("by verified"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("by verified"))
}

/// Live `pending_verify` rows of one mailbox address and destination.
pub fn pending_for(conn: &Connection, address: &str, dest: &str) -> Result<Vec<Forward>, String> {
    let mut st = conn
        .prepare(&format!(
            "SELECT {COLS} FROM mail_forwards WHERE address = ?1 AND dest = ?2 AND state = 'pending_verify' ORDER BY id"
        ))
        .map_err(err("pending"))?;
    let rows = st.query_map(params![address, dest], row).map_err(err("pending"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("pending"))
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter<'a> {
    /// One state only.
    pub state: Option<&'a str>,
    /// Outside destinations only.
    pub external_only: bool,
    /// Removed rows too.
    pub include_removed: bool,
}

pub fn list(conn: &Connection, f: &ListFilter<'_>) -> Result<Vec<Forward>, String> {
    let mut sql = format!("SELECT {COLS} FROM mail_forwards WHERE 1 = 1");
    if !f.include_removed && f.state != Some(ST_REMOVED) {
        sql.push_str(" AND state <> 'removed'");
    }
    if f.external_only {
        sql.push_str(" AND dest_kind = 'external'");
    }
    if f.state.is_some() {
        sql.push_str(" AND state = ?1");
    }
    sql.push_str(" ORDER BY address, updated_at DESC, id");
    let mut st = conn.prepare(&sql).map_err(err("list"))?;
    let rows = match f.state {
        Some(s) => st.query_map(params![s], row).map_err(err("list"))?.collect::<Result<Vec<_>, _>>(),
        None => st.query_map([], row).map_err(err("list"))?.collect::<Result<Vec<_>, _>>(),
    };
    rows.map_err(err("list"))
}

/// Every mailbox (`address_id`) that has any forward row: the reconcile's
/// work list (a removed row's mailbox still needs its block gone).
pub fn address_ids_with_rows(conn: &Connection, since_removed: i64) -> Result<Vec<String>, String> {
    let mut st = conn
        .prepare(
            "SELECT DISTINCT address_id FROM mail_forwards \
             WHERE state <> 'removed' OR removed_at >= ?1 ORDER BY address_id",
        )
        .map_err(err("work list"))?;
    let rows = st.query_map(params![since_removed], |r| r.get::<_, String>(0)).map_err(err("work list"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("work list"))
}

/// A new forward.
#[derive(Debug, Clone)]
pub struct NewForward<'a> {
    pub address_id: &'a str,
    pub address: &'a str,
    pub dest: &'a str,
    pub dest_kind: &'a str,
    pub keep: bool,
    pub set_by: &'a str,
    /// [`ST_PENDING_VERIFY`] or [`ST_ACTIVE`] (an internal one, or an
    /// outside one whose address is already verified).
    pub state: &'a str,
    pub verified_address_id: Option<&'a str>,
}

/// Insert a new forward. Any live row of the same mailbox is retired
/// first (`removed`, event [`E_REPLACED`]). One [`E_SET`] event (and
/// [`E_ACTIVATED`] when it starts active).
pub fn create(conn: &Connection, n: &NewForward<'_>, now: i64) -> Result<Forward, String> {
    if n.dest_kind != KIND_INTERNAL && n.dest_kind != KIND_EXTERNAL {
        return Err(format!("mail forwards: unknown destination kind '{}'", n.dest_kind));
    }
    if n.state != ST_PENDING_VERIFY && n.state != ST_ACTIVE {
        return Err(format!("mail forwards: a new forward can't start '{}'", n.state));
    }
    if n.dest_kind == KIND_EXTERNAL && n.state == ST_ACTIVE && n.verified_address_id.is_none() {
        return Err("mail forwards: an outside destination starts active only when verified".to_string());
    }
    let tx = conn.unchecked_transaction().map_err(err("create"))?;
    if let Some(old) = live_for_address(&tx, n.address_id)? {
        tx.execute(
            "UPDATE mail_forwards SET state = 'removed', pause_reason = NULL, removed_at = ?2, updated_at = ?2 WHERE id = ?1",
            params![old.id, now],
        )
        .map_err(err("replace"))?;
        record_event(&tx, &old.id, E_REPLACED, n.set_by, &serde_json::json!({ "from": old.state }), now)?;
    }
    let id = new_id();
    let activated = (n.state == ST_ACTIVE).then_some(now);
    tx.execute(
        "INSERT INTO mail_forwards (id, address_id, address, dest, dest_kind, verified_address_id, keep, state, set_by, \
         created_at, updated_at, activated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?11)",
        params![
            id,
            n.address_id,
            n.address,
            n.dest,
            n.dest_kind,
            n.verified_address_id,
            n.keep as i64,
            n.state,
            n.set_by,
            now,
            activated
        ],
    )
    .map_err(err("create"))?;
    record_event(&tx, &id, E_SET, n.set_by, &serde_json::json!({ "kind": n.dest_kind, "keep": n.keep, "state": n.state }), now)?;
    if n.state == ST_ACTIVE {
        record_event(&tx, &id, E_ACTIVATED, n.set_by, &serde_json::json!({}), now)?;
    }
    tx.commit().map_err(err("create"))?;
    get(conn, &id)?.ok_or_else(|| "mail forwards: the new row vanished".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionError {
    NotFound,
    /// (from, to): the diagram has no such move.
    Illegal(String, String),
    Db(String),
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransitionError::NotFound => write!(f, "no such forward"),
            TransitionError::Illegal(a, b) => write!(f, "a forward can't go from {a} to {b}"),
            TransitionError::Db(e) => write!(f, "{e}"),
        }
    }
}

/// Is `from → to` a move the state diagram has?
pub fn legal(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        (ST_PENDING_VERIFY, ST_ACTIVE)
            | (ST_PENDING_VERIFY, ST_REMOVED)
            | (ST_ACTIVE, ST_PAUSED)
            | (ST_ACTIVE, ST_REMOVED)
            | (ST_PAUSED, ST_ACTIVE)
            | (ST_PAUSED, ST_REMOVED)
            | (ST_PAUSED, ST_PAUSED)
    )
}

/// What a [`transition`] carries.
#[derive(Debug, Clone, Default)]
pub struct Change<'a> {
    /// Required for [`ST_PAUSED`].
    pub reason: Option<&'a str>,
    /// Set with the move to [`ST_ACTIVE`] from `pending_verify`.
    pub verified_address_id: Option<&'a str>,
    pub actor: &'a str,
    pub detail: Option<J>,
}

/// Move one forward. `paused → paused` only changes the reason. The event
/// written: `activated` (from pending_verify, plus `verified`), `resumed`
/// (paused → active), `paused`, `removed`.
pub fn transition(conn: &Connection, id: &str, to: &str, c: &Change<'_>, now: i64) -> Result<Forward, TransitionError> {
    let db = |e: String| TransitionError::Db(e);
    let cur = get(conn, id).map_err(db)?.ok_or(TransitionError::NotFound)?;
    if !legal(&cur.state, to) {
        return Err(TransitionError::Illegal(cur.state.clone(), to.to_string()));
    }
    let detail = c.detail.clone().unwrap_or_else(|| serde_json::json!({}));
    let tx = conn.unchecked_transaction().map_err(|e| TransitionError::Db(e.to_string()))?;
    let run = |sql: &str, p: &[&dyn rusqlite::ToSql]| tx.execute(sql, p).map_err(|e| TransitionError::Db(format!("mail forwards move: {e}")));
    match to {
        ST_ACTIVE => {
            let verified = c.verified_address_id.or(cur.verified_address_id.as_deref());
            if cur.is_external() && verified.is_none() {
                return Err(TransitionError::Illegal(cur.state.clone(), "active (unverified)".to_string()));
            }
            run(
                "UPDATE mail_forwards SET state = 'active', pause_reason = NULL, paused_at = NULL, verified_address_id = ?2, \
                 activated_at = COALESCE(activated_at, ?3), updated_at = ?3 WHERE id = ?1",
                &[&id, &verified, &now],
            )?;
            if cur.state == ST_PENDING_VERIFY {
                record_event(&tx, id, E_VERIFIED, c.actor, &serde_json::json!({}), now).map_err(db)?;
                record_event(&tx, id, E_ACTIVATED, c.actor, &detail, now).map_err(db)?;
            } else {
                record_event(&tx, id, E_RESUMED, c.actor, &detail, now).map_err(db)?;
            }
        }
        ST_PAUSED => {
            let reason = c.reason.filter(|r| !r.is_empty()).ok_or_else(|| TransitionError::Illegal(cur.state.clone(), "paused (no reason)".to_string()))?;
            run(
                "UPDATE mail_forwards SET state = 'paused', pause_reason = ?2, paused_at = COALESCE(paused_at, ?3), updated_at = ?3 WHERE id = ?1",
                &[&id, &reason, &now],
            )?;
            let mut d = detail.clone();
            if let J::Object(m) = &mut d {
                m.insert("reason".to_string(), J::String(reason.to_string()));
            }
            record_event(&tx, id, E_PAUSED, c.actor, &d, now).map_err(db)?;
        }
        ST_REMOVED => {
            run(
                "UPDATE mail_forwards SET state = 'removed', pause_reason = NULL, removed_at = ?2, updated_at = ?2 WHERE id = ?1",
                &[&id, &now],
            )?;
            record_event(&tx, id, E_REMOVED, c.actor, &detail, now).map_err(db)?;
        }
        other => return Err(TransitionError::Illegal(cur.state.clone(), other.to_string())),
    }
    tx.commit().map_err(|e| TransitionError::Db(e.to_string()))?;
    get(conn, id).map_err(db)?.ok_or(TransitionError::NotFound)
}

/// Change the keep-a-copy setting of a live row.
pub fn set_keep(conn: &Connection, id: &str, keep: bool, actor: &str, now: i64) -> Result<bool, String> {
    let n = conn
        .execute(
            "UPDATE mail_forwards SET keep = ?2, updated_at = ?3 WHERE id = ?1 AND state <> 'removed' AND keep <> ?2",
            params![id, keep as i64, now],
        )
        .map_err(err("keep"))?;
    if n == 1 {
        record_event(conn, id, E_KEEP, actor, &serde_json::json!({ "keep": keep }), now)?;
    }
    Ok(n == 1)
}

// ── audit ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: String,
    pub forward_id: String,
    pub event: String,
    pub actor: String,
    pub detail: J,
    pub at: i64,
}

/// One audit row. `detail` holds ids, counts and reasons, never an address.
pub fn record_event(conn: &Connection, forward_id: &str, event: &str, actor: &str, detail: &J, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO mail_forward_events (id, forward_id, event, actor, detail_json, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![new_id(), forward_id, event, actor, detail.to_string(), now],
    )
    .map_err(err("event"))?;
    Ok(())
}

/// The audit list of one mailbox (all its forwards), newest first.
pub fn events_for_address(conn: &Connection, address_id: &str, limit: usize) -> Result<Vec<Event>, String> {
    let mut st = conn
        .prepare(
            "SELECT e.id, e.forward_id, e.event, e.actor, e.detail_json, e.at FROM mail_forward_events e \
             JOIN mail_forwards f ON f.id = e.forward_id WHERE f.address_id = ?1 \
             ORDER BY e.at DESC, e.rowid DESC LIMIT ?2",
        )
        .map_err(err("events"))?;
    let rows = st
        .query_map(params![address_id, limit as i64], |r| {
            let raw: String = r.get(4)?;
            Ok(Event {
                id: r.get(0)?,
                forward_id: r.get(1)?,
                event: r.get(2)?,
                actor: r.get(3)?,
                detail: serde_json::from_str(&raw).unwrap_or(J::Null),
                at: r.get(5)?,
            })
        })
        .map_err(err("events"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("events"))
}

// ── the webhook join and outcomes ───────────────────────────────────

/// Keep queue id → forward. `Ok(false)`: already linked.
pub fn link_queue(conn: &Connection, queue_id: &str, forward_id: &str, now: i64) -> Result<bool, String> {
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO mail_forward_queue (queue_id, forward_id, at) VALUES (?1, ?2, ?3)",
            params![queue_id, forward_id, now],
        )
        .map_err(err("link"))?;
    Ok(n == 1)
}

pub fn forward_for_queue(conn: &Connection, queue_id: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT forward_id FROM mail_forward_queue WHERE queue_id = ?1", params![queue_id], |r| r.get(0))
        .optional()
        .map_err(err("join"))
}

/// Record one outcome. With a queue id, a repeat (same queue id, same
/// kind) is a no-op: `Ok(false)`.
pub fn record_outcome(conn: &Connection, forward_id: &str, queue_id: Option<&str>, kind: &str, code: Option<&str>, now: i64) -> Result<bool, String> {
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO mail_forward_outcomes (forward_id, queue_id, kind, code, at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![forward_id, queue_id, kind, code, now],
        )
        .map_err(err("outcome"))?;
    Ok(n == 1)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub queued: i64,
    pub delivered: i64,
    pub refused_user: i64,
    pub refused_policy: i64,
    pub failed: i64,
}

impl Counts {
    /// Every refusal (for the box brake).
    pub fn refused(&self) -> i64 {
        self.refused_user + self.refused_policy + self.failed
    }
}

fn counts_where(conn: &Connection, filter: &str, p: &[&dyn rusqlite::ToSql]) -> Result<Counts, String> {
    let mut st = conn
        .prepare(&format!("SELECT kind, COUNT(*) FROM mail_forward_outcomes WHERE {filter} GROUP BY kind"))
        .map_err(err("counts"))?;
    let rows = st.query_map(p, |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).map_err(err("counts"))?;
    let mut c = Counts::default();
    for r in rows {
        let (k, n) = r.map_err(err("counts"))?;
        match k.as_str() {
            O_QUEUED => c.queued = n,
            O_DELIVERED => c.delivered = n,
            O_REFUSED_USER => c.refused_user = n,
            O_REFUSED_POLICY => c.refused_policy = n,
            O_FAILED => c.failed = n,
            _ => {}
        }
    }
    Ok(c)
}

/// One forward's outcomes since `since`.
pub fn counts(conn: &Connection, forward_id: &str, since: i64) -> Result<Counts, String> {
    counts_where(conn, "forward_id = ?1 AND at >= ?2", &[&forward_id, &since])
}

/// Every forward's outcomes since `since` (the box brake).
pub fn box_counts(conn: &Connection, since: i64) -> Result<Counts, String> {
    counts_where(conn, "at >= ?1", &[&since])
}

/// How many `refused_user` outcomes the forward got in a row, newest
/// first, stopping at its last `delivered` (and at its last resume).
pub fn user_refusals_in_a_row(conn: &Connection, forward_id: &str, since: i64) -> Result<i64, String> {
    let mut st = conn
        .prepare(
            "SELECT kind FROM mail_forward_outcomes WHERE forward_id = ?1 AND at >= ?2 AND kind IN ('delivered', 'refused_user') \
             ORDER BY at DESC, id DESC",
        )
        .map_err(err("in a row"))?;
    let rows = st.query_map(params![forward_id, since], |r| r.get::<_, String>(0)).map_err(err("in a row"))?;
    let mut n = 0;
    for k in rows {
        if k.map_err(err("in a row"))? != O_REFUSED_USER {
            break;
        }
        n += 1;
    }
    Ok(n)
}

/// Drop rows past their keep time.
pub fn purge(conn: &Connection, now: i64) -> Result<(), String> {
    conn.execute("DELETE FROM mail_forward_queue WHERE at < ?1", params![now - QUEUE_KEEP_SECS]).map_err(err("purge"))?;
    conn.execute("DELETE FROM mail_forward_outcomes WHERE at < ?1", params![now - OUTCOMES_KEEP_SECS]).map_err(err("purge"))?;
    conn.execute("DELETE FROM mail_forward_events WHERE at < ?1", params![now - EVENTS_KEEP_SECS]).map_err(err("purge"))?;
    Ok(())
}

// ── the person link (FW-A8) ─────────────────────────────────────────

/// The person key that owns a mailbox (`None`: not linked).
pub fn person_principal(conn: &Connection, address_id: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT person_principal FROM mail_addresses WHERE id = ?1", params![address_id], |r| r.get::<_, Option<String>>(0))
        .optional()
        .map(|o| o.flatten().filter(|s| !s.trim().is_empty()))
        .map_err(err("person"))
}

/// Link (or unlink with `None`) a mailbox to a person key.
pub fn set_person_principal(conn: &Connection, address_id: &str, person: Option<&str>) -> Result<bool, String> {
    let n = conn
        .execute("UPDATE mail_addresses SET person_principal = ?2 WHERE id = ?1", params![address_id, person])
        .map_err(err("person link"))?;
    Ok(n == 1)
}

/// `(address_id, address)` of the active mailboxes linked to `person`.
pub fn mailboxes_of_person(conn: &Connection, person: &str) -> Result<Vec<(String, String)>, String> {
    let mut st = conn
        .prepare("SELECT id, address FROM mail_addresses WHERE person_principal = ?1 AND status = 'active' ORDER BY address")
        .map_err(err("person mailboxes"))?;
    let rows = st.query_map(params![person], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err("person mailboxes"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(err("person mailboxes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&c).unwrap();
        c
    }

    fn mailbox(c: &Connection, addr: &str) -> String {
        let id = new_id();
        c.execute(
            "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, owner_project_id, status, created_at) \
             VALUES (?1, ?2, 'd', 'acc', 'ws', 'active', 1)",
            params![id, addr],
        )
        .unwrap();
        id
    }

    fn ext<'a>(aid: &'a str, addr: &'a str, dest: &'a str) -> NewForward<'a> {
        NewForward {
            address_id: aid,
            address: addr,
            dest,
            dest_kind: KIND_EXTERNAL,
            keep: true,
            set_by: "owner",
            state: ST_PENDING_VERIFY,
            verified_address_id: None,
        }
    }

    fn actor() -> Change<'static> {
        Change { actor: "owner", ..Default::default() }
    }

    #[test]
    fn outside_forward_walks_the_diagram() {
        let c = conn();
        let aid = mailbox(&c, "sales@brand.test");
        let f = create(&c, &ext(&aid, "sales@brand.test", "me@outside.test"), 10).unwrap();
        assert_eq!(f.state, ST_PENDING_VERIFY);
        // No activation without a verified address.
        let e = transition(&c, &f.id, ST_ACTIVE, &actor(), 11).unwrap_err();
        assert!(matches!(e, TransitionError::Illegal(_, _)), "{e:?}");
        // pending → paused is not a move.
        let e = transition(&c, &f.id, ST_PAUSED, &Change { reason: Some(P_MANUAL), ..actor() }, 11).unwrap_err();
        assert!(matches!(e, TransitionError::Illegal(_, _)), "{e:?}");
        let f = transition(&c, &f.id, ST_ACTIVE, &Change { verified_address_id: Some("v1"), actor: "recipient", ..Default::default() }, 12).unwrap();
        assert_eq!((f.state.as_str(), f.activated_at), (ST_ACTIVE, Some(12)));
        // Pausing needs a reason.
        assert!(transition(&c, &f.id, ST_PAUSED, &actor(), 13).is_err());
        let f = transition(&c, &f.id, ST_PAUSED, &Change { reason: Some(P_DEST_GONE), ..actor() }, 14).unwrap();
        assert_eq!(f.pause_reason.as_deref(), Some(P_DEST_GONE));
        let f = transition(&c, &f.id, ST_ACTIVE, &actor(), 15).unwrap();
        assert_eq!(f.pause_reason, None);
        let f = transition(&c, &f.id, ST_REMOVED, &actor(), 16).unwrap();
        assert_eq!(f.removed_at, Some(16));
        let e = transition(&c, &f.id, ST_ACTIVE, &actor(), 17).unwrap_err();
        assert!(matches!(e, TransitionError::Illegal(_, _)), "removed is final: {e:?}");
        let ev: Vec<String> = events_for_address(&c, &aid, 50).unwrap().into_iter().rev().map(|e| e.event).collect();
        assert_eq!(ev, vec![E_SET, E_VERIFIED, E_ACTIVATED, E_PAUSED, E_RESUMED, E_REMOVED]);
    }

    #[test]
    fn the_schema_refuses_an_unverified_active_outside_row() {
        let c = conn();
        let aid = mailbox(&c, "a@brand.test");
        let mut n = ext(&aid, "a@brand.test", "x@outside.test");
        n.state = ST_ACTIVE;
        assert!(create(&c, &n, 1).is_err(), "store refuses");
        let raw = c.execute(
            "INSERT INTO mail_forwards (id, address_id, address, dest, dest_kind, keep, state, set_by, created_at, updated_at) \
             VALUES ('x', ?1, 'a@brand.test', 'x@outside.test', 'external', 1, 'active', 'owner', 1, 1)",
            params![aid],
        );
        assert!(raw.is_err(), "the CHECK refuses it too");
    }

    #[test]
    fn one_live_row_per_mailbox_and_set_replaces() {
        let c = conn();
        let aid = mailbox(&c, "b@brand.test");
        let a = create(&c, &ext(&aid, "b@brand.test", "one@outside.test"), 1).unwrap();
        let b = create(&c, &ext(&aid, "b@brand.test", "two@outside.test"), 2).unwrap();
        assert_eq!(get(&c, &a.id).unwrap().unwrap().state, ST_REMOVED);
        assert_eq!(live_for_address(&c, &aid).unwrap().unwrap().id, b.id);
        let dup = c.execute(
            "INSERT INTO mail_forwards (id, address_id, address, dest, dest_kind, keep, state, set_by, created_at, updated_at) \
             VALUES ('y', ?1, 'b@brand.test', 'z@brand.test', 'internal', 1, 'active', 'owner', 1, 1)",
            params![aid],
        );
        assert!(dup.is_err(), "a second live row is refused by the index");
    }

    #[test]
    fn outcomes_count_and_dedupe() {
        let c = conn();
        let aid = mailbox(&c, "c@brand.test");
        let f = create(&c, &ext(&aid, "c@brand.test", "d@outside.test"), 1).unwrap();
        assert!(link_queue(&c, "111", &f.id, 5).unwrap());
        assert!(!link_queue(&c, "111", &f.id, 6).unwrap());
        assert_eq!(forward_for_queue(&c, "111").unwrap().as_deref(), Some(f.id.as_str()));
        assert!(record_outcome(&c, &f.id, Some("111"), O_QUEUED, None, 5).unwrap());
        assert!(!record_outcome(&c, &f.id, Some("111"), O_QUEUED, None, 5).unwrap(), "repeat event");
        record_outcome(&c, &f.id, Some("112"), O_DELIVERED, None, 6).unwrap();
        record_outcome(&c, &f.id, Some("113"), O_REFUSED_USER, Some("550"), 7).unwrap();
        record_outcome(&c, &f.id, Some("114"), O_REFUSED_USER, Some("550"), 8).unwrap();
        let k = counts(&c, &f.id, 0).unwrap();
        assert_eq!((k.queued, k.delivered, k.refused_user), (1, 1, 2));
        assert_eq!(user_refusals_in_a_row(&c, &f.id, 0).unwrap(), 2);
        record_outcome(&c, &f.id, Some("115"), O_DELIVERED, None, 9).unwrap();
        assert_eq!(user_refusals_in_a_row(&c, &f.id, 0).unwrap(), 0, "a delivery breaks the run");
        assert_eq!(box_counts(&c, 0).unwrap().refused(), 2);
        purge(&c, 6 + QUEUE_KEEP_SECS).unwrap();
        assert_eq!(forward_for_queue(&c, "111").unwrap(), None);
    }

    #[test]
    fn person_link() {
        let c = conn();
        let aid = mailbox(&c, "p@brand.test");
        assert_eq!(person_principal(&c, &aid).unwrap(), None);
        assert!(set_person_principal(&c, &aid, Some("pat")).unwrap());
        assert_eq!(person_principal(&c, &aid).unwrap().as_deref(), Some("pat"));
        assert_eq!(mailboxes_of_person(&c, "pat").unwrap(), vec![(aid.clone(), "p@brand.test".to_string())]);
        set_person_principal(&c, &aid, None).unwrap();
        assert!(mailboxes_of_person(&c, "pat").unwrap().is_empty());
    }
}
