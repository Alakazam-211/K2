//! The wake budget: how many times something other than a person may wake
//! one agent in an hour (prd-routines-reminders-and-agent-mail-v1 §4.8,
//! vs-live W38). One count across every automatic source, keyed by agent, so
//! a noisy feed can't spend an agent's whole hour and then some.
//!
//! Sources, reminders, mail and the Org Chart's wakes count today. Anything else that wakes an agent
//! joins by adding its own count to [`wakes_since`] and delivering through
//! the daemon's `agent_wake` path; nothing here knows about any one of them
//! beyond the query.

use rusqlite::{params, Connection};

/// The limit, per agent, per rolling hour. The same number Sources has
/// always used.
pub const AGENT_WAKES_PER_HOUR: u16 = crate::sources::AGENT_WAKES_PER_HOUR;

/// Audit word for a wake refused by the budget.
pub const DECISION_AGENT_CAP: &str = "agent_cap";

/// Automatic wakes of this agent since `since` (unix seconds), across every
/// source that counts. A query error is returned, so a caller can't mistake
/// "couldn't count" for "room left".
pub fn wakes_since(conn: &Connection, project_id: &str, since: i64) -> rusqlite::Result<u16> {
    let sources = i64::from(crate::sources::trigger_store::agent_fires_since(conn, project_id, since)?);
    let since_rfc = chrono::DateTime::<chrono::Utc>::from_timestamp(since, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
    let reminders: i64 = conn.query_row(
        "SELECT COUNT(*) FROM heartbeat_fires \
         WHERE project_id = ?1 AND mode = 'once' AND decision IN ('fired', 'fired_catchup') AND fired_at >= ?2",
        params![project_id, since_rfc],
        |r| r.get(0),
    )?;
    // VIP lines that reached a session, and digests that went out.
    let mail_lines: i64 = conn.query_row(
        "SELECT COUNT(*) FROM agent_mail_notices WHERE project_id = ?1 AND state = 'delivered' AND woke = 1 AND delivered_at >= ?2",
        params![project_id, since],
        |r| r.get(0),
    )?;
    let digests: i64 = conn.query_row(
        "SELECT COUNT(*) FROM heartbeat_fires f \
         JOIN workspace_heartbeats h ON h.project_id = f.project_id AND h.name = f.schedule_name \
         WHERE f.project_id = ?1 AND h.kind = 'mail_digest' \
           AND f.decision IN ('fired', 'fired_catchup') AND f.fired_at >= ?2",
        params![project_id, since_rfc],
        |r| r.get(0),
    )?;
    // Org Chart wakes (prd-org-chart-v1 §5, vs-live A7): night runs, last
    // calls, members' updates at night, heads' asks, reach outcomes.
    let org = crate::org_chart::store::wakes_since(conn, project_id, since)?;
    Ok(sources
        .saturating_add(reminders)
        .saturating_add(mail_lines)
        .saturating_add(digests)
        .saturating_add(org)
        .clamp(0, i64::from(u16::MAX)) as u16)
}

/// True when this agent may take one more automatic wake right now.
pub fn has_room(conn: &Connection, project_id: &str, now: i64) -> rusqlite::Result<bool> {
    Ok(wakes_since(conn, project_id, now - 3600)? < AGENT_WAKES_PER_HOUR)
}
