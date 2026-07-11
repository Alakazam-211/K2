//! Croner-backed heartbeat due-evaluation — the SINGLE due-authority.
//!
//! History: 0.38.2 replaced the hand-rolled `is_past_deadline` (whose
//! `starting_deadline_secs` grace left heartbeats dark for 22+ days
//! after any long pause) with croner-backed "if now is at or past the
//! next occurrence after last_fired, fire". But a second, older
//! evaluator — `should_project_fire` in `crate::scheduler` — still ran
//! FIRST on every Lane-B tick and re-checked the schedule against
//! *today's calendar position* (minute-of-day, weekday, day-of-month).
//! Any miss that crossed a day/week/month boundary was silently dropped
//! as `skipped_schedule "window not open"` — a weekly Monday report
//! missed while the laptop was dead stayed dark until NEXT Monday.
//! (Full diagnosis: `.k2/notes/heartbeat-misfire-study.md`.)
//!
//! The reliability overhaul makes THIS module the only evaluator for
//! `workspace_heartbeats` rows (`should_project_fire` remains for the
//! project-level Lane A only). Semantics:
//!
//! - **Due iff an occurrence after the reference point is ≤ now.**
//!   Reference = `last_fired`, or `created_at` for a never-fired row
//!   (a new heartbeat waits for its first scheduled slot — no
//!   fire-on-create).
//! - **Misses ALWAYS catch up** (owner decision, 2026-07): however old
//!   the miss, the next tick fires ONE coalesced catch-up for the most
//!   recent missed occurrence, then the schedule resumes normally.
//!   There is no grace-window cutoff and no skip-to-next. Fires more
//!   than [`ON_TIME_GRACE_SECS`] late are reported as `DueCatchUp`
//!   (audited `fired_catchup` with the originally-scheduled time) so
//!   the trail distinguishes on-time from recovered fires.
//! - **Firing windows**: an optional per-heartbeat `start`/`end`
//!   time-of-day pair in spec_json (the same keys the hourly frequency
//!   always had — now honored for every frequency). Occurrences whose
//!   scheduled time-of-day falls outside the window don't fire and
//!   don't count as missed; a due fire evaluated while the window is
//!   closed HOLDS (`HoldWindow`) until the window next opens, then
//!   fires once. No window = fire any hour.
//! - **Unparseable specs are loud**: `Invalid { reason }` instead of a
//!   silent false, so the tick can surface a visible error state.
//! - **No manual-fire latch**: manual launches stamp `last_fired`
//!   like any fire, and the next occurrence is computed from there —
//!   the once-per-day "manual fire consumes today's scheduled fire"
//!   behavior of the legacy gate is gone by design.

use crate::db::schema::AgentHeartbeat;
use chrono::{DateTime, Duration, Local, TimeZone, Timelike};
use croner::Cron;
use serde_json::Value;
use std::str::FromStr;

/// A fire at most this many seconds after its scheduled occurrence is
/// still "on time" (audited `fired`); anything later is a recovered
/// miss (audited `fired_catchup` with the originally-scheduled time).
/// 15 minutes absorbs tick cadence (60s default, user-configurable up
/// to a few minutes) plus spawn queueing without mislabeling a real
/// gap — a laptop that slept through a 9 AM fire and woke at 9:20
/// correctly reports a catch-up.
pub const ON_TIME_GRACE_SECS: i64 = 15 * 60;

/// Iteration cap for the backward occurrence walk (latest in-window
/// occurrence). Generous — a window has to appear within this many
/// consecutive occurrences or we report `NotYet` rather than loop
/// unboundedly.
const WALK_CAP: usize = 5000;

/// Iteration cap for the forward "next in-window occurrence" walk.
/// Smaller than [`WALK_CAP`]: this value is display-only (`NotYet::next`)
/// and cron-mode steps each cost a croner search, so we bound the
/// worst case (a window the schedule can never land in) tightly.
const NEXT_WALK_CAP: usize = 400;

/// Result of evaluating one heartbeat row against a clock instant.
/// The tick loop maps these to fire decisions + audit rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DueStatus {
    /// Fire now; the occurrence is at most [`ON_TIME_GRACE_SECS`] old.
    Due {
        scheduled_for: DateTime<Local>,
    },
    /// Fire now, catching up a missed occurrence. When several were
    /// missed they coalesce to this single fire for the MOST RECENT
    /// missed occurrence (`missed_at` = its originally-scheduled time).
    DueCatchUp {
        missed_at: DateTime<Local>,
    },
    /// An occurrence is due (possibly a catch-up) but the firing
    /// window is currently closed — hold, don't fire, don't record a
    /// miss. Re-evaluates every tick; fires once the window opens.
    HoldWindow {
        scheduled_for: DateTime<Local>,
    },
    /// Nothing due. `next` is the next in-window occurrence when it
    /// could be computed (display/diagnostics only).
    NotYet {
        next: Option<DateTime<Local>>,
    },
    /// The row's frequency/spec_json can never produce an occurrence.
    /// The tick surfaces this as a visible `schedule_error` state
    /// instead of the pre-overhaul silent not-due.
    Invalid {
        reason: String,
    },
}

/// Optional per-heartbeat firing window (time-of-day range). Parsed
/// from spec_json `start`/`end` ("HH:MM"). Same in-window test the
/// legacy hourly gate used: `[start, end)`, with overnight wrap when
/// `start > end` (e.g. 22:00–06:00).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FiringWindow {
    start_mins: u32,
    end_mins: u32,
}

impl FiringWindow {
    /// `Ok(None)` when the spec has no window keys; `Err` when a key
    /// is present but malformed (surfaced as `Invalid` — a typo'd
    /// window must not silently widen to always-open).
    fn parse(spec: &Value) -> Result<Option<Self>, String> {
        let start = spec.get("start").and_then(|s| s.as_str());
        let end = spec.get("end").and_then(|s| s.as_str());
        if start.is_none() && end.is_none() {
            return Ok(None);
        }
        let start_mins = match start {
            Some(s) => parse_hhmm_mins(s)
                .ok_or_else(|| format!("firing window start '{s}' is not HH:MM"))?,
            None => 0,
        };
        let end_mins = match end {
            Some(s) => parse_hhmm_mins(s)
                .ok_or_else(|| format!("firing window end '{s}' is not HH:MM"))?,
            None => 24 * 60 - 1,
        };
        Ok(Some(FiringWindow { start_mins, end_mins }))
    }

    fn contains(&self, t: DateTime<Local>) -> bool {
        let mins = t.hour() * 60 + t.minute();
        if self.start_mins <= self.end_mins {
            mins >= self.start_mins && mins < self.end_mins
        } else {
            // Overnight window (e.g. 22:00–06:00).
            mins >= self.start_mins || mins < self.end_mins
        }
    }
}

/// Parse `HH:MM` → minute-of-day (0..=1439).
fn parse_hhmm_mins(s: &str) -> Option<u32> {
    let mut parts = s.split(':');
    let h: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || h > 23 || m > 59 {
        return None;
    }
    Some(h * 60 + m)
}

/// Evaluate a heartbeat row against the wall clock. Runtime entry
/// point; tests inject `now` via [`evaluate_with_now`].
pub fn evaluate(hb: &AgentHeartbeat) -> DueStatus {
    evaluate_with_now(hb, Local::now())
}

/// Core due-evaluation with an explicit `now`. Pure — no DB, no IO.
pub fn evaluate_with_now(hb: &AgentHeartbeat, now: DateTime<Local>) -> DueStatus {
    let spec: Value = match serde_json::from_str(&hb.spec_json) {
        Ok(v) => v,
        Err(e) => {
            return DueStatus::Invalid {
                reason: format!("spec_json is not valid JSON: {e}"),
            }
        }
    };
    let window = match FiringWindow::parse(&spec) {
        Ok(w) => w,
        Err(reason) => return DueStatus::Invalid { reason },
    };

    // Reference point occurrences are computed AFTER. `last_fired`
    // when the row has fired; `created_at` otherwise — a freshly
    // created daily-09:00 heartbeat added at 15:00 waits for
    // tomorrow's 09:00 instead of firing immediately (first-fire
    // semantics, owner decision). An unparseable last_fired falls
    // back to created_at rather than wedging the row.
    let reference = hb
        .last_fired
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Local))
        .unwrap_or_else(|| {
            Local
                .timestamp_opt(hb.created_at, 0)
                .single()
                .unwrap_or_else(Local::now)
        });

    // daily/weekly/monthly/yearly funnel through cron; hourly is a
    // pure interval after the reference point.
    match hb.frequency.as_str() {
        "hourly" => evaluate_interval(&spec, reference, now, window),
        "daily" | "weekly" | "monthly" | "yearly" | "scheduled" => {
            evaluate_cron(&spec, &hb.frequency, reference, now, window)
        }
        other => DueStatus::Invalid {
            reason: format!("unknown frequency '{other}'"),
        },
    }
}

/// Interval mode (`hourly`): occurrences at `reference + k·every_seconds`.
fn evaluate_interval(
    spec: &Value,
    reference: DateTime<Local>,
    now: DateTime<Local>,
    window: Option<FiringWindow>,
) -> DueStatus {
    // Default preserved from the pre-overhaul evaluator so legacy rows
    // whose spec lacks the key keep their observed cadence.
    let every = spec
        .get("every_seconds")
        .and_then(|s| s.as_i64())
        .unwrap_or(3600);
    if every < 1 {
        return DueStatus::Invalid {
            reason: format!("every_seconds must be >= 1 (got {every})"),
        };
    }

    let elapsed = (now - reference).num_seconds();
    let k_max = elapsed / every; // occurrences elapsed since reference
    let occurrence = |k: i64| reference + Duration::seconds(k * every);

    if k_max < 1 {
        return DueStatus::NotYet {
            next: next_in_window(occurrence(1), |t| t + Duration::seconds(every), window),
        };
    }

    // Latest in-window occurrence ≤ now — walk backward from k_max.
    // Keep walking one hit further to learn whether MORE than one
    // in-window occurrence is pending (that's what distinguishes a
    // coalesced catch-up from an on-time fire for interval schedules,
    // whose latest occurrence is by construction never more than
    // `every` seconds old).
    let mut latest: Option<DateTime<Local>> = None;
    let mut multiple_pending = false;
    let mut k = k_max;
    let mut steps = 0usize;
    while k >= 1 && steps < WALK_CAP {
        let occ = occurrence(k);
        if window.map(|w| w.contains(occ)).unwrap_or(true) {
            if latest.is_none() {
                latest = Some(occ);
            } else {
                multiple_pending = true;
                break;
            }
        }
        k -= 1;
        steps += 1;
    }

    match latest {
        Some(occ) => classify(occ, now, window, multiple_pending),
        None => DueStatus::NotYet {
            next: next_in_window(
                occurrence(k_max + 1),
                |t| t + Duration::seconds(every),
                window,
            ),
        },
    }
}

/// Cron mode (`daily`/`weekly`/`monthly`/`yearly`): occurrences from
/// the translated cron expression via croner.
fn evaluate_cron(
    spec: &Value,
    frequency: &str,
    reference: DateTime<Local>,
    now: DateTime<Local>,
    window: Option<FiringWindow>,
) -> DueStatus {
    let expr = match build_cron_expression(spec, frequency) {
        Ok(e) => e,
        Err(reason) => return DueStatus::Invalid { reason },
    };
    let cron = match Cron::from_str(expr.as_str()) {
        Ok(c) => c,
        Err(e) => {
            return DueStatus::Invalid {
                reason: format!("cron expression '{expr}' rejected: {e}"),
            }
        }
    };

    // First occurrence strictly after the reference point. A croner
    // error here (search horizon exhausted — e.g. day 30 in February
    // only) means the schedule can never fire: say so loudly.
    let first = match cron.find_next_occurrence(&reference, false) {
        Ok(t) => t,
        Err(e) => {
            return DueStatus::Invalid {
                reason: format!("schedule never occurs ({e})"),
            }
        }
    };
    if first > now {
        return DueStatus::NotYet {
            next: next_in_window(
                first,
                |t| cron.find_next_occurrence(&t, false).unwrap_or(t + Duration::days(400)),
                window,
            ),
        };
    }

    // Latest in-window occurrence in (reference, now] — walk backward
    // from now. Multiple misses coalesce to this single occurrence;
    // we walk one in-window hit further to know whether more than one
    // is pending (a coalesced catch-up vs a single on-time fire).
    let mut cursor = match cron.find_previous_occurrence(&now, true) {
        Ok(t) => t,
        Err(e) => {
            return DueStatus::Invalid {
                reason: format!("schedule never occurs ({e})"),
            }
        }
    };
    let mut latest: Option<DateTime<Local>> = None;
    let mut multiple_pending = false;
    let mut steps = 0usize;
    while cursor > reference && steps < WALK_CAP {
        if window.map(|w| w.contains(cursor)).unwrap_or(true) {
            if latest.is_none() {
                latest = Some(cursor);
            } else {
                multiple_pending = true;
                break;
            }
        }
        cursor = match cron.find_previous_occurrence(&cursor, false) {
            Ok(t) => t,
            Err(_) => break,
        };
        steps += 1;
    }
    if let Some(occ) = latest {
        return classify(occ, now, window, multiple_pending);
    }

    // Every elapsed occurrence fell outside the window — nothing
    // missed, nothing due (owner decision: out-of-window occurrences
    // don't fire and don't count as missed).
    DueStatus::NotYet {
        next: next_in_window(
            cron.find_next_occurrence(&now, false).unwrap_or(now + Duration::days(400)),
            |t| cron.find_next_occurrence(&t, false).unwrap_or(t + Duration::days(400)),
            window,
        ),
    }
}

/// A due occurrence exists — decide between fire-now (on time /
/// catch-up) and hold-for-window.
///
/// Catch-up when the fire is more than [`ON_TIME_GRACE_SECS`] behind
/// its occurrence, OR when more than one in-window occurrence is
/// pending (`multiple_pending`) — the latter is what makes a
/// long-paused interval schedule report a catch-up even though its
/// most recent occurrence is by construction always fresh.
fn classify(
    occurrence: DateTime<Local>,
    now: DateTime<Local>,
    window: Option<FiringWindow>,
    multiple_pending: bool,
) -> DueStatus {
    if let Some(w) = window {
        if !w.contains(now) {
            return DueStatus::HoldWindow { scheduled_for: occurrence };
        }
    }
    if !multiple_pending && (now - occurrence).num_seconds() <= ON_TIME_GRACE_SECS {
        DueStatus::Due { scheduled_for: occurrence }
    } else {
        DueStatus::DueCatchUp { missed_at: occurrence }
    }
}

/// Walk `start, advance(start), …` forward until an occurrence falls
/// inside the window (or the walk cap is hit). Used only for the
/// `NotYet::next` display value.
fn next_in_window(
    start: DateTime<Local>,
    advance: impl Fn(DateTime<Local>) -> DateTime<Local>,
    window: Option<FiringWindow>,
) -> Option<DateTime<Local>> {
    let Some(w) = window else { return Some(start) };
    let mut t = start;
    for _ in 0..NEXT_WALK_CAP {
        if w.contains(t) {
            return Some(t);
        }
        let n = advance(t);
        if n <= t {
            return None; // advance stalled — refuse to loop
        }
        t = n;
    }
    None
}

/// Compute when this heartbeat is *next* due after a reference time
/// (typically `last_fired`). Kept for the audit/status surfaces that
/// show "next at HH:MM"; window-blind by design (it reports the raw
/// schedule). `None` = spec unparseable/unsupported.
pub fn next_fire_time_after(
    hb: &AgentHeartbeat,
    after: DateTime<Local>,
) -> Option<DateTime<Local>> {
    let spec: Value = serde_json::from_str(&hb.spec_json).ok()?;
    match hb.frequency.as_str() {
        "hourly" => {
            let every_secs = spec
                .get("every_seconds")
                .and_then(|s| s.as_i64())
                .unwrap_or(3600);
            Some(after + Duration::seconds(every_secs))
        }
        "daily" | "weekly" | "monthly" | "yearly" | "scheduled" => {
            let expr = build_cron_expression(&spec, &hb.frequency).ok()?;
            let cron = Cron::from_str(expr.as_str()).ok()?;
            cron.find_next_occurrence(&after, false).ok()
        }
        _ => None,
    }
}

/// Translate K2's spec_json shape into a 5-field cron expression
/// (`minute hour day-of-month month day-of-week`). `Err` carries a
/// human-readable reason surfaced through `DueStatus::Invalid` — the
/// overhaul's replacement for the old silent `None → not due`.
///
/// Supported spec shapes (superset of the pre-overhaul translator —
/// `days_of_month` arrays, ordinal weekdays, and yearly month lists
/// were previously only understood by the retired legacy gate, so
/// UI-created monthly/yearly rows silently degraded to day 1):
///
/// - `daily`:   `{ "time": "HH:MM" }` (+ optional `interval` N →
///   `*/N` on day-of-month, the closest cron can express)
/// - `weekly`:  `{ "time": "HH:MM", "days": ["mon","wed",...] }`
/// - `monthly`: `{ "time": "HH:MM", "days_of_month": [1,15] }` or
///   `{ "day_of_month": 15 }` or
///   `{ "ordinal": "first".."fourth"|"last", "ordinal_day": "mon".. }`
/// - `yearly`:  `{ "time": "HH:MM", "months": ["jan",...],
///   "days_of_month": [..] }` (or singular `month`/`day_of_month`)
fn build_cron_expression(spec: &Value, frequency: &str) -> Result<String, String> {
    let time_str = spec
        .get("time")
        .and_then(|s| s.as_str())
        .unwrap_or("09:00");
    let mins = parse_hhmm_mins(time_str)
        .ok_or_else(|| format!("time '{time_str}' is not HH:MM"))?;
    let (h, m) = (mins / 60, mins % 60);

    // Shared helper: `days_of_month` array (UI shape) with singular
    // `day_of_month` fallback (legacy shape).
    let dom_field = |default: &str| -> String {
        if let Some(arr) = spec.get("days_of_month").and_then(|d| d.as_array()) {
            let days: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_i64())
                .filter(|d| (1..=31).contains(d))
                .map(|d| d.to_string())
                .collect();
            if !days.is_empty() {
                return days.join(",");
            }
        }
        spec.get("day_of_month")
            .and_then(|d| d.as_i64())
            .map(|d| d.to_string())
            .unwrap_or_else(|| default.to_string())
    };

    match frequency {
        "daily" => {
            // Legacy `interval` (every N days) can't be expressed
            // exactly in cron; `*/N` on day-of-month is the closest
            // (matches the legacy gate's day-number-modulo jank —
            // both alias at month/year boundaries).
            let interval = spec.get("interval").and_then(|s| s.as_u64()).unwrap_or(1);
            if interval > 1 {
                Ok(format!("{m} {h} */{interval} * *"))
            } else {
                Ok(format!("{m} {h} * * *"))
            }
        }
        "weekly" => {
            let dow_part = spec
                .get("days")
                .and_then(|d| d.as_array())
                .map(|arr| {
                    let names: Vec<String> = arr
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_uppercase))
                        .collect();
                    if names.is_empty() { "*".to_string() } else { names.join(",") }
                })
                .unwrap_or_else(|| "*".to_string());
            Ok(format!("{m} {h} * * {dow_part}"))
        }
        "monthly" => {
            // Ordinal shape ("first monday", "last fri") → croner's
            // `#`/`#L` day-of-week qualifiers.
            if let Some(ordinal) = spec.get("ordinal").and_then(|s| s.as_str()) {
                let ordinal_day = spec
                    .get("ordinal_day")
                    .and_then(|s| s.as_str())
                    .unwrap_or("day");
                return build_ordinal_expression(m, h, ordinal, ordinal_day);
            }
            Ok(format!("{m} {h} {} * *", dom_field("1")))
        }
        "yearly" => {
            let month_field = spec
                .get("months")
                .and_then(|d| d.as_array())
                .map(|arr| {
                    let names: Vec<String> = arr
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_uppercase))
                        .collect();
                    if names.is_empty() { "1".to_string() } else { names.join(",") }
                })
                .unwrap_or_else(|| {
                    spec.get("month")
                        .and_then(|d| d.as_i64())
                        .map(|mth| mth.to_string())
                        .unwrap_or_else(|| "1".to_string())
                });
            Ok(format!("{m} {h} {} {} *", dom_field("1"), month_field))
        }
        other => Err(format!("unsupported frequency '{other}'")),
    }
}

/// Ordinal monthly shapes → cron. `first`..`fourth` map to croner's
/// `DOW#N`; `last` maps to `DOW#L`. `ordinal_day` "day" uses plain
/// day-of-month (first day = 1st, last day = `L`); "weekday" uses the
/// `W` nearest-weekday qualifier where cron can express it.
fn build_ordinal_expression(
    m: u32,
    h: u32,
    ordinal: &str,
    ordinal_day: &str,
) -> Result<String, String> {
    let nth = match ordinal {
        "first" => "1",
        "second" => "2",
        "third" => "3",
        "fourth" => "4",
        "last" => "L",
        other => return Err(format!("unsupported ordinal '{other}'")),
    };
    match ordinal_day {
        "day" => {
            let dom = match ordinal {
                "first" => "1".to_string(),
                "second" => "8".to_string(),
                "third" => "15".to_string(),
                "fourth" => "22".to_string(),
                "last" => "L".to_string(),
                _ => unreachable!("ordinal validated above"),
            };
            Ok(format!("{m} {h} {dom} * *"))
        }
        "weekday" => match ordinal {
            // `W` = nearest weekday to the day — cron's closest
            // expressible reading of "first/last weekday of month".
            "first" => Ok(format!("{m} {h} 1W * *")),
            "last" => Ok(format!("{m} {h} LW * *")),
            other => Err(format!(
                "'{other} weekday' is not expressible as a cron schedule; \
                 pick a specific weekday or days of month"
            )),
        },
        day => {
            let dow = match day {
                "mon" | "monday" => "MON",
                "tue" | "tuesday" => "TUE",
                "wed" | "wednesday" => "WED",
                "thu" | "thursday" => "THU",
                "fri" | "friday" => "FRI",
                "sat" | "saturday" => "SAT",
                "sun" | "sunday" => "SUN",
                other => return Err(format!("unsupported ordinal day '{other}'")),
            };
            Ok(format!("{m} {h} * * {dow}#{nth}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn mk_heartbeat(frequency: &str, spec_json: &str, last_fired: Option<&str>) -> AgentHeartbeat {
        AgentHeartbeat {
            id: "test".to_string(),
            project_id: "p".to_string(),
            name: "test".to_string(),
            frequency: frequency.to_string(),
            spec_json: spec_json.to_string(),
            wakeup_path: ".k2so/agent/heartbeats/test/WAKEUP.md".to_string(),
            enabled: true,
            last_fired: last_fired.map(str::to_string),
            last_session_id: None,
            archived_at: None,
            // Created long ago so never-fired rows evaluate against a
            // realistic backlog unless a test overrides it.
            created_at: Local
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .unwrap()
                .timestamp(),
            concurrency_policy: "forbid".to_string(),
            starting_deadline_secs: 600,
            active_deadline_secs: 30,
            in_flight_started_at: None,
            active_terminal_id: None,
            use_workspace_session: false,
            consecutive_failures: 0,
            next_retry_at: None,
            disabled_reason: None,
            schedule_error: None,
            session_provider: None,
        }
    }

    fn mk_now(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("test datetime must be unambiguous")
    }

    fn is_fire_now(s: &DueStatus) -> bool {
        matches!(s, DueStatus::Due { .. } | DueStatus::DueCatchUp { .. })
    }

    // ── The misfire-study trace matrix (§1.4), now with catch-up ──────
    //
    // Table-driven port of the five empirical traces: every previously
    // dropped case must now be a DueCatchUp for the most recent missed
    // occurrence.

    #[test]
    fn daily_slept_through_same_day_fires_on_time() {
        // Daily 09:00, last fired yesterday 09:00, now 09:05 today.
        let hb = mk_heartbeat(
            "daily",
            r#"{"time":"09:00"}"#,
            Some(&mk_now(2026, 7, 1, 9, 0).to_rfc3339()),
        );
        let now = mk_now(2026, 7, 2, 9, 5);
        assert_eq!(
            evaluate_with_now(&hb, now),
            DueStatus::Due { scheduled_for: mk_now(2026, 7, 2, 9, 0) },
        );
    }

    #[test]
    fn daily_miss_across_day_boundary_catches_up() {
        // THE money case: daily 23:50, last fired 3 days ago, machine
        // dead through two occurrences, tick at 17:07 (before today's
        // 23:50). Legacy gate dropped this at scheduler.rs:117; the
        // single-authority evaluator fires a catch-up for the MOST
        // RECENT missed occurrence (yesterday 23:50).
        let hb = mk_heartbeat(
            "daily",
            r#"{"time":"23:50"}"#,
            Some(&mk_now(2026, 6, 29, 23, 50).to_rfc3339()),
        );
        let now = mk_now(2026, 7, 2, 17, 7);
        assert_eq!(
            evaluate_with_now(&hb, now),
            DueStatus::DueCatchUp { missed_at: mk_now(2026, 7, 1, 23, 50) },
            "a cross-midnight miss must catch up, coalesced to the latest occurrence",
        );
    }

    #[test]
    fn weekly_missed_weekday_catches_up_next_day() {
        // Weekly Wed 09:00, last fired 8 days ago → yesterday (Wed
        // 2026-07-01) missed; tick Thursday. Legacy gate dropped this
        // at scheduler.rs:150 ("tue ∉ [wed]") until NEXT Wednesday.
        let hb = mk_heartbeat(
            "weekly",
            r#"{"time":"09:00","days":["wed"]}"#,
            Some(&mk_now(2026, 6, 24, 9, 0).to_rfc3339()),
        );
        let now = mk_now(2026, 7, 2, 17, 0); // Thursday
        assert_eq!(
            evaluate_with_now(&hb, now),
            DueStatus::DueCatchUp { missed_at: mk_now(2026, 7, 1, 9, 0) },
        );
    }

    #[test]
    fn monthly_missed_dom_catches_up() {
        // Monthly 15th 09:00, dead on the 15th, tick on the 17th.
        let hb = mk_heartbeat(
            "monthly",
            r#"{"time":"09:00","days_of_month":[15]}"#,
            Some(&mk_now(2026, 5, 15, 9, 0).to_rfc3339()),
        );
        let now = mk_now(2026, 6, 17, 12, 0);
        assert_eq!(
            evaluate_with_now(&hb, now),
            DueStatus::DueCatchUp { missed_at: mk_now(2026, 6, 15, 9, 0) },
        );
    }

    #[test]
    fn hourly_recovers_from_22_day_pause_as_catchup() {
        // The 0.38.2 case, still healthy: 22 days dark → due on the
        // first tick back, now correctly labeled a catch-up.
        let hb = mk_heartbeat(
            "hourly",
            r#"{"every_seconds":3600}"#,
            Some(&(Local::now() - Duration::days(22)).to_rfc3339()),
        );
        match evaluate(&hb) {
            DueStatus::DueCatchUp { missed_at } => {
                let lateness = (Local::now() - missed_at).num_seconds();
                assert!(
                    (0..3600 + ON_TIME_GRACE_SECS).contains(&lateness),
                    "coalesced catch-up must target the most recent occurrence \
                     (lateness={lateness}s)",
                );
            }
            other => panic!("22-day-stale hourly heartbeat must catch up, got {other:?}"),
        }
    }

    #[test]
    fn old_misses_always_catch_up_no_grace_cutoff() {
        // Owner decision 1: no matter how old the miss, it fires once.
        // Weekly Monday report missed for six weeks → one catch-up for
        // the most recent Monday.
        let hb = mk_heartbeat(
            "weekly",
            r#"{"time":"09:00","days":["mon"]}"#,
            Some(&mk_now(2026, 5, 18, 9, 0).to_rfc3339()), // a Monday
        );
        let now = mk_now(2026, 7, 2, 12, 0); // Thursday, ~6.5 weeks later
        assert_eq!(
            evaluate_with_now(&hb, now),
            DueStatus::DueCatchUp { missed_at: mk_now(2026, 6, 29, 9, 0) },
            "misses must ALWAYS catch up — one coalesced fire for the latest occurrence",
        );
    }

    // ── On-time vs catch-up boundary ──────────────────────────────────

    #[test]
    fn hourly_fires_on_time_within_grace() {
        let hb = mk_heartbeat(
            "hourly",
            r#"{"every_seconds":3600}"#,
            Some(&(Local::now() - Duration::seconds(3660)).to_rfc3339()),
        );
        assert!(
            matches!(evaluate(&hb), DueStatus::Due { .. }),
            "60s late is on-time, not a catch-up",
        );
    }

    #[test]
    fn hourly_not_yet_due_within_interval() {
        let hb = mk_heartbeat(
            "hourly",
            r#"{"every_seconds":3600}"#,
            Some(&(Local::now() - Duration::minutes(30)).to_rfc3339()),
        );
        assert!(matches!(evaluate(&hb), DueStatus::NotYet { .. }));
    }

    // ── First-fire semantics ──────────────────────────────────────────

    #[test]
    fn never_fired_waits_for_first_scheduled_slot() {
        // Daily 09:00 created today at 08:00, evaluated at 08:30 —
        // must NOT fire on create; first slot is 09:00.
        let mut hb = mk_heartbeat("daily", r#"{"time":"09:00"}"#, None);
        hb.created_at = mk_now(2026, 7, 2, 8, 0).timestamp();
        let now = mk_now(2026, 7, 2, 8, 30);
        assert_eq!(
            evaluate_with_now(&hb, now),
            DueStatus::NotYet { next: Some(mk_now(2026, 7, 2, 9, 0)) },
        );
    }

    #[test]
    fn never_fired_created_after_todays_slot_waits_for_tomorrow() {
        // Daily 09:00 created at 15:00 — pre-overhaul this fired
        // immediately (gate passed since now ≥ 09:00); now it waits
        // for tomorrow 09:00.
        let mut hb = mk_heartbeat("daily", r#"{"time":"09:00"}"#, None);
        hb.created_at = mk_now(2026, 7, 2, 15, 0).timestamp();
        let now = mk_now(2026, 7, 2, 15, 5);
        assert_eq!(
            evaluate_with_now(&hb, now),
            DueStatus::NotYet { next: Some(mk_now(2026, 7, 3, 9, 0)) },
        );
    }

    #[test]
    fn never_fired_missed_first_slot_catches_up() {
        // Created before the slot, machine dead through it → the first
        // occurrence itself catches up.
        let mut hb = mk_heartbeat("daily", r#"{"time":"09:00"}"#, None);
        hb.created_at = mk_now(2026, 7, 1, 8, 0).timestamp();
        let now = mk_now(2026, 7, 2, 7, 0);
        assert_eq!(
            evaluate_with_now(&hb, now),
            DueStatus::DueCatchUp { missed_at: mk_now(2026, 7, 1, 9, 0) },
        );
    }

    #[test]
    fn hourly_never_fired_waits_one_interval_from_create() {
        let mut hb = mk_heartbeat("hourly", r#"{"every_seconds":3600}"#, None);
        hb.created_at = (Local::now() - Duration::minutes(10)).timestamp();
        assert!(
            matches!(evaluate(&hb), DueStatus::NotYet { .. }),
            "a fresh hourly heartbeat waits every_seconds before its first fire",
        );
    }

    // ── Firing windows ────────────────────────────────────────────────

    #[test]
    fn window_blocks_fire_outside_and_allows_inside() {
        // Hourly every 30 min, window 09:00–17:00.
        let spec = r#"{"every_seconds":1800,"start":"09:00","end":"17:00"}"#;
        let hb = mk_heartbeat(
            "hourly",
            spec,
            Some(&mk_now(2026, 7, 2, 10, 0).to_rfc3339()),
        );
        // 10:35 — occurrence 10:30 in window, now in window → fire.
        assert!(is_fire_now(&evaluate_with_now(&hb, mk_now(2026, 7, 2, 10, 35))));
        // 18:05 — occurrences since 17:00 are out-of-window; the last
        // in-window one (16:30/17:00 boundary aside) is stale but the
        // clock is OUTSIDE the window → hold, don't fire at 6 PM.
        let evening = evaluate_with_now(&hb, mk_now(2026, 7, 2, 18, 5));
        assert!(
            matches!(evening, DueStatus::HoldWindow { .. }),
            "outside the window a due occurrence must hold, got {evening:?}",
        );
    }

    #[test]
    fn out_of_window_occurrences_do_not_count_as_missed() {
        // Daily 20:00 with window 09:00–17:00: the 20:00 occurrence is
        // never in-window, so nothing is ever due — and critically,
        // nothing is recorded as missed. (A schedule whose time is
        // outside its own window is user error surfaced by NotYet/next
        // = None, not by phantom catch-ups.)
        let hb = mk_heartbeat(
            "daily",
            r#"{"time":"20:00","start":"09:00","end":"17:00"}"#,
            Some(&mk_now(2026, 7, 1, 20, 0).to_rfc3339()),
        );
        let s = evaluate_with_now(&hb, mk_now(2026, 7, 2, 21, 0));
        assert!(
            matches!(s, DueStatus::NotYet { .. }),
            "out-of-window occurrences neither fire nor count as missed, got {s:?}",
        );
    }

    #[test]
    fn catchup_landing_outside_window_holds_until_open() {
        // Daily 10:00 with window 09:00–17:00. Missed yesterday's
        // 10:00; machine wakes at 20:00 (window closed) → HOLD. At
        // 09:01 next day the window is open → catch-up fires.
        let hb = mk_heartbeat(
            "daily",
            r#"{"time":"10:00","start":"09:00","end":"17:00"}"#,
            Some(&mk_now(2026, 6, 30, 10, 0).to_rfc3339()),
        );
        let held = evaluate_with_now(&hb, mk_now(2026, 7, 1, 20, 0));
        assert_eq!(
            held,
            DueStatus::HoldWindow { scheduled_for: mk_now(2026, 7, 1, 10, 0) },
        );
        // Window opens next morning BEFORE the day's own 10:00 slot:
        // the held catch-up fires for yesterday's occurrence.
        let fired = evaluate_with_now(&hb, mk_now(2026, 7, 2, 9, 1));
        assert_eq!(
            fired,
            DueStatus::DueCatchUp { missed_at: mk_now(2026, 7, 1, 10, 0) },
        );
    }

    #[test]
    fn overnight_window_wraps_midnight() {
        // Window 22:00–06:00; hourly every hour, last fired 23:00.
        let hb = mk_heartbeat(
            "hourly",
            r#"{"every_seconds":3600,"start":"22:00","end":"06:00"}"#,
            Some(&mk_now(2026, 7, 1, 23, 0).to_rfc3339()),
        );
        // 00:05 — occurrence 00:00 inside the wrapped window → fire.
        assert!(is_fire_now(&evaluate_with_now(&hb, mk_now(2026, 7, 2, 0, 5))));
        // 12:00 — outside the window → hold/not-yet, never fire.
        let noon = evaluate_with_now(&hb, mk_now(2026, 7, 2, 12, 0));
        assert!(!is_fire_now(&noon), "noon is outside 22:00–06:00, got {noon:?}");
    }

    #[test]
    fn malformed_window_is_invalid_not_always_open() {
        let hb = mk_heartbeat(
            "hourly",
            r#"{"every_seconds":60,"start":"9am","end":"17:00"}"#,
            Some(&(Local::now() - Duration::hours(1)).to_rfc3339()),
        );
        assert!(
            matches!(evaluate(&hb), DueStatus::Invalid { .. }),
            "a typo'd window must be a visible error, not silently always-open",
        );
    }

    // ── Manual-fire independence (latch removal) ──────────────────────

    #[test]
    fn manual_fire_before_slot_does_not_consume_scheduled_fire() {
        // Daily 09:00; manual launch stamped last_fired at 07:30 the
        // same day. The legacy once-per-day latch (scheduler.rs:122)
        // would block until tomorrow; croner semantics fire the 09:00
        // occurrence normally.
        let hb = mk_heartbeat(
            "daily",
            r#"{"time":"09:00"}"#,
            Some(&mk_now(2026, 7, 2, 7, 30).to_rfc3339()),
        );
        assert_eq!(
            evaluate_with_now(&hb, mk_now(2026, 7, 2, 9, 2)),
            DueStatus::Due { scheduled_for: mk_now(2026, 7, 2, 9, 0) },
            "a morning manual launch must not consume the day's scheduled fire",
        );
    }

    #[test]
    fn scheduled_fire_not_double_fired_after_it_ran() {
        // …and once the 09:00 fire has stamped last_fired, the next
        // occurrence is tomorrow — no double fire.
        let hb = mk_heartbeat(
            "daily",
            r#"{"time":"09:00"}"#,
            Some(&mk_now(2026, 7, 2, 9, 0).to_rfc3339()),
        );
        assert_eq!(
            evaluate_with_now(&hb, mk_now(2026, 7, 2, 9, 3)),
            DueStatus::NotYet { next: Some(mk_now(2026, 7, 3, 9, 0)) },
        );
    }

    // ── Invalid specs are loud ────────────────────────────────────────

    #[test]
    fn unknown_frequency_is_invalid() {
        let hb = mk_heartbeat("alien", r#"{"every_seconds":1}"#, None);
        assert!(matches!(evaluate(&hb), DueStatus::Invalid { .. }));
    }

    #[test]
    fn non_json_spec_is_invalid() {
        let hb = mk_heartbeat("daily", "{not json", None);
        assert!(matches!(evaluate(&hb), DueStatus::Invalid { .. }));
    }

    #[test]
    fn bad_time_is_invalid() {
        let hb = mk_heartbeat("daily", r#"{"time":"25:99"}"#, None);
        assert!(matches!(evaluate(&hb), DueStatus::Invalid { .. }));
    }

    #[test]
    fn impossible_date_is_invalid_not_silent() {
        // Feb 30 never occurs — croner's search horizon errors and we
        // surface it instead of ticking not_due forever.
        let hb = mk_heartbeat(
            "yearly",
            r#"{"time":"09:00","day_of_month":30,"month":2}"#,
            Some(&mk_now(2026, 1, 1, 9, 0).to_rfc3339()),
        );
        assert!(
            matches!(evaluate_with_now(&hb, mk_now(2026, 7, 2, 9, 0)), DueStatus::Invalid { .. }),
        );
    }

    #[test]
    fn hourly_defaults_to_3600_for_specless_rows() {
        // Legacy rows whose spec lacks every_seconds keep the 1h
        // default rather than turning Invalid.
        let hb = mk_heartbeat(
            "hourly",
            r#"{"garbage":true}"#,
            Some(&Local::now().to_rfc3339()),
        );
        assert!(matches!(evaluate(&hb), DueStatus::NotYet { .. }));
    }

    // ── Spec-shape coverage the legacy translator dropped ─────────────

    #[test]
    fn monthly_days_of_month_array_is_honored() {
        // UI-created monthly rows use `days_of_month`; the old
        // translator only read singular `day_of_month` and silently
        // defaulted to day 1.
        let hb = mk_heartbeat(
            "monthly",
            r#"{"time":"09:00","days_of_month":[10,20]}"#,
            Some(&mk_now(2026, 6, 20, 9, 0).to_rfc3339()),
        );
        assert_eq!(
            evaluate_with_now(&hb, mk_now(2026, 7, 10, 9, 1)),
            DueStatus::Due { scheduled_for: mk_now(2026, 7, 10, 9, 0) },
        );
        assert!(matches!(
            evaluate_with_now(&hb, mk_now(2026, 7, 9, 9, 1)),
            DueStatus::NotYet { .. },
        ));
    }

    #[test]
    fn ordinal_first_monday_translates() {
        // 2026-07-06 is the first Monday of July.
        let hb = mk_heartbeat(
            "monthly",
            r#"{"time":"09:00","ordinal":"first","ordinal_day":"mon"}"#,
            Some(&mk_now(2026, 6, 1, 9, 0).to_rfc3339()),
        );
        match evaluate_with_now(&hb, mk_now(2026, 7, 2, 9, 0)) {
            // June's first Monday (June 1) is after last_fired?  No —
            // last_fired IS June 1 09:00, so next occurrence = July 6.
            DueStatus::NotYet { next } => {
                assert_eq!(next, Some(mk_now(2026, 7, 6, 9, 0)));
            }
            other => panic!("expected NotYet(next=first Monday of July), got {other:?}"),
        }
    }

    #[test]
    fn yearly_months_array_is_honored() {
        let hb = mk_heartbeat(
            "yearly",
            r#"{"time":"09:00","months":["jul"],"days_of_month":[1]}"#,
            Some(&mk_now(2025, 7, 1, 9, 0).to_rfc3339()),
        );
        assert_eq!(
            evaluate_with_now(&hb, mk_now(2026, 7, 2, 12, 0)),
            DueStatus::DueCatchUp { missed_at: mk_now(2026, 7, 1, 9, 0) },
        );
    }

    // ── next_fire_time_after (status display helper) ──────────────────

    #[test]
    fn next_fire_time_after_daily() {
        let yesterday_9am = mk_now(2026, 5, 18, 9, 0);
        let hb = mk_heartbeat("daily", r#"{"time":"09:00"}"#, Some(&yesterday_9am.to_rfc3339()));
        let next = next_fire_time_after(&hb, yesterday_9am)
            .expect("daily schedule should parse via croner");
        assert_eq!(next, mk_now(2026, 5, 19, 9, 0));
    }
}
