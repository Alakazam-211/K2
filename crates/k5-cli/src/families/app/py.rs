//! The shell's Python, step by step, for the `k5 app` printers: each
//! step does what Python does with the value, or raises what Python
//! raises there ([`Raise`]). A printer collects its lines in [`Out`] and
//! ends as [`Out::done`] says: what it printed, then the traceback and
//! exit 1 when it raised.

use super::super::{get, py_iter, raised, Raise, Shown};
use crate::exit::Exit;
use crate::out::json::{Text, Value};
use crate::out::py;
use crate::out::pylib;

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
/// `line`: Python's own `JSONDecodeError` words ([`pylib::json::loads`]).
pub fn load(body: &[u8], line: u32) -> Result<Value, Raise> {
    let text = String::from_utf8_lossy(body);
    pylib::json::loads(&text).map_err(|e| Raise::at(line, e.last_line()))
}

/// `v[key]` with a string key ([`pylib::ops::index`]).
pub fn idx(v: &Value, key: &str, line: u32) -> Result<Value, Raise> {
    pylib::ops::index(v, key, line).cloned()
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

/// `sep.join(v)`: every item must be a string ([`pylib::ops::join`]).
pub fn join(sep: &str, v: &Value, line: u32) -> Result<String, Raise> {
    pylib::ops::join(v, sep, line)
}

/// `needle in v` for a string needle ([`pylib::ops::contains`]).
pub fn contains(v: &Value, needle: &str, line: u32) -> Result<bool, Raise> {
    pylib::ops::contains(v, needle, line)
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

/// `'%-<width>s' % v`.
pub fn left(v: &Value, width: usize) -> String {
    super::super::ljust(&s(v), width)
}

/// `'%<width>s' % v`: `str(v)` padded on the left to `width` code points.
pub fn right(v: &Value, width: usize) -> String {
    rjust(&s(v), width)
}

/// `text.rjust(width)`.
pub fn rjust(text: &str, width: usize) -> String {
    let n = text.chars().count();
    if n < width {
        format!("{}{text}", " ".repeat(width - n))
    } else {
        text.to_string()
    }
}

/// `'%<width>d' % v`, or the exception at `line`.
pub fn d(v: &Value, width: usize, line: u32) -> Result<String, Raise> {
    Ok(rjust(&super::super::py_d(v, line)?, width))
}

/// `v / by` for a float `by`: the float Python gets, or the exception at
/// `line`.
pub fn div(v: &Value, by: f64, line: u32) -> Result<f64, Raise> {
    let x = match v {
        Value::Float(f) => *f,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Int(digits) => {
            let f: f64 = digits.parse().unwrap_or(f64::INFINITY);
            if f.is_infinite() {
                return Err(Raise::at(
                    line,
                    "OverflowError: int too large to convert to float",
                ));
            }
            f
        }
        other => {
            return Err(Raise::at(
                line,
                format!(
                    "TypeError: unsupported operand type(s) for /: '{}' and 'float'",
                    py::type_name(other)
                ),
            ))
        }
    };
    Ok(x / by)
}

/// `'%<width>.<prec>f' % x`.
pub fn f(x: f64, width: usize, prec: usize) -> String {
    let text = if x.is_nan() {
        "nan".to_string()
    } else if x.is_infinite() {
        (if x > 0.0 { "inf" } else { "-inf" }).to_string()
    } else {
        format!("{x:.prec$}")
    };
    rjust(&text, width)
}

/// `v <= now` for a float `now`, or the `TypeError` at `line`.
pub fn le(v: &Value, now: f64, line: u32) -> Result<bool, Raise> {
    match v {
        Value::Float(x) => Ok(*x <= now),
        Value::Bool(b) => Ok(f64::from(u8::from(*b)) <= now),
        Value::Int(digits) => Ok(digits.parse::<f64>().unwrap_or(f64::INFINITY) <= now),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '<=' not supported between instances of '{}' and 'float'",
                py::type_name(other)
            ),
        )),
    }
}

/// `time.time()`.
pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// `time.strftime("%Y-%m-%d %H:%M", time.localtime(v))`, or the exception
/// at `line`.
pub fn local_minutes(v: &Value, line: u32) -> Result<String, Raise> {
    local_time(v, "%Y-%m-%d %H:%M", line)
}

/// `time.strftime(fmt, time.localtime(v))`, or the exception at `line`
/// (`None` is now, as Python takes it).
pub fn local_time(v: &Value, fmt: &str, line: u32) -> Result<String, Raise> {
    const RANGE: &str = "OverflowError: timestamp out of range for platform time_t";
    let secs: i64 = match v {
        Value::Null => now().floor() as i64,
        Value::Bool(b) => i64::from(*b),
        Value::Int(digits) => digits.parse().map_err(|_| Raise::at(line, RANGE))?,
        Value::Float(x) if x.is_nan() => {
            return Err(Raise::at(
                line,
                "ValueError: Invalid value NaN (not a number)",
            ))
        }
        Value::Float(x) => {
            let w = x.floor();
            // i64::MAX as f64 rounds up to 2^63, which is out of range.
            if !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&w) {
                return Err(Raise::at(line, RANGE));
            }
            w as i64
        }
        other => {
            return Err(Raise::at(
                line,
                format!(
                    "TypeError: '{}' object cannot be interpreted as an integer",
                    py::type_name(other)
                ),
            ))
        }
    };
    let mut fmt = fmt.as_bytes().to_vec();
    fmt.push(0);
    local_text(secs, &fmt).map_err(|e| Raise::at(line, e))
}

/// `secs` as local time in `fmt` (NUL-ended), by `localtime_r` and
/// `strftime`; `OSError` where `localtime_r` fails. (The activity
/// family has the same steps for its own format; to share once the
/// families' helpers are gathered.)
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
fn local_text(secs: i64, fmt: &[u8]) -> Result<String, String> {
    // `time_t` is 64 bits on every platform the binary ships for.
    let t = secs as libc::time_t;
    // SAFETY: tzset reads TZ; localtime_r and strftime write only into the
    // buffers given, each sized as passed; `fmt` ends in a NUL.
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
        let mut buf = [0u8; 64];
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr().cast(), &tm);
        Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    }
}

#[cfg(not(unix))]
fn local_text(_secs: i64, _fmt: &[u8]) -> Result<String, String> {
    Err("OSError: no local time here".into())
}

/// `datetime.datetime.fromtimestamp(v).strftime(fmt)`: the time rounded
/// half to even to the microsecond first, as `fromtimestamp` does; or the
/// exception at `line`.
pub fn date_time(v: &Value, fmt: &str, line: u32) -> Result<String, Raise> {
    match v {
        Value::Float(x) if x.is_finite() => {
            let whole = x.trunc();
            let us = ((x - whole) * 1e6).round_ties_even();
            let secs = if us >= 1e6 {
                whole + 1.0
            } else if us < 0.0 {
                whole - 1.0
            } else {
                whole
            };
            local_time(&Value::Float(secs), fmt, line)
        }
        other => local_time(other, fmt, line),
    }
}

/// `sum(v.values())` for a dict of numbers: an int, or a float once one
/// is a float; the `TypeError` at `line` for anything else.
pub fn sum_values(v: &Value, line: u32) -> Result<Value, Raise> {
    let Value::Object(pairs) = v else {
        return Err(Raise::no_attribute(line, v, "values"));
    };
    let mut int: i128 = 0;
    let mut float: Option<f64> = None;
    for (_, x) in pairs {
        let add = match x {
            Value::Bool(b) => Some(i128::from(*b)),
            Value::Int(digits) => digits.parse::<i128>().ok(),
            _ => None,
        };
        let y = match (x, add) {
            (_, Some(n)) => {
                if float.is_none() {
                    int += n;
                    continue;
                }
                n as f64
            }
            (Value::Float(y), _) => *y,
            (other, _) => {
                let left = if float.is_some() { "float" } else { "int" };
                return Err(Raise::at(
                    line,
                    format!(
                        "TypeError: unsupported operand type(s) for +: '{left}' and '{}'",
                        py::type_name(other)
                    ),
                ));
            }
        };
        float = Some(float.unwrap_or(int as f64) + y);
    }
    Ok(match float {
        Some(f) => Value::Float(f),
        None => Value::Int(int.to_string()),
    })
}

/// `{…}.get(key, default)` on a dict of words: the word for a string key
/// in `words`, else `default`; a list or dict key is unhashable.
pub fn word_for(
    words: &[(&str, &str)],
    key: &Value,
    default: Value,
    line: u32,
) -> Result<Value, Raise> {
    match key {
        Value::Array(_) | Value::Object(_) => Err(Raise::at(
            line,
            format!("TypeError: unhashable type: '{}'", py::type_name(key)),
        )),
        Value::Str(_) => Ok(words
            .iter()
            .find(|(k, _)| *key == text(k))
            .map_or(default, |(_, w)| text(w))),
        _ => Ok(default),
    }
}
