//! Numbers as the edit verbs write them. A move adds in decimal on the file's own token, so
//! `1.1 + 0.1` is `1.2` with no float drift; a number the verbs compute or take from a call is
//! written as the shortest decimal that reads back to the same `f64`, never with an exponent.

use crate::mutant;

/// The largest scale (digits after the point) and mantissa an exact sum keeps; past them the sum
/// is made in `f64` ([`fmt_f64`]).
const SCALE_MAX: u32 = 30;
const MANT_MAX: i128 = 10i128.pow(36);

/// A decimal number: `mant / 10^scale`, and whether its text had a point.
#[derive(Debug, Clone, Copy)]
struct Dec {
    mant: i128,
    scale: u32,
    point: bool,
}

/// `s` as a decimal: an optional sign, digits with at most one point, an optional exponent.
fn dec(s: &str) -> Option<Dec> {
    let s = s.trim();
    let (neg, body) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let (num, exp) = match body.find(['e', 'E']) {
        Some(i) => (&body[..i], body[i + 1..].parse::<i32>().ok()?),
        None => (body, 0),
    };
    let (int, frac, point) = match num.find('.') {
        Some(i) => (&num[..i], &num[i + 1..], true),
        None => (num, "", false),
    };
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    if !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut mant: i128 = 0;
    for b in int.bytes().chain(frac.bytes()) {
        mant = mant.checked_mul(10)?.checked_add(i128::from(b - b'0'))?;
        if mant >= MANT_MAX {
            return None;
        }
    }
    let mut scale = i64::try_from(frac.len()).ok()? - i64::from(exp);
    while scale < 0 {
        mant = mant.checked_mul(10)?;
        if mant >= MANT_MAX {
            return None;
        }
        scale += 1;
    }
    let scale = u32::try_from(scale).ok().filter(|s| *s <= SCALE_MAX)?;
    Some(Dec {
        mant: if neg { -mant } else { mant },
        scale,
        point,
    })
}

fn widen(d: Dec, scale: u32) -> Option<i128> {
    let m = d.mant.checked_mul(10i128.checked_pow(scale - d.scale)?)?;
    (m.abs() < MANT_MAX).then_some(m)
}

/// `mant / 10^scale` as text: at least one digit after the point when `point`.
fn text(mant: i128, scale: u32, point: bool) -> String {
    let neg = mant < 0;
    let digits = mant.unsigned_abs().to_string();
    let scale = scale as usize;
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if scale == 0 {
        out.push_str(&digits);
        if point {
            out.push_str(".0");
        }
        return out;
    }
    let padded = format!("{digits:0>width$}", width = scale + 1);
    let (int, frac) = padded.split_at(padded.len() - scale);
    out.push_str(int);
    out.push('.');
    out.push_str(frac);
    out
}

/// `token + delta`, exactly, in the token's style (as many digits after the point as either
/// has; a point when the token has one). `None` when either isn't a plain decimal this sum keeps
/// exactly: the caller adds in `f64` then.
pub(crate) fn add_exact(token: &str, delta: &str) -> Option<String> {
    if mutant::active("FloatDriftOnMove") {
        return None;
    }
    let (a, b) = (dec(token)?, dec(delta)?);
    let scale = a.scale.max(b.scale);
    let sum = widen(a, scale)?.checked_add(widen(b, scale)?)?;
    if sum.abs() >= MANT_MAX {
        return None;
    }
    // no minus sign on a zero
    Some(text(sum, scale, a.point || b.point))
}

/// `token + delta`: exactly when it can be ([`add_exact`]), else in `f64`, written by
/// [`fmt_f64`]. `None` when either isn't a number, or the sum isn't finite.
pub(crate) fn add(token: &str, delta: &str) -> Option<String> {
    if let Some(s) = add_exact(token, delta) {
        return Some(s);
    }
    let x = parse(token)? + parse(delta)?;
    x.is_finite().then(|| fmt_f64(x))
}

/// A number as written in a file or a call: finite, or `None`.
pub(crate) fn parse(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|x| x.is_finite())
}

/// The shortest decimal that reads back to `x`, with no exponent and a point always (`11.0`,
/// `0.5`); `-0` is `0.0`.
pub(crate) fn fmt_f64(x: f64) -> String {
    let x = if x == 0.0 { 0.0 } else { x };
    let s = format!("{x}");
    if s.contains('.') {
        s
    } else {
        format!("{s}.0")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_sums() {
        for (a, b, want) in [
            ("1.1", "0.1", "1.2"),
            ("10.0", "1", "11.0"),
            ("10", "1", "11"),
            ("10", "0.5", "10.5"),
            ("-0.5", "0.5", "0.0"),
            ("0.000", "1.25", "1.250"),
            ("1e2", "1", "101"),
            ("-3.25", "-1.75", "-5.00"),
            ("0.1", "0.2", "0.3"),
        ] {
            assert_eq!(add(a, b).as_deref(), Some(want), "{a} + {b}");
        }
        assert_eq!(add("x", "1"), None);
        assert_eq!(fmt_f64(-0.0), "0.0");
        assert_eq!(fmt_f64(2.0), "2.0");
        assert_eq!(fmt_f64(0.1 + 0.2), "0.30000000000000004");
    }
}
