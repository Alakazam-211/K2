//! The small Python forms the hostmail verbs' branches of `_mail_py`
//! repeat: `d.get(k)` at a line, `x or "?"`, `for x in v or []` and
//! `print(dumps(d) if as_json else (d.get("hint") or "…"))`.

use crate::families::mail::core::{At, Py, MODULE, R};
use crate::families::mail::py::{self as p, attr_get};
use crate::out::json::Value;
use crate::out::py;

/// `v.get(key)`, raised at `l`.
pub fn g(v: &Value, key: &str, l: u32) -> R<Option<Value>> {
    attr_get(v, key).at(l, MODULE)
}

/// A string value.
pub fn sv(t: &str) -> Value {
    p::sv(t)
}

/// `str(v or default)`.
pub fn or_s(v: Option<Value>, default: &str) -> String {
    match v.filter(py::truthy) {
        Some(v) => py::str_of(&v),
        None => default.to_string(),
    }
}

/// `str(v)` of `v.get(key)`: `None` when missing.
pub fn s_of(v: Option<Value>) -> String {
    py::str_of(&v.unwrap_or(Value::Null))
}

/// `d.get(key) or env(env_key) or "?"` at `l`.
pub fn or_env(py_: &Py, d: &Value, key: &str, env_key: &str, l: u32) -> R<String> {
    Ok(match g(d, key, l)?.filter(py::truthy) {
        Some(v) => py::str_of(&v),
        None if !py_.env(env_key).is_empty() => py_.env(env_key).to_string(),
        None => "?".into(),
    })
}

/// `d.get("address") or env("M_ADDRESS") or "?"` at `l`.
pub fn addr(py_: &Py, d: &Value, l: u32) -> R<String> {
    or_env(py_, d, "address", "M_ADDRESS", l)
}

/// `for x in (v or [])`, raised at `l`.
pub fn items(v: Option<Value>, l: u32) -> R<Vec<Value>> {
    match v.filter(py::truthy) {
        Some(v) => p::iter(&v).at(l, MODULE),
        None => Ok(Vec::new()),
    }
}

/// `v or {}`.
pub fn or_dict(v: Option<Value>) -> Value {
    p::or(v, Value::Object(Vec::new()))
}

/// `print(dumps(d) if as_json else (d.get("hint") or default))`.
pub fn hint_or(py_: &mut Py, d: &Value, default: &str, l: u32) -> R<()> {
    if py_.json {
        py_.print_json(d);
        return Ok(());
    }
    let h = or_s(g(d, "hint", l)?, default);
    py_.print(&h);
    Ok(())
}

/// `len(v)`, raised at `l`.
pub fn len(v: &Value, l: u32) -> R<usize> {
    p::len(v).at(l, MODULE)
}

/// `int(v)` as its digits, raised at `l`.
pub fn int(v: &Value, l: u32) -> R<String> {
    p::py_int(v).at(l, MODULE)
}

/// `sep.join(v)`, raised at `l`.
pub fn join(sep: &str, v: &Value, l: u32) -> R<String> {
    p::join(sep, v).at(l, MODULE)
}

/// `"%d" % v`, raised at `l`.
pub fn pd(v: &Value, l: u32) -> R<String> {
    py::percent_d(v).at(l, MODULE)
}

/// `env(k)` as a string value.
pub fn env_v(py_: &Py, key: &str) -> Value {
    sv(py_.env(key))
}

/// `"%-Ns" % s`: `s` padded on the right to `n` characters.
pub fn ljust(s: &str, n: usize) -> String {
    p::ljust(s, n)
}

/// `"%Ns" % s`: `s` padded on the left to `n` characters.
pub fn rjust(s: &str, n: usize) -> String {
    let w = s.chars().count();
    if w >= n {
        s.to_string()
    } else {
        format!("{}{s}", " ".repeat(n - w))
    }
}
