//! `cmd_agent_team`, `cmd_agent_instances` and `cmd_agent_hire_root`
//! (`cli/src/agent-hire.sh`): team links, hire instances and the hire
//! root.
//!
//! - GETs are `cli_request` on the left of a pipe into `_hire_print`
//!   ([`hire_print`]); `link`, `unlink`, `retire` and `hire-root set` are
//!   `cli_post_json` POSTs into it; `instances hire` is
//!   `cli_post_json_stdin` (the recipe on stdin, sent once).
//! - `team` and `instances` are the `agent-teams` tool, an `id` tool: in a
//!   session they send the passport, as the shell does (the caller checks
//!   it, [`super::run`]). `hire-root` is an open verb.
//! - The help word (`team help`, `instances help`) is handed back, before
//!   anything: the front prints its captured page.
//! - A flag that takes a value as the last word (`--kind`, `--scope`,
//!   `--workspace`, `--key`) is `_need_value`'s line, exit 2, nothing
//!   sent ([`out::need_value`]).

use super::{cli, dumps, obj, s};
use crate::cards::Decode;
use crate::exit::Exit;
use crate::families::doc::py::{decode_here, escaped};
use crate::families::{
    end_in_pipe, get, piped_body, pipefail, py_get, py_iter, raised, request_piped_tcp,
    without_nuls, Piped, Raise, Shown,
};
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::{json as pyjson, ops};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::client::{Call, Channel, Error};
use crate::transport::encode::quote;
use crate::transport::urllib::decode;
use crate::transport::{permission, Client};

/// `team` and `team served`.
pub const LINKS: &str = "/cli/agent-teams/links";
/// `team link`.
pub const LINK: &str = "/cli/agent-teams/link";
/// `team unlink`.
pub const UNLINK: &str = "/cli/agent-teams/unlink";
/// `instances` and `instances show`.
pub const INSTANCES: &str = "/cli/agent-instances";
/// `instances retire`.
pub const RETIRE: &str = "/cli/agent-instances/retire";
/// `instances hire`.
pub const HIRE: &str = "/cli/agent-instances/hire";
/// `hire-root` and `hire-root set`.
pub const ROOT: &str = "/cli/agents/hire-root";

/// What `_hire_print` prints (`HIRE_VIEW`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Team,
    Served,
    Instances,
    Instance,
    Hire,
    Retire,
    Link,
    Unlink,
    Root,
}

/// One verb and its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `team [served] [--workspace <ws>] [--json]`: an empty `ws` is the
    /// workspace's own (`$PROJECT`).
    Links {
        view: View,
        ws: String,
        json: bool,
    },
    /// `team link`: the body Python wrote.
    Link {
        body: String,
        json: bool,
    },
    Unlink {
        id: String,
    },
    Show {
        id: String,
        json: bool,
    },
    Retire {
        id: String,
        force: bool,
        json: bool,
    },
    /// `instances hire <file|->`.
    Hire {
        file: String,
        key: String,
        dry: bool,
        json: bool,
    },
    /// `instances [--key <k>] [--json]`.
    List {
        key: String,
        json: bool,
    },
    Root {
        json: bool,
    },
    SetRoot {
        dir: String,
    },
}

/// What the words ask for: a verb, or lines on stderr and an exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Ask(Ask),
    Stop(Vec<String>, Exit),
}

fn stop(line: String, exit: Exit) -> Option<Parsed> {
    Some(Parsed::Stop(vec![line], exit))
}

/// `_need_value "$@"` with no word after `flag`: its line, exit 2.
fn short(flag: &str) -> Option<Parsed> {
    Some(Parsed::Stop(vec![out::need_value(flag)], Exit::Usage))
}

/// The words after `team` (or `teams`). `None`: the help word.
pub fn parse_team(rest: &[&str]) -> Option<Parsed> {
    let cli = cli();
    let sub = rest.first().copied().unwrap_or("");
    match sub {
        "help" | "--help" | "-h" => None,
        "link" => {
            let (mut team, mut ws, mut kind, mut scope) =
                (String::new(), String::new(), String::new(), String::new());
            let (mut mesh, mut json) = (false, false);
            let mut i = 1;
            while i < rest.len() {
                let w = rest[i];
                match w {
                    "--kind" | "--scope" => {
                        let Some(v) = rest.get(i + 1) else {
                            return short(w);
                        };
                        if w == "--kind" {
                            kind = (*v).to_string();
                        } else {
                            scope = (*v).to_string();
                        }
                        i += 2;
                        continue;
                    }
                    "--mesh" => mesh = true,
                    "--json" => json = true,
                    _ if w.starts_with('-') => {
                        return stop(format!("Unknown flag: {w}"), Exit::Usage)
                    }
                    _ if team.is_empty() => team = w.to_string(),
                    _ if ws.is_empty() => ws = w.to_string(),
                    _ => return stop(format!("Unexpected: {w}"), Exit::Usage),
                }
                i += 1;
            }
            if team.is_empty() || ws.is_empty() {
                return stop(
                    format!("Usage: {cli} agent team link <team instance|workspace> <workspace it serves> [--kind serves|helper_of] [--scope JSON] [--mesh]"),
                    Exit::Usage,
                );
            }
            let mut b = vec![("team", s(&team)), ("workspace", s(&ws))];
            if !kind.is_empty() {
                b.push(("kind", s(&kind)));
            }
            if mesh {
                b.push(("topology", s("mesh")));
            }
            if !scope.is_empty() {
                match pyjson::loads(&scope) {
                    Ok(v) => b.push(("scope", v)),
                    Err(_) => return stop("--scope must be JSON".to_string(), Exit::Usage),
                }
            }
            Some(Parsed::Ask(Ask::Link {
                body: dumps(&obj(b)),
                json,
            }))
        }
        "unlink" => {
            let id = rest.get(1).copied().unwrap_or("");
            if id.is_empty() {
                return stop(
                    format!("Usage: {cli} agent team unlink <link-id>"),
                    Exit::Usage,
                );
            }
            Some(Parsed::Ask(Ask::Unlink { id: id.to_string() }))
        }
        "served" | "" | "--json" | "--workspace" => {
            let served = sub == "served";
            let (mut ws, mut json) = (String::new(), false);
            let mut i = usize::from(served);
            while i < rest.len() {
                match rest[i] {
                    "--workspace" => {
                        let Some(v) = rest.get(i + 1) else {
                            return short(rest[i]);
                        };
                        ws = (*v).to_string();
                        i += 2;
                        continue;
                    }
                    "--json" => json = true,
                    w => {
                        return stop(
                            format!("Unknown argument: {w} (see {cli} agent team --help)"),
                            Exit::Usage,
                        )
                    }
                }
                i += 1;
            }
            let view = if served { View::Served } else { View::Team };
            Some(Parsed::Ask(Ask::Links { view, ws, json }))
        }
        _ => stop(
            format!("Unknown: {cli} agent team {sub} (see {cli} agent team --help)"),
            Exit::Usage,
        ),
    }
}

/// The words after `instances` (or `instance`). `None`: the help word.
pub fn parse_instances(rest: &[&str]) -> Option<Parsed> {
    let cli = cli();
    let sub = rest.first().copied().unwrap_or("");
    match sub {
        "help" | "--help" | "-h" => None,
        "show" => {
            let id = rest.get(1).copied().unwrap_or("");
            let json = rest.get(2) == Some(&"--json");
            if id.is_empty() {
                return stop(
                    format!("Usage: {cli} agent instances show <instance>"),
                    Exit::Usage,
                );
            }
            Some(Parsed::Ask(Ask::Show {
                id: id.to_string(),
                json,
            }))
        }
        "retire" => {
            let (mut id, mut force, mut json) = (String::new(), false, false);
            for w in &rest[1..] {
                match *w {
                    "--force" => force = true,
                    "--json" => json = true,
                    _ if w.starts_with('-') => {
                        return stop(format!("Unknown flag: {w}"), Exit::Usage)
                    }
                    _ => id = (*w).to_string(),
                }
            }
            if id.is_empty() {
                return stop(
                    format!("Usage: {cli} agent instances retire <instance> [--force]"),
                    Exit::Usage,
                );
            }
            Some(Parsed::Ask(Ask::Retire { id, force, json }))
        }
        "hire" => {
            let (mut file, mut key) = (String::new(), String::new());
            let (mut dry, mut json) = (false, false);
            let mut i = 1;
            while i < rest.len() {
                let w = rest[i];
                match w {
                    "--key" => {
                        let Some(v) = rest.get(i + 1) else {
                            return short(rest[i]);
                        };
                        key = (*v).to_string();
                        i += 2;
                        continue;
                    }
                    "--dry-run" => dry = true,
                    "--json" => json = true,
                    "-" => file = "-".to_string(),
                    _ if w.starts_with('-') => {
                        return stop(format!("Unknown flag: {w}"), Exit::Usage)
                    }
                    _ => file = w.to_string(),
                }
                i += 1;
            }
            if file.is_empty() {
                return stop(
                    format!("Usage: {cli} agent instances hire <recipe.json|-> [--key <idempotency-key>] [--dry-run]"),
                    Exit::Usage,
                );
            }
            Some(Parsed::Ask(Ask::Hire {
                file,
                key,
                dry,
                json,
            }))
        }
        "" | "--json" | "--key" => {
            let (mut key, mut json) = (String::new(), false);
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    "--key" => {
                        let Some(v) = rest.get(i + 1) else {
                            return short(rest[i]);
                        };
                        key = (*v).to_string();
                        i += 2;
                        continue;
                    }
                    "--json" => json = true,
                    w => return stop(format!("Unknown argument: {w}"), Exit::Usage),
                }
                i += 1;
            }
            Some(Parsed::Ask(Ask::List { key, json }))
        }
        _ => stop(
            format!("Unknown: {cli} agent instances {sub} (see {cli} agent instances --help)"),
            Exit::Usage,
        ),
    }
}

/// The words after `hire-root`.
pub fn parse_root(rest: &[&str]) -> Parsed {
    let cli = cli();
    match rest.first().copied().unwrap_or("") {
        "set" => match rest.get(1).copied().unwrap_or("") {
            "" => Parsed::Stop(
                vec![format!("Usage: {cli} agent hire-root set <dir>")],
                Exit::Usage,
            ),
            dir => Parsed::Ask(Ask::SetRoot {
                dir: dir.to_string(),
            }),
        },
        first @ ("" | "--json") => Parsed::Ask(Ask::Root {
            json: first == "--json",
        }),
        _ => Parsed::Stop(
            vec![format!("Usage: {cli} agent hire-root [set <dir>]  \u{2014} the folder API keys hire agents into (owner sets it)")],
            Exit::Usage,
        ),
    }
}

/// The verb, after the gates.
pub fn run(client: &mut Client, closing: &mut Closing, ask: &Ask) -> Exit {
    match ask {
        Ask::Links { view, ws, json } => {
            let ws = if ws.is_empty() {
                client.project.clone()
            } else {
                ws.clone()
            };
            let q = format!("workspace={}", quote(&ws));
            let left = request_piped_tcp(client, closing, LINKS, &[q.as_str()]);
            read(left, *view, *json)
        }
        Ask::Link { body, json } => {
            let left = post_json_piped(client, closing, LINK, body);
            read(left, View::Link, *json)
        }
        Ask::Unlink { id } => {
            let body = dumps(&obj(vec![("link", s(id))]));
            let left = post_json_piped(client, closing, UNLINK, &body);
            read(left, View::Unlink, false)
        }
        Ask::Show { id, json } => {
            let q = format!("instance={}", quote(id));
            let left = request_piped_tcp(client, closing, INSTANCES, &[q.as_str()]);
            read(left, View::Instance, *json)
        }
        Ask::Retire { id, force, json } => {
            let body = dumps(&obj(vec![
                ("instance", s(id)),
                ("force", Value::Bool(*force)),
            ]));
            let left = post_json_piped(client, closing, RETIRE, &body);
            read(left, View::Retire, *json)
        }
        Ask::Hire {
            file,
            key,
            dry,
            json,
        } => {
            let recipe = match read_recipe(file) {
                Ok(r) => r,
                Err(shown) => return shown.print(),
            };
            let decode = decode_here().unwrap_or(Decode::Strict);
            let body = match recipe_body(&recipe, key, *dry, decode) {
                Ok(b) => b,
                Err(shown) => return shown.print(),
            };
            let left = post_stdin_piped(client, closing, HIRE, body.as_bytes());
            read(left, View::Hire, *json)
        }
        Ask::List { key, json } => {
            let q = format!("key={}", quote(key));
            let args: Vec<&str> = if key.is_empty() {
                vec![]
            } else {
                vec![q.as_str()]
            };
            let left = request_piped_tcp(client, closing, INSTANCES, &args);
            read(left, View::Instances, *json)
        }
        Ask::Root { json } => {
            let left = request_piped_tcp(client, closing, ROOT, &[]);
            read(left, View::Root, *json)
        }
        Ask::SetRoot { dir } => {
            let body = dumps(&obj(vec![("root", s(dir))]));
            let left = post_json_piped(client, closing, ROOT, &body);
            read(left, View::Root, false)
        }
    }
}

/// The reader on the right of the pipe, and `set -o pipefail`.
fn read(left: Piped, view: View, json: bool) -> Exit {
    let shown = hire_print(&decode(&left.body), view, json);
    pipefail(left.exit, shown.print())
}

/// `cli_post_json <endpoint> <body>` on the left of a pipe.
fn post_json_piped(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    body: &str,
) -> Piped {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, endpoint, Call::PostJson(body.as_bytes())) {
        Ok(r) => {
            closing.answer(&r, passport);
            Piped {
                body: piped_body(&r),
                exit: Exit::Ok,
            }
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            end_in_pipe()
        }
        Err(e) => Piped {
            body: Vec::new(),
            exit: Exit::from_curl(e.curl_code()),
        },
    }
}

/// `printf '%s' "$body" | cli_post_json_stdin <endpoint>` on the left of
/// a pipe: sent once, no line; its refusal's `permission` noted.
fn post_stdin_piped(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    body: &[u8],
) -> Piped {
    let passport = client.passport().is_some();
    match client.post_json_stdin(endpoint, body) {
        Ok(mut r) => {
            r.permission = permission::note(&r.body);
            closing.answer(&r, passport);
            Piped {
                body: piped_body(&r),
                exit: Exit::Ok,
            }
        }
        Err(e) => Piped {
            body: Vec::new(),
            exit: Exit::from_curl(e.curl_code()),
        },
    }
}

/// `recipe="$(cat)"` or `[ -r "$file" ]` and `recipe="$(cat "$file")"`:
/// the bytes as `$(…)` holds them (NULs and trailing newlines gone).
fn read_recipe(file: &str) -> Result<Vec<u8>, Shown> {
    let bytes = if file == "-" {
        use std::io::Read;
        let mut b = Vec::new();
        // A stdin that can't be read is cat's empty output here.
        let _ = std::io::stdin().lock().read_to_end(&mut b);
        b
    } else {
        if !readable(file) {
            return Err(Shown::err(
                format!("Error: cannot read {file}\n"),
                Exit::Usage,
            ));
        }
        match std::fs::read(file) {
            Ok(b) => b,
            // `cat` says why on stderr, and the failed `$(…)` ends the
            // command under `set -e`.
            Err(e) => {
                return Err(Shown::err(
                    format!("cat: {file}: {}\n", why(&e)),
                    Exit::Error,
                ))
            }
        }
    };
    let mut held = without_nuls(&bytes);
    while held.last() == Some(&b'\n') {
        held.pop();
    }
    Ok(held)
}

/// `[ -r "$file" ]`.
#[cfg(unix)]
fn readable(file: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(file) else {
        return false;
    };
    // SAFETY: a NUL-terminated path; access reads it and keeps nothing.
    unsafe { libc::access(c.as_ptr(), libc::R_OK) == 0 }
}

#[cfg(not(unix))]
fn readable(file: &str) -> bool {
    std::fs::File::open(file).is_ok()
}

/// What `cat` says of an error: the C library's words.
fn why(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(21) => "Is a directory".to_string(),
        _ => {
            let text = e.to_string();
            match text.find(" (os error") {
                Some(at) => text[..at].to_string(),
                None => text,
            }
        }
    }
}

/// The recipe's Python: `json.load(sys.stdin)` (decoded as the locale
/// says), `key` and `dryRun` set, `json.dumps`. `Err`: what it printed,
/// and exit 2 (`|| exit 2`).
fn recipe_body(recipe: &[u8], key: &str, dry: bool, decode: Decode) -> Result<String, Shown> {
    let not_json = |why: &str| {
        Shown::err(
            format!("Error: the recipe is not JSON: {why}\n"),
            Exit::Usage,
        )
    };
    let lossy = decode == Decode::Lossy && std::str::from_utf8(recipe).is_err();
    let text = match std::str::from_utf8(recipe) {
        Ok(t) => t.to_string(),
        Err(_) if lossy => {
            let mut t = String::with_capacity(recipe.len() + 8);
            for chunk in recipe.utf8_chunks() {
                t.push_str(chunk.valid());
                t.extend(chunk.invalid().iter().map(|b| escaped(*b)));
            }
            t
        }
        Err(_) => {
            let f = pyjson::decode_utf8(recipe).expect_err("not UTF-8");
            return Err(not_json(&f.message));
        }
    };
    let mut b = match pyjson::loads(&text) {
        Ok(v) => v,
        Err(f) => return Err(not_json(&f.message)),
    };
    let set = |b: &mut Value, k: &str, v: Value, line: u32| -> Result<(), Shown> {
        assign(b, k, v, line).map_err(|r| Shown::err(r.text(), Exit::Usage))
    };
    if !key.is_empty() {
        set(&mut b, "key", s(key), 5)?;
    }
    if dry {
        set(&mut b, "dryRun", Value::Bool(true), 6)?;
    }
    let body = dumps(&b);
    Ok(if lossy { surrogates(&body) } else { body })
}

/// `b[k] = v`: a key that is there keeps its place.
fn assign(b: &mut Value, k: &str, v: Value, line: u32) -> Result<(), Raise> {
    match b {
        Value::Object(pairs) => {
            let key = Text::from(k);
            match pairs.iter_mut().find(|(have, _)| *have == key) {
                Some(slot) => slot.1 = v,
                None => pairs.push((key, v)),
            }
            Ok(())
        }
        Value::Array(_) => Err(Raise::at(
            line,
            "TypeError: list indices must be integers or slices, not str",
        )),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object does not support item assignment",
                py::type_name(other)
            ),
        )),
    }
}

/// `json.dumps` of a text read with `surrogateescape`: each stand-in
/// ([`escaped`], U+10FF80 to U+10FFFF, written `\udbff\udfXX`) as the
/// `\udcXX` Python writes.
fn surrogates(body: &str) -> String {
    const LEAD: &str = "\\udbff\\udf";
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find(LEAD) {
        out.push_str(&rest[..at]);
        let tail = &rest[at + LEAD.len()..];
        match tail.get(..2) {
            Some(xx) if u8::from_str_radix(xx, 16).is_ok_and(|b| b >= 0x80) => {
                out.push_str("\\udc");
                out.push_str(xx);
                rest = &tail[2..];
            }
            _ => {
                out.push_str(LEAD);
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------
// `_hire_print`: the Python's lines are its `<string>` lines.

/// `json.dumps(d, indent=2)` and print's newline.
fn indent2(v: &Value) -> String {
    let opts = Options {
        indent: Some(2),
        ..Options::default()
    };
    json::to_string(v, &opts).expect("json.dumps with ensure_ascii") + "\n"
}

/// `x or default`.
fn or(x: Option<&Value>, default: Value) -> Value {
    match x {
        Some(v) if py::truthy(v) => v.clone(),
        _ => default,
    }
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

fn empty_list() -> Value {
    Value::Array(Vec::new())
}

/// `"%s" % x` of what `.get` gave.
fn sv(x: Option<&Value>) -> String {
    ops::s_of(x)
}

/// `.get(k)` on a dict for sure.
fn at<'a>(d: &'a Value, k: &str) -> Option<&'a Value> {
    match d {
        Value::Object(pairs) => get(pairs, k),
        _ => None,
    }
}

fn truthy(x: Option<&Value>) -> bool {
    x.is_some_and(py::truthy)
}

/// `" (lead)" if m.get("lead") else ""`.
fn lead(m: &Value) -> &'static str {
    if truthy(at(m, "lead")) {
        " (lead)"
    } else {
        ""
    }
}

/// `prefix + v`, which needs a string.
fn concat(prefix: &str, v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(format!("{prefix}{}", py::str_of(v))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

/// `(" \u{2014} " + x["reason"]) if x.get("reason") else ""`.
fn reason(x: &Value, line: u32) -> Result<String, Raise> {
    match at(x, "reason") {
        Some(r) if py::truthy(r) => concat(" \u{2014} ", r, line),
        _ => Ok(String::new()),
    }
}

/// `_hire_print <view> <json>` on `raw`.
pub fn hire_print(raw: &str, view: View, as_json: bool) -> Shown {
    let Ok(d) = pyjson::loads(raw) else {
        return Shown::err(
            format!("Error: daemon answered: {}\n", super::cut(raw, 300)),
            Exit::Error,
        );
    };
    if let Some(shown) = refused(&d) {
        return shown;
    }
    if as_json {
        return Shown::out(indent2(&d));
    }
    let mut out = String::new();
    let done = match view {
        View::Team | View::Served => team(&d, view == View::Team, &mut out).map(|()| Exit::Ok),
        View::Instances => instances(&d, &mut out).map(|()| Exit::Ok),
        View::Instance => instance(&d, &mut out).map(|()| Exit::Ok),
        View::Hire => hire(&d, &mut out).map(|()| Exit::Ok),
        View::Retire => retire(&d, &mut out),
        View::Link => link(&d, &mut out).map(|()| Exit::Ok),
        View::Unlink => unlink(&d, &mut out).map(|()| Exit::Ok),
        View::Root => root(&d, &mut out).map(|()| Exit::Ok),
    };
    match done {
        Ok(exit) => Shown {
            stdout: out,
            stderr: String::new(),
            exit,
        },
        Err(r) => raised(out, String::new(), &r),
    }
}

/// Lines 8 to 15: an answer with a true `error`.
fn refused(d: &Value) -> Option<Shown> {
    let err = at(d, "error").cloned().unwrap_or(Value::Null);
    if !py::truthy(&err) {
        return None;
    }
    let (code, hint) = match &err {
        Value::Object(_) => (
            at(&err, "code").cloned().unwrap_or_else(|| s("")),
            at(&err, "hint").cloned().unwrap_or_else(|| s("")),
        ),
        other => (s(""), s(&py::str_of(other))),
    };
    let head = if !py::truthy(&code) {
        String::new()
    } else {
        match &code {
            Value::Str(_) => format!("{}: ", py::str_of(&code)),
            Value::Array(_) => {
                let r = Raise::at(
                    12,
                    "TypeError: can only concatenate list (not \"str\") to list",
                );
                return Some(raised(String::new(), String::new(), &r));
            }
            other => {
                let r = Raise::at(
                    12,
                    format!(
                        "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                        py::type_name(other)
                    ),
                );
                return Some(raised(String::new(), String::new(), &r));
            }
        }
    };
    let line = format!("Error: {head}{}\n", py::str_of(&hint));
    let named = ["owner_only", "forbidden", "exceeds_key", "role_required"]
        .iter()
        .any(|c| ops::py_eq(&code, &s(c)));
    if named {
        return Some(Shown::err(line, Exit::Refused));
    }
    let token = match &hint {
        Value::Str(_) => py::str_of(&hint).to_lowercase().contains("token"),
        other => {
            let r = Raise::no_attribute(13, other, "lower");
            return Some(raised(String::new(), line, &r));
        }
    };
    if token {
        return Some(Shown::err(line, Exit::Refused));
    }
    if let Value::Array(_) | Value::Object(_) = code {
        let r = Raise::at(
            15,
            format!("TypeError: unhashable type: '{}'", py::type_name(&code)),
        );
        return Some(raised(String::new(), line, &r));
    }
    let exit = if ops::py_eq(&code, &s("not_found")) {
        Exit::NotFound
    } else if ops::py_eq(&code, &s("usage")) {
        Exit::Usage
    } else {
        Exit::Error
    };
    Some(Shown::err(line, exit))
}

/// `link_line(l, side)`, called from line `from`.
fn link_line(l: &Value, out_side: bool, from: u32) -> Result<String, Raise> {
    let inside = |r: Raise| Raise {
        frames: vec![(from, "<module>"), (r.frames[0].0, "link_line")],
        exception: r.exception,
    };
    let other_key = if out_side {
        "teamLeadHandle"
    } else {
        "consumerHandle"
    };
    let other = py_get(l, other_key, 20).map_err(inside)?.cloned();
    let team = or(at(l, "teamName"), or(at(l, "teamLeadHandle"), s("?")));
    let scope = match at(l, "scope") {
        Some(v) if *v != Value::Null => dumps(v),
        _ => "-".to_string(),
    };
    let line = if out_side { 24 } else { 25 };
    let id = ops::index(l, "id", line).map_err(inside)?;
    let kind = ops::index(l, "kind", line).map_err(inside)?;
    let serves = ops::py_eq(kind, &s("serves"));
    let other = py::str_of(&or(other.as_ref(), s("?")));
    Ok(if out_side {
        format!(
            "  {}  {} {} (lead {other})  scope {scope}",
            py::str_of(id),
            if serves { "served by" } else { "helped by" },
            py::str_of(&team),
        )
    } else {
        format!(
            "  {}  {} {other}  scope {scope}",
            py::str_of(id),
            if serves { "serves" } else { "helps" },
        )
    })
}

/// Lines 26 to 40.
fn team(d: &Value, team_view: bool, out: &mut String) -> Result<(), Raise> {
    let ws = or(py_get(d, "workspace", 27)?, empty_dict());
    let handle = py_get(&ws, "handle", 28)?.cloned();
    let label = if truthy(handle.as_ref()) {
        handle
    } else {
        py_get(&ws, "id", 28)?.cloned()
    };
    out.push_str(&format!("Workspace: {}\n", sv(label.as_ref())));
    let t = at(d, "team").cloned();
    if team_view {
        if let Some(t) = t.filter(py::truthy) {
            let name = or(py_get(&t, "name", 32)?, s("(single hire)"));
            let inst = at(&t, "instance");
            out.push_str(&format!("Team: {} ({})\n", py::str_of(&name), sv(inst)));
            for m in py_iter(&or(at(&t, "members"), empty_list()), 33)? {
                let h = ops::index(&m, "handle", 34)?;
                let role = at(&m, "role").cloned().unwrap_or_else(|| s(""));
                out.push_str(&format!(
                    "  {}{}  {}\n",
                    py::str_of(h),
                    lead(&m),
                    py::str_of(&role)
                ));
            }
        }
        let served = or(at(d, "servedBy"), empty_list());
        out.push_str(if py::truthy(&served) {
            "Served by:\n"
        } else {
            "Served by: nobody\n"
        });
        for l in py_iter(&served, 37)? {
            out.push_str(&link_line(&l, true, 37)?);
            out.push('\n');
        }
    }
    let serves = or(at(d, "serves"), empty_list());
    out.push_str(if py::truthy(&serves) {
        "Serves:\n"
    } else {
        "Serves: nobody\n"
    });
    for l in py_iter(&serves, 40)? {
        out.push_str(&link_line(&l, false, 40)?);
        out.push('\n');
    }
    Ok(())
}

/// `i["apiKeyId"][:8]`, and `"key " +` it.
fn key_who(v: &Value) -> Result<String, Raise> {
    let l = 47;
    match v {
        Value::Str(_) => Ok(format!("key {}", super::head(v, 8))),
        Value::Array(_) => Err(Raise::at(
            l,
            "TypeError: can only concatenate str (not \"list\") to str",
        )),
        Value::Object(_) => Err(Raise::at(l, "KeyError: slice(None, 8, None)")),
        other => Err(Raise::at(
            l,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// Lines 41 to 49.
fn instances(d: &Value, out: &mut String) -> Result<(), Raise> {
    let rows = or(py_get(d, "instances", 42)?, empty_list());
    if !py::truthy(&rows) {
        out.push_str("No hires yet.\n");
        return Ok(());
    }
    for i in py_iter(&rows, 45)? {
        let members = or(py_get(&i, "members", 46)?, empty_list());
        let mut handles = Vec::new();
        for m in py_iter(&members, 46)? {
            let h = ops::index(&m, "handle", 46).map_err(|r| Raise {
                frames: vec![(46, "<module>"), (46, "<genexpr>")],
                exception: r.exception,
            })?;
            handles.push(h.clone());
        }
        let members = ops::join(&Value::Array(handles), ",", 46)?;
        let who = match at(&i, "apiKeyId") {
            Some(k) if py::truthy(k) => key_who(k)?,
            _ => sv(at(&i, "actor")),
        };
        let id = ops::index(&i, "id", 48)?;
        let status = ops::index(&i, "status", 48)?;
        let team = or(at(&i, "teamName"), s("-"));
        let members = if members.is_empty() {
            "-".to_string()
        } else {
            members
        };
        let behalf = match at(&i, "onBehalfOf") {
            Some(v) if py::truthy(v) => format!("  for {} (app-asserted)", py::str_of(v)),
            _ => String::new(),
        };
        out.push_str(&format!(
            "{}  [{}]  {}  {members}  by {who}{behalf}\n",
            py::str_of(id),
            py::str_of(status),
            py::str_of(&team),
        ));
    }
    Ok(())
}

/// Lines 50 to 59.
fn instance(d: &Value, out: &mut String) -> Result<(), Raise> {
    let i = or(py_get(d, "instance", 51)?, empty_dict());
    let id = py_get(&i, "id", 52)?;
    let team = or(at(&i, "teamName"), s("(single hire)"));
    out.push_str(&format!(
        "{}  [{}]  {}\n",
        sv(id),
        sv(at(&i, "status")),
        py::str_of(&team)
    ));
    if truthy(at(&i, "reason")) {
        out.push_str(&format!("  reason: {}\n", sv(at(&i, "reason"))));
    }
    for m in py_iter(&or(at(&i, "members"), empty_list()), 54)? {
        let h = ops::index(&m, "handle", 55)?;
        out.push_str(&format!(
            "  {}{}  {}  [{}]  {}\n",
            py::str_of(h),
            lead(&m),
            sv(at(&m, "role")),
            sv(at(&m, "status")),
            sv(at(&m, "path")),
        ));
    }
    for l in py_iter(&or(at(d, "links"), empty_list()), 56)? {
        let id = ops::index(&l, "id", 57)?;
        out.push_str(&format!(
            "  link {}  {} \u{2192} {}\n",
            py::str_of(id),
            sv(at(&l, "teamLeadHandle")),
            sv(at(&l, "consumerHandle")),
        ));
    }
    for e in py_iter(&or(at(d, "events"), empty_list()), 58)? {
        let when = ops::index(&e, "at", 59)?;
        let action = ops::index(&e, "action", 59)?;
        let result = ops::index(&e, "result", 59)?;
        let why = reason(&e, 59)?;
        out.push_str(&format!(
            "  event {} {} {}{why}\n",
            py::str_of(when),
            py::str_of(action),
            py::str_of(result),
        ));
    }
    Ok(())
}

/// A member's line in `hire`: `m["handle"]`, the lead mark, `m["path"]`.
fn member_path(m: &Value, line: u32) -> Result<String, Raise> {
    let h = ops::index(m, "handle", line)?;
    let path = ops::index(m, "path", line)?;
    Ok(format!(
        "  {}{}  {}\n",
        py::str_of(h),
        lead(m),
        py::str_of(path)
    ))
}

/// Lines 60 to 71.
fn hire(d: &Value, out: &mut String) -> Result<(), Raise> {
    if truthy(py_get(d, "dryRun", 61)?) {
        let p = or(at(d, "plan"), empty_dict());
        out.push_str("Plan (nothing written):\n");
        for m in py_iter(&or(py_get(&p, "members", 64)?, empty_list()), 64)? {
            out.push_str(&member_path(&m, 65)?);
        }
        let note = at(&p, "note").cloned().unwrap_or_else(|| s(""));
        out.push_str(&py::str_of(&note));
        out.push('\n');
        return Ok(());
    }
    out.push_str(&format!(
        "{} {}\n",
        if truthy(at(d, "changed")) {
            "Hired"
        } else {
            "Already hired (same key):"
        },
        sv(at(d, "instance"))
    ));
    for m in py_iter(&or(at(d, "members"), empty_list()), 68)? {
        out.push_str(&member_path(&m, 69)?);
    }
    for l in py_iter(&or(at(d, "links"), empty_list()), 70)? {
        let id = ops::index(&l, "id", 71)?;
        out.push_str(&format!("  linked {}\n", py::str_of(id)));
    }
    Ok(())
}

/// Lines 72 to 77: exit 3 when the retire is blocked.
fn retire(d: &Value, out: &mut String) -> Result<Exit, Raise> {
    let inst = py_get(d, "instance", 73)?;
    out.push_str(&format!("{} {}\n", sv(inst), sv(at(d, "status"))));
    for m in py_iter(&or(at(d, "members"), empty_list()), 74)? {
        let h = ops::index(&m, "handle", 75)?;
        let st = ops::index(&m, "status", 75)?;
        let why = reason(&m, 75)?;
        out.push_str(&format!("  {} {}{why}\n", py::str_of(h), py::str_of(st)));
    }
    if let Some(h) = at(d, "hint").filter(|h| py::truthy(h)) {
        out.push_str(&py::str_of(h));
        out.push('\n');
    }
    let blocked = at(d, "status").is_some_and(|v| ops::py_eq(v, &s("retire_blocked")));
    Ok(if blocked { Exit::Refused } else { Exit::Ok })
}

/// Lines 78 to 80.
fn link(d: &Value, out: &mut String) -> Result<(), Raise> {
    let l = or(py_get(d, "link", 79)?, empty_dict());
    let word = if truthy(at(d, "changed")) {
        "Linked"
    } else {
        "Already linked:"
    };
    let id = py_get(&l, "id", 80)?;
    out.push_str(&format!("{word} {}\n", sv(id)));
    Ok(())
}

/// Line 82.
fn unlink(d: &Value, out: &mut String) -> Result<(), Raise> {
    let changed = truthy(py_get(d, "changed", 82)?);
    let id = sv(at(d, "link"));
    out.push_str(&if changed {
        format!("Unlinked {id}\n")
    } else {
        format!("Link {id} was already removed\n")
    });
    Ok(())
}

/// Line 84.
fn root(d: &Value, out: &mut String) -> Result<(), Raise> {
    let r = py_get(d, "root", 84)?;
    let default = if truthy(at(d, "default")) {
        "  (default)"
    } else {
        ""
    };
    out.push_str(&format!("{}{default}\n", sv(r)));
    if let Some(Value::Array(lines)) = at(d, "doctor") {
        for line in lines {
            out.push_str(&format!("doctor: {}\n", sv(Some(line))));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team(words: &[&str]) -> Option<Parsed> {
        parse_team(words)
    }

    #[test]
    fn the_team_words() {
        assert_eq!(team(&["help"]), None);
        assert_eq!(
            team(&[]),
            Some(Parsed::Ask(Ask::Links {
                view: View::Team,
                ws: String::new(),
                json: false
            }))
        );
        assert_eq!(
            team(&["served", "--workspace", "acme", "--json"]),
            Some(Parsed::Ask(Ask::Links {
                view: View::Served,
                ws: "acme".into(),
                json: true
            }))
        );
        assert_eq!(
            team(&["--workspace"]),
            Some(Parsed::Stop(
                vec!["Error: --workspace takes a value".into()],
                Exit::Usage
            ))
        );
        assert!(matches!(team(&[""]), Some(Parsed::Stop(_, Exit::Usage))));
        assert_eq!(
            team(&["link", "t", "w", "--kind", "serves", "--mesh", "--scope", "{\"a\": 1}"]),
            Some(Parsed::Ask(Ask::Link {
                body: r#"{"team": "t", "workspace": "w", "kind": "serves", "topology": "mesh", "scope": {"a": 1}}"#.into(),
                json: false
            }))
        );
        assert_eq!(
            team(&["link", "t", "w", "--scope", "nope"]),
            Some(Parsed::Stop(
                vec!["--scope must be JSON".into()],
                Exit::Usage
            ))
        );
        assert_eq!(
            team(&["link", "t", "--kind"]),
            Some(Parsed::Stop(
                vec!["Error: --kind takes a value".into()],
                Exit::Usage
            ))
        );
        assert_eq!(
            team(&["link", "t", "w", "--scope"]),
            Some(Parsed::Stop(
                vec!["Error: --scope takes a value".into()],
                Exit::Usage
            ))
        );
        assert_eq!(
            team(&["link", "", "t", "w"]),
            Some(Parsed::Ask(Ask::Link {
                body: r#"{"team": "t", "workspace": "w"}"#.into(),
                json: false
            }))
        );
    }

    #[test]
    fn the_instances_words() {
        assert_eq!(parse_instances(&["help"]), None);
        assert_eq!(
            parse_instances(&["hire", "-", "--key", "k1", "--dry-run"]),
            Some(Parsed::Ask(Ask::Hire {
                file: "-".into(),
                key: "k1".into(),
                dry: true,
                json: false
            }))
        );
        assert_eq!(
            parse_instances(&["show", "i1", "x", "--json"]),
            Some(Parsed::Ask(Ask::Show {
                id: "i1".into(),
                json: false
            }))
        );
        assert_eq!(
            parse_instances(&["retire", "a", "b", "--force"]),
            Some(Parsed::Ask(Ask::Retire {
                id: "b".into(),
                force: true,
                json: false
            }))
        );
        assert_eq!(
            parse_instances(&["--key"]),
            Some(Parsed::Stop(
                vec!["Error: --key takes a value".into()],
                Exit::Usage
            ))
        );
        assert_eq!(
            parse_instances(&["hire", "f.json", "--key"]),
            Some(Parsed::Stop(
                vec!["Error: --key takes a value".into()],
                Exit::Usage
            ))
        );
    }

    #[test]
    fn the_recipe() {
        assert_eq!(
            recipe_body(br#"{"key": "a", "t": 1.0}"#, "k", true, Decode::Strict).unwrap(),
            r#"{"key": "k", "t": 1.0, "dryRun": true}"#
        );
        let e = recipe_body(b"[1]", "k", false, Decode::Strict).unwrap_err();
        assert!(e.stderr.ends_with(
            "line 5, in <module>\nTypeError: list indices must be integers or slices, not str\n"
        ));
        assert_eq!(e.exit, Exit::Usage);
        assert_eq!(
            recipe_body(b"{\"a\": \"\xff\"}", "", false, Decode::Lossy).unwrap(),
            r#"{"a": "\udcff"}"#
        );
        assert_eq!(
            recipe_body(b"\xff", "", false, Decode::Strict).unwrap_err().stderr,
            "Error: the recipe is not JSON: 'utf-8' codec can't decode byte 0xff in position 0: invalid start byte\n"
        );
    }

    #[test]
    fn the_printer() {
        let p = hire_print(
            r#"{"error": {"code": "owner_only", "hint": "no"}}"#,
            View::Root,
            false,
        );
        assert_eq!(
            (p.stderr.as_str(), p.exit),
            ("Error: owner_only: no\n", Exit::Refused)
        );
        let p = hire_print(r#"{"root": "/srv/h", "default": true}"#, View::Root, false);
        assert_eq!(p.stdout, "/srv/h  (default)\n");
        let p = hire_print(r#"{"root": "/srv/h", "default": false, "doctor": ["one", "two"]}"#, View::Root, false);
        assert_eq!(p.stdout, "/srv/h\ndoctor: one\ndoctor: two\n");
        let p = hire_print(
            r#"{"instances": [{"id": "i1", "status": "live", "members": [{"handle": "a"}, {"handle": "b"}], "apiKeyId": "abcdefghij"}]}"#,
            View::Instances,
            false,
        );
        assert_eq!(p.stdout, "i1  [live]  -  a,b  by key abcdefgh\n");
        let p = hire_print(
            r#"{"instances": [{"members": [{}]}]}"#,
            View::Instances,
            false,
        );
        assert!(p.stderr.ends_with(
            "line 46, in <module>\n  File \"<string>\", line 46, in <genexpr>\nKeyError: 'handle'\n"
        ));
    }
}
