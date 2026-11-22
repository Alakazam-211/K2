//! `k5 project ...`: a server's projects (named groups of agent
//! workspaces with one shared chat and one Point of Contact), as the shell
//! CLI's `project)` arm and its `cmd_project_*` functions answer them
//! (`cli/src/router.sh`, `cli/src/project.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the two gates ([`crate::prelude`]).
//! 2. `project` is an `id` tool in the shell's policy, so with `HOOK_SOCK`
//!    set: a socket that is gone with a passport the probe finds refused,
//!    and no passport at all, each stop it with exit 3
//!    ([`crate::prelude::id_tool`]). The credential is the shell's
//!    `$TOKEN`: the passport, else the owner token.
//! 3. The verb's words ([`parse`]): a word it can't take is one line of
//!    compact JSON on stderr (`_project_err "usage" …`), exit 2, nothing
//!    sent. `msg -` reads its text from stdin (`$(cat)`: NULs and trailing
//!    newlines dropped).
//! 4. `_project_py`, a `python3 -` block with its own urllib client
//!    ([`crate::transport::urllib`]): the verb's GETs and
//!    POSTs of `/cli/project-group/*` (and `/cli/agent-access/set` for
//!    `access`), each `urlopen(…, timeout=30)` once, and its lines. Its
//!    errors are compact JSON on stderr: exit 4 for a name that can't be
//!    resolved, 2 for `usage`, else 1, then a refusal's permission card
//!    lines (`Ask with: …`), as the EXIT trap prints the note file.
//!
//! Answers are read from the response body. Where the shell's Python
//! raises, this prints a traceback whose last line is Python 3.12's, with
//! one `<stdin>` frame at the block's line that raised (the shell's has
//! the nested function frames too).
//!
//! Handed to the shell, before anything is printed or sent: a word that is
//! not UTF-8, `-h` or `--help` anywhere (the front's), `HOOK_SOCK` set with
//! a `CODEX_` variable exported, a port curl won't take, and a `python3`
//! the binary can't tell.
//!
//! # Where this differs from the shell (a named difference)
//!
//! `msg`'s text rides the Python's environment (`P_TEXT`); Linux refuses
//! one environment string over 128 KiB, so the shell prints `Argument
//! list too long` (exit 126) for a longer message. The binary sends it.

use super::feedback::write::{traceback_stdin, PyOut};
use super::feedback::{ago, py_int, PyStr};
use super::{curl_port, get, py_iter, py_len, py_space, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude;
use crate::python;
use crate::transport::urllib::{decode, Method, Opener, Outcome, Refused, Request};
use k2_client::{cli_name, product_name};
use std::ffi::OsString;
use std::io::Read as _;
use std::time::Duration;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["project"];
/// Every form is the family's; a line with `-h` or `--help` is the
/// front's (a help page, or the shell's).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The shell's tool id.
pub const TOOL: &str = "project";
pub const LIST: &str = "/cli/project-group/list";
pub const SHOW: &str = "/cli/project-group/show";
pub const MESSAGES: &str = "/cli/project-group/messages";
const TIMEOUT: Duration = Duration::from_secs(30);

/// The block's lines (`<stdin>`), where a verb's statement raises.
pub mod line {
    pub const DIE: u32 = 52;
    pub const CALL: u32 = 95;
    pub const CREATE: u32 = 180;
    pub const LIST: u32 = 192;
    pub const SHOW: u32 = 212;
    pub const ADD: u32 = 249;
    pub const REMOVE: u32 = 274;
    pub const POC: u32 = 287;
    pub const MSG: u32 = 316;
    pub const READ: u32 = 376;
    pub const RENAME: u32 = 407;
    pub const DELETE: u32 = 415;
    pub const PIN: u32 = 425;
    pub const VIEW: u32 = 439;
    pub const ACCESS: u32 = 514;
}

/// One verb and its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    Create {
        name: String,
    },
    List,
    Show {
        project: String,
    },
    Add {
        project: String,
        workspace: String,
    },
    Remove {
        project: String,
        workspace: String,
    },
    Poc {
        project: String,
        workspace: String,
    },
    /// `project` empty: auto-detected. `stdin`: the text is `-`.
    Msg {
        project: String,
        text: String,
        two: bool,
        stdin: bool,
    },
    Read {
        project: String,
        after: String,
        limit: String,
    },
    Rename {
        project: String,
        name: String,
    },
    Delete {
        project: String,
    },
    Pin {
        project: String,
        state: String,
    },
    View {
        project: String,
        view: String,
        agent: String,
    },
    Access {
        on: bool,
        workspace: String,
    },
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    /// `project` alone or an empty verb: its help.
    Help,
    /// No arm for the verb: `unknown command '<verb>'`, exit 2.
    Unknown(String),
    /// A verb, `--json`, or the usage line that stops it.
    Verb(Result<(Verb, bool), String>),
}

/// `_project_err <code> <hint>`: one line of compact JSON, as written
/// with `ensure_ascii=False`, without its newline.
pub fn err_line(code: &str, hint: &str) -> String {
    compact(&obj(vec![(
        "error",
        obj(vec![("code", s(code)), ("hint", s(hint))]),
    )]))
    .unwrap_or_default()
}

fn usage(hint: &str) -> String {
    err_line("usage", hint)
}

const QUOTE: &str = "quote names containing spaces";
const QUOTE_BOT: &str = "quote names containing spaces (\"QA Bot\")";

/// Positionals into `slots` (each filled once, in order), `--json`, and
/// the shell's lines for an unknown flag and one word too many.
fn positionals(
    rest: &[&str],
    slots: usize,
    flag_usage: &str,
    extra: &str,
) -> Result<(Vec<String>, bool), String> {
    let mut got = vec![String::new(); slots];
    let mut json = false;
    for w in rest {
        match *w {
            "--json" => json = true,
            f if f.starts_with('-') => {
                return Err(usage(&format!("unknown flag '{f}' \u{2014} {flag_usage}")))
            }
            w => match got.iter_mut().find(|g| g.is_empty()) {
                Some(slot) => *slot = w.to_string(),
                None => {
                    return Err(usage(&format!(
                        "unexpected extra argument '{w}' \u{2014} {extra}"
                    )))
                }
            },
        }
    }
    Ok((got, json))
}

fn need(v: &str, hint: &str) -> Result<(), String> {
    if v.is_empty() {
        Err(usage(hint))
    } else {
        Ok(())
    }
}

macro_rules! cli {
    ($($t:expr),*) => { concat!("usage: ", cli_name!(), " project ", $($t),*) };
}

/// The words after `project`.
pub fn parse(words: &[&str]) -> Form {
    let verb = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or_default();
    let v = match verb {
        "" => return Form::Help,
        "create" => {
            positionals(rest, 1, cli!("create <name> [--json]"), QUOTE).and_then(|(p, j)| {
                need(
                    &p[0],
                    "create requires a <name> (quote names containing spaces)",
                )?;
                Ok((Verb::Create { name: p[0].clone() }, j))
            })
        }
        "rename" => positionals(rest, 2, cli!("rename <project> <new-name> [--json]"), QUOTE)
            .and_then(|(p, j)| {
                if p[0].is_empty() || p[1].is_empty() {
                    return Err(usage("rename requires <project> and <new-name>"));
                }
                Ok((
                    Verb::Rename {
                        project: p[0].clone(),
                        name: p[1].clone(),
                    },
                    j,
                ))
            }),
        "delete" => parse_delete(rest),
        "pin" => parse_pin(rest),
        "view" => parse_view(rest),
        "list" => {
            let mut j = false;
            let mut bad = None;
            for w in rest {
                if *w == "--json" {
                    j = true;
                } else {
                    bad = Some(usage(&format!(
                        "unexpected argument '{w}' \u{2014} {}",
                        cli!("list [--json]")
                    )));
                    break;
                }
            }
            match bad {
                Some(e) => Err(e),
                None => Ok((Verb::List, j)),
            }
        }
        "show" => {
            positionals(rest, 1, cli!("show <project> [--json]"), QUOTE).and_then(|(p, j)| {
                need(
                    &p[0],
                    "show requires a <project> (a name; a unique prefix works)",
                )?;
                Ok((
                    Verb::Show {
                        project: p[0].clone(),
                    },
                    j,
                ))
            })
        }
        "add" | "remove" => {
            let flag = if verb == "add" {
                cli!("add <project> <workspace> [--json]")
            } else {
                cli!("remove <project> <workspace> [--json]")
            };
            positionals(rest, 2, flag, QUOTE_BOT).and_then(|(p, j)| {
                if p[0].is_empty() || p[1].is_empty() {
                    return Err(usage(&format!("{verb} requires <project> and <workspace>")));
                }
                let (project, workspace) = (p[0].clone(), p[1].clone());
                Ok((
                    if verb == "add" {
                        Verb::Add { project, workspace }
                    } else {
                        Verb::Remove { project, workspace }
                    },
                    j,
                ))
            })
        }
        "poc" => positionals(
            rest,
            2,
            cli!("poc <project> [<workspace>] [--json]"),
            QUOTE_BOT,
        )
        .and_then(|(p, j)| {
            need(
                &p[0],
                "poc requires a <project> (add <workspace> to reassign)",
            )?;
            Ok((
                Verb::Poc {
                    project: p[0].clone(),
                    workspace: p[1].clone(),
                },
                j,
            ))
        }),
        "msg" => parse_msg(rest),
        "read" => parse_read(rest),
        "access" => parse_access(rest),
        other => return Form::Unknown(other.to_string()),
    };
    Form::Verb(v)
}

fn parse_delete(rest: &[&str]) -> Result<(Verb, bool), String> {
    let (mut project, mut yes, mut j) = (String::new(), false, false);
    for w in rest {
        match *w {
            "--json" => j = true,
            "--yes" | "-y" => yes = true,
            f if f.starts_with('-') => {
                return Err(usage(&format!(
                    "unknown flag '{f}' \u{2014} {}",
                    cli!("delete <project> --yes [--json]")
                )))
            }
            p if project.is_empty() => project = p.to_string(),
            p => {
                return Err(usage(&format!(
                    "unexpected extra argument '{p}' \u{2014} {QUOTE}"
                )))
            }
        }
    }
    need(&project, "delete requires a <project>")?;
    if !yes {
        return Err(usage(
            "delete removes the project, its dashboards and its whole chat; pass --yes to confirm",
        ));
    }
    Ok((Verb::Delete { project }, j))
}

fn parse_pin(rest: &[&str]) -> Result<(Verb, bool), String> {
    let (mut project, mut state, mut j) = (String::new(), String::new(), false);
    for w in rest {
        match *w {
            "--json" => j = true,
            f if f.starts_with('-') => {
                return Err(usage(&format!(
                    "unknown flag '{f}' \u{2014} {}",
                    cli!("pin <project> [on|off] [--json]")
                )))
            }
            p if project.is_empty() => project = p.to_string(),
            "on" | "off" if state.is_empty() => state = (*w).to_string(),
            p if state.is_empty() => return Err(usage(&format!("pin takes on or off, not '{p}'"))),
            p => {
                return Err(usage(&format!(
                    "unexpected extra argument '{p}' \u{2014} {QUOTE}"
                )))
            }
        }
    }
    need(&project, "pin requires a <project>")?;
    if state.is_empty() {
        state = "on".into();
    }
    Ok((Verb::Pin { project, state }, j))
}

const VIEW_USAGE: &str =
    cli!("view <project> [terminal|thread|chatter|default] [--agent <name>] [--json]");

fn parse_view(rest: &[&str]) -> Result<(Verb, bool), String> {
    let (mut project, mut view, mut agent, mut j) =
        (String::new(), String::new(), String::new(), false);
    let mut i = 0;
    while let Some(&w) = rest.get(i) {
        match w {
            "--json" => j = true,
            "--agent" => {
                match rest.get(i + 1) {
                    Some(a) if !a.is_empty() => agent = (*a).to_string(),
                    _ => {
                        return Err(usage(
                            "--agent requires an agent's name, folder path or workspace id",
                        ))
                    }
                }
                i += 1;
            }
            f if f.starts_with('-') => {
                return Err(usage(&format!("unknown flag '{f}' \u{2014} {VIEW_USAGE}")))
            }
            p if project.is_empty() => project = p.to_string(),
            p if view.is_empty() => {
                let lower = p.to_ascii_lowercase();
                match lower.as_str() {
                    "terminal" | "thread" | "chatter" | "default" => view = lower,
                    "split" => {
                        return Err(usage(
                            "split is not a project view: a project's agents open in terminal, thread or chatter",
                        ))
                    }
                    _ => {
                        return Err(usage(&format!(
                            "view must be terminal, thread or chatter, not '{p}'"
                        )))
                    }
                }
            }
            p => {
                return Err(usage(&format!(
                    "unexpected extra argument '{p}' \u{2014} {VIEW_USAGE}"
                )))
            }
        }
        i += 1;
    }
    need(
        &project,
        "view requires a <project> (add terminal, thread or chatter to set it)",
    )?;
    if view == "default" && agent.is_empty() {
        return Err(usage("default clears one agent's own view, so it needs --agent <name>; a project's own view is terminal, thread or chatter"));
    }
    Ok((
        Verb::View {
            project,
            view,
            agent,
        },
        j,
    ))
}

fn parse_msg(rest: &[&str]) -> Result<(Verb, bool), String> {
    let mut j = false;
    let mut words: Vec<&str> = Vec::new();
    // After a bare `--` every word is a positional, as typed.
    let mut dashes = false;
    for w in rest {
        if dashes {
            words.push(w);
        } else if *w == "--" {
            dashes = true;
        } else if *w == "--json" {
            j = true;
        } else if *w != "-" && w.starts_with('-') {
            return Err(usage(&format!(
                "unknown flag '{w}' \u{2014} {}",
                cli!("msg [<project>] <text> [--json]")
            )));
        } else {
            words.push(w);
        }
    }
    match words.as_slice() {
        [] => Err(usage(
            "msg requires <text> (or \"-\" to read it from stdin)",
        )),
        [text] => Ok((
            Verb::Msg {
                project: String::new(),
                text: (*text).to_string(),
                two: false,
                stdin: *text == "-",
            },
            j,
        )),
        [project, text] => Ok((
            Verb::Msg {
                project: (*project).to_string(),
                text: (*text).to_string(),
                two: true,
                stdin: *text == "-",
            },
            j,
        )),
        many => Err(usage(&format!(
            "too many arguments \u{2014} did you mean: {} project msg \"{}\"?",
            k2_client::brand::CLI_NAME,
            many.join(" ")
        ))),
    }
}

const READ_USAGE: &str = cli!("read [<project>] [--after <ts>] [--limit <n>] [--json]");

fn parse_read(rest: &[&str]) -> Result<(Verb, bool), String> {
    let (mut project, mut after, mut limit, mut j) =
        (String::new(), String::new(), String::new(), false);
    let mut i = 0;
    while let Some(&w) = rest.get(i) {
        match w {
            "--after" => {
                after = rest
                    .get(i + 1)
                    .ok_or_else(|| usage("--after requires a unix-seconds timestamp"))?
                    .to_string();
                i += 1;
            }
            "--limit" => {
                limit = rest
                    .get(i + 1)
                    .ok_or_else(|| usage("--limit requires a number"))?
                    .to_string();
                i += 1;
            }
            "--json" => j = true,
            f if f.starts_with('-') => {
                return Err(usage(&format!("unknown flag '{f}' \u{2014} {READ_USAGE}")))
            }
            p if project.is_empty() => project = p.to_string(),
            p => {
                return Err(usage(&format!(
                    "unexpected extra argument '{p}' \u{2014} {READ_USAGE}"
                )))
            }
        }
        i += 1;
    }
    Ok((
        Verb::Read {
            project,
            after,
            limit,
        },
        j,
    ))
}

const ACCESS_USAGE: &str = cli!("access on|off [--workspace <ws>] [--json]");

fn parse_access(rest: &[&str]) -> Result<(Verb, bool), String> {
    let (mut state, mut workspace, mut j) = (String::new(), String::new(), false);
    let mut i = 0;
    while let Some(&w) = rest.get(i) {
        match w {
            "on" | "off" => {
                if !state.is_empty() {
                    return Err(usage("pass on or off once"));
                }
                state = w.to_string();
            }
            "--workspace" => {
                workspace = rest
                    .get(i + 1)
                    .ok_or_else(|| usage("--workspace requires a workspace name or path"))?
                    .to_string();
                i += 1;
            }
            "--json" => j = true,
            other => {
                return Err(usage(&format!(
                    "unexpected argument '{other}' \u{2014} {ACCESS_USAGE}"
                )))
            }
        }
        i += 1;
    }
    if state.is_empty() {
        return Err(usage(&format!(
            "access needs on or off \u{2014} {ACCESS_USAGE}"
        )));
    }
    Ok((
        Verb::Access {
            on: state == "on",
            workspace,
        },
        j,
    ))
}

// ---------------------------------------------------------------------
// The run

/// `k5 project ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
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
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let form = parse(&words);
    if !python::family_native(python::here()) {
        return None;
    }
    for name in k2_client::env_names::AGENT_NAME.iter().chain(&["HOME"]) {
        if let Some(v) = std::env::var_os(name) {
            v.to_str()?;
        }
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    curl_port(settings.port_text.as_deref()?)?;
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock.as_deref(), would_refuse, TOOL) {
        return Some(stop);
    }
    Some(run_form(form, settings, would_refuse))
}

fn help() -> &'static str {
    crate::text::help("project").expect("cli/text/help/project.txt is embedded")
}

fn run_form(form: Form, mut settings: Settings, would_refuse: bool) -> Exit {
    let (verb, json) = match form {
        Form::Help => return Shown::out(help()).print(),
        Form::Unknown(v) => {
            out::stderr_line(&usage(&format!(
                "unknown command '{v}' \u{2014} see {} project --help",
                k2_client::brand::CLI_NAME
            )));
            return Exit::Usage;
        }
        Form::Verb(Err(l)) => {
            out::stderr_line(&l);
            return Exit::Usage;
        }
        Form::Verb(Ok(v)) => v,
    };
    let verb = match verb {
        Verb::Msg {
            project,
            two,
            stdin: true,
            ..
        } => {
            let mut raw = Vec::new();
            let _ = std::io::stdin().lock().read_to_end(&mut raw);
            let held = super::connections::held(&raw);
            Verb::Msg {
                project,
                text: String::from_utf8_lossy(&held).into_owned(),
                two,
                stdin: true,
            }
        }
        v => v,
    };
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut opener = Opener::from_client(&client);
    if let Some(p) = &settings.port_text {
        opener.netloc = format!("{}:{p}", settings.host);
    }
    let agent = k2_client::env_names::first_nonempty_in(k2_client::env_names::AGENT_NAME, |n| {
        std::env::var(n).ok()
    })
    .map(|(_, v)| v)
    .unwrap_or_default();
    let mut py = Py {
        opener,
        ws_path: settings.project.clone(),
        agent,
        home: std::env::var("HOME").ok(),
        json,
        permission: None,
        out: PyOut::default(),
        now: now(),
    };
    let r = py.verb(&verb);
    let o = std::mem::take(&mut py.out);
    match r {
        Ok(()) => o.done(Exit::Ok),
        Err(Halt::Exit(e)) => o.done(e),
        Err(Halt::Raise(r)) => Shown {
            stdout: o.stdout,
            stderr: o.stderr + &traceback_stdin(&r),
            exit: Exit::Error,
        },
    }
    .print()
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

// ---------------------------------------------------------------------
// The Python block

/// How the block stops before its end: `sys.exit(n)`, or an exception.
#[derive(Debug)]
pub enum Halt {
    Exit(Exit),
    Raise(Raise),
}

impl From<Raise> for Halt {
    fn from(r: Raise) -> Halt {
        Halt::Raise(r)
    }
}

type R<T> = Result<T, Halt>;

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// `dumps(obj)`: compact, `ensure_ascii=False`; `None` when a string holds
/// half of a surrogate pair (Python's print would fail on it).
fn compact(v: &Value) -> Option<String> {
    json::to_string(
        v,
        &Options {
            compact: true,
            ensure_ascii: false,
            ..Options::default()
        },
    )
    .ok()
}

/// `str(v)`.
fn st(v: &Value) -> String {
    py::str_of(v)
}

/// `v.get(key)` on what Python must find a dict.
fn at(v: &Value, key: &str, line: u32) -> R<Option<Value>> {
    match v {
        Value::Object(pairs) => Ok(get(pairs, key).cloned()),
        other => Err(Raise::no_attribute(line, other, "get").into()),
    }
}

/// `v.get(key, default)`.
fn at_or(v: &Value, key: &str, default: Value, line: u32) -> R<Value> {
    Ok(at(v, key, line)?.unwrap_or(default))
}

/// `v.get(key)`, `None` as `Null`.
fn at_v(v: &Value, key: &str, line: u32) -> R<Value> {
    at_or(v, key, Value::Null, line)
}

fn truthy(v: &Value) -> bool {
    py::truthy(v)
}

/// `a or b`.
fn or(a: Value, b: Value) -> Value {
    if truthy(&a) {
        a
    } else {
        b
    }
}

/// `a == b` for JSON values, as Python compares them.
fn eq(a: &Value, b: &Value) -> bool {
    super::feedback::ask::py_eq(a, b)
}

/// `"%-<w>s" % v`.
fn ljust(v: &Value, w: usize) -> String {
    PyStr::of(v).ljust(w)
}

/// `s.rstrip()`.
fn rstrip(t: &str) -> String {
    t.trim_end_matches(py_space).to_string()
}

/// `int(v)` as `%s` writes it, or the exception at `line`.
fn int_of(v: &Value, line: u32) -> R<Value> {
    py_int(v)
        .map(Value::Int)
        .map_err(|e| Raise::at(line, e).into())
}

/// `os.path.basename(p)`.
fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// `os.path.realpath(p)` (not strict), from `cwd` for a relative one.
pub fn realpath(p: &str) -> String {
    fn join(a: &str, b: &str) -> String {
        if b.starts_with('/') {
            b.to_string()
        } else if a.is_empty() || a.ends_with('/') {
            format!("{a}{b}")
        } else {
            format!("{a}/{b}")
        }
    }
    fn split(p: &str) -> (String, String) {
        let i = p.rfind('/').map_or(0, |i| i + 1);
        let (head, tail) = p.split_at(i);
        let trimmed = head.trim_end_matches('/');
        let head = if trimmed.is_empty() { head } else { trimmed };
        (head.to_string(), tail.to_string())
    }
    fn walk(
        mut path: String,
        rest: &str,
        seen: &mut std::collections::HashMap<String, Option<String>>,
    ) -> (String, bool) {
        let mut rest = rest.to_string();
        if rest.starts_with('/') {
            rest = rest[1..].to_string();
            path = "/".into();
        }
        while !rest.is_empty() {
            let (name, tail) = match rest.split_once('/') {
                Some((n, t)) => (n.to_string(), t.to_string()),
                None => (rest.clone(), String::new()),
            };
            rest = tail;
            if name.is_empty() || name == "." {
                continue;
            }
            if name == ".." {
                if path.is_empty() {
                    path = "..".into();
                } else {
                    let (head, n) = split(&path);
                    path = if n == ".." {
                        join(&join(&head, ".."), "..")
                    } else {
                        head
                    };
                }
                continue;
            }
            let newpath = join(&path, &name);
            let link =
                std::fs::symlink_metadata(&newpath).is_ok_and(|m| m.file_type().is_symlink());
            if !link {
                path = newpath;
                continue;
            }
            if let Some(known) = seen.get(&newpath) {
                match known {
                    Some(p) => {
                        path = p.clone();
                        continue;
                    }
                    None => return (join(&newpath, &rest), false),
                }
            }
            seen.insert(newpath.clone(), None);
            let target = std::fs::read_link(&newpath)
                .map(|t| t.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (p, ok) = walk(path, &target, seen);
            if !ok {
                return (join(&p, &rest), false);
            }
            seen.insert(newpath, Some(p.clone()));
            path = p;
        }
        (path, true)
    }
    let (path, _) = walk(String::new(), p, &mut std::collections::HashMap::new());
    let abs = if path.starts_with('/') {
        path
    } else {
        let cwd = std::env::current_dir()
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_default();
        join(&cwd, &path)
    };
    super::feedback::write::normpath(&abs)
}

/// The block's state.
struct Py {
    opener: Opener,
    ws_path: String,
    agent: String,
    home: Option<String>,
    json: bool,
    /// `LAST_PERMISSION[0]`.
    permission: Option<Value>,
    out: PyOut,
    now: f64,
}

/// `permission_lines(p)`.
fn permission_lines(p: &Value) -> Vec<String> {
    let Value::Object(pairs) = p else {
        return Vec::new();
    };
    if let Some(Value::Str(c)) = get(pairs, "command") {
        let c = st(&Value::Str(c.clone()));
        if !c.is_empty() {
            return vec![format!("Ask with: {c}")];
        }
    }
    if get(pairs, "requestable") == Some(&Value::Bool(false)) {
        let mut out = Vec::new();
        if let Some(h @ Value::Str(_)) = get(pairs, "hint") {
            let h = st(h);
            if !h.is_empty() {
                out.push(h);
            }
        }
        out.push("It can't be requested.".into());
        return out;
    }
    Vec::new()
}

impl Py {
    fn line(&mut self, t: &str) {
        self.out.line(t);
    }

    /// `print(dumps(v))`.
    fn print_json(&mut self, v: &Value, at_line: u32) -> R<()> {
        match compact(v) {
            Some(t) => {
                self.line(&t);
                Ok(())
            }
            None => Err(Raise::at(
                at_line,
                "UnicodeEncodeError: 'utf-8' codec can't encode character: surrogates not allowed",
            )
            .into()),
        }
    }

    /// `die(err)`.
    fn die(&mut self, err: Value) -> Halt {
        let err = match err {
            Value::Str(_) => obj(vec![("code", s("error")), ("hint", err)]),
            other => other,
        };
        let Value::Object(pairs) = &err else {
            return Raise::at(line::DIE, Raise::no_attribute(0, &err, "get").exception).into();
        };
        let mut code = get(pairs, "code").cloned().unwrap_or(s("error"));
        if code == s("ambiguous_id") {
            code = s("ambiguous");
        }
        let hint = get(pairs, "hint").cloned().unwrap_or(s(""));
        let text = compact(&obj(vec![(
            "error",
            obj(vec![("code", code.clone()), ("hint", hint)]),
        )]));
        let Some(text) = text else {
            return Raise::at(
                line::DIE,
                "UnicodeEncodeError: 'utf-8' codec can't encode character: surrogates not allowed",
            )
            .into();
        };
        self.out.stderr.push_str(&text);
        self.out.stderr.push('\n');
        if let Some(p) = &self.permission {
            for l in permission_lines(p) {
                self.out.stderr.push_str(&l);
                self.out.stderr.push('\n');
            }
        }
        let is = |w: &str| eq(&code, &s(w));
        Halt::Exit(
            if [
                "not_found",
                "ambiguous",
                "ambiguous_id",
                "no_project",
                "ambiguous_project",
            ]
            .iter()
            .any(|w| is(w))
            {
                Exit::NotFound
            } else if is("usage") {
                Exit::Usage
            } else {
                Exit::Error
            },
        )
    }

    fn die_with(&mut self, code: &str, hint: &str) -> Halt {
        self.die(obj(vec![("code", s(code)), ("hint", s(hint))]))
    }

    /// `request(method, route, params, body)`.
    fn request(
        &mut self,
        method: Method,
        route: &str,
        params: &[(&str, String)],
        body: Option<&Value>,
    ) -> R<Option<Value>> {
        let mut req = Request::new(method, route, TIMEOUT);
        if !params.is_empty() {
            req = req.query(params.iter().map(|(k, v)| (*k, v.clone())));
        }
        if let Some(b) = body {
            let data = compact(b).unwrap_or_default();
            req = req
                .data(data.into_bytes())
                .header("Content-Type", "application/json");
        }
        if let Some(t) = self.opener.token() {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        // `_K2NoRedirect`: a redirect's line, then `SystemExit(3)`.
        let out = match self.opener.open_ruled(&req) {
            Ok(out) => out,
            Err(r) => {
                self.out.stderr.push_str(&r.line);
                self.out.stderr.push('\n');
                return Err(Halt::Exit(Refused::EXIT));
            }
        };
        Ok(match out {
            Outcome::Answer { body: Ok(b), .. } => json::parse(&decode(&b)).ok(),
            Outcome::HttpError { code, body } => {
                match body.ok().and_then(|b| json::parse(&decode(&b)).ok()) {
                    Some(v) => {
                        if let Value::Object(pairs) = &v {
                            if let Some(p @ Value::Object(_)) = get(pairs, "permission") {
                                self.permission = Some(p.clone());
                            }
                        }
                        Some(v)
                    }
                    None => Some(obj(vec![(
                        "error",
                        obj(vec![
                            ("code", s("error")),
                            ("hint", s(&format!("HTTP {code} on {route}"))),
                        ]),
                    )])),
                }
            }
            Outcome::Answer { body: Err(_), .. } | Outcome::Raised(_) => None,
        })
    }

    fn unreachable(&mut self) -> Halt {
        self.die_with(
            "daemon_unreachable",
            concat!(
                "no response from the ",
                product_name!(),
                " daemon (is it running?)"
            ),
        )
    }

    /// `call(method, route, params, body)`.
    fn call(
        &mut self,
        method: Method,
        route: &str,
        params: &[(&str, String)],
        body: Option<&Value>,
    ) -> R<Value> {
        let Some(d) = self.request(method, route, params, body)? else {
            return Err(self.unreachable());
        };
        let err = at_v(&d, "error", line::CALL)?;
        if truthy(&err) {
            return Err(self.die(err));
        }
        if !truthy(&at_v(&d, "ok", line::CALL)?) {
            let shown: String = compact(&d).unwrap_or_default().chars().take(200).collect();
            return Err(self.die_with("error", &format!("unexpected daemon response: {shown}")));
        }
        Ok(d)
    }

    fn get_(&mut self, route: &str, params: &[(&str, String)]) -> R<Value> {
        self.call(Method::Get, route, params, None)
    }

    fn post(&mut self, route: &str, body: Value) -> R<Value> {
        self.call(Method::Post, route, &[], Some(&body))
    }

    /// `fetch_show(selector)`.
    fn fetch_show(&mut self, selector: &Value) -> R<Value> {
        self.get_(SHOW, &[("group", st(selector))])
    }

    fn members(show: &Value, line: u32) -> R<Vec<Value>> {
        let m = at_or(show, "members", Value::Array(Vec::new()), line)?;
        Ok(py_iter(&m, line)?)
    }

    /// `member_by_id(show, id)`.
    fn member_by_id(show: &Value, id: &Value, line: u32) -> R<Option<Value>> {
        for m in Self::members(show, line)? {
            if eq(&at_v(&m, "workspaceId", line)?, id) {
                return Ok(Some(m));
            }
        }
        Ok(None)
    }

    fn poc_member(show: &Value, line: u32) -> R<Option<Value>> {
        let pid = at_v(show, "pocWorkspaceId", line)?;
        if truthy(&pid) {
            Self::member_by_id(show, &pid, line)
        } else {
            Ok(None)
        }
    }

    fn poc_name(show: &Value, line: u32) -> R<Option<Value>> {
        match Self::poc_member(show, line)? {
            Some(m) => Ok(Some(mname(&m, line)?)),
            None => Ok(None),
        }
    }

    /// `same_path(a, b)`.
    fn same_path(a: &Value, b: &str) -> bool {
        match a {
            Value::Str(_) => realpath(&st(a)) == realpath(b),
            other => eq(other, &s(b)),
        }
    }

    /// `my_member(show)`.
    fn my_member(&self, show: &Value, line: u32) -> R<Option<Value>> {
        for m in Self::members(show, line)? {
            let p = or(at_v(&m, "path", line)?, s(""));
            if truthy(&p) && !self.ws_path.is_empty() && Self::same_path(&p, &self.ws_path) {
                return Ok(Some(m));
            }
        }
        Ok(None)
    }

    /// `detect_project()`.
    fn detect_project(&mut self, line: u32) -> R<Value> {
        let d = self.get_(LIST, &[])?;
        let groups = at_or(&d, "groups", Value::Array(Vec::new()), line)?;
        let mut hits = Vec::new();
        for g in py_iter(&groups, line)? {
            let id = at_v(&g, "id", line)?;
            let show = self.fetch_show(&id)?;
            if self.my_member(&show, line)?.is_some() {
                hits.push(show);
            }
        }
        if hits.is_empty() {
            return Err(self.die_with(
                "no_project",
                "this workspace is not a member of any project; pass the project name",
            ));
        }
        if hits.len() > 1 {
            let mut names = Vec::new();
            for (i, h) in hits.iter().enumerate() {
                match at_or(h, "name", s("?"), line)? {
                    v @ Value::Str(_) => names.push(st(&v)),
                    other => {
                        return Err(Raise::at(
                            line,
                            format!(
                                "TypeError: sequence item {i}: expected str instance, {} found",
                                py::type_name(&other)
                            ),
                        )
                        .into())
                    }
                }
            }
            return Err(self.die_with(
                "ambiguous_project",
                &format!(
                    "member of: {} \u{2014} pass the project name",
                    names.join(", ")
                ),
            ));
        }
        Ok(hits.remove(0))
    }

    /// `home_path(p)`.
    fn home_path(&self, p: &Value, line: u32) -> R<Value> {
        if truthy(p) {
            let Value::Str(_) = p else {
                return Err(Raise::no_attribute(line, p, "startswith").into());
            };
            let home = self
                .home
                .as_deref()
                .map(|h| {
                    let t = h.trim_end_matches('/');
                    if t.is_empty() {
                        "/".to_string()
                    } else {
                        t.to_string()
                    }
                })
                .unwrap_or_else(|| "~".into());
            let text = st(p);
            if let Some(rest) = text.strip_prefix(&format!("{home}/")) {
                return Ok(s(&format!("~/{rest}")));
            }
            return Ok(p.clone());
        }
        Ok(s("?"))
    }

    /// `print_message_rows(msgs)`.
    fn message_rows(&mut self, msgs: &Value, line: u32) -> R<()> {
        for m in py_iter(msgs, line)? {
            let body = at_or(&m, "body", s(""), line)?;
            let mut lines = PyStr::of(&body).splitlines();
            if lines.is_empty() {
                lines.push(PyStr::default());
            }
            let author = at_or(&m, "author", s("?"), line)?;
            let when = ago(
                &at_or(&m, "createdAt", Value::Int("0".into()), line)?,
                self.now,
                line,
            )?;
            self.line(&format!(
                "  {}{}{}",
                ljust(&author, 9),
                PyStr::from(format!("({when})").as_str()).ljust(11),
                lines[0].render()
            ));
            for extra in &lines[1..] {
                self.line(&format!("{}{}", " ".repeat(22), extra.render()));
            }
        }
        Ok(())
    }

    fn verb(&mut self, v: &Verb) -> R<()> {
        match v {
            Verb::Create { name } => self.create(name),
            Verb::List => self.list(),
            Verb::Show { project } => self.show(project),
            Verb::Add { project, workspace } => self.add(project, workspace),
            Verb::Remove { project, workspace } => self.remove(project, workspace),
            Verb::Poc { project, workspace } => self.poc(project, workspace),
            Verb::Msg {
                project,
                text,
                two,
                stdin,
            } => self.msg(project, text, *two, *stdin),
            Verb::Read {
                project,
                after,
                limit,
            } => self.read(project, after, limit),
            Verb::Rename { project, name } => self.rename(project, name),
            Verb::Delete { project } => self.delete(project),
            Verb::Pin { project, state } => self.pin(project, state),
            Verb::View {
                project,
                view,
                agent,
            } => self.view(project, view, agent),
            Verb::Access { on, workspace } => self.access(*on, workspace),
        }
    }

    fn create(&mut self, name: &str) -> R<()> {
        let l = line::CREATE;
        let d = self.post("/cli/project-group/create", obj(vec![("name", s(name))]))?;
        if self.json {
            let members = int_of(&or(at_v(&d, "memberCount", l)?, Value::Int("0".into())), l)?;
            let out = obj(vec![
                ("ok", Value::Bool(true)),
                ("id", at_v(&d, "id", l)?),
                ("name", at_v(&d, "name", l)?),
                ("members", members),
                ("poc", Value::Null),
            ]);
            return self.print_json(&out, l);
        }
        let n = st(&at_v(&d, "name", l)?);
        self.line(&format!("Created project {n} (no members yet)."));
        self.line(&format!(
            "Add the first member \u{2014} it will become the PoC: {} project add {n} <workspace>",
            k2_client::brand::CLI_NAME
        ));
        Ok(())
    }

    fn list(&mut self) -> R<()> {
        let l = line::LIST;
        let d = self.get_(LIST, &[])?;
        let groups = at_or(&d, "groups", Value::Array(Vec::new()), l)?;
        let mut rows = Vec::new();
        for g in py_iter(&groups, l)? {
            let mut pn = Value::Null;
            if truthy(&at_v(&g, "pocWorkspaceId", l)?) {
                let show = self.fetch_show(&at_v(&g, "id", l)?)?;
                pn = Self::poc_name(&show, l)?.unwrap_or(Value::Null);
            }
            let members = int_of(&or(at_v(&g, "memberCount", l)?, Value::Int("0".into())), l)?;
            rows.push(obj(vec![
                ("id", at_v(&g, "id", l)?),
                ("name", or(at_v(&g, "name", l)?, s("?"))),
                ("members", members),
                ("poc", pn),
                ("pinned", Value::Bool(truthy(&at_v(&g, "pinned", l)?))),
            ]));
        }
        if self.json {
            let out = obj(vec![
                ("ok", Value::Bool(true)),
                ("projects", Value::Array(rows)),
            ]);
            return self.print_json(&out, l);
        }
        if rows.is_empty() {
            self.line(&format!(
                "No projects yet. Create one: {} project create <name>",
                k2_client::brand::CLI_NAME
            ));
            return Ok(());
        }
        let field = |r: &Value, k: &str| at_v(r, k, l).unwrap_or(Value::Null);
        let mut w = 4;
        for r in &rows {
            w = w.max(py_len(&field(r, "name"), l)?);
        }
        let w = w + 2;
        self.line(&rstrip(&format!(
            "    {}{}POC",
            ljust(&s("NAME"), w),
            ljust(&s("MEMBERS"), 9)
        )));
        for r in &rows {
            let poc = or(field(r, "poc"), s(""));
            let lead = if truthy(&field(r, "pinned")) {
                " *  "
            } else {
                "    "
            };
            self.line(&rstrip(&format!(
                "{lead}{}{}{}",
                ljust(&field(r, "name"), w),
                ljust(&field(r, "members"), 9),
                st(&poc)
            )));
        }
        Ok(())
    }

    fn show(&mut self, project: &str) -> R<()> {
        let l = line::SHOW;
        let show = self.fetch_show(&s(project))?;
        let id = st(&at_v(&show, "id", l)?);
        let recent = self.get_(MESSAGES, &[("group", id), ("limit", "3".into())])?;
        let msgs = at_or(&recent, "messages", Value::Array(Vec::new()), l)?;
        let members = Self::members(&show, l)?;
        let pid = at_v(&show, "pocWorkspaceId", l)?;
        let pm = Self::poc_member(&show, l)?;
        if self.json {
            let poc = match &pm {
                Some(m) => obj(vec![("name", mname(m, l)?), ("path", at_v(m, "path", l)?)]),
                None => Value::Null,
            };
            let mut ms = Vec::new();
            for m in &members {
                ms.push(obj(vec![
                    ("name", mname(m, l)?),
                    ("path", at_v(m, "path", l)?),
                    ("isPoc", Value::Bool(eq(&at_v(m, "workspaceId", l)?, &pid))),
                ]));
            }
            let mut rs = Vec::new();
            for m in py_iter(&msgs, l)? {
                rs.push(obj(vec![
                    ("id", at_v(&m, "id", l)?),
                    ("author", at_v(&m, "author", l)?),
                    ("body", at_v(&m, "body", l)?),
                    ("at", at_v(&m, "createdAt", l)?),
                ]));
            }
            let out = obj(vec![
                ("ok", Value::Bool(true)),
                ("id", at_v(&show, "id", l)?),
                ("name", at_v(&show, "name", l)?),
                ("pinned", Value::Bool(truthy(&at_v(&show, "pinned", l)?))),
                ("poc", poc),
                ("members", Value::Array(ms)),
                ("recent", Value::Array(rs)),
            ]);
            return self.print_json(&out, l);
        }
        let name = or(at_v(&show, "name", l)?, s("?"));
        let pinned = if truthy(&at_v(&show, "pinned", l)?) {
            "  (pinned)"
        } else {
            ""
        };
        self.line(&format!("project : {}{pinned}", st(&name)));
        let poc = Self::poc_name(&show, l)?
            .filter(truthy)
            .unwrap_or(s("(none)"));
        self.line(&format!("poc     : {}", st(&poc)));
        self.line(&format!("members : {}", members.len()));
        if !members.is_empty() {
            let mut names = Vec::new();
            let mut paths = Vec::new();
            for m in &members {
                names.push(mname(m, l)?);
            }
            for m in &members {
                paths.push(self.home_path(&at_v(m, "path", l)?, l)?);
            }
            let mut nw = 0;
            let mut pw = 0;
            for n in &names {
                nw = nw.max(py_len(n, l)?);
            }
            for p in &paths {
                pw = pw.max(py_len(p, l)?);
            }
            for ((m, n), p) in members.iter().zip(&names).zip(&paths) {
                let marker = if eq(&at_v(m, "workspaceId", l)?, &pid) {
                    "(PoC)"
                } else {
                    ""
                };
                self.line(&rstrip(&format!(
                    "  {}{}{marker}",
                    ljust(n, nw + 4),
                    ljust(p, pw + 4)
                )));
            }
        }
        if truthy(&msgs) {
            self.line("");
            self.line(&format!(
                "last messages (see `{} project read {}` for more):",
                k2_client::brand::CLI_NAME,
                st(&name)
            ));
            self.message_rows(&msgs, l)?;
        }
        Ok(())
    }

    fn add(&mut self, project: &str, workspace: &str) -> R<()> {
        let l = line::ADD;
        let d = self.post(
            "/cli/project-group/add-member",
            obj(vec![("group", s(project)), ("workspace", s(workspace))]),
        )?;
        let show = self.fetch_show(&at_v(&d, "groupId", l)?)?;
        let m = Self::member_by_id(&show, &at_v(&d, "workspaceId", l)?, l)?;
        let wsname = match m.filter(truthy) {
            Some(m) => mname(&m, l)?,
            None => s(workspace),
        };
        let name = or(at_v(&show, "name", l)?, s("?"));
        let count = Self::members(&show, l)?.len();
        let pn = Self::poc_name(&show, l)?.unwrap_or(Value::Null);
        let already = truthy(&at_v(&d, "alreadyMember", l)?);
        let became = truthy(&at_v(&d, "becamePoc", l)?);
        if self.json {
            let out = obj(vec![
                ("ok", Value::Bool(true)),
                ("changed", Value::Bool(!already)),
                ("project", name),
                ("workspace", wsname),
                ("members", Value::Int(count.to_string())),
                ("poc", pn),
                ("becamePoc", Value::Bool(became)),
            ]);
            return self.print_json(&out, l);
        }
        let (ws, n) = (st(&wsname), st(&name));
        if already {
            self.line(&format!(
                "{ws} is already a member of {n} \u{2014} nothing to do."
            ));
        } else if became {
            self.line(&format!(
                "Added {ws} to {n} (first member \u{2014} {ws} is now the PoC)."
            ));
        } else {
            self.line(&format!(
                "Added {ws} to {n} (member {count} of {count} \u{b7} PoC: {}).",
                st(&or(pn, s("?")))
            ));
        }
        Ok(())
    }

    fn remove(&mut self, project: &str, workspace: &str) -> R<()> {
        let l = line::REMOVE;
        let show = self.fetch_show(&s(project))?;
        let d = self.post(
            "/cli/project-group/remove-member",
            obj(vec![
                ("group", at_v(&show, "id", l)?),
                ("workspace", s(workspace)),
            ]),
        )?;
        let m = Self::member_by_id(&show, &at_v(&d, "workspaceId", l)?, l)?;
        let wsname = match m.filter(truthy) {
            Some(m) => mname(&m, l)?,
            None => s(workspace),
        };
        let name = or(at_v(&show, "name", l)?, s("?"));
        if self.json {
            let out = obj(vec![
                ("ok", Value::Bool(true)),
                ("project", name),
                ("workspace", wsname),
            ]);
            return self.print_json(&out, l);
        }
        self.line(&format!("Removed {} from {}.", st(&wsname), st(&name)));
        Ok(())
    }

    fn poc(&mut self, project: &str, target: &str) -> R<()> {
        let l = line::POC;
        let show = self.fetch_show(&s(project))?;
        let name = or(at_v(&show, "name", l)?, s("?"));
        if target.is_empty() {
            let pn = Self::poc_name(&show, l)?.unwrap_or(Value::Null);
            if self.json {
                let out = obj(vec![
                    ("ok", Value::Bool(true)),
                    ("project", name),
                    ("poc", pn),
                ]);
                return self.print_json(&out, l);
            }
            if truthy(&pn) {
                self.line(&st(&pn));
            }
            return Ok(());
        }
        let old = Self::poc_name(&show, l)?.unwrap_or(Value::Null);
        let d = self.post(
            "/cli/project-group/set-poc",
            obj(vec![
                ("group", at_v(&show, "id", l)?),
                ("workspace", s(target)),
            ]),
        )?;
        let m = Self::member_by_id(&show, &at_v(&d, "pocWorkspaceId", l)?, l)?;
        let new = match m.filter(truthy) {
            Some(m) => mname(&m, l)?,
            None => s(target),
        };
        if self.json {
            let out = obj(vec![
                ("ok", Value::Bool(true)),
                ("project", name),
                ("poc", new),
            ]);
            return self.print_json(&out, l);
        }
        let new = st(&new);
        self.line(&format!(
            "PoC of {}: {} \u{2192} {new}. New chat posts now inject into {new}'s session.",
            st(&name),
            st(&or(old, s("(none)")))
        ));
        Ok(())
    }

    fn msg(&mut self, proj: &str, text: &str, two: bool, from_stdin: bool) -> R<()> {
        let l = line::MSG;
        let show = if !proj.is_empty() {
            let Some(d) = self.request(Method::Get, SHOW, &[("group", proj.to_string())], None)?
            else {
                return Err(self.unreachable());
            };
            let err = at_v(&d, "error", l)?;
            if truthy(&err) {
                let mut err = match err {
                    Value::Str(_) => obj(vec![("code", s("error")), ("hint", err)]),
                    other => other,
                };
                if eq(&at_v(&err, "code", l)?, &s("not_found")) && two && !from_stdin {
                    let whole = format!("{proj} {text}").replace('"', "\\\"");
                    let old = st(&at_or(&err, "hint", s(""), l)?);
                    let hint = format!(
                        "{old} \u{2014} if that was part of your message, quote the whole thing: {} project msg \"{whole}\"",
                        k2_client::brand::CLI_NAME
                    );
                    if let Value::Object(pairs) = &mut err {
                        match pairs.iter_mut().find(|(k, _)| *k == Text::from("hint")) {
                            Some((_, v)) => *v = s(&hint),
                            None => pairs.push((Text::from("hint"), s(&hint))),
                        }
                    }
                }
                return Err(self.die(err));
            }
            d
        } else {
            self.detect_project(l)?
        };
        let author = match self.my_member(&show, l)? {
            Some(me) => or(at_v(&me, "name", l)?, Value::Null),
            None => Value::Null,
        };
        let author = if truthy(&author) {
            author
        } else if let Some(me) = self.my_member(&show, l)? {
            mname(&me, l)?
        } else if !self.agent.is_empty() {
            s(&self.agent)
        } else {
            let b = basename(self.ws_path.trim_end_matches('/'));
            s(if b.is_empty() { "?" } else { b })
        };
        let d = self.post(
            "/cli/project-group/msg",
            obj(vec![
                ("group", at_v(&show, "id", l)?),
                ("body", s(text)),
                ("author", author),
            ]),
        )?;
        let name = or(
            or(at_v(&d, "groupName", l)?, at_v(&show, "name", l)?),
            s("?"),
        );
        let delivered = truthy(&at_v(&d, "delivered", l)?);
        let mut reason = at_v(&d, "deliveryReason", l)?;
        if !delivered
            && reason != Value::Null
            && !["author-is-poc", "no_poc", "queued"]
                .iter()
                .any(|w| eq(&reason, &s(w)))
        {
            reason = s("delivery-failed");
        }
        let n = st(&name);
        if self.json {
            let mut out = vec![
                ("ok", Value::Bool(true)),
                ("id", at_v(&d, "id", l)?),
                ("project", name),
                ("author", at_v(&d, "author", l)?),
                ("delivered", Value::Bool(delivered)),
            ];
            if !delivered && truthy(&reason) {
                out.push(("deliveryReason", reason));
            }
            let sid = at_v(&d, "deliveredSessionId", l)?;
            if truthy(&sid) {
                out.push(("deliveredSessionId", sid));
            }
            return self.print_json(&obj(out), l);
        }
        let poc = || -> R<String> {
            Ok(st(&or(
                Self::poc_name(&show, l)?.unwrap_or(Value::Null),
                s("?"),
            )))
        };
        if delivered {
            let p = poc()?;
            self.line(&format!("Posted to {n} \u{2014} delivered to PoC {p}."));
        } else if reason == s("author-is-poc") {
            self.line(&format!(
                "Posted to {n}. (You are the PoC \u{2014} your own posts are never injected back to you.)"
            ));
        } else if reason == s("delivery-failed") {
            self.line(&format!(
                "Posted to {n} \u{2014} stored, but the PoC's session could not be reached"
            ));
            self.line(&format!(
                "(it will catch up with `{} project read`).",
                k2_client::brand::CLI_NAME
            ));
        } else if reason == s("queued") {
            let p = poc()?;
            self.line(&format!(
                "Posted to {n} \u{2014} on its way to PoC {p} (still being delivered)."
            ));
        } else {
            self.line(&format!("Posted to {n}."));
        }
        Ok(())
    }

    fn read(&mut self, proj: &str, after: &str, limit: &str) -> R<()> {
        let l = line::READ;
        let show = if proj.is_empty() {
            self.detect_project(l)?
        } else {
            self.fetch_show(&s(proj))?
        };
        let mut params = vec![("group", st(&at_v(&show, "id", l)?))];
        if !after.is_empty() {
            params.push(("after", after.to_string()));
        }
        if !limit.is_empty() {
            params.push(("limit", limit.to_string()));
        }
        let d = self.get_(MESSAGES, &params)?;
        let msgs = at_or(&d, "messages", Value::Array(Vec::new()), l)?;
        let name = or(at_v(&show, "name", l)?, s("?"));
        let truncated = truthy(&at_v(&d, "truncated", l)?);
        let list = py_iter(&msgs, l)?;
        let last_at = match list.last() {
            Some(m) => Some(at_v(m, "createdAt", l)?),
            None => None,
        };
        if self.json {
            let mut ms = Vec::new();
            for m in &list {
                ms.push(obj(vec![
                    ("id", at_v(m, "id", l)?),
                    ("author", at_v(m, "author", l)?),
                    ("body", at_v(m, "body", l)?),
                    ("at", at_v(m, "createdAt", l)?),
                ]));
            }
            let mut out = vec![
                ("ok", Value::Bool(true)),
                ("project", name),
                ("messages", Value::Array(ms)),
            ];
            if truncated {
                out.push(("truncated", Value::Bool(true)));
                if let Some(at) = &last_at {
                    out.push((
                        "hint",
                        s(&format!(
                            "more messages remain \u{2014} continue with --after {}",
                            st(at)
                        )),
                    ));
                }
            }
            return self.print_json(&obj(out), l);
        }
        let n = st(&name);
        self.line(&format!(
            "project {n} \u{2014} {} most recent messages (oldest first):",
            py_len(&msgs, l)?
        ));
        self.line("");
        self.message_rows(&msgs, l)?;
        if truncated {
            if let Some(at) = last_at {
                self.line("");
                self.line(&format!(
                    "(more messages remain \u{2014} continue with: {} project read {n} --after {})",
                    k2_client::brand::CLI_NAME,
                    st(&at)
                ));
            }
        }
        Ok(())
    }

    fn rename(&mut self, project: &str, name: &str) -> R<()> {
        let l = line::RENAME;
        let d = self.post(
            "/cli/project-group/rename",
            obj(vec![("group", s(project)), ("name", s(name))]),
        )?;
        if self.json {
            let out = obj(vec![
                ("ok", Value::Bool(true)),
                ("id", at_v(&d, "id", l)?),
                ("name", at_v(&d, "name", l)?),
            ]);
            return self.print_json(&out, l);
        }
        self.line(&format!("Renamed to {}.", st(&at_v(&d, "name", l)?)));
        Ok(())
    }

    fn delete(&mut self, project: &str) -> R<()> {
        let l = line::DELETE;
        let show = self.fetch_show(&s(project))?;
        let d = self.post(
            "/cli/project-group/delete",
            obj(vec![("group", at_v(&show, "id", l)?)]),
        )?;
        if self.json {
            let out = obj(vec![
                ("ok", Value::Bool(true)),
                ("id", at_v(&d, "id", l)?),
                ("name", at_v(&show, "name", l)?),
            ]);
            return self.print_json(&out, l);
        }
        self.line(&format!(
            "Deleted project {}, its dashboards and its chat. Its agents and their folders are untouched.",
            st(&at_v(&show, "name", l)?)
        ));
        Ok(())
    }

    fn pin(&mut self, project: &str, state: &str) -> R<()> {
        let l = line::PIN;
        let d = self.post(
            "/cli/project-group/pin",
            obj(vec![
                ("group", s(project)),
                ("pinned", Value::Bool(state != "off")),
            ]),
        )?;
        let pinned = truthy(&at_v(&d, "pinned", l)?);
        if self.json {
            let out = obj(vec![
                ("ok", Value::Bool(true)),
                ("id", at_v(&d, "id", l)?),
                ("name", at_v(&d, "name", l)?),
                ("pinned", Value::Bool(pinned)),
            ]);
            return self.print_json(&out, l);
        }
        self.line(&format!(
            "{} {} for everyone on this server.",
            st(&at_v(&d, "name", l)?),
            if pinned {
                "is pinned"
            } else {
                "is no longer pinned"
            }
        ));
        Ok(())
    }

    fn view(&mut self, project: &str, want: &str, target: &str) -> R<()> {
        let l = line::VIEW;
        let show = self.fetch_show(&s(project))?;
        let name = or(at_v(&show, "name", l)?, s("?"));
        if at(&show, "defaultView", l)?.is_none() {
            return Err(self.die_with(
                "unsupported",
                concat!(
                    "this ",
                    product_name!(),
                    " server is too old for a project's view; update ",
                    product_name!(),
                    " on it"
                ),
            ));
        }
        let n = st(&name);
        if !target.is_empty() {
            let members = Self::members(&show, l)?;
            let mut old = false;
            for m in &members {
                if at(m, "ownDefaultView", l)?.is_none() {
                    old = true;
                }
            }
            if !members.is_empty() && old {
                return Err(self.die_with(
                    "unsupported",
                    concat!(
                        "this ",
                        product_name!(),
                        " server is too old for one agent's own view; update ",
                        product_name!(),
                        " on it"
                    ),
                ));
            }
            let low = target.to_lowercase();
            let mut found = None;
            for m in &members {
                let lower = |k: &str| -> R<bool> {
                    let v = or(at_v(m, k, l)?, s(""));
                    match &v {
                        Value::Str(_) => Ok(st(&v).to_lowercase() == low),
                        other => Err(Raise::no_attribute(l, other, "lower").into()),
                    }
                };
                let path = at_v(m, "path", l)?;
                if eq(&at_v(m, "workspaceId", l)?, &s(target))
                    || lower("name")?
                    || lower("agentName")?
                    || (target.contains('/') && truthy(&path) && Self::same_path(&path, target))
                {
                    found = Some(m.clone());
                    break;
                }
            }
            let Some(found) = found else {
                return Err(self.die_with(
                    "not_a_member",
                    &format!(
                        "'{target}' is not a member of {n} (see: {} project show {n})",
                        k2_client::brand::CLI_NAME
                    ),
                ));
            };
            let agent = mname(&found, l)?;
            if want.is_empty() {
                let cur = at_v(&found, "defaultView", l)?;
                let own = at_v(&found, "ownDefaultView", l)?;
                if self.json {
                    let out = obj(vec![
                        ("ok", Value::Bool(true)),
                        ("project", name),
                        ("agent", agent),
                        ("defaultView", cur),
                        ("ownDefaultView", own),
                    ]);
                    return self.print_json(&out, l);
                }
                self.line(&format!(
                    "{} in {n} opens in: {} ({})",
                    st(&agent),
                    st(&cur),
                    if truthy(&own) {
                        "its own view"
                    } else {
                        "the project's view"
                    }
                ));
                return Ok(());
            }
            let view = if want == "default" {
                Value::Null
            } else {
                s(want)
            };
            let d = self.post(
                "/cli/project-group/set-member-default-view",
                obj(vec![
                    ("group", at_v(&show, "id", l)?),
                    ("workspace", at_v(&found, "workspaceId", l)?),
                    ("view", view),
                ]),
            )?;
            let cur = at_v(&d, "defaultView", l)?;
            let own = at_v(&d, "ownDefaultView", l)?;
            if self.json {
                let out = obj(vec![
                    ("ok", Value::Bool(true)),
                    ("project", name),
                    ("agent", agent),
                    ("defaultView", cur),
                    ("ownDefaultView", own),
                ]);
                return self.print_json(&out, l);
            }
            if truthy(&own) {
                self.line(&format!("{} in {n} now opens in: {}", st(&agent), st(&cur)));
            } else {
                self.line(&format!(
                    "{} in {n} now follows the project's view: {}",
                    st(&agent),
                    st(&cur)
                ));
            }
            return Ok(());
        }
        if want.is_empty() {
            let cur = at_v(&show, "defaultView", l)?;
            if self.json {
                let out = obj(vec![
                    ("ok", Value::Bool(true)),
                    ("project", name),
                    ("defaultView", cur),
                ]);
                return self.print_json(&out, l);
            }
            self.line(&format!("{n} opens agents in: {}", st(&cur)));
            return Ok(());
        }
        let d = self.post(
            "/cli/project-group/set-default-view",
            obj(vec![("group", at_v(&show, "id", l)?), ("view", s(want))]),
        )?;
        let name = or(at_v(&d, "name", l)?, name);
        let new = or(at_v(&d, "defaultView", l)?, s(want));
        if self.json {
            let out = obj(vec![
                ("ok", Value::Bool(true)),
                ("project", name),
                ("defaultView", new),
            ]);
            return self.print_json(&out, l);
        }
        self.line(&format!("{} now opens agents in: {}", st(&name), st(&new)));
        Ok(())
    }

    fn access(&mut self, on: bool, workspace: &str) -> R<()> {
        let l = line::ACCESS;
        let target = if workspace.is_empty() {
            self.ws_path.clone()
        } else {
            workspace.to_string()
        };
        if target.is_empty() {
            return Err(self.die_with("usage", "pass --workspace <workspace>"));
        }
        let d = self.post(
            "/cli/agent-access/set",
            obj(vec![
                ("workspace", s(&target)),
                ("toggle", s("projects_manage")),
                ("value", Value::Bool(on)),
            ]),
        )?;
        let wsname = or(at_v(&d, "workspace", l)?, s(&target));
        if self.json {
            let out = obj(vec![
                ("ok", Value::Bool(true)),
                ("workspace", wsname),
                (
                    "agentsCanManageProjects",
                    Value::Bool(truthy(&at_v(&d, "agentsCanManageProjects", l)?)),
                ),
            ]);
            return self.print_json(&out, l);
        }
        let ws = st(&wsname);
        if on {
            self.line(&format!(
                "Manage {} projects is ON for {ws}: its agent may create, rename and delete projects and change their members.",
                product_name!()
            ));
        } else {
            self.line(&format!(
                "Manage {} projects is OFF for {ws}: its agent can read projects and post in its project chat, but not change them.",
                product_name!()
            ));
        }
        Ok(())
    }
}

/// `mname(m)`: the agent's name, else the registry name, else the
/// folder's name, else `?`.
fn mname(m: &Value, line: u32) -> R<Value> {
    let a = or(at_v(m, "agentName", line)?, at_v(m, "name", line)?);
    if truthy(&a) {
        return Ok(a);
    }
    let p = or(at_v(m, "path", line)?, s(""));
    let Value::Str(_) = p else {
        return Err(Raise::no_attribute(line, &p, "rstrip").into());
    };
    let text = st(&p);
    let b = basename(text.trim_end_matches('/'));
    Ok(if b.is_empty() { s("?") } else { s(b) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verb(words: &[&str]) -> Result<(Verb, bool), String> {
        match parse(words) {
            Form::Verb(v) => v,
            other => panic!("{words:?}: {other:?}"),
        }
    }

    #[test]
    fn the_words() {
        assert_eq!(parse(&[]), Form::Help);
        assert_eq!(parse(&[""]), Form::Help);
        assert_eq!(parse(&["nope"]), Form::Unknown("nope".into()));
        assert!(verb(&["create"]).is_err());
        assert_eq!(
            verb(&["create", "", "x"]).unwrap().0,
            Verb::Create { name: "x".into() }
        );
        assert!(verb(&["delete", "p"]).unwrap_err().contains("--yes"));
        assert_eq!(
            verb(&["pin", "p"]).unwrap().0,
            Verb::Pin {
                project: "p".into(),
                state: "on".into()
            }
        );
        assert!(verb(&["pin", "p", "maybe"]).is_err());
        assert!(verb(&["view", "p", "Split"])
            .unwrap_err()
            .contains("split is not"));
        assert_eq!(
            verb(&["view", "p", "THREAD"]).unwrap().0,
            Verb::View {
                project: "p".into(),
                view: "thread".into(),
                agent: String::new()
            }
        );
        assert!(verb(&["view", "p", "default"]).is_err());
        assert!(verb(&["view", "p", "--agent", ""]).is_err());
        assert!(verb(&["msg", "a", "b", "c"]).unwrap_err().contains("a b c"));
        assert!(verb(&["msg", "--x"]).is_err());
        // After `--` a word with a dash is the text; a second `--` too.
        assert_eq!(
            verb(&["msg", "--", "-x", "--json", "y"]).unwrap_err(),
            usage(&format!(
                "too many arguments \u{2014} did you mean: {} project msg \"-x --json y\"?",
                k2_client::brand::CLI_NAME
            ))
        );
        assert_eq!(
            verb(&["msg", "--"]).unwrap_err(),
            usage("msg requires <text> (or \"-\" to read it from stdin)")
        );
        match verb(&["msg", "--json", "--", "--", "--help"]).unwrap() {
            (
                Verb::Msg {
                    project, text, two, ..
                },
                j,
            ) => {
                assert_eq!(
                    (project.as_str(), text.as_str(), two, j),
                    ("--", "--help", true, true)
                )
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            verb(&["msg", "-"]).unwrap().0,
            Verb::Msg {
                project: String::new(),
                text: "-".into(),
                two: false,
                stdin: true
            }
        );
        assert!(verb(&["access", "on", "off"]).is_err());
        assert!(verb(&["list", "x"]).is_err());
    }

    #[test]
    fn compact_errors_keep_their_letters() {
        assert_eq!(
            err_line("usage", "a \u{2014} b"),
            "{\"error\":{\"code\":\"usage\",\"hint\":\"a \u{2014} b\"}}"
        );
    }

    #[test]
    fn realpath_without_links() {
        assert_eq!(realpath("/a/./b/../c/"), "/a/c");
        assert_eq!(realpath("/"), "/");
    }
}
