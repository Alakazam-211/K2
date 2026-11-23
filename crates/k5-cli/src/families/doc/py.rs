//! What the doc family's Python readers do, on a value `json.loads`
//! gave: the lines they print (stdout as bytes, stderr as text) and, where
//! Python raises, the traceback with Python's exception. Small wrappers
//! over [`crate::out::pylib`] and [`crate::families`]'s helpers.
//!
//! An answer that is not UTF-8 reaches the shell's readers on stdin, which
//! Python decodes as its locale says ([`crate::cards::decode`]). In a
//! UTF-8 locale a bad byte raises `UnicodeDecodeError`. In the C or POSIX
//! locale (or a name Python coerces to it) each bad byte becomes a lone
//! surrogate (`surrogateescape`, U+DC80 to U+DCFF), which goes back out on
//! stdout as the same byte, on stderr as `\udcXX` (`backslashreplace`), and
//! in `json.dumps` and `repr` as `\udcXX`. A Rust string can't hold a lone
//! surrogate, so [`stdin_text`] keeps each such byte as a code point of
//! its own ([`escaped`]: U+10FF00 plus the byte), as long as a surrogate
//! in every way the readers use (one character), and [`Pr`] writes each
//! out as Python writes the surrogate. While no answer had a bad byte,
//! nothing is changed (a legitimate U+10FF80 to U+10FFFF in an answer that
//! also had a bad byte would be taken for one: none of the daemon's
//! answers holds such a code point).

use crate::cards::Decode;
use crate::exit::Exit;
use crate::families::{py_d, py_get, py_iter, py_left, Raise};
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::{json as pyjson, ops, time as pytime};
use crate::out::{self, py};
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};

pub type R<T> = Result<T, Raise>;

/// An answer with a byte that is not UTF-8 was read the C locale's way.
static ESCAPED: AtomicBool = AtomicBool::new(false);

/// The code point that stands for the bad byte `b` (0x80 to 0xFF), as
/// Python's U+DC00 plus `b` does.
pub fn escaped(b: u8) -> char {
    char::from_u32(0x10_FF00 + u32::from(b)).expect("U+10FF80..U+10FFFF are code points")
}

/// The byte [`escaped`] stands for.
fn unescaped(c: char) -> Option<u8> {
    let n = u32::from(c);
    if (0x10_FF80..=0x10_FFFF).contains(&n) {
        u8::try_from(n - 0x10_FF00).ok()
    } else {
        None
    }
}

/// The settings that say how `python3` decodes its stdin: the locale's,
/// and the three of Python's own that change it.
const DECODE_VARS: [&str; 6] = [
    "LC_ALL",
    "LC_CTYPE",
    "LANG",
    "PYTHONIOENCODING",
    "PYTHONUTF8",
    "PYTHONCOERCECLOCALE",
];

/// How the shell's `python3` decodes its stdin here
/// ([`crate::cards::decode`] of [`DECODE_VARS`]): `None` when the binary
/// can't tell (one of Python's three set, or a locale that is neither
/// UTF-8 nor C or POSIX), and the family goes to the shell then.
pub fn decode_here() -> Option<Decode> {
    let held: Vec<(&str, String)> = DECODE_VARS
        .iter()
        .filter_map(|n| std::env::var_os(n).map(|v| (*n, v.to_string_lossy().into_owned())))
        .collect();
    let pairs: Vec<(&str, &str)> = held.iter().map(|(n, v)| (*n, v.as_str())).collect();
    crate::cards::decode(&crate::offline::Env::of(&pairs))
}

/// `sys.stdin.read()` of an answer: the text, a bad byte as [`escaped`]
/// in the C locale, or the `UnicodeDecodeError` Python raises at `line`
/// in a UTF-8 one.
pub fn stdin_text(body: &[u8], line: u32) -> R<Cow<'_, str>> {
    stdin_text_as(body, line, decode_here())
}

/// [`stdin_text`] for Python's `decode`.
pub fn stdin_text_as(body: &[u8], line: u32, decode: Option<Decode>) -> R<Cow<'_, str>> {
    let e = match std::str::from_utf8(body) {
        Ok(t) => return Ok(Cow::Borrowed(t)),
        Err(e) => e,
    };
    if decode != Some(Decode::Lossy) {
        return Err(super::utf8_raise(line, body, e));
    }
    ESCAPED.store(true, Ordering::Relaxed);
    let mut t = String::with_capacity(body.len() + 8);
    for chunk in body.utf8_chunks() {
        t.push_str(chunk.valid());
        t.extend(chunk.invalid().iter().map(|b| escaped(*b)));
    }
    Ok(Cow::Owned(t))
}

/// What Python writes for a text holding [`escaped`] code points: on
/// stdout each as its byte; on stderr each as `\udcXX`; and the
/// `\udbff\udfXX` of `json.dumps` or the `\U0010ffXX` of `repr` as
/// `\udcXX` on either.
fn python_writes(s: &str, stdout: bool) -> Cow<'_, [u8]> {
    if !ESCAPED.load(Ordering::Relaxed) {
        return Cow::Borrowed(s.as_bytes());
    }
    let s = surrogate_escapes(s);
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        match unescaped(c) {
            Some(b) if stdout => out.push(b),
            Some(b) => out.extend_from_slice(format!("\\udc{b:02x}").as_bytes()),
            None => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    Cow::Owned(out)
}

/// `\udbff\udfXX` (U+10FFXX in `json.dumps`) and `\U0010ffXX` (in
/// `repr`) for XX of 80 to ff, as `\udcXX`.
fn surrogate_escapes(s: &str) -> Cow<'_, str> {
    const DUMPS: &str = "\\udbff\\udf";
    const REPR: &str = "\\U0010ff";
    if !s.contains(DUMPS) && !s.contains(REPR) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        let lead = [DUMPS, REPR].into_iter().find(|p| rest.starts_with(p));
        let byte = lead.and_then(|p| {
            let xx = rest.get(p.len()..p.len() + 2)?;
            let lower = xx
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            let v = u8::from_str_radix(xx, 16).ok()?;
            (lower && v >= 0x80).then_some((p.len() + 2, v))
        });
        match byte {
            Some((n, v)) => {
                out.push_str(&format!("\\udc{v:02x}"));
                rest = &rest[n..];
            }
            None => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    Cow::Owned(out)
}

/// What a reader printed so far.
#[derive(Debug, Default)]
pub struct Pr {
    pub out: Vec<u8>,
    pub err: String,
    /// A reader of a daemon answer that starts with `_DOC_PY_PLAIN`
    /// (`_doc_print_json_or`'s, the watch line): a raise prints
    /// [`plain_lines`], not a traceback.
    pub plain: bool,
}

/// `_DOC_PY_PLAIN`'s lines where a reader raises: `_doc_die`'s two, code
/// `bad_response`. Never a traceback.
pub fn plain_lines() -> String {
    let hint = format!(
        "{} doc can't read the daemon's answer (a daemon on another {} version?)",
        k2_client::brand::CLI_NAME,
        k2_client::brand::PRODUCT_NAME
    );
    let obj = Value::Object(vec![(
        Text::from("error"),
        Value::Object(vec![
            (Text::from("code"), Value::Str(Text::from("bad_response"))),
            (Text::from("hint"), Value::Str(Text::from(hint.as_str()))),
        ]),
    )]);
    let line = json::to_string(&obj, &Options::default()).unwrap_or_default();
    format!("Error: {hint}\n{line}\n")
}

impl Pr {
    /// `print(s)`.
    pub fn line(&mut self, s: &str) {
        self.out.extend_from_slice(&python_writes(s, true));
        self.out.push(b'\n');
    }
    /// `sys.stdout.write(s)`.
    pub fn put(&mut self, s: &str) {
        self.out.extend_from_slice(&python_writes(s, true));
    }
    /// `print(s, file=sys.stderr)`.
    pub fn eline(&mut self, s: &str) {
        self.eput(s);
        self.err.push('\n');
    }
    /// `sys.stderr.write(s)`.
    pub fn eput(&mut self, s: &str) {
        match python_writes(s, false) {
            Cow::Borrowed(_) => self.err.push_str(s),
            Cow::Owned(b) => self
                .err
                .push_str(&String::from_utf8(b).expect("stderr's text is UTF-8")),
        }
    }

    /// Writes what was printed (and, when the reader raised, the plain
    /// lines or the traceback); the reader's exit.
    pub fn finish(self, r: R<Exit>) -> Exit {
        let (exit, tb) = match r {
            Ok(e) => (e, String::new()),
            Err(_) if self.plain => (Exit::Error, plain_lines()),
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

/// `json.load(sys.stdin)` of an answer's bytes ([`stdin_text`]), or the
/// traceback's exception at `line`.
pub fn load(body: &[u8], line: u32) -> R<Value> {
    let text = stdin_text(body, line)?;
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
