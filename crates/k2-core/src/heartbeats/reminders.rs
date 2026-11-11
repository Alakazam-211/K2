//! One-time Reminders: a heartbeat that fires once and goes away
//! (prd-routines-reminders-and-agent-mail-v1 R1, vs-live W6–W13). The product
//! keeps calling the repeating kind heartbeats; a reminder is a row in the
//! same `workspace_heartbeats` table with `kind = 'reminder'` and
//! `frequency = 'once'`, so it gets the lease, the delivery paths, the wait
//! state, the OS wake and the fires audit for free.
//!
//! Its life: pending → `fired` (the delivery stamp retires it in one UPDATE,
//! `AgentHeartbeat::stamp_fired_and_release`), or `missed` (more than 24 h
//! late), `failed` (5 failed deliveries) or `cancelled`. A retired row stays
//! 90 days as history, like the fires audit.
//!
//! Daemon-first: everything here runs with no client attached.

use std::sync::OnceLock;

use chrono::{DateTime, Local, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::when::{self, WhenInput};
use crate::db::schema::{AgentHeartbeat, HeartbeatFire, REMINDER_KIND};
use crate::workspace::agent_identity::resolve_project_id;

/// Longest reminder text. Always inline, under the 4096-byte file-pointer
/// cutoff, so the agent gets the words and not a pointer to them.
pub const MAX_BODY_CHARS: usize = 2000;
/// Pending reminders per workspace.
pub const MAX_PENDING: i64 = 100;
/// Reminder fires per workspace per hour; later ones wait a minute.
pub const MAX_FIRES_PER_HOUR: i64 = 20;
/// How long a retired reminder row is kept (matches the fires prune).
pub const RETIRED_KEEP_DAYS: i64 = 90;
/// Audit decision: the reminder was due, but the workspace is at its hourly
/// cap. It waits for the next minute.
pub const DECISION_DEFERRED_CAP: &str = "deferred_cap";

/// Where a reminder is delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionChoice {
    /// The workspace's pinned chat (the default, as for new heartbeats).
    Pinned,
    /// A session of its own, started for this fire.
    Own,
}

/// What `reminder/add` takes.
#[derive(Debug, Clone)]
pub struct NewReminder<'a> {
    pub text: &'a str,
    pub when: WhenInput<'a>,
    pub name: Option<&'a str>,
    pub file: Option<&'a str>,
    pub session: SessionChoice,
    /// `request_actor`'s label for who set it.
    pub created_by: &'a str,
}

fn fmt_utc(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A person set this reminder (their fire is noted on the Thread). An agent's
/// is delivered to its own session only.
pub fn set_by_a_person(created_by: &str) -> bool {
    !created_by.starts_with("agent:") && !created_by.starts_with("app-token:")
}

/// Who set it, for the stamp. Square brackets can't appear in a stamp.
pub fn who_set(created_by: &str) -> String {
    let who = match created_by.split_once(':') {
        Some(("user" | "agent" | "app" | "app-token", rest)) => rest.to_string(),
        _ => {
            let name = crate::person_name::sender_identity_now(crate::person_name::Sender::OwnerToken);
            let name = name.trim();
            if name.is_empty() || name.eq_ignore_ascii_case("owner") { "the owner".to_string() } else { name.to_string() }
        }
    };
    who.replace(['[', ']'], "").replace(['\r', '\n'], " ")
}

/// The line the agent receives:
/// `[reminder:call-jane · set by Rosson Long 10-26 09:12] Call Jane back.`,
/// with `· late by 3h 10m` when it fires late.
pub fn fire_prompt(hb: &AgentHeartbeat, now: DateTime<Utc>) -> String {
    let fire_at = hb.fire_at.as_deref().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|t| t.with_timezone(&Utc));
    let set_when = DateTime::<Utc>::from_timestamp(hb.created_at, 0).map(|t| local_stamp(t, hb.tz.as_deref()));
    let mut stamp = format!("reminder:{}", hb.name);
    stamp.push_str(&format!(" · set by {}", who_set(&hb.created_by)));
    if let Some(when) = set_when {
        stamp.push_str(&format!(" {when}"));
    }
    if let Some(at) = fire_at {
        let late = (now - at).num_seconds();
        if late > super::cron::ON_TIME_GRACE_SECS {
            stamp.push_str(&format!(" · late by {}", when::human_span(late)));
        }
    }
    format!("[{stamp}] {}", hb.body.as_deref().unwrap_or_default().trim())
}

/// `10-26 09:12` in the reminder's zone (the box's own clock without one).
fn local_stamp(t: DateTime<Utc>, tz: Option<&str>) -> String {
    match tz.and_then(|z| z.parse::<chrono_tz::Tz>().ok()) {
        Some(z) => t.with_timezone(&z).format("%m-%d %H:%M").to_string(),
        None => t.with_timezone(&Local).format("%m-%d %H:%M").to_string(),
    }
}

/// A generated `r-` name that no row (active or retired) has.
fn generate_name(conn: &Connection, project_id: &str) -> Result<String, String> {
    for _ in 0..20 {
        let hex = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("r-{}", &hex[..6]);
        if AgentHeartbeat::get_by_name(conn, project_id, &name).map_err(|e| e.to_string())?.is_none() {
            return Ok(name);
        }
    }
    Err("could not pick a free reminder name; pass --name".to_string())
}

/// Workspace-relative path for `--file`, inside the workspace and present.
fn check_file(project_path: &str, raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let root = std::path::Path::new(project_path);
    let candidate = std::path::Path::new(trimmed);
    let abs = if candidate.is_absolute() { candidate.to_path_buf() } else { root.join(candidate) };
    let real_root = root.canonicalize().map_err(|e| format!("workspace folder: {e}"))?;
    let real = abs.canonicalize().map_err(|_| format!("--file '{trimmed}' doesn't exist"))?;
    let rel = real.strip_prefix(&real_root).map_err(|_| format!("--file '{trimmed}' is outside this workspace"))?;
    if !real.is_file() {
        return Err(format!("--file '{trimmed}' is not a file"));
    }
    Ok(crate::paths::slash_rel(rel))
}

fn body_for(text: &str, file: Option<&str>) -> String {
    match (text.trim().is_empty(), file) {
        (true, Some(f)) => format!("Read {f}."),
        (false, Some(f)) => format!("{} (Read {f}.)", text.trim()),
        (_, None) => text.trim().to_string(),
    }
}

/// Create a reminder. `Err` is a sentence for the caller (exit 2).
pub fn add(project_path: &str, new: NewReminder<'_>, now: DateTime<Utc>) -> Result<serde_json::Value, String> {
    let owner_tz = crate::app_settings::load().owner_timezone;
    let resolved = when::resolve(new.when, owner_tz.as_deref(), now)?;
    let file = new.file.map(|f| check_file(project_path, f)).transpose()?;
    let body = body_for(new.text, file.as_deref());
    if body.is_empty() {
        return Err("a reminder needs some text, or --file".to_string());
    }
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(format!(
            "reminder text is over {MAX_BODY_CHARS} characters; put the long part in a file and use --file"
        ));
    }
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path).ok_or_else(|| format!("Project not found: {project_path}"))?;
    let pending = pending_count(&conn, &project_id).map_err(|e| e.to_string())?;
    if pending >= MAX_PENDING {
        return Err(format!(
            "this workspace already has {MAX_PENDING} pending reminders (the most it can hold); cancel some first: k2 heartbeat remind list"
        ));
    }
    let name = match new.name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => {
            AgentHeartbeat::validate_name(n).map_err(|e| e.to_string())?;
            n.to_string()
        }
        None => generate_name(&conn, &project_id)?,
    };
    if let Some(existing) = AgentHeartbeat::get_by_name(&conn, &project_id, &name).map_err(|e| e.to_string())? {
        if !existing.is_reminder() {
            return Err(format!("a heartbeat is already named '{name}'; pick another --name"));
        }
        if existing.retired_at.is_none() {
            return Err(format!("a reminder named '{name}' is already pending; pick another --name, or: k2 heartbeat remind snooze {name} <when>"));
        }
        // W13: the name of a finished reminder is free again. Its fires
        // keep `schedule_name`, so the history stays.
        AgentHeartbeat::delete(&conn, &project_id, &name).map_err(|e| e.to_string())?;
    }
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO workspace_heartbeats \
         (id, project_id, name, frequency, spec_json, wakeup_path, enabled, created_at, use_workspace_session, \
          kind, fire_at, tz, body, created_by, schedule_anchor_at) \
         VALUES (?1, ?2, ?3, 'once', '{}', '', 1, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            id,
            project_id,
            name,
            now.timestamp(),
            (new.session == SessionChoice::Pinned) as i64,
            REMINDER_KIND,
            fmt_utc(resolved.fire_at),
            resolved.tz,
            body,
            new.created_by,
            fmt_utc(now),
        ],
    )
    .map_err(|e| e.to_string())?;
    let row = AgentHeartbeat::get_by_name(&conn, &project_id, &name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "the reminder vanished as it was saved".to_string())?;
    drop(conn);
    if let Err(e) = super::wait::refresh_project(project_path) {
        crate::log_debug!("[reminders] refresh after add: {e}");
    }
    Ok(serde_json::json!({
        "success": true,
        "name": name,
        "fireAt": fmt_utc(resolved.fire_at),
        "tz": resolved.tz,
        "fires": resolved.display,
        "reminder": reminder_json(&row),
    }))
}

fn pending_count(conn: &Connection, project_id: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM workspace_heartbeats \
         WHERE project_id = ?1 AND kind = 'reminder' AND retired_at IS NULL",
        params![project_id],
        |r| r.get(0),
    )
}

/// The wire shape of one reminder.
pub fn reminder_json(hb: &AgentHeartbeat) -> serde_json::Value {
    serde_json::json!({
        "name": hb.name,
        "kind": hb.kind,
        "text": hb.body,
        "fireAt": hb.fire_at,
        "nextFireAt": hb.next_fire_at,
        "tz": hb.tz,
        "state": match hb.retired_reason.as_deref() {
            Some(r) => r,
            None => "pending",
        },
        "retiredAt": hb.retired_at,
        "firedAt": hb.last_fired,
        "createdAt": hb.created_at,
        "createdBy": hb.created_by,
        "waitReason": hb.wait_reason,
        "waitDetail": hb.wait_detail,
        "failures": hb.consecutive_failures,
        "pinnedChat": hb.use_workspace_session,
    })
}

/// Reminders of a workspace: pending first (soonest first), then, with
/// `all`, the retired ones newest first (kept 90 days).
pub fn list(project_path: &str, all: bool) -> Result<Vec<serde_json::Value>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path).ok_or_else(|| format!("Project not found: {project_path}"))?;
    let rows = AgentHeartbeat::list_by_project(&conn, &project_id).map_err(|e| e.to_string())?;
    let mut pending: Vec<AgentHeartbeat> =
        rows.iter().filter(|h| h.is_reminder() && h.retired_at.is_none()).cloned().collect();
    pending.sort_by(|a, b| a.fire_at.cmp(&b.fire_at));
    let mut out: Vec<serde_json::Value> = pending.iter().map(reminder_json).collect();
    if all {
        let mut retired: Vec<AgentHeartbeat> =
            rows.into_iter().filter(|h| h.is_reminder() && h.retired_at.is_some()).collect();
        retired.sort_by(|a, b| b.retired_at.cmp(&a.retired_at));
        out.extend(retired.iter().map(reminder_json));
    }
    Ok(out)
}

fn load_reminder(conn: &Connection, project_id: &str, name: &str) -> Result<AgentHeartbeat, String> {
    match AgentHeartbeat::get_by_name(conn, project_id, name).map_err(|e| e.to_string())? {
        Some(h) if h.is_reminder() => Ok(h),
        _ => Err(format!("{}: reminder '{name}' not found", super::NO_SUCH_HEARTBEAT)),
    }
}

/// One reminder and its fire history.
pub fn show(project_path: &str, name: &str) -> Result<serde_json::Value, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path).ok_or_else(|| format!("Project not found: {project_path}"))?;
    let hb = load_reminder(&conn, &project_id, name)?;
    let fires = HeartbeatFire::list_by_schedule_name(&conn, &project_id, name, 20).map_err(|e| e.to_string())?;
    let mut v = reminder_json(&hb);
    v["fires"] = serde_json::to_value(fires).map_err(|e| e.to_string())?;
    Ok(v)
}

/// Re-arm a pending or finished reminder for a new time.
pub fn snooze(project_path: &str, name: &str, when_input: WhenInput<'_>, actor: &str, now: DateTime<Utc>) -> Result<serde_json::Value, String> {
    let owner_tz = crate::app_settings::load().owner_timezone;
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path).ok_or_else(|| format!("Project not found: {project_path}"))?;
    let hb = load_reminder(&conn, &project_id, name)?;
    if hb.retired_at.is_none() && hb.in_flight_started_at.is_some() {
        return Err(format!("reminder '{name}' is being delivered right now; try again in a moment"));
    }
    let input = WhenInput { tz: when_input.tz.or(hb.tz.as_deref()), ..when_input };
    let resolved = when::resolve(input, owner_tz.as_deref(), now)?;
    if hb.retired_at.is_some() && pending_count(&conn, &project_id).map_err(|e| e.to_string())? >= MAX_PENDING {
        return Err(format!("this workspace already has {MAX_PENDING} pending reminders; cancel some first"));
    }
    conn.execute(
        "UPDATE workspace_heartbeats SET fire_at = ?1, tz = ?2, enabled = 1, archived_at = NULL, \
                retired_at = NULL, retired_reason = NULL, last_fired = NULL, \
                consecutive_failures = 0, next_retry_at = NULL, disabled_reason = NULL, \
                schedule_anchor_at = ?3 \
         WHERE project_id = ?4 AND name = ?5 AND kind = 'reminder'",
        params![fmt_utc(resolved.fire_at), resolved.tz, fmt_utc(now), project_id, name],
    )
    .map_err(|e| e.to_string())?;
    let detail = format!("snoozed to {} by {}", resolved.display, if actor.is_empty() { "the owner" } else { actor });
    HeartbeatFire::insert_with_schedule(&conn, &project_id, None, Some(name), "once", "changed", Some(&detail), None, None, None)
        .map_err(|e| e.to_string())?;
    drop(conn);
    if let Err(e) = super::wait::refresh_project(project_path) {
        crate::log_debug!("[reminders] refresh after snooze: {e}");
    }
    Ok(serde_json::json!({ "success": true, "name": name, "fireAt": fmt_utc(resolved.fire_at), "fires": resolved.display }))
}

/// Cancel a pending reminder (kept as history). A finished one is refused:
/// there is nothing left to stop.
pub fn cancel(project_path: &str, name: &str, actor: &str, now: DateTime<Utc>) -> Result<serde_json::Value, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path).ok_or_else(|| format!("Project not found: {project_path}"))?;
    let hb = load_reminder(&conn, &project_id, name)?;
    if let Some(reason) = hb.retired_reason.as_deref() {
        return Err(format!("reminder '{name}' already finished ({reason}); nothing to cancel"));
    }
    retire(&conn, &project_id, name, "cancelled", now).map_err(|e| e.to_string())?;
    let detail = format!("cancelled by {}", if actor.is_empty() { "the owner" } else { actor });
    HeartbeatFire::insert_with_schedule(&conn, &project_id, None, Some(name), "once", "changed", Some(&detail), None, None, None)
        .map_err(|e| e.to_string())?;
    drop(conn);
    if let Err(e) = super::wait::refresh_project(project_path) {
        crate::log_debug!("[reminders] refresh after cancel: {e}");
    }
    Ok(serde_json::json!({ "success": true, "name": name }))
}

/// Retire a reminder that did not fire (`missed`, `failed`, `cancelled`).
/// A reminder that fired is retired by `stamp_fired_and_release`.
pub fn retire(conn: &Connection, project_id: &str, name: &str, reason: &str, now: DateTime<Utc>) -> rusqlite::Result<usize> {
    let at = fmt_utc(now);
    conn.execute(
        "UPDATE workspace_heartbeats SET enabled = 0, retired_at = ?1, retired_reason = ?2, \
                archived_at = ?1, in_flight_started_at = NULL, next_retry_at = NULL \
         WHERE project_id = ?3 AND name = ?4 AND kind = 'reminder' AND retired_at IS NULL",
        params![at, reason, project_id, name],
    )
}

/// How many reminders this workspace fired in the last hour.
///
/// `fired_at` is written in local time with its offset, so it is compared
/// as an instant (SQLite's `datetime()` turns any offset into UTC), never
/// as a string: west of UTC a string compare missed every recent fire.
pub fn fires_last_hour(conn: &Connection, project_id: &str, now: DateTime<Utc>) -> rusqlite::Result<i64> {
    let since = (now - chrono::Duration::hours(1)).to_rfc3339();
    conn.query_row(
        "SELECT COUNT(*) FROM heartbeat_fires \
         WHERE project_id = ?1 AND mode = 'once' AND decision IN ('fired', 'fired_catchup') \
           AND datetime(fired_at) >= datetime(?2)",
        params![project_id, since],
        |r| r.get(0),
    )
}

/// Delete retired reminder rows older than [`RETIRED_KEEP_DAYS`] (their
/// fires are pruned on the same clock). Returns the count.
pub fn prune_retired(conn: &Connection, now: DateTime<Utc>) -> rusqlite::Result<usize> {
    let cutoff = fmt_utc(now - chrono::Duration::days(RETIRED_KEEP_DAYS));
    conn.execute(
        "DELETE FROM workspace_heartbeats WHERE kind = 'reminder' AND retired_at IS NOT NULL AND retired_at < ?1",
        params![cutoff],
    )
}

/// The soonest pending reminder's `fire_at` at or before `now` and after
/// `after`, for the monitor's precise pass (W10). `None` = nothing new is due.
pub fn newly_due(conn: &Connection, after: Option<&str>, now: DateTime<Utc>) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT MAX(fire_at) FROM workspace_heartbeats \
         WHERE kind = 'reminder' AND enabled = 1 AND archived_at IS NULL \
           AND in_flight_started_at IS NULL AND fire_at <= ?1 \
           AND (?2 IS NULL OR fire_at > ?2)",
        params![fmt_utc(now), after],
        |r| r.get(0),
    )
    .optional()
    .map(Option::flatten)
}

// ── Notes from core to the daemon ─────────────────────────────────────

type NoteSink = fn(project_path: &str, text: &str);
static NOTE_SINK: OnceLock<NoteSink> = OnceLock::new();

/// The daemon registers how a reminder note reaches the owner's Thread
/// (`overlay_routes::post_workspace_note`), since the Thread store is the
/// daemon's. Called once at boot.
pub fn set_note_sink(sink: NoteSink) {
    let _ = NOTE_SINK.set(sink);
}

/// Tell the owner's Thread something happened to a reminder. Logged if the
/// note can't be sent; a reminder's retirement does not wait on it.
pub fn post_note(project_path: &str, text: &str) {
    match NOTE_SINK.get() {
        Some(sink) => sink(project_path, text),
        None => crate::log_debug!("[reminders] no note sink for {project_path}: {text}"),
    }
}

/// The owner's note for a reminder that will not fire.
pub fn missed_note(hb: &AgentHeartbeat, why: &str) -> String {
    let text = hb.body.as_deref().unwrap_or_default();
    let short: String = text.chars().take(120).collect();
    format!("Reminder '{}' didn't fire: {why}. It said: {short}", hb.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heartbeats::cron::{self, DueStatus};

    fn workspace(label: &str) -> (std::path::PathBuf, String) {
        crate::db::init_for_tests();
        let dir = std::env::temp_dir().join(format!("k2-rem-{label}-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let id = uuid::Uuid::new_v4().to_string();
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, 'rem', ?2)",
            params![id, dir.to_string_lossy()],
        )
        .expect("project");
        (dir, id)
    }

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).expect("instant").with_timezone(&Utc)
    }

    fn add_in(path: &std::path::Path, name: &str, input: WhenInput<'_>, now: DateTime<Utc>) -> serde_json::Value {
        add(
            path.to_str().expect("path"),
            NewReminder { text: "call Jane back", when: input, name: Some(name), file: None, session: SessionChoice::Pinned, created_by: "owner-token" },
            now,
        )
        .expect("add")
    }

    fn row(project: &str, name: &str) -> AgentHeartbeat {
        let db = crate::db::shared();
        let conn = db.lock();
        AgentHeartbeat::get_by_name(&conn, project, name).expect("query").expect("row")
    }

    fn eval(hb: &AgentHeartbeat, t: DateTime<Utc>) -> DueStatus {
        cron::evaluate_with_now(hb, t.with_timezone(&Local))
    }

    #[test]
    fn once_is_not_due_one_second_before_and_due_at_the_instant() {
        let (dir, pid) = workspace("due");
        let now = at("2026-06-10T18:00:00Z");
        add_in(&dir, "a", WhenInput { in_: Some("45m"), ..Default::default() }, now);
        let hb = row(&pid, "a");
        let t = at("2026-06-10T18:45:00Z");
        assert!(matches!(eval(&hb, t - chrono::Duration::seconds(1)), DueStatus::NotYet { .. }), "not one second before");
        assert!(matches!(eval(&hb, t), DueStatus::Due { .. }), "due at the instant");
    }

    #[test]
    fn once_catches_up_past_the_twelve_hour_repeat_rule_and_misses_only_after_24h() {
        // W8: 13 h late still fires, which classify()'s 12 h rule would skip.
        let (dir, pid) = workspace("late");
        let now = at("2026-06-10T18:00:00Z");
        add_in(&dir, "a", WhenInput { in_: Some("1h"), ..Default::default() }, now);
        let hb = row(&pid, "a");
        let fire = at("2026-06-10T19:00:00Z");
        assert!(matches!(eval(&hb, fire + chrono::Duration::hours(13)), DueStatus::DueCatchUp { .. }));
        assert!(matches!(eval(&hb, fire + chrono::Duration::hours(24)), DueStatus::DueCatchUp { .. }), "exactly 24 h is still late, not missed");
        assert!(matches!(eval(&hb, fire + chrono::Duration::hours(24) + chrono::Duration::seconds(1)), DueStatus::SkippedMissed { .. }));
    }

    #[test]
    fn every_allowed_frequency_and_once_evaluate_to_something_real() {
        let (dir, pid) = workspace("freq");
        let now = at("2026-06-10T18:00:00Z");
        add_in(&dir, "a", WhenInput { in_: Some("1h"), ..Default::default() }, now);
        let hb = row(&pid, "a");
        assert!(!matches!(eval(&hb, now), DueStatus::Invalid { .. }), "once must evaluate");
        for freq in crate::heartbeats::ALLOWED_FREQUENCIES {
            let mut h = hb.clone();
            h.frequency = freq.to_string();
            h.fire_at = None;
            h.kind = crate::db::schema::HEARTBEAT_KIND.to_string();
            h.spec_json = match freq {
                "weekly" => r#"{"time":"09:00","days":["mon"]}"#.to_string(),
                _ => r#"{"time":"09:00"}"#.to_string(),
            };
            assert!(!matches!(eval(&h, now), DueStatus::Invalid { .. }), "{freq} must evaluate");
        }
    }

    #[test]
    fn scheduled_is_not_a_frequency_any_more() {
        crate::heartbeats::validate_spec_json("scheduled", "{}").expect_err("scheduled is gone");
        crate::heartbeats::validate_spec_json("once", "{}").expect_err("once is reminder/add's, not heartbeat/add's");
    }

    #[test]
    fn stamp_retires_a_reminder_and_leaves_a_heartbeat_alone() {
        let (dir, pid) = workspace("stamp");
        let now = at("2026-06-10T18:00:00Z");
        add_in(&dir, "a", WhenInput { in_: Some("1h"), ..Default::default() }, now);
        let db = crate::db::shared();
        {
            let conn = db.lock();
            AgentHeartbeat::insert(&conn, "plain-id", &pid, "plain", "daily", "{}", ".k2/heartbeats/plain/WAKEUP.md", true).expect("insert");
            AgentHeartbeat::stamp_fired_and_release(&conn, &pid, "a").expect("stamp reminder");
            AgentHeartbeat::stamp_fired_and_release(&conn, &pid, "plain").expect("stamp heartbeat");
        }
        let r = row(&pid, "a");
        assert!(!r.enabled, "a fired reminder is off");
        assert_eq!(r.retired_reason.as_deref(), Some("fired"));
        assert!(r.retired_at.is_some() && r.archived_at.is_some(), "retired and archived together");
        let h = row(&pid, "plain");
        assert!(h.enabled && h.archived_at.is_none() && h.retired_at.is_none(), "a heartbeat keeps going");
    }

    #[test]
    fn a_fired_reminder_is_not_a_scan_candidate_again() {
        let (dir, pid) = workspace("candidate");
        add_in(&dir, "a", WhenInput { in_: Some("1h"), ..Default::default() }, at("2026-06-10T18:00:00Z"));
        let db = crate::db::shared();
        let conn = db.lock();
        AgentHeartbeat::stamp_fired_and_release(&conn, &pid, "a").expect("stamp");
        let enabled = AgentHeartbeat::list_enabled(&conn, &pid).expect("list");
        assert!(enabled.iter().all(|h| h.name != "a"), "a retired reminder is out of the scan");
    }

    #[test]
    fn the_lease_recheck_refuses_a_second_scan_of_the_same_reminder() {
        use crate::db::schema::{LeaseCheck, LeaseOutcome};
        let (dir, pid) = workspace("race");
        add_in(&dir, "a", WhenInput { in_: Some("1h"), ..Default::default() }, at("2026-06-10T18:00:00Z"));
        let db = crate::db::shared();
        let conn = db.lock();
        assert_eq!(
            AgentHeartbeat::try_acquire_heartbeat_checked(&conn, &pid, "a", LeaseCheck::Scheduled(None)).expect("first"),
            LeaseOutcome::Acquired
        );
        AgentHeartbeat::stamp_fired_and_release(&conn, &pid, "a").expect("stamp");
        assert_eq!(
            AgentHeartbeat::try_acquire_heartbeat_checked(&conn, &pid, "a", LeaseCheck::Scheduled(None)).expect("second"),
            LeaseOutcome::AlreadyFired,
            "the slower scan read last_fired before the fast one fired it"
        );
    }

    #[test]
    fn next_fire_estimate_is_the_instant_and_none_once_missed() {
        let (dir, pid) = workspace("next");
        let now = at("2026-06-10T18:00:00Z");
        add_in(&dir, "a", WhenInput { in_: Some("1h"), ..Default::default() }, now);
        let hb = row(&pid, "a");
        let fire = at("2026-06-10T19:00:00Z");
        assert_eq!(
            cron::next_fire_estimate(&hb, now.with_timezone(&Local)).map(|t| t.with_timezone(&Utc)),
            Some(fire),
            "the OS wake plan reads this"
        );
        assert_eq!(cron::next_fire_estimate(&hb, (fire + chrono::Duration::hours(25)).with_timezone(&Local)), None);
        assert_eq!(cron::first_slot_after_reference(&hb).map(|t| t.with_timezone(&Utc)), Some(fire));
    }

    #[test]
    fn add_stores_next_fire_at_equal_to_fire_at() {
        let (dir, pid) = workspace("next-stored");
        add_in(&dir, "a", WhenInput { in_: Some("1h"), ..Default::default() }, Utc::now());
        let hb = row(&pid, "a");
        assert!(hb.fire_at.is_some());
        assert_eq!(hb.next_fire_at, hb.fire_at, "the wait pass stored the same instant");
    }

    #[test]
    fn body_is_required_and_capped_and_file_is_jailed() {
        let (dir, _pid) = workspace("body");
        let path = dir.to_str().expect("path");
        let now = at("2026-06-10T18:00:00Z");
        let when = WhenInput { in_: Some("1h"), ..Default::default() };
        let base = NewReminder { text: "", when, name: None, file: None, session: SessionChoice::Pinned, created_by: "" };
        assert!(add(path, base.clone(), now).expect_err("empty").contains("needs some text"));
        let long = "x".repeat(MAX_BODY_CHARS + 1);
        assert!(add(path, NewReminder { text: &long, ..base.clone() }, now).expect_err("long").contains("over 2000"));
        assert!(add(path, NewReminder { text: "x", file: Some("nope.md"), ..base.clone() }, now).expect_err("missing").contains("doesn't exist"));
        let outside = std::env::temp_dir().join(format!("k2-rem-outside-{}", uuid::Uuid::new_v4()));
        std::fs::write(&outside, "secret").expect("outside");
        let outside_s = outside.to_str().expect("outside path");
        assert!(add(path, NewReminder { text: "x", file: Some(outside_s), ..base.clone() }, now).expect_err("outside").contains("outside this workspace"));
        std::fs::create_dir_all(dir.join("notes")).expect("notes");
        std::fs::write(dir.join("notes/x.md"), "hi").expect("file");
        let v = add(path, NewReminder { text: "", file: Some("notes/x.md"), name: Some("f"), ..base }, now).expect("file ok");
        assert_eq!(v["reminder"]["text"], "Read notes/x.md.");
    }

    #[test]
    fn a_pending_name_is_refused_but_a_retired_one_is_free_again() {
        let (dir, pid) = workspace("names");
        let now = at("2026-06-10T18:00:00Z");
        let when = WhenInput { in_: Some("1h"), ..Default::default() };
        add_in(&dir, "call-jane", when, now);
        let e = add(dir.to_str().expect("p"), NewReminder { text: "x", when, name: Some("call-jane"), file: None, session: SessionChoice::Pinned, created_by: "" }, now)
            .expect_err("pending name");
        assert!(e.contains("already pending"), "{e}");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            AgentHeartbeat::stamp_fired_and_release(&conn, &pid, "call-jane").expect("stamp");
        }
        add_in(&dir, "call-jane", when, now);
        assert!(row(&pid, "call-jane").retired_at.is_none(), "a fresh pending reminder under the old name");
    }

    #[test]
    fn the_101st_pending_reminder_is_refused() {
        let (dir, _pid) = workspace("cap");
        let now = at("2026-06-10T18:00:00Z");
        let when = WhenInput { in_: Some("1h"), ..Default::default() };
        for i in 0..MAX_PENDING {
            add_in(&dir, &format!("r{i}"), when, now);
        }
        let e = add(dir.to_str().expect("p"), NewReminder { text: "x", when, name: Some("one-more"), file: None, session: SessionChoice::Pinned, created_by: "" }, now)
            .expect_err("over the cap");
        assert!(e.contains("100 pending"), "{e}");
    }

    #[test]
    fn snooze_rearms_a_fired_reminder_and_cancel_keeps_history() {
        let (dir, pid) = workspace("snooze");
        let path = dir.to_str().expect("p");
        let now = at("2026-06-10T18:00:00Z");
        add_in(&dir, "a", WhenInput { in_: Some("1h"), ..Default::default() }, now);
        {
            let db = crate::db::shared();
            let conn = db.lock();
            AgentHeartbeat::stamp_fired_and_release(&conn, &pid, "a").expect("stamp");
        }
        snooze(path, "a", WhenInput { in_: Some("10m"), ..Default::default() }, "owner-token", at("2026-06-10T19:05:00Z")).expect("snooze");
        let r = row(&pid, "a");
        assert!(r.enabled && r.retired_at.is_none() && r.archived_at.is_none() && r.last_fired.is_none(), "armed again");
        assert_eq!(r.fire_at.as_deref(), Some("2026-06-10T19:15:00Z"));
        cancel(path, "a", "owner-token", at("2026-06-10T19:06:00Z")).expect("cancel");
        let c = row(&pid, "a");
        assert_eq!(c.retired_reason.as_deref(), Some("cancelled"));
        assert!(!c.enabled);
        let all = list(path, true).expect("list");
        assert_eq!(all.len(), 1, "a cancelled reminder stays as history: {all:?}");
        assert!(list(path, false).expect("pending").is_empty(), "and is not pending");
        assert!(cancel(path, "a", "", at("2026-06-10T19:07:00Z")).expect_err("again").contains("already finished"));
    }

    #[test]
    fn fire_prompt_stamps_who_when_and_how_late() {
        let (dir, pid) = workspace("prompt");
        let now = at("2026-06-10T18:00:00Z");
        add(
            dir.to_str().expect("p"),
            NewReminder { text: "call Jane back", when: WhenInput { in_: Some("1h"), tz: Some("America/Denver"), ..Default::default() }, name: Some("a"), file: None, session: SessionChoice::Pinned, created_by: "user:rosson" },
            now,
        )
        .expect("add");
        let hb = row(&pid, "a");
        assert_eq!(fire_prompt(&hb, at("2026-06-10T19:00:00Z")), "[reminder:a · set by rosson 06-10 12:00] call Jane back");
        assert_eq!(
            fire_prompt(&hb, at("2026-06-10T22:10:00Z")),
            "[reminder:a · set by rosson 06-10 12:00 · late by 3h 10m] call Jane back"
        );
    }

    #[test]
    fn newly_due_only_reports_what_it_has_not_already() {
        let (dir, _pid) = workspace("due2");
        add_in(&dir, "a", WhenInput { in_: Some("1h"), ..Default::default() }, at("2026-06-10T18:00:00Z"));
        let db = crate::db::shared();
        let conn = db.lock();
        assert_eq!(newly_due(&conn, None, at("2026-06-10T18:59:00Z")).expect("q"), None);
        let t = newly_due(&conn, None, at("2026-06-10T19:00:05Z")).expect("q").expect("due");
        assert_eq!(t, "2026-06-10T19:00:00Z");
        assert_eq!(newly_due(&conn, Some(&t), at("2026-06-10T19:00:10Z")).expect("q"), None, "reported once");
    }

    #[test]
    fn set_by_a_person_tells_people_from_agents() {
        assert!(set_by_a_person("owner-token"));
        assert!(set_by_a_person("user:rosson"));
        assert!(set_by_a_person(""));
        assert!(!set_by_a_person("agent:sales"));
    }

    /// `fired_at` is written in local time with its offset and the windows
    /// are UTC: counts compare instants, so a zone west or east of UTC
    /// can't hide a recent fire or count an old one. Two fires written at
    /// -07:00 in the last half hour count; one written at +09:00 two hours
    /// ago doesn't. (A string compare in UTC got this exactly backwards.)
    #[test]
    fn hour_counts_compare_instants_whatever_offset_a_fire_was_written_with() {
        let (dir, pid) = workspace("offsets");
        let now = chrono::Utc::now();
        let west = chrono::FixedOffset::west_opt(7 * 3600).expect("-07:00");
        let east = chrono::FixedOffset::east_opt(9 * 3600).expect("+09:00");
        let rows = [
            (now - chrono::Duration::minutes(10)).with_timezone(&west).to_rfc3339(),
            (now - chrono::Duration::minutes(20)).with_timezone(&west).to_rfc3339(),
            (now - chrono::Duration::hours(2)).with_timezone(&east).to_rfc3339(),
        ];
        let db = crate::db::shared();
        let conn = db.lock();
        for at in &rows {
            conn.execute(
                "INSERT INTO heartbeat_fires (project_id, agent_name, fired_at, mode, decision) VALUES (?1, 'manager', ?2, 'once', 'fired')",
                params![pid, at],
            )
            .expect("fire");
        }
        assert_eq!(fires_last_hour(&conn, &pid, now).expect("count"), 2, "the reminder cap's hour: {rows:?}");
        assert_eq!(
            crate::wake_budget::wakes_since(&conn, &pid, now.timestamp() - 3600).expect("budget"),
            2,
            "the agent wake budget's hour: {rows:?}"
        );
        // The 90-day prune compares instants too: a cutoff one hour ago
        // removes only the +09:00 row from two hours ago.
        let cutoff = (now - chrono::Duration::hours(1)).to_rfc3339();
        assert_eq!(crate::db::schema::HeartbeatFire::prune_before(&conn, &cutoff).expect("prune"), 1, "only the old row");
        drop(conn);
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
