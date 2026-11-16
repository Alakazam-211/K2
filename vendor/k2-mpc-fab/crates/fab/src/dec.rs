//! Exact decimals as a file writes them: `mant * 10^-scale`, never a float. A reader keeps
//! every decimal it reads this way, and converts it to a file's integer coordinate units only
//! when the conversion is exact.

use std::fmt;

/// Most digits a decimal may have (before and after the point together): within an `i64`.
pub const DEC_DIGITS_MAX: usize = 18;

/// An exact decimal: `mant * 10^-scale`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Dec {
    /// The digits as an integer, signed.
    pub mant: i64,
    /// How many of them are after the point.
    pub scale: u8,
}

/// Why a decimal didn't parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecError {
    /// Not the decimal syntax.
    Syntax,
    /// More digits than [`DEC_DIGITS_MAX`].
    TooLong,
}

impl Dec {
    /// Zero.
    pub const ZERO: Dec = Dec { mant: 0, scale: 0 };
    /// One.
    pub const ONE: Dec = Dec { mant: 1, scale: 0 };

    /// Parses `[+-]?(digits[.digits?] | .digits)`, the whole of `s`. `signed`: a sign may lead.
    pub fn parse(s: &[u8], signed: bool) -> Result<Dec, DecError> {
        let (neg, body) = match s.first() {
            Some(b'-') if signed => (true, &s[1..]),
            Some(b'+') if signed => (false, &s[1..]),
            _ => (false, s),
        };
        let (int, frac) = match body.iter().position(|&b| b == b'.') {
            Some(p) => (&body[..p], Some(&body[p + 1..])),
            None => (body, None),
        };
        let frac_digits = frac.unwrap_or(&[]);
        if int.is_empty() && frac_digits.is_empty() {
            return Err(DecError::Syntax);
        }
        if !int.iter().all(u8::is_ascii_digit) || !frac_digits.iter().all(u8::is_ascii_digit) {
            return Err(DecError::Syntax);
        }
        // leading zeros of the integer part and trailing zeros of the fraction carry no value
        let int = trim_start_zeros(int);
        let frac_digits = trim_end_zeros(frac_digits);
        if int.len() + frac_digits.len() > DEC_DIGITS_MAX {
            return Err(DecError::TooLong);
        }
        let mut mant: i64 = 0;
        for &d in int.iter().chain(frac_digits) {
            mant = mant * 10 + i64::from(d - b'0');
        }
        Ok(Dec {
            mant: if neg { -mant } else { mant },
            scale: frac_digits.len() as u8,
        })
    }

    /// The value in units of `10^-decimals`, when that is exact and fits an `i64`.
    pub fn to_units(self, decimals: u8) -> Option<i64> {
        if self.scale <= decimals {
            let k = 10i64.checked_pow(u32::from(decimals - self.scale))?;
            self.mant.checked_mul(k)
        } else {
            let k = 10i64.checked_pow(u32::from(self.scale - decimals))?;
            (self.mant % k == 0).then(|| self.mant / k)
        }
    }

    /// Whether the value is a whole number.
    pub fn is_integer(self) -> bool {
        self.to_units(0).is_some()
    }

    /// The value as a float (for checks that need one; never stored).
    pub fn to_f64(self) -> f64 {
        self.mant as f64 / 10f64.powi(i32::from(self.scale))
    }

    /// The sign: -1, 0 or 1.
    pub fn signum(self) -> i64 {
        self.mant.signum()
    }
}

fn trim_start_zeros(s: &[u8]) -> &[u8] {
    let n = s.iter().take_while(|&&b| b == b'0').count();
    &s[n..]
}

fn trim_end_zeros(s: &[u8]) -> &[u8] {
    let n = s.iter().rev().take_while(|&&b| b == b'0').count();
    &s[..s.len() - n]
}

impl fmt::Display for Dec {
    /// The shortest exact form: `0.25`, `-3`, `100`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let neg = self.mant < 0;
        let digits = self.mant.unsigned_abs().to_string();
        let scale = usize::from(self.scale);
        let s = if scale == 0 {
            digits
        } else if digits.len() > scale {
            format!(
                "{}.{}",
                &digits[..digits.len() - scale],
                &digits[digits.len() - scale..]
            )
        } else {
            format!("0.{}{}", "0".repeat(scale - digits.len()), digits)
        };
        if neg {
            write!(f, "-{s}")
        } else {
            f.write_str(&s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_parse_exactly() {
        let d = |s: &str| Dec::parse(s.as_bytes(), true);
        assert_eq!(d("0.25"), Ok(Dec { mant: 25, scale: 2 }));
        assert_eq!(d("-.5"), Ok(Dec { mant: -5, scale: 1 }));
        assert_eq!(d("+16"), Ok(Dec { mant: 16, scale: 0 }));
        assert_eq!(d("5."), Ok(Dec { mant: 5, scale: 0 }));
        assert_eq!(d("1.500"), Ok(Dec { mant: 15, scale: 1 }));
        assert_eq!(d("007"), Ok(Dec { mant: 7, scale: 0 }));
        for bad in ["", ".", "-", "1.2.3", "1e3", " 1", "1,5"] {
            assert_eq!(d(bad), Err(DecError::Syntax), "{bad:?}");
        }
        assert_eq!(Dec::parse(b"-1", false), Err(DecError::Syntax));
        assert_eq!(d("1234567890123456789"), Err(DecError::TooLong));
        assert_eq!(d("0.25").unwrap().to_units(6), Some(250_000));
        assert_eq!(d("0.0000005").unwrap().to_units(6), None);
        assert_eq!(d("2.000000").unwrap().to_units(0), Some(2));
        assert_eq!(d("-0.050").unwrap().to_string(), "-0.05");
        assert_eq!(d("12").unwrap().to_string(), "12");
    }
}
