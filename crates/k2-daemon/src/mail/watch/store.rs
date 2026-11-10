//! The one record of who hears about which mail (`agent_mail_*`). Both
//! Settings surfaces (the inboxes area and the agent's Mail tab) and the CLI
//! read and write through these functions, so they can't disagree.

use rusqlite::{params, Connection, OptionalExtension};

use crate::mail::access::Source;

/// Per-inbox wake choice: `vip` = only VIP senders; `all` = VIPs at once and
/// everyone else in the digest. No row at all = off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    Vip,
    All,
}

impl Wake {
    pub fn parse(s: &str) -> Option<Wake> {
        match s {
            "vip" => Some(Wake::Vip),
            "all" => Some(Wake::All),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Wake::Vip => "vip",
            Wake::All => "all",
        }
    }
}

fn source_str(s: Source) -> &'static str {
    s.as_str()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub project_id: String,
    pub enabled: bool,
    pub vip_outside_window: bool,
    pub snippets: bool,
    pub vip_session_line: bool,
    pub paused_until: Option<String>,
}

impl Settings {
    /// What an agent has before anyone chooses anything: off.
    pub fn off(project_id: &str) -> Settings {
        Settings {
            project_id: project_id.to_string(),
            enabled: false,
            vip_outside_window: true,
            snippets: true,
            vip_session_line: true,
            paused_until: None,
        }
    }

    pub fn paused_now(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.paused_until
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .is_some_and(|until| now < until)
    }
}

pub fn get_settings(conn: &Connection, project_id: &str) -> rusqlite::Result<Settings> {
    let row = conn
        .query_row(
            "SELECT enabled, vip_outside_window, snippets, vip_session_line, paused_until \
             FROM agent_mail_watch WHERE project_id = ?1",
            params![project_id],
            |r| {
                Ok(Settings {
                    project_id: project_id.to_string(),
                    enabled: r.get::<_, i64>(0)? == 1,
                    vip_outside_window: r.get::<_, i64>(1)? == 1,
                    snippets: r.get::<_, i64>(2)? == 1,
                    vip_session_line: r.get::<_, i64>(3)? == 1,
                    paused_until: r.get(4)?,
                })
            },
        )
        .optional()?;
    Ok(row.unwrap_or_else(|| Settings::off(project_id)))
}

/// Create the row if there isn't one yet (off, until someone says on).
pub fn ensure_row(conn: &Connection, project_id: &str) -> rusqlite::Result<()> {
    conn.execute("INSERT OR IGNORE INTO agent_mail_watch (project_id) VALUES (?1)", params![project_id])?;
    Ok(())
}

/// One setting, by its column. Only the fixed list below may be named.
pub fn set_flag(conn: &Connection, project_id: &str, column: &str, on: bool, by: &str) -> rusqlite::Result<()> {
    assert!(
        matches!(column, "enabled" | "vip_outside_window" | "snippets" | "vip_session_line"),
        "set_flag column is a fixed list"
    );
    ensure_row(conn, project_id)?;
    conn.execute(
        &format!("UPDATE agent_mail_watch SET {column} = ?1, updated_at = ?2, updated_by = ?3 WHERE project_id = ?4"),
        params![on as i64, chrono::Utc::now().to_rfc3339(), by, project_id],
    )?;
    Ok(())
}

pub fn set_paused(conn: &Connection, project_id: &str, until: Option<&str>, by: &str) -> rusqlite::Result<()> {
    ensure_row(conn, project_id)?;
    conn.execute(
        "UPDATE agent_mail_watch SET paused_until = ?1, updated_at = ?2, updated_by = ?3 WHERE project_id = ?4",
        params![until, chrono::Utc::now().to_rfc3339(), by, project_id],
    )?;
    Ok(())
}

/// Stop or start every agent's notifications (the owner's kill switch).
pub fn set_paused_all(conn: &Connection, until: Option<&str>, by: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE agent_mail_watch SET paused_until = ?1, updated_at = ?2, updated_by = ?3",
        params![until, chrono::Utc::now().to_rfc3339(), by],
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchedInbox {
    pub source: Source,
    pub inbox_id: String,
    pub wake: Wake,
}

fn parse_source(s: &str) -> Source {
    if s == "linked" { Source::Linked } else { Source::Hosted }
}

pub fn inboxes_of(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<WatchedInbox>> {
    let mut stmt = conn.prepare(
        "SELECT source, inbox_id, wake FROM agent_mail_watch_inboxes WHERE project_id = ?1 ORDER BY source, inbox_id",
    )?;
    let rows = stmt.query_map(params![project_id], |r| {
        Ok(WatchedInbox {
            source: parse_source(&r.get::<_, String>(0)?),
            inbox_id: r.get(1)?,
            wake: Wake::parse(&r.get::<_, String>(2)?).unwrap_or(Wake::Vip),
        })
    })?;
    rows.collect()
}

pub fn set_inbox(conn: &Connection, project_id: &str, source: Source, inbox_id: &str, wake: Wake, by: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO agent_mail_watch_inboxes (project_id, source, inbox_id, wake, added_by, added_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT (project_id, source, inbox_id) DO UPDATE SET wake = excluded.wake",
        params![project_id, source_str(source), inbox_id, wake.as_str(), by, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

pub fn remove_inbox(conn: &Connection, project_id: &str, source: Source, inbox_id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM agent_mail_watch_inboxes WHERE project_id = ?1 AND source = ?2 AND inbox_id = ?3",
        params![project_id, source_str(source), inbox_id],
    )
}

/// Agents that hear about this inbox, and how. Only enabled agents count.
pub fn watchers_of(conn: &Connection, source: Source, inbox_id: &str) -> rusqlite::Result<Vec<(String, Wake)>> {
    let mut stmt = conn.prepare(
        "SELECT i.project_id, i.wake FROM agent_mail_watch_inboxes i \
         JOIN agent_mail_watch w ON w.project_id = i.project_id \
         WHERE i.source = ?1 AND i.inbox_id = ?2 AND w.enabled = 1",
    )?;
    let rows = stmt.query_map(params![source_str(source), inbox_id], |r| {
        Ok((r.get::<_, String>(0)?, Wake::parse(&r.get::<_, String>(1)?).unwrap_or(Wake::Vip)))
    })?;
    rows.collect()
}

/// The distinct inboxes at least one enabled agent hears about.
pub fn all_watched(conn: &Connection) -> rusqlite::Result<Vec<(Source, String)>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT i.source, i.inbox_id FROM agent_mail_watch_inboxes i \
         JOIN agent_mail_watch w ON w.project_id = i.project_id WHERE w.enabled = 1",
    )?;
    let rows = stmt.query_map([], |r| Ok((parse_source(&r.get::<_, String>(0)?), r.get::<_, String>(1)?)))?;
    rows.collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VipKind {
    Address,
    Domain,
    Person,
}

impl VipKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            VipKind::Address => "address",
            VipKind::Domain => "domain",
            VipKind::Person => "person",
        }
    }
    fn parse(s: &str) -> VipKind {
        match s {
            "domain" => VipKind::Domain,
            "person" => VipKind::Person,
            _ => VipKind::Address,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vip {
    pub kind: VipKind,
    pub value: String,
}

pub fn vips_of(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<Vip>> {
    let mut stmt = conn.prepare("SELECT kind, value FROM agent_mail_vips WHERE project_id = ?1 ORDER BY kind, value")?;
    let rows = stmt.query_map(params![project_id], |r| {
        Ok(Vip { kind: VipKind::parse(&r.get::<_, String>(0)?), value: r.get(1)? })
    })?;
    rows.collect()
}

pub fn add_vip(conn: &Connection, project_id: &str, vip: &Vip, by: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT OR IGNORE INTO agent_mail_vips (project_id, kind, value, added_by, added_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![project_id, vip.kind.as_str(), vip.value, by, chrono::Utc::now().to_rfc3339()],
    )
}

pub fn remove_vip(conn: &Connection, project_id: &str, vip: &Vip) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM agent_mail_vips WHERE project_id = ?1 AND kind = ?2 AND value = ?3",
        params![project_id, vip.kind.as_str(), vip.value],
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cursor {
    pub uidvalidity: Option<u32>,
    pub last_uid: Option<u32>,
    pub jmap_state: Option<String>,
    pub last_received_at: Option<String>,
    pub needs_relink: bool,
    pub relink_cred: Option<String>,
    pub next_poll_at: i64,
    pub consecutive_errs: i64,
}

pub fn get_cursor(conn: &Connection, source: Source, inbox_id: &str) -> rusqlite::Result<Cursor> {
    Ok(conn
        .query_row(
            "SELECT uidvalidity, last_uid, jmap_state, last_received_at, needs_relink, relink_cred, next_poll_at, consecutive_errs \
             FROM mail_watch_cursors WHERE source = ?1 AND inbox_id = ?2",
            params![source_str(source), inbox_id],
            |r| {
                Ok(Cursor {
                    uidvalidity: r.get::<_, Option<i64>>(0)?.map(|v| v as u32),
                    last_uid: r.get::<_, Option<i64>>(1)?.map(|v| v as u32),
                    jmap_state: r.get(2)?,
                    last_received_at: r.get(3)?,
                    needs_relink: r.get::<_, i64>(4)? == 1,
                    relink_cred: r.get(5)?,
                    next_poll_at: r.get(6)?,
                    consecutive_errs: r.get(7)?,
                })
            },
        )
        .optional()?
        .unwrap_or_default())
}

pub fn put_cursor(conn: &Connection, source: Source, inbox_id: &str, c: &Cursor, last_error: Option<&str>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO mail_watch_cursors (source, inbox_id, uidvalidity, last_uid, jmap_state, last_received_at, last_poll_at, last_error, needs_relink, relink_cred, next_poll_at, consecutive_errs) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
         ON CONFLICT (source, inbox_id) DO UPDATE SET uidvalidity = excluded.uidvalidity, last_uid = excluded.last_uid, \
           jmap_state = excluded.jmap_state, last_received_at = excluded.last_received_at, last_poll_at = excluded.last_poll_at, \
           last_error = excluded.last_error, needs_relink = excluded.needs_relink, relink_cred = excluded.relink_cred, \
           next_poll_at = excluded.next_poll_at, consecutive_errs = excluded.consecutive_errs",
        params![
            source_str(source),
            inbox_id,
            c.uidvalidity.map(|v| v as i64),
            c.last_uid.map(|v| v as i64),
            c.jmap_state,
            c.last_received_at,
            chrono::Utc::now().to_rfc3339(),
            last_error,
            c.needs_relink as i64,
            c.relink_cred,
            c.next_poll_at,
            c.consecutive_errs,
        ],
    )?;
    Ok(())
}

/// One new message for one agent.
#[derive(Debug, Clone)]
pub struct NewNotice<'a> {
    pub project_id: &'a str,
    pub source: Source,
    pub inbox_id: &'a str,
    pub inbox_address: &'a str,
    pub message_token: &'a str,
    pub rfc_message_id: &'a str,
    pub from_addr: &'a str,
    pub received_at: &'a str,
    pub class: &'a str,
    pub reason: &'a str,
    pub state: &'a str,
}

/// `Some(id)` when this is news; `None` when this agent has already been told
/// about this message (the same mail in two watched inboxes, or a re-poll).
pub fn insert_notice(conn: &Connection, n: &NewNotice<'_>, now: i64) -> rusqlite::Result<Option<i64>> {
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO agent_mail_notices \
         (project_id, source, inbox_id, inbox_address, message_token, rfc_message_id, from_addr, received_at, class, reason, state, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            n.project_id, source_str(n.source), n.inbox_id, n.inbox_address, n.message_token, n.rfc_message_id,
            n.from_addr, n.received_at, n.class, n.reason, n.state, now
        ],
    )?;
    if inserted == 0 {
        return Ok(None);
    }
    Ok(Some(conn.last_insert_rowid()))
}

pub fn mark_state(conn: &Connection, id: i64, state: &str, woke: bool, delivery_ref: Option<&str>, now: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE agent_mail_notices SET state = ?1, woke = ?2, delivery_ref = COALESCE(?3, delivery_ref), delivered_at = ?4 WHERE id = ?5",
        params![state, woke as i64, delivery_ref, now, id],
    )
}

pub fn demote_to_digest(conn: &Connection, id: i64, reason: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE agent_mail_notices SET class = 'digest', reason = ?1, state = 'pending' WHERE id = ?2",
        params![reason, id],
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRow {
    pub id: i64,
    pub source: Source,
    pub inbox_id: String,
    pub inbox_address: String,
    pub message_token: String,
    pub rfc_message_id: String,
    pub from_addr: String,
    pub received_at: String,
    pub class: String,
    pub reason: String,
}

fn pending_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PendingRow> {
    Ok(PendingRow {
        id: r.get(0)?,
        source: parse_source(&r.get::<_, String>(1)?),
        inbox_id: r.get(2)?,
        inbox_address: r.get(3)?,
        message_token: r.get(4)?,
        rfc_message_id: r.get(5)?,
        from_addr: r.get(6)?,
        received_at: r.get(7)?,
        class: r.get(8)?,
        reason: r.get(9)?,
    })
}

const PENDING_COLS: &str = "id, source, inbox_id, inbox_address, message_token, rfc_message_id, from_addr, received_at, class, reason";

/// What the next digest will carry: held VIPs first, then by arrival.
pub fn pending_for_digest(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<PendingRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PENDING_COLS} FROM agent_mail_notices WHERE project_id = ?1 AND state = 'pending' \
           AND (class = 'digest' OR reason = 'held_outside_window') \
         ORDER BY (reason = 'held_outside_window') DESC, received_at ASC"
    ))?;
    let rows = stmt.query_map(params![project_id], pending_row)?;
    rows.collect()
}

/// VIP rows waiting for room under the rate limits.
pub fn pending_vips(conn: &Connection, project_id: &str) -> rusqlite::Result<Vec<PendingRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PENDING_COLS} FROM agent_mail_notices WHERE project_id = ?1 AND state = 'pending' AND class = 'vip' \
           AND reason <> 'held_outside_window' ORDER BY received_at ASC"
    ))?;
    let rows = stmt.query_map(params![project_id], pending_row)?;
    rows.collect()
}

pub fn get_notice(conn: &Connection, id: i64) -> rusqlite::Result<Option<(String, PendingRow, String, Option<String>)>> {
    conn.query_row(
        &format!("SELECT {PENDING_COLS}, project_id, state, delivery_ref FROM agent_mail_notices WHERE id = ?1"),
        params![id],
        |r| Ok((r.get::<_, String>(10)?, pending_row(r)?, r.get::<_, String>(11)?, r.get::<_, Option<String>>(12)?)),
    )
    .optional()
}

/// VIP lines delivered to this agent since `since` (unix seconds).
pub fn vip_delivered_since(conn: &Connection, project_id: &str, since: i64) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM agent_mail_notices WHERE project_id = ?1 AND class = 'vip' AND state = 'delivered' AND delivered_at >= ?2",
        params![project_id, since],
        |r| r.get(0),
    )
}

/// The most recent VIP from this sender delivered to this agent.
pub fn last_vip_from(conn: &Connection, project_id: &str, from_addr: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT MAX(delivered_at) FROM agent_mail_notices WHERE project_id = ?1 AND from_addr = ?2 AND class = 'vip' AND state = 'delivered'",
        params![project_id, from_addr],
        |r| r.get(0),
    )
}

/// Drop what's left pending when an agent lost access to the inbox.
pub fn drop_pending_for_inbox(conn: &Connection, project_id: &str, source: Source, inbox_id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE agent_mail_notices SET state = 'dropped' WHERE project_id = ?1 AND source = ?2 AND inbox_id = ?3 AND state = 'pending'",
        params![project_id, source_str(source), inbox_id],
    )
}

/// Notices older than 30 days go (they hold addresses and Message-IDs).
pub fn prune_notices(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM agent_mail_notices WHERE created_at < ?1", params![now - 30 * 86_400])
}

/// The log for one agent, newest first.
pub fn log_of(conn: &Connection, project_id: &str, limit: i64) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT from_addr, inbox_address, received_at, class, reason, state, created_at, delivered_at, woke FROM agent_mail_notices \
         WHERE project_id = ?1 ORDER BY id DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![project_id, limit], |r| {
        Ok(serde_json::json!({
            "from": r.get::<_, String>(0)?,
            "inbox": r.get::<_, String>(1)?,
            "receivedAt": r.get::<_, String>(2)?,
            "class": r.get::<_, String>(3)?,
            "reason": r.get::<_, String>(4)?,
            "state": r.get::<_, String>(5)?,
            "createdAt": r.get::<_, i64>(6)?,
            "deliveredAt": r.get::<_, Option<i64>>(7)?,
            "woke": r.get::<_, i64>(8)? == 1,
        }))
    })?;
    rows.collect()
}

/// Counts for the settings' "recent" line (since `since`).
pub fn counts_since(conn: &Connection, project_id: &str, since: i64) -> rusqlite::Result<serde_json::Value> {
    let mut stmt = conn.prepare(
        "SELECT state, class, reason, COUNT(*) FROM agent_mail_notices WHERE project_id = ?1 AND created_at >= ?2 GROUP BY state, class, reason",
    )?;
    let mut vip_delivered = 0i64;
    let mut digested = 0i64;
    let mut failed_auth = 0i64;
    let rows = stmt.query_map(params![project_id, since], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?))
    })?;
    for row in rows {
        let (state, class, reason, n) = row?;
        if class == "vip" && state == "delivered" {
            vip_delivered += n;
        }
        if state == "digested" {
            digested += n;
        }
        if reason == "claimed_vip_failed_auth" {
            failed_auth += n;
        }
    }
    Ok(serde_json::json!({ "vipDelivered": vip_delivered, "digested": digested, "failedAuth": failed_auth }))
}
