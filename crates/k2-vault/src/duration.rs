//! One duration grammar for every vault time box (prd-vault-for-agents-v1
//! §3.3, V10): `90s`, `10m`, `2h`, `7d`, or a bare number of seconds.
//!
//! The CLI sends the flag as typed (`"ttl": "2h"`) and the daemon parses it
//! here, so `--ttl` means the same thing on `set`, `file`, `env-file`,
//! `grant` and secret cards. Each caller still applies its own floor and
//! cap; this only turns the words into seconds and refuses what isn't one.

use serde_json::Value;

use crate::error::{Result, VaultError};

/// The refusal code for a duration that doesn't parse.
pub const BAD_TTL: &str = "bad_ttl";

/// Longer than any vault time box (the longest is 30 days). Above it the
/// words are almost surely a mistake, and `now + secs` stays in range.
pub const MAX_SECS: i64 = 366 * 24 * 3600;

const HINT: &str = "a duration is like 90s, 10m, 2h or 7d (or seconds: 3600)";

fn bad(what: &str) -> VaultError {
    VaultError::bad_request(BAD_TTL, format!("{HINT}, not '{what}'"))
}

/// `90s` / `10m` / `2h` / `7d` / `3600` → seconds. Whitespace around it is
/// fine; zero, negatives, other units, fractions and anything over a year
/// are refused with [`BAD_TTL`].
pub fn parse(s: &str) -> Result<i64> {
    let t = s.trim();
    let (num, mult) = match t.as_bytes().last() {
        Some(b's') => (&t[..t.len() - 1], 1),
        Some(b'm') => (&t[..t.len() - 1], 60),
        Some(b'h') => (&t[..t.len() - 1], 3600),
        Some(b'd') => (&t[..t.len() - 1], 86_400),
        _ => (t, 1),
    };
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad(t));
    }
    let n: i64 = num.parse().map_err(|_| bad(t))?;
    let secs = n.checked_mul(mult).ok_or_else(|| bad(t))?;
    if secs <= 0 {
        return Err(VaultError::bad_request(BAD_TTL, format!("a duration must be more than 0, not '{t}'")));
    }
    if secs > MAX_SECS {
        return Err(VaultError::bad_request(BAD_TTL, format!("'{t}' is longer than a year; {HINT}")));
    }
    Ok(secs)
}

/// A request body's `ttl`: the string as typed (`"2h"`) or a number of
/// seconds. Anything else is [`BAD_TTL`].
pub fn from_json(v: &Value) -> Result<i64> {
    match v {
        Value::String(s) => parse(s),
        Value::Number(n) => match n.as_i64() {
            Some(secs) => parse(&secs.to_string()),
            None => Err(bad(&n.to_string())),
        },
        other => Err(bad(&other.to_string())),
    }
}

/// A body's time box from `ttl` (any form [`from_json`] takes) or the older
/// CLI's `ttlSecs` (a plain number, kept so an older `k2` still works).
/// Both at once is refused: they would disagree.
pub fn body_ttl(ttl: Option<&Value>, legacy_secs: Option<i64>) -> Result<Option<i64>> {
    match (ttl.filter(|v| !v.is_null()), legacy_secs) {
        (Some(_), Some(_)) => Err(VaultError::bad_request(BAD_TTL, "send ttl or ttlSecs, not both")),
        (Some(v), None) => from_json(v).map(Some),
        (None, Some(secs)) => Ok(Some(secs)),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_four_units_and_bare_seconds() {
        for (s, want) in [
            ("90s", 90),
            ("10m", 600),
            ("2h", 7200),
            ("7d", 604_800),
            ("3600", 3600),
            ("1", 1),
            (" 2h ", 7200),
            ("366d", MAX_SECS),
        ] {
            assert_eq!(parse(s).unwrap_or_else(|e| panic!("{s}: {e}")), want, "{s}");
        }
    }

    #[test]
    fn everything_else_is_refused_with_bad_ttl() {
        for s in [
            "", " ", "soon", "h", "2w", "2H", "1.5h", "-5m", "+5m", "0", "0s", "0d", "2 h", "2hh", "h2", "1e3", "367d",
            "99999999999999999999", "9223372036854775807d",
        ] {
            let e = parse(s).expect_err(s);
            assert_eq!(e.code, BAD_TTL, "{s}: {e}");
            assert_eq!(e.status, 400, "{s}");
        }
        assert!(parse("soon").expect_err("soon").hint.contains("90s, 10m, 2h or 7d"));
    }

    #[test]
    fn a_body_takes_the_string_a_number_or_the_old_ttl_secs() {
        assert_eq!(body_ttl(Some(&json!("2h")), None).expect("2h"), Some(7200));
        assert_eq!(body_ttl(Some(&json!(120)), None).expect("120"), Some(120));
        assert_eq!(body_ttl(None, Some(90)).expect("legacy"), Some(90));
        assert_eq!(body_ttl(None, None).expect("none"), None);
        assert_eq!(body_ttl(Some(&Value::Null), None).expect("null"), None);
        for bad in [json!("soon"), json!(-1), json!(1.5), json!(true), json!(["2h"])] {
            assert_eq!(body_ttl(Some(&bad), None).expect_err("bad").code, BAD_TTL, "{bad}");
        }
        assert_eq!(body_ttl(Some(&json!("2h")), Some(60)).expect_err("both").code, BAD_TTL);
    }
}
