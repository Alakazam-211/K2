//! 0.38.2 — cron-shaped heartbeat scheduling.
//!
//! Replaces the hand-rolled `is_past_deadline` + per-frequency arithmetic
//! that powered the heartbeat scheduler through 0.38.1. That code carried
//! a `starting_deadline_secs` grace window designed for small slips
//! (concurrency lock, brief crash); it could not handle long pauses —
//! once `elapsed` exceeded the grace, `lateness` grew unboundedly and
//! the heartbeat was permanently dark while the scheduler happily
//! ticked `skipped_deadline` every interval. Observed live: 11 workspace
//! `triage` heartbeats dark for 22+ days while reporting `enabled=yes`.
//!
//! New shape: ask `croner` what the next scheduled time is. If now is at
//! or past that time, fire. No deadline concept. No skip-because-late.
//! Long pauses recover automatically — after a 22-day gap the next
//! scheduled time is way in the past, `now >= next` is trivially true,
//! fire. Short slips are also handled identically — once due, fire.
//!
//! What we keep around our croner-backed helpers: schedule-window
//! guard (`should_project_fire`), concurrency policy, mode-off gating,
//! `heartbeat_fires` audit log, `last_fired` persistence. Those stay
//! exactly as they were. Only the "is it time?" math moved to croner.

use crate::db::schema::AgentHeartbeat;
use chrono::{DateTime, Local};
use croner::Cron;
use serde_json::Value;
use std::str::FromStr;

/// Returns true if this heartbeat is due to fire. Semantics:
///
/// - **Never fired** (`last_fired` is None): due now. A freshly enabled
///   heartbeat fires on the next scheduler tick rather than waiting for
///   its first scheduled slot (matches the pre-0.38.2 behavior).
/// - **Hourly** (`{every_seconds: N}`): due when
///   `now >= last_fired + every_seconds`.
/// - **Scheduled** (`daily|weekly|monthly|yearly`): translate the
///   `time` + day fields into a cron expression, ask croner for the
///   next occurrence after `last_fired`, return true if now is at
///   or past that occurrence.
///
/// Returns false (don't fire) when the spec can't be parsed — we
/// prefer a stuck-but-loud heartbeat over an unpredictable firing
/// schedule the operator can't reason about. Such cases land in the
/// audit log via the caller's wrapping logic.
pub fn is_due(hb: &AgentHeartbeat) -> bool {
    let now = Local::now();

    let Some(last_fired) = hb.last_fired.as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Local))
    else {
        // First fire — go now. Stamps `last_fired` for subsequent ticks.
        return true;
    };

    match next_fire_time_after(hb, last_fired) {
        Some(next) => now >= next,
        None => false,
    }
}

/// Compute when this heartbeat is *next* due, relative to a reference
/// time (typically `last_fired`). Returned in `Local` time so callers
/// don't have to convert.
///
/// `None` means the spec is unparseable or unsupported. Used by the
/// caller's audit log to record "not due yet — next at HH:MM" so the
/// operator can sanity-check schedules without grepping spec_json.
pub fn next_fire_time_after(
    hb: &AgentHeartbeat,
    after: DateTime<Local>,
) -> Option<DateTime<Local>> {
    let spec: Value = serde_json::from_str(&hb.spec_json).ok()?;

    // Normalize: daily/weekly/monthly/yearly all funnel through cron.
    // Hourly is a pure interval — `every_seconds` after `last_fired`.
    let mode = match hb.frequency.as_str() {
        "daily" | "weekly" | "monthly" | "yearly" | "scheduled" => "scheduled",
        other => other,
    };

    match mode {
        "hourly" => {
            let every_secs = spec
                .get("every_seconds")
                .and_then(|s| s.as_i64())
                .unwrap_or(3600);
            Some(after + chrono::Duration::seconds(every_secs))
        }
        "scheduled" => {
            let expr = build_cron_expression(&spec, &hb.frequency)?;
            let cron = Cron::from_str(expr.as_str()).ok()?;
            cron.find_next_occurrence(&after, false).ok()
        }
        _ => None,
    }
}

/// Translate K2SO's spec_json shape into a 5-field cron expression
/// (`minute hour day-of-month month day-of-week`). Returns `None` for
/// unsupported frequencies; callers fall through to "spec unparseable,
/// don't fire."
///
/// Supported spec shapes (preserved from the pre-0.38.2 hand-rolled logic
/// so existing rows continue to work without migration):
///
/// - `daily`:   `{ "time": "HH:MM" }`
/// - `weekly`:  `{ "time": "HH:MM", "days": ["MON","WED",...] }`
/// - `monthly`: `{ "time": "HH:MM", "day_of_month": 15 }`
/// - `yearly`:  `{ "time": "HH:MM", "day_of_month": 1, "month": 1 }`
fn build_cron_expression(spec: &Value, frequency: &str) -> Option<String> {
    let time_str = spec
        .get("time")
        .and_then(|s| s.as_str())
        .unwrap_or("09:00");
    let mut parts = time_str.split(':');
    let h: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;

    match frequency {
        "daily" => Some(format!("{m} {h} * * *")),
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
            Some(format!("{m} {h} * * {dow_part}"))
        }
        "monthly" => {
            let dom = spec
                .get("day_of_month")
                .and_then(|d| d.as_i64())
                .unwrap_or(1);
            Some(format!("{m} {h} {dom} * *"))
        }
        "yearly" => {
            let dom = spec.get("day_of_month").and_then(|d| d.as_i64()).unwrap_or(1);
            let month = spec.get("month").and_then(|d| d.as_i64()).unwrap_or(1);
            Some(format!("{m} {h} {dom} {month} *"))
        }
        // Unrecognized frequency — caller logs "spec unparseable."
        _ => None,
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
            created_at: 0,
            concurrency_policy: "forbid".to_string(),
            starting_deadline_secs: 600,
            active_deadline_secs: 30,
            in_flight_started_at: None,
            active_terminal_id: None,
            use_workspace_session: false,
        }
    }

    #[test]
    fn never_fired_is_due_immediately() {
        let hb = mk_heartbeat("hourly", r#"{"every_seconds":3600}"#, None);
        assert!(is_due(&hb), "fresh heartbeat without last_fired should be due");
    }

    #[test]
    fn hourly_fires_after_interval() {
        // last_fired 2 hours ago, every_seconds=3600 (1 hour) → due
        let two_hours_ago = (Local::now() - chrono::Duration::hours(2))
            .to_rfc3339();
        let hb = mk_heartbeat("hourly", r#"{"every_seconds":3600}"#, Some(&two_hours_ago));
        assert!(is_due(&hb));
    }

    #[test]
    fn hourly_not_yet_due_within_interval() {
        // last_fired 30 minutes ago, every_seconds=3600 → not due
        let half_hour_ago = (Local::now() - chrono::Duration::minutes(30))
            .to_rfc3339();
        let hb = mk_heartbeat("hourly", r#"{"every_seconds":3600}"#, Some(&half_hour_ago));
        assert!(!is_due(&hb));
    }

    #[test]
    fn hourly_recovers_from_22_day_pause() {
        // The bug we're closing: 22 days dark while enabled.
        // Pre-0.38.2: `lateness = 22d - 1h > 600s grace` → skip forever.
        // Post-0.38.2: `now >= last_fired + every_seconds` → fire.
        let way_ago = (Local::now() - chrono::Duration::days(22)).to_rfc3339();
        let hb = mk_heartbeat("hourly", r#"{"every_seconds":3600}"#, Some(&way_ago));
        assert!(is_due(&hb), "22-day-stale heartbeat must be due (this was the bug)");
    }

    #[test]
    fn daily_uses_cron_expression() {
        // last_fired yesterday at 09:00, spec "09:00 daily" → next fire today at 09:00.
        // If now is past 09:00 local → due; if before → not due.
        let yesterday_9am = Local
            .with_ymd_and_hms(2026, 5, 18, 9, 0, 0)
            .single()
            .unwrap();
        let hb = mk_heartbeat("daily", r#"{"time":"09:00"}"#, Some(&yesterday_9am.to_rfc3339()));
        let next = next_fire_time_after(&hb, yesterday_9am)
            .expect("daily schedule should parse via croner");
        assert_eq!(next.format("%H:%M").to_string(), "09:00");
        // Next fire is the next day at 09:00.
        assert!(next > yesterday_9am);
    }

    #[test]
    fn unparseable_spec_returns_not_due() {
        let hb = mk_heartbeat("hourly", r#"{"garbage":true}"#, Some(&Local::now().to_rfc3339()));
        // every_seconds defaults to 3600; fires after 1h — won't be due immediately.
        assert!(!is_due(&hb));
    }

    #[test]
    fn unknown_frequency_returns_not_due_via_none_next() {
        let now_str = Local::now().to_rfc3339();
        let hb = mk_heartbeat("alien", r#"{"every_seconds":1}"#, Some(&now_str));
        // Unknown frequency: next_fire_time_after returns None, is_due returns false.
        assert!(!is_due(&hb));
    }
}
