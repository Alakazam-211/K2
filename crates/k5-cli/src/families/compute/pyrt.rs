//! The few Python operations the shell's compute readers use, on values
//! `json.loads` gave, each as Python does it, or the exception it raises
//! (its last traceback line, `TypeError: …`).
//!
//! - `%` formatting ([`pct`]): the flags `-`, `+`, ` ` and `0`, a width, a
//!   precision, and `s`, `d`, `f`, `g` and `%`.
//! - `d.get(k)` ([`get`]), `d[k]` ([`idx`]), `k in d` ([`has`]), `for x in
//!   v` ([`iter`]), `len(v)` ([`len`]), `v[a:b]` ([`slice`]), `sep.join(v)`
//!   ([`join`]).
//! - Numbers: `+`, `-`, `*`, `/`, `//`, `%`, `<` and friends, `int()`,
//!   `round()` and `max()`. An integer is held in 128 bits; one past that
//!   (the daemon writes none) is read as a float.
//! - `time.strftime(fmt, time.localtime(t))` ([`strftime`]): the C
//!   library's `localtime_r` and `strftime`, as Python's are, so the same
//!   `TZ` and the C locale's month names. `None` is now.
//!
//! Where this differs from Python: a string with half of a surrogate pair
//! in it is handled as its `str()` text (`\udXXX`), which the daemon never
//! sends; exception wording follows Python 3.12.

use crate::out::json::{self, Options, Text, Value};
use crate::out::py;

/// An exception: its last traceback line.
pub type Exc = String;
/// A value, or the exception Python raises.
pub type R<T> = Result<T, Exc>;

/// A string value.
pub fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// An integer value.
pub fn int(n: i128) -> Value {
    Value::Int(n.to_string())
}

/// `type(v).__name__`.
pub fn tname(v: &Value) -> &'static str {
    py::type_name(v)
}

/// `bool(v)`.
pub fn truthy(v: &Value) -> bool {
    py::truthy(v)
}

/// `a or b`.
pub fn or(a: Value, b: Value) -> Value {
    if truthy(&a) {
        a
    } else {
        b
    }
}

/// `str(v)`.
pub fn str_of(v: &Value) -> String {
    py::str_of(v)
}

/// `isinstance(v, dict)`.
pub fn is_dict(v: &Value) -> bool {
    matches!(v, Value::Object(_))
}

/// `isinstance(v, list)`.
pub fn is_list(v: &Value) -> bool {
    matches!(v, Value::Array(_))
}

/// `v is None`.
pub fn is_none(v: &Value) -> bool {
    matches!(v, Value::Null)
}

fn not_subscriptable(v: &Value) -> Exc {
    format!("TypeError: '{}' object is not subscriptable", tname(v))
}

/// `v.get(key)`.
pub fn get(v: &Value, key: &str) -> R<Value> {
    match v {
        Value::Object(pairs) => Ok(super::super::get(pairs, key)
            .cloned()
            .unwrap_or(Value::Null)),
        other => Err(format!(
            "AttributeError: '{}' object has no attribute 'get'",
            tname(other)
        )),
    }
}

/// `v.get(key, default)`.
pub fn get_or(v: &Value, key: &str, default: Value) -> R<Value> {
    match v {
        Value::Object(pairs) => Ok(super::super::get(pairs, key).cloned().unwrap_or(default)),
        other => Err(format!(
            "AttributeError: '{}' object has no attribute 'get'",
            tname(other)
        )),
    }
}

/// `v[key]`, a string key.
pub fn idx(v: &Value, key: &str) -> R<Value> {
    match v {
        Value::Object(pairs) => super::super::get(pairs, key)
            .cloned()
            .ok_or_else(|| format!("KeyError: {}", py::repr(&s(key)))),
        Value::Array(_) => {
            Err("TypeError: list indices must be integers or slices, not str".into())
        }
        Value::Str(_) => Err("TypeError: string indices must be integers, not 'str'".into()),
        other => Err(not_subscriptable(other)),
    }
}

/// `v[i]`, an integer index (negative from the end).
pub fn at(v: &Value, i: i64) -> R<Value> {
    let pick = |n: usize| -> Option<usize> {
        if i < 0 {
            n.checked_sub(i.unsigned_abs() as usize)
        } else {
            let i = i as usize;
            (i < n).then_some(i)
        }
    };
    match v {
        Value::Array(items) => pick(items.len())
            .map(|k| items[k].clone())
            .ok_or_else(|| "IndexError: list index out of range".into()),
        Value::Str(t) => {
            let cps = t.code_points();
            pick(cps.len())
                .map(|k| Value::Str(super::super::code_point(cps[k])))
                .ok_or_else(|| "IndexError: string index out of range".into())
        }
        Value::Object(pairs) => {
            let key = int(i128::from(i));
            pairs
                .iter()
                .find(|(k, _)| eq(&Value::Str(k.clone()), &key))
                .map(|(_, v)| v.clone())
                .ok_or_else(|| format!("KeyError: {i}"))
        }
        other => Err(not_subscriptable(other)),
    }
}

/// `key in v`, a string key.
pub fn has(v: &Value, key: &str) -> R<bool> {
    match v {
        Value::Object(pairs) => Ok(super::super::get(pairs, key).is_some()),
        Value::Array(items) => Ok(items.iter().any(|x| eq(x, &s(key)))),
        Value::Str(t) => Ok(str_text(t).contains(key)),
        other => Err(format!(
            "TypeError: argument of type '{}' is not iterable",
            tname(other)
        )),
    }
}

fn str_text(t: &Text) -> String {
    py::str_of(&Value::Str(t.clone()))
}

/// `for x in v`: a list's items, a string's characters, a dict's keys.
pub fn iter(v: &Value) -> R<Vec<Value>> {
    match v {
        Value::Array(items) => Ok(items.clone()),
        Value::Str(t) => Ok(t
            .code_points()
            .into_iter()
            .map(|cp| Value::Str(super::super::code_point(cp)))
            .collect()),
        Value::Object(pairs) => Ok(pairs.iter().map(|(k, _)| Value::Str(k.clone())).collect()),
        other => Err(format!(
            "TypeError: '{}' object is not iterable",
            tname(other)
        )),
    }
}

/// `v or []`, then walked: what `for x in v.get(k) or []` walks.
pub fn iter_or_empty(v: Value) -> R<Vec<Value>> {
    if truthy(&v) {
        iter(&v)
    } else {
        Ok(Vec::new())
    }
}

/// `len(v)`.
pub fn len(v: &Value) -> R<usize> {
    match v {
        Value::Array(items) => Ok(items.len()),
        Value::Str(t) => Ok(t.code_points().len()),
        Value::Object(pairs) => Ok(pairs.len()),
        other => Err(format!(
            "TypeError: object of type '{}' has no len()",
            tname(other)
        )),
    }
}

/// `v[start:end]` (`None` for an open end).
pub fn slice(v: &Value, start: Option<i64>, end: Option<i64>) -> R<Value> {
    fn bounds(n: usize, start: Option<i64>, end: Option<i64>) -> (usize, usize) {
        let fix = |x: i64| -> usize {
            if x < 0 {
                (n as i64 + x).max(0) as usize
            } else {
                (x as usize).min(n)
            }
        };
        let a = start.map_or(0, fix);
        let b = end.map_or(n, fix);
        (a, b.max(a))
    }
    match v {
        Value::Str(t) => {
            let cps = t.code_points();
            let (a, b) = bounds(cps.len(), start, end);
            let text: String = cps[a..b]
                .iter()
                .map(|cp| str_text(&super::super::code_point(*cp)))
                .collect();
            Ok(s(&text))
        }
        Value::Array(items) => {
            let (a, b) = bounds(items.len(), start, end);
            Ok(Value::Array(items[a..b].to_vec()))
        }
        // Python 3.12: a slice is hashable, so a dict looks it up.
        Value::Object(_) => {
            let part = |x: Option<i64>| x.map_or("None".to_string(), |n| n.to_string());
            Err(format!(
                "KeyError: slice({}, {}, None)",
                part(start),
                part(end)
            ))
        }
        other => Err(not_subscriptable(other)),
    }
}

/// `sep.join(v)`.
pub fn join(sep: &str, v: &Value) -> R<String> {
    let items = match v {
        Value::Array(_) | Value::Str(_) | Value::Object(_) => iter(v)?,
        _ => return Err("TypeError: can only join an iterable".into()),
    };
    let mut parts = Vec::with_capacity(items.len());
    for (i, x) in items.iter().enumerate() {
        match x {
            Value::Str(t) => parts.push(str_text(t)),
            other => {
                return Err(format!(
                    "TypeError: sequence item {i}: expected str instance, {} found",
                    tname(other)
                ))
            }
        }
    }
    Ok(parts.join(sep))
}

/// `sep.join(items)` over strings this code made.
pub fn join_strs(sep: &str, items: &[String]) -> String {
    items.join(sep)
}

/// `v.startswith(prefix)`.
pub fn startswith(v: &Value, prefix: &str) -> R<bool> {
    match v {
        Value::Str(t) => Ok(str_text(t).starts_with(prefix)),
        other => Err(format!(
            "AttributeError: '{}' object has no attribute 'startswith'",
            tname(other)
        )),
    }
}

/// `v.replace(a, b)`.
pub fn replace(v: &Value, a: &str, b: &str) -> R<String> {
    match v {
        Value::Str(t) => Ok(str_text(t).replace(a, b)),
        other => Err(format!(
            "AttributeError: '{}' object has no attribute 'replace'",
            tname(other)
        )),
    }
}

/// `v.splitlines()`.
pub fn splitlines(v: &Value) -> R<Vec<String>> {
    match v {
        Value::Str(t) => Ok(py_splitlines(&str_text(t))),
        other => Err(format!(
            "AttributeError: '{}' object has no attribute 'splitlines'",
            tname(other)
        )),
    }
}

/// Python's `str.splitlines()`: the line ends it splits at.
pub fn py_splitlines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
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

/// `json.dumps(v)` (`compact`: `separators=(",", ":")`; `indent`).
pub fn dumps(v: &Value, compact: bool, indent: Option<usize>) -> R<String> {
    let o = Options {
        compact,
        indent,
        ..Options::default()
    };
    json::to_string(v, &o).map_err(|e| format!("ValueError: {e}"))
}

/// `sorted(v.items())`: a dict's pairs by key (keys never tie).
pub fn sorted_items(v: &Value) -> R<Vec<(Value, Value)>> {
    match v {
        Value::Object(pairs) => {
            let mut p: Vec<(Text, Value)> = pairs.clone();
            p.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(p.into_iter().map(|(k, v)| (Value::Str(k), v)).collect())
        }
        other => Err(format!(
            "AttributeError: '{}' object has no attribute 'items'",
            tname(other)
        )),
    }
}

// ---- numbers -------------------------------------------------------------

/// A number as Python holds it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum N {
    I(i128),
    F(f64),
}

impl N {
    fn f(self) -> f64 {
        match self {
            N::I(i) => i as f64,
            N::F(f) => f,
        }
    }

    pub fn value(self) -> Value {
        match self {
            N::I(i) => int(i),
            N::F(f) => Value::Float(f),
        }
    }
}

/// The number in `v` (a bool is 0 or 1), if it is one.
pub fn num(v: &Value) -> Option<N> {
    match v {
        Value::Bool(b) => Some(N::I(i128::from(*b))),
        Value::Int(d) => Some(match d.parse::<i128>() {
            Ok(i) => N::I(i),
            Err(_) => N::F(d.parse::<f64>().unwrap_or(f64::INFINITY)),
        }),
        Value::Float(f) => Some(N::F(*f)),
        _ => None,
    }
}

fn unsupported(op: &str, a: &Value, b: &Value) -> Exc {
    format!(
        "TypeError: unsupported operand type(s) for {op}: '{}' and '{}'",
        tname(a),
        tname(b)
    )
}

fn nums(op: &str, a: &Value, b: &Value) -> R<(N, N)> {
    match (num(a), num(b)) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(unsupported(op, a, b)),
    }
}

/// `a + b`.
pub fn add(a: &Value, b: &Value) -> R<Value> {
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => Ok(s(&(str_text(x) + &str_text(y)))),
        (Value::Str(_), other) => Err(format!(
            "TypeError: can only concatenate str (not \"{}\") to str",
            tname(other)
        )),
        (Value::Array(x), Value::Array(y)) => {
            Ok(Value::Array(x.iter().chain(y.iter()).cloned().collect()))
        }
        (Value::Array(_), other) => Err(format!(
            "TypeError: can only concatenate list (not \"{}\") to list",
            tname(other)
        )),
        _ => {
            let (x, y) = nums("+", a, b)?;
            Ok(match (x, y) {
                (N::I(p), N::I(q)) => p.checked_add(q).map_or(N::F(p as f64 + q as f64), N::I),
                _ => N::F(x.f() + y.f()),
            }
            .value())
        }
    }
}

/// `a += b`: a list takes `b`'s items (any iterable), else `a + b`.
pub fn iadd(a: &Value, b: &Value) -> R<Value> {
    match a {
        Value::Array(x) => {
            let mut x = x.clone();
            x.extend(iter(b)?);
            Ok(Value::Array(x))
        }
        _ => add(a, b),
    }
}

/// `a - b`.
pub fn sub(a: &Value, b: &Value) -> R<Value> {
    let (x, y) = nums("-", a, b)?;
    Ok(match (x, y) {
        (N::I(p), N::I(q)) => p.checked_sub(q).map_or(N::F(p as f64 - q as f64), N::I),
        _ => N::F(x.f() - y.f()),
    }
    .value())
}

/// `a * b` (numbers only).
pub fn mul(a: &Value, b: &Value) -> R<Value> {
    let (x, y) = nums("*", a, b)?;
    Ok(match (x, y) {
        (N::I(p), N::I(q)) => p.checked_mul(q).map_or(N::F(p as f64 * q as f64), N::I),
        _ => N::F(x.f() * y.f()),
    }
    .value())
}

/// `a / b`.
pub fn div(a: &Value, b: &Value) -> R<Value> {
    let (x, y) = nums("/", a, b)?;
    if y.f() == 0.0 {
        return Err(match (x, y) {
            (N::I(_), N::I(_)) => "ZeroDivisionError: division by zero".into(),
            _ => "ZeroDivisionError: float division by zero".into(),
        });
    }
    Ok(Value::Float(match (x, y) {
        // Exact below 2^53, as Python's int / int.
        (N::I(p), N::I(q)) if p.unsigned_abs() < (1 << 53) && q.unsigned_abs() < (1 << 53) => {
            p as f64 / q as f64
        }
        _ => x.f() / y.f(),
    }))
}

/// `a // b`.
pub fn floordiv(a: &Value, b: &Value) -> R<Value> {
    let (x, y) = nums("//", a, b)?;
    match (x, y) {
        (N::I(_), N::I(0)) => Err("ZeroDivisionError: integer division or modulo by zero".into()),
        (N::I(p), N::I(q)) => Ok(int(
            p.div_euclid(q) - i128::from(q < 0 && p.rem_euclid(q) != 0)
        )),
        _ if y.f() == 0.0 => Err("ZeroDivisionError: float floor division by zero".into()),
        _ => {
            let (p, q) = (x.f(), y.f());
            let m = py_fmod(p, q);
            Ok(Value::Float(((p - m) / q).round()))
        }
    }
}

/// `a % b` (numbers).
pub fn rem(a: &Value, b: &Value) -> R<Value> {
    let (x, y) = nums("%", a, b)?;
    match (x, y) {
        (N::I(_), N::I(0)) => Err("ZeroDivisionError: integer division or modulo by zero".into()),
        (N::I(p), N::I(q)) => {
            let r = p.rem_euclid(q);
            Ok(int(if q < 0 && r != 0 { r + q } else { r }))
        }
        _ if y.f() == 0.0 => Err("ZeroDivisionError: float modulo".into()),
        _ => Ok(Value::Float(py_fmod(x.f(), y.f()))),
    }
}

fn py_fmod(p: f64, q: f64) -> f64 {
    let m = p % q;
    if m != 0.0 && ((m < 0.0) != (q < 0.0)) {
        m + q
    } else if m == 0.0 {
        0.0_f64.copysign(q)
    } else {
        m
    }
}

/// `a == b`.
pub fn eq(a: &Value, b: &Value) -> bool {
    match (num(a), num(b)) {
        (Some(N::I(x)), Some(N::I(y))) => x == y,
        (Some(x), Some(y)) => x.f() == y.f(),
        _ => match (a, b) {
            (Value::Null, Value::Null) => true,
            (Value::Str(x), Value::Str(y)) => x == y,
            (Value::Array(x), Value::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(p, q)| eq(p, q))
            }
            (Value::Object(x), Value::Object(y)) => {
                x.len() == y.len()
                    && x.iter().all(|(k, v)| {
                        y.iter()
                            .find(|(k2, _)| k2 == k)
                            .is_some_and(|(_, w)| eq(v, w))
                    })
            }
            _ => false,
        },
    }
}

/// `a == "text"`.
pub fn eq_s(a: &Value, text: &str) -> bool {
    matches!(a, Value::Str(t) if str_text(t) == text)
}

/// `a < b`, `a <= b`, `a > b` or `a >= b` (`op` names it).
pub fn cmp(op: &str, a: &Value, b: &Value) -> R<bool> {
    let ord = match (num(a), num(b)) {
        (Some(N::I(x)), Some(N::I(y))) => Some(x.cmp(&y)),
        (Some(x), Some(y)) => x.f().partial_cmp(&y.f()),
        _ => match (a, b) {
            (Value::Str(x), Value::Str(y)) => Some(x.cmp(y)),
            _ => {
                return Err(format!(
                    "TypeError: '{op}' not supported between instances of '{}' and '{}'",
                    tname(a),
                    tname(b)
                ))
            }
        },
    };
    use std::cmp::Ordering::*;
    Ok(match (op, ord) {
        (_, None) => false,
        ("<", Some(o)) => o == Less,
        ("<=", Some(o)) => o != Greater,
        (">", Some(o)) => o == Greater,
        (">=", Some(o)) => o != Less,
        _ => unreachable!("cmp takes <, <=, > or >="),
    })
}

/// `int(v)`.
pub fn to_int(v: &Value) -> R<i128> {
    match v {
        Value::Str(t) => {
            let text = str_text(t);
            let trimmed = text.trim_matches(super::super::py_space).replace('_', "");
            trimmed.parse::<i128>().map_err(|_| {
                format!(
                    "ValueError: invalid literal for int() with base 10: {}",
                    py::repr(v)
                )
            })
        }
        Value::Float(f) if f.is_nan() => Err("ValueError: cannot convert float NaN to integer".into()),
        Value::Float(f) if f.is_infinite() => {
            Err("OverflowError: cannot convert float infinity to integer".into())
        }
        Value::Float(f) => Ok(f.trunc() as i128),
        _ => match num(v) {
            Some(N::I(i)) => Ok(i),
            Some(N::F(f)) => Ok(f.trunc() as i128),
            None => Err(format!(
                "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
                tname(v)
            )),
        },
    }
}

/// `round(v)`: an int, ties to even.
pub fn round0(v: &Value) -> R<Value> {
    match num(v) {
        Some(N::I(i)) => Ok(int(i)),
        Some(N::F(f)) if f.is_nan() => {
            Err("ValueError: cannot convert float NaN to integer".into())
        }
        Some(N::F(f)) if f.is_infinite() => {
            Err("OverflowError: cannot convert float infinity to integer".into())
        }
        Some(N::F(f)) => Ok(int(f.round_ties_even() as i128)),
        None => Err(format!(
            "TypeError: type {} doesn't define __round__ method",
            tname(v)
        )),
    }
}

/// `round(v, n)`: an int stays one; a float rounds correctly to `n`
/// decimals, ties to even.
pub fn round_n(v: &Value, n: usize) -> R<Value> {
    match num(v) {
        Some(N::I(i)) => Ok(int(i)),
        Some(N::F(f)) if !f.is_finite() => Ok(Value::Float(f)),
        Some(N::F(f)) => Ok(Value::Float(format!("{f:.n$}").parse::<f64>().unwrap_or(f))),
        None => Err(format!(
            "TypeError: type {} doesn't define __round__ method",
            tname(v)
        )),
    }
}

/// `max(a, b)`.
pub fn max2(a: &Value, b: &Value) -> R<Value> {
    // Python keeps the first when they are equal.
    Ok(if cmp(">", b, a)? {
        b.clone()
    } else {
        a.clone()
    })
}

// ---- % formatting --------------------------------------------------------

/// `fmt % args`.
pub fn pct(fmt: &str, args: &[Value]) -> R<String> {
    let mut out = String::new();
    let mut it = args.iter();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let (mut left, mut plus, mut space, mut zero) = (false, false, false, false);
        while let Some(&f) = chars.peek() {
            match f {
                '-' => left = true,
                '+' => plus = true,
                ' ' => space = true,
                '0' => zero = true,
                _ => break,
            }
            chars.next();
        }
        let mut width = 0usize;
        while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
            width = width * 10 + d as usize;
            chars.next();
        }
        let mut precision = None;
        if chars.peek() == Some(&'.') {
            chars.next();
            let mut p = 0usize;
            while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
                p = p * 10 + d as usize;
                chars.next();
            }
            precision = Some(p);
        }
        let conv = chars.next().expect("a conversion after %");
        if conv == '%' {
            out.push('%');
            continue;
        }
        let v = it
            .next()
            .ok_or_else(|| "TypeError: not enough arguments for format string".to_string())?;
        let (sign, body) = match conv {
            's' => {
                let mut t = str_of(v);
                if let Some(p) = precision {
                    t = t.chars().take(p).collect();
                }
                (String::new(), t)
            }
            'r' => (String::new(), py::repr(v)),
            'd' | 'i' => {
                let digits = py::percent_d(v)?;
                split_sign(digits)
            }
            'f' | 'g' => {
                let f = float_arg(v)?;
                let text = if conv == 'f' {
                    fixed(f, precision.unwrap_or(6))
                } else {
                    general(f, precision.unwrap_or(6))
                };
                split_sign(text)
            }
            other => panic!("pct: no %{other} here"),
        };
        let numeric = matches!(conv, 'd' | 'i' | 'f' | 'g');
        let sign = if numeric && sign.is_empty() {
            if plus {
                "+".to_string()
            } else if space {
                " ".to_string()
            } else {
                sign
            }
        } else {
            sign
        };
        let n = sign.chars().count() + body.chars().count();
        if n >= width {
            out.push_str(&sign);
            out.push_str(&body);
        } else if left {
            out.push_str(&sign);
            out.push_str(&body);
            out.push_str(&" ".repeat(width - n));
        } else if zero
            && numeric
            && body
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == 'e' || c == '+' || c == '-')
        {
            out.push_str(&sign);
            out.push_str(&"0".repeat(width - n));
            out.push_str(&body);
        } else {
            out.push_str(&" ".repeat(width - n));
            out.push_str(&sign);
            out.push_str(&body);
        }
    }
    if it.next().is_some() {
        return Err("TypeError: not all arguments converted during string formatting".into());
    }
    Ok(out)
}

fn split_sign(text: String) -> (String, String) {
    match text.strip_prefix('-') {
        Some(rest) => ("-".to_string(), rest.to_string()),
        None => (String::new(), text),
    }
}

/// `float(v)` for `%f` and `%g`.
fn float_arg(v: &Value) -> R<f64> {
    match num(v) {
        Some(N::I(i)) => Ok(i as f64),
        Some(N::F(f)) => Ok(f),
        None => Err(format!("TypeError: must be real number, not {}", tname(v))),
    }
}

fn special(f: f64) -> Option<String> {
    if f.is_nan() {
        Some("nan".into())
    } else if f.is_infinite() {
        Some(if f > 0.0 { "inf" } else { "-inf" }.into())
    } else {
        None
    }
}

/// `'%.<p>f' % f`.
pub fn fixed(f: f64, p: usize) -> String {
    special(f).unwrap_or_else(|| format!("{f:.p$}"))
}

/// `'%.<p>g' % f`.
pub fn general(f: f64, p: usize) -> String {
    if let Some(t) = special(f) {
        return t;
    }
    let p = p.max(1);
    if f == 0.0 {
        return if f.is_sign_negative() { "-0" } else { "0" }.into();
    }
    let e_text = format!("{:.*e}", p - 1, f);
    let (mantissa, exp) = e_text.split_once('e').expect("{:e} has an e");
    let exp: i32 = exp.parse().expect("{:e}'s exponent");
    if exp >= -4 && exp < p as i32 {
        let decimals = (p as i32 - 1 - exp).max(0) as usize;
        strip_zeros(&format!("{f:.decimals$}"))
    } else {
        let m = strip_zeros(mantissa);
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{m}e{sign}{:02}", exp.abs())
    }
}

fn strip_zeros(t: &str) -> String {
    if t.contains('.') {
        t.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        t.to_string()
    }
}

// ---- time ------------------------------------------------------------------

/// The seconds `time.localtime(v)` takes (`None`: `now`).
pub fn secs(v: &Value, now: i64) -> R<i64> {
    const RANGE: &str = "OverflowError: timestamp out of range for platform time_t";
    match v {
        Value::Null => Ok(now),
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Int(digits) => digits.parse().map_err(|_| RANGE.to_string()),
        Value::Float(f) if f.is_nan() => Err("ValueError: Invalid value NaN (not a number)".into()),
        Value::Float(f) => {
            let w = f.floor();
            if !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&w) {
                return Err(RANGE.into());
            }
            Ok(w as i64)
        }
        other => Err(format!(
            "TypeError: '{}' object cannot be interpreted as an integer",
            tname(other)
        )),
    }
}

/// `time.strftime(fmt, time.localtime(v))`.
pub fn strftime(fmt: &str, v: &Value, now: i64) -> R<String> {
    local_text(fmt, secs(v, now)?)
}

/// The local time of `secs` by `fmt`, by `localtime_r` and `strftime`;
/// `OSError` where `localtime_r` fails.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
pub fn local_text(fmt: &str, secs: i64) -> R<String> {
    let t = secs as libc::time_t;
    let cfmt = std::ffi::CString::new(fmt).expect("a format with no NUL");
    // SAFETY: tzset reads TZ; localtime_r and strftime write only into the
    // buffers given, each sized as passed.
    unsafe {
        extern "C" {
            fn tzset();
        }
        tzset();
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            let e = std::io::Error::last_os_error();
            let n = e.raw_os_error().unwrap_or(0);
            let text = e.to_string();
            let msg = text
                .strip_suffix(&format!(" (os error {n})"))
                .unwrap_or(&text);
            return Err(format!("OSError: [Errno {n}] {msg}"));
        }
        let mut buf = [0u8; 128];
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), cfmt.as_ptr(), &tm);
        Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    }
}

/// Off Unix, the C library there ([`crate::out::pylib::time::strftime_local`]).
#[cfg(not(unix))]
pub fn local_text(fmt: &str, secs: i64) -> R<String> {
    crate::out::pylib::time::strftime_local(secs, fmt)
}

/// `int(time.time())`.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_g_as_python() {
        for (f, want) in [
            (0.0, "0"),
            (1.0, "1"),
            (0.5, "0.5"),
            (1.5, "1.5"),
            (12.0, "12"),
            (123456.0, "123456"),
            (1234567.0, "1.23457e+06"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (2.25, "2.25"),
            (7.8, "7.8"),
            (-3.5, "-3.5"),
            (1e16, "1e+16"),
        ] {
            assert_eq!(general(f, 6), want, "{f}");
        }
    }

    #[test]
    fn percent_formats() {
        let n = |x: i128| int(x);
        assert_eq!(pct("%-5s|", &[s("ab")]).unwrap(), "ab   |");
        assert_eq!(pct("%5s|", &[s("ab")]).unwrap(), "   ab|");
        assert_eq!(pct("%02d", &[n(5)]).unwrap(), "05");
        assert_eq!(pct("%03d", &[n(-5)]).unwrap(), "-05");
        assert_eq!(pct("%+d", &[n(5)]).unwrap(), "+5");
        assert_eq!(pct("%+d", &[n(-5)]).unwrap(), "-5");
        assert_eq!(pct("%4d", &[n(12)]).unwrap(), "  12");
        assert_eq!(pct("%-4s", &[Value::Null]).unwrap(), "None");
        assert_eq!(pct("%8.1f", &[Value::Float(1.25)]).unwrap(), "     1.2");
        assert_eq!(pct("%.0f GB", &[Value::Float(2.5)]).unwrap(), "2 GB");
        assert_eq!(pct("%d%%", &[n(3)]).unwrap(), "3%");
        assert_eq!(pct("%d", &[Value::Float(3.9)]).unwrap(), "3");
        assert!(pct("%d", &[s("x")]).unwrap_err().starts_with("TypeError"));
        assert!(pct("%.1f", &[Value::Null])
            .unwrap_err()
            .starts_with("TypeError"));
    }

    #[test]
    fn numbers_as_python() {
        assert_eq!(floordiv(&int(-7), &int(2)).unwrap(), int(-4));
        assert_eq!(rem(&int(-7), &int(2)).unwrap(), int(1));
        assert_eq!(floordiv(&int(7), &int(2)).unwrap(), int(3));
        assert_eq!(rem(&int(7), &int(-2)).unwrap(), int(-1));
        assert_eq!(
            floordiv(&Value::Float(7.5), &int(2)).unwrap(),
            Value::Float(3.0)
        );
        assert_eq!(round0(&Value::Float(2.5)).unwrap(), int(2));
        assert_eq!(round0(&Value::Float(3.5)).unwrap(), int(4));
        assert_eq!(round_n(&Value::Float(0.25), 1).unwrap(), Value::Float(0.2));
        assert!(eq(&int(1), &Value::Float(1.0)));
        assert!(eq(&Value::Bool(true), &int(1)));
        assert!(cmp("<", &s("a"), &int(1)).is_err());
        assert_eq!(to_int(&s(" 12 ")).unwrap(), 12);
    }

    #[test]
    fn slices_count_code_points() {
        assert_eq!(slice(&s("héllo"), None, Some(2)).unwrap(), s("hé"));
        assert_eq!(slice(&s("ab"), None, Some(-1)).unwrap(), s("a"));
        assert_eq!(slice(&s("abc"), Some(6), None).unwrap(), s(""));
        assert!(slice(&Value::Null, None, Some(2)).is_err());
    }
}
