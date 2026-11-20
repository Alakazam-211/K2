//! The few Python operations the shell's readers use on a value
//! `json.loads` gave, with the exception Python raises where the value is
//! not what the reader expects: `v[k]`, `k in v`, `", ".join(v)`, `x or
//! default`, Python's `float()` of a word, `a == b` and `x in v`. The rest is in
//! [`crate::families`] (`py_get`, `py_len`, `py_d`) and [`crate::out::py`].

use crate::families::{get, Raise};
use crate::out::json::{Text, Value};
use crate::out::py;

/// `v[key]` for a string key.
pub fn index<'a>(v: &'a Value, key: &str, line: u32) -> Result<&'a Value, Raise> {
    match v {
        Value::Object(pairs) => get(pairs, key).ok_or_else(|| {
            Raise::at(
                line,
                format!("KeyError: {}", py::repr(&Value::Str(Text::from(key)))),
            )
        }),
        Value::Array(_) => Err(Raise::at(
            line,
            "TypeError: list indices must be integers or slices, not str",
        )),
        Value::Str(_) => Err(Raise::at(
            line,
            "TypeError: string indices must be integers, not 'str'",
        )),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// `key in v`.
pub fn contains(v: &Value, key: &str, line: u32) -> Result<bool, Raise> {
    match v {
        Value::Object(pairs) => Ok(get(pairs, key).is_some()),
        Value::Array(items) => Ok(items
            .iter()
            .any(|i| matches!(i, Value::Str(t) if t.as_str() == Some(key)))),
        Value::Str(t) => Ok(py::str_of(&Value::Str(t.clone())).contains(key)),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: argument of type '{}' is not iterable",
                py::type_name(other)
            ),
        )),
    }
}

/// `sep.join(v)`.
pub fn join(v: &Value, sep: &str, line: u32) -> Result<String, Raise> {
    let items: Vec<Value> = match v {
        Value::Array(items) => items.clone(),
        Value::Object(pairs) => pairs.iter().map(|(k, _)| Value::Str(k.clone())).collect(),
        Value::Str(t) => {
            return Ok(t
                .code_points()
                .into_iter()
                .map(|cp| py::str_of(&Value::Str(crate::families::code_point(cp))))
                .collect::<Vec<_>>()
                .join(sep))
        }
        _ => return Err(Raise::at(line, "TypeError: can only join an iterable")),
    };
    let mut out = Vec::with_capacity(items.len());
    for (n, item) in items.iter().enumerate() {
        match item {
            Value::Str(_) => out.push(py::str_of(item)),
            other => {
                return Err(Raise::at(
                    line,
                    format!(
                        "TypeError: sequence item {n}: expected str instance, {} found",
                        py::type_name(other)
                    ),
                ))
            }
        }
    }
    Ok(out.join(sep))
}

/// `d.get(key)` on a value that must be a dict, else Python's
/// `AttributeError` at `line`.
pub fn get_in<'a>(v: &'a Value, key: &str, line: u32) -> Result<Option<&'a Value>, Raise> {
    crate::families::py_get(v, key, line)
}

/// `x or default`: `x` when it is there and true.
pub fn or<'a>(x: Option<&'a Value>, default: &'a Value) -> &'a Value {
    match x {
        Some(v) if py::truthy(v) => v,
        _ => default,
    }
}

/// `"%s" % x` for what `d.get(k)` gave (`None` when missing).
pub fn s_of(x: Option<&Value>) -> String {
    x.map_or_else(|| "None".to_string(), py::str_of)
}

/// Python's `float(word)`: spaces around it, a sign, `_` between digits,
/// `inf`, `infinity` and `nan` in any case. Digits other than ASCII ones
/// (which Python also reads) are not read here.
pub fn float(word: &str) -> Option<f64> {
    let w = word.trim_matches(crate::families::py_space);
    let (neg, body) = match w.as_bytes().first() {
        Some(b'-') => (true, &w[1..]),
        Some(b'+') => (false, &w[1..]),
        _ => (false, w),
    };
    let lower = body.to_ascii_lowercase();
    let f = match lower.as_str() {
        "inf" | "infinity" => f64::INFINITY,
        "nan" => f64::NAN,
        _ => {
            let digits = decimal(body)?;
            digits.parse::<f64>().ok()?
        }
    };
    Some(if neg { -f } else { f })
}

/// A decimal Python's `float()` reads, `_` taken out: `1_000.5`, `.5`,
/// `5.`, `1e-3`. `None` for anything else.
fn decimal(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = String::new();
    // Digits with single `_` between two digits; how many there were.
    let run = |i: &mut usize, out: &mut String| -> Option<usize> {
        let mut n = 0;
        while *i < b.len() {
            match b[*i] {
                d @ b'0'..=b'9' => {
                    out.push(char::from(d));
                    n += 1;
                    *i += 1;
                }
                b'_' if n > 0 && b.get(*i + 1).is_some_and(u8::is_ascii_digit) => *i += 1,
                b'_' => return None,
                _ => break,
            }
        }
        Some(n)
    };
    let whole = run(&mut i, &mut out)?;
    let mut frac = 0;
    if b.get(i) == Some(&b'.') {
        out.push('.');
        i += 1;
        frac = run(&mut i, &mut out)?;
    }
    if whole + frac == 0 {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        out.push('e');
        i += 1;
        if let Some(&s @ (b'+' | b'-')) = b.get(i) {
            out.push(char::from(s));
            i += 1;
        }
        if run(&mut i, &mut out)? == 0 {
            return None;
        }
    }
    (i == b.len()).then_some(out)
}

/// Python's `a == b` on two values `json.loads` gave: numbers by value
/// (`True == 1`, `1 == 1.0`), strings by code points, lists item by item,
/// dicts by their keys and values in any order.
pub fn py_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.iter().any(|(k2, v2)| k == k2 && py_eq(v, v2)))
        }
        _ => match (number(a), number(b)) {
            (Some(Num::Int(x)), Some(Num::Int(y))) => x == y,
            (Some(Num::Float(x)), Some(Num::Float(y))) => x == y,
            (Some(Num::Int(i)), Some(Num::Float(f))) | (Some(Num::Float(f)), Some(Num::Int(i))) => {
                int_eq_float(&i, f)
            }
            _ => false,
        },
    }
}

enum Num {
    Int(String),
    Float(f64),
}

fn number(v: &Value) -> Option<Num> {
    match v {
        Value::Bool(b) => Some(Num::Int(if *b { "1" } else { "0" }.into())),
        Value::Int(d) => Some(Num::Int(d.clone())),
        Value::Float(f) => Some(Num::Float(*f)),
        _ => None,
    }
}

/// An integer's digits against a float, exactly.
fn int_eq_float(digits: &str, f: f64) -> bool {
    if !f.is_finite() || f.fract() != 0.0 {
        return false;
    }
    if f == 0.0 {
        return digits == "0";
    }
    format!("{f:.0}") == digits
}

/// Python's `needle in hay`, or the `TypeError` at `line`: a substring in
/// a string, an item in a list, a key in a dict.
pub fn py_in(needle: &Value, hay: &Value, line: u32) -> Result<bool, Raise> {
    match hay {
        Value::Str(t) => match needle {
            Value::Str(n) => {
                let (h, n) = (t.code_points(), n.code_points());
                Ok(n.is_empty() || h.windows(n.len()).any(|w| w == n.as_slice()))
            }
            other => Err(Raise::at(
                line,
                format!(
                    "TypeError: 'in <string>' requires string as left operand, not {}",
                    py::type_name(other)
                ),
            )),
        },
        Value::Array(items) => Ok(items.iter().any(|x| py_eq(x, needle))),
        Value::Object(pairs) => match needle {
            Value::Array(_) | Value::Object(_) => Err(Raise::at(
                line,
                format!("TypeError: unhashable type: '{}'", py::type_name(needle)),
            )),
            _ => Ok(pairs
                .iter()
                .any(|(k, _)| py_eq(&Value::Str(k.clone()), needle))),
        },
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: argument of type '{}' is not iterable",
                py::type_name(other)
            ),
        )),
    }
}

/// `s.splitlines()`: at `\n`, `\r`, `\r\n`, `\v`, `\f`, `\x1c` to
/// `\x1e`, `\x85`, `\u2028` and `\u2029`, the ends dropped.
pub fn splitlines(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(std::mem::take(&mut cur));
            }
            '\n' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::out::json::parse;

    #[test]
    fn python_s_float() {
        for (w, want) in [
            ("10", 10.0),
            (" 1_000.5\n", 1000.5),
            (".5", 0.5),
            ("5.", 5.0),
            ("-1e-3", -0.001),
            ("+2E2", 200.0),
            ("1e500", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
        ] {
            assert_eq!(float(w), Some(want), "{w:?}");
        }
        assert!(float("NaN").is_some_and(f64::is_nan));
        for bad in [
            "", "x", "1__0", "_1", "1_", "1_.5", "1e", ".", "1.2.3", "0x10", "1 0", "--1",
        ] {
            assert_eq!(float(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn index_contains_and_join_raise_as_python_does() {
        let v = parse(r#"{"a":["x","y"],"n":5}"#).unwrap();
        assert_eq!(join(index(&v, "a", 1).unwrap(), ", ", 1).unwrap(), "x, y");
        assert_eq!(index(&v, "b", 3).unwrap_err().exception, "KeyError: 'b'");
        assert_eq!(
            contains(index(&v, "n", 1).unwrap(), "x", 4)
                .unwrap_err()
                .exception,
            "TypeError: argument of type 'int' is not iterable"
        );
        assert_eq!(
            join(&parse("[1]").unwrap(), ", ", 2).unwrap_err().exception,
            "TypeError: sequence item 0: expected str instance, int found"
        );
        assert!(contains(&parse(r#"["export"]"#).unwrap(), "export", 1).unwrap());
        assert!(contains(&parse(r#""an export""#).unwrap(), "export", 1).unwrap());
    }
}
