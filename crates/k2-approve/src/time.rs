//! UTC times in signed approvals: exactly `YYYY-MM-DDTHH:MM:SSZ`.

/// Seconds since 1970-01-01T00:00:00Z for a strict UTC time, or None.
/// Years 2000–9999; real calendar dates only; no leap seconds.
pub fn parse_utc(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let mut v = 0i64;
        for &c in &b[r] {
            if !c.is_ascii_digit() {
                return None;
            }
            v = v * 10 + (c - b'0') as i64;
        }
        Some(v)
    };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(2000..=9999).contains(&y) || !(1..=12).contains(&mo) || d < 1 || d > days_in_month(y, mo) || h > 23 || mi > 59 || se > 59 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se)
}

/// The same time written back (for tests and for relying parties writing payloads).
pub fn format_utc(t: i64) -> String {
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

/// How a time reads on an approval screen: `2026-11-12 02:00:00 UTC`.
pub fn display_utc(s: &str) -> String {
    format!("{} {} UTC", &s[0..10], &s[11..19])
}

fn leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap(y) => 29,
        _ => 28,
    }
}

// Howard Hinnant's days_from_civil / civil_from_days.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_refuses() {
        assert_eq!(parse_utc("2026-11-11T14:00:00Z"), Some(1_794_405_600));
        assert_eq!(format_utc(1_794_405_600), "2026-11-11T14:00:00Z");
        assert_eq!(parse_utc("2028-02-29T00:00:00Z").map(format_utc).as_deref(), Some("2028-02-29T00:00:00Z"));
        for bad in ["2026-02-29T00:00:00Z", "2026-11-11T14:00:00.5Z", "2026-11-11 14:00:00Z", "2026-11-11T24:00:00Z", "1999-12-31T23:59:59Z", "2026-11-11T14:00:00+00:00", "2026-13-01T00:00:00Z"] {
            assert_eq!(parse_utc(bad), None, "{bad}");
        }
        assert_eq!(display_utc("2026-11-12T02:00:00Z"), "2026-11-12 02:00:00 UTC");
    }
}
