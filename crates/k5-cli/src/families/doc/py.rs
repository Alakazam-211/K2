//! What the doc family's Python readers do, on a value `json.loads`
//! gave: the lines they print (stdout as bytes, stderr as text) and, where
//! Python raises, the traceback with Python's exception. Small wrappers
//! over [`crate::out::pylib`] and [`crate::families`]'s helpers.

use crate::exit::Exit;
use crate::families::{py_d, py_get, py_iter, py_left, Raise};
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::{json as pyjson, ops, time as pytime};
use crate::out::{self, py};

pub type R<T> = Result<T, Raise>;

/// What a reader printed so far.
#[derive(Debug, Default)]
pub struct Pr {
    pub out: Vec<u8>,
    pub err: String,
}

impl Pr {
    /// `print(s)`.
    pub fn line(&mut self, s: &str) {
        self.out.extend_from_slice(s.as_bytes());
        self.out.push(b'\n');
    }
    /// `sys.stdout.write(s)`.
    pub fn put(&mut self, s: &str) {
        self.out.extend_from_slice(s.as_bytes());
    }
    /// `print(s, file=sys.stderr)`.
    pub fn eline(&mut self, s: &str) {
        self.err.push_str(s);
        self.err.push('\n');
    }
    /// `sys.stderr.write(s)`.
    pub fn eput(&mut self, s: &str) {
        self.err.push_str(s);
    }

    /// Writes what was printed (and the traceback when the reader
    /// raised); the reader's exit.
    pub fn finish(self, r: R<Exit>) -> Exit {
        let (exit, tb) = match r {
            Ok(e) => (e, String::new()),
            Err(raise) => (Exit::Error, raise.text()),
        };
        if !self.out.is_empty() {
            if let Err(e) = out::write_flushed(&mut std::io::stdout().lock(), &self.out) {
                return out::after_stdout_error(&e);
            }
        }
        let err = self.err + &tb;
        if !err.is_empty() {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), err.as_bytes());
        }
        exit
    }
}

/// `json.load(...)` of an answer's bytes (read as UTF-8, a bad byte as
/// U+FFFD), or the traceback's exception at `line`.
pub fn load(body: &[u8], line: u32) -> R<Value> {
    let text = String::from_utf8_lossy(body);
    pyjson::loads(&text).map_err(|f| Raise::at(line, f.last_line()))
}

/// `v[k]`.
pub fn ix<'a>(v: &'a Value, k: &str, line: u32) -> R<&'a Value> {
    ops::index(v, k, line)
}

/// `v.get(k)`: `None` (missing) as `Option`.
pub fn gt<'a>(v: &'a Value, k: &str, line: u32) -> R<Option<&'a Value>> {
    py_get(v, k, line)
}

/// `v.get(k) or default` as a value: `None` when missing or false.
pub fn gt_or<'a>(v: &'a Value, k: &str, line: u32) -> R<Option<&'a Value>> {
    Ok(gt(v, k, line)?.filter(|x| py::truthy(x)))
}

/// `bool(v.get(k))`.
pub fn tr(v: &Value, k: &str, line: u32) -> R<bool> {
    Ok(gt(v, k, line)?.is_some_and(py::truthy))
}

/// `"%s" % v.get(k)`.
pub fn sg(v: &Value, k: &str, line: u32) -> R<String> {
    Ok(ops::s_of(gt(v, k, line)?))
}

/// `"%s" % x`.
pub fn st(v: &Value) -> String {
    py::str_of(v)
}

/// `"%s" % x` for an optional value (`None` when missing).
pub fn so(v: Option<&Value>) -> String {
    ops::s_of(v)
}

/// `"%-<w>s" % x`.
pub fn left(v: Option<&Value>, w: usize) -> String {
    py_left(v.unwrap_or(&Value::Null), w)
}

/// `"%d" % x`.
pub fn d(v: &Value, line: u32) -> R<String> {
    py_d(v, line)
}

/// `for x in v`.
pub fn iter(v: &Value, line: u32) -> R<Vec<Value>> {
    py_iter(v, line)
}

/// `v.get(k) or []`, walked.
pub fn rows(v: &Value, k: &str, line: u32) -> R<Vec<Value>> {
    match gt_or(v, k, line)? {
        Some(x) => iter(x, line),
        None => Ok(Vec::new()),
    }
}

/// `v[i]` for an int index.
pub fn at(v: &Value, i: usize, line: u32) -> R<Value> {
    match v {
        Value::Array(items) => items
            .get(i)
            .cloned()
            .ok_or_else(|| Raise::at(line, "IndexError: list index out of range")),
        Value::Str(t) => t
            .code_points()
            .get(i)
            .map(|cp| Value::Str(crate::families::code_point(*cp)))
            .ok_or_else(|| Raise::at(line, "IndexError: string index out of range")),
        // JSON keys are strings: an int key is never there.
        Value::Object(_) => Err(Raise::at(line, format!("KeyError: {i}"))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// A string value for `s.rstrip(...)`, `s.endswith(...)` and the
/// like: the text, or Python's `AttributeError` for `name`.
pub fn text(v: &Value, name: &str, line: u32) -> R<String> {
    match v {
        Value::Str(_) => Ok(py::str_of(v)),
        other => Err(Raise::no_attribute(line, other, name)),
    }
}

/// `sys.stderr.write(x)`: a string, else Python's `TypeError`.
pub fn write_arg(v: &Value, line: u32) -> R<String> {
    match v {
        Value::Str(_) => Ok(py::str_of(v)),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: write() argument must be str, not {}",
                py::type_name(other)
            ),
        )),
    }
}

/// A number for arithmetic: `(int, float)`, or the `TypeError` an
/// operator with `op` raises.
pub fn num(v: &Value, op: &str, other: &str, line: u32) -> R<f64> {
    match v {
        Value::Int(s) => Ok(s.parse::<f64>().unwrap_or(f64::INFINITY)),
        Value::Float(f) => Ok(*f),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        x => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for {op}: '{}' and '{other}'",
                py::type_name(x)
            ),
        )),
    }
}

/// `x > n` for an int `n`.
pub fn gt_int(v: &Value, n: i64, line: u32) -> R<bool> {
    match v {
        Value::Int(s) => Ok(s
            .parse::<i128>()
            .map_or(!s.starts_with('-'), |x| x > i128::from(n))),
        Value::Float(f) => Ok(*f > n as f64),
        Value::Bool(b) => Ok(i64::from(*b) > n),
        x => Err(Raise::at(
            line,
            format!(
                "TypeError: '>' not supported between instances of '{}' and 'int'",
                py::type_name(x)
            ),
        )),
    }
}

/// `int(x)` of a float.
pub fn int_of_float(f: f64, line: u32) -> R<Value> {
    if f.is_nan() {
        return Err(Raise::at(
            line,
            "ValueError: cannot convert float NaN to integer",
        ));
    }
    if f.is_infinite() {
        return Err(Raise::at(
            line,
            "OverflowError: cannot convert float infinity to integer",
        ));
    }
    Ok(Value::Int(format!("{:.0}", f.trunc()).replace("-0", "0")))
}

/// `int(v)`.
pub fn int(v: &Value, line: u32) -> R<Value> {
    match v {
        Value::Int(_) => Ok(v.clone()),
        Value::Bool(b) => Ok(Value::Int(if *b { "1" } else { "0" }.into())),
        Value::Float(f) => int_of_float(*f, line),
        Value::Str(_) => {
            let s = py::str_of(v);
            match int_text(&s) {
                Some(n) => Ok(Value::Int(n)),
                None => Err(Raise::at(
                    line,
                    format!(
                        "ValueError: invalid literal for int() with base 10: {}",
                        py::repr(v)
                    ),
                )),
            }
        }
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
                py::type_name(other)
            ),
        )),
    }
}

/// Python's `int(s)` of a text: spaces around, a sign, `_` between digits.
pub fn int_text(s: &str) -> Option<String> {
    let t = s.trim_matches(crate::families::py_space);
    let (neg, digits) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    if !clean.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let clean = clean.trim_start_matches('0');
    let clean = if clean.is_empty() { "0" } else { clean };
    Some(if neg && clean != "0" {
        format!("-{clean}")
    } else {
        clean.to_string()
    })
}

/// `sys.exit(n)` for an int value: the process's code (`n & 0xFF`).
pub fn exit_of(v: &Value) -> Exit {
    let n = match v {
        Value::Int(s) => {
            // The low byte of a decimal of any size.
            let neg = s.starts_with('-');
            let m = s
                .trim_start_matches('-')
                .bytes()
                .fold(0u32, |acc, b| (acc * 10 + u32::from(b - b'0')) % 256);
            if neg {
                (256 - m) % 256
            } else {
                m
            }
        }
        Value::Bool(b) => u32::from(*b),
        _ => 1,
    };
    Exit::from_curl(n as i32)
}

/// `sys.exit(int(x))`.
pub fn exit_int(v: &Value, line: u32) -> R<Exit> {
    Ok(exit_of(&int(v, line)?))
}

/// `json.dumps(v)`.
pub fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).expect("a value from JSON or words always prints")
}

pub fn s(t: &str) -> Value {
    Value::Str(Text::from(t))
}

pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `time.strftime(fmt, time.localtime(ms / 1000))` for a value of
/// milliseconds.
pub fn local_ms(ms: &Value, fmt: &str, line: u32) -> R<String> {
    let secs = num(ms, "/", "int", line)? / 1000.0;
    let t = pytime::seconds(&Value::Float(secs), 0).map_err(|e| Raise::at(line, e))?;
    pytime::strftime_local(t, fmt).map_err(|e| Raise::at(line, e))
}

/// The seconds now, as `time.time()`.
pub fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// `base64.b64decode(x)` of a value that is not a string.
pub fn b64_type(v: &Value, line: u32) -> Raise {
    Raise::at(
        line,
        format!(
            "TypeError: argument should be a bytes-like object or ASCII string, not '{}'",
            py::type_name(v)
        ),
    )
}
