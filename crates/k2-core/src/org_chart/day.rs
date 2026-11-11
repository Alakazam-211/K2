//! Days and nights in the owner's timezone (prd-org-chart-v1 §3, §5;
//! Rosson 2026-11-10, answers 2 and 7).
//!
//! - **The night** of date N runs from N 22:00 to the next day's start
//!   (`dayStartsAt`, 06:00 by default). The three night times (`runAt`,
//!   `lastCallAt`, `dayStartsAt`) all fall inside 22:00–07:00, in that
//!   order. Each is placed on night N by its offset from 22:00, so 23:30 is
//!   N 23:30 and 01:00 is (N+1) 01:00.
//! - **A day** (for the once-a-day read marker) starts at `dayStartsAt`
//!   and is named by the date it starts on. Before 06:00 it is still
//!   yesterday, so an agent that read it already sees nothing new.
//! - **The zone** is the owner's (Settings → General), else this box's
//!   clock. Times compare as local wall-clock times, so a DST night moves
//!   with the clock on the wall.

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc};

/// The window the night times must fall in.
pub const WINDOW_START: (u32, u32) = (22, 0);
/// Minutes from 22:00 to 07:00.
pub const WINDOW_MINUTES: i64 = 9 * 60;

/// The three night times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NightTimes {
    pub run_at: NaiveTime,
    pub last_call_at: NaiveTime,
    pub day_starts_at: NaiveTime,
}

impl Default for NightTimes {
    fn default() -> Self {
        NightTimes { run_at: hm(1, 0), last_call_at: hm(4, 30), day_starts_at: hm(6, 0) }
    }
}

fn hm(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).unwrap_or(NaiveTime::MIN)
}

/// `HH:MM` (24-hour).
pub fn parse_hhmm(raw: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(raw.trim(), "%H:%M").ok()
}

/// `HH:MM`.
pub fn fmt_hhmm(t: NaiveTime) -> String {
    t.format("%H:%M").to_string()
}

/// Minutes after 22:00 (0 = 22:00, 540 = 07:00, 1439 = 21:59).
pub fn offset_minutes(t: NaiveTime) -> i64 {
    let m = i64::from(t.hour()) * 60 + i64::from(t.minute());
    (m - i64::from(WINDOW_START.0) * 60).rem_euclid(24 * 60)
}

impl NightTimes {
    /// From the three `HH:MM` strings, checked. `Err` is the sentence.
    pub fn parse(run_at: &str, last_call_at: &str, day_starts_at: &str) -> Result<NightTimes, String> {
        let p = |name: &str, v: &str| parse_hhmm(v).ok_or_else(|| format!("{name} '{v}' is not a time (use HH:MM, 24-hour)"));
        let t = NightTimes {
            run_at: p("runAt", run_at)?,
            last_call_at: p("lastCallAt", last_call_at)?,
            day_starts_at: p("dayStartsAt", day_starts_at)?,
        };
        t.validate()?;
        Ok(t)
    }

    /// All three within 22:00–07:00, in order: run, last call, day start.
    pub fn validate(&self) -> Result<(), String> {
        for (name, t) in [("runAt", self.run_at), ("lastCallAt", self.last_call_at), ("dayStartsAt", self.day_starts_at)] {
            if offset_minutes(t) > WINDOW_MINUTES {
                return Err(format!("{name} {} is outside the night window (22:00–07:00)", fmt_hhmm(t)));
            }
        }
        let (r, l, s) = (offset_minutes(self.run_at), offset_minutes(self.last_call_at), offset_minutes(self.day_starts_at));
        if !(r < l && l < s) {
            return Err(format!(
                "the night times go in order within 22:00–07:00: runAt ({}) before lastCallAt ({}) before dayStartsAt ({})",
                fmt_hhmm(self.run_at),
                fmt_hhmm(self.last_call_at),
                fmt_hhmm(self.day_starts_at)
            ));
        }
        Ok(())
    }

    /// From the stored setting; a bad stored value falls back to the
    /// defaults (the write path refuses bad values).
    pub fn from_strings_or_default(run_at: &str, last_call_at: &str, day_starts_at: &str) -> NightTimes {
        NightTimes::parse(run_at, last_call_at, day_starts_at).unwrap_or_default()
    }
}

/// The zone times are read in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zone {
    Named(chrono_tz::Tz),
    Box,
}

/// The owner's IANA zone, else this box's clock (a bad stored name too).
pub fn zone(owner_tz: Option<&str>) -> Zone {
    owner_tz
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .and_then(|t| t.parse::<chrono_tz::Tz>().ok())
        .map_or(Zone::Box, Zone::Named)
}

/// `now` on the zone's wall clock.
pub fn local(zone: Zone, now: DateTime<Utc>) -> NaiveDateTime {
    match zone {
        Zone::Named(z) => now.with_timezone(&z).naive_local(),
        Zone::Box => now.with_timezone(&Local).naive_local(),
    }
}

/// Night N's 22:00.
pub fn night_base(night: NaiveDate) -> NaiveDateTime {
    night.and_time(hm(WINDOW_START.0, WINDOW_START.1))
}

/// Time `t` on night N (by its offset from 22:00).
pub fn on_night(night: NaiveDate, t: NaiveTime) -> NaiveDateTime {
    night_base(night) + Duration::minutes(offset_minutes(t))
}

/// The read marker's day at `local_now`: the date of the latest day start
/// at or before it.
pub fn read_day_at(times: &NightTimes, local_now: NaiveDateTime) -> NaiveDate {
    let d = local_now.date();
    for back in 0..3 {
        let n = d - Duration::days(back);
        let start = on_night(n, times.day_starts_at);
        if start <= local_now {
            return start.date();
        }
    }
    d - Duration::days(1)
}

/// The day that started most recently (zone + `now`).
pub fn read_day(zone: Zone, times: &NightTimes, now: DateTime<Utc>) -> NaiveDate {
    read_day_at(times, local(zone, now))
}

/// When `day` started, as a wall-clock time.
pub fn day_start(times: &NightTimes, day: NaiveDate) -> NaiveDateTime {
    // The night whose day start lands on `day`.
    for back in 0..2 {
        let n = day - Duration::days(back);
        let s = on_night(n, times.day_starts_at);
        if s.date() == day {
            return s;
        }
    }
    day.and_time(times.day_starts_at)
}

/// The night `local_now` is in (N 22:00 ≤ now < the day start), if any.
pub fn current_night_at(times: &NightTimes, local_now: NaiveDateTime) -> Option<NaiveDate> {
    let d = local_now.date();
    [d, d - Duration::days(1)]
        .into_iter()
        .find(|n| night_base(*n) <= local_now && local_now < on_night(*n, times.day_starts_at))
}

/// `YYYY-MM-DD`.
pub fn day_key(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// `Tue 11-10`.
pub fn day_label(d: NaiveDate) -> String {
    d.format("%a %m-%d").to_string()
}

/// `11-10`.
pub fn short_date(d: NaiveDate) -> String {
    d.format("%m-%d").to_string()
}

/// A stored unix time on the zone's wall clock.
pub fn local_of(zone: Zone, unix: i64) -> Option<NaiveDateTime> {
    DateTime::<Utc>::from_timestamp(unix, 0).map(|t| local(zone, t))
}

/// The zone's name for people (`America/Denver`, or `this server's clock`).
pub fn zone_name(zone: Zone) -> String {
    match zone {
        Zone::Named(z) => z.name().to_string(),
        Zone::Box => "this server's clock".to_string(),
    }
}

// ── the night run's spread (Rosson 2026-11-10: "stagger it so we don't
// hit the computer with a ton of work all at the same time") ──────────
//
// Every org's head gets its own start time between `runAt` and
// `lastCallAt` less a margin (01:00–04:00 by default). The time is a
// stable hash of the org's id, so it is the same every night and after a
// restart, and never before `runAt`. The daemon's wake governor then
// starts only as many at once as the computer can take; a head still
// waiting when the spread ends is skipped for the night (its bulletin
// stays, marked "not updated since …"), never run late.

/// The spread ends this long before last call (less when the night times
/// are close together: never more than half the gap).
pub const SPREAD_MARGIN_MINUTES: i64 = 30;

// The other pacing defaults, all here. A wake's time cap is when its
// governor slot ends even if the agent is still working.

/// A head's night run holds its slot until it writes its bulletin, goes
/// idle, or this long.
pub const RUN_CAP_SECS: i64 = 20 * 60;
/// A last call's cap.
pub const LAST_CALL_CAP_SECS: i64 = 10 * 60;
/// A member's ask holds its slot until it answers, goes idle, or this long.
pub const ASK_CAP_SECS: i64 = 10 * 60;
/// `ask` goes to one member at a time, about this far apart…
pub const ASK_GAP_SECS: i64 = 120;
/// …plus a stable jitter of up to this much.
pub const ASK_JITTER_SECS: i64 = 30;
/// A copy still queued this long after the ask is dropped (not news by
/// then); the head sees it under "not delivered".
pub const ASK_QUEUE_TTL_SECS: i64 = 3 * 3600;

/// When the copy at `pos` in its ask may go out: the first at once, then
/// one about every [`ASK_GAP_SECS`] (jittered by its id), and never sooner
/// than that after the copy before it went out (`prev_sent_at`).
pub fn ask_due(created_at: i64, pos: usize, copy_id: &str, prev_sent_at: Option<i64>) -> i64 {
    if pos == 0 {
        return created_at;
    }
    let gap = ASK_GAP_SECS + (stable_hash(copy_id) % ASK_JITTER_SECS as u64) as i64;
    let planned = created_at + pos as i64 * ASK_GAP_SECS + (gap - ASK_GAP_SECS);
    prev_sent_at.map_or(planned, |p| planned.max(p + gap))
}

/// FNV-1a (64-bit): stable across builds and platforms, unlike std's
/// hasher. Spreads work; never used for security.
pub fn stable_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

impl NightTimes {
    /// Minutes from `runAt` to the end of the spread (at least 1).
    pub fn spread_minutes(&self) -> i64 {
        let gap = offset_minutes(self.last_call_at) - offset_minutes(self.run_at);
        let margin = SPREAD_MARGIN_MINUTES.min(gap / 2);
        (gap - margin).max(1)
    }

    /// When night N's spread ends: no night run starts at or after it.
    pub fn spread_end(&self, night: NaiveDate) -> NaiveDateTime {
        on_night(night, self.run_at) + Duration::minutes(self.spread_minutes())
    }

    /// The org's stable start, in seconds after `runAt` (0 ≤ x < spread).
    pub fn run_offset_secs(&self, org_id: &str) -> i64 {
        let span = (self.spread_minutes() * 60).max(1) as u64;
        (stable_hash(org_id) % span) as i64
    }

    /// When the org's head is due on night N (never before `runAt`).
    pub fn run_due(&self, night: NaiveDate, org_id: &str) -> NaiveDateTime {
        on_night(night, self.run_at) + Duration::seconds(self.run_offset_secs(org_id))
    }
}

/// The current night, or during the day the night that just ended (for
/// "how did tonight go").
pub fn latest_night_at(times: &NightTimes, local_now: NaiveDateTime) -> NaiveDate {
    current_night_at(times, local_now).unwrap_or_else(|| read_day_at(times, local_now) - Duration::days(1))
}
