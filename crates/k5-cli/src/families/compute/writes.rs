//! The `compute` verbs that write, wait or move files, as the shell CLI's
//! `cmd_compute` answers them (`cli/src/compute-command.sh` and the files
//! it calls). Made native one verb at a time:
//!
//! - [`cancel`]: `k5 compute cancel <job> [--force|--if-queued]` and
//!   `k5 compute cancel --all-mine|--workspace-all [...]`.
//! - [`wait`]: `k5 compute wait <job>... [--any] [--max-wait D] [--quiet]
//!   [--json]`.
//! - [`grant`]: `k5 compute grant <node> --workspace WS [...]`.
//! - [`fetch`]: `k5 compute fetch <job> [NAME...] [--out DIR] [--json]
//!   [--over-relay] [--attempt N]`.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]).
//! 2. `compute` is an `id` tool, so with `HOOK_SOCK` set: the
//!    stale-passport or require-passport lines, exit 3
//!    ([`crate::prelude::id_tool`]).
//! 3. The verb's words, then its requests through `_compute_send`
//!    ([`Client::compute_send`]): `_compute_call` ([`call`]) fails on any
//!    status but 200 with `_compute_fail`'s line ([`fail_line`]).
//! 4. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! What the binary hands to the shell, before it prints or sends anything:
//! a verb not made native here, a command line with `-h` or `--help` (the
//! front does that), a word that is not UTF-8, `HOOK_SOCK` set with a
//! `CODEX_` variable exported, a port curl won't take, a `python3` the
//! binary can't tell, and the environment's own settings for the shell's
//! internals that would change what it sends or prints ([`shell_settings`]).
//!
//! The helpers here (`_compute_send`'s answer as `_compute_split` holds
//! it, `_compute_fail`, `_compute_json`'s bodies, `_compute_ws`) are the
//! shell's shared compute library; the read verbs need the same ones.
//! They live in this family's module until there is a shared place.

use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::families::{curl_port, get, py_space, without_nuls, Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::client::{Auth, Client, TRIES};
use crate::transport::{permission, Error, Response};
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

pub mod cancel;
pub mod fetch;
pub mod grant;
pub mod wait;

#[cfg(test)]
mod tests;

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "compute";

/// The verbs this module answers (the family's [`super::answers`]).
pub const ANSWERS: &[&str] = &["cancel", "wait", "fetch", "grant"];

/// The status `_compute_split` reads when no answer came (`printf '\n000'`).
pub const NO_STATUS: &str = "000";

/// A verb's words, read, with nothing to stop it.
enum Ready {
    Cancel(cancel::Ask),
    Wait(wait::Ask),
    Fetch(fetch::Ask),
    Grant(grant::Ask),
}

impl<T> Words<T> {
    /// The ask, or what stopping prints and the exit.
    fn ready(self) -> Result<T, Exit> {
        match self {
            Words::Ask(a) => Ok(a),
            Words::Usage(line) => {
                out::stderr_line(&line);
                Err(Exit::Usage)
            }
            Words::ShiftFailed(flag) => Err(out::stop_need_value(&flag)),
        }
    }
}

/// What a verb's words ask for: the request, or what stops it first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Words<T> {
    Ask(T),
    /// `echo "<line>" >&2; exit 2`.
    Usage(String),
    /// A value flag as the last word (the flag): the shell's
    /// `_need_value` line ([`crate::out::need_value`]), exit 2.
    ShiftFailed(String),
}

/// A verb this module answers, with its words read.
enum Form {
    Cancel(Words<cancel::Ask>),
    Wait(Words<wait::Ask>),
    Fetch(Words<fetch::Ask>),
    /// Read once the settings say whether this is a session.
    Grant(Vec<String>),
}

/// `k5 compute <verb> ...`: `args` is the whole command line after
/// `argv[0]`. `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = crate::families::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let form = match words.first() {
        Some(&"cancel") => Form::Cancel(cancel::parse(&words[1..])),
        Some(&"wait") => {
            let w = wait::parse(&words[1..])?;
            // `mktemp -d` before the first poll: where it can't make its
            // folder, the shell says so in its own words. A follow makes
            // its own file instead ([`wait::follow_handed`]).
            if matches!(&w, Words::Ask(a) if !a.any && !a.follow) && !wait::temp_usable() {
                return None;
            }
            if matches!(&w, Words::Ask(a) if wait::follow_handed(a)) {
                return None;
            }
            Form::Wait(w)
        }
        Some(&"fetch") => {
            let f = fetch::parse(&words[1..]);
            // A folder `mkdir -p` would refuse: its line is the system's.
            if matches!(&f, Words::Ask(a) if !fetch::creatable(&a.out)) {
                return None;
            }
            Form::Fetch(f)
        }
        Some(&"grant") => Form::Grant(words[1..].iter().map(|w| w.to_string()).collect()),
        _ => return None,
    };
    let max_time = post_max_time(std::env::var_os("K2_CP_MAX_TIME"))?;
    if shell_settings(std::env::vars_os()) {
        return None;
    }
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
    if prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    let as_json = words.iter().skip(1).any(|w| *w == "--json");
    Some(run_with(
        args,
        form,
        as_json,
        max_time,
        hook_sock.as_deref(),
        settings,
    ))
}

fn run_with(
    args: &[OsString],
    form: Form,
    as_json: bool,
    max_time: Option<Duration>,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let verb = match form {
        Form::Cancel(w) => w.ready().map(Ready::Cancel),
        Form::Wait(w) => w.ready().map(Ready::Wait),
        Form::Fetch(w) => w.ready().map(Ready::Fetch),
        Form::Grant(w) => {
            let w: Vec<&str> = w.iter().map(String::as_str).collect();
            grant::parse(&w, in_session(&settings.auth))
                .ready()
                .map(Ready::Grant)
        }
    };
    let ask = match verb {
        Ok(a) => a,
        Err(exit) => return exit,
    };
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    client.compute_post_max_time = max_time;
    let mut cp = Cp {
        client: &mut client,
        closing: Closing::default(),
        project: settings.project.clone(),
        as_json,
    };
    let exit = match &ask {
        Ready::Cancel(a) => cancel::send(&mut cp, a),
        Ready::Wait(a) => wait::send(&mut cp, a),
        Ready::Fetch(a) => fetch::send(&mut cp, a),
        Ready::Grant(a) => grant::send(&mut cp, a),
    };
    let closing = std::mem::take(&mut cp.closing);
    closing.flush(&mut client, exit);
    exit
}

/// `--max-time "${K2_CP_MAX_TIME:-120}"` as curl reads it: `None` (the
/// outer one) for a value the binary leaves to the shell (curl's own
/// reading of anything but plain seconds, and 0, which is no limit).
pub fn post_max_time(v: Option<OsString>) -> Option<Option<Duration>> {
    let Some(v) = v else {
        return Some(None);
    };
    let text = v.to_str()?;
    if text.is_empty() {
        return Some(None);
    }
    let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || !digits(frac) || (whole.is_empty() && frac.is_empty()) {
        return None;
    }
    let secs: f64 = format!("0{whole}.{frac}0").parse().ok()?;
    (secs > 0.0 && secs < 1.0e9).then(|| Some(Duration::from_secs_f64(secs)))
}

/// Settings of the shell's own internals that an environment can carry
/// into it and that change what a compute verb sends or prints: a
/// `K2_CJ_` variable (`_compute_json` puts every one in the body),
/// `_K2_REFUSAL_AREA` (the App refusal rows) and `_K2_NOANSWER_SOFT` (no
/// line for a POST with no answer). Any of them set: the shell answers.
/// `_CP_404_EXIT` is not one: `_compute_fail` takes its exit for a 404
/// from its caller and never reads the environment.
pub fn shell_settings(vars: impl IntoIterator<Item = (OsString, OsString)>) -> bool {
    vars.into_iter().any(|(name, value)| {
        let name = name.to_string_lossy();
        name.starts_with("K2_CJ_")
            || (matches!(name.as_ref(), "_K2_REFUSAL_AREA" | "_K2_NOANSWER_SOFT")
                && !value.is_empty())
    })
}

/// What a compute verb's requests share.
pub struct Cp<'a> {
    pub client: &'a mut Client,
    /// The EXIT trap's lines.
    pub closing: Closing,
    /// `$PROJECT`.
    pub project: String,
    /// A `--json` anywhere after the verb (`cmd_compute`'s scan).
    pub as_json: bool,
}

/// `_compute_send`'s answer as `_compute_split` holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// `_CP_STATUS`: curl's `%{http_code}`, `000` for none.
    pub status: String,
    /// `_CP_BODY`: the answer, NULs gone.
    pub body: Vec<u8>,
}

impl<'a> Cp<'a> {
    /// `raw="$(_compute_send ...)"; _compute_split "$raw"`. A request
    /// that may have been applied (a POST the cell socket took and never
    /// answered), or one a session would send as the owner, is its line
    /// and exit 1 (`Err`). Any other failure is curl's write-out, then
    /// `printf '\n000'`: status `000`, and the body `\n000` once for each
    /// time curl ran (`_k2_curl` runs it again on a refused connect, up to
    /// [`TRIES`] times).
    pub fn send(
        &mut self,
        post: bool,
        path: &str,
        payload: &str,
        tcp: bool,
    ) -> Result<Split, Exit> {
        match self.client.compute_send(post, path, payload, tcp) {
            Ok(r) => Ok(self.split(&r)),
            Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
                out::stderr_line(&e.to_string());
                Err(Exit::Error)
            }
            Err(e) => {
                let runs = if matches!(e, Error::Unreachable { .. }) {
                    TRIES
                } else {
                    1
                };
                Ok(Split {
                    status: NO_STATUS.to_string(),
                    body: b"\n000".repeat(runs as usize),
                })
            }
        }
    }

    /// `_compute_split`'s `_permission_note "$_CP_BODY"`.
    fn split(&mut self, r: &Response) -> Split {
        let body = without_nuls(&r.body);
        let noted = Response {
            status: r.status,
            body: body.clone(),
            channel: r.channel,
            permission: permission::note(&body),
            server_version: r.server_version.clone(),
        };
        let passport = self.client.passport().is_some();
        self.closing.answer(&noted, passport);
        Split {
            status: format!("{:03}", r.status),
            body,
        }
    }

    /// `_compute_call METHOD PATH [payload] [tcp]`: the answer's body on a
    /// 200; else `_compute_fail`'s line and its exit (`Err`).
    pub fn call(
        &mut self,
        post: bool,
        path: &str,
        payload: &str,
        tcp: bool,
    ) -> Result<Vec<u8>, Exit> {
        let s = self.send(post, path, payload, tcp)?;
        if s.status == "200" {
            return Ok(s.body);
        }
        Err(fail(&s.status, &s.body))
    }

    /// `_compute_ws "$explicit"`: the explicit workspace, else this
    /// checkout's outside a session (a passport's comes from the passport).
    pub fn ws(&self, explicit: &str) -> String {
        if !explicit.is_empty() {
            explicit.to_string()
        } else if in_session(&self.client.auth) {
            String::new()
        } else {
            self.project.clone()
        }
    }
}

/// `_compute_in_session`: the call carries a session passport.
pub fn in_session(auth: &Auth) -> bool {
    matches!(auth, Auth::Passport { .. })
}

/// `_compute_fail STATUS BODY`: the line on stderr, and the exit (2 for
/// 400 and 413, else 3).
pub fn fail(status: &str, body: &[u8]) -> Exit {
    out::stderr_line(&fail_line(status, body));
    match status {
        "400" | "413" => Exit::Usage,
        _ => Exit::Refused,
    }
}

/// `_compute_fail`'s line: its Python's on a body (the daemon's error as
/// one JSON line, or `daemon_error: ` and the first 200 characters of a
/// body that is not JSON), with `$(…)`'s trailing newlines gone; the
/// unreachable line when there is no body.
pub fn fail_line(status: &str, body: &[u8]) -> String {
    if !body.is_empty() {
        let raw = String::from_utf8_lossy(body);
        let line = match json::parse(&raw) {
            Err(_) => format!(
                "daemon_error: {}",
                raw.chars().take(200).collect::<String>()
            ),
            Ok(v) => {
                let e = match &v {
                    Value::Object(pairs) => get(pairs, "error").cloned().unwrap_or(Value::Null),
                    _ => Value::Null,
                };
                let message = match &v {
                    Value::Object(pairs) => get(pairs, "message").cloned(),
                    _ => None,
                };
                let (code, hint) = match (&e, message) {
                    (Value::Object(pairs), _) => (
                        get(pairs, "code")
                            .cloned()
                            .unwrap_or_else(|| str_value("error")),
                        get(pairs, "hint").cloned().unwrap_or_else(|| str_value("")),
                    ),
                    (Value::Str(_), Some(m @ Value::Str(_))) => (e.clone(), m),
                    _ => (str_value("error"), str_value(&py::str_of(&e))),
                };
                dumps(&object(vec![("error", code), ("hint", hint)]))
            }
        };
        let line = line.trim_end_matches('\n');
        if !line.is_empty() {
            return line.to_string();
        }
    }
    format!(
        concat!(
            "{{\"error\":\"daemon_unreachable\",\"hint\":\"no answer from the ",
            k2_client::product_name!(),
            " daemon (status {})\"}}"
        ),
        status
    )
}

/// A string value.
pub fn str_value(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// An object with these keys, in this order.
pub fn object(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `json.dumps(v)`.
pub fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).unwrap_or_default()
}

/// `_compute_json K2_CJ_<key>=<value> ...` for string values: a JSON
/// object of the pairs whose value is not empty, in their order.
pub fn body_of(pairs: &[(&str, &str)]) -> String {
    dumps(&object(
        pairs
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| (*k, str_value(v)))
            .collect(),
    ))
}

/// `printf '%s\n' "$_CP_BODY"` (the `--json` forms): exit 0.
pub fn print_body(body: &[u8]) -> Exit {
    let mut b = body.to_vec();
    b.push(b'\n');
    match out::write_flushed(&mut std::io::stdout().lock(), &b) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// `json.load(sys.stdin)` on what `printf '%s' "$_CP_BODY"` hands the
/// reader, at line `line` of its Python: the value, or the
/// `JSONDecodeError`.
pub fn load(body: &[u8], line: u32) -> Result<Value, Raise> {
    let raw = String::from_utf8_lossy(body);
    json::parse(&raw).map_err(|e| {
        let why = if raw.chars().all(py_space) {
            "Expecting value: line 1 column 1 (char 0)".to_string()
        } else {
            e.to_string()
        };
        Raise::at(line, format!("json.decoder.JSONDecodeError: {why}"))
    })
}

/// Python's `str` equal to `text`.
pub fn is_str(v: Option<&Value>, text: &str) -> bool {
    matches!(v, Some(Value::Str(t)) if *t == Text::from(text))
}

/// `x or default`, for `%s`.
pub fn or_str(v: Option<&Value>, default: &str) -> String {
    match v {
        Some(x) if py::truthy(x) => py::str_of(x),
        _ => default.to_string(),
    }
}

/// `'%s' % d.get(key)` (`None` when it is missing).
pub fn str_at(v: Option<&Value>) -> String {
    v.map_or_else(|| "None".to_string(), py::str_of)
}

/// The first `n` code points of a word (`s[:n]`).
pub fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `x[key]` where `x` is anything: the value, or the exception.
pub fn subscript(x: &Value, key: &str) -> Result<Value, String> {
    match x {
        Value::Object(pairs) => get(pairs, key)
            .cloned()
            .ok_or_else(|| format!("KeyError: {}", py::repr(&str_value(key)))),
        Value::Str(_) => Err("TypeError: string indices must be integers, not 'str'".to_string()),
        Value::Array(_) => {
            Err("TypeError: list indices must be integers or slices, not str".to_string())
        }
        other => Err(format!(
            "TypeError: '{}' object is not subscriptable",
            py::type_name(other)
        )),
    }
}

/// `int(v)` for a value `json.load` gave: the digits (a leading `-` when
/// negative), or the exception.
pub fn int_of(v: &Value) -> Result<String, String> {
    match v {
        Value::Int(_) | Value::Bool(_) | Value::Float(_) => py::percent_d(v),
        Value::Str(t) => {
            let s = py::str_of(v);
            int_literal(&s).ok_or_else(|| {
                format!(
                    "ValueError: invalid literal for int() with base 10: {}",
                    py::repr(&Value::Str(t.clone()))
                )
            })
        }
        other => Err(format!(
            "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py::type_name(other)
        )),
    }
}

/// `int(s)` of a string: spaces around it, a sign, ASCII digits with
/// single underscores between them.
fn int_literal(s: &str) -> Option<String> {
    let t = s.trim_matches(py_space);
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
    let plain: String = digits.chars().filter(|c| *c != '_').collect();
    let plain = plain.trim_start_matches('0');
    Some(match (plain.is_empty(), neg) {
        (true, _) => "0".to_string(),
        (false, true) => format!("-{plain}"),
        (false, false) => plain.to_string(),
    })
}

/// Python's `divmod(n, d)` on an integer's digits, `d` > 0: floor division.
pub fn divmod(n: &str, d: u32) -> (String, u32) {
    let (neg, digits) = n.strip_prefix('-').map_or((false, n), |r| (true, r));
    let mut q = String::new();
    let mut r: u64 = 0;
    for b in digits.bytes() {
        r = r * 10 + u64::from(b - b'0');
        let digit = r / u64::from(d);
        r %= u64::from(d);
        if !(q.is_empty() && digit == 0) {
            q.push(char::from(b'0' + digit as u8));
        }
    }
    if q.is_empty() {
        q.push('0');
    }
    let r = r as u32;
    if !neg || (q == "0" && r == 0) {
        return (q, r);
    }
    // Floor: -(|n| // d) - 1 when there is a remainder.
    if r == 0 {
        return (format!("-{q}"), 0);
    }
    (format!("-{}", add_one(&q)), d - r)
}

fn add_one(digits: &str) -> String {
    let mut b: Vec<u8> = digits.bytes().collect();
    let mut i = b.len();
    loop {
        if i == 0 {
            b.insert(0, b'1');
            break;
        }
        i -= 1;
        if b[i] == b'9' {
            b[i] = b'0';
        } else {
            b[i] += 1;
            break;
        }
    }
    String::from_utf8(b).unwrap_or_default()
}

/// What a reader printed, then the exception that stopped it: exit 1.
pub fn stopped(stdout: String, r: &Raise) -> Shown {
    crate::families::raised(stdout, String::new(), r)
}
