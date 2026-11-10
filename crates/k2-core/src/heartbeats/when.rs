//! When a one-time reminder fires: the one parser for `--in`, `--at`, `--on`
//! and `--tz` (prd-routines-reminders-and-agent-mail-v1 §4.2, vs-live W11).
//! The daemon resolves; the CLI only passes strings.
//!
//! | Flag | Meaning |
//! |---|---|
//! | `--in 45m`, `1h30m` | `s m h d w`, combined. At least 1 minute, at most 365 days. |
//! | `--at 13:00` | The next 13:00 in the zone: today if still ahead, else tomorrow. |
//! | `--at 2026-10-30T09:00` | That date and time in the zone. |
//! | `--at <RFC3339 with offset>` | That exact instant. The zone is ignored. |
//! | `--on 2026-10-30` | That date at 09:00 in the zone. |
//! | `--on 2026-10-30 --at 09:00`, `--on '2026-10-30 09:00'` | That date and time. |
//! | `--tz America/Denver` | IANA names only. |
//!
//! Zone order: `--tz`, then the owner's timezone, then this box's own clock.
//! A local time that doesn't exist (spring forward) moves to the first valid
//! minute after it; an ambiguous one (fall back) takes the earlier instant.
//! A time in the past is refused rather than firing at once.

use chrono::{DateTime, Duration, Local, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

/// `--in` floor: the due scan can't honour anything shorter.
pub const MIN_IN_SECS: i64 = 60;
/// `--in` ceiling: 365 days.
pub const MAX_IN_SECS: i64 = 365 * 24 * 60 * 60;
/// `--on` with no `--at`.
const DEFAULT_ON_TIME: (u32, u32) = (9, 0);

/// What the caller asked for, as strings.
#[derive(Debug, Clone, Copy, Default)]
pub struct WhenInput<'a> {
    pub in_: Option<&'a str>,
    pub at: Option<&'a str>,
    pub on: Option<&'a str>,
    pub tz: Option<&'a str>,
}

/// A resolved fire time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct When {
    pub fire_at: DateTime<Utc>,
    /// IANA zone, `None` when this box's own clock was used.
    pub tz: Option<String>,
    /// For the person: `Tomorrow 13:00 MDT`.
    pub display: String,
}

/// The zone a time is read in.
#[derive(Debug, Clone, Copy)]
enum Zone {
    Named(chrono_tz::Tz),
    Box,
}

fn pick_zone(tz: Option<&str>, owner_tz: Option<&str>) -> Result<Zone, String> {
    if let Some(t) = tz.map(str::trim).filter(|t| !t.is_empty()) {
        return t
            .parse::<chrono_tz::Tz>()
            .map(Zone::Named)
            .map_err(|_| format!("--tz '{t}' is not an IANA time zone (for example America/Denver)"));
    }
    // A stored owner zone is already validated at write; a bad one falls to
    // the box clock instead of refusing every reminder on this server.
    Ok(owner_tz
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .and_then(|t| t.parse::<chrono_tz::Tz>().ok())
        .map_or(Zone::Box, Zone::Named))
}

/// `Some(instant)` for a local time, applying the DST rules above.
fn instant_in<Z: TimeZone>(zone: &Z, local: NaiveDateTime) -> Option<DateTime<Utc>> {
    // A spring-forward gap is at most a few hours; step a minute at a time.
    for minute in 0..=(6 * 60) {
        let at = local + Duration::minutes(minute);
        match zone.from_local_datetime(&at) {
            LocalResult::Single(t) => return Some(t.with_timezone(&Utc)),
            LocalResult::Ambiguous(a, b) => return Some(a.min(b).with_timezone(&Utc)),
            LocalResult::None => {}
        }
    }
    None
}

fn to_instant(zone: Zone, local: NaiveDateTime) -> Option<DateTime<Utc>> {
    match zone {
        Zone::Named(z) => instant_in(&z, local),
        Zone::Box => instant_in(&Local, local),
    }
}

fn today_in(zone: Zone, now: DateTime<Utc>) -> NaiveDate {
    match zone {
        Zone::Named(z) => now.with_timezone(&z).date_naive(),
        Zone::Box => now.with_timezone(&Local).date_naive(),
    }
}

fn parse_hhmm(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M").or_else(|_| NaiveTime::parse_from_str(s, "%H:%M:%S")).ok()
}

/// `45m`, `1h30m`, `2d`, `1w`. `Err` on anything else.
pub fn parse_duration_secs(raw: &str) -> Result<i64, String> {
    let s = raw.trim().to_ascii_lowercase();
    let bad = || format!("'{raw}' is not a duration (use s, m, h, d or w: 45m, 1h30m, 2d)");
    if s.is_empty() {
        return Err(bad());
    }
    let (mut total, mut digits) = (0i64, String::new());
    for c in s.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let unit = match c {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            'w' => 604_800,
            _ => return Err(bad()),
        };
        let n: i64 = digits.parse().map_err(|_| bad())?;
        digits.clear();
        total = n.checked_mul(unit).and_then(|v| total.checked_add(v)).ok_or_else(bad)?;
    }
    // Trailing digits with no unit ("45") are not a duration.
    if !digits.is_empty() {
        return Err(bad());
    }
    Ok(total)
}

/// Resolve the flags against `now`. `Err` is the sentence the caller shows
/// (the CLI exits 2 on it).
pub fn resolve(input: WhenInput<'_>, owner_tz: Option<&str>, now: DateTime<Utc>) -> Result<When, String> {
    let zone = pick_zone(input.tz, owner_tz)?;
    let at = input.at.map(str::trim).filter(|s| !s.is_empty());
    let on = input.on.map(str::trim).filter(|s| !s.is_empty());
    let in_ = input.in_.map(str::trim).filter(|s| !s.is_empty());

    let fire_at = match (in_, at, on) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            return Err("use --in, or --at/--on, not both".to_string());
        }
        (None, None, None) => return Err("say when: --in 45m, --at 13:00 or --on 2026-10-30".to_string()),
        (Some(d), None, None) => {
            let secs = parse_duration_secs(d)?;
            if secs < MIN_IN_SECS {
                return Err(format!("--in {d} is too soon: at least 1 minute"));
            }
            if secs > MAX_IN_SECS {
                return Err(format!("--in {d} is too far off: at most 365 days"));
            }
            now + Duration::seconds(secs)
        }
        (None, at, on) => resolve_clock(at, on, zone, now)?,
    };
    let tz = match zone {
        Zone::Named(z) => Some(z.name().to_string()),
        Zone::Box => None,
    };
    let display = display_for(fire_at, zone, now);
    Ok(When { fire_at, tz, display })
}

fn resolve_clock(at: Option<&str>, on: Option<&str>, zone: Zone, now: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    let bad_at = |a: &str| format!("--at '{a}' is not a time (use 13:00, 2026-10-30T09:00 or an RFC 3339 time)");
    let past = |what: &str| format!("{what} is in the past");

    // `--on '2026-10-30 09:00'` carries both in one argument.
    let (on, at_from_on) = match on {
        Some(o) => match o.split_once(char::is_whitespace) {
            Some((d, t)) => (Some(d), Some(t.trim())),
            None => (Some(o), None),
        },
        None => (None, None),
    };
    if at.is_some() && at_from_on.is_some() {
        return Err("--on already has a time; drop --at or take the time off --on".to_string());
    }
    let at = at.or(at_from_on);

    if let Some(o) = on {
        let date = NaiveDate::parse_from_str(o, "%Y-%m-%d")
            .map_err(|_| format!("--on '{o}' is not a date (use 2026-10-30)"))?;
        let time = match at {
            None => NaiveTime::from_hms_opt(DEFAULT_ON_TIME.0, DEFAULT_ON_TIME.1, 0).expect("09:00 is a time"),
            Some(a) => parse_hhmm(a).ok_or_else(|| format!("--at '{a}' with --on must be a time of day (13:00)"))?,
        };
        let fire = to_instant(zone, date.and_time(time)).ok_or_else(|| "that local time doesn't exist".to_string())?;
        return if fire <= now { Err(past(o)) } else { Ok(fire) };
    }

    let a = at.ok_or_else(|| "say when: --in 45m, --at 13:00 or --on 2026-10-30".to_string())?;
    // An exact instant, with its own offset: the zone is ignored.
    if let Ok(exact) = DateTime::parse_from_rfc3339(a) {
        let fire = exact.with_timezone(&Utc);
        return if fire <= now { Err(past(a)) } else { Ok(fire) };
    }
    // A date and time in the zone.
    if let Some(naive) = ["%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S"]
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(a, f).ok())
    {
        let fire = to_instant(zone, naive).ok_or_else(|| "that local time doesn't exist".to_string())?;
        return if fire <= now { Err(past(a)) } else { Ok(fire) };
    }
    // Time of day: today if still ahead, else tomorrow.
    let time = parse_hhmm(a).ok_or_else(|| bad_at(a))?;
    let today = today_in(zone, now);
    let fire = to_instant(zone, today.and_time(time)).ok_or_else(|| "that local time doesn't exist".to_string())?;
    if fire > now {
        return Ok(fire);
    }
    let tomorrow = today.succ_opt().ok_or_else(|| bad_at(a))?;
    to_instant(zone, tomorrow.and_time(time)).ok_or_else(|| "that local time doesn't exist".to_string())
}

fn display_for(fire_at: DateTime<Utc>, zone: Zone, now: DateTime<Utc>) -> String {
    let (date, text) = match zone {
        Zone::Named(z) => {
            let t = fire_at.with_timezone(&z);
            (t.date_naive(), t.format("%H:%M %Z").to_string())
        }
        Zone::Box => {
            let t = fire_at.with_timezone(&Local);
            (t.date_naive(), t.format("%H:%M %Z").to_string())
        }
    };
    let today = today_in(zone, now);
    let day = if date == today {
        "Today".to_string()
    } else if today.succ_opt() == Some(date) {
        "Tomorrow".to_string()
    } else {
        date.format("%a %Y-%m-%d").to_string()
    };
    format!("{day} {text}")
}

/// `3h 10m`, for a late stamp. Under a minute reads `1m`.
pub fn human_span(secs: i64) -> String {
    let mins = (secs.max(0) / 60).max(1);
    let (d, h, m) = (mins / 1440, (mins % 1440) / 60, mins % 60);
    match (d, h, m) {
        (0, 0, m) => format!("{m}m"),
        (0, h, 0) => format!("{h}h"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, 0, _) => format!("{d}d"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).expect("test instant").with_timezone(&Utc)
    }

    fn resolve_in(input: WhenInput<'_>, now: &str) -> Result<When, String> {
        resolve(input, Some("America/Denver"), utc(now))
    }

    #[test]
    fn in_takes_combined_units_and_keeps_the_owner_zone() {
        let w = resolve_in(WhenInput { in_: Some("1h30m"), ..Default::default() }, "2026-06-10T18:00:00Z").expect("in");
        assert_eq!(w.fire_at, utc("2026-06-10T19:30:00Z"));
        assert_eq!(w.tz.as_deref(), Some("America/Denver"));
    }

    #[test]
    fn in_refuses_under_a_minute_and_over_a_year_and_nonsense() {
        for (raw, needle) in [("30s", "too soon"), ("366d", "too far"), ("45", "not a duration"), ("soon", "not a duration"), ("", "say when")] {
            let e = resolve_in(WhenInput { in_: Some(raw), ..Default::default() }, "2026-06-10T18:00:00Z")
                .expect_err(raw);
            assert!(e.contains(needle), "{raw}: {e}");
        }
    }

    #[test]
    fn at_past_today_is_tomorrow_and_ahead_today_is_today() {
        // 12:00 MDT now.
        let now = "2026-06-10T18:00:00Z";
        let later = resolve_in(WhenInput { at: Some("13:00"), ..Default::default() }, now).expect("today");
        assert_eq!(later.fire_at, utc("2026-06-10T19:00:00Z"));
        assert!(later.display.starts_with("Today 13:00 MDT"), "{}", later.display);
        let earlier = resolve_in(WhenInput { at: Some("11:00"), ..Default::default() }, now).expect("tomorrow");
        assert_eq!(earlier.fire_at, utc("2026-06-11T17:00:00Z"));
        assert!(earlier.display.starts_with("Tomorrow 11:00 MDT"), "{}", earlier.display);
    }

    #[test]
    fn on_alone_is_nine_in_the_zone() {
        let w = resolve_in(WhenInput { on: Some("2026-06-12"), ..Default::default() }, "2026-06-10T18:00:00Z").expect("on");
        assert_eq!(w.fire_at, utc("2026-06-12T15:00:00Z"));
    }

    #[test]
    fn on_with_at_in_either_spelling() {
        let now = "2026-06-10T18:00:00Z";
        let two = resolve_in(WhenInput { on: Some("2026-06-12"), at: Some("14:30"), ..Default::default() }, now).expect("two flags");
        let one = resolve_in(WhenInput { on: Some("2026-06-12 14:30"), ..Default::default() }, now).expect("one flag");
        assert_eq!(two.fire_at, utc("2026-06-12T20:30:00Z"));
        assert_eq!(one.fire_at, two.fire_at);
    }

    #[test]
    fn rfc3339_with_an_offset_ignores_tz() {
        let w = resolve(
            WhenInput { at: Some("2026-06-12T09:00:00-04:00"), tz: Some("Asia/Tokyo"), ..Default::default() },
            None,
            utc("2026-06-10T18:00:00Z"),
        )
        .expect("rfc3339");
        assert_eq!(w.fire_at, utc("2026-06-12T13:00:00Z"));
    }

    #[test]
    fn tz_beats_the_owner_zone() {
        let w = resolve(
            WhenInput { at: Some("2026-06-12T09:00"), tz: Some("Asia/Tokyo"), ..Default::default() },
            Some("America/Denver"),
            utc("2026-06-10T18:00:00Z"),
        )
        .expect("tz");
        assert_eq!(w.fire_at, utc("2026-06-12T00:00:00Z"));
        assert_eq!(w.tz.as_deref(), Some("Asia/Tokyo"));
    }

    #[test]
    fn a_bad_tz_is_refused_and_not_guessed() {
        let e = resolve(WhenInput { in_: Some("5m"), tz: Some("Mars/Base"), ..Default::default() }, None, utc("2026-06-10T18:00:00Z"))
            .expect_err("bad tz");
        assert!(e.contains("IANA"), "{e}");
    }

    #[test]
    fn past_absolute_times_are_refused() {
        let now = "2026-06-10T18:00:00Z";
        for input in [
            WhenInput { at: Some("2026-06-10T01:00"), ..Default::default() },
            WhenInput { on: Some("2026-06-01"), ..Default::default() },
            WhenInput { at: Some("2026-06-01T00:00:00Z"), ..Default::default() },
        ] {
            let e = resolve_in(input, now).expect_err("past");
            assert!(e.contains("past"), "{e}");
        }
    }

    #[test]
    fn spring_forward_gap_moves_to_the_first_valid_minute() {
        // 02:30 on 2026-03-08 doesn't exist in Denver (02:00 → 03:00).
        let w = resolve_in(WhenInput { at: Some("2026-03-08T02:30"), ..Default::default() }, "2026-03-01T00:00:00Z").expect("gap");
        assert_eq!(w.fire_at, utc("2026-03-08T09:00:00Z"), "the first valid minute after the gap, 03:00 MDT");
    }

    #[test]
    fn fall_back_overlap_takes_the_earlier_instant() {
        // 01:30 on 2026-11-01 happens twice in Denver; MDT (-06:00) is first.
        let w = resolve_in(WhenInput { at: Some("2026-11-01T01:30"), ..Default::default() }, "2026-10-20T00:00:00Z").expect("overlap");
        assert_eq!(w.fire_at, utc("2026-11-01T07:30:00Z"));
    }

    #[test]
    fn in_and_at_together_are_refused() {
        let e = resolve_in(WhenInput { in_: Some("5m"), at: Some("13:00"), ..Default::default() }, "2026-06-10T18:00:00Z").expect_err("both");
        assert!(e.contains("not both"), "{e}");
    }

    #[test]
    fn human_span_reads_cleanly() {
        assert_eq!(human_span(11_400), "3h 10m");
        assert_eq!(human_span(7200), "2h");
        assert_eq!(human_span(30), "1m");
        assert_eq!(human_span(90_000), "1d 1h");
    }
}
