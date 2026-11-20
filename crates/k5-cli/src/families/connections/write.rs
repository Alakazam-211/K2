//! `k5 connections add <name>` and `k5 connections remove <name>`, as the
//! shell CLI's `cmd_connections` answers its `add` and `remove`
//! (`cli/src/usage.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings, the two gates and the `id` tool's checks,
//!    as for `connections list` ([`super`]).
//! 2. The target: the word after the verb (`${1:-}`); none, or an empty
//!    one, is `Usage: k2 connections <verb> <workspace-name>`, exit 1.
//!    Any word after it is not read.
//! 3. One `cli_request_post /cli/connections
//!    "action=<verb>&target=<escaped>"` over TCP (the shell calls it
//!    directly: never the cell socket), on the left of a pipe into the
//!    shell's Python ([`super::super::request_post_piped_tcp`]). A POST
//!    is sent once: only a connect that was refused or timed out is tried
//!    again, and a 503 `migrating` (no handler ran). An answer with no
//!    body is the answer (the reader's "unparseable" line on stderr,
//!    exit 1); no
//!    answer (none in time, an empty reply, a 5xx with no body) is the
//!    transport's line, then death by SIGPIPE as the shell's
//!    `_k2_end_command` ends a pipe.
//! 4. The Python ([`reader`]): the daemon's error, as JSON on stderr with
//!    exit 3 when it is the "agents may not change connections" gate,
//!    else `Error: <hint>` on stderr and exit 1 (0.46.17: every error on
//!    stderr, stdout only the result lines); else `Connected to <x>`
//!    (or `Already connected to <x> (<message>)`), the workspace a remote
//!    add reaches, and the daemon's warnings on stderr (`add`), or
//!    `Disconnected.` (`remove`).
//! 5. The EXIT trap's lines, with the pipeline's code (`set -o pipefail`).
//!
//! The shell's other actions (`create` is one: its usage lines, exit 1)
//! stay the shell's: the route table sends only `add` and `remove` here.
//! Where the shell's Python raises, this prints a Python 3.12 traceback.
//! An answer that is not UTF-8, and half of a surrogate pair, are as
//! [`super::super::inbox`] says.

use super::super::{get, pipefail, py_get, raised, request_post_piped_tcp, s, Raise, Shown};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::encode::quote;
use crate::transport::Client;

/// What the verb does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Add,
    Remove,
}

impl Verb {
    /// The verb's word, as the shell's `case` has it.
    pub fn word(self) -> &'static str {
        match self {
            Verb::Add => "add",
            Verb::Remove => "remove",
        }
    }

    /// The verb a first word names.
    pub fn of(word: &str) -> Option<Verb> {
        match word {
            "add" => Some(Verb::Add),
            "remove" => Some(Verb::Remove),
            _ => None,
        }
    }

    /// No target: exit 1.
    pub fn usage(self) -> String {
        format!("Usage: k2 connections {} <workspace-name>", self.word())
    }
}

/// The gate's codes (`code in (…)`).
pub const GATED_CODES: [&str; 3] = ["agents_create_connections_disabled", "gated", "forbidden"];
/// The two hints that mean the gate.
pub const GATED_HINTS: [&str; 2] = [
    "Agents can add connections",
    "isn't allowed to create or remove connections",
];
/// What a gated hint without `Settings` in it gets after it.
pub const TEACHING: &str = concat!(
    "this agent isn't allowed to create or remove connections \u{2014} ",
    "Owner can turn on: Settings \u{2192} Network \u{2192} Tunnel \u{2192} Policies \u{2192} ",
    "\"Agents can add connections\" (or Settings \u{2192} Workspaces / Agents \u{2192} ",
    "<workspace> \u{2192} Connections \u{2192} \"Let this agent add connections\")."
);
/// The gate's code when the error had none.
pub const DEFAULT_CODE: &str = "agents_create_connections_disabled";

/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    pub const ERR: u32 = 9;
    pub const ALLOW: u32 = 19;
    pub const NOT_ALLOWED: u32 = 20;
    pub const SETTINGS: u32 = 23;
    pub const JOIN: u32 = 24;
    pub const RPARTITION: u32 = 42;
    pub const LABEL: u32 = 43;
    pub const STRIP: u32 = 44;
}

/// The call and its reader, after the checks: `target` is the word after
/// the verb.
pub fn send(client: &mut Client, verb: Verb, target: &str) -> Exit {
    let mut closing = Closing::default();
    let target_pair = format!("target={}", quote(target));
    let action = format!("action={}", verb.word());
    let left = request_post_piped_tcp(
        client,
        &mut closing,
        super::ENDPOINT,
        &[action.as_str(), target_pair.as_str()],
    );
    let shown = reader(&left.body, verb);
    let exit = pipefail(left.exit, shown.print());
    closing.flush(client, exit);
    exit
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

/// `x or default`.
fn or(v: Option<&Value>, default: Value) -> Value {
    v.filter(|x| py::truthy(x)).cloned().unwrap_or(default)
}

/// `needle in v` for a string needle, or the `TypeError` at `line`.
fn py_in(needle: &str, v: &Value, line: u32) -> Result<bool, Raise> {
    match v {
        Value::Str(_) => Ok(py::str_of(v).contains(needle)),
        Value::Array(items) => Ok(items.iter().any(|x| *x == s(needle))),
        Value::Object(pairs) => Ok(pairs.iter().any(|(k, _)| *k == Text::from(needle))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: argument of type '{}' is not iterable",
                py::type_name(other)
            ),
        )),
    }
}

/// `v + '<str>'`: the text, or the `TypeError` at `line`.
fn py_plus_str(v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(py::str_of(v)),
        Value::Array(_) => Err(Raise::at(
            line,
            "TypeError: can only concatenate list (not \"str\") to list",
        )),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                py::type_name(other)
            ),
        )),
    }
}

/// `v.<name>(...)` on a value that must be a string: its text, or the
/// `AttributeError` at `line`.
fn py_str_method(v: &Value, name: &str, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(py::str_of(v)),
        other => Err(Raise::no_attribute(line, other, name)),
    }
}

/// What the shell's Python prints for the answer `raw` (what the call
/// printed: the body, NULs dropped).
pub fn reader(raw: &[u8], verb: Verb) -> Shown {
    let text = String::from_utf8_lossy(raw).into_owned();
    let Ok(d) = json::parse(&text) else {
        let head: String = text.chars().take(200).collect();
        return Shown {
            stdout: String::new(),
            stderr: format!(
                "{}{}\n",
                super::UNPARSEABLE,
                py::repr(&Value::Str(Text::from(head)))
            ),
            exit: Exit::Error,
        };
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    let r = lines(&d, verb, &mut stdout, &mut stderr);
    match r {
        Ok(exit) => Shown {
            stdout,
            stderr,
            exit,
        },
        Err(r) => raised(stdout, stderr, &r),
    }
}

fn lines(d: &Value, verb: Verb, stdout: &mut String, stderr: &mut String) -> Result<Exit, Raise> {
    if let Some(err) = py_get(d, "error", line::ERR)? {
        if *err != Value::Null {
            return error_lines(err, stdout, stderr);
        }
    }
    if verb == Verb::Remove {
        stdout.push_str("Disconnected.\n");
        return Ok(Exit::Ok);
    }
    let target_or_q = at(d, "target").cloned().unwrap_or_else(|| s("?"));
    if truthy_at(d, "noop") {
        let message = at(d, "message").cloned().unwrap_or_else(|| s(""));
        stdout.push_str(&format!(
            "Already connected to {} ({})\n",
            py::str_of(&target_or_q),
            py::str_of(&message)
        ));
    } else {
        stdout.push_str(&format!("Connected to {}\n", py::str_of(&target_or_q)));
    }
    if truthy_at(d, "bound") && truthy_at(d, "handle") {
        let target = or(at(d, "target"), s(""));
        let target = py_str_method(&target, "rpartition", line::RPARTITION)?;
        let host = match target.rfind("::") {
            Some(i) => target[i + 2..].to_string(),
            None => target,
        };
        let handle = at(d, "handle").cloned().unwrap_or(Value::Null);
        let mut label = py_plus_str(&handle, line::LABEL)?;
        if !host.is_empty() {
            label.push_str("::");
            label.push_str(&host);
        }
        if let Some(display) = at(d, "displayName").filter(|x| py::truthy(x)) {
            let shown = py_str_method(display, "strip", line::STRIP)?;
            let low = |t: &str| t.trim_matches(super::super::py_space).to_lowercase();
            let handle_text = py::str_of(&handle);
            if low(&shown) != low(&handle_text) {
                label.push_str(&format!(" ({shown})"));
            }
        }
        stdout.push_str(&format!("  reaches {label}\n"));
    }
    for key in ["bindWarning", "warning"] {
        if let Some(w) = at(d, key).filter(|x| py::truthy(x)) {
            stderr.push_str(&format!("warning: {}\n", py::str_of(w)));
        }
    }
    Ok(Exit::Ok)
}

/// `if err is not None:` (both verbs).
fn error_lines(err: &Value, stdout: &mut String, stderr: &mut String) -> Result<Exit, Raise> {
    let (code, hint) = match err {
        Value::Object(_) => {
            let code = or(at(err, "code"), s(""));
            let hint = match at(err, "hint").filter(|x| py::truthy(x)) {
                Some(h) => h.clone(),
                None => s(&json::to_string(err, &Options::default()).unwrap_or_default()),
            };
            (code, hint)
        }
        other => (s(""), s(&py::str_of(other))),
    };
    let gated = GATED_CODES.iter().any(|c| code == s(c))
        || py_in(GATED_HINTS[0], &hint, line::ALLOW)?
        || py_in(GATED_HINTS[1], &hint, line::NOT_ALLOWED)?;
    if !gated {
        stderr.push_str(&format!("Error: {}\n", py::str_of(&hint)));
        return Ok(Exit::Error);
    }
    let hint = if py_in("Settings", &hint, line::SETTINGS)? {
        hint
    } else {
        let head = if py::truthy(&hint) {
            py_plus_str(&hint, line::JOIN)? + " \u{2014} "
        } else {
            String::new()
        };
        s(&(head + TEACHING))
    };
    let code = if py::truthy(&code) {
        code
    } else {
        s(DEFAULT_CODE)
    };
    let shape = Value::Object(vec![(
        Text::from("error"),
        Value::Object(vec![(Text::from("code"), code), (Text::from("hint"), hint)]),
    )]);
    let plain = Options {
        ensure_ascii: false,
        ..Options::default()
    };
    let line = json::to_string(&shape, &plain)
        .or_else(|_| json::to_string(&shape, &Options::default()))
        .unwrap_or_default();
    stderr.push_str(&line);
    stderr.push('\n');
    Ok(Exit::Refused)
}

/// `run`'s part once the checks passed: the target, or the usage line.
pub fn target<'a>(verb: Verb, rest: &[&'a str]) -> Result<&'a str, String> {
    match rest.first() {
        Some(t) if !t.is_empty() => Ok(t),
        _ => Err(verb.usage()),
    }
}

/// Prints the usage line: exit 1.
pub fn usage(verb: Verb) -> Exit {
    out::stderr_line(&verb.usage());
    Exit::Error
}

#[cfg(test)]
mod tests;
