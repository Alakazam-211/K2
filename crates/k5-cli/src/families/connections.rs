//! `k5 connections list [--json] [--users]` (also `connections --json` and
//! `connections --users`), as the shell CLI's `cmd_connections` answers
//! its `list` (`cli/src/usage.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    the 0.40.4 rename line, and the two gates ([`crate::prelude`]).
//! 2. `connections` is an `id` tool, so with `HOOK_SOCK` set: the
//!    stale-passport or require-passport lines, exit 3
//!    ([`crate::prelude::id_tool`]).
//! 3. The words ([`parse`]): `--json` and `--users`; any other flag is
//!    `Error: unknown flag: <it>`, any other word `Error: unexpected
//!    argument: <it>`, each with the usage line, exit 2.
//! 4. One `raw=$(cli_send cli_request /cli/connections action=list
//!    [users=1])`: the cell socket first when there is a passport with a
//!    live socket, then TCP. `$(…)` drops the answer's NULs and trailing
//!    newlines ([`held`]). A call that failed ends the command there with
//!    its code (`set -e`); one with no answer is the transport's line and
//!    exit 1.
//! 5. The Python on `raw` and a newline (`<<< "$raw"`, [`reader`]): the
//!    daemon's error (an object as JSON on stderr, else `Error: <it>` on
//!    stdout), exit 1; with `--json` one line of compact JSON; else who is
//!    working in this workspace, the connections, and the people on this
//!    host with `--users`.
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The route table sends `connections list`, `connections --json` and
//! `connections --users` here (a verb row, [`crate::route`]), and `add`
//! (its own verb row, answered by [`write`]): every other action of
//! `cmd_connections`, `connections` alone and its help stay the shell's. So does a command
//! line with `-h` or `--help` ([`crate::front`]), a word that is not
//! UTF-8, `HOOK_SOCK` set with a `CODEX_` variable exported, a port that
//! is set but is no port curl takes, and a `python3` the binary can't
//! tell, as for [`super::inbox`].
//!
//! Where the shell's Python raises (an answer that is not an object, a
//! field of a type it doesn't expect), this prints a Python 3.12
//! traceback. Bytes that are not UTF-8 and half of a surrogate pair are as
//! [`super::inbox`] says.

use super::{
    curl_port, get, py_get, py_iter, py_len, py_space, raised, s, without_nuls, Raise, Shown,
};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::{Call, Error};
use std::ffi::OsString;
use std::path::Path;

/// The route.
pub const ENDPOINT: &str = "/cli/connections";
/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "connections";
/// After a word `list` doesn't take: exit 2.
pub const USAGE: &str = "Usage: k2 connections list [--json] [--users]";
/// The Python on an answer that is not JSON, before its `%r`.
pub const UNPARSEABLE: &str = "Error: daemon returned an unparseable response: ";
/// `--users` and an answer with no list of them.
pub const NO_USERS: &str = "Error: daemon omitted users[] (asked with users=1)";
/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    pub const ERR: u32 = 10;
    pub const J_WS: u32 = 24;
    pub const J_FOR: u32 = 30;
    pub const J_C: u32 = 31;
    pub const LABEL: u32 = 76;
    pub const TW_LEN: u32 = 84;
    pub const TW_ROW: u32 = 86;
    pub const FOR: u32 = 97;
    pub const C: u32 = 98;
    pub const DISPLAY: u32 = 107;
}

/// What the words ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ask {
    pub json: bool,
    pub users: bool,
}

/// The words after `connections` (its verb row: `list`, or `--json` or
/// `--users` first, which the shell reads as `list` and them): the ask, or
/// the two lines that stop it (exit 2). `None`: `-h` or `--help`, the
/// shell's.
pub fn parse(words: &[&str]) -> Option<Result<Ask, [String; 2]>> {
    let rest = match words.first() {
        Some(&"list") => &words[1..],
        Some(&("--json" | "--users")) => words,
        _ => return None,
    };
    let mut ask = Ask::default();
    for w in rest {
        match *w {
            "--json" => ask.json = true,
            "--users" => ask.users = true,
            "--help" | "-h" => return None,
            w if w.starts_with('-') => {
                return Some(Err([format!("Error: unknown flag: {w}"), USAGE.into()]))
            }
            w => {
                return Some(Err([
                    format!("Error: unexpected argument: {w}"),
                    USAGE.into(),
                ]))
            }
        }
    }
    Some(Ok(ask))
}

/// What the words ask for: `list`'s ask (or the lines that stop it), or
/// a write verb and the words after it.
enum Form<'a> {
    List(Result<Ask, [String; 2]>),
    Write(write::Verb, Vec<&'a str>),
}

/// `k5 connections list ...`, `add <name>` and `remove <name>`: `args` is
/// the whole command line after `argv[0]`. `None`: the shell CLI answers
/// it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let form = match words.first().and_then(|w| write::Verb::of(w)) {
        Some(verb) => Form::Write(verb, words[1..].to_vec()),
        None => Form::List(parse(&words)?),
    };
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
    Some(run_with(args, form, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    form: Form<'_>,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    if let Some(notice) = prelude::rename_notice(Path::new(&settings.project)) {
        out::stderr_line(notice);
    }
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let (ask, write) = match form {
        Form::List(Ok(a)) => (a, None),
        Form::List(Err(lines)) => {
            for l in lines {
                out::stderr_line(&l);
            }
            return Exit::Usage;
        }
        Form::Write(verb, rest) => match write::target(verb, &rest) {
            Ok(t) => (Ask::default(), Some((verb, t))),
            Err(_) => return write::usage(verb),
        },
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
    if let Some((verb, target)) = write {
        return write::send(&mut client, verb, target);
    }
    let mut closing = Closing::default();
    let passport = client.passport().is_some();
    let call: &[&str] = if ask.users {
        &["action=list", "users=1"]
    } else {
        &["action=list"]
    };
    let exit = match client.send(ENDPOINT, Call::Request(call)) {
        Ok(r) => {
            closing.answer(&r, passport);
            reader(&held(&r.body), ask).print()
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            Exit::Error
        }
        Err(e) => Exit::from_curl(e.curl_code()),
    };
    closing.flush(&mut client, exit);
    exit
}

/// What `raw=$(…)` holds of an answer: NULs and trailing newlines gone.
pub fn held(body: &[u8]) -> Vec<u8> {
    let mut b = without_nuls(body);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    b
}

/// `x or default`.
fn or(v: Option<&Value>, default: Value) -> Value {
    v.filter(|x| py::truthy(x)).cloned().unwrap_or(default)
}

/// `d.get(key)` where `d` is a dict for sure.
fn at<'a>(d: &'a Value, key: &str) -> Option<&'a Value> {
    match d {
        Value::Object(pairs) => get(pairs, key),
        _ => None,
    }
}

fn truthy_at(d: &Value, key: &str) -> bool {
    at(d, key).is_some_and(py::truthy)
}

/// The shell's Python on `raw` (what `$(…)` held), with the newline
/// `<<<` adds.
pub fn reader(raw: &[u8], ask: Ask) -> Shown {
    let mut text = String::from_utf8_lossy(raw).into_owned();
    text.push('\n');
    let Ok(d) = json::parse(&text) else {
        let head: String = text.chars().take(200).collect();
        return Shown {
            stdout: format!("{UNPARSEABLE}{}\n", py::repr(&Value::Str(Text::from(head)))),
            stderr: String::new(),
            exit: Exit::Error,
        };
    };
    let mut o = Out::default();
    let r = match py_get(&d, "error", line::ERR) {
        Err(r) => Err(r),
        Ok(Some(err)) if *err != Value::Null => {
            if let Value::Object(_) = err {
                let shape = Value::Object(vec![(Text::from("error"), err.clone())]);
                let plain = Options {
                    ensure_ascii: false,
                    ..Options::default()
                };
                let line = json::to_string(&shape, &plain)
                    .or_else(|_| json::to_string(&shape, &Options::default()))
                    .unwrap_or_default();
                o.stderr.push_str(&line);
                o.stderr.push('\n');
            } else {
                o.line(&format!("Error: {}", py::str_of(err)));
            }
            Ok(Exit::Error)
        }
        Ok(_) if ask.json => json_lines(&d, ask, &mut o),
        Ok(_) => text_lines(&d, ask, &mut o),
    };
    match r {
        Ok(exit) => Shown {
            stdout: o.stdout,
            stderr: o.stderr,
            exit,
        },
        Err(r) => raised(o.stdout, o.stderr, &r),
    }
}

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
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `c.get(key)`, `None` when missing.
fn get_or_null(c: &Value, key: &str) -> Value {
    at(c, key).cloned().unwrap_or(Value::Null)
}

/// `c.get('address') or ('%s::%s' % (c.get('agent', '?'), c.get('host', '?')))`.
fn address(c: &Value) -> Value {
    or(
        at(c, "address"),
        s(&format!(
            "{}::{}",
            at(c, "agent").map_or_else(|| "?".into(), py::str_of),
            at(c, "host").map_or_else(|| "?".into(), py::str_of)
        )),
    )
}

/// `ws` and `conns`: `d.get('workspace') or {}`, `d.get('connections', []) or []`.
fn ws_and_conns(d: &Value) -> (Value, Value) {
    (
        or(at(d, "workspace"), Value::Object(Vec::new())),
        or(at(d, "connections"), Value::Array(Vec::new())),
    )
}

/// `if as_json:`.
fn json_lines(d: &Value, ask: Ask, o: &mut Out) -> Result<Exit, Raise> {
    let (ws, conns) = ws_and_conns(d);
    let name = or(py_get(&ws, "name", line::J_WS)?, s(""));
    let path = or(at(&ws, "path"), s(""));
    let id = or(
        at(&ws, "id")
            .filter(|x| py::truthy(x))
            .or(at(&ws, "projectId")),
        s(""),
    );
    let mut rows = Vec::new();
    for c in py_iter(&conns, line::J_FOR)? {
        if py_get(&c, "remote", line::J_C)?.is_some_and(py::truthy) {
            let addr = address(&c);
            rows.push(obj(vec![
                ("name", addr.clone()),
                ("kind", s("remote")),
                ("address", addr),
                ("agent", get_or_null(&c, "agent")),
                ("host", get_or_null(&c, "host")),
                ("remoteWorkspaceId", get_or_null(&c, "remoteWorkspaceId")),
                ("handle", get_or_null(&c, "handle")),
                ("displayName", get_or_null(&c, "displayName")),
                (
                    "remoteWorkspaceGone",
                    Value::Bool(truthy_at(&c, "remoteWorkspaceGone")),
                ),
            ]));
        } else {
            rows.push(obj(vec![
                (
                    "name",
                    or(
                        at(&c, "projectName")
                            .filter(|x| py::truthy(x))
                            .or(at(&c, "name")),
                        s("?"),
                    ),
                ),
                ("kind", s("local")),
                (
                    "projectId",
                    at(&c, "projectId")
                        .filter(|x| py::truthy(x))
                        .or(at(&c, "id"))
                        .cloned()
                        .unwrap_or(Value::Null),
                ),
                (
                    "reachable",
                    at(&c, "reachable").cloned().unwrap_or(Value::Bool(true)),
                ),
            ]));
        }
    }
    let mut out = vec![
        ("ok", Value::Bool(true)),
        (
            "workspace",
            obj(vec![("name", name), ("path", path), ("id", id)]),
        ),
        ("connections", Value::Array(rows)),
    ];
    if ask.users {
        let Some(Value::Array(users)) = at(d, "users") else {
            o.line(NO_USERS);
            return Ok(Exit::Error);
        };
        let users = users
            .iter()
            .filter(|u| matches!(u, Value::Object(_)))
            .map(|u| {
                obj(vec![
                    ("username", or(at(u, "username"), s(""))),
                    ("role", or(at(u, "role"), s(""))),
                    ("disabled", Value::Bool(truthy_at(u, "disabled"))),
                    ("firstName", get_or_null(u, "firstName")),
                    ("lastName", get_or_null(u, "lastName")),
                    (
                        "displayName",
                        or(
                            at(u, "displayName")
                                .filter(|x| py::truthy(x))
                                .or(at(u, "username")),
                            s(""),
                        ),
                    ),
                ])
            })
            .collect();
        out.push(("users", Value::Array(users)));
    }
    if let Some(tw) = at(d, "thisWorkspace") {
        out.push(("thisWorkspace", tw.clone()));
    }
    let compact = Options {
        compact: true,
        ensure_ascii: false,
        ..Options::default()
    };
    match json::to_string(&obj(out), &compact) {
        Ok(text) => o.line(&text),
        // Half of a surrogate pair: Python's print fails on it.
        Err(_) => return Err(Raise::at(
            74,
            "UnicodeEncodeError: 'utf-8' codec can't encode a surrogate: surrogates not allowed",
        )),
    }
    Ok(Exit::Ok)
}

/// The text form.
fn text_lines(d: &Value, ask: Ask, o: &mut Out) -> Result<Exit, Raise> {
    let (ws, conns) = ws_and_conns(d);
    let label = or(
        py_get(&ws, "name", line::LABEL)?
            .filter(|x| py::truthy(x))
            .or(at(&ws, "path")),
        s("?"),
    );
    let label_text = py::str_of(&label);
    if let Some(tw @ Value::Object(_)) = at(d, "thisWorkspace") {
        if !ask.users {
            if let Some(e) = at(tw, "error").filter(|x| py::truthy(x)) {
                o.stderr.push_str(&format!(
                    "This workspace: (could not read sessions: {})\n",
                    py::str_of(e)
                ));
            } else {
                let rows = or(at(tw, "sessions"), Value::Array(Vec::new()));
                let n = py_len(&rows, line::TW_LEN)?;
                let handle = or(at(tw, "handle"), label.clone());
                o.line(&format!(
                    "This workspace ({}): {n} live",
                    py::str_of(&handle)
                ));
                let items = py_iter(&rows, line::TW_LEN)?;
                for r in &items {
                    let title = match py_get(r, "title", line::TW_ROW)?.filter(|x| py::truthy(x)) {
                        None => String::new(),
                        Some(t @ Value::Str(_)) => py::str_of(t).replace('\n', " "),
                        Some(other) => {
                            return Err(Raise::no_attribute(line::TW_ROW, other, "replace"))
                        }
                    };
                    let title = if title.chars().count() > 50 {
                        format!("{}\u{2026}", title.chars().take(49).collect::<String>())
                    } else {
                        title
                    };
                    let addr = or(
                        at(r, "address")
                            .filter(|x| py::truthy(x))
                            .or(at(r, "handle")),
                        s("-"),
                    );
                    o.line(&format!(
                        "  {} {} {} {} {} {title}",
                        if truthy_at(r, "you") { "*" } else { " " },
                        super::py_left(&addr, 16),
                        super::py_left(&or(at(r, "role"), s("-")), 9),
                        super::py_left(&or(at(r, "cli"), s("-")), 7),
                        super::py_left(&or(at(r, "state"), s("-")), 8),
                    ));
                }
                let tail = if items.iter().any(|r| truthy_at(r, "you")) {
                    "  * = you. "
                } else {
                    "  "
                };
                o.line(&format!("{tail}Same workspace: always allowed to k2 msg. More: k2 sessions (--past for older chats)"));
            }
            o.line("");
        }
    }
    o.line(&format!("Connections for {label_text}:"));
    if !py::truthy(&conns) {
        o.line("  (none)");
    }
    for c in py_iter(&conns, line::FOR)? {
        if py_get(&c, "remote", line::C)?.is_some_and(py::truthy) {
            let addr = py::str_of(&address(&c));
            let handle = or(at(&c, "handle"), s(""));
            let display = or(at(&c, "displayName"), s(""));
            let mut bound = String::new();
            if truthy_at(&c, "remoteWorkspaceId") && py::truthy(&handle) {
                bound = format!(" \u{2192} {}", py::str_of(&handle));
                if py::truthy(&display) {
                    let low = |v: &Value| -> Result<String, Raise> {
                        if matches!(v, Value::Str(_)) {
                            Ok(py::str_of(v).trim_matches(py_space).to_lowercase())
                        } else {
                            Err(Raise::no_attribute(line::DISPLAY, v, "strip"))
                        }
                    };
                    if low(&display)? != low(&handle)? {
                        bound.push_str(&format!(" ({})", py::str_of(&display)));
                    }
                }
            }
            let repair = if truthy_at(&c, "repairRecommended") {
                "  [older pairing code: messages still work; re-pair recommended]"
            } else {
                ""
            };
            let gone = if truthy_at(&c, "remoteWorkspaceGone") {
                "  [GONE: that workspace is no longer on the host; handles are never reused, so this connection stays closed until your human adds it again]"
            } else {
                ""
            };
            o.line(&format!("  - {addr}{bound}   (remote){repair}{gone}"));
        } else {
            let flag = if at(&c, "reachable").is_none_or(py::truthy) {
                ""
            } else {
                "  [UNREACHABLE \u{2014} project missing or path gone]"
            };
            o.line(&format!(
                "  - {}{flag}",
                at(&c, "projectName").map_or_else(|| "?".into(), py::str_of)
            ));
        }
    }
    if ask.users {
        let Some(Value::Array(users)) = at(d, "users") else {
            o.line(NO_USERS);
            return Ok(Exit::Error);
        };
        o.line("");
        o.line("People on this host (humans \u{2014} not workspace-agents; do not k2 msg):");
        let rows: Vec<[String; 4]> = users
            .iter()
            .filter(|u| matches!(u, Value::Object(_)))
            .map(|u| {
                let named = truthy_at(u, "firstName") || truthy_at(u, "lastName");
                let name = if named {
                    at(u, "displayName")
                        .filter(|x| py::truthy(x))
                        .map(py::str_of)
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                [
                    py::str_of(&or(at(u, "username"), s("?"))),
                    if name.is_empty() { "-".into() } else { name },
                    py::str_of(&or(at(u, "role"), s("?"))),
                    if truthy_at(u, "disabled") {
                        "yes"
                    } else {
                        "no"
                    }
                    .into(),
                ]
            })
            .collect();
        if rows.is_empty() {
            o.line("  (none)");
        } else {
            let width = |i: usize, head: &str| {
                rows.iter()
                    .map(|r| r[i].chars().count())
                    .chain([head.len()])
                    .max()
                    .unwrap_or(0)
            };
            let (wu, wn, wr) = (width(0, "USERNAME"), width(1, "NAME"), width(2, "ROLE"));
            let row = |c: [&str; 4]| {
                format!(
                    "{}  {}  {}  {}",
                    super::ljust(c[0], wu),
                    super::ljust(c[1], wn),
                    super::ljust(c[2], wr),
                    c[3]
                )
            };
            o.line(&row(["USERNAME", "NAME", "ROLE", "DISABLED"]));
            for r in &rows {
                o.line(&row([&r[0], &r[1], &r[2], &r[3]]));
            }
        }
    } else {
        o.line("These are workspace-agents you can k2 msg. Humans on this box are not listed \u{2014} k2 connections list --users (do not k2 msg those names).");
    }
    Ok(Exit::Ok)
}

pub mod write;

#[cfg(test)]
mod tests;
