//! `k5 sessions live <workspace> [--json|--count]`: the daemon's live
//! sessions under a workspace, as the shell's `cmd_sessions_live`
//! (`cli/src/awareness.sh`) reads them.
//!
//! After the gates ([`super`]), in the shell's order:
//!
//! 1. The words ([`parse`]): the first is the workspace, whatever it is;
//!    after it `--json` and `--count` (the last one wins), every other
//!    word skipped. No workspace is two lines, exit 1.
//! 2. A folder is taken as `cd <it> && pwd` names it; anything else is
//!    asked of `/cli/workspace/resolve` (`cli_request … | python3 …`, its
//!    errors hidden): its `path` when the answer has one. No answer there
//!    ends the command by SIGPIPE ([`super::super::end_in_pipe`]).
//! 3. `response=$(cli_request /cli/sessions/list-for-workspace …
//!    2>/dev/null)`: a failure ends the command with curl's code, no
//!    answer with exit 1, each with nothing said. An answer that names
//!    `"route not found"` is asked again of `/cli/agents/running` (an
//!    older daemon), whose failures say their line.
//! 4. The shell's Python on the answer ([`lay_out`]): the table, the
//!    count, the JSON (as it came, or the older daemon's rows as Python
//!    indents them), or the refusal's lines (exit 3 when it reads as the
//!    owner's view, else 1).
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The shell answers it when `CDPATH` moves `cd` to another folder
//! ([`crate::families::sim::cd_moved`]), or the folder is one `cd` can't
//! enter. An answer that is not UTF-8: the shell's Python raises on
//! it; this reads it with U+FFFD.

use super::super::sidecar::py_slice;
use super::super::{end_in_pipe, get, ljust, onboarding, py_left, raised, s, Raise, Shown};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::encode::quote;
use crate::transport::{Call, Channel, Client, Error, Response};
use k2_client::brand::CLI_NAME as CLI;

pub const RESOLVE: &str = "/cli/workspace/resolve";
pub const LIST: &str = "/cli/sessions/list-for-workspace";
pub const RUNNING: &str = "/cli/agents/running";

/// What the table, count or JSON shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Table,
    Json,
    Count,
}

/// The words after `live`: the workspace and the mode.
pub fn parse(words: &[&str]) -> (String, Mode) {
    let path = words.first().copied().unwrap_or("").to_string();
    let mut mode = Mode::Table;
    for w in words.iter().skip(1) {
        match *w {
            "--json" => mode = Mode::Json,
            "--count" => mode = Mode::Count,
            _ => {}
        }
    }
    (path, mode)
}

/// No workspace.
pub fn usage() -> String {
    format!(
        "Usage: {CLI} sessions live <workspace> [--json|--count]\n  Lists live daemon-side v2 sessions whose cwd is under the workspace (a path, handle or name).\n"
    )
}

/// `$(cd "$path" && pwd)` for a folder, from the logical folder. `None`:
/// not a folder. `Some(None)`: a folder the binary leaves to the shell.
pub fn folder(path: &str) -> Option<Option<String>> {
    if path.is_empty() || !std::path::Path::new(path).is_dir() {
        return None;
    }
    if crate::families::sim::cd_moved(path) {
        return Some(None);
    }
    let Ok(pwd) = crate::daemon::logical_cwd(std::env::var_os("PWD")) else {
        return Some(None);
    };
    Some(onboarding::cd_pwd(&pwd, path))
}

/// The resolve Python on what the call printed: `d.get("path", "")` of a
/// dict, else nothing; `$(…)` drops the trailing newlines.
pub fn resolved(body: &[u8]) -> String {
    let Ok(text) = std::str::from_utf8(body) else {
        return String::new();
    };
    let mut p = match json::parse(text) {
        Ok(Value::Object(pairs)) => get(&pairs, "path").map_or_else(String::new, py::str_of),
        _ => String::new(),
    };
    while p.ends_with('\n') {
        p.pop();
    }
    p
}

/// What `$(…)` holds of an answer: NULs and trailing newlines gone.
fn held(r: &Response) -> Vec<u8> {
    let mut b = super::super::without_nuls(&r.body);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    b
}

/// Steps 2 to 4 of the module's notes, on a connected client; `path`
/// is not empty, and a folder was taken as `cd` names it.
pub fn run(
    client: &mut Client,
    closing: &mut Closing,
    path: &str,
    dir: Option<&str>,
    mode: Mode,
) -> Exit {
    let passport = client.passport().is_some();
    let path = match dir {
        Some(d) => d.to_string(),
        None => {
            let q = format!("q={}", quote(path));
            let got = match client.call(Channel::Tcp, RESOLVE, Call::Request(&[q.as_str()])) {
                Ok(r) => {
                    closing.answer(&r, passport);
                    resolved(&super::super::piped_body(&r))
                }
                Err(Error::NoAnswer(_)) => end_in_pipe(),
                Err(_) => String::new(),
            };
            if got.is_empty() {
                path.to_string()
            } else {
                got
            }
        }
    };
    let q = format!("path={}", quote(&path));
    let response = match client.call(Channel::Tcp, LIST, Call::Request(&[q.as_str()])) {
        Ok(r) => {
            closing.answer(&r, passport);
            held(&r)
        }
        Err(Error::NoAnswer(_)) => return Exit::Error,
        Err(e) => return Exit::from_curl(e.curl_code()),
    };
    let mut fallback = false;
    let response = if contains(&response, "\"route not found\"") {
        fallback = true;
        match client.call(Channel::Tcp, RUNNING, Call::Request(&[])) {
            Ok(r) => {
                closing.answer(&r, passport);
                held(&r)
            }
            Err(Error::NoAnswer(n)) => {
                out::stderr_line(&n.to_string());
                return Exit::Error;
            }
            Err(e) => return Exit::from_curl(e.curl_code()),
        }
    } else {
        response
    };
    lay_out(&response, mode, &path, fallback).print()
}

fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// The lines of the shell's Python that raise.
pub mod line {
    pub const REFUSED: u32 = 26;
    pub const MATCHES: u32 = 35;
    pub const ROWS: u32 = 49;
    pub const KIND: u32 = 68;
    pub const HANDLE: u32 = 69;
    pub const NAME: u32 = 70;
    pub const CMD: u32 = 71;
}

/// `x or default`.
fn or(v: Option<&Value>, default: Value) -> Value {
    v.filter(|x| py::truthy(x)).cloned().unwrap_or(default)
}

fn at<'a>(d: &'a [(Text, Value)], key: &str) -> Option<&'a Value> {
    get(d, key)
}

/// The shell's Python on the answer (`printf '%s' "$response"`).
pub fn lay_out(body: &[u8], mode: Mode, path: &str, fallback: bool) -> Shown {
    let raw = String::from_utf8_lossy(body).into_owned();
    let mut o = String::new();
    match shown(&raw, mode, path, fallback, &mut o) {
        Ok(s) => s,
        Err(r) => raised(o, String::new(), &r),
    }
}

fn first(raw: &str, n: usize) -> String {
    raw.chars().take(n).collect()
}

fn shown(
    raw: &str,
    mode: Mode,
    path: &str,
    fallback: bool,
    o: &mut String,
) -> Result<Shown, Raise> {
    let data = match json::parse(raw) {
        Ok(d) => d,
        Err(_) => {
            return Ok(Shown::err(
                format!(
                    "Error: the daemon answered something that is not JSON: {}\n",
                    py::repr(&s(&first(raw, 300)))
                ),
                Exit::Error,
            ))
        }
    };
    let rows = match &data {
        Value::Array(rows) => rows.clone(),
        _ => return Ok(refusal(&data, raw)),
    };
    let sessions: Vec<Vec<(Text, Value)>> = if fallback {
        let mut out = Vec::new();
        for r in &rows {
            let Value::Object(d) = r else { continue };
            let cwd = or(at(d, "cwd"), s(""));
            if !matches(&cwd, path)? {
                continue;
            }
            out.push(vec![
                (
                    "sessionId".into(),
                    at(d, "terminalId").cloned().unwrap_or(Value::Null),
                ),
                (
                    "agentName".into(),
                    at(d, "agentName").cloned().unwrap_or(Value::Null),
                ),
                (
                    "command".into(),
                    at(d, "command").cloned().unwrap_or(Value::Null),
                ),
                ("args".into(), Value::Array(Vec::new())),
                ("cwd".into(), cwd),
                ("isV2".into(), Value::Bool(true)),
            ]);
        }
        out
    } else {
        rows.iter()
            .filter_map(|r| match r {
                Value::Object(d) => Some(d.clone()),
                _ => None,
            })
            .collect()
    };
    let note = if fallback {
        "  (using /cli/agents/running fallback \u{2014} daemon pre-0.37.11)"
    } else {
        ""
    };
    match mode {
        Mode::Json => {
            let text = if fallback {
                let v = Value::Array(sessions.into_iter().map(Value::Object).collect());
                json::to_string(
                    &v,
                    &Options {
                        indent: Some(2),
                        ..Options::default()
                    },
                )
                .unwrap_or_default()
            } else {
                raw.to_string()
            };
            return Ok(Shown::out(format!("{text}\n")));
        }
        Mode::Count => return Ok(Shown::out(format!("{}\n", sessions.len()))),
        Mode::Table => {}
    }
    if sessions.is_empty() {
        return Ok(Shown::out(format!(
            "No live sessions in this workspace.{note}\n"
        )));
    }
    o.push_str(&format!("{} live session(s):{note}\n", sessions.len()));
    o.push_str(&format!(
        "  {}  {}  {}  {}  {}  CWD\n",
        ljust("TERMINAL_ID", 38),
        ljust("KIND", 10),
        ljust("HANDLE", 22),
        ljust("AGENT", 24),
        ljust("CMD", 14)
    ));
    for d in &sessions {
        let sid = or(at(d, "sessionId"), or(at(d, "terminalId"), s("?")));
        let kind = py_slice(&or(at(d, "kind"), s("")), 10, line::KIND)?;
        let handle = py_slice(&or(at(d, "handle"), s("")), 22, line::HANDLE)?;
        let name = py_slice(&or(at(d, "agentName"), s("?")), 24, line::NAME)?;
        let cmd = py_slice(&or(at(d, "command"), s("shell")), 14, line::CMD)?;
        let cwd = or(at(d, "cwd"), s(""));
        o.push_str(&format!(
            "  {}  {}  {}  {}  {}  {}\n",
            py_left(&sid, 38),
            py_left(&kind, 10),
            py_left(&handle, 22),
            py_left(&name, 24),
            py_left(&cmd, 14),
            py::str_of(&cwd)
        ));
    }
    Ok(Shown::out(std::mem::take(o)))
}

/// `matches(cwd)`: the session's folder is the workspace or under it.
fn matches(cwd: &Value, path: &str) -> Result<bool, Raise> {
    if !py::truthy(cwd) {
        return Ok(false);
    }
    let Value::Str(t) = cwd else {
        return Err(Raise {
            frames: vec![(line::ROWS, "<module>"), (line::MATCHES, "matches")],
            exception: format!(
                "AttributeError: '{}' object has no attribute 'rstrip'",
                py::type_name(cwd)
            ),
        });
    };
    let cwd = py::str_of(&Value::Str(t.clone()));
    let p = path.trim_end_matches('/');
    let c = cwd.trim_end_matches('/');
    Ok(c == p || cwd.starts_with(&format!("{p}/")))
}

/// An answer that is no list: its error, and the owner's-view line when
/// it reads as a refusal.
fn refusal(data: &Value, raw: &str) -> Shown {
    let d = match data {
        Value::Object(d) => Some(d),
        _ => None,
    };
    let err = d
        .and_then(|d| get(d, "error"))
        .cloned()
        .unwrap_or(Value::Null);
    let (code, msg) = match &err {
        Value::Object(e) => {
            let code = py::str_of(&or(get(e, "code"), s("")));
            let msg = or(
                get(e, "hint"),
                or(
                    get(e, "message"),
                    if code.is_empty() {
                        s(&json::to_string(&err, &Options::default()).unwrap_or_default())
                    } else {
                        s(&code)
                    },
                ),
            );
            (code, msg)
        }
        e if py::truthy(e) => {
            let m = py::str_of(e);
            (m.clone(), s(&m))
        }
        _ => (
            String::new(),
            s(&format!("unexpected answer: {}", first(raw, 300))),
        ),
    };
    let mut stderr = format!("Error: {}\n", py::str_of(&msg));
    let refused = match d {
        None => false,
        Some(d) if get(d, "permission").is_some() => true,
        Some(_) => {
            let Value::Str(_) = msg else {
                let r = Raise {
                    frames: vec![(line::REFUSED, "<module>"), (line::REFUSED, "<genexpr>")],
                    exception: format!(
                        "TypeError: can only concatenate str (not \"{}\") to str",
                        py::type_name(&msg)
                    ),
                };
                return raised(String::new(), stderr, &r);
            };
            let all = format!("{code} {}", py::str_of(&msg)).to_lowercase();
            [
                "owner",
                "forbidden",
                "credential",
                "token",
                "person",
                "not allowed",
            ]
            .iter()
            .any(|w| all.contains(w))
        }
    };
    if refused {
        stderr.push_str(&format!(
            "This is the owner's raw PTY view. To see who is working in your workspace, run: {CLI} sessions\n"
        ));
    }
    Shown::err(stderr, if refused { Exit::Refused } else { Exit::Error })
}
