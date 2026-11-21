//! The shell's embedded Python, as the mail families' Rust reads it: what
//! `v[key]`, `v.get(key)`, `sep.join(v)`, `head + v` and `int(v)` give
//! for a value `json.loads` returned, or the exception's last line
//! (`TypeError: ...`) where Python raises. `mail watch`'s printers and
//! `_mail_py`'s ([`super::core`]) share them.

use super::super::{get, py_iter, py_space, s};
use crate::out::json::{Text, Value};
use crate::out::py;

/// `str(t)`: the string as Python prints it.
pub fn text(t: &Text) -> String {
    py::str_of(&Value::Str(t.clone()))
}

/// `v[key]`, or the exception's text.
pub fn py_index(v: &Value, key: &str) -> Result<Value, String> {
    match v {
        Value::Object(p) => get(p, key)
            .cloned()
            .ok_or_else(|| format!("KeyError: {}", py::repr(&s(key)))),
        Value::Array(_) => {
            Err("TypeError: list indices must be integers or slices, not str".into())
        }
        Value::Str(_) => Err("TypeError: string indices must be integers, not 'str'".into()),
        other => Err(format!(
            "TypeError: '{}' object is not subscriptable",
            py::type_name(other)
        )),
    }
}

/// `v.get(key)`, or the exception's text.
pub fn attr_get(v: &Value, key: &str) -> Result<Option<Value>, String> {
    match v {
        Value::Object(p) => Ok(get(p, key).cloned()),
        other => Err(format!(
            "AttributeError: '{}' object has no attribute 'get'",
            py::type_name(other)
        )),
    }
}

/// `for x in v`, or the exception's text.
pub fn iter(v: &Value) -> Result<Vec<Value>, String> {
    py_iter(v, 0).map_err(|r| r.exception)
}

/// `sep.join(v)`, or the exception's text.
pub fn join(sep: &str, v: &Value) -> Result<String, String> {
    let items = match v {
        Value::Array(_) | Value::Str(_) | Value::Object(_) => iter(v)?,
        _ => return Err("TypeError: can only join an iterable".into()),
    };
    let mut parts = Vec::with_capacity(items.len());
    for (n, item) in items.iter().enumerate() {
        match item {
            Value::Str(t) => parts.push(text(t)),
            other => {
                return Err(format!(
                    "TypeError: sequence item {n}: expected str instance, {} found",
                    py::type_name(other)
                ))
            }
        }
    }
    Ok(parts.join(sep))
}

pub fn concat_error(v: &Value) -> String {
    format!(
        "TypeError: can only concatenate str (not \"{}\") to str",
        py::type_name(v)
    )
}

/// `head + v`, or the exception's text.
pub fn concat(head: &str, v: &Value) -> Result<String, String> {
    match v {
        Value::Str(t) => Ok(format!("{head}{}", text(t))),
        other => Err(concat_error(other)),
    }
}

/// `int(v)` as decimal digits (a `-` first when negative), or the
/// exception's text.
pub fn py_int(v: &Value) -> Result<String, String> {
    match v {
        Value::Int(d) => Ok(d.clone()),
        Value::Bool(b) => Ok(if *b { "1" } else { "0" }.into()),
        Value::Float(f) if f.is_nan() => Err("ValueError: cannot convert float NaN to integer".into()),
        Value::Float(f) if f.is_infinite() => {
            Err("OverflowError: cannot convert float infinity to integer".into())
        }
        Value::Float(f) => {
            let t = format!("{:.0}", f.trunc());
            Ok(if t == "-0" { "0".into() } else { t })
        }
        Value::Str(t) => {
            let raw = text(t);
            let bad = || format!("ValueError: invalid literal for int() with base 10: {}", py::repr(v));
            let trimmed = raw.trim_matches(py_space);
            let (neg, body) = match trimmed.as_bytes().first() {
                Some(b'-') => (true, &trimmed[1..]),
                Some(b'+') => (false, &trimmed[1..]),
                _ => (false, trimmed),
            };
            let b = body.as_bytes();
            let ok = !b.is_empty()
                && b.first().is_some_and(u8::is_ascii_digit)
                && b.last().is_some_and(u8::is_ascii_digit)
                && b.iter().all(|c| c.is_ascii_digit() || *c == b'_')
                && !body.contains("__");
            if !ok {
                return Err(bad());
            }
            let digits: String = body.chars().filter(char::is_ascii_digit).collect();
            if digits.len() > big::MAX_DIGITS {
                return Err(format!(
                    "ValueError: Exceeds the limit ({}) for integer string conversion: value has {} digits; use sys.set_int_max_str_digits() to increase the limit",
                    big::MAX_DIGITS,
                    digits.len()
                ));
            }
            let digits = big::trim(&digits);
            Ok(if neg && digits != "0" {
                format!("-{digits}")
            } else {
                digits
            })
        }
        other => Err(format!(
            "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py::type_name(other)
        )),
    }
}

/// A string value.
pub fn sv(t: &str) -> Value {
    s(t)
}

/// `v.get(key, default)`, or the exception's text.
pub fn get_or(v: &Value, key: &str, default: Value) -> Result<Value, String> {
    Ok(attr_get(v, key)?.unwrap_or(default))
}

/// `x or default`, where `x` is a `get` (`None` when missing).
pub fn or(v: Option<Value>, default: Value) -> Value {
    v.filter(py::truthy).unwrap_or(default)
}

/// `if x:` where `x` is a `get`.
pub fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(py::truthy)
}

/// `v == text`.
pub fn eq_str(v: &Value, t: &str) -> bool {
    matches!(v, Value::Str(x) if x.as_str() == Some(t))
}

/// `item in container` for a string `item`, or the exception's text.
pub fn contains(container: &Value, item: &str) -> Result<bool, String> {
    match container {
        Value::Str(t) => Ok(text(t).contains(item)),
        Value::Array(items) => Ok(items.iter().any(|x| eq_str(x, item))),
        Value::Object(pairs) => Ok(pairs.iter().any(|(k, _)| k.as_str() == Some(item))),
        other => Err(format!(
            "TypeError: argument of type '{}' is not iterable",
            py::type_name(other)
        )),
    }
}

/// `a == b` for two values `json.loads` returned: numbers by value
/// (`True == 1`, `1 == 1.0`), strings by their code points, lists item by
/// item and dicts by their keys and values in any order.
pub fn py_eq(a: &Value, b: &Value) -> bool {
    fn num(v: &Value) -> Option<f64> {
        match v {
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            Value::Int(d) => d.parse().ok(),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x.code_points() == y.code_points(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(i, j)| py_eq(i, j))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter().all(|(k, v)| {
                    y.iter()
                        .find(|(k2, _)| k2.code_points() == k.code_points())
                        .is_some_and(|(_, v2)| py_eq(v, v2))
                })
        }
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

/// `item in container`, or the exception's text.
pub fn py_in(item: &Value, container: &Value) -> Result<bool, String> {
    match (item, container) {
        (Value::Str(t), _) => contains(container, &text(t)),
        (_, Value::Str(_)) => Err(format!(
            "TypeError: 'in <string>' requires string as left operand, not {}",
            py::type_name(item)
        )),
        (_, Value::Array(items)) => Ok(items.iter().any(|x| py_eq(x, item))),
        (Value::Array(_) | Value::Object(_), Value::Object(_)) => Err(format!(
            "TypeError: unhashable type: '{}'",
            py::type_name(item)
        )),
        // A dict's keys are strings: no other value is one of them.
        (_, Value::Object(_)) => Ok(false),
        _ => Err(format!(
            "TypeError: argument of type '{}' is not iterable",
            py::type_name(container)
        )),
    }
}

/// `v + tail` for a string `tail`, or the exception's text.
pub fn concat_str(v: &Value, tail: &str) -> Result<String, String> {
    match v {
        Value::Str(t) => Ok(format!("{}{tail}", text(t))),
        Value::Array(_) => Err("TypeError: can only concatenate list (not \"str\") to list".into()),
        other => Err(format!(
            "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
            py::type_name(other)
        )),
    }
}

/// `"%-<n>s" % text`: padded with spaces to `n` code points.
pub fn ljust(t: &str, n: usize) -> String {
    let len = t.chars().count();
    if len >= n {
        t.to_string()
    } else {
        format!("{t}{}", " ".repeat(n - len))
    }
}

/// `len(v)`, or the exception's text.
pub fn len(v: &Value) -> Result<usize, String> {
    match v {
        Value::Array(items) => Ok(items.len()),
        Value::Str(t) => Ok(t.code_points().len()),
        Value::Object(pairs) => Ok(pairs.len()),
        other => Err(format!(
            "TypeError: object of type '{}' has no len()",
            py::type_name(other)
        )),
    }
}

/// `{...}.get(key, default)` on a dict of strings: a key that is a list
/// or a dict can't be looked up.
pub fn lookup(table: &[(&str, &str)], key: &Value) -> Result<Option<String>, String> {
    match key {
        Value::Array(_) => Err("TypeError: unhashable type: 'list'".into()),
        Value::Object(_) => Err("TypeError: unhashable type: 'dict'".into()),
        Value::Str(t) => Ok(table
            .iter()
            .find(|(k, _)| t.as_str() == Some(k))
            .map(|(_, v)| (*v).to_string())),
        _ => Ok(None),
    }
}

/// Decimal integers as digit strings, for Python's unbounded `int`.
pub mod big {
    /// Python's limit on the digits of an `int` read from or printed as
    /// text.
    pub const MAX_DIGITS: usize = 4300;

    /// The digits without leading zeros ("0" for none).
    pub fn trim(d: &str) -> String {
        let t = d.trim_start_matches('0');
        if t.is_empty() {
            "0".into()
        } else {
            t.into()
        }
    }

    /// `a + b` for two non-negative numbers.
    pub fn add(a: &str, b: &str) -> String {
        let (a, b) = (a.as_bytes(), b.as_bytes());
        let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
        let (mut i, mut j, mut carry) = (a.len(), b.len(), 0u8);
        while i > 0 || j > 0 || carry > 0 {
            let mut sum = carry;
            if i > 0 {
                i -= 1;
                sum += a[i] - b'0';
            }
            if j > 0 {
                j -= 1;
                sum += b[j] - b'0';
            }
            out.push(b'0' + sum % 10);
            carry = sum / 10;
        }
        out.reverse();
        trim(&String::from_utf8(out).expect("digits"))
    }

    /// `d * m` for a non-negative number `d`.
    pub fn mul_small(d: &str, m: u32) -> String {
        let mut out = Vec::with_capacity(d.len() + 6);
        let mut carry: u64 = 0;
        for c in d.bytes().rev() {
            let x = u64::from(c - b'0') * u64::from(m) + carry;
            out.push(b'0' + (x % 10) as u8);
            carry = x / 10;
        }
        while carry > 0 {
            out.push(b'0' + (carry % 10) as u8);
            carry /= 10;
        }
        out.reverse();
        trim(&String::from_utf8(out).expect("digits"))
    }

    /// `n // m` (Python's floor division) for a number with an optional
    /// leading `-`.
    pub fn floor_div(n: &str, m: u32) -> String {
        let (neg, d) = match n.strip_prefix('-') {
            Some(d) => (true, d),
            None => (false, n),
        };
        let mut q = Vec::with_capacity(d.len());
        let mut rem: u64 = 0;
        for c in d.bytes() {
            let x = rem * 10 + u64::from(c - b'0');
            q.push(b'0' + (x / u64::from(m)) as u8);
            rem = x % u64::from(m);
        }
        let mut q = trim(&String::from_utf8(q).expect("digits"));
        if neg {
            if rem > 0 {
                q = add(&q, "1");
            }
            if q != "0" {
                q = format!("-{q}");
            }
        }
        q
    }
}
