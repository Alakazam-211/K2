//! `k5 feedback list` and `k5 feedback show <id>` (`tickets` is the same
//! command), as the shell CLI's `cmd_feedback_list` and
//! `cmd_feedback_show` answer them (`cli/src/feedback.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    the 0.40.4 rename line, and the two gates ([`crate::prelude`]).
//!    `feedback` is no tool in the shell's policy, so it is an open one:
//!    no passport check comes after them.
//! 2. The words ([`parse`]). A word the verb doesn't take, a missing
//!    value or id, and a status that is not one of [`STATUSES`] are
//!    `_feedback_err "usage" …`: one line of JSON on stderr
//!    (`{"error": {"code": "usage", "hint": …}}`, as `json.dumps` writes
//!    it), exit 2. Nothing is sent.
//! 3. `list`: `_cli_use_passport_project` ([`super::use_passport_project`]),
//!    then `resp=$(cli_request /cli/feedback/list [status=…|all=1]) ||
//!    resp=""`, over TCP only. `$(…)` drops the answer's NULs and trailing
//!    newlines. A request with no answer is the transport's line and
//!    exit 1 (the main shell's USR1 trap); a call that failed any other
//!    way, or an empty answer, is the `daemon_unreachable` line, exit 1.
//!    Then the shell's Python on it ([`list_reader`]).
//! 4. `show`: `resp=$(cli_request /cli/feedback/show id=… brief=1) ||
//!    resp=""`, over TCP only, as `list`'s (no `_cli_use_passport_project`
//!    first). The same lines for no answer and an empty one. Then the
//!    shell's Python on it ([`show_reader`]): the brief's HTML alone with
//!    `--html`, the JSON with `--json`, else the ticket, its brief's text
//!    and its thread.
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]): both answers
//!    are noted.
//!
//! Both readers get the answer on stdin and decode it as UTF-8 with
//! `replace`.
//!
//! The daemon's error is one line of JSON on stderr, exit 4 for
//! `not_found` and `ambiguous_id`, 2 for `usage`, else 1.
//!
//! The route table sends only `list` and `show` here (two verb rows,
//! [`crate::route`]); `feedback` alone, its help and every other verb
//! (`ask`, `comment`, `resolve` and the rest) stay the shell's. So does a
//! command line with `-h` or `--help` ([`crate::front`]), a word that is
//! not UTF-8, a port that is set but is no port curl takes, and a
//! `python3` the binary can't tell ([`crate::families`], Without Python).
//! The binary's own transport never uses a proxy (loopback and plain http
//! never do, by the shell's rule too).
//!
//! Where the shell's Python raises (an answer that is not an object, a
//! field of a type it doesn't expect), this prints a Python 3.12
//! traceback. Bytes that are not UTF-8 and half of a surrogate pair are as
//! [`super::inbox`] says.

use super::{curl_port, get, py_get, py_iter, py_space, use_passport_project, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{Call, Channel, Client, Error};
use std::ffi::OsString;
use std::path::Path;

/// The routes.
pub const LIST: &str = "/cli/feedback/list";
pub const SHOW: &str = "/cli/feedback/show";
/// The statuses `--status` takes.
pub const STATUSES: [&str; 6] = [
    "waiting",
    "needs_discussion",
    "answered",
    "planned",
    "resolved",
    "dismissed",
];
/// The hint of the error for a call that got no answer.
pub const UNREACHABLE: &str = concat!(
    "no response from the ",
    k2_client::product_name!(),
    " daemon (is it running?)"
);

/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    /// `list`'s Python (`python3 -c "$_feedback_list_py"`, so `<string>`;
    /// the text starts with a newline, so its first line is line 2).
    pub const L_ERR: u32 = 10;
    pub const L_CODE: u32 = 15;
    pub const L_JSON: u32 = 18;
    pub const L_FOR: u32 = 21;
    pub const L_ROW: u32 = 23;
    /// `show`'s Python (`python3 -c "$_feedback_show_py"`, likewise).
    pub const S_DIE_CODE: u32 = 12;
    pub const S_UNPARSEABLE: u32 = 19;
    pub const S_UNEXPECTED: u32 = 21;
    pub const S_ERR: u32 = 24;
    pub const S_NO_BRIEF: u32 = 31;
    pub const S_HTML_JSON: u32 = 33;
    pub const S_JSON: u32 = 41;
    pub const S_AGO_INT: u32 = 45;
    pub const S_SIZE_KB: u32 = 53;
    pub const S_SIZE_MB: u32 = 54;
    pub const S_ASKED: u32 = 62;
    pub const S_ASSIGNED: u32 = 65;
    pub const S_OPTIONS: u32 = 67;
    pub const S_BRIEF: u32 = 80;
    pub const S_FOR: u32 = 89;
    pub const S_C: u32 = 90;
    pub const S_AT: u32 = 91;
    pub const S_FILES: u32 = 95;
    /// `_ticket_files_check_py` (`cli/src/browser.sh`), run by `exec`, so
    /// `<string>`.
    pub const T_SIZE_KB: u32 = 10;
    pub const T_SIZE_MB: u32 = 11;
    pub const T_FOR: u32 = 15;
    pub const T_JOIN: u32 = 22;
    pub const T_EXTRA: u32 = 25;
    pub const T_SIZE: u32 = 26;
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `list [--all] [--status <s>] [--json]`. An empty status counts as
    /// none, as the shell's `[ -n "$status" ]` reads it.
    List {
        all: bool,
        status: String,
        json: bool,
    },
    /// `show <id> [--html] [--json]`.
    Show { id: String, json: bool, html: bool },
}

const LIST_USAGE: &str = "usage: k2 tickets list [--all] [--status <s>] [--json]";

/// `_feedback_err <code> <hint>`: the line on stderr, without its newline.
pub fn err_line(code: &str, hint: &str) -> String {
    dumps(&obj(vec![(
        "error",
        obj(vec![("code", s(code)), ("hint", s(hint))]),
    )]))
}

/// The words after `feedback` (its verb rows: `list` or `show` first):
/// the ask, or the usage line that stops it (exit 2). `None`: `-h` or
/// `--help` (the shell's), or another verb.
pub fn parse(words: &[&str]) -> Option<Result<Ask, String>> {
    let (verb, rest) = words.split_first()?;
    if rest.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    match *verb {
        "list" => Some(parse_list(rest)),
        "show" => Some(parse_show(rest)),
        _ => None,
    }
}

fn parse_list(rest: &[&str]) -> Result<Ask, String> {
    let usage = |hint: String| err_line("usage", &hint);
    let (mut all, mut status, mut json) = (false, String::new(), false);
    let mut i = 0;
    while let Some(w) = rest.get(i) {
        match *w {
            "--all" => all = true,
            "--status" => {
                let Some(v) = rest.get(i + 1) else {
                    return Err(usage(
                        "--status requires a value (waiting | needs_discussion | answered | planned | resolved | dismissed)".into(),
                    ));
                };
                status = (*v).to_string();
                i += 2;
                continue;
            }
            "--json" => json = true,
            w if w.starts_with('-') => {
                return Err(usage(format!("unknown flag '{w}' \u{2014} {LIST_USAGE}")))
            }
            w => {
                return Err(usage(format!(
                    "unexpected argument '{w}' \u{2014} {LIST_USAGE}"
                )))
            }
        }
        i += 1;
    }
    if !status.is_empty() && !STATUSES.contains(&status.as_str()) {
        return Err(usage(format!(
            "invalid --status '{status}' \u{2014} valid: waiting, needs_discussion, answered, planned, resolved, dismissed"
        )));
    }
    Ok(Ask::List { all, status, json })
}

fn parse_show(rest: &[&str]) -> Result<Ask, String> {
    let usage = |hint: String| err_line("usage", &hint);
    let (mut id, mut json, mut html) = (String::new(), false, false);
    for w in rest {
        match *w {
            "--json" => json = true,
            "--html" => html = true,
            w if w.starts_with('-') => {
                return Err(usage(format!(
                    "unknown flag '{w}' \u{2014} usage: k2 tickets show <id> [--html] [--json]"
                )))
            }
            // `if [ -z "$id" ]; then id="$1"`: an empty id is taken again.
            w if id.is_empty() => id = w.to_string(),
            w => {
                return Err(usage(format!(
                    "unexpected extra argument '{w}' \u{2014} usage: k2 tickets show <id>"
                )))
            }
        }
    }
    if id.is_empty() {
        return Err(usage(
            "show requires an <id> (from ask/list; a short unique prefix works)".into(),
        ));
    }
    Ok(Ask::Show { id, json, html })
}

/// `k5 feedback list|show ...`: `args` is the whole command line after
/// `argv[0]`. `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let ask = parse(&words)?;
    if !python::family_native(python::here()) {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if prelude::gate(&settings).is_none() {
        // A port curl won't take is the shell's.
        curl_port(settings.port_text.as_deref()?)?;
    }
    Some(run_with(args, ask, settings))
}

fn run_with(args: &[OsString], ask: Result<Ask, String>, mut settings: Settings) -> Exit {
    if let Some(notice) = prelude::rename_notice(Path::new(&settings.project)) {
        out::stderr_line(notice);
    }
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let ask = match ask {
        Ok(a) => a,
        Err(line) => {
            out::stderr_line(&line);
            return Exit::Usage;
        }
    };
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let exit = match ask {
        Ask::List { all, status, json } => list(&mut client, &mut closing, all, &status, json),
        Ask::Show { id, json, html } => show(&mut client, &mut closing, &id, json, html),
    };
    closing.flush(&mut client, exit);
    exit
}

fn unreachable_line() -> Shown {
    Shown::err(
        err_line("daemon_unreachable", UNREACHABLE) + "\n",
        Exit::Error,
    )
}

fn list(client: &mut Client, closing: &mut Closing, all: bool, status: &str, json: bool) -> Exit {
    use_passport_project(client);
    let status_arg = format!("status={}", quote(status));
    let args: &[&str] = if !status.is_empty() {
        &[status_arg.as_str()]
    } else if all {
        &["all=1"]
    } else {
        &[]
    };
    fetch(client, closing, LIST, args, |raw| list_reader(raw, json))
}

fn show(client: &mut Client, closing: &mut Closing, id: &str, json: bool, html: bool) -> Exit {
    let id_arg = format!("id={}", quote(id));
    fetch(client, closing, SHOW, &[id_arg.as_str(), "brief=1"], |raw| {
        show_reader(&String::from_utf8_lossy(raw), json, html, now())
    })
}

/// `resp=$(cli_request <ep> <args>) || resp=""`, then the reader on it:
/// the transport's line when there was no answer, the
/// `daemon_unreachable` line when the call failed any other way or the
/// answer, as `$(…)` holds it, is empty.
fn fetch(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    args: &[&str],
    reader: impl FnOnce(&[u8]) -> Shown,
) -> Exit {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, endpoint, Call::Request(args)) {
        Ok(r) => {
            closing.answer(&r, passport);
            let raw = super::connections::held(&r.body);
            if raw.is_empty() {
                unreachable_line().print()
            } else {
                reader(&raw).print()
            }
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            Exit::Error
        }
        Err(_) => unreachable_line().print(),
    }
}

/// `time.time()`.
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

// ---------------------------------------------------------------------
// Python's values, as the two readers use them.

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// `json.dumps(v)`: Python's defaults (`ensure_ascii`, `, ` and `: `),
/// which can write any value `json.loads` gave.
fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default())
        .expect("json.dumps with ensure_ascii writes any value json.loads gave")
}

/// `d.get(key)` where `d` is a dict for sure.
fn at<'a>(d: &'a Value, key: &str) -> Option<&'a Value> {
    match d {
        Value::Object(pairs) => get(pairs, key),
        _ => None,
    }
}

/// `d.get(key, default)`.
fn at_or(d: &Value, key: &str, default: Value) -> Value {
    at(d, key).cloned().unwrap_or(default)
}

fn truthy_at(d: &Value, key: &str) -> bool {
    at(d, key).is_some_and(py::truthy)
}

/// A Python `str`: its code points (half of a surrogate pair too).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PyStr(pub Vec<u32>);

impl PyStr {
    /// `str(v)`.
    pub fn of(v: &Value) -> PyStr {
        match v {
            Value::Str(t) => PyStr(t.code_points()),
            other => PyStr::from(py::repr(other).as_str()),
        }
    }

    /// `s[:n]`.
    pub fn head(mut self, n: usize) -> PyStr {
        self.0.truncate(n);
        self
    }

    /// As Python writes it out, half of a surrogate pair as `\udXXX`.
    pub fn render(&self) -> String {
        self.0
            .iter()
            .map(|&cp| char::from_u32(cp).map_or_else(|| format!("\\u{cp:04x}"), String::from))
            .collect()
    }

    /// `'%-<w>s'`: padded with spaces on the right to `w` code points.
    pub fn ljust(&self, w: usize) -> String {
        let pad = w.saturating_sub(self.0.len());
        format!("{}{}", self.render(), " ".repeat(pad))
    }

    /// `'%<w>s'`: padded with spaces on the left to `w` code points.
    pub fn rjust(&self, w: usize) -> String {
        let pad = w.saturating_sub(self.0.len());
        format!("{}{}", " ".repeat(pad), self.render())
    }

    /// `s.splitlines()`: at `\n`, `\r`, `\r\n`, `\v`, `\f`, `\x1c`,
    /// `\x1d`, `\x1e`, `\x85`, ` ` and ` `; no ends kept, and
    /// no empty last line after a final break.
    pub fn splitlines(&self) -> Vec<PyStr> {
        let mut out = Vec::new();
        let mut cur = Vec::new();
        let mut i = 0;
        while i < self.0.len() {
            let cp = self.0[i];
            if matches!(
                cp,
                0x0a | 0x0b | 0x0c | 0x0d | 0x1c | 0x1d | 0x1e | 0x85 | 0x2028 | 0x2029
            ) {
                out.push(PyStr(std::mem::take(&mut cur)));
                if cp == 0x0d && self.0.get(i + 1) == Some(&0x0a) {
                    i += 1;
                }
            } else {
                cur.push(cp);
            }
            i += 1;
        }
        if !cur.is_empty() {
            out.push(PyStr(cur));
        }
        out
    }
}

impl From<&str> for PyStr {
    fn from(text: &str) -> PyStr {
        PyStr(text.chars().map(u32::from).collect())
    }
}

/// `str(v)`, written out.
fn str_of(v: &Value) -> String {
    PyStr::of(v).render()
}

/// `str(d.get(key, ""))[:8]`.
fn short_id(d: &Value) -> String {
    PyStr::of(&at_or(d, "id", s(""))).head(8).render()
}

/// A Python `int`: its digits, `-` first when negative, never `-0`.
fn int_digits(v: &str) -> String {
    let (neg, digits) = match v.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, v),
    };
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        "0".into()
    } else if neg {
        format!("-{digits}")
    } else {
        digits.into()
    }
}

/// `int(v)`, or the exception's last line.
pub fn py_int(v: &Value) -> Result<String, String> {
    match v {
        Value::Int(d) => Ok(d.clone()),
        Value::Bool(b) => Ok(if *b { "1" } else { "0" }.into()),
        Value::Float(f) => py::percent_d(&Value::Float(*f)),
        Value::Str(t) => {
            let bad = || {
                format!(
                    "ValueError: invalid literal for int() with base 10: {}",
                    py::repr(v)
                )
            };
            let text = t.as_str().ok_or_else(bad)?;
            let text = text.trim_matches(py_space);
            let (neg, body) = match text.as_bytes().first() {
                Some(b'-') => (true, &text[1..]),
                Some(b'+') => (false, &text[1..]),
                _ => (false, text),
            };
            // Digits, a single `_` only between two of them.
            let ok = !body.is_empty()
                && body.bytes().all(|b| b.is_ascii_digit() || b == b'_')
                && !body.starts_with('_')
                && !body.ends_with('_')
                && !body.contains("__");
            if !ok {
                return Err(bad());
            }
            let digits: String = body.chars().filter(|c| *c != '_').collect();
            Ok(int_digits(&if neg { format!("-{digits}") } else { digits }))
        }
        other => Err(format!(
            "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py::type_name(other)
        )),
    }
}

/// `n < bound` for a Python int's digits.
fn int_below(n: &str, bound: u64) -> bool {
    if n.starts_with('-') {
        return true;
    }
    n.parse::<u64>().is_ok_and(|v| v < bound)
}

/// `float(n)` for a Python int's digits, or the `OverflowError`.
fn int_to_float(n: &str) -> Result<f64, String> {
    let f: f64 = n.parse().unwrap_or(f64::INFINITY);
    if f.is_infinite() {
        Err("OverflowError: int too large to convert to float".into())
    } else {
        Ok(f)
    }
}

/// `'%.1f' % (x / 2**p)` for a whole `x >= 0`, rounded half to even on
/// the exact value, as Python rounds.
pub fn tenths(x: f64, p: u32) -> String {
    if x < 2f64.powi(100) {
        let t = (x as u128) * 10;
        let mut q = t >> p;
        let r = t & ((1u128 << p) - 1);
        let half = 1u128 << (p - 1);
        if r > half || (r == half && q & 1 == 1) {
            q += 1;
        }
        format!("{}.{}", q / 10, q % 10)
    } else {
        // A whole number here: its exact digits.
        format!("{:.1}", x / 2f64.powi(p as i32))
    }
}

/// The shell's `size(n)` (and `ticket_size`) on a Python int's digits:
/// the text, or the exception and the line it is raised at (`kb`, `mb`).
pub fn size(n: &str, kb: u32, mb: u32) -> Result<String, (u32, String)> {
    if int_below(n, 1024) {
        return Ok(format!("{n} B"));
    }
    let f = int_to_float(n).map_err(|e| (if int_below(n, 1 << 20) { kb } else { mb }, e))?;
    if int_below(n, 1 << 20) {
        Ok(format!("{} KB", tenths(f, 10)))
    } else {
        Ok(format!("{} MB", tenths(f, 20)))
    }
}

/// `n // d` for a whole, positive number's digits.
fn floor_div(digits: &str, d: u64) -> String {
    let mut out = String::new();
    let mut rem: u128 = 0;
    for b in digits.bytes() {
        rem = rem * 10 + u128::from(b - b'0');
        let q = rem / u128::from(d);
        rem %= u128::from(d);
        if !(out.is_empty() && q == 0) {
            out.push(char::from(b'0' + q as u8));
        }
    }
    if out.is_empty() {
        "0".into()
    } else {
        out
    }
}

/// The shell's `ago(ts)` (`show`), called from line `from`.
pub fn ago(ts: &Value, now: f64, from: u32) -> Result<String, Raise> {
    let inside = |e: String| Raise::inside(from, line::S_AGO_INT, "ago", e);
    let t = match ts {
        Value::Int(d) => int_to_float(d).map_err(inside)?,
        Value::Float(f) => *f,
        Value::Bool(b) => f64::from(u8::from(*b)),
        other => {
            return Err(inside(format!(
                "TypeError: unsupported operand type(s) for -: 'float' and '{}'",
                py::type_name(other)
            )))
        }
    };
    let x = now - t;
    if x.is_nan() {
        return Err(inside(
            "ValueError: cannot convert float NaN to integer".into(),
        ));
    }
    if x.is_infinite() {
        return Err(inside(
            "OverflowError: cannot convert float infinity to integer".into(),
        ));
    }
    let sec = x.trunc();
    if sec < 60.0 {
        return Ok("just now".into());
    }
    let digits = py::percent_d(&Value::Float(sec)).map_err(inside)?;
    Ok(if sec < 3600.0 {
        format!("{}m ago", floor_div(&digits, 60))
    } else if sec < 172_800.0 {
        format!("{}h ago", floor_div(&digits, 3600))
    } else {
        format!("{}d ago", floor_div(&digits, 86400))
    })
}

/// `sep.join(v)`, or the `TypeError`.
fn join(v: &Value, sep: &str) -> Result<String, String> {
    let items: Vec<Value> = match v {
        Value::Array(items) => items.clone(),
        Value::Str(_) | Value::Object(_) => py_iter(v, 0).map_err(|r| r.exception)?,
        _ => return Err("TypeError: can only join an iterable".into()),
    };
    let mut parts = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match item {
            Value::Str(_) => parts.push(str_of(item)),
            other => {
                return Err(format!(
                    "TypeError: sequence item {i}: expected str instance, {} found",
                    py::type_name(other)
                ))
            }
        }
    }
    Ok(parts.join(sep))
}

/// What the Python printed so far.
#[derive(Default)]
struct Out {
    stdout: String,
    stderr: String,
}

impl Out {
    fn line(&mut self, text: &str) {
        self.stdout.push_str(text);
        self.stdout.push('\n');
    }

    fn done(self, exit: Exit) -> Shown {
        Shown {
            stdout: self.stdout,
            stderr: self.stderr,
            exit,
        }
    }

    /// The traceback after what was printed: every frame is `<string>`'s,
    /// the reader's own (`python3 -c`) and `_ticket_files_check_py`'s (run
    /// by `exec`) alike.
    fn raised(self, r: &Raise) -> Shown {
        Shown {
            stdout: self.stdout,
            stderr: self.stderr + &super::traceback(&r.frames, &r.exception),
            exit: Exit::Error,
        }
    }
}

/// The exit for the daemon's error `code`.
fn code_exit(code: Option<&Value>) -> Exit {
    match code.and_then(|c| match c {
        Value::Str(t) => t.as_str(),
        _ => None,
    }) {
        Some("not_found" | "ambiguous_id") => Exit::NotFound,
        Some("usage") => Exit::Usage,
        _ => Exit::Error,
    }
}

/// `die(err)` (and `list`'s same lines): the error as JSON on stderr,
/// then its exit; `err.get` on what is not a dict raises at `get_line`
/// (inside `die`, called from `from`, when `from` is not 0).
fn die(o: &mut Out, err: &Value, from: u32, get_line: u32) -> Result<Exit, Raise> {
    let err = match err {
        Value::Str(_) => obj(vec![("code", s("error")), ("hint", err.clone())]),
        other => other.clone(),
    };
    o.stderr
        .push_str(&dumps(&obj(vec![("error", err.clone())])));
    o.stderr.push('\n');
    match &err {
        Value::Object(pairs) => Ok(code_exit(get(pairs, "code"))),
        other => {
            let e = Raise::no_attribute(get_line, other, "get");
            Err(if from == 0 {
                e
            } else {
                Raise::inside(from, get_line, "die", e.exception)
            })
        }
    }
}

/// `"...: %r" % raw[:200]`.
fn head_repr(raw: &str) -> String {
    py::repr(&Value::Str(Text::from(
        raw.chars().take(200).collect::<String>(),
    )))
}

// ---------------------------------------------------------------------
// `list`

/// `list`'s Python on `raw` (what `$(…)` held, handed over in `RESP`).
pub fn list_reader(raw: &[u8], as_json: bool) -> Shown {
    let text = String::from_utf8_lossy(raw);
    let Ok(d) = json::parse(&text) else {
        let hint = format!(
            "daemon returned an unparseable response: {}",
            head_repr(&text)
        );
        return Shown::err(err_line("bad_response", &hint) + "\n", Exit::Error);
    };
    let mut o = Out::default();
    match list_lines(&d, as_json, &mut o) {
        Ok(exit) => o.done(exit),
        Err(r) => o.raised(&r),
    }
}

fn list_lines(d: &Value, as_json: bool, o: &mut Out) -> Result<Exit, Raise> {
    if let Some(err) = py_get(d, "error", line::L_ERR)? {
        if *err != Value::Null {
            return die(o, err, 0, line::L_CODE);
        }
    }
    if as_json {
        o.line(&dumps(d));
        return Ok(Exit::Ok);
    }
    o.line("ID        STATUS             PRI  KIND      TITLE");
    let items = at_or(d, "items", Value::Array(Vec::new()));
    for i in py_iter(&items, line::L_FOR)? {
        py_get(&i, "id", line::L_ROW)?;
        o.line(&format!(
            "{}{}{}   {}{}",
            PyStr::of(&at_or(&i, "id", s(""))).head(8).ljust(10),
            PyStr::of(&at_or(&i, "status", s("?"))).ljust(18),
            PyStr::of(&at_or(&i, "priority", s("?"))).rjust(2),
            PyStr::of(&at_or(&i, "kind", s("?"))).ljust(10),
            str_of(&at_or(&i, "title", s(""))),
        ));
    }
    Ok(Exit::Ok)
}

// ---------------------------------------------------------------------
// `show`

/// `show`'s Python on the answer's body, decoded as UTF-8 with U+FFFD for
/// each bad byte (`decode("utf-8", "replace")`). `now`: its `time.time()`.
pub fn show_reader(raw: &str, as_json: bool, want_html: bool, now: f64) -> Shown {
    let mut o = Out::default();
    match show_lines(raw, as_json, want_html, now, &mut o) {
        Ok(exit) => o.done(exit),
        Err(r) => o.raised(&r),
    }
}

fn die_with(o: &mut Out, code: &str, hint: &str, from: u32) -> Result<Exit, Raise> {
    die(
        o,
        &obj(vec![("code", s(code)), ("hint", s(hint))]),
        from,
        line::S_DIE_CODE,
    )
}

fn show_lines(
    raw: &str,
    as_json: bool,
    want_html: bool,
    now: f64,
    o: &mut Out,
) -> Result<Exit, Raise> {
    let Ok(mut d) = json::parse(raw) else {
        let hint = format!(
            "daemon returned an unparseable response: {}",
            head_repr(raw)
        );
        return die_with(o, "bad_response", &hint, line::S_UNPARSEABLE);
    };
    if !matches!(d, Value::Object(_)) {
        let hint = format!("daemon returned an unexpected response: {}", head_repr(raw));
        return die_with(o, "bad_response", &hint, line::S_UNEXPECTED);
    }
    if let Some(err) = at(&d, "error") {
        if *err != Value::Null {
            return die(o, &err.clone(), line::S_ERR, line::S_DIE_CODE);
        }
    }
    let brief = at(&d, "brief")
        .filter(|b| matches!(b, Value::Object(_)))
        .cloned();
    if want_html {
        let html = brief.as_ref().and_then(|b| match at(b, "html") {
            Some(h @ Value::Str(_)) => Some(h.clone()),
            _ => None,
        });
        let Some(html) = html else {
            let hint = format!("ticket {} has no HTML brief", short_id(&d));
            return die_with(o, "no_brief", &hint, line::S_NO_BRIEF);
        };
        if as_json {
            o.line(&dumps(&d));
        } else {
            o.stdout.push_str(&str_of(&html));
        }
        return Ok(Exit::Ok);
    }
    if let Value::Object(pairs) = &mut d {
        pairs.retain(|(k, _)| *k != Text::from("brief"));
    }
    if as_json {
        o.line(&dumps(&d));
        return Ok(Exit::Ok);
    }
    let d = &d;
    o.line(&format!(
        "feedback : {}  ({})",
        short_id(d),
        str_of(&at_or(d, "status", s("?")))
    ));
    o.line(&format!("title    : {}", str_of(&at_or(d, "title", s("")))));
    o.line(&format!(
        "kind     : {} \u{b7} priority {}",
        str_of(&at_or(d, "kind", s("?"))),
        str_of(&at_or(d, "priority", s("?")))
    ));
    let mut asked = match at(d, "agentName").filter(|v| py::truthy(v)) {
        Some(v) => str_of(v),
        None => "?".into(),
    };
    if let Some(sid) = at(d, "sessionId").filter(|v| py::truthy(v)) {
        asked.push_str(&format!(
            " \u{b7} session {}",
            PyStr::of(sid).head(8).render()
        ));
    }
    let created = at_or(d, "createdAt", Value::Int("0".into()));
    asked.push_str(&format!(" \u{b7} {}", ago(&created, now, line::S_ASKED)?));
    o.line(&format!("asked by : {asked}"));
    let assignees = at(d, "assignees")
        .filter(|v| py::truthy(v))
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let assigned = if py::truthy(&assignees) {
        join(&assignees, ", ").map_err(|e| Raise::at(line::S_ASSIGNED, e))?
    } else {
        "(none)".into()
    };
    o.line(&format!("assigned : {assigned}"));
    if let Some(options) = at(d, "options").filter(|v| py::truthy(v)) {
        let text = join(options, " | ").map_err(|e| Raise::at(line::S_OPTIONS, e))?;
        o.line(&format!("options  : {text}"));
    }
    if let Some(answer) = at(d, "answer").filter(|v| py::truthy(v)) {
        o.line(&format!("answer   : {}", str_of(answer)));
    }
    if let Some(body) = at(d, "body").filter(|v| py::truthy(v)) {
        let mut lines = PyStr::of(body).splitlines();
        if lines.is_empty() {
            lines.push(PyStr::default());
        }
        o.line(&format!("body     : {}", lines[0].render()));
        for extra in &lines[1..] {
            o.line(&format!("{}{}", " ".repeat(11), extra.render()));
        }
    }
    if truthy_at(d, "hasBrief") {
        let mut n = at_or(d, "briefBytes", Value::Null);
        if n == Value::Null {
            if let Some(b) = &brief {
                n = at_or(b, "bytes", Value::Null);
            }
        }
        let shown = match &n {
            Value::Int(_) | Value::Float(_) | Value::Bool(_) => {
                let digits = py_int(&n).map_err(|e| Raise::at(line::S_BRIEF, e))?;
                size(&digits, line::S_SIZE_KB, line::S_SIZE_MB)
                    .map_err(|(l, e)| Raise::inside(line::S_BRIEF, l, "size", e))?
            }
            _ => "?".into(),
        };
        o.line(&format!(
            "brief    : HTML, {shown}  (k2 tickets show {} --html > brief.html)",
            short_id(d)
        ));
        let text = match brief.as_ref().and_then(|b| at(b, "text")) {
            Some(t) if py::truthy(t) => PyStr::of(t),
            _ => PyStr::default(),
        };
        let lines = text.splitlines();
        for l in lines.iter().take(40) {
            o.line(&format!("  {}", l.render()));
        }
        if lines.len() > 40 {
            o.line(&format!(
                "  \u{2026} {} more lines (open the ticket, or use --html)",
                lines.len() - 40
            ));
        }
    }
    o.line("");
    o.line("thread:");
    let comments = at_or(d, "comments", Value::Array(Vec::new()));
    let root = at_or(d, "projectPath", Value::Null);
    for c in py_iter(&comments, line::S_FOR)? {
        let body = py_get(&c, "body", line::S_C)?.cloned().unwrap_or(s(""));
        let mut lines = PyStr::of(&body).splitlines();
        if lines.is_empty() {
            lines.push(PyStr::default());
        }
        let author = PyStr::of(&at_or(&c, "author", s("?"))).ljust(9);
        let when = ago(&at_or(&c, "at", Value::Int("0".into())), now, line::S_AT)?;
        o.line(&format!(
            "  {author}{}{}",
            PyStr::from(format!("({when})").as_str()).ljust(10),
            lines[0].render()
        ));
        for extra in &lines[1..] {
            o.line(&format!("{}{}", " ".repeat(21), extra.render()));
        }
        let atts = at_or(&c, "attachments", Value::Null);
        for l in ticket_file_lines(&atts, &root, &" ".repeat(21))? {
            o.line(&l);
        }
    }
    Ok(Exit::Ok)
}

/// `_ticket_files_check_py`'s `ticket_file_lines(atts, root, indent)`
/// (`cli/src/browser.sh`), called from `show`'s line [`line::S_FILES`].
pub fn ticket_file_lines(atts: &Value, root: &Value, indent: &str) -> Result<Vec<String>, Raise> {
    let raise = |l: u32, e: String| Raise::inside(line::S_FILES, l, "ticket_file_lines", e);
    let mut out = Vec::new();
    if !py::truthy(atts) {
        return Ok(out);
    }
    let items = py_iter(atts, 0).map_err(|r| raise(line::T_FOR, r.exception))?;
    for a in &items {
        if !matches!(a, Value::Object(_)) {
            continue;
        }
        let state = at(a, "state");
        if state.is_some_and(|v| *v == s("hidden")) {
            out.push(format!("{indent}[file] (not shared with you)"));
            continue;
        }
        let rel = PyStr::of(&at_or(a, "path", s("")));
        let mut shown = if py::truthy(root) {
            let Value::Str(t) = root else {
                return Err(raise(
                    line::T_JOIN,
                    format!(
                        "TypeError: expected str, bytes or os.PathLike object, not {}",
                        py::type_name(root)
                    ),
                ));
            };
            // posixpath.join(root, rel).
            let mut path = t.code_points();
            if rel.0.first() == Some(&u32::from('/')) {
                path = rel.0.clone();
            } else {
                if path.last() != Some(&u32::from('/')) {
                    path.push(u32::from('/'));
                }
                path.extend_from_slice(&rel.0);
            }
            PyStr(path).render()
        } else {
            rel.render()
        };
        if truthy_at(a, "line") {
            shown.push(':');
            shown.push_str(&str_of(&at_or(a, "line", Value::Null)));
            if truthy_at(a, "lineEnd") {
                shown.push('-');
                shown.push_str(&str_of(&at_or(a, "lineEnd", Value::Null)));
            }
        }
        let extra = match state {
            Some(v @ (Value::Array(_) | Value::Object(_))) => {
                return Err(raise(
                    line::T_EXTRA,
                    format!("TypeError: unhashable type: '{}'", py::type_name(v)),
                ))
            }
            Some(Value::Str(t)) if t.as_str() == Some("changed") => " (changed since sent)",
            Some(Value::Str(t)) if t.as_str() == Some("gone") => " (gone)",
            _ => "",
        };
        let n = match at(a, "size").filter(|v| py::truthy(v)) {
            Some(v) => py_int(v).map_err(|e| raise(line::T_SIZE, e))?,
            None => "0".into(),
        };
        let size = size(&n, line::T_SIZE_KB, line::T_SIZE_MB).map_err(|(l, e)| Raise {
            frames: vec![
                (line::S_FILES, "<module>"),
                (line::T_SIZE, "ticket_file_lines"),
                (l, "ticket_size"),
            ],
            exception: e,
        })?;
        out.push(format!("{indent}[file] {shown} ({size}){extra}"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
