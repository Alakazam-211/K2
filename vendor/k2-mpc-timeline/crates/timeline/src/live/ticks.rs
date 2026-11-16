//! The live store's one time unit, the **tick**: 1/254,016,000,000 of a second. Every whole
//! frame rate in use, the 1001 family, and the common sample rates (44.1, 48, 88.2, 96 and
//! 192 kHz) are whole numbers of ticks, so a time on any of their grids is an integer here.
//!
//! - A time that isn't a whole number of ticks is refused ([`TickError::OffTick`]); one past an
//!   `i64` of ticks (about 420 days) is [`TickError::Range`].
//! - **Ticks are decimal strings** in the keyed state ([`text`], [`parse`]): an exact integer
//!   on every wire and in every client, never a floating-point number. A frame of a common rate
//!   is a multiple of at least 32 ticks, so a double holds frame-aligned times exactly far past
//!   2^53; an odd tick count past 2^53 (about 9.85 hours) it doesn't.

use crate::mutant;
use crate::time::{Rate, Time, TimeError};

/// Ticks in one second.
pub const PER_SECOND: i64 = 254_016_000_000;

/// The largest record time (from the timeline's zero) a live timeline holds, in ticks: the
/// view index carries record geometry as JSON numbers, exact only up to 2^53.
pub const RECORD_MAX: i64 = 1 << 53;

/// Why a time isn't a tick count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickError {
    /// Not a whole number of ticks.
    OffTick,
    /// Past an `i64` of ticks.
    Range,
}

/// `t` in ticks, exactly.
pub fn of(t: Time) -> Result<i64, TickError> {
    let n = i128::from(t.num()) * i128::from(PER_SECOND);
    let d = i128::from(t.den());
    if n % d != 0 && !mutant::active("TickRounded") {
        return Err(TickError::OffTick);
    }
    i64::try_from(n / d).map_err(|_| TickError::Range)
}

/// `n` ticks as a time.
pub fn time(n: i64) -> Result<Time, TimeError> {
    Time::new(n, PER_SECOND as u64)
}

/// One frame of `rate` in ticks, when it is a whole number of them.
pub fn frame(rate: Rate) -> Option<i64> {
    of(rate.frame()).ok()
}

/// A tick count as stored: its decimal digits, `-` for a negative one.
pub fn text(n: i64) -> String {
    n.to_string()
}

/// A stored tick count: decimal digits, an optional leading `-`, no leading zeros, no `+`, no
/// spaces. Anything else is `None`.
pub fn parse(s: &str) -> Option<i64> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    if digits.is_empty()
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
        || s == "-0"
    {
        return None;
    }
    if mutant::active("TicksAsDouble") {
        // planted: the count read as a double, as a client that parses JSON numbers would
        return s.parse::<f64>().ok().map(|f| f as i64);
    }
    s.parse().ok()
}

/// Two tick counts stored as one field, `a,b` (a geometry's start and duration).
pub fn pair_text(a: i64, b: i64) -> String {
    format!("{a},{b}")
}

/// [`pair_text`]'s inverse.
pub fn parse_pair(s: &str) -> Option<(i64, i64)> {
    let (a, b) = s.split_once(',')?;
    Some((parse(a)?, parse(b)?))
}

/// A rate as stored: `num/den`.
pub fn rate_text(r: Rate) -> String {
    format!("{}/{}", r.num(), r.den())
}

/// [`rate_text`]'s inverse.
pub fn parse_rate(s: &str) -> Option<Rate> {
    let (n, d) = s.split_once('/')?;
    Rate::new(n.parse().ok()?, d.parse().ok()?).ok()
}

/// `ticks` as the value of an OTIO time at `rate` (frames, or units of the rate): its text in
/// the style `like` is written in (`86.0` keeps a `.0`, `86` doesn't), exactly. `None` when
/// the value isn't a finite decimal (between frames of a rate that can't write it).
pub fn value_text(ticks: i64, rate: Rate, like: &str) -> Option<String> {
    // value = ticks * rate.num / (PER_SECOND * rate.den)
    let mut n = i128::from(ticks) * i128::from(rate.num());
    let mut d = i128::from(PER_SECOND) * i128::from(rate.den());
    let g = gcd(n.unsigned_abs(), d.unsigned_abs()) as i128;
    if g > 1 {
        n /= g;
        d /= g;
    }
    if d == 1 {
        let point = like.contains(['.', 'e', 'E']);
        return Some(if point {
            format!("{n}.0")
        } else {
            n.to_string()
        });
    }
    // a finite decimal only when the denominator is 2^a * 5^b
    let (mut rest, mut twos, mut fives) = (d, 0u32, 0u32);
    while rest % 2 == 0 {
        rest /= 2;
        twos += 1;
    }
    while rest % 5 == 0 {
        rest /= 5;
        fives += 1;
    }
    if rest != 1 {
        return None;
    }
    let places = twos.max(fives);
    if places > 30 {
        return None;
    }
    let scaled = n.checked_mul(10i128.checked_pow(places)? / d)?;
    let neg = scaled < 0;
    let digits = scaled.unsigned_abs().to_string();
    let p = places as usize;
    let digits = if digits.len() <= p {
        format!("{}{digits}", "0".repeat(p + 1 - digits.len()))
    } else {
        digits
    };
    let (whole, frac) = digits.split_at(digits.len() - p);
    let frac = frac.trim_end_matches('0');
    Some(format!("{}{whole}.{frac}", if neg { "-" } else { "" }))
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_rates_are_whole_ticks() {
        for r in [
            Rate::whole(24).unwrap(),
            Rate::whole(25).unwrap(),
            Rate::ntsc(24).unwrap(),
            Rate::ntsc(30).unwrap(),
            Rate::ntsc(60).unwrap(),
            Rate::whole(48_000).unwrap(),
            Rate::whole(44_100).unwrap(),
            Rate::whole(96_000).unwrap(),
            Rate::whole(192_000).unwrap(),
            Rate::whole(88_200).unwrap(),
        ] {
            let f = frame(r).unwrap_or_else(|| panic!("{r}"));
            assert_eq!(time(f).unwrap(), r.frame(), "{r}");
        }
        assert_eq!(frame(Rate::ntsc(30).unwrap()), Some(8_475_667_200));
        assert_eq!(of(Time::new(1, 3).unwrap()), Ok(84_672_000_000));
        assert_eq!(
            of(Time::new(1, 254_016_000_001).unwrap()),
            Err(TickError::OffTick)
        );
        assert_eq!(of(Time::seconds(40_000_000)), Err(TickError::Range));
    }

    #[test]
    fn stored_ticks_are_strict_decimals() {
        for (s, v) in [
            ("0", 0),
            ("-5", -5),
            ("21963254400000001", 21_963_254_400_000_001),
        ] {
            assert_eq!(parse(s), Some(v));
            assert_eq!(text(v), s);
        }
        for bad in [
            "",
            "-",
            "-0",
            "01",
            "+1",
            "1.0",
            "1e3",
            " 1",
            "9223372036854775808",
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
        assert_eq!(parse_pair("10,x"), None);
        assert_eq!(parse_pair("-10,3"), Some((-10, 3)));
        assert_eq!(parse_rate("30000/1001"), Rate::ntsc(30).ok());
    }

    #[test]
    fn values_are_written_exactly_in_the_file_style() {
        let ntsc = Rate::ntsc(30).unwrap();
        let f = frame(ntsc).unwrap();
        assert_eq!(value_text(86 * f, ntsc, "10.0").as_deref(), Some("86.0"));
        assert_eq!(value_text(86 * f, ntsc, "10").as_deref(), Some("86"));
        assert_eq!(value_text(-2 * f, ntsc, "1"), Some("-2".into()));
        // half a frame at 24: 0.5
        let r24 = Rate::whole(24).unwrap();
        let half = frame(r24).unwrap() / 2;
        assert_eq!(value_text(half, r24, "1.0").as_deref(), Some("0.5"));
        // a third of a frame has no finite decimal
        assert_eq!(value_text(frame(r24).unwrap() / 3, r24, "1.0"), None);
    }
}
