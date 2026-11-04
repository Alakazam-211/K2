//! A small JSON value and writer for error reports and `capabilities` (no dependency).

use std::fmt::Write;

/// A JSON value. Objects keep their keys in insertion order (stable output).
///
/// `non_exhaustive`: another crate's `match` on it needs a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    /// A number with a fraction (a coordinate, say): always finite ([`Value::float`]).
    Float(Finite),
    Str(String),
    List(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

/// A finite `f64`: NaN and the infinities can't be built, so a [`Value`] always prints as
/// JSON. Equal by bit pattern, so `-0.0` and `0.0` are two values (and print as `-0` and
/// `0`), and `Value` keeps `Eq`.
#[derive(Debug, Clone, Copy)]
pub struct Finite(f64);

impl Finite {
    /// `f`, or `None` for NaN or an infinity.
    pub fn new(f: f64) -> Option<Finite> {
        f.is_finite().then_some(Finite(f))
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

impl PartialEq for Finite {
    fn eq(&self, other: &Finite) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Finite {}

impl From<Finite> for Value {
    fn from(f: Finite) -> Self {
        Value::Float(f)
    }
}

impl Value {
    /// An object from `(key, value)` pairs, in order.
    pub fn obj<K: Into<String>>(pairs: impl IntoIterator<Item = (K, Value)>) -> Value {
        Value::Obj(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// A list of values.
    pub fn list<T: Into<Value>>(items: impl IntoIterator<Item = T>) -> Value {
        Value::List(items.into_iter().map(Into::into).collect())
    }

    /// The value at `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::UInt(n) => Some(*n),
            Value::Int(n) => u64::try_from(*n).ok(),
            _ => None,
        }
    }

    /// A number: a [`Value::Float`], or an integer `f64` holds exactly (at most 2^53 from
    /// zero), as [`Value::as_u64`] takes an `Int` only when it fits.
    pub fn as_f64(&self) -> Option<f64> {
        const EXACT: u64 = 1 << 53;
        match self {
            Value::Float(f) => Some(f.get()),
            Value::UInt(n) if *n <= EXACT => Some(*n as f64),
            Value::Int(n) if n.unsigned_abs() <= EXACT => Some(*n as f64),
            _ => None,
        }
    }

    /// A [`Value::Float`] of `f`, or `None` for NaN or an infinity. There is no `From<f64>`,
    /// so a value that isn't finite can't slip in.
    pub fn float(f: f64) -> Option<Value> {
        Finite::new(f).map(Value::Float)
    }

    /// Compact JSON, one line.
    pub fn to_json(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, None, 0);
        s
    }

    /// Indented JSON (two spaces), ending without a newline.
    pub fn to_json_pretty(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, Some(2), 0);
        s
    }

    fn write(&self, out: &mut String, indent: Option<usize>, depth: usize) {
        let nl = |out: &mut String, d: usize| {
            if let Some(n) = indent {
                out.push('\n');
                out.extend(std::iter::repeat(' ').take(n * d));
            }
        };
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(n) => {
                let _ = write!(out, "{n}");
            }
            Value::UInt(n) => {
                let _ = write!(out, "{n}");
            }
            // Rust's shortest form that reads back as the same f64, never an exponent: `1.0`
            // prints `1`, `-0.0` prints `-0` (both JSON numbers)
            Value::Float(f) => {
                let _ = write!(out, "{}", f.get());
            }
            Value::Str(s) => string(out, s),
            Value::List(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                // short lists of scalars stay on one line in pretty output
                let flat = items
                    .iter()
                    .all(|v| !matches!(v, Value::List(_) | Value::Obj(_)));
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                        if flat && indent.is_some() {
                            out.push(' ');
                        }
                    }
                    if !flat {
                        nl(out, depth + 1);
                    }
                    v.write(out, indent, depth + 1);
                }
                if !flat {
                    nl(out, depth);
                }
                out.push(']');
            }
            Value::Obj(m) => {
                if m.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    nl(out, depth + 1);
                    string(out, k);
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    v.write(out, indent, depth + 1);
                }
                nl(out, depth);
                out.push('}');
            }
        }
    }
}

fn string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_owned())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}
impl From<&String> for Value {
    fn from(s: &String) -> Self {
        Value::Str(s.clone())
    }
}
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}
impl From<u8> for Value {
    fn from(n: u8) -> Self {
        Value::UInt(n.into())
    }
}
impl From<u32> for Value {
    fn from(n: u32) -> Self {
        Value::UInt(n.into())
    }
}
impl From<u64> for Value {
    fn from(n: u64) -> Self {
        Value::UInt(n)
    }
}
impl From<usize> for Value {
    fn from(n: usize) -> Self {
        Value::UInt(n as u64)
    }
}
impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Int(n)
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(o: Option<T>) -> Self {
        o.map_or(Value::Null, Into::into)
    }
}
impl From<(usize, usize)> for Value {
    /// A line range `[first, last]`.
    fn from((a, b): (usize, usize)) -> Self {
        Value::List(vec![Value::from(a), Value::from(b)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_orders() {
        let v = Value::obj([
            ("b", Value::from("a\"b\\c\n\u{1}")),
            ("a", Value::list([1u64, 2])),
            ("n", Value::Null),
        ]);
        assert_eq!(v.to_json(), r#"{"b":"a\"b\\c\n\u0001","a":[1,2],"n":null}"#);
        assert_eq!(
            v.to_json_pretty(),
            "{\n  \"b\": \"a\\\"b\\\\c\\n\\u0001\",\n  \"a\": [1, 2],\n  \"n\": null\n}"
        );
    }

    /// Every finite value prints as a JSON number that reads back as the same bits.
    #[test]
    fn floats_print_and_read_back() {
        let cases = [
            (0.5, "0.5"),
            (0.1, "0.1"),
            (-2.75, "-2.75"),
            (1.0, "1"),
            (-0.0, "-0"),
            (0.0, "0"),
            (123_456_789.125, "123456789.125"),
            (1e21, "1000000000000000000000"),
            (1e-7, "0.0000001"),
        ];
        for (f, want) in cases {
            let v = Value::float(f).expect("finite");
            let got = v.to_json();
            assert_eq!(got, want, "{f:?}");
            let back: f64 = got.parse().expect("a number");
            assert_eq!(back.to_bits(), f.to_bits(), "{f:?} read back as {back:?}");
            assert_eq!(v.as_f64().map(f64::to_bits), Some(f.to_bits()), "{f:?}");
        }
        for f in [
            f64::MAX,
            f64::MIN,
            f64::MIN_POSITIVE,
            5e-324,
            1e300,
            -1e-300,
        ] {
            let got = Value::float(f).expect("finite").to_json();
            assert!(!got.contains(['e', 'E', 'i', 'N']), "{f:?} printed {got}");
            let back: f64 = got.parse().expect("a number");
            assert_eq!(back.to_bits(), f.to_bits(), "{f:?} printed {got}");
        }
    }

    #[test]
    fn nan_and_infinities_are_refused() {
        for f in [f64::NAN, -f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(Finite::new(f).is_none(), "{f:?}");
            assert!(Value::float(f).is_none(), "{f:?}");
        }
    }

    #[test]
    fn floats_are_equal_by_bits() {
        assert_eq!(Value::float(0.5), Value::float(0.5));
        assert_eq!(Finite::new(-2.75), Finite::new(-2.75));
        assert_ne!(Value::float(0.0), Value::float(-0.0));
        assert_ne!(Value::float(1.0), Some(Value::UInt(1)));
        let v = Value::obj([("x", Value::float(0.25).unwrap())]);
        assert_eq!(v.clone(), v);
    }

    #[test]
    fn as_f64_takes_floats_and_exact_integers() {
        assert_eq!(Value::UInt(1 << 53).as_f64(), Some(9_007_199_254_740_992.0));
        assert_eq!(Value::UInt((1 << 53) + 1).as_f64(), None);
        assert_eq!(
            Value::Int(-(1 << 53)).as_f64(),
            Some(-9_007_199_254_740_992.0)
        );
        assert_eq!(Value::Int(i64::MIN).as_f64(), None);
        assert_eq!(Value::Int(-3).as_f64(), Some(-3.0));
        assert_eq!(Value::from("1").as_f64(), None);
        assert_eq!(Value::float(0.5).unwrap().as_u64(), None);
    }
}
