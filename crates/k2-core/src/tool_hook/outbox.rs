//! Durable outbox (`tool_outbox`) and per-tool targets (`tool_targets`)
//! in `skin.db` (S27 §8).
//!
//! Schema is idempotent `CREATE TABLE IF NOT EXISTS` run from
//! `skin::open_db`, like `grants` — not a ladder step (`user_version`
//! stays 3; step 4 is reserved for room-scope R3), so this rides 0.46.0.x.
//!
//! Row lifecycle: `pending` → `done` | `blocked` | `held` | `skipped` |
//! `superseded`.
//! - `blocked`: a bug or bad data; halts that subject until an owner runs
//!   retry or skip.
//! - `held` (HC2): the tool answered 2xx but listed the subject as
//!   `skipped` (e.g. banned). Final; reconcile leaves the subject alone
//!   until its grant changes (a new [`super::grant_rev`]).
//! - `skipped`: an owner skip, or the target was dropped.
//! - `superseded`: an unattempted `update` replaced by a newer row.
//!
//! Times are Unix seconds (`now_secs`).

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::{NewOp, Op};

pub const STATE_PENDING: &str = "pending";
pub const STATE_DONE: &str = "done";
pub const STATE_BLOCKED: &str = "blocked";
pub const STATE_HELD: &str = "held";
pub const STATE_SKIPPED: &str = "skipped";
pub const STATE_SUPERSEDED: &str = "superseded";

/// Finished rows are kept this long, then pruned (§8.4).
pub const RETENTION_SECS: i64 = 90 * 24 * 3600;

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Idempotent DDL, called from `skin::open_db`.
pub fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_outbox (
            seq              INTEGER PRIMARY KEY AUTOINCREMENT,
            id               TEXT NOT NULL UNIQUE,
            tool             TEXT NOT NULL,
            op               TEXT NOT NULL,
            subject_key      TEXT NOT NULL,
            payload          TEXT NOT NULL,
            idem_key         TEXT NOT NULL,
            grant_id         TEXT,
            grant_rev        TEXT,
            cause            TEXT NOT NULL,
            actor            TEXT NOT NULL,
            after_subject    TEXT,
            state            TEXT NOT NULL,
            attempts         INTEGER NOT NULL DEFAULT 0,
            first_attempt_at INTEGER,
            next_attempt_at  INTEGER NOT NULL,
            last_status      INTEGER,
            last_error       TEXT,
            result           TEXT,
            created_at       INTEGER NOT NULL,
            updated_at       INTEGER NOT NULL,
            done_at          INTEGER
         );
         CREATE INDEX IF NOT EXISTS tool_outbox_due
            ON tool_outbox(tool, state, next_attempt_at);
         CREATE INDEX IF NOT EXISTS tool_outbox_subject
            ON tool_outbox(tool, subject_key, seq);
         CREATE TABLE IF NOT EXISTS tool_targets (
            kind              TEXT PRIMARY KEY,
            enabled           INTEGER NOT NULL DEFAULT 0,
            config            TEXT NOT NULL DEFAULT '{}',
            install_id        TEXT,
            healthy           INTEGER NOT NULL DEFAULT 0,
            last_ok_at        INTEGER,
            last_error        TEXT,
            last_reconcile_at INTEGER,
            last_reconcile    TEXT,
            updated_at        INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db tool_outbox schema: {e}"))
}

/// `k2ob_` + the row uuid without hyphens (37 chars, inside
/// `[A-Za-z0-9_-]{1,64}`).
pub fn idem_key_for(id: &str) -> String {
    format!("k2ob_{}", id.replace('-', ""))
}

/// One outbox row on the wire (camelCase like `SkinGrant`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxRow {
    pub id: String,
    pub seq: i64,
    pub tool: String,
    pub op: String,
    pub subject_key: String,
    /// Only for `blocked` rows on the wire (payloads hold emails).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(skip)]
    pub payload_raw: String,
    #[serde(skip)]
    pub idem_key: String,
    pub grant_id: Option<String>,
    pub grant_rev: Option<String>,
    pub cause: String,
    /// Attribution only ("reported by K2"); never person proof.
    pub actor: String,
    pub after_subject: Option<String>,
    pub state: String,
    pub attempts: i64,
    pub first_attempt_at: Option<i64>,
    pub next_attempt_at: i64,
    pub last_status: Option<i64>,
    pub last_error: Option<String>,
    pub result: Option<serde_json::Value>,
    pub created_at: i64,
    pub updated_at: i64,
    pub done_at: Option<i64>,
}

const ROW_COLS: &str = "id, seq, tool, op, subject_key, payload, idem_key, grant_id,
        grant_rev, cause, actor, after_subject, state, attempts, first_attempt_at,
        next_attempt_at, last_status, last_error, result, created_at, updated_at, done_at";
const ROW_SELECT: &str = "SELECT id, seq, tool, op, subject_key, payload, idem_key, grant_id,
        grant_rev, cause, actor, after_subject, state, attempts, first_attempt_at,
        next_attempt_at, last_status, last_error, result, created_at, updated_at, done_at
     FROM tool_outbox";

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<OutboxRow> {
    let result: Option<String> = r.get(18)?;
    Ok(OutboxRow {
        id: r.get(0)?,
        seq: r.get(1)?,
        tool: r.get(2)?,
        op: r.get(3)?,
        subject_key: r.get(4)?,
        payload: None,
        payload_raw: r.get(5)?,
        idem_key: r.get(6)?,
        grant_id: r.get(7)?,
        grant_rev: r.get(8)?,
        cause: r.get(9)?,
        actor: r.get(10)?,
        after_subject: r.get(11)?,
        state: r.get(12)?,
        attempts: r.get(13)?,
        first_attempt_at: r.get(14)?,
        next_attempt_at: r.get(15)?,
        last_status: r.get(16)?,
        last_error: r.get(17)?,
        result: result.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: r.get(19)?,
        updated_at: r.get(20)?,
        done_at: r.get(21)?,
    })
}

fn collect(
    conn: &Connection,
    sql: &str,
    p: &[&dyn rusqlite::ToSql],
) -> Result<Vec<OutboxRow>, String> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("tool_outbox query: {e}"))?;
    let rows = stmt
        .query_map(p, map_row)
        .map_err(|e| format!("tool_outbox query: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("tool_outbox row: {e}"))?);
    }
    Ok(out)
}

/// Enqueue `ops` in order. MUST run inside the caller's transaction (the
/// grant/principal write's), so a rolled-back write leaves no row and a
/// committed write always has its row. Returns the new row ids.
///
/// Supersede (§8.2): a new `update` or `deprovision` turns older rows of
/// the same subject that are `pending`, `update` and never attempted into
/// `superseded`. Nothing else is ever superseded.
pub fn enqueue(
    conn: &Connection,
    ops: &[NewOp],
    actor: &str,
    now: i64,
) -> Result<Vec<String>, String> {
    let mut ids = Vec::with_capacity(ops.len());
    for o in ops {
        if matches!(o.op, Op::Update | Op::Deprovision) {
            conn.execute(
                "UPDATE tool_outbox SET state = ?1, updated_at = ?2
                 WHERE tool = ?3 AND subject_key = ?4 AND state = ?5
                   AND op = 'update' AND attempts = 0",
                params![STATE_SUPERSEDED, now, o.tool, o.subject_key, STATE_PENDING],
            )
            .map_err(|e| format!("tool_outbox supersede: {e}"))?;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let idem = idem_key_for(&id);
        let payload =
            serde_json::to_string(&o.payload).map_err(|e| format!("tool_outbox payload: {e}"))?;
        conn.execute(
            "INSERT INTO tool_outbox
             (id, tool, op, subject_key, payload, idem_key, grant_id, grant_rev, cause, actor,
              after_subject, state, attempts, next_attempt_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, ?13, ?13, ?13)",
            params![
                id,
                o.tool,
                o.op.as_str(),
                o.subject_key,
                payload,
                idem,
                o.grant_id,
                o.grant_rev,
                o.cause.as_str(),
                actor,
                o.after_subject,
                STATE_PENDING,
                now
            ],
        )
        .map_err(|e| format!("tool_outbox insert: {e}"))?;
        crate::log_debug!(
            "[grant-hook] enqueue {} op={} subject={} cause={} actor={}",
            o.tool,
            o.op.as_str(),
            o.subject_key,
            o.cause.as_str(),
            super::sew_actor_header(actor)
        );
        ids.push(id);
    }
    Ok(ids)
}

fn delivered_ops_sql() -> String {
    Op::DELIVERED
        .iter()
        .map(|o| format!("'{}'", o.as_str()))
        .collect::<Vec<_>>()
        .join(",")
}

/// Due rows for one tool (§8.3): the head of each subject's queue (lowest
/// `seq` among its `pending`/`blocked` rows of ops this worker delivers),
/// when that head is `pending`, due, and its `after_subject` has no open
/// row with a lower `seq`. Ops this worker does not deliver (S28's
/// `sew_session_revoke`) are invisible here: never sent, never blocked,
/// never in the way of the provisioning order.
pub fn due_heads(
    conn: &Connection,
    tool: &str,
    now: i64,
    limit: usize,
) -> Result<Vec<OutboxRow>, String> {
    let ops = delivered_ops_sql();
    let sql = format!(
        "SELECT {ROW_COLS} FROM tool_outbox o
         WHERE o.tool = ?1 AND o.state = 'pending' AND o.next_attempt_at <= ?2
           AND o.op IN ({ops})
           AND NOT EXISTS (SELECT 1 FROM tool_outbox e
                 WHERE e.tool = o.tool AND e.subject_key = o.subject_key
                   AND e.state IN ('pending','blocked') AND e.op IN ({ops}) AND e.seq < o.seq)
           AND (o.after_subject IS NULL OR NOT EXISTS (SELECT 1 FROM tool_outbox d
                 WHERE d.tool = o.tool AND d.subject_key = o.after_subject
                   AND d.state IN ('pending','blocked') AND d.op IN ({ops}) AND d.seq < o.seq))
         ORDER BY o.seq LIMIT ?3"
    );
    collect(conn, &sql, &[&tool, &now, &(limit as i64)])
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<OutboxRow>, String> {
    Ok(
        collect(conn, &format!("{ROW_SELECT} WHERE id = ?1"), &[&id])?
            .into_iter()
            .next(),
    )
}

fn require(conn: &Connection, id: &str) -> Result<OutboxRow, String> {
    get(conn, id)?.ok_or_else(|| format!("unknown outbox row '{id}'"))
}

/// 2xx: `done`, store the response body.
pub fn mark_done(
    conn: &Connection,
    id: &str,
    status: i64,
    result: Option<&str>,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE tool_outbox SET state = ?1, attempts = attempts + 1,
            first_attempt_at = COALESCE(first_attempt_at, ?2), last_status = ?3,
            last_error = NULL, result = ?4, updated_at = ?2, done_at = ?2
         WHERE id = ?5",
        params![STATE_DONE, now, status, result, id],
    )
    .map_err(|e| format!("tool_outbox done: {e}"))?;
    Ok(())
}

/// HC2: 2xx with the subject listed as `skipped`. Final.
pub fn mark_held(
    conn: &Connection,
    id: &str,
    status: i64,
    result: Option<&str>,
    error: &str,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE tool_outbox SET state = ?1, attempts = attempts + 1,
            first_attempt_at = COALESCE(first_attempt_at, ?2), last_status = ?3,
            last_error = ?4, result = ?5, updated_at = ?2, done_at = ?2
         WHERE id = ?6",
        params![STATE_HELD, now, status, error, result, id],
    )
    .map_err(|e| format!("tool_outbox held: {e}"))?;
    Ok(())
}

/// A transient failure: stays `pending`, attempt counted, next try at
/// `next_at`.
pub fn mark_retry(
    conn: &Connection,
    id: &str,
    status: Option<i64>,
    error: &str,
    next_at: i64,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE tool_outbox SET attempts = attempts + 1,
            first_attempt_at = COALESCE(first_attempt_at, ?1), last_status = ?2,
            last_error = ?3, next_attempt_at = ?4, updated_at = ?1
         WHERE id = ?5",
        params![now, status, error, next_at, id],
    )
    .map_err(|e| format!("tool_outbox retry: {e}"))?;
    Ok(())
}

/// A terminal failure: `blocked` halts this subject.
pub fn mark_blocked(
    conn: &Connection,
    id: &str,
    status: Option<i64>,
    error: &str,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE tool_outbox SET state = ?1, attempts = attempts + 1,
            first_attempt_at = COALESCE(first_attempt_at, ?2), last_status = ?3,
            last_error = ?4, updated_at = ?2
         WHERE id = ?5",
        params![STATE_BLOCKED, now, status, error, id],
    )
    .map_err(|e| format!("tool_outbox blocked: {e}"))?;
    Ok(())
}

/// Username conflict, first time (§10.4): new body with the suffixed
/// username under a new key (`<old>_u`), due now. The only rewrite ever.
pub fn rewrite_payload(
    conn: &Connection,
    id: &str,
    payload: &serde_json::Value,
    idem_key: &str,
    status: i64,
    now: i64,
) -> Result<(), String> {
    let body = serde_json::to_string(payload).map_err(|e| format!("tool_outbox payload: {e}"))?;
    conn.execute(
        "UPDATE tool_outbox SET payload = ?1, idem_key = ?2, attempts = attempts + 1,
            first_attempt_at = COALESCE(first_attempt_at, ?3), last_status = ?4,
            last_error = 'username taken; retrying with a suffix', next_attempt_at = ?3,
            updated_at = ?3
         WHERE id = ?5",
        params![body, idem_key, now, status, id],
    )
    .map_err(|e| format!("tool_outbox rewrite: {e}"))?;
    Ok(())
}

/// Owner retry (S27-A11): `blocked`, or `pending` that already failed at
/// least once → `pending`, due now, attempts kept.
pub fn retry(conn: &Connection, id: &str, now: i64) -> Result<OutboxRow, String> {
    let row = require(conn, id)?;
    let ok = row.state == STATE_BLOCKED || (row.state == STATE_PENDING && row.attempts >= 1);
    if !ok {
        return Err(format!(
            "outbox row '{id}' is {} with {} attempt(s): retry needs a blocked row or a pending row that has failed",
            row.state, row.attempts
        ));
    }
    conn.execute(
        "UPDATE tool_outbox SET state = ?1, next_attempt_at = ?2, updated_at = ?2 WHERE id = ?3",
        params![STATE_PENDING, now, id],
    )
    .map_err(|e| format!("tool_outbox retry: {e}"))?;
    require(conn, id)
}

/// Owner skip: `blocked` → `skipped` only.
pub fn skip(conn: &Connection, id: &str, now: i64) -> Result<OutboxRow, String> {
    let row = require(conn, id)?;
    if row.state != STATE_BLOCKED {
        return Err(format!(
            "outbox row '{id}' is {}: only a blocked row can be skipped",
            row.state
        ));
    }
    conn.execute(
        "UPDATE tool_outbox SET state = ?1, updated_at = ?2, done_at = ?2 WHERE id = ?3",
        params![STATE_SKIPPED, now, id],
    )
    .map_err(|e| format!("tool_outbox skip: {e}"))?;
    require(conn, id)
}

/// `pending` / `blocked` rows of one tool → `skipped` (target dropped).
pub fn skip_open_rows(conn: &Connection, tool: &str, now: i64, why: &str) -> Result<usize, String> {
    conn.execute(
        "UPDATE tool_outbox SET state = ?1, last_error = ?2, updated_at = ?3, done_at = ?3
         WHERE tool = ?4 AND state IN ('pending','blocked')",
        params![STATE_SKIPPED, why, now, tool],
    )
    .map_err(|e| format!("tool_outbox skip all: {e}"))
}

/// Newest first, `limit` ≤ 500. Payload only on `blocked` rows.
pub fn list(
    conn: &Connection,
    limit: usize,
    subject: Option<&str>,
) -> Result<Vec<OutboxRow>, String> {
    let limit = limit.clamp(1, 500) as i64;
    let mut rows = match subject {
        Some(s) => collect(
            conn,
            &format!("{ROW_SELECT} WHERE subject_key = ?1 ORDER BY seq DESC LIMIT ?2"),
            &[&s, &limit],
        )?,
        None => collect(
            conn,
            &format!("{ROW_SELECT} ORDER BY seq DESC LIMIT ?1"),
            &[&limit],
        )?,
    };
    for r in &mut rows {
        if r.state == STATE_BLOCKED {
            r.payload = serde_json::from_str(&r.payload_raw).ok();
        }
    }
    Ok(rows)
}

pub fn rows_in_state(
    conn: &Connection,
    state: &str,
    limit: usize,
    with_payload: bool,
) -> Result<Vec<OutboxRow>, String> {
    let mut rows = collect(
        conn,
        &format!("{ROW_SELECT} WHERE state = ?1 ORDER BY seq LIMIT ?2"),
        &[&state, &(limit as i64)],
    )?;
    if with_payload {
        for r in &mut rows {
            r.payload = serde_json::from_str(&r.payload_raw).ok();
        }
    }
    Ok(rows)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxSummary {
    pub pending: i64,
    pub blocked: i64,
    pub held: i64,
    pub oldest_pending_age_secs: Option<i64>,
}

pub fn summary(conn: &Connection, now: i64) -> Result<OutboxSummary, String> {
    let count = |state: &str| -> Result<i64, String> {
        conn.query_row(
            "SELECT COUNT(*) FROM tool_outbox WHERE state = ?1",
            params![state],
            |r| r.get(0),
        )
        .map_err(|e| format!("tool_outbox count: {e}"))
    };
    let oldest: Option<i64> = conn
        .query_row(
            "SELECT MIN(created_at) FROM tool_outbox WHERE state = 'pending'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("tool_outbox oldest: {e}"))?;
    Ok(OutboxSummary {
        pending: count(STATE_PENDING)?,
        blocked: count(STATE_BLOCKED)?,
        held: count(STATE_HELD)?,
        oldest_pending_age_secs: oldest.map(|t| (now - t).max(0)),
    })
}

/// Subjects with any `pending` or `blocked` row: reconcile leaves them alone.
pub fn busy_subjects(conn: &Connection, tool: &str) -> Result<HashSet<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT subject_key FROM tool_outbox
             WHERE tool = ?1 AND state IN ('pending','blocked')",
        )
        .map_err(|e| format!("tool_outbox busy: {e}"))?;
    let rows = stmt
        .query_map(params![tool], |r| r.get::<_, String>(0))
        .map_err(|e| format!("tool_outbox busy: {e}"))?;
    let mut out = HashSet::new();
    for r in rows {
        out.insert(r.map_err(|e| format!("tool_outbox busy row: {e}"))?);
    }
    Ok(out)
}

/// HC2: subjects whose NEWEST row is `held`, with the grant rev it was
/// held under. Reconcile skips them while the rev is unchanged.
pub fn held_subjects(
    conn: &Connection,
    tool: &str,
) -> Result<HashMap<String, Option<String>>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT o.subject_key, o.grant_rev FROM tool_outbox o
             WHERE o.tool = ?1 AND o.state = 'held'
               AND o.seq = (SELECT MAX(x.seq) FROM tool_outbox x
                            WHERE x.tool = o.tool AND x.subject_key = o.subject_key
                              AND x.state != 'superseded')",
        )
        .map_err(|e| format!("tool_outbox held: {e}"))?;
    let rows = stmt
        .query_map(params![tool], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })
        .map_err(|e| format!("tool_outbox held: {e}"))?;
    let mut out = HashMap::new();
    for r in rows {
        let (k, rev) = r.map_err(|e| format!("tool_outbox held row: {e}"))?;
        out.insert(k, rev);
    }
    Ok(out)
}

/// `synced` | `pending` | `blocked` | `held` (+ error) from a subject's
/// newest non-superseded row. No row yet → `pending` (nothing delivered).
pub fn subject_hook_state(
    conn: &Connection,
    tool: &str,
    subject: &str,
) -> Result<(String, Option<String>), String> {
    let open: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT state, last_error FROM tool_outbox
             WHERE tool = ?1 AND subject_key = ?2 AND state IN ('pending','blocked')
             ORDER BY CASE state WHEN 'blocked' THEN 0 ELSE 1 END, seq LIMIT 1",
            params![tool, subject],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("tool_outbox state: {e}"))?;
    if let Some(s) = open {
        return Ok(s);
    }
    let newest: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT state, last_error FROM tool_outbox
             WHERE tool = ?1 AND subject_key = ?2 AND state != 'superseded'
             ORDER BY seq DESC LIMIT 1",
            params![tool, subject],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("tool_outbox state: {e}"))?;
    Ok(match newest {
        Some((s, e)) if s == STATE_HELD => (STATE_HELD.to_string(), e),
        Some(_) => ("synced".to_string(), None),
        None => (STATE_PENDING.to_string(), None),
    })
}

/// Prune finished rows older than [`RETENTION_SECS`]. `pending` /
/// `blocked` / `held` are never pruned (`held` is HC2's memory).
pub fn prune(conn: &Connection, now: i64) -> Result<usize, String> {
    conn.execute(
        "DELETE FROM tool_outbox
         WHERE state IN ('done','skipped','superseded') AND COALESCE(done_at, updated_at) < ?1",
        params![now - RETENTION_SECS],
    )
    .map_err(|e| format!("tool_outbox prune: {e}"))
}

/// `set | pending | blocked | none` from the box owner's rows (S27-A10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OwnerState {
    Set,
    Pending,
    Blocked,
    None,
}

pub fn owner_state(conn: &Connection, tool: &str) -> Result<OwnerState, String> {
    let owner = super::sew_wire::ExternalId::K2Owner.to_string();
    let blocked: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tool_outbox WHERE tool = ?1 AND subject_key = ?2 AND state = 'blocked'",
            params![tool, owner],
            |r| r.get(0),
        )
        .map_err(|e| format!("tool_outbox owner: {e}"))?;
    if blocked > 0 {
        return Ok(OwnerState::Blocked);
    }
    let pending: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tool_outbox WHERE tool = ?1 AND subject_key = ?2 AND state = 'pending'
               AND op IN ('provision','owner_set')",
            params![tool, owner],
            |r| r.get(0),
        )
        .map_err(|e| format!("tool_outbox owner: {e}"))?;
    if pending > 0 {
        return Ok(OwnerState::Pending);
    }
    let set: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tool_outbox WHERE tool = ?1 AND subject_key = ?2
               AND op = 'owner_set' AND state = 'done'",
            params![tool, owner],
            |r| r.get(0),
        )
        .map_err(|e| format!("tool_outbox owner: {e}"))?;
    Ok(if set > 0 {
        OwnerState::Set
    } else {
        OwnerState::None
    })
}

// ── targets ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolTarget {
    pub kind: String,
    pub enabled: bool,
    pub socket: String,
    pub token_file: String,
    pub install_id: Option<String>,
    pub healthy: bool,
    pub last_ok_at: Option<i64>,
    pub last_error: Option<String>,
    pub last_reconcile_at: Option<i64>,
    pub last_reconcile: Option<serde_json::Value>,
    pub updated_at: i64,
}

const TARGET_SELECT: &str = "SELECT kind, enabled, config, install_id, healthy, last_ok_at,
        last_error, last_reconcile_at, last_reconcile, updated_at FROM tool_targets";

fn map_target(r: &rusqlite::Row<'_>) -> rusqlite::Result<ToolTarget> {
    let config: String = r.get(2)?;
    let cfg: serde_json::Value = serde_json::from_str(&config).unwrap_or_default();
    let last: Option<String> = r.get(8)?;
    Ok(ToolTarget {
        kind: r.get(0)?,
        enabled: r.get::<_, i64>(1)? != 0,
        socket: cfg
            .get("socket")
            .and_then(|v| v.as_str())
            .unwrap_or(super::DEFAULT_MGMT_SOCKET)
            .to_string(),
        token_file: cfg
            .get("tokenFile")
            .and_then(|v| v.as_str())
            .unwrap_or(super::DEFAULT_MGMT_TOKEN_FILE)
            .to_string(),
        install_id: r.get(3)?,
        healthy: r.get::<_, i64>(4)? != 0,
        last_ok_at: r.get(5)?,
        last_error: r.get(6)?,
        last_reconcile_at: r.get(7)?,
        last_reconcile: last.and_then(|s| serde_json::from_str(&s).ok()),
        updated_at: r.get(9)?,
    })
}

pub fn get_target(conn: &Connection, kind: &str) -> Result<Option<ToolTarget>, String> {
    conn.query_row(
        &format!("{TARGET_SELECT} WHERE kind = ?1"),
        params![kind],
        map_target,
    )
    .optional()
    .map_err(|e| format!("tool_targets lookup: {e}"))
}

pub fn list_targets(conn: &Connection) -> Result<Vec<ToolTarget>, String> {
    let mut stmt = conn
        .prepare(&format!("{TARGET_SELECT} ORDER BY kind"))
        .map_err(|e| format!("tool_targets list: {e}"))?;
    let rows = stmt
        .query_map([], map_target)
        .map_err(|e| format!("tool_targets list: {e}"))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| format!("tool_targets row: {e}"))?);
    }
    Ok(out)
}

pub fn target_enabled(conn: &Connection, kind: &str) -> Result<bool, String> {
    Ok(get_target(conn, kind)?.is_some_and(|t| t.enabled))
}

/// Upsert. `None` socket/token keep the stored value (or the default).
/// Returns the row and whether this call turned it on.
pub fn upsert_target(
    conn: &Connection,
    kind: &str,
    enabled: bool,
    socket: Option<&str>,
    token_file: Option<&str>,
    now: i64,
) -> Result<(ToolTarget, bool), String> {
    let old = get_target(conn, kind)?;
    let was_on = old.as_ref().is_some_and(|t| t.enabled);
    let clean = |v: Option<&str>| {
        v.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let socket = clean(socket)
        .or_else(|| old.as_ref().map(|t| t.socket.clone()))
        .unwrap_or_else(|| super::DEFAULT_MGMT_SOCKET.to_string());
    let token_file = clean(token_file)
        .or_else(|| old.as_ref().map(|t| t.token_file.clone()))
        .unwrap_or_else(|| super::DEFAULT_MGMT_TOKEN_FILE.to_string());
    for (name, p) in [("socket", &socket), ("tokenFile", &token_file)] {
        if !p.starts_with('/') {
            return Err(format!("{name} must be an absolute path"));
        }
    }
    let config = serde_json::json!({ "socket": socket, "tokenFile": token_file }).to_string();
    conn.execute(
        "INSERT INTO tool_targets (kind, enabled, config, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(kind) DO UPDATE SET enabled = excluded.enabled, config = excluded.config,
            updated_at = excluded.updated_at",
        params![kind, if enabled { 1 } else { 0 }, config, now],
    )
    .map_err(|e| format!("tool_targets upsert: {e}"))?;
    let t = get_target(conn, kind)?.ok_or_else(|| "tool_targets upsert lost".to_string())?;
    Ok((t, enabled && !was_on))
}

pub fn set_target_health(
    conn: &Connection,
    kind: &str,
    healthy: bool,
    error: Option<&str>,
    now: i64,
) -> Result<(), String> {
    if healthy {
        conn.execute(
            "UPDATE tool_targets SET healthy = 1, last_ok_at = ?1, last_error = NULL WHERE kind = ?2",
            params![now, kind],
        )
    } else {
        conn.execute(
            "UPDATE tool_targets SET healthy = 0, last_error = ?1 WHERE kind = ?2",
            params![error, kind],
        )
    }
    .map_err(|e| format!("tool_targets health: {e}"))?;
    Ok(())
}

pub fn set_target_install_id(
    conn: &Connection,
    kind: &str,
    install_id: &str,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE tool_targets SET install_id = ?1, updated_at = ?2 WHERE kind = ?3",
        params![install_id, now, kind],
    )
    .map_err(|e| format!("tool_targets install id: {e}"))?;
    Ok(())
}

pub fn set_target_reconcile(
    conn: &Connection,
    kind: &str,
    summary: Option<&serde_json::Value>,
    error: Option<&str>,
    now: i64,
) -> Result<(), String> {
    match summary {
        Some(s) => conn.execute(
            "UPDATE tool_targets SET last_reconcile_at = ?1, last_reconcile = ?2 WHERE kind = ?3",
            params![now, s.to_string(), kind],
        ),
        None => conn.execute(
            "UPDATE tool_targets SET last_error = ?1 WHERE kind = ?2",
            params![error, kind],
        ),
    }
    .map_err(|e| format!("tool_targets reconcile: {e}"))?;
    Ok(())
}

/// Tests: make every pending row due now (no sleeping on backoff).
#[cfg(any(test, feature = "test-util"))]
pub fn force_due_for_tests(conn: &Connection) -> Result<usize, String> {
    conn.execute(
        "UPDATE tool_outbox SET next_attempt_at = 0 WHERE state = 'pending'",
        [],
    )
    .map_err(|e| format!("tool_outbox force due: {e}"))
}
