//! A heartbeat that fails on the LLM usage limit is paused, not turned
//! off; one turned off after repeated failures says so on the agent's
//! Thread (0.46.18, field report: two daily heartbeats were auto-disabled
//! after 5 fires that failed on a weekly limit, and nothing told the agent
//! or its owner for 12 days).
//!
//! This module is the pure part plus the row writes: which failure reasons
//! are the usage limit, how long a pause lasts, the re-check rule, the
//! notices' words and buttons, and the `workspace_heartbeats` updates. The
//! daemon (`heartbeat_turned_off`) adds the out-of-usage signal
//! (`llm_accounts::out_of_usage`), posts the notices and emits the event.
//!
//! **The pause lives in the existing columns** (no migration): the row
//! stays `enabled = 1`, `disabled_reason = 'usage_limit'` marks it paused
//! by the system, and `next_retry_at` holds the time it may fire again
//! (the scheduler's backoff gate already honours it). A known reset pauses
//! until just after it; an unknown one re-checks every hour.
//!
//! - A successful fire, a failure that isn't the limit, and a person's
//!   toggle clear the mark.
//! - The re-check ([`recheck`]) either moves the time on (still out) or
//!   resumes: `next_retry_at` is cleared and the row fires on its schedule
//!   (a missed slot within 12 h catches up once). The mark stays until
//!   that fire's outcome, so a fire that hits the limit again re-pauses
//!   without a second notice.
//! - Listings show `pausedUntil` ([`paused_until`]) while the time is
//!   ahead.

use chrono::{DateTime, FixedOffset, SecondsFormat, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension, Result};
use serde_json::json;

use crate::llm_accounts::out_of_usage::{self as ou, OutKind};
use crate::overlay::{NoticeAction, SystemNotice};

/// `disabled_reason` on an ENABLED row: paused by the usage limit.
pub const PAUSE_REASON: &str = "usage_limit";
/// With no known reset, look again this often.
pub const RECHECK_SECS: i64 = 60 * 60;
/// A known reset pauses until this long after it (the vendor's clock).
pub const RESET_SLACK_SECS: i64 = 60;
/// `waitDetail` while paused (`waitReason` stays `backoff`).
pub const WAIT_DETAIL: &str = "paused by the LLM usage limit; it resumes by itself";

/// [`SystemNotice::code`]s.
pub const CODE_DISABLED: &str = "heartbeat.auto_disabled";
pub const CODE_PAUSED: &str = "heartbeat.paused_usage_limit";
/// [`NoticeAction::id`]: turn the heartbeat back on
/// (`POST /cli/heartbeat/enable {project, name, enabled: true}`).
pub const ACTION_ENABLE: &str = "heartbeat_enable";
/// The route [`ACTION_ENABLE`] posts to.
pub const ENABLE_ROUTE: &str = "/cli/heartbeat/enable";

/// The longest failure reason a notice quotes.
const SHORT_REASON_MAX: usize = 90;

/// What ran out, and when it is back (unix seconds), when known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cause {
    pub kind: OutKind,
    pub resets_at: Option<i64>,
}

/// Codes and words the daemon itself writes into a failed fire's reason when the
/// login is out: the `StopFailure` errors, the heartbeat restart's stale
/// codes (`hb_stale`: `weekly_limit`, `usage_limit`) and their words.
const USAGE_CODES: [&str; 7] =
    ["rate_limit", "weekly_limit", "usage_limit", "weekly limit", "weekly-limit", "usage-limit", "5-hour limit"];
const CREDIT_CODES: [&str; 2] = ["billing_error", "credit balance is too low"];

/// Is this failed fire's reason the usage limit (or an API key's credit)?
/// The reset, when the reason carries one (`usage limit reached|<ts>`).
pub fn cause_from_reason(reason: &str) -> Option<Cause> {
    let t = reason.to_ascii_lowercase();
    let kind = if CREDIT_CODES.iter().any(|w| t.contains(w)) {
        OutKind::Credit
    } else if USAGE_CODES.iter().any(|w| t.contains(w)) {
        OutKind::Usage
    } else {
        ou::kind_from_words(reason)?
    };
    Some(Cause { kind, resets_at: ou::reset_from_text(reason) })
}

/// How long a pause lasts: just past a known future reset, else one
/// re-check interval. `(until, reset shown to people)`.
pub fn pause_until(reset: Option<i64>, now: i64) -> (i64, Option<i64>) {
    match reset.filter(|r| *r > now) {
        Some(r) => (r + RESET_SLACK_SECS, Some(r)),
        None => (now + RECHECK_SECS, None),
    }
}

/// The re-check of a pause whose time came. `still_out`: `None` = nothing
/// says the login is out now; `Some(reset)` = it is (reset when known).
/// `Some(until)`: stay paused until then. `None`: resume.
pub fn recheck(still_out: Option<Option<i64>>, now: i64) -> Option<i64> {
    let reset = still_out?;
    Some(pause_until(reset, now).0)
}

/// UTC RFC 3339 with whole seconds and a `Z` (the stored form).
pub fn fmt(t: i64) -> String {
    Utc.timestamp_opt(t, 0)
        .single()
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn parse(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp())
}

/// The row is paused by the limit, until `next_retry_at` (still ahead).
/// What `pausedUntil` shows.
pub fn paused_until(enabled: bool, disabled_reason: Option<&str>, next_retry_at: Option<&str>, now: i64) -> Option<String> {
    if !enabled || disabled_reason != Some(PAUSE_REASON) {
        return None;
    }
    let raw = next_retry_at?;
    (parse(raw)? > now).then(|| raw.to_string())
}

// ── The row writes ──────────────────────────────────────────────────

/// Pause an enabled row until `until`. `Some(true)` when it wasn't paused
/// already (a new pause: one notice), `Some(false)` for a re-pause. A row
/// that is off or gone: `None`, nothing written.
pub fn pause(conn: &Connection, project_id: &str, name: &str, until: i64) -> Result<Option<bool>> {
    let before: Option<Option<String>> = conn
        .query_row(
            "SELECT disabled_reason FROM workspace_heartbeats \
             WHERE project_id = ?1 AND name = ?2 AND enabled = 1 AND archived_at IS NULL",
            params![project_id, name],
            |r| r.get(0),
        )
        .optional()?;
    let Some(before) = before else { return Ok(None) };
    conn.execute(
        "UPDATE workspace_heartbeats SET next_retry_at = ?1, disabled_reason = ?2 \
         WHERE project_id = ?3 AND name = ?4 AND enabled = 1",
        params![fmt(until), PAUSE_REASON, project_id, name],
    )?;
    Ok(Some(before.as_deref() != Some(PAUSE_REASON)))
}

/// Move a paused row's time on (a re-check found it still out).
pub fn extend(conn: &Connection, project_id: &str, name: &str, until: i64) -> Result<usize> {
    conn.execute(
        "UPDATE workspace_heartbeats SET next_retry_at = ?1 \
         WHERE project_id = ?2 AND name = ?3 AND enabled = 1 AND disabled_reason = ?4",
        params![fmt(until), project_id, name, PAUSE_REASON],
    )
}

/// Resume a paused row: it fires on its schedule again. The mark stays
/// until that fire's outcome ([`clear`]).
pub fn resume(conn: &Connection, project_id: &str, name: &str) -> Result<usize> {
    conn.execute(
        "UPDATE workspace_heartbeats SET next_retry_at = NULL \
         WHERE project_id = ?1 AND name = ?2 AND enabled = 1 AND disabled_reason = ?3",
        params![project_id, name, PAUSE_REASON],
    )
}

/// Drop the pause mark (a fire went through, or failed for another
/// reason). Leaves `next_retry_at` to the caller.
pub fn clear(conn: &Connection, project_id: &str, name: &str) -> Result<usize> {
    conn.execute(
        "UPDATE workspace_heartbeats SET disabled_reason = NULL \
         WHERE project_id = ?1 AND name = ?2 AND enabled = 1 AND disabled_reason = ?3",
        params![project_id, name, PAUSE_REASON],
    )
}

/// The workspace's paused rows whose time came (`next_retry_at <= now`).
pub fn due_rechecks(conn: &Connection, project_id: &str, now: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name, next_retry_at FROM workspace_heartbeats \
         WHERE project_id = ?1 AND enabled = 1 AND archived_at IS NULL \
           AND disabled_reason = ?2 AND next_retry_at IS NOT NULL",
    )?;
    let rows = stmt.query_map(params![project_id, PAUSE_REASON], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (name, at) = row?;
        if parse(&at).is_none_or(|t| t <= now) {
            out.push(name);
        }
    }
    Ok(out)
}

// ── The notices ─────────────────────────────────────────────────────

/// The first clause of a failure reason, on one line, at most
/// [`SHORT_REASON_MAX`] characters (the notice's parenthesis).
pub fn short_reason(reason: &str) -> String {
    let first = reason.split("; ").next().unwrap_or(reason);
    let one_line = first.split_whitespace().collect::<Vec<_>>().join(" ");
    let one_line = one_line.trim_end_matches(['.', ';', ',']).to_string();
    if one_line.chars().count() <= SHORT_REASON_MAX {
        return one_line;
    }
    let cut: String = one_line.chars().take(SHORT_REASON_MAX - 1).collect();
    format!("{}…", cut.trim_end())
}

/// "<agent>'s heartbeat '<name>' was turned off after 5 failed runs
/// (<short reason>)."
pub fn disabled_text(agent: &str, name: &str, failures: i64, reason: &str) -> String {
    format!(
        "{}'s heartbeat '{}' was turned off after {failures} failed runs ({}).",
        ou::plain(agent),
        ou::plain(name),
        ou::plain(&short_reason(reason)),
    )
}

/// The disable notice: an action line with one button, Turn it back on,
/// which posts the heartbeat's own enable route (Owner and Admin; a Member
/// is refused there).
pub fn disabled_notice(agent: &str, name: &str) -> SystemNotice {
    let mut n = SystemNotice::action(CODE_DISABLED);
    n.actions.push(NoticeAction {
        id: ACTION_ENABLE.to_string(),
        label: "Turn it back on".to_string(),
        route: Some(ENABLE_ROUTE.to_string()),
        body: Some(json!({ "project": agent, "name": name, "enabled": true })),
        workspace: None,
        done_at: None,
        done_by: None,
    });
    n
}

fn why(kind: OutKind) -> &'static str {
    match kind {
        OutKind::Usage => "usage limit",
        OutKind::Credit => "out of credit",
    }
}

/// "<agent>'s heartbeat '<name>' is paused until 15:00 (UTC-07:00) (usage
/// limit); it resumes by itself." With no known reset, it says it checks
/// again every hour.
pub fn paused_text(agent: &str, name: &str, kind: OutKind, reset: Option<i64>, now: i64, offset: FixedOffset) -> String {
    let (agent, name) = (ou::plain(agent), ou::plain(name));
    match reset.filter(|r| *r > now) {
        Some(r) => format!(
            "{agent}'s heartbeat '{name}' is paused until {} ({}); it resumes by itself.",
            ou::when_words(r, now, offset),
            why(kind)
        ),
        None => format!(
            "{agent}'s heartbeat '{name}' is paused ({}); it checks again every hour and resumes by itself.",
            why(kind)
        ),
    }
}

/// The pause notice: an info line (nothing for a person to do).
pub fn paused_notice() -> SystemNotice {
    SystemNotice::info(CODE_PAUSED)
}

#[cfg(test)]
#[path = "usage_pause_tests.rs"]
mod tests;
