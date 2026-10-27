//! SQLite store for Sew chat (migration 0234): per-Space agent access, the
//! write-attempt audit (`sew_chat_actions`), the approval queue
//! (`sew_chat_outbox`) and the per-box policy (`sew_chat_settings`).
//!
//! Fail closed: a stored word that doesn't parse is an error, never a
//! default. Every queue transition is one `UPDATE … WHERE id = ? AND status
//! = 'queued'`; zero rows changed means someone else decided first.
//! Bodies live only on a queued row and are cleared on every decision.

use rusqlite::{params, Connection, OptionalExtension, Row};

use super::{Action, ApprovalMode, ApprovalsBy, Level, APPROVAL_TTL_SECS, POLICY_APPROVALS_BY};

fn db(e: rusqlite::Error) -> String {
    e.to_string()
}

// ── access ───────────────────────────────────────────────────────────

/// One (agent workspace, Space) grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    pub project_id: String,
    pub space_id: String,
    pub space_slug: String,
    pub level: Level,
    pub approval: ApprovalMode,
    pub updated_by: String,
    pub updated_at: i64,
}

fn access_from(r: &Row) -> rusqlite::Result<(String, String, String, String, String, String, i64)> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
}

fn parse_access(t: (String, String, String, String, String, String, i64)) -> Result<Access, String> {
    Ok(Access {
        project_id: t.0,
        space_id: t.1,
        space_slug: t.2,
        level: Level::parse(&t.3).map_err(|e| format!("stored access is unreadable: {e}"))?,
        approval: ApprovalMode::parse(&t.4).map_err(|e| format!("stored access is unreadable: {e}"))?,
        updated_by: t.5,
        updated_at: t.6,
    })
}

const ACCESS_COLS: &str = "project_id, space_id, space_slug, level, approval, updated_by, updated_at";

/// The workspace's access in one Space (`None` = no chat there).
pub fn get_access(conn: &Connection, project_id: &str, space_id: &str) -> Result<Option<Access>, String> {
    let t = conn
        .query_row(
            &format!("SELECT {ACCESS_COLS} FROM sew_chat_access WHERE project_id = ?1 AND space_id = ?2"),
            params![project_id, space_id],
            access_from,
        )
        .optional()
        .map_err(db)?;
    t.map(parse_access).transpose()
}

/// Every Space the workspace has access to, by slug.
pub fn list_access(conn: &Connection, project_id: &str) -> Result<Vec<Access>, String> {
    let mut s = conn
        .prepare(&format!("SELECT {ACCESS_COLS} FROM sew_chat_access WHERE project_id = ?1 ORDER BY space_slug"))
        .map_err(db)?;
    let rows = s.query_map(params![project_id], access_from).map_err(db)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(parse_access(r.map_err(db)?)?);
    }
    Ok(out)
}

/// Grant or change access (the K19 admin routes and tests write here).
pub fn set_access(conn: &Connection, a: &Access) -> Result<(), String> {
    conn.execute(
        &format!(
            "INSERT INTO sew_chat_access ({ACCESS_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(project_id, space_id) DO UPDATE SET space_slug = excluded.space_slug, level = excluded.level,
               approval = excluded.approval, updated_by = excluded.updated_by, updated_at = excluded.updated_at"
        ),
        params![a.project_id, a.space_id, a.space_slug, a.level.as_str(), a.approval.as_str(), a.updated_by, a.updated_at],
    )
    .map_err(db)?;
    // S31: a first grant starts the workspace's delivery hub.
    super::hub::kick();
    Ok(())
}

/// Remove access to one Space and cancel the workspace's queued items
/// there (bodies cleared). Returns how many were cancelled.
pub fn revoke_access(conn: &Connection, project_id: &str, space_id: &str, by: &str, now: i64) -> Result<usize, String> {
    let tx = conn.unchecked_transaction().map_err(db)?;
    tx.execute("DELETE FROM sew_chat_access WHERE project_id = ?1 AND space_id = ?2", params![project_id, space_id])
        .map_err(db)?;
    let ids: Vec<String> = {
        let mut s = tx
            .prepare("SELECT id FROM sew_chat_outbox WHERE project_id = ?1 AND space_id = ?2 AND status = 'queued'")
            .map_err(db)?;
        let rows = s.query_map(params![project_id, space_id], |r| r.get(0)).map_err(db)?;
        rows.collect::<Result<_, _>>().map_err(db)?
    };
    let d = Decision { status: OutboxStatus::Cancelled, by, kind: DecidedKind::System, method: None, note: Some("access removed") };
    for id in &ids {
        decide_in(&tx, id, &d, now).map_err(|e| format!("{e:?}"))?;
    }
    tx.commit().map_err(db)?;
    // S31: the last revoke stops the workspace's delivery hub.
    super::hub::kick();
    Ok(ids.len())
}

// ── policy ───────────────────────────────────────────────────────────

/// Who may approve queued posts (absent = `people`).
pub fn approvals_by(conn: &Connection) -> Result<ApprovalsBy, String> {
    let v: Option<String> = conn
        .query_row("SELECT value FROM sew_chat_settings WHERE key = ?1", params![POLICY_APPROVALS_BY], |r| r.get(0))
        .optional()
        .map_err(db)?;
    match v {
        None => Ok(ApprovalsBy::People),
        Some(v) => ApprovalsBy::parse(&v).map_err(|e| format!("stored policy is unreadable: {e}")),
    }
}

pub fn set_approvals_by(conn: &Connection, by: ApprovalsBy, who: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO sew_chat_settings (key, value, updated_by, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
        params![POLICY_APPROVALS_BY, by.as_str(), who, now],
    )
    .map_err(db)?;
    Ok(())
}

/// The settings key naming K2's default Space (its slug): the Space whose
/// access governs DMs outside every Space.
pub const DEFAULT_SPACE_KEY: &str = "default_space";

/// The default Space's slug, if one is set.
pub fn default_space(conn: &Connection) -> Result<Option<String>, String> {
    let v: Option<String> = conn
        .query_row("SELECT value FROM sew_chat_settings WHERE key = ?1", params![DEFAULT_SPACE_KEY], |r| r.get(0))
        .optional()
        .map_err(db)?;
    Ok(v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
}

/// Set (`Some(slug)`) or clear (`None`) the default Space.
pub fn set_default_space(conn: &Connection, slug: Option<&str>, who: &str, now: i64) -> Result<(), String> {
    match slug {
        Some(s) => conn
            .execute(
                "INSERT INTO sew_chat_settings (key, value, updated_by, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
                params![DEFAULT_SPACE_KEY, s, who, now],
            )
            .map_err(db)?,
        None => conn.execute("DELETE FROM sew_chat_settings WHERE key = ?1", params![DEFAULT_SPACE_KEY]).map_err(db)?,
    };
    Ok(())
}

// ── actions (audit + idempotency) ────────────────────────────────────

/// One write attempt. No content: only the body's hash and size.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActionRow {
    pub id: String,
    pub project_id: String,
    pub k2_session: String,
    pub action: String,
    pub space_id: Option<String>,
    pub channel_id: Option<String>,
    pub target_id: Option<String>,
    pub addr: String,
    pub on_behalf_of: Option<String>,
    pub level: Option<String>,
    pub approval: Option<String>,
    pub addressed: Option<bool>,
    pub status: String,
    pub reason: Option<String>,
    pub client_key: String,
    pub body_sha256: Option<String>,
    pub body_bytes: Option<i64>,
    pub outbox_id: Option<String>,
    pub message_id: Option<String>,
    pub sew_status: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

const ACTION_COLS: &str = "id, project_id, k2_session, action, space_id, channel_id, target_id, addr, on_behalf_of, level, approval, \
     addressed, status, reason, client_key, body_sha256, body_bytes, outbox_id, message_id, sew_status, created_at, updated_at";

fn action_from(r: &Row) -> rusqlite::Result<ActionRow> {
    Ok(ActionRow {
        id: r.get(0)?,
        project_id: r.get(1)?,
        k2_session: r.get(2)?,
        action: r.get(3)?,
        space_id: r.get(4)?,
        channel_id: r.get(5)?,
        target_id: r.get(6)?,
        addr: r.get(7)?,
        on_behalf_of: r.get(8)?,
        level: r.get(9)?,
        approval: r.get(10)?,
        addressed: r.get::<_, Option<i64>>(11)?.map(|v| v == 1),
        status: r.get(12)?,
        reason: r.get(13)?,
        client_key: r.get(14)?,
        body_sha256: r.get(15)?,
        body_bytes: r.get(16)?,
        outbox_id: r.get(17)?,
        message_id: r.get(18)?,
        sew_status: r.get(19)?,
        created_at: r.get(20)?,
        updated_at: r.get(21)?,
    })
}

/// Insert an attempt. A live row with the same key is a constraint error
/// (the caller replays it instead).
pub fn insert_action(conn: &Connection, a: &ActionRow) -> Result<(), String> {
    conn.execute(
        &format!(
            "INSERT INTO sew_chat_actions ({ACTION_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)"
        ),
        params![
            a.id,
            a.project_id,
            a.k2_session,
            a.action,
            a.space_id,
            a.channel_id,
            a.target_id,
            a.addr,
            a.on_behalf_of,
            a.level,
            a.approval,
            a.addressed.map(i64::from),
            a.status,
            a.reason,
            a.client_key,
            a.body_sha256,
            a.body_bytes,
            a.outbox_id,
            a.message_id,
            a.sew_status,
            a.created_at,
            a.updated_at
        ],
    )
    .map_err(db)?;
    Ok(())
}

/// The live attempt (not refused/failed) for a workspace's key, if any.
pub fn live_action_by_key(conn: &Connection, project_id: &str, key: &str) -> Result<Option<ActionRow>, String> {
    conn.query_row(
        &format!(
            "SELECT {ACTION_COLS} FROM sew_chat_actions WHERE project_id = ?1 AND client_key = ?2
             AND status IN ('sending','posted','queued','approved','denied','cancelled')"
        ),
        params![project_id, key],
        action_from,
    )
    .optional()
    .map_err(db)
}

pub fn action(conn: &Connection, id: &str) -> Result<Option<ActionRow>, String> {
    conn.query_row(&format!("SELECT {ACTION_COLS} FROM sew_chat_actions WHERE id = ?1"), params![id], action_from)
        .optional()
        .map_err(db)
}

/// Every attempt by a workspace, newest first (audit; no content).
pub fn actions_for(conn: &Connection, project_id: &str, limit: i64) -> Result<Vec<ActionRow>, String> {
    let mut s = conn
        .prepare(&format!(
            "SELECT {ACTION_COLS} FROM sew_chat_actions WHERE project_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2"
        ))
        .map_err(db)?;
    let rows = s.query_map(params![project_id, limit], action_from).map_err(db)?;
    rows.collect::<Result<_, _>>().map_err(db)
}

/// Close a `sending` attempt: `posted` (with the Sew message id) or
/// `failed` (with Sew's status and code).
pub fn finish_action(
    conn: &Connection,
    id: &str,
    status: &str,
    message_id: Option<&str>,
    reason: Option<&str>,
    sew_status: Option<i64>,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE sew_chat_actions SET status = ?2, message_id = COALESCE(?3, message_id), reason = COALESCE(?4, reason),
           sew_status = COALESCE(?5, sew_status), updated_at = ?6 WHERE id = ?1",
        params![id, status, message_id, reason, sew_status, now],
    )
    .map_err(db)?;
    // An on-behalf write's review follows it: it learns its message id, and
    // goes when nothing reached Sew (failed, denied or cancelled).
    if matches!(status, "failed" | "refused" | "denied" | "cancelled") {
        conn.execute("DELETE FROM sew_chat_behalf WHERE action_id = ?1", params![id]).map_err(db)?;
    } else if message_id.is_some() {
        conn.execute("UPDATE sew_chat_behalf SET message_id = ?2 WHERE action_id = ?1", params![id, message_id])
            .map_err(db)?;
    }
    Ok(())
}

/// Attempts of one class since `since` (every status counts, so the limit
/// survives a restart), and the oldest counted time.
pub fn count_since(conn: &Connection, project_id: &str, content: bool, since: i64) -> Result<(i64, Option<i64>), String> {
    let kinds = if content { "('send','reply','dm','edit')" } else { "('delete')" };
    conn.query_row(
        &format!(
            "SELECT COUNT(*), MIN(created_at) FROM sew_chat_actions WHERE project_id = ?1 AND created_at >= ?2 AND action IN {kinds}"
        ),
        params![project_id, since],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .map_err(db)
}

// ── outbox (approval queue) ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxStatus {
    Queued,
    Approved,
    Denied,
    Cancelled,
}

impl OutboxStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            OutboxStatus::Queued => "queued",
            OutboxStatus::Approved => "approved",
            OutboxStatus::Denied => "denied",
            OutboxStatus::Cancelled => "cancelled",
        }
    }
}

/// Who decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecidedKind {
    /// A person with a vault confirm.
    Person,
    /// An agent (Manage Sew, box policy people+agents). Never person proof.
    Agent,
    /// Expiry, access removal, the agent's own cancel.
    System,
}

impl DecidedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DecidedKind::Person => "person",
            DecidedKind::Agent => "agent",
            DecidedKind::System => "system",
        }
    }
}

/// One queued (or decided) write.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OutboxRow {
    pub id: String,
    pub action_id: String,
    pub project_id: String,
    pub k2_session: String,
    pub agent_name: String,
    pub action: String,
    pub space_id: Option<String>,
    pub channel_id: String,
    pub target_id: Option<String>,
    pub addr: String,
    pub on_behalf_of: Option<String>,
    /// The person's Sew external id (`k2:owner` / `k2:principal:<uuid>`).
    pub on_behalf_person: Option<String>,
    pub broadcast: bool,
    pub client_key: String,
    /// Present only while queued.
    pub body: Option<String>,
    pub body_sha256: String,
    pub body_bytes: i64,
    pub message_kind: Option<String>,
    pub wake: Option<bool>,
    pub response: Option<String>,
    pub status: String,
    pub decided_by: Option<String>,
    pub decided_kind: Option<String>,
    pub confirm_method: Option<String>,
    pub note: Option<String>,
    pub delivery: Option<String>,
    pub message_id: Option<String>,
    pub sew_code: Option<String>,
    /// Why it queued: `approval_required` (the Space's mode) or
    /// `on_behalf` (an assistant posting as a person who hasn't opted in).
    pub queue_reason: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub decided_at: Option<i64>,
}

const OUTBOX_COLS: &str = "id, action_id, project_id, k2_session, agent_name, action, space_id, channel_id, target_id, addr, \
     on_behalf_of, on_behalf_person, broadcast, client_key, body, body_sha256, body_bytes, message_kind, wake, response, status, decided_by, \
     decided_kind, confirm_method, note, delivery, message_id, sew_code, queue_reason, created_at, expires_at, decided_at";

fn outbox_from(r: &Row) -> rusqlite::Result<OutboxRow> {
    Ok(OutboxRow {
        id: r.get(0)?,
        action_id: r.get(1)?,
        project_id: r.get(2)?,
        k2_session: r.get(3)?,
        agent_name: r.get(4)?,
        action: r.get(5)?,
        space_id: r.get(6)?,
        channel_id: r.get(7)?,
        target_id: r.get(8)?,
        addr: r.get(9)?,
        on_behalf_of: r.get(10)?,
        on_behalf_person: r.get(11)?,
        broadcast: r.get::<_, i64>(12)? == 1,
        client_key: r.get(13)?,
        body: r.get(14)?,
        body_sha256: r.get(15)?,
        body_bytes: r.get(16)?,
        message_kind: r.get(17)?,
        wake: r.get::<_, Option<i64>>(18)?.map(|v| v == 1),
        response: r.get(19)?,
        status: r.get(20)?,
        decided_by: r.get(21)?,
        decided_kind: r.get(22)?,
        confirm_method: r.get(23)?,
        note: r.get(24)?,
        delivery: r.get(25)?,
        message_id: r.get(26)?,
        sew_code: r.get(27)?,
        queue_reason: r.get(28)?,
        created_at: r.get(29)?,
        expires_at: r.get(30)?,
        decided_at: r.get(31)?,
    })
}

/// Queue a write: the action row (`queued`) and the outbox row in one
/// transaction. The row's `expires_at` is `created_at + 24 h`.
pub fn queue(conn: &Connection, a: &ActionRow, o: &OutboxRow) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(db)?;
    insert_action(&tx, a)?;
    tx.execute(
        &format!(
            "INSERT INTO sew_chat_outbox ({OUTBOX_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32)"
        ),
        params![
            o.id,
            o.action_id,
            o.project_id,
            o.k2_session,
            o.agent_name,
            o.action,
            o.space_id,
            o.channel_id,
            o.target_id,
            o.addr,
            o.on_behalf_of,
            o.on_behalf_person,
            i64::from(o.broadcast),
            o.client_key,
            o.body,
            o.body_sha256,
            o.body_bytes,
            o.message_kind,
            o.wake.map(i64::from),
            o.response,
            o.status,
            o.decided_by,
            o.decided_kind,
            o.confirm_method,
            o.note,
            o.delivery,
            o.message_id,
            o.sew_code,
            if o.queue_reason.is_empty() { "approval_required" } else { o.queue_reason.as_str() },
            o.created_at,
            o.expires_at,
            o.decided_at
        ],
    )
    .map_err(db)?;
    tx.commit().map_err(db)?;
    Ok(())
}

/// The expiry for an item queued at `now`.
pub fn expires_at(now: i64) -> i64 {
    now + APPROVAL_TTL_SECS
}

pub fn outbox(conn: &Connection, id: &str) -> Result<Option<OutboxRow>, String> {
    conn.query_row(&format!("SELECT {OUTBOX_COLS} FROM sew_chat_outbox WHERE id = ?1"), params![id], outbox_from)
        .optional()
        .map_err(db)
}

/// A workspace's items, newest first.
pub fn outbox_for(conn: &Connection, project_id: &str, limit: i64) -> Result<Vec<OutboxRow>, String> {
    let mut s = conn
        .prepare(&format!(
            "SELECT {OUTBOX_COLS} FROM sew_chat_outbox WHERE project_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2"
        ))
        .map_err(db)?;
    let rows = s.query_map(params![project_id, limit], outbox_from).map_err(db)?;
    rows.collect::<Result<_, _>>().map_err(db)
}

/// Every queued item, oldest first.
pub fn queued(conn: &Connection) -> Result<Vec<OutboxRow>, String> {
    let mut s = conn
        .prepare(&format!("SELECT {OUTBOX_COLS} FROM sew_chat_outbox WHERE status = 'queued' ORDER BY created_at, rowid"))
        .map_err(db)?;
    let rows = s.query_map([], outbox_from).map_err(db)?;
    rows.collect::<Result<_, _>>().map_err(db)
}

pub fn queued_count(conn: &Connection, project_id: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM sew_chat_outbox WHERE project_id = ?1 AND status = 'queued'",
        params![project_id],
        |r| r.get(0),
    )
    .map_err(db)
}

/// A decision on a queued item.
#[derive(Debug, Clone)]
pub struct Decision<'a> {
    pub status: OutboxStatus,
    pub by: &'a str,
    pub kind: DecidedKind,
    pub method: Option<&'a str>,
    pub note: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecideErr {
    NotFound,
    /// Already decided; carries the current status.
    NotQueued(String),
    Db(String),
}

fn decide_in(conn: &Connection, id: &str, d: &Decision, now: i64) -> Result<OutboxRow, DecideErr> {
    assert!(d.status != OutboxStatus::Queued, "a decision moves an item out of queued");
    let n = conn
        .execute(
            "UPDATE sew_chat_outbox SET status = ?2, decided_by = ?3, decided_kind = ?4, confirm_method = ?5, note = ?6,
               decided_at = ?7, body = NULL WHERE id = ?1 AND status = 'queued'",
            params![id, d.status.as_str(), d.by, d.kind.as_str(), d.method, d.note, now],
        )
        .map_err(|e| DecideErr::Db(e.to_string()))?;
    let row = outbox(conn, id).map_err(DecideErr::Db)?.ok_or(DecideErr::NotFound)?;
    if n == 0 {
        return Err(DecideErr::NotQueued(row.status));
    }
    conn.execute(
        "UPDATE sew_chat_actions SET status = ?2, updated_at = ?3 WHERE id = ?1",
        params![row.action_id, d.status.as_str(), now],
    )
    .map_err(|e| DecideErr::Db(e.to_string()))?;
    if d.status != OutboxStatus::Approved {
        // Never posted: no on-behalf review to keep.
        conn.execute("DELETE FROM sew_chat_behalf WHERE action_id = ?1", params![row.action_id])
            .map_err(|e| DecideErr::Db(e.to_string()))?;
    }
    Ok(row)
}

/// Atomically move a queued item to `d.status` and clear its body. The
/// returned row no longer holds the body, so a caller that will post an
/// approved item reads [`OutboxRow::body`] from a copy taken BEFORE this
/// call ([`take_for_approval`] does both under one transaction).
pub fn decide(conn: &Connection, id: &str, d: &Decision, now: i64) -> Result<OutboxRow, DecideErr> {
    let tx = conn.unchecked_transaction().map_err(|e| DecideErr::Db(e.to_string()))?;
    let r = decide_in(&tx, id, d, now)?;
    tx.commit().map_err(|e| DecideErr::Db(e.to_string()))?;
    Ok(r)
}

/// Approve: read the queued body and decide in one transaction. Returns
/// the decided row and the body to post (the stored copy is gone).
pub fn take_for_approval(conn: &Connection, id: &str, d: &Decision, now: i64) -> Result<(OutboxRow, String), DecideErr> {
    assert_eq!(d.status, OutboxStatus::Approved);
    let tx = conn.unchecked_transaction().map_err(|e| DecideErr::Db(e.to_string()))?;
    let before = outbox(&tx, id).map_err(DecideErr::Db)?.ok_or(DecideErr::NotFound)?;
    if before.status != "queued" {
        return Err(DecideErr::NotQueued(before.status));
    }
    let body = before.body.clone().ok_or_else(|| DecideErr::Db(format!("queued item {id} has no body")))?;
    let r = decide_in(&tx, id, d, now)?;
    tx.commit().map_err(|e| DecideErr::Db(e.to_string()))?;
    Ok((r, body))
}

/// Record what happened when an approved item was posted.
pub fn set_delivery(
    conn: &Connection,
    id: &str,
    posted: bool,
    message_id: Option<&str>,
    sew_code: Option<&str>,
    sew_status: Option<i64>,
    now: i64,
) -> Result<OutboxRow, String> {
    let tx = conn.unchecked_transaction().map_err(db)?;
    tx.execute(
        "UPDATE sew_chat_outbox SET delivery = ?2, message_id = ?3, sew_code = ?4 WHERE id = ?1 AND status = 'approved'",
        params![id, if posted { "posted" } else { "failed" }, message_id, sew_code],
    )
    .map_err(db)?;
    let row = outbox(&tx, id)?.ok_or_else(|| format!("no outbox row {id}"))?;
    finish_action(&tx, &row.action_id, if posted { "posted" } else { "failed" }, message_id, sew_code, sew_status, now)?;
    tx.commit().map_err(db)?;
    Ok(row)
}

/// Lazy expiry: every queued item past `expires_at` is denied by
/// `system:expired` (body cleared). Returns the expired rows.
pub fn expire_due(conn: &Connection, now: i64) -> Result<Vec<OutboxRow>, String> {
    let ids: Vec<String> = {
        let mut s = conn
            .prepare("SELECT id FROM sew_chat_outbox WHERE status = 'queued' AND expires_at <= ?1")
            .map_err(db)?;
        let rows = s.query_map(params![now], |r| r.get(0)).map_err(db)?;
        rows.collect::<Result<_, _>>().map_err(db)?
    };
    let d = Decision {
        status: OutboxStatus::Denied,
        by: "system:expired",
        kind: DecidedKind::System,
        method: None,
        note: Some("expired: not decided within 24 h"),
    };
    let mut out = Vec::new();
    for id in ids {
        match decide(conn, &id, &d, now) {
            Ok(r) => out.push(r),
            Err(DecideErr::NotQueued(_)) | Err(DecideErr::NotFound) => {}
            Err(DecideErr::Db(e)) => return Err(e),
        }
    }
    Ok(out)
}

// ── on-behalf reviews (Rosson 2026-10-27, ticket 411eda53) ───────────

/// An assistant's write that speaks for a person, and that person's
/// after-the-fact review. Metadata only: nothing waits on it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Behalf {
    pub action_id: String,
    /// The person's Sew external id (`k2:owner` / `k2:principal:<uuid>`).
    pub person: String,
    /// Their chat username.
    pub person_name: String,
    pub project_id: String,
    pub action: String,
    pub message_id: Option<String>,
    pub outbox_id: Option<String>,
    /// `pending`, `approved` or `declined`.
    pub state: String,
    pub decided_by: Option<String>,
    /// `login` (a Connect login, a person channel) or `owner_token`.
    pub decided_kind: Option<String>,
    pub confirm_method: Option<String>,
    pub note: Option<String>,
    pub created_at: i64,
    pub decided_at: Option<i64>,
}

const BEHALF_COLS: &str = "action_id, person, person_name, project_id, action, message_id, outbox_id, state, decided_by, \
     decided_kind, confirm_method, note, created_at, decided_at";

fn behalf_from(r: &Row) -> rusqlite::Result<Behalf> {
    Ok(Behalf {
        action_id: r.get(0)?,
        person: r.get(1)?,
        person_name: r.get(2)?,
        project_id: r.get(3)?,
        action: r.get(4)?,
        message_id: r.get(5)?,
        outbox_id: r.get(6)?,
        state: r.get(7)?,
        decided_by: r.get(8)?,
        decided_kind: r.get(9)?,
        confirm_method: r.get(10)?,
        note: r.get(11)?,
        created_at: r.get(12)?,
        decided_at: r.get(13)?,
    })
}

/// Record an on-behalf write (state `pending`), next to its action row.
pub fn insert_behalf(conn: &Connection, b: &Behalf) -> Result<(), String> {
    conn.execute(
        "INSERT INTO sew_chat_behalf (action_id, person, person_name, project_id, action, message_id, outbox_id, state, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8)",
        params![b.action_id, b.person, b.person_name, b.project_id, b.action, b.message_id, b.outbox_id, b.created_at],
    )
    .map_err(db)?;
    Ok(())
}

/// One review by its write's action id, or the newest one for a Sew
/// message id (an edit is a newer write of the same message).
pub fn get_behalf(conn: &Connection, id: &str) -> Result<Option<Behalf>, String> {
    conn.query_row(
        &format!(
            "SELECT {BEHALF_COLS} FROM sew_chat_behalf WHERE action_id = ?1 OR message_id = ?1
             ORDER BY created_at DESC, rowid DESC LIMIT 1"
        ),
        params![id],
        behalf_from,
    )
    .optional()
    .map_err(db)
}

/// A person's on-behalf writes, newest first (`state` filters).
pub fn list_behalf(conn: &Connection, person: &str, state: Option<&str>, limit: i64) -> Result<Vec<Behalf>, String> {
    let mut st = conn
        .prepare(&format!(
            "SELECT {BEHALF_COLS} FROM sew_chat_behalf WHERE person = ?1 AND (?2 IS NULL OR state = ?2)
             ORDER BY created_at DESC, rowid DESC LIMIT ?3"
        ))
        .map_err(db)?;
    let rows = st.query_map(params![person, state, limit.clamp(1, 500)], behalf_from).map_err(db)?;
    rows.collect::<Result<_, _>>().map_err(db)
}

/// The newest review per Sew message id, for the messages a list shows.
pub fn behalf_for_messages(conn: &Connection, ids: &[String]) -> Result<std::collections::HashMap<String, Behalf>, String> {
    let mut out = std::collections::HashMap::new();
    let sql = format!("SELECT {BEHALF_COLS} FROM sew_chat_behalf WHERE message_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1");
    for id in ids {
        if let Some(b) = conn.query_row(&sql, params![id], behalf_from).optional().map_err(db)? {
            out.insert(id.clone(), b);
        }
    }
    Ok(out)
}

/// The person's decision (`approved` / `declined`). A later decision
/// replaces an earlier one (they may change their mind); the row keeps the
/// newest and the audit keeps each. False when there is no such row.
#[allow(clippy::too_many_arguments)]
pub fn review_behalf(
    conn: &Connection,
    action_id: &str,
    state: &str,
    decided_by: &str,
    decided_kind: &str,
    confirm_method: Option<&str>,
    note: Option<&str>,
    now: i64,
) -> Result<bool, String> {
    if !matches!(state, "approved" | "declined") {
        return Err(format!("unknown review decision {state:?}"));
    }
    let n = conn
        .execute(
            "UPDATE sew_chat_behalf SET state = ?2, decided_by = ?3, decided_kind = ?4, confirm_method = ?5, note = ?6,
               decided_at = ?7 WHERE action_id = ?1",
            params![action_id, state, decided_by, decided_kind, confirm_method, note, now],
        )
        .map_err(db)?;
    Ok(n > 0)
}

// ── audit (K19/K20 administration) ───────────────────────────────────

/// One administration record (no message content).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuditRow {
    pub id: String,
    pub project_id: Option<String>,
    pub actor: String,
    /// `login` | `owner_token` | `agent` | `system`.
    pub actor_kind: String,
    pub op: String,
    pub space_id: Option<String>,
    pub target: Option<String>,
    pub detail: Option<String>,
    /// `ok` | `refused` | `failed`.
    pub outcome: String,
    pub reason: Option<String>,
    pub created_at: i64,
}

const AUDIT_COLS: &str = "id, project_id, actor, actor_kind, op, space_id, target, detail, outcome, reason, created_at";

pub fn insert_audit(conn: &Connection, a: &AuditRow) -> Result<(), String> {
    conn.execute(
        &format!("INSERT INTO sew_chat_audit ({AUDIT_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"),
        params![
            a.id,
            a.project_id,
            a.actor,
            a.actor_kind,
            a.op,
            a.space_id,
            a.target,
            a.detail,
            a.outcome,
            a.reason,
            a.created_at
        ],
    )
    .map_err(db)?;
    Ok(())
}

/// The newest administration records, newest first.
pub fn audit_recent(conn: &Connection, limit: i64) -> Result<Vec<AuditRow>, String> {
    let mut s = conn
        .prepare(&format!("SELECT {AUDIT_COLS} FROM sew_chat_audit ORDER BY created_at DESC, rowid DESC LIMIT ?1"))
        .map_err(db)?;
    let rows = s
        .query_map(params![limit], |r| {
            Ok(AuditRow {
                id: r.get(0)?,
                project_id: r.get(1)?,
                actor: r.get(2)?,
                actor_kind: r.get(3)?,
                op: r.get(4)?,
                space_id: r.get(5)?,
                target: r.get(6)?,
                detail: r.get(7)?,
                outcome: r.get(8)?,
                reason: r.get(9)?,
                created_at: r.get(10)?,
            })
        })
        .map_err(db)?;
    rows.collect::<Result<_, _>>().map_err(db)
}

/// Every access row on the box (the admin list), by workspace then Space.
pub fn list_all_access(conn: &Connection) -> Result<Vec<Access>, String> {
    let mut s = conn
        .prepare(&format!("SELECT {ACCESS_COLS} FROM sew_chat_access ORDER BY project_id, space_slug"))
        .map_err(db)?;
    let rows = s.query_map([], access_from).map_err(db)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(parse_access(r.map_err(db)?)?);
    }
    Ok(out)
}

/// The [`Action`] of a stored row (fails closed on an unknown word).
pub fn action_of(row: &OutboxRow) -> Result<Action, String> {
    Action::parse(&row.action).ok_or_else(|| format!("stored action '{}' is unknown", row.action))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sew_chat::{body_sha256, new_id};

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&c).unwrap();
        c
    }

    fn access(level: Level, approval: ApprovalMode) -> Access {
        Access {
            project_id: "w1".into(),
            space_id: "s1".into(),
            space_slug: "hearth".into(),
            level,
            approval,
            updated_by: "owner".into(),
            updated_at: 1,
        }
    }

    fn queued_item(c: &Connection, now: i64) -> OutboxRow {
        let aid = new_id("act_");
        let a = ActionRow {
            id: aid.clone(),
            project_id: "w1".into(),
            k2_session: "sess".into(),
            action: "send".into(),
            addr: "hearth/general".into(),
            status: "queued".into(),
            client_key: new_id("k_"),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        let o = OutboxRow {
            id: new_id("out_"),
            action_id: aid,
            project_id: "w1".into(),
            k2_session: "sess".into(),
            agent_name: "w1".into(),
            action: "send".into(),
            space_id: Some("s1".into()),
            channel_id: "c1".into(),
            addr: "hearth/general".into(),
            client_key: a.client_key.clone(),
            body: Some("secret plans".into()),
            body_sha256: body_sha256("secret plans"),
            body_bytes: 12,
            status: "queued".into(),
            created_at: now,
            expires_at: expires_at(now),
            ..Default::default()
        };
        queue(c, &a, &o).unwrap();
        o
    }

    #[test]
    fn access_round_trips_and_unreadable_rows_fail_closed() {
        let c = conn();
        assert_eq!(get_access(&c, "w1", "s1").unwrap(), None);
        set_access(&c, &access(Level::Reply, ApprovalMode::Required)).unwrap();
        assert_eq!(get_access(&c, "w1", "s1").unwrap().unwrap().level, Level::Reply);
        set_access(&c, &access(Level::Manage, ApprovalMode::None)).unwrap();
        let a = get_access(&c, "w1", "s1").unwrap().unwrap();
        assert_eq!((a.level, a.approval), (Level::Manage, ApprovalMode::None));
        assert_eq!(list_access(&c, "w1").unwrap().len(), 1);
        // A bad stored word (only reachable by hand) is an error, not a default.
        c.execute_batch("PRAGMA ignore_check_constraints = ON;").unwrap();
        c.execute("UPDATE sew_chat_access SET level = 'god'", []).unwrap();
        assert!(get_access(&c, "w1", "s1").is_err());
    }

    #[test]
    fn policy_defaults_to_people() {
        let c = conn();
        assert_eq!(approvals_by(&c).unwrap(), ApprovalsBy::People);
        set_approvals_by(&c, ApprovalsBy::PeopleAndAgents, "owner", 5).unwrap();
        assert_eq!(approvals_by(&c).unwrap(), ApprovalsBy::PeopleAndAgents);
    }

    #[test]
    fn decide_is_atomic_and_clears_the_body() {
        let c = conn();
        let o = queued_item(&c, 100);
        let d = Decision { status: OutboxStatus::Approved, by: "owner", kind: DecidedKind::Person, method: Some("vault-password"), note: None };
        let (row, body) = take_for_approval(&c, &o.id, &d, 101).unwrap();
        assert_eq!(body, "secret plans");
        assert_eq!(row.status, "approved");
        assert_eq!(row.body, None, "no chat text kept after a decision");
        assert_eq!(outbox(&c, &o.id).unwrap().unwrap().body, None);
        assert_eq!(take_for_approval(&c, &o.id, &d, 102).unwrap_err(), DecideErr::NotQueued("approved".into()));
        let r = set_delivery(&c, &o.id, true, Some("m9"), None, None, 103).unwrap();
        assert_eq!((r.delivery.as_deref(), r.message_id.as_deref()), (Some("posted"), Some("m9")));
        assert_eq!(action(&c, &o.action_id).unwrap().unwrap().status, "posted");
    }

    #[test]
    fn expiry_denies_and_revoke_cancels() {
        let c = conn();
        let old = queued_item(&c, 0);
        let fresh = queued_item(&c, APPROVAL_TTL_SECS);
        let gone = expire_due(&c, APPROVAL_TTL_SECS).unwrap();
        assert_eq!(gone.iter().map(|r| r.id.clone()).collect::<Vec<_>>(), vec![old.id.clone()]);
        let r = outbox(&c, &old.id).unwrap().unwrap();
        assert_eq!((r.status.as_str(), r.decided_by.as_deref(), r.body.as_deref()), ("denied", Some("system:expired"), None));
        set_access(&c, &access(Level::Send, ApprovalMode::Required)).unwrap();
        assert_eq!(revoke_access(&c, "w1", "s1", "owner", 7).unwrap(), 1);
        let r = outbox(&c, &fresh.id).unwrap().unwrap();
        assert_eq!((r.status.as_str(), r.body.as_deref()), ("cancelled", None));
        assert_eq!(get_access(&c, "w1", "s1").unwrap(), None);
    }

    #[test]
    fn live_keys_replay_and_counts_include_every_status() {
        let c = conn();
        let o = queued_item(&c, 50);
        let live = live_action_by_key(&c, "w1", &o.client_key).unwrap().unwrap();
        assert_eq!(live.status, "queued");
        assert_eq!(count_since(&c, "w1", true, 0).unwrap(), (1, Some(50)));
        assert_eq!(count_since(&c, "w1", false, 0).unwrap().0, 0);
        assert_eq!(queued_count(&c, "w1").unwrap(), 1);
    }
}
