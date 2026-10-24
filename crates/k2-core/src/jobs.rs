//! `k2 job` — daemon-run scheduled jobs (prd-vault-gaps-v1 rev 2, item 5).
//!
//! This module is the store (migration 0220: `jobs`, `job_runs`,
//! `job_settings`) and the due evaluator. The daemon owns the loop, the
//! process, the passport and vault delivery (`k2_daemon::jobs`).
//!
//! Schedules: `every:<secs>` (`--every 15m`, at least a minute) or
//! `cron:<expr>` (5-field cron, local time, evaluated with croner like
//! heartbeats). Semantics follow heartbeats (`heartbeats::cron`):
//! - **Due iff a slot after the anchor is ≤ now.** The anchor is the last
//!   slot looked at; a new job, enable, a schedule edit and `resume --all`
//!   anchor at now, so none of them fires a catch-up.
//! - **One catch-up within 12 hours.** Missed slots coalesce into the
//!   latest one; it fires when at most [`MISSED_RUN_GRACE_SECS`] late and is
//!   audited `skipped_missed` otherwise.

use chrono::{DateTime, Duration, Local, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::str::FromStr;

/// A missed slot still runs once when at most this late (as heartbeats).
pub const MISSED_RUN_GRACE_SECS: i64 = 12 * 3600;
/// A run this late after its slot is a catch-up (audited `catchup`).
pub const ON_TIME_GRACE_SECS: i64 = 15 * 60;
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;
pub const MAX_TIMEOUT_SECS: u64 = 6 * 3600;
pub const MIN_EVERY_SECS: u64 = 60;
pub const MAX_EVERY_SECS: u64 = 31 * 86_400;
/// Jobs per workspace.
pub const MAX_JOBS_PER_WORKSPACE: usize = 50;
/// Logs kept per job (older runs keep their row, lose the log).
pub const KEEP_LOGS: usize = 20;
/// Run rows kept per job.
pub const KEEP_RUN_ROWS: usize = 200;
const MAX_ARGS: usize = 64;
const MAX_ARGV_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobError {
    pub code: &'static str,
    pub status: u16,
    pub hint: String,
}

impl JobError {
    pub fn new(code: &'static str, status: u16, hint: impl Into<String>) -> JobError {
        JobError { code, status, hint: hint.into() }
    }
    fn bad(code: &'static str, hint: impl Into<String>) -> JobError {
        JobError::new(code, 400, hint)
    }
    fn db(e: rusqlite::Error) -> JobError {
        JobError::new("db_error", 500, format!("job store: {e}"))
    }
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.hint)
    }
}

pub type Result<T> = std::result::Result<T, JobError>;

// ── schedule ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schedule {
    Every(u64),
    Cron(String),
}

impl Schedule {
    /// `15m`, `90s`, `2h`, `1d`, or plain seconds. At least a minute.
    pub fn parse_every(s: &str) -> Result<Schedule> {
        let t = s.trim();
        let bad = || JobError::bad("bad_schedule", "--every takes a duration like 15m, 1h or 1d (at least 1m)");
        if t.is_empty() {
            return Err(bad());
        }
        let (num, unit) = match t.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
            Some((i, _)) => (&t[..i], &t[i..]),
            None => (t, "s"),
        };
        let n: u64 = num.parse().map_err(|_| bad())?;
        let mult = match unit {
            "s" | "sec" | "secs" => 1,
            "m" | "min" | "mins" => 60,
            "h" | "hr" | "hrs" => 3600,
            "d" | "day" | "days" => 86_400,
            _ => return Err(bad()),
        };
        let secs = n.checked_mul(mult).ok_or_else(bad)?;
        if !(MIN_EVERY_SECS..=MAX_EVERY_SECS).contains(&secs) {
            return Err(bad());
        }
        Ok(Schedule::Every(secs))
    }

    /// A 5-field cron expression (minute hour day month weekday), local time.
    pub fn parse_cron(expr: &str) -> Result<Schedule> {
        let e = expr.split_whitespace().collect::<Vec<_>>().join(" ");
        if e.split(' ').count() != 5 {
            return Err(JobError::bad("bad_schedule", "--cron takes 5 fields: minute hour day month weekday"));
        }
        let c = croner::Cron::from_str(&e).map_err(|x| JobError::bad("bad_schedule", format!("cron expression rejected: {x}")))?;
        c.find_next_occurrence(&Local::now(), false)
            .map_err(|x| JobError::bad("bad_schedule", format!("that schedule never runs ({x})")))?;
        Ok(Schedule::Cron(e))
    }

    pub fn to_db(&self) -> String {
        match self {
            Schedule::Every(s) => format!("every:{s}"),
            Schedule::Cron(e) => format!("cron:{e}"),
        }
    }

    pub fn from_db(s: &str) -> Result<Schedule> {
        if let Some(n) = s.strip_prefix("every:") {
            let secs: u64 = n.parse().map_err(|_| JobError::bad("bad_schedule", "stored schedule"))?;
            return Ok(Schedule::Every(secs));
        }
        if let Some(e) = s.strip_prefix("cron:") {
            return Ok(Schedule::Cron(e.to_string()));
        }
        Err(JobError::bad("bad_schedule", "stored schedule"))
    }

    /// `every 15m` / `cron */15 * * * *`.
    pub fn describe(&self) -> String {
        match self {
            Schedule::Every(s) => format!("every {}", human_secs(*s)),
            Schedule::Cron(e) => format!("cron {e}"),
        }
    }
}

/// `900` → `15m`, `5400` → `90m`, `7200` → `2h`.
pub fn human_secs(s: u64) -> String {
    if s % 86_400 == 0 {
        format!("{}d", s / 86_400)
    } else if s % 3600 == 0 {
        format!("{}h", s / 3600)
    } else if s % 60 == 0 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

/// `--timeout`: same duration grammar, 1 s to 6 h.
pub fn parse_timeout(s: &str) -> Result<u64> {
    let t = s.trim();
    let bad = || JobError::bad("bad_timeout", "--timeout takes a duration like 30s, 10m or 1h (at most 6h)");
    let (num, unit) = match t.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => (&t[..i], &t[i..]),
        None => (t, "s"),
    };
    let n: u64 = num.parse().map_err(|_| bad())?;
    let mult = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return Err(bad()),
    };
    let secs = n.checked_mul(mult).ok_or_else(bad)?;
    if secs == 0 || secs > MAX_TIMEOUT_SECS {
        return Err(bad());
    }
    Ok(secs)
}

/// What the evaluator says about one job at `now`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Due {
    /// Nothing due; the next slot (for `show`).
    NotYet { next: Option<DateTime<Utc>> },
    /// Run now for `slot`; `catchup` when the slot is over 15 min old.
    Fire { slot: DateTime<Utc>, catchup: bool },
    /// The latest missed slot is over 12 h old: audit it, re-anchor.
    SkipMissed { slot: DateTime<Utc> },
    Invalid { reason: String },
}

/// Due evaluation (see the module doc). `anchor` = the last slot looked at.
pub fn evaluate(schedule: &Schedule, anchor: DateTime<Utc>, now: DateTime<Utc>) -> Due {
    let latest: Option<DateTime<Utc>> = match schedule {
        Schedule::Every(secs) => {
            let secs = (*secs).max(MIN_EVERY_SECS) as i64;
            let since = (now - anchor).num_seconds();
            if since < secs {
                return Due::NotYet { next: Some(anchor + Duration::seconds(secs)) };
            }
            Some(anchor + Duration::seconds((since / secs) * secs))
        }
        Schedule::Cron(expr) => {
            let cron = match croner::Cron::from_str(expr) {
                Ok(c) => c,
                Err(e) => return Due::Invalid { reason: format!("cron expression rejected: {e}") },
            };
            let now_l = now.with_timezone(&Local);
            let anchor_l = anchor.with_timezone(&Local);
            let next = match cron.find_next_occurrence(&anchor_l, false) {
                Ok(t) => t,
                Err(e) => return Due::Invalid { reason: format!("schedule never runs ({e})") },
            };
            if next > now_l {
                return Due::NotYet { next: Some(next.with_timezone(&Utc)) };
            }
            match cron.find_previous_occurrence(&now_l, true) {
                Ok(p) if p > anchor_l => Some(p.with_timezone(&Utc)),
                // Walking back failed (or landed at/before the anchor): the
                // forward slot is the one that's due.
                _ => Some(next.with_timezone(&Utc)),
            }
        }
    };
    let slot = latest.expect("set above");
    let late = (now - slot).num_seconds();
    if late > MISSED_RUN_GRACE_SECS {
        return Due::SkipMissed { slot };
    }
    Due::Fire { slot, catchup: late > ON_TIME_GRACE_SECS }
}

/// The next slot after `anchor` (for `show`), or `None`.
pub fn next_slot(schedule: &Schedule, anchor: DateTime<Utc>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match evaluate(schedule, anchor, now) {
        Due::NotYet { next } => next,
        Due::Fire { .. } => Some(now),
        _ => None,
    }
}

// ── validation ──────────────────────────────────────────────────────

/// `[a-z0-9][a-z0-9_.-]{0,63}` (lower case: names are addresses).
pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_lowercase() | name.as_bytes()[0].is_ascii_digit()
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-'))
        && name != "."
        && name != "..";
    if !ok {
        return Err(JobError::bad("bad_name", "job names are 1–64 characters: a-z 0-9 _ . - (starting with a letter or digit)"));
    }
    Ok(())
}

pub fn validate_argv(argv: &[String]) -> Result<()> {
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(JobError::bad("command_required", "give the command after --: k2 job add NAME --every 15m -- ./collect.sh"));
    }
    if argv.len() > MAX_ARGS {
        return Err(JobError::bad("bad_command", format!("at most {MAX_ARGS} arguments")));
    }
    if argv.iter().map(|a| a.len()).sum::<usize>() > MAX_ARGV_BYTES || argv.iter().any(|a| a.contains('\0')) {
        return Err(JobError::bad("bad_command", "the command is at most 16 KiB, no NUL bytes"));
    }
    Ok(())
}

/// A relative folder inside the workspace (no `..`, not absolute). The
/// daemon checks it again on disk (symlinks) at every run.
pub fn validate_cwd(cwd: &str) -> Result<String> {
    let c = cwd.trim().trim_end_matches('/');
    if c.is_empty() || c == "." {
        return Ok(String::new());
    }
    let p = std::path::Path::new(c);
    if p.is_absolute() || c.contains('\0') || p.components().any(|x| !matches!(x, std::path::Component::Normal(_))) {
        return Err(JobError::bad("path_outside_workspace", "--cwd is a folder inside your workspace (relative, no ..)"));
    }
    Ok(c.to_string())
}

// ── rows ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    #[serde(serialize_with = "ser_schedule")]
    pub schedule: Schedule,
    pub argv: Vec<String>,
    pub cwd: String,
    pub timeout_secs: u64,
    pub enabled: bool,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
    pub updated_by: String,
    pub anchor_at: String,
}

fn ser_schedule<S: serde::Serializer>(s: &Schedule, ser: S) -> std::result::Result<S::Ok, S::Error> {
    ser.serialize_str(&s.describe())
}

impl Job {
    pub fn anchor(&self) -> DateTime<Utc> {
        parse_ts(&self.anchor_at).unwrap_or_else(Utc::now)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub job_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub outcome: String,
    pub trigger: String,
    pub exit_code: Option<i64>,
    #[serde(skip)]
    pub log_path: Option<String>,
    pub bytes: u64,
    pub pid: Option<i64>,
    pub started_by: String,
}

pub fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&Utc))
}

fn job_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(Job, String)> {
    let sched: String = r.get("schedule")?;
    let argv_json: String = r.get("argv_json")?;
    let argv: Vec<String> = serde_json::from_str(&argv_json).unwrap_or_default();
    Ok((
        Job {
            id: r.get("id")?,
            workspace_id: r.get("workspace_id")?,
            name: r.get("name")?,
            schedule: Schedule::from_db(&sched).unwrap_or(Schedule::Every(0)),
            argv,
            cwd: r.get("cwd")?,
            timeout_secs: r.get::<_, i64>("timeout_secs")?.max(1) as u64,
            enabled: r.get::<_, i64>("enabled")? != 0,
            created_by: r.get("created_by")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
            updated_by: r.get("updated_by")?,
            anchor_at: r.get("anchor_at")?,
        },
        sched,
    ))
}

fn run_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: r.get("id")?,
        job_id: r.get("job_id")?,
        started_at: r.get("started_at")?,
        ended_at: r.get("ended_at")?,
        outcome: r.get("outcome")?,
        trigger: r.get("trigger")?,
        exit_code: r.get("exit_code")?,
        log_path: r.get("log_path")?,
        bytes: r.get::<_, i64>("bytes")?.max(0) as u64,
        pid: r.get("pid")?,
        started_by: r.get("started_by")?,
    })
}

/// A new job (validated here).
#[derive(Debug, Clone)]
pub struct NewJob {
    pub workspace_id: String,
    pub name: String,
    pub schedule: Schedule,
    pub argv: Vec<String>,
    pub cwd: String,
    pub timeout_secs: u64,
    pub created_by: String,
}

pub fn insert(conn: &Connection, j: NewJob, now: DateTime<Utc>) -> Result<Job> {
    validate_name(&j.name)?;
    validate_argv(&j.argv)?;
    let cwd = validate_cwd(&j.cwd)?;
    if j.timeout_secs == 0 || j.timeout_secs > MAX_TIMEOUT_SECS {
        return Err(JobError::bad("bad_timeout", "timeouts are 1 s to 6 h"));
    }
    if get_by_name(conn, &j.workspace_id, &j.name)?.is_some() {
        return Err(JobError::new("job_exists", 409, format!("a job named {} already exists here (k2 job edit, or k2 job rm first)", j.name)));
    }
    if list(conn, Some(&j.workspace_id))?.len() >= MAX_JOBS_PER_WORKSPACE {
        return Err(JobError::new("too_many_jobs", 409, format!("a workspace has at most {MAX_JOBS_PER_WORKSPACE} jobs")));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let at = ts(now);
    conn.execute(
        "INSERT INTO jobs (id, workspace_id, name, schedule, argv_json, cwd, timeout_secs, enabled, created_by, created_at, updated_at, updated_by, anchor_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?9, ?8, ?9)",
        params![
            id,
            j.workspace_id,
            j.name,
            j.schedule.to_db(),
            serde_json::to_string(&j.argv).expect("argv json"),
            cwd,
            j.timeout_secs as i64,
            j.created_by,
            at
        ],
    )
    .map_err(JobError::db)?;
    get(conn, &id)?.ok_or_else(|| JobError::new("db_error", 500, "job vanished after insert"))
}

/// An edit: `None` keeps the field. A schedule change re-anchors at now.
#[derive(Debug, Clone, Default)]
pub struct JobEdit {
    pub schedule: Option<Schedule>,
    pub argv: Option<Vec<String>>,
    pub cwd: Option<String>,
    pub timeout_secs: Option<u64>,
}

impl JobEdit {
    pub fn is_empty(&self) -> bool {
        self.schedule.is_none() && self.argv.is_none() && self.cwd.is_none() && self.timeout_secs.is_none()
    }
}

pub fn edit(conn: &Connection, id: &str, e: JobEdit, by: &str, now: DateTime<Utc>) -> Result<Job> {
    let cur = get(conn, id)?.ok_or_else(|| JobError::new("no_such_job", 404, "no such job"))?;
    if e.is_empty() {
        return Err(JobError::bad("nothing_to_change", "give --every/--cron, --timeout, --cwd or a new command after --"));
    }
    let argv = match e.argv {
        Some(a) => {
            validate_argv(&a)?;
            a
        }
        None => cur.argv.clone(),
    };
    let cwd = match e.cwd {
        Some(c) => validate_cwd(&c)?,
        None => cur.cwd.clone(),
    };
    let timeout = e.timeout_secs.unwrap_or(cur.timeout_secs);
    if timeout == 0 || timeout > MAX_TIMEOUT_SECS {
        return Err(JobError::bad("bad_timeout", "timeouts are 1 s to 6 h"));
    }
    let at = ts(now);
    let (schedule, anchor) = match e.schedule {
        Some(s) => (s, at.clone()),
        None => (cur.schedule.clone(), cur.anchor_at.clone()),
    };
    conn.execute(
        "UPDATE jobs SET schedule = ?2, argv_json = ?3, cwd = ?4, timeout_secs = ?5, updated_at = ?6, updated_by = ?7, anchor_at = ?8 WHERE id = ?1",
        params![id, schedule.to_db(), serde_json::to_string(&argv).expect("argv json"), cwd, timeout as i64, at, by, anchor],
    )
    .map_err(JobError::db)?;
    get(conn, id)?.ok_or_else(|| JobError::new("no_such_job", 404, "no such job"))
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Job>> {
    conn.query_row("SELECT * FROM jobs WHERE id = ?1", params![id], job_from_row)
        .optional()
        .map(|o| o.map(|(j, _)| j))
        .map_err(JobError::db)
}

pub fn get_by_name(conn: &Connection, workspace_id: &str, name: &str) -> Result<Option<Job>> {
    conn.query_row("SELECT * FROM jobs WHERE workspace_id = ?1 AND name = ?2", params![workspace_id, name], job_from_row)
        .optional()
        .map(|o| o.map(|(j, _)| j))
        .map_err(JobError::db)
}

/// Every job (or one workspace's), by workspace then name.
pub fn list(conn: &Connection, workspace_id: Option<&str>) -> Result<Vec<Job>> {
    let mut out = vec![];
    match workspace_id {
        Some(ws) => {
            let mut st = conn.prepare("SELECT * FROM jobs WHERE workspace_id = ?1 ORDER BY name").map_err(JobError::db)?;
            let rows = st.query_map(params![ws], job_from_row).map_err(JobError::db)?;
            for r in rows {
                out.push(r.map_err(JobError::db)?.0);
            }
        }
        None => {
            let mut st = conn.prepare("SELECT * FROM jobs ORDER BY workspace_id, name").map_err(JobError::db)?;
            let rows = st.query_map([], job_from_row).map_err(JobError::db)?;
            for r in rows {
                out.push(r.map_err(JobError::db)?.0);
            }
        }
    }
    Ok(out)
}

/// Enable re-anchors at now (no catch-up for slots missed while off).
pub fn set_enabled(conn: &Connection, id: &str, on: bool, by: &str, now: DateTime<Utc>) -> Result<Job> {
    let at = ts(now);
    let n = if on {
        conn.execute(
            "UPDATE jobs SET enabled = 1, updated_at = ?2, updated_by = ?3, anchor_at = ?2 WHERE id = ?1",
            params![id, at, by],
        )
    } else {
        conn.execute("UPDATE jobs SET enabled = 0, updated_at = ?2, updated_by = ?3 WHERE id = ?1", params![id, at, by])
    }
    .map_err(JobError::db)?;
    if n == 0 {
        return Err(JobError::new("no_such_job", 404, "no such job"));
    }
    get(conn, id)?.ok_or_else(|| JobError::new("no_such_job", 404, "no such job"))
}

pub fn set_anchor(conn: &Connection, id: &str, at: DateTime<Utc>) -> Result<()> {
    conn.execute("UPDATE jobs SET anchor_at = ?2 WHERE id = ?1", params![id, ts(at)]).map_err(JobError::db)?;
    Ok(())
}

/// Remove a job. Its run rows stay (the audit trail); returns their logs.
pub fn remove(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let logs = list_runs(conn, id, KEEP_RUN_ROWS)?.into_iter().filter_map(|r| r.log_path).collect();
    let n = conn.execute("DELETE FROM jobs WHERE id = ?1", params![id]).map_err(JobError::db)?;
    if n == 0 {
        return Err(JobError::new("no_such_job", 404, "no such job"));
    }
    conn.execute("UPDATE job_runs SET log_path = NULL WHERE job_id = ?1", params![id]).map_err(JobError::db)?;
    Ok(logs)
}

/// `k2 job pause --all` state: (paused, by, at).
pub fn paused(conn: &Connection) -> Result<(bool, String, Option<String>)> {
    conn.query_row("SELECT paused, paused_by, paused_at FROM job_settings WHERE id = 1", [], |r| {
        Ok((r.get::<_, i64>(0)? != 0, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))
    })
    .optional()
    .map(|o| o.unwrap_or((false, String::new(), None)))
    .map_err(JobError::db)
}

/// Pause or resume every job. Resume re-anchors every job at now.
pub fn set_paused(conn: &Connection, on: bool, by: &str, now: DateTime<Utc>) -> Result<()> {
    let at = ts(now);
    conn.execute(
        "INSERT INTO job_settings (id, paused, paused_by, paused_at) VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET paused = ?1, paused_by = ?2, paused_at = ?3",
        params![on as i64, by, at],
    )
    .map_err(JobError::db)?;
    if !on {
        conn.execute("UPDATE jobs SET anchor_at = ?1", params![at]).map_err(JobError::db)?;
    }
    Ok(())
}

// ── runs ────────────────────────────────────────────────────────────

/// Start a run row (`running`) or record a skipped slot.
#[allow(clippy::too_many_arguments)]
pub fn insert_run(
    conn: &Connection,
    id: &str,
    job_id: &str,
    outcome: &str,
    trigger: &str,
    log_path: Option<&str>,
    started_by: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    let at = ts(now);
    let ended: Option<&str> = if outcome == "running" { None } else { Some(at.as_str()) };
    conn.execute(
        "INSERT INTO job_runs (id, job_id, started_at, ended_at, outcome, trigger, log_path, started_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, job_id, at, ended, outcome, trigger, log_path, started_by],
    )
    .map_err(JobError::db)?;
    Ok(())
}

pub fn set_run_pid(conn: &Connection, id: &str, pid: i64) -> Result<()> {
    conn.execute("UPDATE job_runs SET pid = ?2 WHERE id = ?1", params![id, pid]).map_err(JobError::db)?;
    Ok(())
}

pub fn finish_run(conn: &Connection, id: &str, outcome: &str, exit_code: Option<i64>, bytes: u64, now: DateTime<Utc>) -> Result<()> {
    conn.execute(
        "UPDATE job_runs SET outcome = ?2, exit_code = ?3, bytes = ?4, ended_at = ?5 WHERE id = ?1",
        params![id, outcome, exit_code, bytes as i64, ts(now)],
    )
    .map_err(JobError::db)?;
    Ok(())
}

/// Newest first.
pub fn list_runs(conn: &Connection, job_id: &str, limit: usize) -> Result<Vec<Run>> {
    let mut st = conn
        .prepare("SELECT * FROM job_runs WHERE job_id = ?1 ORDER BY started_at DESC, rowid DESC LIMIT ?2")
        .map_err(JobError::db)?;
    let rows = st.query_map(params![job_id, limit as i64], run_from_row).map_err(JobError::db)?;
    let mut out = vec![];
    for r in rows {
        out.push(r.map_err(JobError::db)?);
    }
    Ok(out)
}

pub fn get_run(conn: &Connection, id: &str) -> Result<Option<Run>> {
    conn.query_row("SELECT * FROM job_runs WHERE id = ?1", params![id], run_from_row).optional().map_err(JobError::db)
}

/// Boot: a run the last daemon left `running` is `interrupted`. Returns
/// the rows (the daemon also stops their leftover process groups).
pub fn mark_interrupted(conn: &Connection, now: DateTime<Utc>) -> Result<Vec<Run>> {
    let mut st = conn.prepare("SELECT * FROM job_runs WHERE outcome = 'running'").map_err(JobError::db)?;
    let rows: Vec<Run> = st.query_map([], run_from_row).map_err(JobError::db)?.collect::<rusqlite::Result<_>>().map_err(JobError::db)?;
    conn.execute("UPDATE job_runs SET outcome = 'interrupted', ended_at = ?1 WHERE outcome = 'running'", params![ts(now)])
        .map_err(JobError::db)?;
    Ok(rows)
}

/// Keep the newest [`KEEP_LOGS`] logs and [`KEEP_RUN_ROWS`] rows of a job.
/// Returns the log files to delete.
pub fn prune_runs(conn: &Connection, job_id: &str) -> Result<Vec<String>> {
    let all = list_runs(conn, job_id, 100_000)?;
    let mut drop_logs = vec![];
    let mut with_logs = 0usize;
    for r in &all {
        if let Some(p) = &r.log_path {
            with_logs += 1;
            if with_logs > KEEP_LOGS {
                drop_logs.push(p.clone());
                conn.execute("UPDATE job_runs SET log_path = NULL WHERE id = ?1", params![r.id]).map_err(JobError::db)?;
            }
        }
    }
    for r in all.iter().skip(KEEP_RUN_ROWS) {
        if let Some(p) = &r.log_path {
            drop_logs.push(p.clone());
        }
        conn.execute("DELETE FROM job_runs WHERE id = ?1", params![r.id]).map_err(JobError::db)?;
    }
    Ok(drop_logs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn db() -> Connection {
        let conn = Connection::open(":memory:").expect("open");
        crate::db::run_migrations(&conn).expect("migrations");
        conn
    }

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 24, h, m, 0).single().expect("time")
    }

    fn new(ws: &str, name: &str) -> NewJob {
        NewJob {
            workspace_id: ws.into(),
            name: name.into(),
            schedule: Schedule::Every(900),
            argv: vec!["./collect.sh".into()],
            cwd: String::new(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            created_by: "owner".into(),
        }
    }

    #[test]
    fn durations_parse_and_refuse() {
        assert_eq!(Schedule::parse_every("15m").expect("15m"), Schedule::Every(900));
        assert_eq!(Schedule::parse_every("1h").expect("1h"), Schedule::Every(3600));
        assert_eq!(Schedule::parse_every("2d").expect("2d"), Schedule::Every(172_800));
        assert_eq!(Schedule::parse_every("90").expect("90"), Schedule::Every(90));
        assert_eq!(Schedule::parse_every("30s").expect_err("too often").code, "bad_schedule");
        assert_eq!(Schedule::parse_every("5x").expect_err("unit").code, "bad_schedule");
        assert_eq!(Schedule::parse_every("").expect_err("empty").code, "bad_schedule");
        assert_eq!(parse_timeout("10m").expect("10m"), 600);
        assert_eq!(parse_timeout("6h").expect("6h"), 21_600);
        assert_eq!(parse_timeout("7h").expect_err("max").code, "bad_timeout");
        assert_eq!(parse_timeout("0").expect_err("zero").code, "bad_timeout");
        assert_eq!(Schedule::Every(900).describe(), "every 15m");
        assert_eq!(Schedule::from_db("every:900").expect("db"), Schedule::Every(900));
    }

    #[test]
    fn cron_needs_five_fields() {
        assert_eq!(Schedule::parse_cron("*/15 * * * *").expect("ok"), Schedule::Cron("*/15 * * * *".into()));
        assert_eq!(Schedule::parse_cron("0 */15 * * * *").expect_err("six").code, "bad_schedule");
        assert_eq!(Schedule::parse_cron("nope").expect_err("bad").code, "bad_schedule");
    }

    #[test]
    fn every_fires_on_slots_and_coalesces_misses() {
        let s = Schedule::Every(900);
        assert_eq!(evaluate(&s, at(10, 0), at(10, 14)), Due::NotYet { next: Some(at(10, 15)) });
        assert_eq!(evaluate(&s, at(10, 0), at(10, 15)), Due::Fire { slot: at(10, 15), catchup: false });
        // Down for 2 h: the latest slot runs once, on time.
        assert_eq!(evaluate(&s, at(10, 0), at(12, 7)), Due::Fire { slot: at(12, 0), catchup: false });
        // A daily job missed by 20 h is skipped and audited.
        let d = Schedule::Every(86_400);
        let anchor = at(0, 0) - Duration::days(1) - Duration::hours(20);
        assert_eq!(evaluate(&d, anchor, at(0, 0)), Due::SkipMissed { slot: at(0, 0) - Duration::hours(20) });
        // Missed by 2 h: a catch-up.
        let anchor = at(9, 0) - Duration::days(1);
        assert_eq!(evaluate(&d, anchor, at(11, 0)), Due::Fire { slot: at(9, 0), catchup: true });
    }

    #[test]
    fn cron_fires_after_the_anchor_only() {
        let s = Schedule::parse_cron("*/15 * * * *").expect("cron");
        assert!(matches!(evaluate(&s, at(10, 0), at(10, 10)), Due::NotYet { .. }));
        assert_eq!(evaluate(&s, at(10, 0), at(10, 16)), Due::Fire { slot: at(10, 15), catchup: false });
        assert_eq!(evaluate(&s, at(10, 15), at(10, 16)), Due::NotYet { next: Some(at(10, 30)) });
        assert_eq!(evaluate(&s, at(8, 0), at(10, 31)), Due::Fire { slot: at(10, 30), catchup: false });
    }

    #[test]
    fn names_argv_and_cwd_are_checked() {
        validate_name("collect-ovh.v2").expect("ok");
        assert!(validate_name("Collect").is_err());
        assert!(validate_name("-x").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name(&"a".repeat(65)).is_err());
        assert_eq!(validate_argv(&[]).expect_err("empty").code, "command_required");
        assert_eq!(validate_cwd("scripts/").expect("rel"), "scripts");
        assert_eq!(validate_cwd(".").expect("dot"), "");
        assert_eq!(validate_cwd("../x").expect_err("up").code, "path_outside_workspace");
        assert_eq!(validate_cwd("/etc").expect_err("abs").code, "path_outside_workspace");
        assert_eq!(validate_cwd("a/../../b").expect_err("mid").code, "path_outside_workspace");
    }

    #[test]
    fn store_round_trip_enable_pause_and_runs() {
        let conn = db();
        let j = insert(&conn, new("ws-1", "collect"), at(10, 0)).expect("insert");
        assert_eq!(j.anchor_at, ts(at(10, 0)));
        assert!(j.enabled);
        assert_eq!(insert(&conn, new("ws-1", "collect"), at(10, 0)).expect_err("dup").code, "job_exists");
        insert(&conn, new("ws-2", "collect"), at(10, 0)).expect("same name, other workspace");
        assert_eq!(list(&conn, Some("ws-1")).expect("list").len(), 1);
        assert_eq!(list(&conn, None).expect("list all").len(), 2);

        let off = set_enabled(&conn, &j.id, false, "owner", at(11, 0)).expect("disable");
        assert!(!off.enabled);
        assert_eq!(off.anchor_at, ts(at(10, 0)), "disable keeps the anchor");
        let on = set_enabled(&conn, &j.id, true, "owner", at(12, 0)).expect("enable");
        assert_eq!(on.anchor_at, ts(at(12, 0)), "enable re-anchors: no catch-up");

        let e = edit(&conn, &j.id, JobEdit { timeout_secs: Some(30), ..Default::default() }, "agent:ws-1/s", at(12, 5)).expect("edit");
        assert_eq!(e.timeout_secs, 30);
        assert_eq!(e.anchor_at, ts(at(12, 0)), "no schedule change, same anchor");
        let e = edit(&conn, &j.id, JobEdit { schedule: Some(Schedule::Every(3600)), ..Default::default() }, "owner", at(12, 6)).expect("edit");
        assert_eq!(e.anchor_at, ts(at(12, 6)), "a schedule edit re-anchors");
        assert_eq!(e.updated_by, "owner");
        assert_eq!(edit(&conn, &j.id, JobEdit::default(), "owner", at(12, 7)).expect_err("empty").code, "nothing_to_change");

        assert!(!paused(&conn).expect("paused").0);
        set_paused(&conn, true, "owner", at(13, 0)).expect("pause");
        assert!(paused(&conn).expect("paused").0);
        set_paused(&conn, false, "owner", at(14, 0)).expect("resume");
        assert_eq!(get(&conn, &j.id).expect("get").expect("row").anchor_at, ts(at(14, 0)), "resume re-anchors");

        for i in 0..(KEEP_LOGS + 3) {
            let id = format!("r{i:03}");
            insert_run(&conn, &id, &j.id, "running", "schedule", Some(&format!("/logs/{id}.log")), "daemon", at(15, 0) + Duration::minutes(i as i64))
                .expect("run");
            finish_run(&conn, &id, "ok", Some(0), 12, at(15, 0) + Duration::minutes(i as i64)).expect("finish");
        }
        insert_run(&conn, "skip1", &j.id, "skipped_overlap", "schedule", None, "daemon", at(16, 0)).expect("skip row");
        let dropped = prune_runs(&conn, &j.id).expect("prune");
        assert_eq!(dropped.len(), 3, "{dropped:?}");
        assert!(dropped.contains(&"/logs/r000.log".to_string()));
        let runs = list_runs(&conn, &j.id, 5).expect("runs");
        assert_eq!(runs[0].id, "skip1");
        assert_eq!(runs[0].ended_at.as_deref(), Some(ts(at(16, 0)).as_str()), "a skipped slot ends at once");

        insert_run(&conn, "live", &j.id, "running", "manual", None, "owner", at(17, 0)).expect("live");
        let stuck = mark_interrupted(&conn, at(18, 0)).expect("boot");
        assert_eq!(stuck.len(), 1);
        assert_eq!(get_run(&conn, "live").expect("get").expect("row").outcome, "interrupted");

        let logs = remove(&conn, &j.id).expect("remove");
        assert_eq!(logs.len(), KEEP_LOGS);
        assert!(get(&conn, &j.id).expect("get").is_none());
        assert!(!list_runs(&conn, &j.id, 500).expect("rows kept").is_empty(), "the audit trail stays");
        assert_eq!(remove(&conn, &j.id).expect_err("gone").code, "no_such_job");
    }
}
