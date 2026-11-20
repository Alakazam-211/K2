//! The shell's Python, step by step, for the `k5 app` printers: each
//! step does what Python does with the value, or raises what Python
//! raises there ([`Raise`]). A printer collects its lines in [`Out`] and
//! ends as [`Out::done`] says: what it printed, then the traceback and
//! exit 1 when it raised.

use super::super::{code_point, get, py_iter, raised, Raise, Shown};
use crate::exit::Exit;
use crate::out::json::{self, Text, Value};
use crate::out::py;

/// What a printer printed so far.
#[derive(Debug, Default)]
pub struct Out {
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    /// `print(text)`.
    pub fn line(&mut self, text: &str) {
        self.stdout.push_str(text);
        self.stdout.push('\n');
    }

    /// `print(text, file=sys.stderr)`.
    pub fn err(&mut self, text: &str) {
        self.stderr.push_str(text);
        self.stderr.push('\n');
    }

    /// The printer's end: `r` is its exit, or where it raised.
    pub fn done(self, r: Result<Exit, Raise>) -> Shown {
        match r {
            Ok(exit) => Shown {
                stdout: self.stdout,
                stderr: self.stderr,
                exit,
            },
            Err(e) => raised(self.stdout, self.stderr, &e),
        }
    }
}

/// `json.load(sys.stdin)` on what `printf '%s'` handed the reader, at
/// `line`.
pub fn load(body: &[u8], line: u32) -> Result<Value, Raise> {
    let text = String::from_utf8_lossy(body);
    json::parse(&text).map_err(|e| {
        let why = if text.trim_matches(super::super::py_space).is_empty() {
            "Expecting value: line 1 column 1 (char 0)".to_string()
        } else {
            e.to_string()
        };
        Raise::at(line, format!("json.decoder.JSONDecodeError: {why}"))
    })
}

/// `repr(key)` for a string key, as a `KeyError` shows it.
fn key_repr(key: &str) -> String {
    py::repr(&Value::Str(Text::from(key)))
}

/// `v[key]` with a string key.
pub fn idx(v: &Value, key: &str, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Object(pairs) => get(pairs, key)
            .cloned()
            .ok_or_else(|| Raise::at(line, format!("KeyError: {}", key_repr(key)))),
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

/// `v.get(key)`: `None` for a missing key; `AttributeError` on anything
/// but a dict.
pub fn dget(v: &Value, key: &str, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Object(pairs) => Ok(get(pairs, key).cloned().unwrap_or(Value::Null)),
        other => Err(Raise::no_attribute(line, other, "get")),
    }
}

/// `v.get(key, default)`.
pub fn dget_or(v: &Value, key: &str, default: Value, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Object(pairs) => Ok(get(pairs, key).cloned().unwrap_or(default)),
        other => Err(Raise::no_attribute(line, other, "get")),
    }
}

/// `a or b`.
pub fn or(a: Value, b: Value) -> Value {
    if py::truthy(&a) {
        a
    } else {
        b
    }
}

/// `bool(v)`.
pub fn truthy(v: &Value) -> bool {
    py::truthy(v)
}

/// `str(v)`.
pub fn s(v: &Value) -> String {
    py::str_of(v)
}

/// A string value.
pub fn text(t: &str) -> Value {
    Value::Str(Text::from(t))
}

/// An empty list.
pub fn empty_list() -> Value {
    Value::Array(Vec::new())
}

/// `for x in v`, or the `TypeError` at `line`.
pub fn iter(v: &Value, line: u32) -> Result<Vec<Value>, Raise> {
    py_iter(v, line)
}

/// `sep.join(v)`: every item must be a string.
pub fn join(sep: &str, v: &Value, line: u32) -> Result<String, Raise> {
    let items = match v {
        Value::Array(items) => items.clone(),
        Value::Str(t) => t
            .code_points()
            .into_iter()
            .map(|cp| Value::Str(code_point(cp)))
            .collect(),
        Value::Object(pairs) => pairs.iter().map(|(k, _)| Value::Str(k.clone())).collect(),
        _ => {
            return Err(Raise::at(line, "TypeError: can only join an iterable"));
        }
    };
    let mut parts = Vec::with_capacity(items.len());
    for (n, item) in items.iter().enumerate() {
        match item {
            Value::Str(_) => parts.push(py::str_of(item)),
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
    Ok(parts.join(sep))
}

/// `needle in v` for a string needle.
pub fn contains(v: &Value, needle: &str, line: u32) -> Result<bool, Raise> {
    match v {
        Value::Object(pairs) => Ok(get(pairs, needle).is_some()),
        Value::Array(items) => Ok(items.contains(&text(needle))),
        Value::Str(_) => Ok(py::str_of(v).contains(needle)),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: argument of type '{}' is not iterable",
                py::type_name(other)
            ),
        )),
    }
}

/// `v[:n]`: a string's or a list's first `n`.
pub fn head(v: &Value, n: usize, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Str(t) => {
            // A lone surrogate is kept as Python writes it out.
            let s: String = match t.as_str() {
                Some(s) => s.chars().take(n).collect(),
                None => py::str_of(v).chars().take(n).collect(),
            };
            Ok(Value::Str(Text::from(s)))
        }
        Value::Array(items) => Ok(Value::Array(items.iter().take(n).cloned().collect())),
        // Python 3.12: a slice is hashable, so a dict looks it up.
        Value::Object(_) => Err(Raise::at(line, format!("KeyError: slice(None, {n}, None)"))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// `prefix + v`: `v` must be a string.
pub fn concat(prefix: &str, v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(format!("{prefix}{}", py::str_of(v))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

/// An int's value, when it fits.
fn int_of(v: &Value) -> Option<i128> {
    match v {
        Value::Int(digits) => digits.parse().ok(),
        Value::Bool(b) => Some(i128::from(*b)),
        _ => None,
    }
}

/// `'%d' % (v >> n)`.
pub fn shr(v: &Value, n: u32, line: u32) -> Result<String, Raise> {
    match (v, int_of(v)) {
        (_, Some(i)) => Ok((i >> n).to_string()),
        // Past i128: no daemon sends one.
        (Value::Int(digits), None) => Ok(digits.clone()),
        (other, None) => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for >>: '{}' and 'int'",
                py::type_name(other)
            ),
        )),
    }
}

/// `v < 1 << 60`.
pub fn lt_2_60(v: &Value, line: u32) -> Result<bool, Raise> {
    const LIMIT: i128 = 1 << 60;
    match (v, int_of(v)) {
        (_, Some(i)) => Ok(i < LIMIT),
        (Value::Int(digits), None) => Ok(digits.starts_with('-')),
        (Value::Float(f), _) => Ok(*f < LIMIT as f64),
        (other, _) => Err(Raise::at(
            line,
            format!(
                "TypeError: '<' not supported between instances of '{}' and 'int'",
                py::type_name(other)
            ),
        )),
    }
}
