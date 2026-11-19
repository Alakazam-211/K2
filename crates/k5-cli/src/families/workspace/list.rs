//! `k5 workspace list [--running] [--json]`, as `cmd_workspace_list`
//! (`cli/src/agent-context.sh`) answers it.
//!
//! 1. The words ([`parse`]): `--running` and `--json`; any other word is
//!    skipped (`*) shift`).
//! 2. One `cli_request` over TCP (the shell calls it directly: never the
//!    cell socket), on the left of a pipe into the shell's Python
//!    ([`super::super::request_piped_tcp`]): `/cli/projects/list` for the
//!    table, `/cli/agent/list` with `--json`.
//! 3. The Python on what the call printed: the table ([`text`]): an
//!    answer that is not JSON, or an object, is one `Error:` line on
//!    stdout and exit 1; else a line per workspace (`--running`: only the
//!    agent-enabled ones), or the line for none. With `--json`
//!    ([`json_form`]): the daemon's error as one line of JSON on stderr,
//!    exit 1; else `{"ok": true, "workspaces": [...]}`, the agent rows
//!    (`--running`: only the enabled ones).
//! 4. The EXIT trap's lines, with the pipeline's code (`set -o pipefail`).
//!
//! A request with no answer is the transport's line, then death by
//! SIGPIPE, as for [`super::super::inbox`]; any other failure leaves the
//! reader nothing to read. Where the shell's Python raises (rows that are
//! not a list of objects, a name that is a list), this prints a Python
//! 3.12 traceback.

use super::super::{get, pipefail, py_get, py_iter, raised, request_piped_tcp, s, Raise, Shown};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use crate::prelude::Closing;
use crate::transport::Client;

/// The table's route.
pub const PROJECTS: &str = "/cli/projects/list";
/// `--json`'s route.
pub const AGENTS: &str = "/cli/agent/list";
/// The table's Python on an answer that is not JSON, before its `%r`.
pub const UNPARSEABLE: &str = "Error: daemon returned an unparseable response: ";
/// `--running` and none enabled.
pub const NO_AGENTS: &str = "No agent-enabled workspaces.";
/// No workspaces.
pub const NONE: &str = "No workspaces registered.";

/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    /// The table's `--running` filter.
    pub const RUNNING: u32 = 13;
    pub const FOR: u32 = 20;
    pub const NAME: u32 = 21;
    pub const ROW: u32 = 24;
    /// `--json`'s `--running` filter.
    pub const J_RUNNING: u32 = 17;
}

/// What the words ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ask {
    pub running: bool,
    pub json: bool,
}

/// The words after `list`.
pub fn parse(words: &[&str]) -> Ask {
    let mut ask = Ask::default();
    for w in words {
        match *w {
            "--running" => ask.running = true,
            "--json" => ask.json = true,
            _ => {}
        }
    }
    ask
}

/// The call and its reader, after the checks.
pub fn send(client: &mut Client, ask: Ask) -> Exit {
    let mut closing = Closing::default();
    let endpoint = if ask.json { AGENTS } else { PROJECTS };
    let left = request_piped_tcp(client, &mut closing, endpoint, &[]);
    let shown = if ask.json {
        json_form(&left.body, ask.running)
    } else {
        text(&left.body, ask.running)
    };
    let exit = pipefail(left.exit, shown.print());
    closing.flush(client, exit);
    exit
}

/// `raw[:200]`: the first 200 code points of what the Python read.
fn head(raw: &str) -> Value {
    s(&raw.chars().take(200).collect::<String>())
}

/// `x or default`.
fn or(v: Option<&Value>, default: Value) -> Value {
    v.filter(|x| py::truthy(x)).cloned().unwrap_or(default)
}

/// `f'{v:<width}'`: Python's `format(v, '<width')`.
fn left(v: &Value, width: usize) -> Result<String, Raise> {
    match v {
        // `bool` formats as `int` once there is a spec.
        Value::Bool(b) => Ok(super::super::ljust(if *b { "1" } else { "0" }, width)),
        Value::Str(_) | Value::Int(_) | Value::Float(_) => Ok(super::super::py_left(v, width)),
        other => Err(Raise::at(
            line::ROW,
            format!(
                "TypeError: unsupported format string passed to {}.__format__",
                py::type_name(other)
            ),
        )),
    }
}

/// The table's Python on what it read (`RUNNING_ONLY`: `running`).
pub fn text(body: &[u8], running: bool) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let Ok(rows) = json::parse(&raw) else {
        return Shown {
            stdout: format!("{UNPARSEABLE}{}\n", py::repr(&head(&raw))),
            stderr: String::new(),
            exit: Exit::Error,
        };
    };
    if let Value::Object(pairs) = &rows {
        let err = get(pairs, "error").cloned().unwrap_or_else(|| head(&raw));
        return Shown {
            stdout: format!("Error: {}\n", py::str_of(&err)),
            stderr: String::new(),
            exit: Exit::Error,
        };
    }
    let mut stdout = String::new();
    match table(rows, running, &mut stdout) {
        Ok(()) => Shown::out(stdout),
        Err(r) => raised(stdout, String::new(), &r),
    }
}

/// Lines 12 on.
fn table(rows: Value, running: bool, stdout: &mut String) -> Result<(), Raise> {
    let rows = if running {
        let mut kept = Vec::new();
        for r in py_iter(&rows, line::RUNNING)? {
            if py_get(&r, "agentEnabled", line::RUNNING)?.is_some_and(py::truthy) {
                kept.push(r);
            }
        }
        if kept.is_empty() {
            stdout.push_str(NO_AGENTS);
            stdout.push('\n');
            return Ok(());
        }
        Value::Array(kept)
    } else {
        rows
    };
    if !py::truthy(&rows) {
        stdout.push_str(NONE);
        stdout.push('\n');
        return Ok(());
    }
    for r in py_iter(&rows, line::FOR)? {
        let name = or(py_get(&r, "name", line::NAME)?, s("?"));
        let mode = or(py_get(&r, "agentMode", line::NAME + 1)?, s("off"));
        let path = py_get(&r, "path", line::NAME + 2)?
            .cloned()
            .unwrap_or_else(|| s(""));
        let name = left(&name, 24)?;
        let mode = left(&mode, 8)?;
        stdout.push_str(&format!("  {name} [{mode}] {}\n", py::str_of(&path)));
    }
    Ok(())
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `json.dumps(v)` and a newline.
fn dumps(v: &Value) -> String {
    format!(
        "{}\n",
        json::to_string(v, &Options::default()).unwrap_or_default()
    )
}

/// `--json`'s Python on what it read (`RUNNING_ONLY`: `running`).
pub fn json_form(body: &[u8], running: bool) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let Ok(d) = json::parse(&raw) else {
        let hint = format!(
            "daemon returned an unparseable response: {}",
            py::repr(&head(&raw))
        );
        let err = obj(vec![(
            "error",
            obj(vec![("code", s("bad_response")), ("hint", s(&hint))]),
        )]);
        return Shown::err(dumps(&err), Exit::Error);
    };
    let pairs = match &d {
        Value::Object(pairs) => Some(pairs),
        _ => None,
    };
    let failed = pairs.is_none_or(|p| {
        get(p, "error").is_some_and(py::truthy) || !get(p, "ok").is_some_and(py::truthy)
    });
    if failed {
        let err = pairs
            .and_then(|p| get(p, "error"))
            .cloned()
            .unwrap_or(Value::Null);
        let err = if matches!(err, Value::Object(_)) {
            err
        } else {
            let hint = if py::truthy(&err) {
                py::str_of(&err)
            } else {
                py::str_of(&head(&raw))
            };
            obj(vec![("code", s("error")), ("hint", s(&hint))])
        };
        return Shown::err(dumps(&obj(vec![("error", err)])), Exit::Error);
    }
    let rows = or(
        pairs.and_then(|p| get(p, "agents")),
        Value::Array(Vec::new()),
    );
    let rows = if running {
        let kept = py_iter(&rows, line::J_RUNNING).and_then(|items| {
            let mut kept = Vec::new();
            for r in items {
                if py_get(&r, "enabled", line::J_RUNNING)?.is_some_and(py::truthy) {
                    kept.push(r);
                }
            }
            Ok(kept)
        });
        match kept {
            Ok(k) => Value::Array(k),
            Err(r) => return raised(String::new(), String::new(), &r),
        }
    } else {
        rows
    };
    Shown::out(dumps(&obj(vec![
        ("ok", Value::Bool(true)),
        ("workspaces", rows),
    ])))
}
