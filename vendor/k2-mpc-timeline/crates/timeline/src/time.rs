//! Exact time: a [`Time`] is a rational number of seconds, a [`Rate`] a rational number of
//! frames per second. Nothing here rounds unless a caller asks it to ([`Snap`]): a time that is
//! not on a rate's frame grid is refused with the two frames around it
//! ([`TimeError::OffGrid`]), so a person or an agent picks one.
//!
//! - [`Time`]: `i64` over `u64`, always reduced, so equal times compare and hash equal. Every
//!   operation is checked: a result that doesn't fit is [`TimeError::Overflow`], never a wrap.
//! - [`Rate`]: `u32` over `u32` (`30000/1001`, `25`), from whole numbers, from the NTSC family
//!   (`n * 1000 / 1001`), from a frame duration, or from a floating-point rate a file wrote
//!   ([`Rate::from_f64`], which only accepts the rates it can name exactly).
//! - [`Timecode`]: `HH:MM:SS:FF`, and drop-frame `HH:MM:SS;FF` at the `30000/1001` and
//!   `60000/1001` families, to and from frame counts.
//! - [`parse_time`]: what a verb's `--at` takes: a timecode, `123f`, `12.5s` or `1001/24000s`,
//!   with an optional sign for a relative time.

use std::cmp::Ordering;
use std::fmt;

/// Why a time could not be made, read or placed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TimeError {
    /// Nothing was given.
    #[error("no time given")]
    Empty,
    /// The text is not a time this parser reads.
    #[error("not a time: {why}")]
    Syntax {
        /// What was wrong (fixed text).
        why: &'static str,
    },
    /// A value or a result doesn't fit.
    #[error("a time out of range")]
    Overflow,
    /// A rate of zero, a negative rate, or one this crate can't name exactly.
    #[error("not a frame rate this reader can name exactly")]
    BadRate,
    /// The time is between two frames of the rate, and the caller asked for no snapping.
    #[error("{time} is between frames at {rate}: the frames around it are {before} and {after}")]
    OffGrid {
        /// The time as given.
        time: Time,
        /// The rate whose grid it misses.
        rate: Rate,
        /// The frame before it.
        before: Time,
        /// The frame after it.
        after: Time,
    },
    /// A timecode that names no frame at this rate.
    #[error("not a timecode at this rate: {why}")]
    BadTimecode {
        /// What was wrong (fixed text).
        why: &'static str,
    },
    /// Timecode needs a whole-number rate or an NTSC one.
    #[error("{rate} has no timecode")]
    NoTimecode {
        /// The rate.
        rate: Rate,
    },
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// An exact time in seconds: `num / den`, reduced, `den >= 1`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Time {
    num: i64,
    den: u64,
}

impl Time {
    /// Zero seconds.
    pub const ZERO: Time = Time { num: 0, den: 1 };

    /// `num / den` seconds, reduced. A zero `den` is refused.
    pub fn new(num: i64, den: u64) -> Result<Time, TimeError> {
        Self::from_i128(i128::from(num), i128::from(den))
    }

    /// Whole seconds.
    pub const fn seconds(s: i64) -> Time {
        Time { num: s, den: 1 }
    }

    /// The numerator (reduced; carries the sign).
    pub const fn num(self) -> i64 {
        self.num
    }

    /// The denominator (reduced; at least 1).
    pub const fn den(self) -> u64 {
        self.den
    }

    pub(crate) fn from_i128(num: i128, den: i128) -> Result<Time, TimeError> {
        if den == 0 {
            return Err(TimeError::Syntax {
                why: "a zero denominator",
            });
        }
        let (mut n, mut d) = if den < 0 {
            (
                num.checked_neg().ok_or(TimeError::Overflow)?,
                den.checked_neg().ok_or(TimeError::Overflow)?,
            )
        } else {
            (num, den)
        };
        let g = gcd(n.unsigned_abs(), d.unsigned_abs());
        if g > 1 {
            // g divides both, and both fit in i128, so g does too
            let g = g as i128;
            n /= g;
            d /= g;
        }
        Ok(Time {
            num: i64::try_from(n).map_err(|_| TimeError::Overflow)?,
            den: u64::try_from(d).map_err(|_| TimeError::Overflow)?,
        })
    }

    /// `count` frames at `rate`.
    pub fn frames(count: i64, rate: Rate) -> Result<Time, TimeError> {
        Self::from_i128(
            i128::from(count) * i128::from(rate.den),
            i128::from(rate.num),
        )
    }

    /// `self + other`.
    pub fn checked_add(self, other: Time) -> Result<Time, TimeError> {
        let g = gcd(u128::from(self.den), u128::from(other.den)) as i128;
        let (d1, d2) = (i128::from(self.den), i128::from(other.den));
        let a = i128::from(self.num)
            .checked_mul(d2 / g)
            .ok_or(TimeError::Overflow)?;
        let b = i128::from(other.num)
            .checked_mul(d1 / g)
            .ok_or(TimeError::Overflow)?;
        let n = a.checked_add(b).ok_or(TimeError::Overflow)?;
        let d = d1.checked_mul(d2 / g).ok_or(TimeError::Overflow)?;
        Self::from_i128(n, d)
    }

    /// `self - other`.
    pub fn checked_sub(self, other: Time) -> Result<Time, TimeError> {
        self.checked_add(other.checked_neg()?)
    }

    /// `-self`.
    pub fn checked_neg(self) -> Result<Time, TimeError> {
        Ok(Time {
            num: self.num.checked_neg().ok_or(TimeError::Overflow)?,
            den: self.den,
        })
    }

    /// `self * num / den`.
    pub fn checked_scale(self, num: i64, den: u64) -> Result<Time, TimeError> {
        if den == 0 {
            return Err(TimeError::Syntax {
                why: "a zero denominator",
            });
        }
        let n = i128::from(self.num)
            .checked_mul(i128::from(num))
            .ok_or(TimeError::Overflow)?;
        let d = i128::from(self.den)
            .checked_mul(i128::from(den))
            .ok_or(TimeError::Overflow)?;
        Self::from_i128(n, d)
    }

    /// Whether this is below zero.
    pub const fn is_negative(self) -> bool {
        self.num < 0
    }

    /// Whether this is zero.
    pub const fn is_zero(self) -> bool {
        self.num == 0
    }

    /// This time as a frame count at `rate`: exact, or snapped as `snap` says. Off the grid
    /// with [`Snap::Refuse`] is [`TimeError::OffGrid`].
    pub fn to_frames(self, rate: Rate, snap: Snap) -> Result<i64, TimeError> {
        // frames = num * rate.num / (den * rate.den)
        let n = i128::from(self.num) * i128::from(rate.num);
        let d = i128::from(self.den) * i128::from(rate.den);
        let floor = n.div_euclid(d);
        let exact = n.rem_euclid(d) == 0;
        let fit = |f: i128| i64::try_from(f).map_err(|_| TimeError::Overflow);
        if exact {
            return fit(floor);
        }
        match snap {
            Snap::Floor => fit(floor),
            Snap::Ceil => fit(floor + 1),
            Snap::Nearest => {
                // ties go to the earlier frame
                let twice = n.rem_euclid(d) * 2;
                fit(if twice > d { floor + 1 } else { floor })
            }
            Snap::Refuse => Err(TimeError::OffGrid {
                time: self,
                rate,
                before: Time::frames(fit(floor)?, rate)?,
                after: Time::frames(fit(floor + 1)?, rate)?,
            }),
        }
    }

    /// Whether this time is a whole number of frames at `rate`.
    pub fn on_grid(self, rate: Rate) -> bool {
        self.to_frames(rate, Snap::Refuse).is_ok()
    }

    /// Reads `N/Ds`, `Ns` or `N.Fs` (seconds, with an optional leading `-`), exactly.
    pub fn parse_seconds(s: &str) -> Result<Time, TimeError> {
        let body = s.strip_suffix('s').ok_or(TimeError::Syntax {
            why: "seconds end in s",
        })?;
        parse_number(body)
    }

    /// The time as approximate seconds, for display only.
    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }
}

/// Reads `N/D`, `N` or `N.F` (with an optional leading `-`) as an exact rational.
pub(crate) fn parse_number(s: &str) -> Result<Time, TimeError> {
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s),
    };
    if body.is_empty() {
        return Err(TimeError::Empty);
    }
    let t = if let Some((n, d)) = body.split_once('/') {
        let n = digits(n)?;
        let d = digits(d)?;
        Time::from_i128(n, d)?
    } else if let Some((w, f)) = body.split_once('.') {
        if f.is_empty() || f.len() > 18 {
            return Err(TimeError::Syntax {
                why: "a decimal needs 1 to 18 digits after the point",
            });
        }
        let w = if w.is_empty() { 0 } else { digits(w)? };
        let scale = 10i128.pow(f.len() as u32);
        let f = digits(f)?;
        let n = w
            .checked_mul(scale)
            .and_then(|x| x.checked_add(f))
            .ok_or(TimeError::Overflow)?;
        Time::from_i128(n, scale)?
    } else {
        Time::from_i128(digits(body)?, 1)?
    };
    if neg {
        t.checked_neg()
    } else {
        Ok(t)
    }
}

/// ASCII digits as a number (at most 30 of them).
fn digits(s: &str) -> Result<i128, TimeError> {
    if s.is_empty() || s.len() > 30 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(TimeError::Syntax {
            why: "expected digits",
        });
    }
    let mut v: i128 = 0;
    for b in s.bytes() {
        v = v
            .checked_mul(10)
            .and_then(|v| v.checked_add(i128::from(b - b'0')))
            .ok_or(TimeError::Overflow)?;
    }
    Ok(v)
}

impl PartialOrd for Time {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Time {
    fn cmp(&self, other: &Self) -> Ordering {
        // (2^63 - 1) * (2^64 - 1) < 2^127: the cross products fit in i128
        let a = i128::from(self.num) * i128::from(other.den);
        let b = i128::from(other.num) * i128::from(self.den);
        a.cmp(&b)
    }
}

/// `N/Ds`, or `Ns` for whole seconds.
impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}s", self.num)
        } else {
            write!(f, "{}/{}s", self.num, self.den)
        }
    }
}

impl fmt::Debug for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// What to do with a time between two frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snap {
    /// Refuse it ([`TimeError::OffGrid`]): the default for anything a person or agent typed.
    Refuse,
    /// The frame at or before it.
    Floor,
    /// The frame at or after it.
    Ceil,
    /// The nearer frame; a tie goes to the earlier one.
    Nearest,
}

/// Frames per second, `num / den`, reduced, both above zero.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rate {
    num: u32,
    den: u32,
}

impl Rate {
    /// `num / den` frames per second, reduced.
    pub fn new(num: u32, den: u32) -> Result<Rate, TimeError> {
        if num == 0 || den == 0 {
            return Err(TimeError::BadRate);
        }
        let g = gcd(u128::from(num), u128::from(den)) as u32;
        Ok(Rate {
            num: num / g,
            den: den / g,
        })
    }

    /// A whole number of frames per second.
    pub fn whole(fps: u32) -> Result<Rate, TimeError> {
        Self::new(fps, 1)
    }

    /// The NTSC rate near `nominal`: `nominal * 1000 / 1001` (`ntsc(30)` is 29.97).
    pub fn ntsc(nominal: u32) -> Result<Rate, TimeError> {
        Self::new(nominal.checked_mul(1000).ok_or(TimeError::BadRate)?, 1001)
    }

    /// The rate whose frame lasts `frame`: `1 / frame`.
    pub fn from_frame_duration(frame: Time) -> Result<Rate, TimeError> {
        if frame.num <= 0 {
            return Err(TimeError::BadRate);
        }
        let num = u32::try_from(frame.den).map_err(|_| TimeError::BadRate)?;
        let den = u32::try_from(frame.num).map_err(|_| TimeError::BadRate)?;
        Self::new(num, den)
    }

    /// A rate a file wrote as a floating-point number. Taken only when it names one rate
    /// exactly: a whole number; an NTSC rate (`23.976`, `29.97`, `23.976023976023978`; within
    /// two parts in a million of `n * 1000 / 1001`); or a fraction with a denominator up to
    /// 1000 (`12.5`).
    /// Anything else is [`TimeError::BadRate`].
    pub fn from_f64(fps: f64) -> Result<Rate, TimeError> {
        if !fps.is_finite() || fps <= 0.0 || fps > f64::from(u32::MAX) {
            return Err(TimeError::BadRate);
        }
        let whole = fps.round();
        if (fps - whole).abs() < 1e-9 && whole >= 1.0 {
            return Self::whole(whole as u32);
        }
        let nominal = (fps * 1.001).round();
        let ntsc = nominal * 1000.0 / 1001.0;
        // relative: the NTSC rates written to three decimals (23.976, 29.97, 119.88) are all
        // within one part in a million of the exact rate
        if (1.0..=4_000_000.0).contains(&nominal) && (fps - ntsc).abs() <= 2e-6 * ntsc {
            return Self::ntsc(nominal as u32);
        }
        for den in 2..=1000u32 {
            let num = (fps * f64::from(den)).round();
            if (1.0..=f64::from(u32::MAX)).contains(&num)
                && (num / f64::from(den) - fps).abs() < 1e-9
            {
                return Self::new(num as u32, den);
            }
        }
        Err(TimeError::BadRate)
    }

    /// The numerator.
    pub const fn num(self) -> u32 {
        self.num
    }

    /// The denominator.
    pub const fn den(self) -> u32 {
        self.den
    }

    /// One frame's duration.
    pub fn frame(self) -> Time {
        Time {
            num: i64::from(self.den),
            den: u64::from(self.num),
        }
    }

    /// Frames per timecode second: the rate itself when whole, the nominal rate for an NTSC
    /// one (30 for 29.97), else `None`.
    pub fn timecode_base(self) -> Option<u32> {
        if self.den == 1 {
            Some(self.num)
        } else if self.den == 1001 && self.num % 1000 == 0 {
            Some(self.num / 1000)
        } else {
            None
        }
    }

    /// Frame labels dropped per minute in drop-frame timecode (2 at 29.97, 4 at 59.94), or
    /// `None` when the rate has no drop-frame timecode.
    pub fn drop_per_minute(self) -> Option<u32> {
        match self.timecode_base() {
            Some(b) if self.den == 1001 && b % 30 == 0 => Some(b / 15),
            _ => None,
        }
    }

    /// The rate as approximate frames per second, for display only.
    pub fn as_f64(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }
}

/// `25 fps`, `30000/1001 fps`.
impl fmt::Display for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{} fps", self.num)
        } else {
            write!(f, "{}/{} fps", self.num, self.den)
        }
    }
}

impl fmt::Debug for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// A timecode label: `HH:MM:SS:FF`, or `HH:MM:SS;FF` when drop-frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timecode {
    /// Hours (any number).
    pub hours: u32,
    /// Minutes, below 60.
    pub minutes: u32,
    /// Seconds, below 60.
    pub seconds: u32,
    /// Frames, below the rate's timecode base.
    pub frames: u32,
    /// Drop-frame.
    pub drop: bool,
}

impl Timecode {
    /// Reads the label's syntax: four fields of digits, the last separator `:` (non-drop) or
    /// `;` (drop-frame); the others are `:`. Whether it names a frame depends on the rate
    /// ([`Timecode::to_frames`]).
    pub fn parse(s: &str) -> Result<Timecode, TimeError> {
        let bad = |why| TimeError::BadTimecode { why };
        let (head, frames, drop) = match (s.rfind(':'), s.rfind(';')) {
            (_, Some(i)) if s[i + 1..].bytes().all(|b| b.is_ascii_digit()) => {
                (&s[..i], &s[i + 1..], true)
            }
            (Some(i), None) => (&s[..i], &s[i + 1..], false),
            _ => return Err(bad("expected HH:MM:SS:FF or HH:MM:SS;FF")),
        };
        let parts: Vec<&str> = head.split(':').collect();
        if parts.len() != 3 {
            return Err(bad("expected HH:MM:SS:FF or HH:MM:SS;FF"));
        }
        let field = |p: &str, max: usize| -> Result<u32, TimeError> {
            if p.is_empty() || p.len() > max || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad("each field is digits"));
            }
            p.parse::<u32>().map_err(|_| bad("each field is digits"))
        };
        let tc = Timecode {
            hours: field(parts[0], 6)?,
            minutes: field(parts[1], 2)?,
            seconds: field(parts[2], 2)?,
            frames: field(frames, 4)?,
            drop,
        };
        if tc.minutes >= 60 || tc.seconds >= 60 {
            return Err(bad("minutes and seconds are below 60"));
        }
        Ok(tc)
    }

    /// The label of frame `frame` (from zero) at `rate`.
    pub fn from_frames(frame: i64, rate: Rate, drop: bool) -> Result<Timecode, TimeError> {
        let base = i64::from(rate.timecode_base().ok_or(TimeError::NoTimecode { rate })?);
        if frame < 0 {
            return Err(TimeError::BadTimecode {
                why: "a timecode is never negative",
            });
        }
        let mut label = frame;
        if drop {
            let d = i64::from(rate.drop_per_minute().ok_or(TimeError::BadTimecode {
                why: "drop-frame needs a 30000/1001 or 60000/1001 rate",
            })?);
            let per_ten = base * 600 - 9 * d;
            let per_minute = base * 60 - d;
            let tens = frame / per_ten;
            let rest = frame % per_ten;
            let skipped = 9 * d * tens
                + if rest >= d {
                    d * ((rest - d) / per_minute)
                } else {
                    0
                };
            label = frame.checked_add(skipped).ok_or(TimeError::Overflow)?;
        }
        let frames = label % base;
        let secs = label / base;
        Ok(Timecode {
            hours: u32::try_from(secs / 3600).map_err(|_| TimeError::Overflow)?,
            minutes: ((secs / 60) % 60) as u32,
            seconds: (secs % 60) as u32,
            frames: frames as u32,
            drop,
        })
    }

    /// The frame (from zero) this label names at `rate`. A label drop-frame skips (the first
    /// frames of a minute not divisible by ten) is refused.
    pub fn to_frames(&self, rate: Rate) -> Result<i64, TimeError> {
        let base = rate.timecode_base().ok_or(TimeError::NoTimecode { rate })?;
        if self.frames >= base {
            return Err(TimeError::BadTimecode {
                why: "the frame field is at or past the rate's frames per second",
            });
        }
        let base = i64::from(base);
        let minutes = i64::from(self.hours) * 60 + i64::from(self.minutes);
        let mut frame = (minutes * 60 + i64::from(self.seconds)) * base + i64::from(self.frames);
        if self.drop {
            let d = i64::from(rate.drop_per_minute().ok_or(TimeError::BadTimecode {
                why: "drop-frame needs a 30000/1001 or 60000/1001 rate",
            })?);
            if self.minutes % 10 != 0 && self.seconds == 0 && i64::from(self.frames) < d {
                return Err(TimeError::BadTimecode {
                    why: "drop-frame skips this label",
                });
            }
            frame -= d * (minutes - minutes / 10);
        }
        Ok(frame)
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = if self.drop { ';' } else { ':' };
        write!(
            f,
            "{:02}:{:02}:{:02}{sep}{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

/// A sign a time was typed with: a relative time (`+2s`, `-10f`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    /// `+`: later.
    Plus,
    /// `-`: earlier.
    Minus,
}

/// A time a person or agent typed, read at a rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeInput {
    /// `Some` for a relative time.
    pub sign: Option<Sign>,
    /// The frame count it names (unsigned; the sign says which way).
    pub frames: i64,
    /// The same as a time: `frames` at the rate.
    pub time: Time,
}

/// Reads what a verb's time argument takes, at `rate`:
///
/// - a timecode, `HH:MM:SS:FF`, or `HH:MM:SS;FF` (drop-frame) at an NTSC rate;
/// - a frame count, `123f`;
/// - seconds, `12.5s`, `1001/24000s` or `3s`, which must be on the frame grid unless `snap`
///   says how to round ([`TimeError::OffGrid`] names the frames around it);
/// - any of these after `+` or `-`, for a relative time.
///
/// A bare number is refused: a time needs its unit.
pub fn parse_time(s: &str, rate: Rate, snap: Snap) -> Result<TimeInput, TimeError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(TimeError::Empty);
    }
    let (sign, body) = match s.as_bytes()[0] {
        b'+' => (Some(Sign::Plus), &s[1..]),
        b'-' => (Some(Sign::Minus), &s[1..]),
        _ => (None, s),
    };
    if body.is_empty() {
        return Err(TimeError::Empty);
    }
    let frames = if body.contains(':') || body.contains(';') {
        Timecode::parse(body)?.to_frames(rate)?
    } else if let Some(n) = body.strip_suffix('f') {
        i64::try_from(digits(n)?).map_err(|_| TimeError::Overflow)?
    } else if body.ends_with('s') {
        let t = Time::parse_seconds(body)?;
        if t.is_negative() {
            return Err(TimeError::Syntax {
                why: "a sign goes before the number",
            });
        }
        t.to_frames(rate, snap)?
    } else {
        return Err(TimeError::Syntax {
            why: "a time needs a unit: a timecode, f (frames) or s (seconds)",
        });
    };
    Ok(TimeInput {
        sign,
        frames,
        time: Time::frames(frames, rate)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_reduce_and_compare_exactly() {
        let a = Time::new(1001, 24000).unwrap();
        let b = Time::new(2002, 48000).unwrap();
        assert_eq!(a, b);
        assert_eq!((a.num(), a.den()), (1001, 24000));
        assert!(Time::new(1, 3).unwrap() < Time::new(334, 1000).unwrap());
        assert_eq!(Time::new(-6, 4).unwrap(), Time::new(-3, 2).unwrap());
        assert!(Time::new(1, 0).is_err());
    }

    #[test]
    fn arithmetic_is_checked() {
        let max = Time::new(i64::MAX, 1).unwrap();
        assert_eq!(max.checked_add(Time::seconds(1)), Err(TimeError::Overflow));
        assert_eq!(
            Time::new(1, 3)
                .unwrap()
                .checked_add(Time::new(1, 6).unwrap())
                .unwrap(),
            Time::new(1, 2).unwrap()
        );
    }
}
