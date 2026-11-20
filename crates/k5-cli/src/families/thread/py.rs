//! What the shell's Python readers for `thread` share: the answer read
//! from stdin, the error line, `int()`, the file sizes and the
//! `[file]` lines.
//!
//! The readers run with Python's UTF-8 mode (a CLI run with no `LANG`):
//! an answer that is not JSON is written back as it came
//! (`sys.stdout.write(raw)` with `surrogateescape`), byte for byte. An
//! answer that is not UTF-8 is taken as not JSON here; Python would read
//! a JSON text with such a byte inside a string, which the daemon (whose
//! JSON comes from Rust strings) never sends.

use super::super::{get, Raise};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::py::{repr, str_of, truthy, type_name};
use crate::out::{self};

/// What a reader printed, and its exit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Shown {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub exit: i32,
}

impl Shown {
    /// `print(line)`.
    pub fn line(&mut self, line: &str) {
        self.stdout.extend_from_slice(line.as_bytes());
        self.stdout.push(b'\n');
    }

    /// `sys.stderr.write(text)`.
    pub fn err(&mut self, text: &str) {
        self.stderr.push_str(text);
    }

    /// The reader stopped at `exit`.
    pub fn exit(mut self, exit: i32) -> Shown {
        self.exit = exit;
        self
    }

    /// What it printed before it raised, then the traceback: exit 1.
    pub fn raised(mut self, r: &Raise) -> Shown {
        self.stderr.push_str(&r.text());
        self.exit = 1;
        self
    }

    /// Writes stdout, then stderr; the exit.
    pub fn print(&self) -> Exit {
        if !self.stdout.is_empty() {
            if let Err(e) = out::write_flushed(&mut std::io::stdout().lock(), &self.stdout) {
                return out::after_stdout_error(&e);
            }
        }
        if !self.stderr.is_empty() {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), self.stderr.as_bytes());
        }
        Exit::from_curl(self.exit)
    }
}

/// `json.loads(raw)`, or `None` where it raises.
pub fn loads(raw: &[u8]) -> Option<Value> {
    json::parse(std::str::from_utf8(raw).ok()?).ok()
}

/// A string value.
pub fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// `v.get(key)`, or the `AttributeError` at `line` for anything but a
/// dict.
pub fn get_at(v: &Value, key: &str, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Object(pairs) => Ok(get(pairs, key).cloned().unwrap_or(Value::Null)),
        other => Err(Raise::no_attribute(line, other, "get")),
    }
}

/// `v.get(key, default)`.
pub fn get_or(v: &Value, key: &str, default: Value, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Object(pairs) => Ok(get(pairs, key).cloned().unwrap_or(default)),
        other => Err(Raise::no_attribute(line, other, "get")),
    }
}

/// `a or b`.
pub fn or(a: Value, b: Value) -> Value {
    if truthy(&a) {
        a
    } else {
        b
    }
}

/// `d.get("ok") is not True` (`d` a dict).
pub fn not_ok(d: &Value, line: u32) -> Result<bool, Raise> {
    Ok(!matches!(get_at(d, "ok", line)?, Value::Bool(true)))
}

/// The readers' error branch on a dict `d` whose `ok` is not true:
/// `err=d.get("error") or d`, then `"%s: %s\n" % (err.get("code") or
/// "error", err.get("hint") or err)` for a dict, else `str(err)+"\n"`.
/// The code too (empty when `err` is no dict).
pub fn error_line(d: &Value, line: u32) -> Result<(String, Value), Raise> {
    let err = or(get_at(d, "error", line)?, d.clone());
    if let Value::Object(_) = err {
        let code = or(get_at(&err, "code", line + 2)?, s("error"));
        let hint = or(get_at(&err, "hint", line + 2)?, err.clone());
        return Ok((format!("{}: {}\n", str_of(&code), str_of(&hint)), code));
    }
    Ok((format!("{}\n", str_of(&err)), Value::Null))
}

/// Python's `int(v)` for a value `json.loads` gave.
pub fn int(v: &Value, line: u32) -> Result<i128, Raise> {
    let bad = || {
        Raise::at(
            line,
            format!(
                "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
                type_name(v)
            ),
        )
    };
    match v {
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Int(digits) => digits.parse().map_err(|_| bad()),
        Value::Float(f) => {
            if f.is_nan() {
                Err(Raise::at(
                    line,
                    "ValueError: cannot convert float NaN to integer",
                ))
            } else if f.is_infinite() {
                Err(Raise::at(
                    line,
                    "OverflowError: cannot convert float infinity to integer",
                ))
            } else {
                Ok(f.trunc() as i128)
            }
        }
        Value::Str(t) => {
            let text = str_of(v);
            int_text(&text).ok_or_else(|| {
                Raise::at(
                    line,
                    format!(
                        "ValueError: invalid literal for int() with base 10: {}",
                        repr(&Value::Str(t.clone()))
                    ),
                )
            })
        }
        _ => Err(bad()),
    }
}

/// `int("…")`: spaces around, a sign, ASCII digits with single `_`
/// between them.
fn int_text(text: &str) -> Option<i128> {
    let t = text.trim_matches(super::super::py_space);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
    {
        return None;
    }
    let n: i128 = digits.replace('_', "").parse().ok()?;
    Some(if neg { -n } else { n })
}

/// The readers' `size(n)`: `int(n or 0)` in B, KB, MB or GB.
pub fn size(n: &Value, line: u32) -> Result<String, Raise> {
    let n = int(&or(n.clone(), Value::Int("0".into())), line)?;
    if n < 1024 {
        return Ok(format!("{n} B"));
    }
    let mut f = n as f64 / 1024.0;
    for unit in ["KB", "MB"] {
        if f < 1024.0 {
            return Ok(format!("{f:.0} {unit}"));
        }
        f /= 1024.0;
    }
    Ok(format!("{f:.0} GB"))
}

/// `":%s" % a["line"] + ("-%s" % a["lineEnd"] if a.get("lineEnd") else
/// "")` when `a.get("line")` is true, else nothing (`a` a dict).
pub fn line_spec(a: &Value, line: u32) -> Result<String, Raise> {
    let l = get_at(a, "line", line)?;
    if !truthy(&l) {
        return Ok(String::new());
    }
    let end = get_at(a, "lineEnd", line + 1)?;
    let end = if truthy(&end) {
        format!("-{}", str_of(&end))
    } else {
        String::new()
    };
    Ok(format!(":{}{end}", str_of(&l)))
}

/// `json.dumps(v)`.
pub fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).unwrap_or_default()
}

/// `raw.strip()`.
pub fn strip(raw: &str) -> &str {
    raw.trim_matches(super::super::py_space)
}

/// `for x in v or []` where `v` must be a list of dicts to the reader.
pub fn items(v: Value, line: u32) -> Result<Vec<Value>, Raise> {
    super::super::py_iter(&or(v, Value::Array(Vec::new())), line)
}
