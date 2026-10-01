//! S5 — creation traps (`.k2/prds/prd-heartbeat-firing-v1.md` HB33,
//! HB35, Rosson decision D4).
//!
//! - **Empty is not missing (HB33).** A WAKEUP.md that exists with an
//!   empty body (the Settings/CLI scaffold before anyone writes
//!   instructions) never auto-disables and never counts a failure. The
//!   row stays enabled and waits with `wait_reason = wakeup_empty`.
//!   `wakeup_missing` stays for a file that is really gone.
//! - **No surprise catch-up (D4).** Instructions added later wait for
//!   the next scheduled slot. The tick records the episode in the audit
//!   trail: one `wakeup_empty` row when a due slot finds the body
//!   empty, then one `wakeup_added` row on the first due tick that finds
//!   a body. From that row on, the row's schedule counts from the moment
//!   the instructions were seen, so slots that passed while it was empty
//!   are skipped, not replayed. The first real fire closes the episode.
//! - **Flag bad rows (HB35).** [`boot_reconcile`] flags every enabled
//!   row whose schedule can never fire, and relabels old
//!   `wakeup_missing` auto-disables whose file is actually just empty.
//!
//!
//! S3 — the daemon owns next-fire (HB1, HB4, HB18–HB24). Every
//! non-archived row stores `next_fire_at` (UTC RFC3339) and a
//! `wait_reason` from one fixed vocabulary ([`ALL_REASONS`], S5's names
//! included). The stored columns are the single source; the list routes
//! read them and only overlay the read-time `no_ticks` (HB21).
//!
//! - [`derive`] is pure: row + workspace facts + `now` → the wait state.
//!   Due-ness comes from `cron::evaluate_with_now` (S2's anchor and 12 h
//!   catch-up window included), so there is no second copy of the rules.
//! - [`refresh_project`] is the one writer. It re-derives every
//!   non-archived row of a workspace, writes only what changed, and runs
//!   the overdue watchdog (HB22): an enabled row more than
//!   [`OVERDUE_AFTER_SECS`] past `next_fire_at` with no lease gets
//!   `overdue` (unless a more specific reason is set) and exactly one
//!   `overdue` audit row per episode, naming the gate that blocked it.
//!   A change calls the listener the daemon registers ([`set_change_listener`]),
//!   which emits `heartbeat_roster_changed` (HB23).

use std::path::Path;
use std::sync::OnceLock;

use chrono::{DateTime, Local, SecondsFormat, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::cron::{self, DueStatus};
use crate::db::schema::{AgentHeartbeat, HeartbeatFire, SchedulerMeta};

/// HB20 `wait_reason`: the WAKEUP.md body is empty.
pub const WAIT_WAKEUP_EMPTY: &str = "wakeup_empty";
/// HB20 `wait_reason`: the schedule can never fire.
pub const WAIT_SCHEDULE_ERROR: &str = "schedule_error";
/// `wait_detail` for [`WAIT_WAKEUP_EMPTY`].
pub const WAKEUP_EMPTY_DETAIL: &str =
    "needs instructions: WAKEUP.md is empty. It fires at the next scheduled slot after you add them.";

/// Audit decision: a due slot found the body empty (opens the episode).
pub const DECISION_WAKEUP_EMPTY: &str = "wakeup_empty";
/// Audit decision: the body was seen again; wait for the next slot.
pub const DECISION_WAKEUP_ADDED: &str = "wakeup_added";

/// `disabled_reason` for an old auto-disable whose file is just empty.
pub const DISABLED_WAKEUP_EMPTY: &str = "wakeup_empty";
/// `disabled_reason` for a WAKEUP.md that does not exist.
pub const DISABLED_WAKEUP_MISSING: &str = "wakeup_missing";

/// What is on disk at a heartbeat's WAKEUP.md path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeupBody {
    /// No file.
    Missing,
    /// The file exists; with frontmatter stripped, nothing is left.
    Empty,
    /// The file has instructions (or could not be read — the launcher
    /// reports an unreadable file itself, as before S5).
    Present,
}

/// Classify the WAKEUP.md at `path`. Same emptiness test the launcher's
/// composer uses (`compose_agent_wake_from_body`), so the tick and the
/// fire path can never disagree about "empty".
pub fn wakeup_body(path: &Path) -> WakeupBody {
    if !path.exists() {
        return WakeupBody::Missing;
    }
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            if crate::workspace::wake_prompts::compose_agent_wake_from_body(Some(&raw)).is_none() {
                WakeupBody::Empty
            } else {
                WakeupBody::Present
            }
        }
        Err(_) => WakeupBody::Present,
    }
}

/// The open empty-WAKEUP episode for a heartbeat, from its audit rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeupEpisode {
    /// A due slot found the body empty; nothing fired since.
    Empty,
    /// Instructions were seen at this time; nothing fired since. The
    /// schedule counts from here.
    Added(DateTime<Local>),
}

/// Latest `wakeup_empty` / `wakeup_added` audit row for this heartbeat,
/// if it is newer than `last_fired`. A fire after the marker closes the
/// episode, so the marker no longer applies.
pub fn open_episode(
    conn: &Connection,
    project_id: &str,
    name: &str,
    last_fired: Option<&str>,
) -> Option<WakeupEpisode> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT decision, fired_at FROM heartbeat_fires \
             WHERE project_id = ?1 AND schedule_name = ?2 AND decision IN (?3, ?4) \
             ORDER BY id DESC LIMIT 1",
            params![project_id, name, DECISION_WAKEUP_EMPTY, DECISION_WAKEUP_ADDED],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten();
    let (decision, at) = row?;
    let at = DateTime::parse_from_rfc3339(&at).ok()?.with_timezone(&Local);
    if let Some(fired) = last_fired.and_then(|s| DateTime::parse_from_rfc3339(s).ok()) {
        if fired >= at {
            return None;
        }
    }
    if decision == DECISION_WAKEUP_ADDED {
        Some(WakeupEpisode::Added(at))
    } else {
        Some(WakeupEpisode::Empty)
    }
}

/// The row as the evaluator should see it while an `Added` episode is
/// open: the reference point moves to when the instructions were seen,
/// so the next occurrence is the first slot after that (D4).
pub fn with_reference(hb: &AgentHeartbeat, at: DateTime<Local>) -> AgentHeartbeat {
    let mut shifted = hb.clone();
    shifted.last_fired = Some(at.to_rfc3339());
    shifted
}

// ── S3: the HB20 vocabulary ──────────────────────────────────────────
// A new value needs a PRD amendment. Keep `ALL_REASONS` and
// `fixtures/wait-reasons.json` in step (test `vocabulary_fixture_matches`).
// S5's two names (`WAIT_WAKEUP_EMPTY`, `WAIT_SCHEDULE_ERROR`) are part of it.

pub const WAIT_SCHEDULED: &str = "scheduled";
pub const WAIT_IN_FLIGHT: &str = "in_flight";
pub const WAIT_WINDOW_CLOSED: &str = "window_closed";
pub const WAIT_BACKOFF: &str = "backoff";
pub const WAIT_NO_AGENT: &str = "no_agent";
pub const WAIT_NO_PROJECT: &str = "no_project";
pub const WAIT_DISABLED_USER: &str = "disabled:user";
pub const WAIT_DISABLED_FAILURES: &str = "disabled:failures";
pub const WAIT_DISABLED_WAKEUP_MISSING: &str = "disabled:wakeup_missing";
pub const WAIT_DISABLED_WAKEUP_EMPTY: &str = "disabled:wakeup_empty";
pub const WAIT_OVERDUE: &str = "overdue";
/// Read-time only (HB21). Never written to the row.
pub const WAIT_NO_TICKS: &str = "no_ticks";

pub const ALL_REASONS: &[&str] = &[
    WAIT_SCHEDULED,
    WAIT_IN_FLIGHT,
    WAIT_WINDOW_CLOSED,
    WAIT_BACKOFF,
    WAIT_SCHEDULE_ERROR,
    WAIT_WAKEUP_EMPTY,
    WAIT_NO_AGENT,
    WAIT_NO_PROJECT,
    WAIT_DISABLED_USER,
    WAIT_DISABLED_FAILURES,
    WAIT_DISABLED_WAKEUP_MISSING,
    WAIT_DISABLED_WAKEUP_EMPTY,
    WAIT_OVERDUE,
    WAIT_NO_TICKS,
];

/// HB4 / HB22: an enabled row this far past `next_fire_at` is overdue.
pub const OVERDUE_AFTER_SECS: i64 = 120;
/// HB21: no daemon tick (S2's ticker, `last_daemon_tick_at`) for this
/// long while a row is overdue → the list routes say `no_ticks`.
pub const NO_TICKS_AFTER_SECS: i64 = 180;
/// Display only ("failure 2 of 5"). The daemon's auto-disable threshold
/// is `heartbeat_launch::MAX_CONSECUTIVE_FAILURES`; keep them equal.
pub const MAX_CONSECUTIVE_FAILURES: i64 = 5;

/// Facts about the workspace the pure derivation needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitInputs {
    pub project_dir_exists: bool,
    pub agent_resolvable: bool,
    pub wakeup: WakeupBody,
}

/// One row's derived wait state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitState {
    pub next_fire_at: Option<String>,
    pub reason: String,
    pub detail: Option<String>,
}

impl WaitState {
    fn new(next: Option<DateTime<Local>>, reason: &str, detail: Option<String>) -> Self {
        WaitState { next_fire_at: next.map(fmt_utc), reason: reason.to_string(), detail }
    }
}

/// UTC RFC3339 with whole seconds and a `Z` — the stored form. Stable
/// across passes, so an unchanged schedule never rewrites the row.
pub fn fmt_utc<Tz: TimeZone>(t: DateTime<Tz>) -> String {
    t.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn parse_utc(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&Utc))
}

/// HB19 — derive a row's wait state. Pure: no DB, no IO. `hb` is the row
/// as the evaluator should see it (an open S5 `wakeup_added` episode
/// already shifted by [`with_reference`]).
///
/// Precedence (first match wins): disabled → invalid schedule → project
/// folder gone → lease held → empty WAKEUP.md → no agent → failure
/// backoff → the evaluator's own answer (scheduled / window closed).
/// Archived rows are not derived (the caller skips them).
pub fn derive(hb: &AgentHeartbeat, inputs: &WaitInputs, now: DateTime<Local>) -> WaitState {
    if !hb.enabled {
        let (reason, detail) = match hb.disabled_reason.as_deref() {
            None => (WAIT_DISABLED_USER, None),
            Some("failures") => (
                WAIT_DISABLED_FAILURES,
                Some(format!("{} consecutive failed fires", hb.consecutive_failures)),
            ),
            Some(DISABLED_WAKEUP_MISSING) => {
                (WAIT_DISABLED_WAKEUP_MISSING, Some(hb.wakeup_path.clone()))
            }
            Some(DISABLED_WAKEUP_EMPTY) => (WAIT_DISABLED_WAKEUP_EMPTY, Some(hb.wakeup_path.clone())),
            // A system reason this vocabulary has no slot for: keep the
            // fixed reason, carry the raw value as detail.
            Some(other) => (WAIT_DISABLED_USER, Some(other.to_string())),
        };
        return WaitState::new(None, reason, detail);
    }

    // The evaluator's slot: the next (or first missed) occurrence.
    let (slot, base_reason, base_detail): (Option<DateTime<Local>>, &str, Option<String>) =
        match cron::evaluate_with_now(hb, now) {
            DueStatus::Invalid { reason } => {
                return WaitState::new(None, WAIT_SCHEDULE_ERROR, Some(reason));
            }
            // Due: store the FIRST slot after the reference (when it
            // should have fired), not the latest occurrence, so the
            // overdue clock does not slide forward each interval.
            DueStatus::Due { scheduled_for } => (
                Some(cron::first_slot_after_reference(hb).unwrap_or(scheduled_for)),
                WAIT_SCHEDULED,
                None,
            ),
            DueStatus::DueCatchUp { missed_at } => (
                Some(cron::first_slot_after_reference(hb).unwrap_or(missed_at)),
                WAIT_SCHEDULED,
                Some("missed slot; the next tick fires one catch-up".to_string()),
            ),
            // S2 owns the window-open and 12 h-skip math.
            DueStatus::HoldWindow { .. } => {
                let opens = cron::next_fire_estimate(hb, now);
                (
                    opens,
                    WAIT_WINDOW_CLOSED,
                    opens.map(|t| format!("window opens {}", t.format("%H:%M"))),
                )
            }
            DueStatus::SkippedMissed { .. } => (
                cron::next_fire_estimate(hb, now),
                WAIT_SCHEDULED,
                Some("missed slot older than 12 h is skipped".to_string()),
            ),
            DueStatus::NotYet { next: Some(next) } => (Some(next), WAIT_SCHEDULED, None),
            DueStatus::NotYet { next: None } => {
                return WaitState::new(
                    None,
                    WAIT_SCHEDULE_ERROR,
                    Some("no upcoming slot falls inside the firing window".to_string()),
                );
            }
        };

    if !inputs.project_dir_exists {
        return WaitState::new(slot, WAIT_NO_PROJECT, Some("workspace folder not found".to_string()));
    }
    if hb.in_flight_started_at.is_some() {
        return WaitState::new(slot, WAIT_IN_FLIGHT, None);
    }
    // S5 HB33: the row's own missing instructions come before the
    // workspace-wide gates — it is what the user can fix on this row.
    // D4: once its slot has passed while empty, the next fire is the
    // first slot after instructions are seen — not known yet, so no time
    // is stored (and an empty row is a designed wait, never `overdue`;
    // S5's one `wakeup_empty` audit row is the episode's record).
    if inputs.wakeup == WakeupBody::Empty {
        let next = slot.filter(|s| *s > now);
        return WaitState::new(next, WAIT_WAKEUP_EMPTY, Some(WAKEUP_EMPTY_DETAIL.to_string()));
    }
    if !inputs.agent_resolvable {
        return WaitState::new(
            slot,
            WAIT_NO_AGENT,
            Some("no scheduleable agent in this workspace".to_string()),
        );
    }
    if let Some(retry_at) = hb.next_retry_at.as_deref().and_then(parse_utc) {
        let retry_local = retry_at.with_timezone(&Local);
        if retry_local > now && slot.map_or(true, |s| retry_local >= s) {
            return WaitState::new(
                Some(retry_local),
                WAIT_BACKOFF,
                Some(format!(
                    "failure {} of {}",
                    hb.consecutive_failures, MAX_CONSECUTIVE_FAILURES
                )),
            );
        }
    }
    WaitState::new(slot, base_reason, base_detail)
}

/// The daemon's own tick — S2's ticker (`ticker.lastTickAt` on
/// `/cli/heartbeat/scheduler-status`, stored as `last_daemon_tick_at`).
pub fn last_tick_at(conn: &Connection) -> Option<DateTime<Utc>> {
    SchedulerMeta::get(conn, SchedulerMeta::LAST_DAEMON_TICK_AT).and_then(|s| parse_utc(&s))
}

fn ticks_stale(last_tick: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    last_tick.map_or(true, |t| (now - t).num_seconds() > NO_TICKS_AFTER_SECS)
}

fn is_overdue(next_fire_at: Option<&str>, now: DateTime<Utc>) -> bool {
    next_fire_at
        .and_then(parse_utc)
        .map_or(false, |t| (now - t).num_seconds() > OVERDUE_AFTER_SECS)
}

/// HB22 — the gate that kept an overdue row from firing, for the audit
/// row and the `overdue` detail. Stable across passes (no durations), so
/// an unchanged episode never rewrites the row.
fn overdue_gate(state: &WaitState, last_tick: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    if state.reason != WAIT_SCHEDULED {
        return match &state.detail {
            Some(d) => format!("{}: {d}", state.reason),
            None => state.reason.clone(),
        };
    }
    if ticks_stale(last_tick, now) {
        return match last_tick {
            Some(t) => format!("{WAIT_NO_TICKS}: no scheduler tick since {}", fmt_utc(t)),
            None => format!("{WAIT_NO_TICKS}: no scheduler tick recorded"),
        };
    }
    "not_fired: the scheduler is ticking but has not fired this slot".to_string()
}

/// HB23 — called with the workspace path whenever [`refresh_project`]
/// changed a row. The daemon registers one that emits
/// `heartbeat_roster_changed`; k2-core alone has no event bus.
static CHANGE_LISTENER: OnceLock<fn(&str)> = OnceLock::new();

/// Register the change listener (first call wins; later calls no-op).
pub fn set_change_listener(f: fn(&str)) {
    let _ = CHANGE_LISTENER.set(f);
}

/// HB19 + HB22 — re-derive and store every non-archived heartbeat of a
/// workspace, run the overdue watchdog, and write the one `overdue`
/// audit row per episode. Returns `true` when any row's `next_fire_at`,
/// `wait_reason` or `wait_detail` changed (and then calls the change
/// listener, HB23). Unknown workspace → `Ok(false)`.
pub fn refresh_project(project_path: &str) -> Result<bool, String> {
    refresh_project_at(project_path, Local::now())
}

/// [`refresh_project`] with an explicit clock (tests).
pub fn refresh_project_at(project_path: &str, now: DateTime<Local>) -> Result<bool, String> {
    let changed = refresh_rows(project_path, now)?;
    if changed {
        if let Some(f) = CHANGE_LISTENER.get() {
            f(project_path);
        }
    }
    Ok(changed)
}

fn refresh_rows(project_path: &str, now: DateTime<Local>) -> Result<bool, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(project_id) =
        crate::workspace::agent_identity::resolve_project_id(&conn, project_path)
    else {
        return Ok(false);
    };
    let rows = AgentHeartbeat::list_active(&conn, &project_id).map_err(|e| e.to_string())?;
    if rows.is_empty() {
        return Ok(false);
    }
    let project_dir_exists = Path::new(project_path).is_dir();
    let agent_name = crate::workspace::agent_identity::resolve_agent_name(project_path);
    let now_utc = now.with_timezone(&Utc);
    let now_s = fmt_utc(now);
    let last_tick = last_tick_at(&conn);

    let mut changed = false;
    for hb in rows {
        let wakeup = wakeup_body(&Path::new(project_path).join(&hb.wakeup_path));
        // S5 D4: while a `wakeup_added` episode is open the schedule
        // counts from when the instructions were seen. A body that
        // appeared since the last tick (episode still `Empty`) will be
        // seen now: its next fire is the first slot after now.
        let seen = match open_episode(&conn, &project_id, &hb.name, hb.last_fired.as_deref()) {
            Some(WakeupEpisode::Added(at)) => Some(at),
            Some(WakeupEpisode::Empty) if wakeup == WakeupBody::Present => Some(now),
            _ => None,
        };
        let eval_hb = match seen {
            Some(at) => with_reference(&hb, at),
            None => hb.clone(),
        };
        let inputs = WaitInputs {
            project_dir_exists,
            agent_resolvable: agent_name.is_some(),
            wakeup,
        };
        let mut state = derive(&eval_hb, &inputs, now);

        let overdue = hb.enabled
            && hb.in_flight_started_at.is_none()
            && is_overdue(state.next_fire_at.as_deref(), now_utc);
        let gate = overdue.then(|| overdue_gate(&state, last_tick, now_utc));
        if let (Some(gate), true) = (&gate, state.reason == WAIT_SCHEDULED) {
            // HB22: `overdue` only when no more specific reason is set.
            state.reason = WAIT_OVERDUE.to_string();
            state.detail = Some(gate.clone());
        }

        changed |= AgentHeartbeat::set_wait_state(
            &conn,
            &project_id,
            &hb.name,
            state.next_fire_at.as_deref(),
            &state.reason,
            state.detail.as_deref(),
            &now_s,
        )
        .map_err(|e| e.to_string())?;

        match gate {
            Some(gate) => {
                let opened = AgentHeartbeat::open_overdue_episode(&conn, &project_id, &hb.name, &now_s)
                    .map_err(|e| e.to_string())?;
                if opened {
                    HeartbeatFire::insert_with_schedule(
                        &conn,
                        &project_id,
                        agent_name.as_deref(),
                        Some(&hb.name),
                        &hb.frequency,
                        WAIT_OVERDUE,
                        Some(&format!(
                            "next fire {} not delivered; blocked by gate {gate}",
                            state.next_fire_at.as_deref().unwrap_or("?"),
                        )),
                        None,
                        None,
                        None,
                    )
                    .map_err(|e| e.to_string())?;
                    crate::log_debug!("[heartbeat-wait] {} overdue — gate {}", hb.name, gate);
                }
            }
            None => {
                AgentHeartbeat::close_overdue_episode(&conn, &project_id, &hb.name)
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(changed)
}

/// Every workspace with at least one non-archived heartbeat. The
/// daemon's wait pass refreshes each (boot fill, HB18, and the
/// watchdog, HB22).
pub fn projects_with_heartbeats() -> Vec<String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = match conn.prepare(
        "SELECT DISTINCT p.path FROM workspace_heartbeats h \
         JOIN projects p ON h.project_id = p.id \
         WHERE h.archived_at IS NULL ORDER BY p.path",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = match stmt.query_map([], |r| r.get::<_, String>(0)) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    rows.filter_map(|r| r.ok()).collect()
}

/// HB21 — read-time `no_ticks`. An enabled row whose stored
/// `next_fire_at` is more than [`OVERDUE_AFTER_SECS`] past, while the
/// daemon ticker has not ticked for [`NO_TICKS_AFTER_SECS`], reads
/// `no_ticks` with `wait_since` = the last tick (None when none was ever
/// recorded). Only the returned rows change; the stored row is untouched.
pub fn overlay_no_ticks(
    rows: &mut [AgentHeartbeat],
    last_tick: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) {
    if !ticks_stale(last_tick, now) {
        return;
    }
    for hb in rows.iter_mut() {
        let disabled_or_error = hb
            .wait_reason
            .as_deref()
            .map_or(false, |r| r.starts_with("disabled:") || r == WAIT_SCHEDULE_ERROR);
        if hb.enabled
            && hb.archived_at.is_none()
            && !disabled_or_error
            && is_overdue(hb.next_fire_at.as_deref(), now)
        {
            hb.wait_reason = Some(WAIT_NO_TICKS.to_string());
            hb.wait_detail = Some(match last_tick {
                Some(t) => format!("no scheduler tick since {}", fmt_utc(t)),
                None => "no scheduler tick recorded".to_string(),
            });
            hb.wait_since = last_tick.map(fmt_utc);
        }
    }
}

/// [`overlay_no_ticks`] against the live clock and the stored ticker
/// stamp — what the list routes call.
pub fn overlay_no_ticks_now(conn: &Connection, rows: &mut [AgentHeartbeat]) {
    overlay_no_ticks(rows, last_tick_at(conn), Utc::now());
}

/// What [`boot_reconcile`] changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BootReconcile {
    /// Disabled `wakeup_missing` rows whose file exists but is empty,
    /// now `wakeup_empty`. They stay disabled.
    pub relabelled: Vec<String>,
    /// Enabled rows whose schedule can never fire, newly flagged with
    /// `schedule_error` (rows already carrying the same error are not
    /// counted and get no second audit row). Not disabled, not deleted.
    pub flagged: Vec<String>,
}

/// Daemon boot pass (HB33 relabel, HB35 flag). Reads every non-archived
/// row in every workspace — including workspaces the tick skips because
/// no agent name resolves, which is where an invalid row could sit
/// unflagged forever.
pub fn boot_reconcile() -> Result<BootReconcile, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    boot_reconcile_with(&conn)
}

/// [`boot_reconcile`] on a given connection (tests).
pub fn boot_reconcile_with(conn: &Connection) -> Result<BootReconcile, String> {
    let rows = AgentHeartbeat::list_all_active_with_project(conn)
        .map_err(|e| format!("list heartbeats: {e}"))?;
    let mut out = BootReconcile::default();
    for (hb, _project_name, project_path) in rows {
        let label = format!("{project_path}::{}", hb.name);
        if !hb.enabled && hb.disabled_reason.as_deref() == Some(DISABLED_WAKEUP_MISSING) {
            let abs = Path::new(&project_path).join(&hb.wakeup_path);
            if wakeup_body(&abs) == WakeupBody::Empty {
                AgentHeartbeat::auto_disable(conn, &hb.project_id, &hb.name, DISABLED_WAKEUP_EMPTY)
                    .map_err(|e| format!("relabel {label}: {e}"))?;
                out.relabelled.push(label.clone());
            }
        }
        if hb.enabled {
            if let cron::DueStatus::Invalid { reason } = cron::evaluate(&hb) {
                if hb.schedule_error.as_deref() != Some(reason.as_str()) {
                    AgentHeartbeat::set_schedule_error(conn, &hb.project_id, &hb.name, Some(&reason))
                        .map_err(|e| format!("flag {label}: {e}"))?;
                    HeartbeatFire::insert_with_schedule(
                        conn,
                        &hb.project_id,
                        None,
                        Some(&hb.name),
                        &hb.frequency,
                        "schedule_invalid",
                        Some(&reason),
                        None,
                        None,
                        None,
                    )
                    .map_err(|e| format!("audit {label}: {e}"))?;
                    out.flagged.push(label);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    //! S5 tick + boot behaviour. Rows are seeded straight into the
    //! shared test DB (no add → no scheduler install path at all).

    use super::*;
    use crate::heartbeats::k2so_agents_heartbeat_tick;

    const SCAFFOLD: &str = "---\ndescription:\n---\n\n";

    struct Ws {
        dir: std::path::PathBuf,
        path: String,
        project_id: String,
    }

    impl Drop for Ws {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A registered workspace with a configured agent (so the tick
    /// resolves an agent name and actually evaluates its rows).
    fn workspace(label: &str) -> Ws {
        crate::db::init_for_tests();
        let dir = std::env::temp_dir().join(format!(
            "k2-hb-s5-wait-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().to_string();
        let project_id = uuid::Uuid::new_v4().to_string();
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, agent_enabled) VALUES (?1, 's5-wait', ?2, 1)",
            rusqlite::params![project_id, path],
        )
        .unwrap();
        Ws { dir, path, project_id }
    }

    /// Insert an enabled row whose reference point is `age_secs` ago,
    /// with WAKEUP.md written as `body` (None = no file).
    fn seed(
        ws: &Ws,
        name: &str,
        frequency: &str,
        spec: &str,
        age_secs: i64,
        body: Option<&str>,
    ) -> std::path::PathBuf {
        let rel = format!(".k2/heartbeats/{name}/WAKEUP.md");
        let abs = ws.dir.join(&rel);
        if let Some(body) = body {
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::fs::write(&abs, body).unwrap();
        }
        let db = crate::db::shared();
        let conn = db.lock();
        AgentHeartbeat::insert(
            &conn,
            &uuid::Uuid::new_v4().to_string(),
            &ws.project_id,
            name,
            frequency,
            spec,
            &rel,
            true,
        )
        .unwrap();
        conn.execute(
            "UPDATE workspace_heartbeats SET created_at = unixepoch() - ?1 \
             WHERE project_id = ?2 AND name = ?3",
            rusqlite::params![age_secs, ws.project_id, name],
        )
        .unwrap();
        abs
    }

    fn row(ws: &Ws, name: &str) -> AgentHeartbeat {
        let db = crate::db::shared();
        let conn = db.lock();
        AgentHeartbeat::get_by_name(&conn, &ws.project_id, name)
            .unwrap()
            .expect("row exists")
    }

    fn decisions(ws: &Ws, name: &str) -> Vec<String> {
        let db = crate::db::shared();
        let conn = db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT decision FROM heartbeat_fires WHERE project_id = ?1 AND schedule_name = ?2 \
                 ORDER BY fired_at, id",
            )
            .unwrap();
        let out = stmt
            .query_map(rusqlite::params![ws.project_id, name], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        out
    }

    fn candidate_names(ws: &Ws) -> Vec<String> {
        k2so_agents_heartbeat_tick(&ws.path).into_iter().map(|c| c.name).collect()
    }

    fn listed(ws: &Ws, name: &str) -> AgentHeartbeat {
        crate::heartbeats::k2so_heartbeat_list(ws.path.clone())
            .expect("list")
            .into_iter()
            .find(|r| r.name == name)
            .expect("row listed")
    }

    fn assert_waiting_enabled(ws: &Ws, name: &str) {
        let hb = row(ws, name);
        assert!(hb.enabled, "{name}: an empty WAKEUP.md must not disable the row");
        assert_eq!(hb.disabled_reason, None, "{name}: no disabled_reason");
        assert_eq!(hb.consecutive_failures, 0, "{name}: empty is not a failure");
        assert_eq!(hb.next_retry_at, None, "{name}: empty sets no backoff");
    }

    /// T8 (HB33 + D4), hourly. Due + empty: waits enabled, one audit row
    /// per episode. Body written: no immediate fire. First slot after
    /// the body was seen: fires on time (not as a catch-up).
    #[test]
    fn empty_wakeup_waits_then_fires_at_the_next_slot_not_at_once() {
        let ws = workspace("hourly");
        let abs = seed(&ws, "minutely", "hourly", r#"{"every_seconds":60}"#, 630, Some(SCAFFOLD));

        assert_eq!(candidate_names(&ws), Vec::<String>::new(), "empty body must not be a candidate");
        assert_eq!(candidate_names(&ws), Vec::<String>::new());
        assert_waiting_enabled(&ws, "minutely");
        assert_eq!(
            decisions(&ws, "minutely"),
            vec![DECISION_WAKEUP_EMPTY.to_string()],
            "exactly one wakeup_empty audit row per episode"
        );
        assert_eq!(listed(&ws, "minutely").wait_reason.as_deref(), Some(WAIT_WAKEUP_EMPTY));

        std::fs::write(&abs, format!("{SCAFFOLD}check inbox\n")).unwrap();
        assert_eq!(
            candidate_names(&ws),
            Vec::<String>::new(),
            "instructions added after the slot must not fire at once (D4)"
        );
        assert_eq!(
            candidate_names(&ws),
            Vec::<String>::new(),
            "still waiting for the first slot after the body was seen"
        );
        assert_eq!(
            decisions(&ws, "minutely"),
            vec![DECISION_WAKEUP_EMPTY.to_string(), DECISION_WAKEUP_ADDED.to_string()]
        );
        assert_waiting_enabled(&ws, "minutely");
        // S3: the stored reason moves from `wakeup_empty` to `scheduled`,
        // and the next fire is the first slot after the body was seen (D4).
        let after = listed(&ws, "minutely");
        assert_eq!(after.wait_reason.as_deref(), Some(WAIT_SCHEDULED), "a body clears the empty wait");
        let next = DateTime::parse_from_rfc3339(after.next_fire_at.as_deref().expect("next fire"))
            .expect("RFC3339");
        assert!(next > chrono::Utc::now(), "no catch-up: the next fire is ahead");

        // Next slot: age both markers (order kept), so the body was seen
        // 61s ago and the first slot after it has just passed.
        {
            let db = crate::db::shared();
            let conn = db.lock();
            for (decision, secs) in [(DECISION_WAKEUP_EMPTY, 121), (DECISION_WAKEUP_ADDED, 61)] {
                let back = (chrono::Local::now() - chrono::Duration::seconds(secs)).to_rfc3339();
                let n = conn
                    .execute(
                        "UPDATE heartbeat_fires SET fired_at = ?1 \
                         WHERE project_id = ?2 AND schedule_name = 'minutely' AND decision = ?3",
                        rusqlite::params![back, ws.project_id, decision],
                    )
                    .unwrap();
                assert_eq!(n, 1, "{decision}");
            }
        }
        let fired = k2so_agents_heartbeat_tick(&ws.path);
        assert_eq!(fired.len(), 1, "fires at the first slot after the body was seen");
        assert_eq!(fired[0].name, "minutely");
        assert_eq!(fired[0].catchup_of, None, "the next slot is on time, not a catch-up");

        // A fire closes the episode: the marker no longer applies.
        crate::heartbeats::stamp_heartbeat_fired(&ws.path, "minutely");
        let hb = row(&ws, "minutely");
        let db = crate::db::shared();
        let conn = db.lock();
        assert_eq!(open_episode(&conn, &ws.project_id, "minutely", hb.last_fired.as_deref()), None);
    }

    /// T8 (D4), daily: a slot missed while empty is never replayed.
    #[test]
    fn daily_slot_missed_while_empty_is_not_caught_up() {
        let ws = workspace("daily");
        let two_hours_ago = (chrono::Local::now() - chrono::Duration::hours(2))
            .format("%H:%M")
            .to_string();
        let spec = format!(r#"{{"time":"{two_hours_ago}"}}"#);
        let abs = seed(&ws, "morning", "daily", &spec, 3 * 86_400, Some(SCAFFOLD));

        assert_eq!(candidate_names(&ws), Vec::<String>::new());
        assert_waiting_enabled(&ws, "morning");
        std::fs::write(&abs, "do the morning brief\n").unwrap();
        for _ in 0..3 {
            assert_eq!(
                candidate_names(&ws),
                Vec::<String>::new(),
                "today's slot passed while empty; it waits for tomorrow's"
            );
        }
        assert_eq!(
            decisions(&ws, "morning"),
            vec![DECISION_WAKEUP_EMPTY.to_string(), DECISION_WAKEUP_ADDED.to_string()]
        );
        assert_waiting_enabled(&ws, "morning");
    }

    /// A missing file keeps today's behaviour: auto-disable as
    /// `wakeup_missing`.
    #[test]
    fn missing_wakeup_still_disables_as_missing() {
        let ws = workspace("missing");
        seed(&ws, "gone", "hourly", r#"{"every_seconds":60}"#, 630, None);
        assert_eq!(candidate_names(&ws), Vec::<String>::new());
        let hb = row(&ws, "gone");
        assert!(!hb.enabled);
        assert_eq!(hb.disabled_reason.as_deref(), Some(DISABLED_WAKEUP_MISSING));
    }

    /// A body present from the start fires on its slot (no episode).
    #[test]
    fn body_from_the_start_fires_normally() {
        let ws = workspace("normal");
        seed(&ws, "ready", "hourly", r#"{"every_seconds":60}"#, 90, Some("check inbox\n"));
        let fired = k2so_agents_heartbeat_tick(&ws.path);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].name, "ready");
        assert_eq!(decisions(&ws, "ready"), Vec::<String>::new(), "no wait markers");
    }

    /// T-S5e (HB33) + HB35: boot relabels empty-file auto-disables
    /// (still disabled), leaves truly missing ones, and flags invalid
    /// schedules once without disabling, rewriting or deleting them.
    #[test]
    fn boot_reconcile_relabels_empty_and_flags_invalid_once() {
        let ws = workspace("boot");
        seed(&ws, "was-empty", "daily", "{}", 60, Some(SCAFFOLD));
        seed(&ws, "really-gone", "daily", "{}", 60, None);
        seed(&ws, "reggie", "list", "{}", 60, Some("do things\n"));
        {
            let db = crate::db::shared();
            let conn = db.lock();
            for name in ["was-empty", "really-gone"] {
                AgentHeartbeat::auto_disable(&conn, &ws.project_id, name, DISABLED_WAKEUP_MISSING)
                    .unwrap();
            }
        }

        // S3: the stored wait state (single source) is written by the
        // daemon's wait pass; one pass flags reggie before boot_reconcile.
        refresh_project(&ws.path).expect("wait pass");
        let reggie = listed(&ws, "reggie");
        assert_eq!(reggie.wait_reason.as_deref(), Some(WAIT_SCHEDULE_ERROR));
        assert_eq!(reggie.wait_detail.as_deref(), Some("unknown frequency 'list'"));

        let first = {
            let db = crate::db::shared();
            let conn = db.lock();
            boot_reconcile_with(&conn).expect("reconcile")
        };
        let label = |n: &str| format!("{}::{n}", ws.path);
        assert!(first.relabelled.contains(&label("was-empty")), "{first:?}");
        assert!(!first.relabelled.contains(&label("really-gone")), "{first:?}");
        assert!(first.flagged.contains(&label("reggie")), "{first:?}");

        let was_empty = row(&ws, "was-empty");
        assert!(!was_empty.enabled, "relabel keeps the row disabled");
        assert_eq!(was_empty.disabled_reason.as_deref(), Some(DISABLED_WAKEUP_EMPTY));
        let gone = row(&ws, "really-gone");
        assert_eq!(gone.disabled_reason.as_deref(), Some(DISABLED_WAKEUP_MISSING));
        let reggie = row(&ws, "reggie");
        assert!(reggie.enabled, "an invalid schedule is flagged, not disabled");
        assert_eq!(reggie.frequency, "list", "the row is not rewritten");
        assert_eq!(reggie.schedule_error.as_deref(), Some("unknown frequency 'list'"));
        assert_eq!(decisions(&ws, "reggie"), vec!["schedule_invalid".to_string()]);

        let second = {
            let db = crate::db::shared();
            let conn = db.lock();
            boot_reconcile_with(&conn).expect("reconcile again")
        };
        assert!(!second.flagged.contains(&label("reggie")), "{second:?}");
        assert!(!second.relabelled.contains(&label("was-empty")), "{second:?}");
        assert_eq!(
            decisions(&ws, "reggie"),
            vec!["schedule_invalid".to_string()],
            "one audit row per episode, not per boot"
        );
    }
}

#[cfg(test)]
mod s3_derive_tests {
    //! Heartbeat S3 — the pure derivation (HB19, HB20) and the read-time
    //! `no_ticks` overlay (HB21).

    use super::*;
    use chrono::Duration;

    fn row(frequency: &str, spec: &str) -> AgentHeartbeat {
        AgentHeartbeat {
            id: "t".into(),
            project_id: "p".into(),
            name: "t".into(),
            frequency: frequency.into(),
            spec_json: spec.into(),
            wakeup_path: ".k2/heartbeats/t/WAKEUP.md".into(),
            enabled: true,
            last_fired: None,
            last_session_id: None,
            archived_at: None,
            created_at: 0,
            concurrency_policy: "forbid".into(),
            starting_deadline_secs: 600,
            active_deadline_secs: 30,
            in_flight_started_at: None,
            active_terminal_id: None,
            use_workspace_session: true,
            consecutive_failures: 0,
            next_retry_at: None,
            disabled_reason: None,
            schedule_error: None,
            session_provider: None,
            wait_reason: None,
            wait_detail: None,
            schedule_anchor_at: None,
            next_fire_at: None,
            wait_since: None,
            overdue_noted_at: None,
        }
    }

    fn ok_inputs() -> WaitInputs {
        WaitInputs { project_dir_exists: true, agent_resolvable: true, wakeup: WakeupBody::Present }
    }

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).single().expect("unambiguous test time")
    }

    #[test]
    fn interval_never_fired_is_created_plus_every() {
        let now = at(2026, 10, 1, 12, 0);
        let mut hb = row("hourly", r#"{"every_seconds":900}"#);
        hb.created_at = (now - Duration::seconds(10)).timestamp();
        let st = derive(&hb, &ok_inputs(), now);
        assert_eq!(st.reason, WAIT_SCHEDULED);
        assert_eq!(
            st.next_fire_at.as_deref(),
            Some(fmt_utc(now - Duration::seconds(10) + Duration::seconds(900)).as_str()),
        );
    }

    /// The research case: sew-build-drive last fired 07:06:32, every
    /// 900 s, nothing ticked. next_fire_at stays at the first missed
    /// slot (07:21:32), so the overdue clock starts there.
    #[test]
    fn missed_interval_keeps_the_first_missed_slot() {
        let last = at(2026, 10, 1, 7, 6) + Duration::seconds(32);
        let mut hb = row("hourly", r#"{"every_seconds":900}"#);
        hb.last_fired = Some(last.with_timezone(&Utc).to_rfc3339());
        for later_mins in [16, 30, 8 * 60] {
            let st = derive(&hb, &ok_inputs(), last + Duration::minutes(later_mins));
            assert_eq!(st.reason, WAIT_SCHEDULED);
            assert_eq!(
                st.next_fire_at.as_deref(),
                Some(fmt_utc(last + Duration::seconds(900)).as_str()),
                "{later_mins} min after the fire",
            );
        }
    }

    #[test]
    fn closed_window_stores_the_open_time() {
        // Every 30 min, window 09:00–17:00, last fired 16:00; at 18:05
        // the clock is outside the window with a due slot held.
        let mut hb = row("hourly", r#"{"every_seconds":1800,"start":"09:00","end":"17:00"}"#);
        hb.last_fired = Some(at(2026, 7, 2, 16, 0).to_rfc3339());
        let st = derive(&hb, &ok_inputs(), at(2026, 7, 2, 18, 5));
        assert_eq!(st.reason, WAIT_WINDOW_CLOSED, "got {st:?}");
        assert_eq!(st.next_fire_at.as_deref(), Some(fmt_utc(at(2026, 7, 3, 9, 0)).as_str()));
        assert_eq!(st.detail.as_deref(), Some("window opens 09:00"));
    }

    #[test]
    fn backoff_uses_next_retry_at_and_counts_failures() {
        let now = at(2026, 10, 1, 12, 0);
        let mut hb = row("hourly", r#"{"every_seconds":900}"#);
        hb.last_fired = Some((now - Duration::seconds(1000)).to_rfc3339());
        hb.consecutive_failures = 2;
        let retry = now + Duration::seconds(120);
        hb.next_retry_at = Some(retry.with_timezone(&Utc).to_rfc3339());
        let st = derive(&hb, &ok_inputs(), now);
        assert_eq!(st.reason, WAIT_BACKOFF);
        assert_eq!(st.next_fire_at.as_deref(), Some(fmt_utc(retry).as_str()));
        assert_eq!(st.detail.as_deref(), Some("failure 2 of 5"));
    }

    #[test]
    fn disabled_reasons_map_to_the_fixed_vocabulary() {
        let now = at(2026, 10, 1, 12, 0);
        let mut hb = row("daily", r#"{"time":"07:00"}"#);
        hb.enabled = false;
        for (raw, want) in [
            (None, WAIT_DISABLED_USER),
            (Some("failures"), WAIT_DISABLED_FAILURES),
            (Some("wakeup_missing"), WAIT_DISABLED_WAKEUP_MISSING),
            (Some("wakeup_empty"), WAIT_DISABLED_WAKEUP_EMPTY),
        ] {
            hb.disabled_reason = raw.map(str::to_string);
            let st = derive(&hb, &ok_inputs(), now);
            assert_eq!(st.reason, want);
            assert_eq!(st.next_fire_at, None, "a disabled row has no next fire");
        }
    }

    #[test]
    fn invalid_spec_is_schedule_error_with_null_next() {
        let st = derive(&row("list", "{}"), &ok_inputs(), at(2026, 10, 1, 12, 0));
        assert_eq!(st.reason, WAIT_SCHEDULE_ERROR);
        assert_eq!(st.next_fire_at, None);
        assert_eq!(st.detail.as_deref(), Some("unknown frequency 'list'"));
    }

    #[test]
    fn gates_in_precedence_order() {
        let now = at(2026, 10, 1, 12, 0);
        let mut hb = row("hourly", r#"{"every_seconds":900}"#);
        hb.created_at = now.timestamp();
        let no_agent = WaitInputs { agent_resolvable: false, ..ok_inputs() };
        assert_eq!(derive(&hb, &no_agent, now).reason, WAIT_NO_AGENT);
        let empty = WaitInputs { wakeup: WakeupBody::Empty, ..ok_inputs() };
        let st = derive(&hb, &empty, now);
        assert_eq!(st.reason, WAIT_WAKEUP_EMPTY);
        assert_eq!(st.detail.as_deref(), Some(WAKEUP_EMPTY_DETAIL), "S5's copy is kept");
        let empty_no_agent = WaitInputs { agent_resolvable: false, ..empty };
        assert_eq!(derive(&hb, &empty_no_agent, now).reason, WAIT_WAKEUP_EMPTY);
        let gone = WaitInputs { project_dir_exists: false, ..empty_no_agent };
        assert_eq!(derive(&hb, &gone, now).reason, WAIT_NO_PROJECT);
        hb.in_flight_started_at = Some(now.to_rfc3339());
        assert_eq!(derive(&hb, &empty, now).reason, WAIT_IN_FLIGHT);
        // Every gate keeps the evaluator's slot as next_fire_at.
        assert_eq!(
            derive(&hb, &no_agent, now).next_fire_at.as_deref(),
            Some(fmt_utc(now + Duration::seconds(900)).as_str()),
        );
    }

    /// D4: an empty row whose slot passed has no known next fire (it is
    /// the first slot after instructions are seen), so it is never
    /// `overdue` — a designed wait, not a stuck one.
    #[test]
    fn empty_wakeup_past_its_slot_stores_no_next_fire() {
        let now = at(2026, 10, 1, 12, 0);
        let mut hb = row("hourly", r#"{"every_seconds":900}"#);
        hb.created_at = (now - Duration::hours(1)).timestamp();
        let empty = WaitInputs { wakeup: WakeupBody::Empty, ..ok_inputs() };
        let st = derive(&hb, &empty, now);
        assert_eq!(st.reason, WAIT_WAKEUP_EMPTY);
        assert_eq!(st.next_fire_at, None);
    }

    #[test]
    fn no_ticks_overlays_only_overdue_enabled_rows() {
        let now = Utc::now();
        let mut overdue = row("hourly", "{}");
        overdue.next_fire_at = Some(fmt_utc(now - Duration::minutes(5)));
        overdue.wait_reason = Some(WAIT_OVERDUE.into());
        let mut future = row("hourly", "{}");
        future.next_fire_at = Some(fmt_utc(now + Duration::minutes(5)));
        future.wait_reason = Some(WAIT_SCHEDULED.into());
        let mut rows = vec![overdue, future];
        let last = now - Duration::minutes(10);

        overlay_no_ticks(&mut rows, Some(last), now);
        assert_eq!(rows[0].wait_reason.as_deref(), Some(WAIT_NO_TICKS));
        assert_eq!(rows[0].wait_since.as_deref(), Some(fmt_utc(last).as_str()));
        assert_eq!(rows[1].wait_reason.as_deref(), Some(WAIT_SCHEDULED));

        // A tick 30 s ago: nothing is overlaid.
        rows[0].wait_reason = Some(WAIT_OVERDUE.into());
        overlay_no_ticks(&mut rows, Some(now - Duration::seconds(30)), now);
        assert_eq!(rows[0].wait_reason.as_deref(), Some(WAIT_OVERDUE));
    }

    /// T-S3f (Rust half) — the HB20 vocabulary is pinned in a fixture the
    /// renderer formatter test reads. A new reason must land in both.
    #[test]
    fn vocabulary_fixture_matches() {
        let fixture = include_str!("fixtures/wait-reasons.json");
        let parsed: Vec<String> =
            serde_json::from_str(fixture).expect("wait-reasons.json is a JSON string array");
        let want: Vec<String> = ALL_REASONS.iter().map(|s| s.to_string()).collect();
        assert_eq!(parsed, want, "fixtures/wait-reasons.json must list ALL_REASONS in order");
    }
}
