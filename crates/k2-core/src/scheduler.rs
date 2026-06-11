//! Pure heartbeat-schedule evaluation.
//!
//! Given a project's `heartbeat_mode` + `heartbeat_schedule` JSON + the
//! RFC3339 string of its last fire, decide whether the scheduler tick
//! should fire this project now. No DB, no Tauri, no IO — just string /
//! JSON / chrono math — so the logic is testable in isolation and runs
//! identically inside the Tauri app today and the k2so-daemon tomorrow.
//!
//! Previously lived inline at `src-tauri/src/commands/k2so_agents.rs`
//! next to the `scheduler_tick` Tauri command. Extracted here so the
//! daemon can call it directly without pulling in the rest of the
//! commands module.
//!
//! Callers still take `chrono::Local::now()` through the convenience
//! [`should_project_fire`] entry point; tests go through
//! [`should_project_fire_with_now`] to inject a deterministic clock.

use chrono::{DateTime, Datelike, Local, NaiveDate, Timelike};

/// Parse `HH:MM` → minute-of-day (0..=1439). Returns `None` on malformed input.
fn parse_hhmm_mins(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 2 {
        let h: u32 = parts[0].parse().ok()?;
        let m: u32 = parts[1].parse().ok()?;
        Some(h * 60 + m)
    } else {
        None
    }
}

/// Public entry point used at runtime — reads wall-clock time via
/// `chrono::Local::now()`. For deterministic testing use
/// [`should_project_fire_with_now`] directly.
pub fn should_project_fire(
    mode: &str,
    schedule_json: Option<&str>,
    last_fire: Option<&str>,
) -> bool {
    should_project_fire_with_now(mode, schedule_json, last_fire, Local::now())
}

/// Core evaluation with an explicit `now`. Everything else in this
/// module delegates here. Unit tests inject `now` directly so behavior
/// is reproducible.
pub fn should_project_fire_with_now(
    mode: &str,
    schedule_json: Option<&str>,
    last_fire: Option<&str>,
    now: DateTime<Local>,
) -> bool {
    let last_fire_time =
        last_fire.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());

    // P5.5: normalize legacy `agent_heartbeats.frequency` values that
    // were stored as the inner JSON frequency (`daily`, `weekly`,
    // `monthly`, `yearly`) when the outer mode column should be
    // `scheduled`. This was a 0.32-era migration mistake — the row
    // shape is { frequency: "daily", spec_json: {"frequency":"daily",...} }
    // and the scheduler's match arm only knew "hourly" / "scheduled".
    // Rather than churn data with a backfill migration, alias at the
    // boundary so both old rows and new "scheduled" rows work.
    // Without this, daily/weekly/monthly/yearly heartbeats never
    // fired from cron (they were silently `_ => false`-d).
    let mode = match mode {
        "daily" | "weekly" | "monthly" | "yearly" => "scheduled",
        other => other,
    };

    match mode {
        "hourly" => {
            // {"start":"09:00","end":"17:00","every_seconds":1800}
            let Some(json_str) = schedule_json else { return false };
            let v: serde_json::Value = match serde_json::from_str(json_str) {
                Ok(v) => v,
                Err(_) => return false,
            };

            let start = v.get("start").and_then(|s| s.as_str()).unwrap_or("00:00");
            let end = v.get("end").and_then(|s| s.as_str()).unwrap_or("23:59");
            let every_secs = v.get("every_seconds").and_then(|s| s.as_u64()).unwrap_or(300);

            let now_mins = now.hour() * 60 + now.minute();
            let start_mins = parse_hhmm_mins(start).unwrap_or(0);
            let end_mins = parse_hhmm_mins(end).unwrap_or(1439);

            let in_window = if start_mins <= end_mins {
                now_mins >= start_mins && now_mins < end_mins
            } else {
                // Overnight windows (e.g. 22:00–06:00).
                now_mins >= start_mins || now_mins < end_mins
            };
            if !in_window {
                return false;
            }

            match last_fire_time {
                Some(lf) => {
                    let elapsed = (now.timestamp() - lf.timestamp()) as u64;
                    elapsed >= every_secs
                }
                None => true,
            }
        }
        "scheduled" => {
            let Some(json_str) = schedule_json else { return false };
            let v: serde_json::Value = match serde_json::from_str(json_str) {
                Ok(v) => v,
                Err(_) => return false,
            };

            let frequency = v.get("frequency").and_then(|s| s.as_str()).unwrap_or("daily");
            let time_str = v.get("time").and_then(|s| s.as_str()).unwrap_or("09:00");
            let schedule_mins = parse_hhmm_mins(time_str).unwrap_or(540);
            let now_mins = now.hour() * 60 + now.minute();

            if now_mins < schedule_mins {
                return false;
            }

            // Already-fired-today guards.
            if let Some(lf) = &last_fire_time {
                let lf_local = lf.with_timezone(&Local);
                if lf_local.date_naive() == now.date_naive() && frequency == "daily" {
                    return false;
                }
                if lf_local.date_naive() == now.date_naive() {
                    return false;
                }
            }

            match frequency {
                "daily" => {
                    let interval = v
                        .get("interval")
                        .and_then(|s| s.as_u64())
                        .unwrap_or(1);
                    if interval > 1 {
                        let day_of_year = now.ordinal() as u64;
                        if day_of_year % interval != 0 {
                            return false;
                        }
                    }
                    true
                }
                "weekly" => {
                    let days = v.get("days").and_then(|d| d.as_array());
                    let weekday = weekday_short(now.weekday());
                    match days {
                        Some(day_arr) => day_arr.iter().any(|d| d.as_str() == Some(weekday)),
                        None => true,
                    }
                }
                "monthly" => {
                    let day_of_month = now.day();
                    if let Some(days_arr) =
                        v.get("days_of_month").and_then(|d| d.as_array())
                    {
                        return days_arr
                            .iter()
                            .any(|d| d.as_u64() == Some(day_of_month as u64));
                    }
                    if let Some(ordinal) = v.get("ordinal").and_then(|s| s.as_str()) {
                        let ordinal_day = v
                            .get("ordinal_day")
                            .and_then(|s| s.as_str())
                            .unwrap_or("day");
                        return matches_ordinal_day(now.date_naive(), ordinal, ordinal_day);
                    }
                    true
                }
                "yearly" => {
                    let month_name = match now.month() {
                        1 => "jan",
                        2 => "feb",
                        3 => "mar",
                        4 => "apr",
                        5 => "may",
                        6 => "jun",
                        7 => "jul",
                        8 => "aug",
                        9 => "sep",
                        10 => "oct",
                        11 => "nov",
                        12 => "dec",
                        _ => return false,
                    };
                    let months = v.get("months").and_then(|d| d.as_array());
                    match months {
                        Some(m_arr) => m_arr.iter().any(|m| m.as_str() == Some(month_name)),
                        None => true,
                    }
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn weekday_short(w: chrono::Weekday) -> &'static str {
    match w {
        chrono::Weekday::Mon => "mon",
        chrono::Weekday::Tue => "tue",
        chrono::Weekday::Wed => "wed",
        chrono::Weekday::Thu => "thu",
        chrono::Weekday::Fri => "fri",
        chrono::Weekday::Sat => "sat",
        chrono::Weekday::Sun => "sun",
    }
}

/// Returns `true` if `date` is the Nth (or last) occurrence of
/// `day_type` in its month. Exposed publicly so the CLI's heartbeat
/// preview + the daemon's own dry-run can share one implementation.
pub fn matches_ordinal_day(date: NaiveDate, ordinal: &str, day_type: &str) -> bool {
    let dom = date.day();
    let weekday = date.weekday();

    let day_matches = match day_type {
        "day" => true,
        "weekday" => weekday != chrono::Weekday::Sat && weekday != chrono::Weekday::Sun,
        "mon" | "monday" => weekday == chrono::Weekday::Mon,
        "tue" | "tuesday" => weekday == chrono::Weekday::Tue,
        "wed" | "wednesday" => weekday == chrono::Weekday::Wed,
        "thu" | "thursday" => weekday == chrono::Weekday::Thu,
        "fri" | "friday" => weekday == chrono::Weekday::Fri,
        "sat" | "saturday" => weekday == chrono::Weekday::Sat,
        "sun" | "sunday" => weekday == chrono::Weekday::Sun,
        _ => true,
    };
    if !day_matches {
        return false;
    }

    match ordinal {
        "first" => dom <= 7,
        "second" => dom > 7 && dom <= 14,
        "third" => dom > 14 && dom <= 21,
        "fourth" => dom > 21 && dom <= 28,
        "last" => {
            let days_in_month = if date.month() == 12 {
                NaiveDate::from_ymd_opt(date.year() + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(date.year(), date.month() + 1, 1)
            }
            .map(|d| d.pred_opt().map(|p| p.day()).unwrap_or(28))
            .unwrap_or(28);
            dom + 7 > days_in_month
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn mk_now(year: i32, month: u32, day: u32, h: u32, m: u32) -> DateTime<Local> {
        // Build directly in Local so `now.hour()` returns the
        // intended hour regardless of the test runner's timezone.
        // Earlier versions built UTC then "coerced" via
        // `from_utc_datetime`, which actually time-shifts (e.g. in
        // MDT 18:00 UTC becomes 12:00 Local) and made the
        // `hourly_outside_window_does_not_fire` test silently pass
        // only in UTC-aligned runners.
        Local
            .with_ymd_and_hms(year, month, day, h, m, 0)
            .single()
            .expect("test datetime must be unambiguous")
    }

    #[test]
    fn mode_off_never_fires() {
        let now = mk_now(2026, 4, 19, 12, 0);
        assert!(!should_project_fire_with_now("off", None, None, now));
        assert!(!should_project_fire_with_now("off", Some("{}"), None, now));
    }

    #[test]
    fn invalid_json_never_fires() {
        let now = mk_now(2026, 4, 19, 12, 0);
        assert!(!should_project_fire_with_now("hourly", Some("{not json"), None, now));
        assert!(!should_project_fire_with_now("scheduled", Some("{not json"), None, now));
    }

    #[test]
    fn hourly_no_schedule_never_fires() {
        let now = mk_now(2026, 4, 19, 12, 0);
        assert!(!should_project_fire_with_now("hourly", None, None, now));
    }

    #[test]
    fn hourly_outside_window_does_not_fire() {
        // Window 09:00–17:00, now is 18:00.
        let now = mk_now(2026, 4, 19, 18, 0);
        assert!(!should_project_fire_with_now(
            "hourly",
            Some(r#"{"start":"09:00","end":"17:00","every_seconds":1800}"#),
            None,
            now
        ));
    }

    #[test]
    fn daily_mode_aliases_to_scheduled_and_fires() {
        // Pre-P5.5 regression: agent_heartbeats rows with frequency='daily'
        // never fired because scheduler's match arm only handled 'hourly'
        // and 'scheduled'. P5.5 aliases daily/weekly/monthly/yearly to
        // 'scheduled' at the boundary so legacy data + the daemon's
        // tick converge.
        // Pick a time well after schedule so all timezones are past 09:00.
        let now = mk_now(2026, 4, 19, 23, 0);
        assert!(should_project_fire_with_now(
            "daily",
            Some(r#"{"frequency":"daily","interval":1,"time":"09:00"}"#),
            None,
            now
        ), "daily-as-mode should alias to scheduled and fire after 9 AM");
    }

    #[test]
    fn weekly_mode_aliases_to_scheduled() {
        // 2026-04-20 is a Monday at UTC 23:00 (well past 09:00 in any tz).
        let now = mk_now(2026, 4, 20, 23, 0);
        assert!(should_project_fire_with_now(
            "weekly",
            Some(r#"{"frequency":"weekly","time":"09:00","days":["mon","wed","fri"]}"#),
            None,
            now
        ), "weekly-as-mode should alias to scheduled");
    }

    #[test]
    fn hourly_in_window_fires_when_never_fired() {
        // mk_now returns Local from UTC. Need a time that maps to
        // inside the window regardless of local offset — pick a 12h
        // window covering most offsets.
        let now = mk_now(2026, 4, 19, 12, 0);
        assert!(should_project_fire_with_now(
            "hourly",
            Some(r#"{"start":"00:00","end":"23:59","every_seconds":60}"#),
            None,
            now
        ));
    }

    #[test]
    fn ordinal_first_monday_matches_first_week() {
        // 2026-04-06 is the first Monday of April.
        let d = NaiveDate::from_ymd_opt(2026, 4, 6).unwrap();
        assert!(matches_ordinal_day(d, "first", "monday"));
        // 2026-04-13 is second Monday.
        let d = NaiveDate::from_ymd_opt(2026, 4, 13).unwrap();
        assert!(!matches_ordinal_day(d, "first", "monday"));
        assert!(matches_ordinal_day(d, "second", "monday"));
    }

    #[test]
    fn ordinal_last_is_any_day_in_final_week() {
        // April has 30 days — last week is days 24–30.
        let d = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();
        assert!(matches_ordinal_day(d, "last", "tuesday"));
        // April 21 is a Tuesday but not the LAST tuesday (28th is).
        let d = NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        assert!(!matches_ordinal_day(d, "last", "tuesday"));
    }

    #[test]
    fn ordinal_weekday_type_excludes_weekends() {
        // 2026-04-04 is a Saturday.
        let d = NaiveDate::from_ymd_opt(2026, 4, 4).unwrap();
        assert!(!matches_ordinal_day(d, "first", "weekday"));
        // 2026-04-06 is a Monday, first weekday of the month.
        let d = NaiveDate::from_ymd_opt(2026, 4, 6).unwrap();
        assert!(matches_ordinal_day(d, "first", "weekday"));
    }
}
