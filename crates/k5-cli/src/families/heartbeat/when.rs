//! The times the shell's heartbeat readers print (`cli/src/heartbeat.sh`'s
//! `_HB_WHEN_PY` and `cli/src/remind.sh`'s list): Python 3.12's
//! `datetime.fromisoformat`, `.astimezone()` (this computer's zone, by the
//! C library's `localtime_r`, as Python's is) and
//! `.astimezone(ZoneInfo(tz))` (the zone's TZif file, read by the C
//! library with `TZ` pointing at it, as `zoneinfo` finds it on `TZPATH`).
//!
//! An `Err` is the exception Python raises, as `Class: message`.

use crate::out::json::Value;
use crate::out::py;

/// A `datetime` as `fromisoformat` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dt {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// The UTC offset in whole seconds; `None`: a naive time.
    pub offset: Option<i64>,
}

fn at(b: &[u8], i: usize) -> u8 {
    b.get(i).copied().unwrap_or(0)
}

/// `parse_digits`: exactly `n` digits at `*p`, or `None`.
fn digits(b: &[u8], p: &mut usize, n: usize) -> Option<i64> {
    let mut v = 0i64;
    for _ in 0..n {
        let c = at(b, *p);
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + i64::from(c - b'0');
        *p += 1;
    }
    Some(v)
}

/// `_find_isoformat_datetime_separator`.
fn separator(b: &[u8]) -> usize {
    let len = b.len();
    if len == 7 {
        return 7;
    }
    if at(b, 4) == b'-' {
        if at(b, 5) == b'W' {
            if len < 8 {
                return usize::MAX;
            }
            if len > 8 && at(b, 8) == b'-' {
                if len == 9 {
                    return usize::MAX;
                }
                if len > 10 && at(b, 10).is_ascii_digit() {
                    return 8;
                }
                return 10;
            }
            return 8;
        }
        return 10;
    }
    if at(b, 4) == b'W' {
        let mut idx = 7;
        while idx < len && at(b, idx).is_ascii_digit() {
            idx += 1;
        }
        if idx < 9 {
            return idx;
        }
        return if idx % 2 == 0 { 7 } else { 8 };
    }
    8
}

fn leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap(y) => 29,
        _ => 28,
    }
}

/// Days from 1970-01-01 to `y-m-d` (proleptic Gregorian).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `days` after 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `parse_isoformat_date`: the date, or `None` for a `ValueError`.
fn date(b: &[u8]) -> Option<(i64, u32, u32)> {
    let mut p = 0;
    let year = digits(b, &mut p, 4)?;
    let sep = at(b, p) == b'-';
    if sep {
        p += 1;
    }
    if at(b, p) == b'W' {
        p += 1;
        let week = digits(b, &mut p, 2)?;
        let day = if p < b.len() {
            if sep {
                if at(b, p) != b'-' {
                    return None;
                }
                p += 1;
            }
            digits(b, &mut p, 1)?
        } else {
            1
        };
        let jan1 = days_from_civil(year, 1, 1);
        // 1970-01-01 was a Thursday: Monday is 0.
        let first_weekday = (jan1 + 3).rem_euclid(7);
        if !(1..53).contains(&week)
            && !(week == 53 && (first_weekday == 3 || (first_weekday == 2 && leap(year))))
        {
            return None;
        }
        if !(1..8).contains(&day) {
            return None;
        }
        let mut monday = jan1 - first_weekday;
        if first_weekday > 3 {
            monday += 7;
        }
        let (y, m, d) = civil_from_days(monday + (week - 1) * 7 + day - 1);
        return Some((y, m, d));
    }
    let month = digits(b, &mut p, 2)?;
    if sep {
        if at(b, p) != b'-' {
            return None;
        }
        p += 1;
    }
    let day = digits(b, &mut p, 2)?;
    Some((year, month as u32, day as u32))
}

/// `parse_hh_mm_ss_ff` over `b[from..to]`: the time and whether something
/// is left (`Err`: the parse failed).
fn hh_mm_ss(b: &[u8], from: usize, to: usize) -> Result<([i64; 3], i64, bool), ()> {
    let mut vals = [0i64; 3];
    let mut p = from;
    let mut has_sep = true;
    for (i, v) in vals.iter_mut().enumerate() {
        *v = digits(b, &mut p, 2).ok_or(())?;
        let c = at(b, p);
        p += 1;
        if i == 0 {
            has_sep = c == b':';
        }
        if p >= to {
            return Ok((vals, 0, c != 0));
        } else if has_sep && c == b':' {
            continue;
        } else if c == b'.' || c == b',' {
            break;
        } else if !has_sep {
            p -= 1;
        } else {
            return Err(());
        }
    }
    // The fraction (C parses it whenever the loop ends without returning).
    let take = to.saturating_sub(p).min(6);
    if take == 0 {
        return Err(());
    }
    let us = digits(b, &mut p, take).ok_or(())? * [100_000, 10_000, 1000, 100, 10, 1][take - 1];
    while at(b, p).is_ascii_digit() {
        p += 1;
    }
    Ok((vals, us, at(b, p) != 0))
}

/// `datetime.fromisoformat(s)`: `None` where Python raises `ValueError`.
/// (An hour of 24 is a `ValueError` here, as in Python 3.12.)
pub fn fromisoformat(s: &str) -> Option<Dt> {
    if s.chars().count() < 7 {
        return None;
    }
    let b = s.as_bytes();
    let sep = separator(b);
    if sep == usize::MAX {
        return None;
    }
    let (year, month, day) = date(&b[..sep.min(b.len())])?;
    let (mut h, mut mi, mut sec, mut offset) = (0, 0, 0, None);
    if sep < b.len() {
        let from = sep + 1;
        let len = b.len();
        let mut tz = from;
        while tz < len && !matches!(b[tz], b'Z' | b'+' | b'-') {
            tz += 1;
        }
        let (v, _us, more) = hh_mm_ss(b, from, tz).ok()?;
        if tz == len {
            if more {
                return None;
            }
        } else if b[tz] == b'Z' {
            if tz + 1 != len {
                return None;
            }
            offset = Some(0);
        } else {
            let sign = if b[tz] == b'-' { -1 } else { 1 };
            let (t, tus, rest) = hh_mm_ss(b, tz + 1, len).ok()?;
            if rest {
                return None;
            }
            let secs = sign * (t[0] * 3600 + t[1] * 60 + t[2]);
            let tus = sign * tus;
            // -24 h < offset < 24 h, microseconds counted.
            let total = secs * 1_000_000 + tus;
            if total.abs() >= 86_400 * 1_000_000 {
                return None;
            }
            offset = Some(total.div_euclid(1_000_000));
        }
        h = v[0];
        mi = v[1];
        sec = v[2];
    }
    if !(1..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || !(0..24).contains(&h)
        || !(0..60).contains(&mi)
        || !(0..60).contains(&sec)
    {
        return None;
    }
    Some(Dt {
        year,
        month,
        day,
        hour: h as u32,
        minute: mi as u32,
        second: sec as u32,
        offset,
    })
}

const OVERFLOW: &str = "OverflowError: date value out of range";

/// `localtime_r(secs)`: the UTC offset and the zone's name.
#[cfg(unix)]
fn local(secs: i64) -> Option<(i64, String)> {
    let t = secs as libc::time_t;
    // SAFETY: tzset reads TZ; localtime_r writes only into `tm`.
    unsafe {
        extern "C" {
            fn tzset();
        }
        tzset();
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return None;
        }
        let name = if tm.tm_zone.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(tm.tm_zone)
                .to_string_lossy()
                .into_owned()
        };
        #[allow(clippy::unnecessary_cast)]
        Some((tm.tm_gmtoff as i64, name))
    }
}

#[cfg(not(unix))]
fn local(_secs: i64) -> Option<(i64, String)> {
    None
}

/// The wall clock of `dt` as seconds since 1970 (its fields read as UTC).
fn wall(dt: &Dt) -> i64 {
    days_from_civil(dt.year, dt.month, dt.day) * 86_400
        + i64::from(dt.hour) * 3600
        + i64::from(dt.minute) * 60
        + i64::from(dt.second)
}

/// `local_to_seconds` (fold 0): the instant a naive wall time is here.
fn local_to_seconds(t: i64) -> Result<i64, String> {
    let off = |u: i64| local(u).map(|(o, _)| o).ok_or_else(|| OVERFLOW.to_string());
    let lt = |u: i64| off(u).map(|o| u + o);
    let a = lt(t)? - t;
    let u1 = t - a;
    let t1 = lt(u1)?;
    let b = if t1 == t {
        let u2 = u1 - 86_400;
        let bb = lt(u2)? - u2;
        if a == bb {
            return Ok(u1);
        }
        bb
    } else {
        t1 - u1
    };
    let u2 = t - b;
    let t2 = lt(u2)?;
    if t2 == t {
        return Ok(u2);
    }
    if t1 == t {
        return Ok(u1);
    }
    Ok(u1.max(u2))
}

/// The instant of `dt` in seconds since 1970: `.astimezone()`'s UTC.
fn instant(dt: &Dt) -> Result<i64, String> {
    let t = wall(dt);
    match dt.offset {
        Some(o) => Ok(t - o),
        None => {
            let u = local_to_seconds(t)?;
            let (o, _) = local(u).ok_or_else(|| OVERFLOW.to_string())?;
            Ok(t - o)
        }
    }
}

/// Whether the instant's date in a zone `offset` seconds east is in
/// Python's range (years 1 to 9999).
fn in_range(secs: i64, offset: i64) -> bool {
    let (y, _, _) = civil_from_days((secs + offset).div_euclid(86_400));
    (1..=9999).contains(&y)
}

/// `dt.astimezone().strftime(fmt)` and the local UTC offset.
pub fn here(dt: &Dt, fmt: &str) -> Result<(String, i64), String> {
    let secs = instant(dt)?;
    let (off, _) = local(secs).ok_or_else(|| OVERFLOW.to_string())?;
    if !in_range(secs, off) {
        return Err(OVERFLOW.into());
    }
    let text = crate::out::pylib::time::strftime_local(secs, fmt)?;
    Ok((text, off))
}

/// Python's `TZPATH` (`PYTHONTZPATH`'s absolute entries when it is set).
fn tzpath() -> Vec<String> {
    match std::env::var("PYTHONTZPATH") {
        Ok(v) => v
            .split(':')
            .filter(|p| p.starts_with('/'))
            .map(str::to_string)
            .collect(),
        Err(_) => [
            "/usr/share/zoneinfo",
            "/usr/lib/zoneinfo",
            "/usr/share/lib/zoneinfo",
            "/etc/zoneinfo",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    }
}

/// `ZoneInfo(key)`'s file, or `None` where Python raises.
fn zone_file(key: &str) -> Option<String> {
    use crate::out::pylib::path::normpath;
    if key.starts_with('/') || key.contains('\0') {
        return None;
    }
    if normpath(key).chars().count() != key.chars().count() {
        return None;
    }
    if !normpath(&format!("_/{key}")).starts_with("_/") {
        return None;
    }
    for dir in tzpath() {
        let file = format!("{dir}/{key}");
        if std::path::Path::new(&file).is_file() {
            let mut head = [0u8; 4];
            let ok = std::fs::File::open(&file)
                .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head))
                .is_ok();
            return (ok && &head == b"TZif").then_some(file);
        }
    }
    None
}

/// `strftime(fmt)` of `secs` in the zone of `file`, and its offset.
fn in_zone(file: &str, secs: i64, fmt: &str) -> Option<(String, i64)> {
    let old = std::env::var_os("TZ");
    std::env::set_var("TZ", format!(":{file}"));
    let r = local(secs).and_then(|(off, _)| {
        crate::out::pylib::time::strftime_local(secs, fmt)
            .ok()
            .map(|t| (t, off))
    });
    match old {
        Some(v) => std::env::set_var("TZ", v),
        None => std::env::remove_var("TZ"),
    }
    local(0);
    r.filter(|(_, off)| in_range(secs, *off))
}

/// `datetime.fromisoformat(s.replace('Z', '+00:00'))`: `Ok(None)` for
/// the `ValueError`, `Err` for what `.replace` raises on a value that is
/// not a string.
pub fn parse(s: &Value) -> Result<Option<Dt>, String> {
    match s {
        Value::Str(t) => Ok(t
            .as_str()
            .and_then(|s| fromisoformat(&s.replace('Z', "+00:00")))),
        other => Err(format!(
            "AttributeError: '{}' object has no attribute 'replace'",
            py::type_name(other)
        )),
    }
}

const FMT: &str = "%Y-%m-%d %H:%M %Z";

/// `hb_when(s, tz)` (`_HB_WHEN_PY`).
pub fn hb_when(s: Option<&Value>, tz: Option<&Value>) -> Result<String, String> {
    let s = match s {
        Some(v) if py::truthy(v) => v,
        _ => return Ok("-".into()),
    };
    let Some(t) = parse(s)? else {
        return Ok(py::str_of(s));
    };
    let (here_text, here_off) = here(&t, FMT)?;
    let here_text = here_text.trim_matches(super::super::py_space).to_string();
    let tz = match tz {
        Some(v) if py::truthy(v) => v,
        _ => return Ok(here_text),
    };
    let there = match tz {
        Value::Str(k) => k
            .as_str()
            .and_then(zone_file)
            .and_then(|file| in_zone(&file, instant(&t).ok()?, FMT)),
        _ => None,
    };
    let Some((there_text, there_off)) = there else {
        return Ok(format!("{here_text} here"));
    };
    if there_off == here_off {
        return Ok(there_text);
    }
    Ok(format!("{there_text} ({here_text} here)"))
}

/// `datetime.fromisoformat(s.replace('Z', '+00:00')).astimezone()
/// .strftime('%Y-%m-%d %H:%M')`, `s` itself on a `ValueError`.
pub fn local_minute(s: &Value) -> Result<String, String> {
    match parse(s)? {
        Some(t) => here(&t, "%Y-%m-%d %H:%M").map(|(text, _)| text),
        None => Ok(py::str_of(s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Fields = (i64, u32, u32, u32, u32, u32, Option<i64>);

    fn dt(s: &str) -> Option<Fields> {
        fromisoformat(s).map(|d| {
            (
                d.year, d.month, d.day, d.hour, d.minute, d.second, d.offset,
            )
        })
    }

    #[test]
    fn python_s_fromisoformat() {
        assert_eq!(
            dt("2026-11-20T10:00:00+00:00"),
            Some((2026, 11, 20, 10, 0, 0, Some(0)))
        );
        assert_eq!(
            dt("2026-11-20 10:00"),
            Some((2026, 11, 20, 10, 0, 0, None))
        );
        assert_eq!(
            dt("20261120T1000"),
            Some((2026, 11, 20, 10, 0, 0, None))
        );
        assert_eq!(
            dt("2026-11-20T10:00:00.1234567+01:30"),
            Some((2026, 11, 20, 10, 0, 0, Some(5400)))
        );
        assert_eq!(dt("2026-11-20x10"), Some((2026, 11, 20, 10, 0, 0, None)));
        assert_eq!(dt("2026-11-20"), Some((2026, 11, 20, 0, 0, 0, None)));
        assert_eq!(dt("2026-W47-5"), Some((2026, 11, 20, 0, 0, 0, None)));
        assert_eq!(dt("2026-11-20T10:00:00-07:00"), Some((2026, 11, 20, 10, 0, 0, Some(-25200))));
        for bad in [
            "2026-11-20T10:00:00 ",
            "2026-13-01",
            "2026-02-30",
            "0000-01-01",
            "soon",
            "",
            "2026-11-20T",
            "2026-11-20T10:0",
            "2026-11-20T10:00:00+24:00",
        ] {
            assert_eq!(dt(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_civil_calendar() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        let d = days_from_civil(2026, 11, 20);
        assert_eq!(civil_from_days(d), (2026, 11, 20));
        assert_eq!(civil_from_days(days_from_civil(1, 1, 1)), (1, 1, 1));
    }
}
