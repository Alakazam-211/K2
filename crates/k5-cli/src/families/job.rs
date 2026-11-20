//! `k5 job` (also `jobs`), as the shell CLI's `cmd_job` answers it
//! (`cli/src/job.sh`): the daemon's scheduled jobs. Every form: `add`,
//! `edit`, `list` (`ls`), `show`, `logs` (`log`), `run` (`run-now`),
//! `enable`, `disable`, `kill`, `rm` (`remove`, `delete`), `pause`,
//! `resume`, each with `--json`, and the help.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at
//!    once, then the virtual desktop's guard and the connection gate
//!    ([`crate::prelude::gate`]). The help (`job` alone, `help`, `--help`,
//!    `-h`, or `-h`/`--help` before any `--`) skips the connection gate;
//!    a command line with `-h` or `--help` anywhere skips the guard.
//! 2. `job` is an `id` tool (`jobs` in the shell's policy), so with
//!    `HOOK_SOCK` set: the stale-passport or require-passport lines, exit
//!    3 ([`crate::prelude::id_tool`]), the help too.
//! 3. The words ([`parse`]): each problem is one `usage:` line, exit 2.
//!    The JSON body and the query are the shell's Python's
//!    ([`Ask::body`], [`Ask::query`]): the workspace is `--workspace`,
//!    else the project when there is no passport (inside a K2 terminal
//!    the daemon pins the passport's own workspace).
//! 4. One `_sidecar_send` ([`crate::transport::Client::sidecar_get`] or
//!    [`crate::transport::Client::sidecar_post`]): the cell socket with
//!    the passport when there is a live one, else TCP. A POST that went
//!    out on the socket is never sent again: no usable answer there is
//!    the shell's "may have been applied" line, exit 1.
//! 5. `_job_call`: a status that is not 200 is `daemon_unreachable` for
//!    none (`000`), else its Python's `code: hint` line ([`fail_line`])
//!    with `_job_exit_for`'s exit. No closing line and no bulletin: the
//!    shell's `_job_call` notes nothing.
//! 6. `--json` prints the body as it came; else the shell's Python lays
//!    it out ([`render`]).
//!
//! The shell CLI answers these itself ([`run`] returns `None`): a word
//! that is not UTF-8, `HOOK_SOCK` set with a `CODEX_` variable exported,
//! a port that is set but is no port curl takes (for a form that asks),
//! and a `python3` the binary can't tell.
//!
//! Where the shell's Python raises (an answer that is not JSON, a field
//! of a type it doesn't expect), this prints a Python 3.12 traceback, as
//! [`super::sessions`] says; bytes that are not UTF-8 and half of a
//! surrogate pair are as [`super::inbox`] says.

use super::{curl_port, get, ljust, py_get, py_iter, py_left, py_len, py_space, raised, s, Raise};
use super::{without_nuls, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Gate};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{Error, Response};
use k2_client::brand::CLI_NAME as CLI;
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["job", "jobs"];
/// The binary answers every form ([`run`] may still hand one to the
/// shell CLI).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: the whole row is native.
pub const VERBS: &[crate::route::Verb] = &[];

/// The routes, under `/cli/jobs/`.
pub const ROUTE: &str = "/cli/jobs/";
/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "jobs";
/// `_job_call` with no status.
pub const UNREACHABLE: &str = concat!(
    "daemon_unreachable: the ",
    k2_client::product_name!(),
    " daemon did not answer"
);
/// `add` with no command.
pub const ADD_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " job add NAME --every 15m -- <cmd> [args\u{2026}]"
);

/// A value flag with no value (or an empty one).
pub fn needs_value(flag: &str) -> String {
    format!("usage: {flag} needs a value ({CLI} job --help)")
}

/// A `--` flag the family doesn't have.
pub fn unknown_flag(flag: &str) -> String {
    format!("usage: unknown flag '{flag}' ({CLI} job --help)")
}

/// A verb that needs a name, without one (an unknown verb too).
pub fn needs_name(sub: &str) -> String {
    format!("usage: {CLI} job {sub} NAME ({CLI} job --help)")
}

/// `pause` or `resume` with neither a name nor `--all`.
pub fn pause_usage(sub: &str) -> String {
    format!("usage: {CLI} job {sub} NAME | {CLI} job {sub} --all")
}

/// A verb `job` doesn't have (with a name).
pub fn unknown_verb(sub: &str) -> String {
    format!("usage: unknown {CLI} job verb '{sub}' ({CLI} job --help)")
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    /// The verb, as typed.
    pub sub: String,
    pub json: bool,
    pub all: bool,
    pub every: String,
    pub cron: String,
    pub timeout: String,
    pub cwd: String,
    pub have_cwd: bool,
    /// `--workspace`; empty for none.
    pub workspace: String,
    pub run: String,
    /// The first word that is no flag; empty for none.
    pub name: String,
    /// `--` was given: the words after it.
    pub argv: Option<Vec<String>>,
}

/// What `cmd_job` does with its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// `_job_usage`: the help, exit 0.
    Help,
    /// A call to make.
    Call(Ask),
    /// A `usage:` line, exit 2, before the body and the query.
    Early(String),
}

/// `cmd_job`'s words (after `job`): the help, the flags, then the checks
/// that come before any request. A usage error found after the body and
/// the query are made (a missing name, an unknown verb) is left to
/// [`check`].
pub fn parse(words: &[&str]) -> Parsed {
    let sub = words.first().copied().unwrap_or("");
    if matches!(sub, "" | "help" | "--help" | "-h") {
        return Parsed::Help;
    }
    let rest = &words[1..];
    for a in rest {
        if *a == "--" {
            break;
        }
        if matches!(*a, "-h" | "--help") {
            return Parsed::Help;
        }
    }
    let mut ask = Ask {
        sub: sub.to_string(),
        ..Ask::default()
    };
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        match a {
            "--" => {
                ask.argv = Some(rest[i + 1..].iter().map(|w| w.to_string()).collect());
                break;
            }
            "--json" => ask.json = true,
            "--all" => ask.all = true,
            "--every" | "--cron" | "--timeout" | "--cwd" | "--workspace" | "--run" => {
                let value = rest.get(i + 1).copied().unwrap_or("");
                if value.is_empty() {
                    return Parsed::Early(needs_value(a));
                }
                let v = value.to_string();
                match a {
                    "--every" => ask.every = v,
                    "--cron" => ask.cron = v,
                    "--timeout" => ask.timeout = v,
                    "--cwd" => {
                        ask.cwd = v;
                        ask.have_cwd = true;
                    }
                    "--workspace" => ask.workspace = v,
                    _ => ask.run = v,
                }
                i += 1;
            }
            _ if a.starts_with("--") => return Parsed::Early(unknown_flag(a)),
            _ => pos.push(a),
        }
        i += 1;
    }
    ask.name = pos.first().map_or_else(String::new, |w| w.to_string());
    Parsed::Call(ask)
}

/// Which request a verb makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Add,
    Edit,
    List,
    Show,
    Logs,
    /// A POST whose route is the given word, answered by the last
    /// Python ([`done_lines`]).
    Post(&'static str),
}

impl Verb {
    /// The route under [`ROUTE`].
    pub fn route(self) -> &'static str {
        match self {
            Verb::Add => "add",
            Verb::Edit => "edit",
            Verb::List => "list",
            Verb::Show => "show",
            Verb::Logs => "logs",
            Verb::Post(r) => r,
        }
    }

    /// A GET with the query; else a POST with the body.
    pub fn get(self) -> bool {
        matches!(self, Verb::List | Verb::Show | Verb::Logs)
    }
}

/// The checks after the body and the query, in the shell's order: the
/// name a verb needs, then the verb's own. `Err`: the `usage:` line,
/// exit 2.
pub fn check(ask: &Ask) -> Result<Verb, String> {
    let sub = ask.sub.as_str();
    let need_name = !matches!(sub, "list" | "ls" | "pause" | "resume");
    if need_name && ask.name.is_empty() {
        return Err(needs_name(sub));
    }
    Ok(match sub {
        "add" => {
            if !ask.argv.as_ref().is_some_and(|a| !a.is_empty()) {
                return Err(ADD_USAGE.to_string());
            }
            Verb::Add
        }
        "edit" => Verb::Edit,
        "list" | "ls" => Verb::List,
        "show" => Verb::Show,
        "logs" | "log" => Verb::Logs,
        "run" | "run-now" => Verb::Post("run"),
        "enable" => Verb::Post("enable"),
        "disable" => Verb::Post("disable"),
        "kill" => Verb::Post("kill"),
        "rm" | "remove" | "delete" => Verb::Post("remove"),
        "pause" | "resume" => {
            if !ask.all && ask.name.is_empty() {
                return Err(pause_usage(sub));
            }
            Verb::Post(if sub == "pause" { "pause" } else { "resume" })
        }
        _ => return Err(unknown_verb(sub)),
    })
}

impl Ask {
    /// The workspace sent: `--workspace`, else the project when there is
    /// no passport, else none.
    pub fn workspace_for(&self, project: &str, passport: bool) -> String {
        if self.workspace.is_empty() && !passport {
            project.to_string()
        } else {
            self.workspace.clone()
        }
    }

    /// The shell's body, `json.dumps(o)`: each of `workspace`, `name`,
    /// `every`, `cron`, `timeout` and `cwd` that is not empty, in that
    /// order, then `all: true` with `--all` and `argv` after `--`.
    pub fn body(&self, workspace: &str) -> String {
        let mut o: Vec<(Text, Value)> = Vec::new();
        for (k, v) in [
            ("workspace", workspace),
            ("name", self.name.as_str()),
            ("every", self.every.as_str()),
            ("cron", self.cron.as_str()),
            ("timeout", self.timeout.as_str()),
            ("cwd", self.cwd.as_str()),
        ] {
            if !v.is_empty() {
                o.push((Text::from(k), s(v)));
            }
        }
        if self.all {
            o.push((Text::from("all"), Value::Bool(true)));
        }
        if let Some(argv) = &self.argv {
            o.push((
                Text::from("argv"),
                Value::Array(argv.iter().map(|w| s(w)).collect()),
            ));
        }
        // Every value is a string from a UTF-8 word: nothing to refuse.
        json::to_string(&Value::Object(o), &Options::default()).unwrap_or_default()
    }

    /// The shell's query, `urllib.parse.urlencode(o)`: each of
    /// `workspace`, `name` and `run` that is not empty, then `all=1`.
    pub fn query(&self, workspace: &str) -> String {
        let mut q: Vec<String> = Vec::new();
        for (k, v) in [
            ("workspace", workspace),
            ("name", self.name.as_str()),
            ("run", self.run.as_str()),
        ] {
            if !v.is_empty() {
                q.push(format!("{k}={}", quote_plus(v)));
            }
        }
        if self.all {
            q.push("all=1".into());
        }
        q.join("&")
    }
}

/// `urllib.parse.quote_plus(s)`: [`quote`], with a space as `+`.
pub fn quote_plus(text: &str) -> String {
    quote(text).replace("%20", "+")
}

/// Whether a command line skips the virtual desktop's guard: `-h` or
/// `--help` anywhere in it (`_k2_offline_verb`).
fn offline_verb(words: &[&str]) -> bool {
    words.iter().any(|w| matches!(*w, "-h" | "--help"))
}

/// `k5 job ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let all: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    let parsed = parse(all.get(1..).unwrap_or_default());
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
    let help = parsed == Parsed::Help;
    let gate = prelude::gate(&settings);
    if !help && gate.is_none() && settings.port_text.as_deref().and_then(curl_port).is_none() {
        return None;
    }
    Some(run_with(
        args,
        &parsed,
        offline_verb(&all),
        hook_sock.as_deref(),
        settings,
    ))
}

fn run_with(
    args: &[OsString],
    parsed: &Parsed,
    offline: bool,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    // The guard, unless `-h` or `--help` is anywhere; then the connection
    // gate, unless this is the help.
    match prelude::gate(&settings) {
        Some(Gate::VirtualDesktop) if !offline => return Gate::VirtualDesktop.stop(),
        Some(_) if *parsed != Parsed::Help => return Gate::CannotConnect.stop(),
        _ => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let ask = match parsed {
        Parsed::Help => return Shown::out(crate::text::help("job").unwrap_or_default()).print(),
        Parsed::Early(line) => {
            out::stderr_line(line);
            return Exit::Usage;
        }
        Parsed::Call(ask) => ask,
    };
    let verb = match check(ask) {
        Ok(v) => v,
        Err(line) => {
            out::stderr_line(&line);
            return Exit::Usage;
        }
    };
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let passport = matches!(settings.auth, crate::transport::Auth::Passport { .. });
    let workspace = ask.workspace_for(&settings.project, passport);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let path = format!("{ROUTE}{}", verb.route());
    let answer = if verb.get() {
        client.sidecar_get(&path, &ask.query(&workspace))
    } else {
        client.sidecar_post(&path, &ask.body(&workspace))
    };
    let (status, body) = match held(answer) {
        Ok(held) => held,
        Err(line) => {
            out::stderr_line(&line);
            return Exit::Error;
        }
    };
    if status != "200" {
        if status == "000" {
            out::stderr_line(UNREACHABLE);
            return Exit::Error;
        }
        let shown = Shown::err(fail_line(&body), exit_for(&status));
        return shown.print();
    }
    if ask.json {
        let mut text = body;
        text.push(b'\n');
        return match out::write_flushed(&mut std::io::stdout().lock(), &text) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        };
    }
    render(verb, &ask.sub, &body).print()
}

/// `_sidecar_split` on what `_sidecar_send` printed: the status (`000`
/// for none) and the body, NULs dropped. `Err`: a line the shell prints
/// from inside the send before it ends the command with 1 (a POST that
/// may have been applied, or a session with no passport).
pub fn held(r: Result<Response, Error>) -> Result<(String, Vec<u8>), String> {
    match r {
        Ok(r) => Ok((format!("{:03}", r.status), without_nuls(&r.body))),
        Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
            Err(e.to_string())
        }
        Err(_) => Ok(("000".into(), Vec::new())),
    }
}

/// `_job_exit_for`.
pub fn exit_for(status: &str) -> Exit {
    match status {
        "400" | "413" => Exit::Usage,
        "401" | "403" => Exit::Refused,
        "404" => Exit::NotFound,
        _ => Exit::Error,
    }
}

/// `_job_call`'s Python on a refusal's body: `code: hint` from an
/// `error` object, `str(error)` for any other error, else the first 300
/// characters of the body (or a line saying there was none) when it is
/// not a JSON object. Each with its newline.
pub fn fail_line(body: &[u8]) -> String {
    let raw = String::from_utf8_lossy(body);
    let from_json = match json::parse(&raw) {
        Ok(Value::Object(pairs)) => {
            let e = get(&pairs, "error")
                .filter(|e| py::truthy(e))
                .cloned()
                .unwrap_or(Value::Object(Vec::new()));
            Some(match &e {
                Value::Object(e) => format!(
                    "{}: {}",
                    get(e, "code").map_or_else(|| "error".into(), py::str_of),
                    get(e, "hint").map_or_else(String::new, py::str_of)
                ),
                other => py::str_of(other),
            })
        }
        _ => None,
    };
    let line = from_json.unwrap_or_else(|| {
        let head: String = raw.chars().take(300).collect();
        if head.is_empty() {
            "the daemon did not answer".into()
        } else {
            head
        }
    });
    format!("{line}\n")
}

/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    /// Each reader's `json.load` (the last one's is a line lower).
    pub const LOAD: u32 = 3;
    pub const L_JOBS: u32 = 4;
    pub const L_PAUSED: u32 = 6;
    pub const L_FOR: u32 = 11;
    pub const L_STATE: u32 = 12;
    pub const L_LAST: u32 = 13;
    pub const L_LR: u32 = 14;
    pub const L_NAME: u32 = 15;
    pub const L_ALL: u32 = 16;
    pub const L_CMD: u32 = 18;
    pub const L_LEN: u32 = 19;
    pub const L_CUT: u32 = 20;
    pub const L_ROW: u32 = 21;
    pub const S_JOB: u32 = 3;
    pub const S_STATE: u32 = 4;
    pub const S_CHANGED: u32 = 13;
    pub const S_GRANTS: u32 = 15;
    pub const S_SECRETS: u32 = 16;
    pub const S_RUNS: u32 = 17;
    pub const S_FOR: u32 = 21;
    pub const S_EC: u32 = 22;
    pub const G_RUN: u32 = 4;
    pub const G_EC: u32 = 5;
    pub const G_WRITE: u32 = 7;
    pub const D_LOAD: u32 = 4;
    pub const D_JOB: u32 = 5;
    pub const D_ADD: u32 = 7;
    pub const D_EDIT: u32 = 10;
}

/// The shell's Python on a 200's body, for the verb; `sub` is the verb
/// as typed (the last reader reads it).
pub fn render(verb: Verb, sub: &str, body: &[u8]) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let load = match verb {
        Verb::List | Verb::Show | Verb::Logs => line::LOAD,
        _ => line::D_LOAD,
    };
    let v = match json::parse(&raw) {
        Ok(v) => v,
        Err(e) => {
            let why = if raw.trim_matches(py_space).is_empty() {
                "Expecting value: line 1 column 1 (char 0)".to_string()
            } else {
                e.to_string()
            };
            return raised(
                String::new(),
                String::new(),
                &Raise::at(load, format!("json.decoder.JSONDecodeError: {why}")),
            );
        }
    };
    let mut o = String::new();
    let r = match verb {
        Verb::List => list_lines(&v, &mut o),
        Verb::Show => show_lines(&v, &mut o),
        Verb::Logs => logs_lines(&v, &mut o),
        _ => done_lines(sub, &v, &mut o),
    };
    match r {
        Ok(()) => Shown::out(o),
        Err(r) => raised(o, String::new(), &r),
    }
}

/// `x or default`.
fn or(v: Option<&Value>, default: Value) -> Value {
    v.filter(|x| py::truthy(x)).cloned().unwrap_or(default)
}

/// `str(d.get(key))` where `d` is a dict for sure: `None` for a missing
/// key.
fn str_at(d: &Value, key: &str) -> String {
    match d {
        Value::Object(pairs) => get(pairs, key).map_or_else(|| "None".into(), py::str_of),
        _ => "None".into(),
    }
}

/// `d.get(key, default)` where `d` is a dict for sure.
fn at_or(d: &Value, key: &str, default: Value) -> Value {
    match d {
        Value::Object(pairs) => get(pairs, key).cloned().unwrap_or(default),
        _ => default,
    }
}

fn push_line(o: &mut String, text: &str) {
    o.push_str(text);
    o.push('\n');
}

/// What a set holds a value as: Python's equal values are one entry
/// (`1`, `1.0` and `True`), and a dict or a list can't be in one.
fn set_key(v: &Value, at: u32) -> Result<String, Raise> {
    Ok(match v {
        Value::Null => "n".into(),
        Value::Str(t) => format!("s{}", py::str_of(&Value::Str(t.clone()))),
        Value::Bool(b) => format!("#{}", u8::from(*b)),
        Value::Int(d) => format!("#{d}"),
        Value::Float(f) if f.is_nan() => "nan".into(),
        Value::Float(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => format!("#{}", *f as i64),
        Value::Float(f) => format!("f{}", f.to_bits()),
        other => {
            return Err(Raise::at(
                at,
                format!("TypeError: unhashable type: '{}'", py::type_name(other)),
            ))
        }
    })
}

/// Python's `a == b` on two JSON values (`None` for a missing key).
fn py_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    let a = a.unwrap_or(&Value::Null);
    let b = b.unwrap_or(&Value::Null);
    let num = |v: &Value| -> Option<f64> {
        match v {
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            Value::Int(d) => d.parse().ok(),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    };
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(Some(p), Some(q)))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.iter().any(|(k2, v2)| k == k2 && py_eq(Some(v), Some(v2))))
        }
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Null, Value::Null) => true,
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

/// `x[5:16]` on a value Python must slice.
fn slice_5_16(v: &Value, at: u32) -> Result<Value, Raise> {
    match v {
        Value::Str(t) => {
            let cps = t.code_points();
            let part: Vec<Value> = cps
                .into_iter()
                .skip(5)
                .take(11)
                .map(|cp| Value::Str(super::code_point(cp)))
                .collect();
            let text: String = part.iter().map(py::str_of).collect();
            Ok(s(&text))
        }
        Value::Array(items) => Ok(Value::Array(
            items.iter().skip(5).take(11).cloned().collect(),
        )),
        Value::Object(_) => Err(Raise::at(at, "KeyError: slice(5, 16, None)")),
        other => Err(Raise::at(
            at,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// `_job_render_list`.
fn list_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    let jobs = or(py_get(v, "jobs", line::L_JOBS)?, Value::Array(Vec::new()));
    if py::truthy(&at_or(v, "paused", Value::Null)) {
        let by = or(py_get(v, "pausedBy", line::L_PAUSED)?, s("?"));
        push_line(
            o,
            &format!(
                "ALL JOBS PAUSED by {} ({CLI} job resume --all)",
                py::str_of(&by)
            ),
        );
    }
    if !py::truthy(&jobs) {
        push_line(
            o,
            &format!("no jobs ({CLI} job add NAME --every 15m -- <cmd>)"),
        );
        return Ok(());
    }
    let row = |c: [&str; 5]| {
        format!(
            "{} {} {} {} {}",
            ljust(c[0], 24),
            ljust(c[1], 22),
            ljust(c[2], 8),
            ljust(c[3], 28),
            c[4]
        )
    };
    push_line(
        o,
        &row(["NAME", "SCHEDULE", "STATE", "LAST RUN", "COMMAND"]),
    );
    let items = py_iter(&jobs, line::L_FOR)?;
    let every = py::truthy(&at_or(v, "all", Value::Null));
    // The set of workspaces is the same on every pass: made once, when
    // first asked for.
    let mut many: Option<bool> = None;
    for j in &items {
        let running = py_get(j, "running", line::L_STATE)?.is_some_and(py::truthy);
        let state = if running {
            "running"
        } else if py::truthy(&at_or(j, "enabled", Value::Null)) {
            "on"
        } else {
            "off"
        };
        let last = or(
            py_get(j, "lastRun", line::L_LAST)?,
            Value::Object(Vec::new()),
        );
        let lr = if py::truthy(&last) {
            let outcome = py_get(&last, "outcome", line::L_LR)?
                .cloned()
                .unwrap_or_else(|| s(""));
            let started = or(Some(&at_or(&last, "startedAt", Value::Null)), s(""));
            let part = slice_5_16(&started, line::L_LR)?;
            format!("{} {}", py::str_of(&outcome), py::str_of(&part))
        } else {
            "-".into()
        };
        let mut name = at_or(j, "name", s(""));
        let prefixed = if every {
            true
        } else {
            match many {
                Some(m) => m,
                None => {
                    let mut seen = std::collections::BTreeSet::new();
                    for x in &items {
                        let w = py_get(x, "workspace", line::L_ALL)?;
                        seen.insert(set_key(w.unwrap_or(&Value::Null), line::L_ALL)?);
                    }
                    let m = seen.len() > 1;
                    many = Some(m);
                    m
                }
            }
        };
        if prefixed {
            name = s(&format!("{}/{}", str_at(j, "workspace"), py::str_of(&name)));
        }
        let mut cmd = at_or(j, "command", s(""));
        if py_len(&cmd, line::L_LEN)? > 60 {
            cmd = match &cmd {
                Value::Str(t) => {
                    let head: String = t
                        .code_points()
                        .into_iter()
                        .take(57)
                        .map(|cp| py::str_of(&Value::Str(super::code_point(cp))))
                        .collect();
                    s(&format!("{head}..."))
                }
                Value::Array(_) => {
                    return Err(Raise::at(
                        line::L_CUT,
                        "TypeError: can only concatenate list (not \"str\") to list",
                    ))
                }
                _ => return Err(Raise::at(line::L_CUT, "KeyError: slice(None, 57, None)")),
            };
        }
        let schedule = at_or(j, "schedule", s(""));
        push_line(
            o,
            &format!(
                "{} {} {} {} {}",
                py_left(&name, 24),
                py_left(&schedule, 22),
                ljust(state, 8),
                ljust(&lr, 28),
                py::str_of(&cmd)
            ),
        );
    }
    Ok(())
}

/// `_job_render_show`.
fn show_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    let j = or(py_get(v, "job", line::S_JOB)?, Value::Object(Vec::new()));
    let running = py_get(&j, "running", line::S_STATE)?
        .cloned()
        .unwrap_or(Value::Null);
    let state = if py::truthy(&running) {
        format!("running (run {})", py::str_of(&running))
    } else if py::truthy(&at_or(&j, "enabled", Value::Null)) {
        "on".into()
    } else {
        "off".into()
    };
    push_line(
        o,
        &format!(
            "job:        {} (workspace {})",
            str_at(&j, "name"),
            str_at(&j, "workspace")
        ),
    );
    push_line(o, &format!("schedule:   {}", str_at(&j, "schedule")));
    push_line(o, &format!("state:      {state}"));
    let next = or(Some(&at_or(&j, "next", Value::Null)), s("-"));
    push_line(o, &format!("next:       {}", py::str_of(&next)));
    push_line(o, &format!("command:    {}", str_at(&j, "command")));
    let cwd = or(
        Some(&at_or(&j, "cwd", Value::Null)),
        s("(the workspace folder)"),
    );
    push_line(o, &format!("folder:     {}", py::str_of(&cwd)));
    push_line(o, &format!("timeout:    {}s", str_at(&j, "timeoutSecs")));
    push_line(
        o,
        &format!(
            "made by:    {} at {}",
            str_at(&j, "createdBy"),
            str_at(&j, "createdAt")
        ),
    );
    let field = |k: &str| match &j {
        Value::Object(pairs) => get(pairs, k).cloned(),
        _ => None,
    };
    if field("updatedBy").is_some_and(|x| py::truthy(&x))
        && !py_eq(field("updatedAt").as_ref(), field("createdAt").as_ref())
    {
        push_line(
            o,
            &format!(
                "changed by: {} at {}",
                str_at(&j, "updatedBy"),
                str_at(&j, "updatedAt")
            ),
        );
    }
    let g = or(field("grants").as_ref(), Value::Array(Vec::new()));
    let secrets = if py::truthy(&g) {
        let mut parts = Vec::new();
        for x in py_iter(&g, line::S_SECRETS)? {
            let inside =
                |e: String| Raise::inside(line::S_SECRETS, line::S_SECRETS, "<genexpr>", e);
            let Value::Object(pairs) = &x else {
                return Err(inside(format!(
                    "AttributeError: '{}' object has no attribute 'get'",
                    py::type_name(&x)
                )));
            };
            let env = match get(pairs, "envName").filter(|e| py::truthy(e)) {
                None => String::new(),
                Some(Value::Str(t)) => format!(" as ${}", py::str_of(&Value::Str(t.clone()))),
                Some(other) => {
                    return Err(inside(format!(
                        "TypeError: can only concatenate str (not \"{}\") to str",
                        py::type_name(other)
                    )))
                }
            };
            parts.push(format!(
                "{} ({}{env})",
                get(pairs, "secret").map_or_else(|| "None".into(), py::str_of),
                get(pairs, "delivery").map_or_else(|| "None".into(), py::str_of)
            ));
        }
        parts.join(", ")
    } else {
        format!(
            "none granted ({CLI} vault grant KEY --to job:{})",
            str_at(&j, "name")
        )
    };
    push_line(o, &format!("secrets:    {secrets}"));
    let runs = or(field("runs").as_ref(), Value::Array(Vec::new()));
    if py::truthy(&runs) {
        push_line(o, "");
        let row = |c: [&str; 5]| {
            format!(
                "{} {} {} {} {}",
                ljust(c[0], 38),
                ljust(c[1], 16),
                ljust(c[2], 10),
                ljust(c[3], 6),
                c[4]
            )
        };
        push_line(o, &row(["RUN", "OUTCOME", "TRIGGER", "EXIT", "STARTED"]));
        for r in py_iter(&runs, line::S_FOR)? {
            let ec = py_get(&r, "exitCode", line::S_EC)?;
            let ec = match ec {
                None | Some(Value::Null) => "-".to_string(),
                Some(x) => py::str_of(x),
            };
            push_line(
                o,
                &row([
                    &str_at(&r, "id"),
                    &str_at(&r, "outcome"),
                    &str_at(&r, "trigger"),
                    &ec,
                    &str_at(&r, "startedAt"),
                ]),
            );
        }
    }
    Ok(())
}

/// `logs`' Python.
fn logs_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    let r = or(py_get(v, "run", line::G_RUN)?, Value::Object(Vec::new()));
    let ec = match py_get(&r, "exitCode", line::G_EC)? {
        None | Some(Value::Null) => "-".to_string(),
        Some(x) => py::str_of(x),
    };
    let tail = if py::truthy(&at_or(v, "truncated", Value::Null)) {
        "  (tail)"
    } else {
        ""
    };
    push_line(
        o,
        &format!(
            "# run {}  {}  {}  exit {ec}  {} bytes{tail}",
            str_at(&r, "id"),
            str_at(&r, "startedAt"),
            str_at(&r, "outcome"),
            str_at(&r, "bytes")
        ),
    );
    match or(Some(&at_or(v, "log", Value::Null)), s("")) {
        Value::Str(t) => o.push_str(&py::str_of(&Value::Str(t))),
        other => {
            return Err(Raise::at(
                line::G_WRITE,
                format!(
                    "TypeError: write() argument must be str, not {}",
                    py::type_name(&other)
                ),
            ))
        }
    }
    Ok(())
}

/// The last Python, after a POST: what changed, by the verb as typed.
fn done_lines(sub: &str, v: &Value, o: &mut String) -> Result<(), Raise> {
    let job = py_get(v, "job", line::D_JOB)?;
    let j = job.filter(|x| matches!(x, Value::Object(_)));
    let none = |at: u32| {
        Raise::at(
            at,
            "AttributeError: 'NoneType' object has no attribute 'get'",
        )
    };
    match sub {
        "add" => {
            let j = j.ok_or_else(|| none(line::D_ADD))?;
            push_line(
                o,
                &format!(
                    "added {}: {}, runs {}",
                    str_at(j, "name"),
                    str_at(j, "schedule"),
                    str_at(j, "command")
                ),
            );
            let next = or(Some(&at_or(j, "next", Value::Null)), s("-"));
            push_line(
                o,
                &format!(
                    "next run: {}. Grant it secrets with {CLI} vault grant KEY --to job:{}.",
                    py::str_of(&next),
                    str_at(j, "name")
                ),
            );
        }
        "edit" => {
            let j = j.ok_or_else(|| none(line::D_EDIT))?;
            push_line(
                o,
                &format!(
                    "changed {}: {}, runs {}",
                    str_at(j, "name"),
                    str_at(j, "schedule"),
                    str_at(j, "command")
                ),
            );
        }
        "run" | "run-now" => {
            let (job, run) = (str_at(v, "job"), str_at(v, "run"));
            push_line(
                o,
                &format!("started {job}, run {run} ({CLI} job logs {job} --run {run})"),
            );
        }
        "kill" => push_line(
            o,
            &format!("stopping {}, run {}", str_at(v, "job"), str_at(v, "run")),
        ),
        "rm" | "remove" | "delete" => push_line(
            o,
            &format!(
                "removed {} ({} secret grant(s) removed)",
                str_at(v, "removed"),
                str_at(v, "grantsRevoked")
            ),
        ),
        _ => {
            if let Some(j) = j {
                let on = if py::truthy(&at_or(j, "enabled", Value::Null)) {
                    "on"
                } else {
                    "off"
                };
                push_line(o, &format!("{} is {on}", str_at(j, "name")));
            } else if let Value::Object(pairs) = v {
                if pairs.iter().any(|(k, _)| k.as_str() == Some("paused")) {
                    let what = if py::truthy(&at_or(v, "paused", Value::Null)) {
                        "paused"
                    } else {
                        "running again"
                    };
                    push_line(o, &format!("every job is {what}"));
                } else {
                    push_line(o, "ok");
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
