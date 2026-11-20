//! `cmd_agent_context` and its verbs (`cli/src/agent-context.sh`): the
//! AGENTS.md layer stack over `/cli/context/*`, each call a urllib request
//! of `_context_http`'s own (K2's ruling D4).

use super::{
    abs_user, at, cli, cut, dumps, err_json, obj, product, py_strip, raised, s, str_or,
};
use crate::exit::Exit;
use crate::families::{ljust, py_get, py_iter, without_nuls, Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::{json as pyjson, ops};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::client::{Call, Channel, Error};
use crate::transport::encode::quote;
use crate::transport::urllib::{decode, Method, Opener, Outcome, Request};
use crate::transport::Client;
use std::time::Duration;

const LAYERS: &str = "/cli/context/layers";
const ADD: &str = "/cli/context/add";
const REMOVE: &str = "/cli/context/remove";
const SET_ENABLED: &str = "/cli/context/set-enabled";
const MOVE: &str = "/cli/context/move";
const SHOW: &str = "/cli/context/show";
const REGEN: &str = "/cli/context/regen";
const CATALOG: &str = "/cli/context/catalog";
const RESOLVE: &str = "/cli/workspace/resolve";

/// The blocks' lines (`<stdin>`).
mod line {
    pub const HTTP_ERR: u32 = 77;
    pub const HTTP_EXIT: u32 = 30;
    pub const WARN: u32 = 8;
    pub const RESOLVE_PINNED: u32 = 27;
    pub const RESOLVE_LAYERS: u32 = 37;
    pub const LIST_PINNED: u32 = 18;
    pub const LIST_LAYERS: u32 = 36;
    pub const CATALOG: u32 = 15;
    pub const SHOW: u32 = 9;
    pub const ADD: u32 = 4;
}

/// One context verb and its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    List,
    Add { target: String, label: String },
    Remove { token: String },
    Enabled { on: bool, token: String },
    Move { token: String, to: String },
    Show { outline: bool },
    Regen,
    Catalog,
}

/// The line's words after `context`: the workspace flags, in order, and
/// what is left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextArgs {
    /// `--workspace <w>` and `--project <w>` (and `=` forms), in order.
    pub workspaces: Vec<String>,
    /// A workspace flag with no value: its usage line (after the ones
    /// before it were applied).
    pub missing: Option<String>,
    pub rest: Vec<String>,
}

/// `cmd_agent_context`'s first loop.
pub fn parse(words: &[&str]) -> ContextArgs {
    let mut a = ContextArgs {
        workspaces: Vec::new(),
        missing: None,
        rest: Vec::new(),
    };
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if w == "--workspace" || w == "--project" {
            match words.get(i + 1) {
                Some(v) => a.workspaces.push((*v).to_string()),
                None => {
                    a.missing = Some(format!("{w} requires a workspace path, name, or uuid"));
                    return a;
                }
            }
            i += 2;
            continue;
        }
        if let Some(v) = w
            .strip_prefix("--workspace=")
            .or_else(|| w.strip_prefix("--project="))
        {
            a.workspaces.push(v.to_string());
        } else {
            a.rest.push(w.to_string());
        }
        i += 1;
    }
    a
}

/// The verb's own words, or its usage hint; `None` for no verb (the
/// help page).
fn verb(rest: &[String]) -> Option<Result<(Verb, bool), String>> {
    let (sub, words) = rest.split_first()?;
    let cli = cli();
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    Some(match sub.as_str() {
        "list" => no_args(&words, &format!("{cli} agent context list [--json]"), false)
            .map(|(json, _)| (Verb::List, json)),
        "regen" => no_args(&words, &format!("{cli} agent context regen [--json]"), false)
            .map(|(json, _)| (Verb::Regen, json)),
        "catalog" => no_args(&words, &format!("{cli} agent context catalog [--json]"), false)
            .map(|(json, _)| (Verb::Catalog, json)),
        "show" => no_args(
            &words,
            &format!("{cli} agent context show [--outline] [--json]"),
            true,
        )
        .map(|(json, outline)| (Verb::Show { outline }, json)),
        "add" => add_words(&words),
        "remove" | "rm" | "delete" => {
            one_token(&words, &format!("{cli} agent context remove <id|path> [--json]"))
                .map(|(token, json)| (Verb::Remove { token }, json))
        }
        "on" | "off" => {
            let on = sub == "on";
            one_token(&words, &format!("{cli} agent context {sub} <id|path> [--json]"))
                .map(|(token, json)| (Verb::Enabled { on, token }, json))
        }
        "move" => move_words(&words),
        other => Err(format!(
            "unknown context subcommand '{other}' \u{2014} see: {cli} agent context --help"
        )),
    })
}

/// A verb with flags only: `--json` (and `--outline` for `show`).
fn no_args(words: &[&str], usage: &str, outline_ok: bool) -> Result<(bool, bool), String> {
    let (mut json, mut outline) = (false, false);
    for w in words {
        match *w {
            "--json" => json = true,
            "--outline" if outline_ok => outline = true,
            _ if w.starts_with('-') => {
                return Err(format!("unknown flag '{w}' \u{2014} usage: {usage}"))
            }
            _ => return Err(format!("unexpected argument '{w}' \u{2014} usage: {usage}")),
        }
    }
    Ok((json, outline))
}

/// `remove`, `on` and `off`: one `<id|path>` and `--json`.
fn one_token(words: &[&str], usage: &str) -> Result<(String, bool), String> {
    let (mut token, mut json) = (String::new(), false);
    for w in words {
        if *w == "--json" {
            json = true;
        } else if w.starts_with('-') {
            return Err(format!("unknown flag '{w}' \u{2014} usage: {usage}"));
        } else if token.is_empty() {
            token = (*w).to_string();
        } else {
            return Err(format!("unexpected extra argument '{w}'"));
        }
    }
    if token.is_empty() {
        return Err(format!("missing <id|path> \u{2014} usage: {usage}"));
    }
    Ok((token, json))
}

fn add_words(words: &[&str]) -> Result<(Verb, bool), String> {
    let usage = format!(
        "usage: {} agent context add <path|catalog-id> [--label name] [--json]",
        cli()
    );
    let (mut target, mut label, mut json) = (String::new(), String::new(), false);
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--label" => {
                let Some(v) = words.get(i + 1) else {
                    return Err("--label requires a value".to_string());
                };
                label = (*v).to_string();
                i += 1;
            }
            "--json" => json = true,
            _ if w.starts_with('-') => return Err(format!("unknown flag '{w}' \u{2014} {usage}")),
            _ if target.is_empty() => target = w.to_string(),
            _ => return Err(format!("unexpected extra argument '{w}' \u{2014} {usage}")),
        }
        i += 1;
    }
    if target.is_empty() {
        return Err(format!("missing <path|catalog-id> \u{2014} {usage}"));
    }
    Ok((Verb::Add { target, label }, json))
}

fn move_words(words: &[&str]) -> Result<(Verb, bool), String> {
    let usage = format!(
        "{} agent context move <id|path> <n|up|down|top|bottom> [--json]",
        cli()
    );
    let (mut token, mut to, mut json) = (String::new(), String::new(), false);
    for w in words {
        if *w == "--json" {
            json = true;
        } else if w.starts_with('-') {
            return Err(format!("unknown flag '{w}' \u{2014} usage: {usage}"));
        } else if token.is_empty() {
            token = (*w).to_string();
        } else if to.is_empty() {
            to = (*w).to_string();
        } else {
            return Err(format!("unexpected extra argument '{w}'"));
        }
    }
    if token.is_empty() || to.is_empty() {
        return Err(format!("usage: {usage}"));
    }
    // `printf '%s' "$where" | grep -Eq '^(up|down|top|bottom|[0-9]+)$'`:
    // any line of it.
    let fits = to.split('\n').any(|l| {
        matches!(l, "up" | "down" | "top" | "bottom")
            || (!l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()))
    });
    if !fits {
        return Err(format!(
            "invalid move target '{to}' \u{2014} use n (0-based), up, down, top, or bottom"
        ));
    }
    Ok((Verb::Move { token, to }, json))
}

/// The help page `cmd_help_agent_context` prints.
fn help() -> &'static str {
    crate::text::help("agent/context").expect("cli/text/help/agent/context.txt is embedded")
}

/// `cmd_agent_context`, past the gate.
pub fn run(client: &mut Client, closing: &mut Closing, a: &ContextArgs) -> Exit {
    let mut project_path: Option<String> = None;
    for sel in &a.workspaces {
        apply_project(client, closing, sel);
        project_path = Some(client.project.clone());
    }
    if let Some(hint) = &a.missing {
        out::stderr_line(&err_json("bad_usage", hint));
        return Exit::Usage;
    }
    let (verb, as_json) = match verb(&a.rest) {
        None => return Shown::out(help()).print(),
        Some(Err(hint)) => {
            out::stderr_line(&err_json("bad_usage", &hint));
            return Exit::Usage;
        }
        Some(Ok(v)) => v,
    };
    let project = project_of(project_path);
    let opener = Opener::from_client(client);
    let ctx = Ctx {
        opener: &opener,
        project: &project,
    };
    match ctx.verb(&verb, as_json) {
        Ok(shown) | Err(shown) => shown.print(),
    }
}

/// `_wiki_apply_project <sel>`: the client's project becomes the folder,
/// the workspace the daemon resolves it to, or `sel` as typed.
fn apply_project(client: &mut Client, closing: &mut Closing, sel: &str) {
    if sel.is_empty() {
        return;
    }
    if std::path::Path::new(sel).is_dir() {
        client.project = crate::families::sim::cd_pwd(&crate::families::sim::logical_pwd(), sel)
            .unwrap_or_default();
        return;
    }
    let q = format!("q={}", quote(sel));
    let passport = client.passport().is_some();
    let got = match client.call(Channel::Tcp, RESOLVE, Call::Request(&[q.as_str()])) {
        Ok(r) => {
            closing.answer(&r, passport);
            resolved(&without_nuls(&r.body))
        }
        // Its line went to /dev/null; `_k2_end_command` still ends the
        // command from inside the pipe.
        Err(Error::NoAnswer(_)) => crate::families::end_in_pipe(),
        Err(_) => String::new(),
    };
    client.project = if got.is_empty() { sel.to_string() } else { got };
}

/// `_wiki_apply_project`'s reader of `/cli/workspace/resolve`, as `$(…)`
/// holds what it prints.
fn resolved(body: &[u8]) -> String {
    let text = decode(body);
    let text = if text.is_empty() { "{}".to_string() } else { text };
    let Ok(d @ Value::Object(_)) = pyjson::loads(&text) else {
        return String::new();
    };
    let v = ["path", "projectPath", "project_path"]
        .iter()
        .find_map(|k| at(&d, k).filter(|v| py::truthy(v)));
    let mut out = v.map_or_else(String::new, py::str_of);
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// `_context_project_path`: `PROJECT_PATH` (or the workspace a flag
/// set), else this folder, made absolute.
fn project_of(set: Option<String>) -> String {
    let p = set.or_else(|| {
        k2_client::env_names::first_nonempty_os(k2_client::env_names::PROJECT_PATH)
            .map(|(_, v)| v.to_string_lossy().into_owned())
    });
    match p {
        Some(p) => abs_user(&p),
        None => std::env::current_dir()
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// One verb's calls.
struct Ctx<'a> {
    opener: &'a Opener,
    project: &'a str,
}

/// A verb's output, or what it stopped with.
type R<T> = Result<T, Shown>;

fn fail(code: &str, hint: &str, exit: Exit) -> Shown {
    Shown::err(err_json(code, hint) + "\n", exit)
}

impl Ctx<'_> {
    fn verb(&self, verb: &Verb, as_json: bool) -> R<Shown> {
        match verb {
            Verb::List => {
                let raw = self.http(Method::Get, LAYERS, &[], "")?;
                self.reply(&raw, as_json, true, |d| list_printer(d))
            }
            Verb::Catalog => {
                let raw = self.http(Method::Get, CATALOG, &[], "")?;
                self.reply(&raw, as_json, false, |d| catalog_printer(d))
            }
            Verb::Show { outline } => {
                let qs: &[(&str, &str)] = if *outline { &[("outline", "1")] } else { &[] };
                let raw = self.http(Method::Get, SHOW, qs, "")?;
                let outline = *outline;
                self.reply(&raw, as_json, true, |d| show_printer(d, outline))
            }
            Verb::Regen => {
                let body = dumps(&obj(vec![("project", s(self.project))]));
                let raw = self.http(Method::Post, REGEN, &[], &body)?;
                self.reply(&raw, as_json, true, |d| Ok(regen_line(d)))
            }
            Verb::Add { target, label } => {
                let is_path = target.contains('/')
                    || target.starts_with('.')
                    || target.ends_with(".md")
                    || target.ends_with(".markdown")
                    || target.starts_with('~');
                let mut body = vec![("project", s(self.project))];
                body.push((if is_path { "path" } else { "catalog" }, s(target)));
                if !label.is_empty() {
                    body.push(("label", s(label)));
                }
                let raw = self.http(Method::Post, ADD, &[], &dumps(&obj(body)))?;
                self.reply(&raw, as_json, true, add_printer)
            }
            Verb::Remove { token } => {
                let lid = self.layer_id(token)?;
                let body = dumps(&obj(vec![("project", s(self.project)), ("id", s(&lid))]));
                let raw = self.http(Method::Post, REMOVE, &[], &body)?;
                self.reply(&raw, as_json, true, |_| Ok(format!("removed layer {lid}\n")))
            }
            Verb::Enabled { on, token } => {
                let lid = self.layer_id(token)?;
                let body = dumps(&obj(vec![
                    ("project", s(self.project)),
                    ("id", s(&lid)),
                    ("enabled", Value::Bool(*on)),
                ]));
                let raw = self.http(Method::Post, SET_ENABLED, &[], &body)?;
                let word = if *on { "enabled" } else { "disabled" };
                self.reply(&raw, as_json, true, |_| Ok(format!("{word} layer {lid}\n")))
            }
            Verb::Move { token, to } => {
                let lid = self.layer_id(token)?;
                let mut body = vec![("project", s(self.project)), ("id", s(&lid))];
                if !to.is_empty() && to.bytes().all(|b| b.is_ascii_digit()) {
                    let digits = to.trim_start_matches('0');
                    let digits = if digits.is_empty() { "0" } else { digits };
                    body.push(("position", Value::Int(digits.to_string())));
                } else {
                    body.push(("direction", s(to)));
                }
                let raw = self.http(Method::Post, MOVE, &[], &dumps(&obj(body)))?;
                self.reply(&raw, as_json, true, |_| {
                    Ok(format!("moved layer {lid} \u{2192} {to}\n"))
                })
            }
        }
    }

    /// After `raw=$(_context_http …)`: the soft warnings (not for
    /// `catalog`), then `--json`'s `printf '%s\n' "$raw"` or the printer.
    fn reply(
        &self,
        raw: &str,
        as_json: bool,
        warn: bool,
        printer: impl FnOnce(&Value) -> Result<String, (String, Raise)>,
    ) -> R<Shown> {
        let warnings = if warn {
            soft_warn(raw)?
        } else {
            String::new()
        };
        if as_json {
            return Ok(Shown {
                stdout: format!("{raw}\n"),
                stderr: warnings,
                exit: Exit::Ok,
            });
        }
        let d = pyjson::loads(raw).unwrap_or(Value::Null);
        Ok(match printer(&d) {
            Ok(text) => Shown {
                stdout: text,
                stderr: warnings,
                exit: Exit::Ok,
            },
            Err((stdout, r)) => raised(stdout, warnings, &r),
        })
    }

    /// `_context_http <project> <method> <route> <qs> <body>`: the answer
    /// as `raw=$(…)` holds it (trailing newlines gone), or what it
    /// printed and its exit.
    fn http(&self, method: Method, route: &str, qs: &[(&str, &str)], body: &str) -> R<String> {
        let mut query = vec![("project".to_string(), self.project.to_string())];
        query.extend(qs.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let post = method == Method::Post;
        let timeout = if post { 60 } else { 30 };
        let mut r = Request::new(method, route, Duration::from_secs(timeout)).query(query);
        if let Some(t) = self.opener.token() {
            r = r.header("Authorization", format!("Bearer {t}"));
        }
        if post {
            let data = if body.is_empty() { "{}" } else { body };
            r = r
                .data(data.as_bytes().to_vec())
                .header("Content-Type", "application/json");
        }
        let (status, raw) = match self.opener.open(&r) {
            Outcome::Answer { status, body: Ok(b) } => (status, decode(&b)),
            Outcome::Answer { body: Err(e), .. } | Outcome::Raised(e) => {
                return Err(fail(
                    "daemon_unreachable",
                    &format!(
                        "no response from the {} daemon (is it running?): {}",
                        product(),
                        e.text()
                    ),
                    Exit::Error,
                ))
            }
            Outcome::HttpError { code, body } => {
                let raw = body.map(|b| decode(&b)).unwrap_or_default();
                if raw.is_empty() {
                    return Err(fail(
                        "daemon_error",
                        &format!("HTTP {code} on {route}"),
                        Exit::Error,
                    ));
                }
                (code, raw)
            }
        };
        if py_strip(&raw).is_empty() {
            return Err(fail(
                "daemon_error",
                &format!("empty response from {route} (HTTP {status})"),
                Exit::Error,
            ));
        }
        let Ok(d) = pyjson::loads(&raw) else {
            return Err(fail(
                "bad_response",
                &format!(
                    "daemon returned unparseable response from {route}: {}",
                    py::repr(&s(&cut(&raw, 200)))
                ),
                Exit::Error,
            ));
        };
        let none = |r: Raise| raised(String::new(), String::new(), &r);
        let err = py_get(&d, "error", line::HTTP_ERR)
            .map_err(none)?
            .cloned()
            .unwrap_or(Value::Null);
        if err != Value::Null {
            let err = match err {
                Value::Str(_) => obj(vec![("code", s("error")), ("hint", err)]),
                other => other,
            };
            let code = py_get(&err, "code", line::HTTP_ERR)
                .map_err(none)?
                .filter(|c| py::truthy(c))
                .cloned()
                .unwrap_or_else(|| s("error"));
            let line = dumps(&obj(vec![("error", err.clone())])) + "\n";
            let exit = match &code {
                Value::Str(t) => exit_for(&py::str_of(&Value::Str(t.clone())).to_lowercase()),
                other => {
                    return Err(raised(
                        String::new(),
                        line,
                        &Raise::no_attribute(line::HTTP_EXIT, other, "lower"),
                    ))
                }
            };
            return Err(Shown::err(line, exit));
        }
        let has = |k: &str| at(&d, k).is_some();
        if at(&d, "ok") == Some(&Value::Bool(false))
            && !at(&d, "layers").is_some_and(py::truthy)
            && !has("pinned")
            && !has("catalog")
            && !has("content")
            && !has("outline")
            && !has("text")
        {
            let shown: String = dumps(&d).chars().take(300).collect();
            return Err(fail(
                "daemon_error",
                &format!("unexpected response: {shown}"),
                Exit::Error,
            ));
        }
        if status == 404 && !has("ok") && !has("layers") && !has("catalog") {
            return Err(fail(
                "not_found",
                &format!(
                    "route {route} not found (is the context-stack daemon build running?)"
                ),
                Exit::NotFound,
            ));
        }
        let mut raw = raw;
        while raw.ends_with('\n') {
            raw.pop();
        }
        Ok(raw)
    }

    /// `_context_resolve_layer_id <token> <project>`.
    fn layer_id(&self, token: &str) -> R<String> {
        match token {
            "pinned:agent" | "agent" => return Ok("pinned:agent".to_string()),
            "pinned:project" | "project" => return Ok("pinned:project".to_string()),
            "pinned:tooling" | "tooling" => return Ok("pinned:tooling".to_string()),
            _ => {}
        }
        let raw = self.http(Method::Get, LAYERS, &[], "")?;
        let d = pyjson::loads(&raw).unwrap_or(Value::Null);
        match resolve_layer(&d, token) {
            Ok(Some(id)) => {
                let mut id = id;
                while id.ends_with('\n') {
                    id.pop();
                }
                Ok(id)
            }
            Ok(None) => Err(fail(
                "not_found",
                &format!(
                    "no context layer matches '{token}' \u{2014} see: {} agent context list",
                    cli()
                ),
                Exit::NotFound,
            )),
            Err(r) => Err(raised(String::new(), String::new(), &r)),
        }
    }
}

/// `_exit_for(code)` of `_context_http`.
fn exit_for(code: &str) -> Exit {
    match code {
        "bad_usage" | "path_escape" | "catalog_unknown" | "usage" | "invalid_move"
        | "invalid_position" => Exit::Usage,
        "duplicate_layer" | "refused" | "precondition" => Exit::Refused,
        "not_found" => Exit::NotFound,
        _ => Exit::Error,
    }
}

/// `_context_print_soft_warn "$raw"`: its lines on stderr.
fn soft_warn(raw: &str) -> R<String> {
    let d = if raw.is_empty() {
        obj(vec![])
    } else {
        match pyjson::loads(raw) {
            Ok(d) => d,
            Err(_) => return Ok(String::new()),
        }
    };
    let mut warn = Value::Null;
    for k in ["softWarn", "soft_warn", "warn"] {
        match py_get(&d, k, line::WARN) {
            Ok(Some(v)) if py::truthy(v) => {
                warn = v.clone();
                break;
            }
            Ok(_) => {}
            Err(r) => return Err(raised(String::new(), String::new(), &r)),
        }
    }
    Ok(match &warn {
        v if !py::truthy(v) => String::new(),
        Value::Array(items) => items
            .iter()
            .map(|w| format!("warning: {}\n", py::str_of(w)))
            .collect(),
        Value::Object(_) => {
            let pick = at(&warn, "hint")
                .filter(|v| py::truthy(v))
                .or_else(|| at(&warn, "message").filter(|v| py::truthy(v)))
                .map_or_else(|| dumps(&warn), py::str_of);
            format!("warning: {pick}\n")
        }
        other => format!("warning: {}\n", py::str_of(other)),
    })
}

/// `(x or "")` of a value Python calls a string method on; the
/// `AttributeError` for anything else truthy.
fn text_or_empty(v: Option<&Value>, method: &str, line: u32) -> Result<String, Raise> {
    match v {
        Some(x) if py::truthy(x) => match x {
            Value::Str(_) => Ok(py::str_of(x)),
            other => Err(Raise::no_attribute(line, other, method)),
        },
        _ => Ok(String::new()),
    }
}

/// `x.get(k)` on a value that must be a dict.
fn dget<'a>(x: &'a Value, k: &str, line: u32) -> Result<Option<&'a Value>, Raise> {
    py_get(x, k, line)
}

/// The first truthy of `x.get(k)` for each key.
fn first_of<'a>(x: &'a Value, keys: &[&str], line: u32) -> Result<Option<&'a Value>, Raise> {
    for k in keys {
        if let Some(v) = dget(x, k, line)?.filter(|v| py::truthy(v)) {
            return Ok(Some(v));
        }
    }
    Ok(None)
}

/// The resolver's `norm(p)`.
fn norm(p: Option<&Value>, line: u32) -> Result<String, Raise> {
    let p = text_or_empty(p, "replace", line)?.replace('\\', "/");
    Ok(py_strip(&p)
        .trim_start_matches(['.', '/'])
        .to_string())
}

/// The resolver's Python: the layer id it prints, or `None` (exit 4).
fn resolve_layer(d: &Value, token: &str) -> Result<Option<String>, Raise> {
    let l = line::RESOLVE_PINNED;
    let empty = Value::Array(Vec::new());
    let layers = first_of(d, &["layers", "optional"], l)?.unwrap_or(&empty).clone();
    let pinned = first_of(d, &["pinned"], l)?.unwrap_or(&empty).clone();
    let lower = token.to_lowercase();
    for p in py_iter(&pinned, l)? {
        let pid = dget(&p, "id", l)?
            .filter(|v| py::truthy(v))
            .map_or_else(String::new, py::str_of);
        if pid == token || pid.is_empty() {
            return Ok(Some(if pid.is_empty() { token.to_string() } else { pid }));
        }
        let lab = py_strip(&text_or_empty(dget(&p, "label", l)?, "strip", l)?).to_lowercase();
        if !lab.is_empty() && lab == lower {
            return Ok(Some(if pid.is_empty() { token.to_string() } else { pid }));
        }
    }
    let l = line::RESOLVE_LAYERS;
    let items = py_iter(&layers, l)?;
    let id_of = |x: &Value| -> Result<String, Raise> {
        Ok(at(x, "id").map_or_else(|| "None".to_string(), py::str_of))
    };
    for x in &items {
        let id = dget(x, "id", l)?.filter(|v| py::truthy(v)).map_or_else(String::new, py::str_of);
        if id == token {
            return id_of(x).map(Some);
        }
    }
    let nt = norm(Some(&s(token)), l)?;
    for x in &items {
        let p = norm(dget(x, "path", l)?, l)?;
        let raw = dget(x, "path", l)?
            .filter(|v| py::truthy(v))
            .cloned()
            .unwrap_or_else(|| s(""));
        if p == nt || p.ends_with(&format!("/{nt}")) || ops::py_eq(&raw, &s(token)) {
            return id_of(x).map(Some);
        }
    }
    for x in &items {
        let lab = py_strip(&text_or_empty(dget(x, "label", l)?, "strip", l)?).to_string();
        if !lab.is_empty() && lab.to_lowercase() == lower {
            return id_of(x).map(Some);
        }
    }
    for x in &items {
        let src = text_or_empty(dget(x, "source", l)?, "endswith", l)?;
        if src == token
            || src.ends_with(&format!(":{token}"))
            || src.replace("preset:", "") == token
        {
            return id_of(x).map(Some);
        }
    }
    Ok(None)
}

/// `on_off(en)`.
fn on_off(en: Option<&Value>) -> &'static str {
    let on = match en {
        None | Some(Value::Null) => true,
        Some(v) => {
            ops::py_eq(v, &Value::Bool(true))
                || ops::py_eq(v, &s("1"))
                || ops::py_eq(v, &s("true"))
                || ops::py_eq(v, &s("on"))
        }
    };
    if on {
        "on "
    } else {
        "off"
    }
}

/// `int(L.get("position", 0))`, `0` for what raises `TypeError` or
/// `ValueError`.
fn position_key(v: Option<&Value>, line: u32) -> Result<i128, Raise> {
    Ok(match v {
        None => 0,
        Some(Value::Bool(b)) => i128::from(*b),
        Some(Value::Int(d)) => d.parse().unwrap_or(0),
        Some(Value::Float(f)) if f.is_nan() => 0,
        Some(Value::Float(f)) if f.is_infinite() => {
            return Err(Raise::at(
                line,
                "OverflowError: cannot convert float infinity to integer",
            ))
        }
        Some(Value::Float(f)) => f.trunc() as i128,
        Some(Value::Str(t)) => {
            let w = py_strip(&py::str_of(&Value::Str(t.clone()))).replace('_', "");
            w.parse().unwrap_or(0)
        }
        Some(_) => 0,
    })
}

/// `_context_format_list`.
pub fn list_printer(d: &Value) -> Result<String, (String, Raise)> {
    let mut out = String::new();
    match list_lines(d, &mut out) {
        Ok(()) => Ok(out),
        Err(r) => Err((out, r)),
    }
}

fn list_lines(d: &Value, out: &mut String) -> Result<(), Raise> {
    let cli = cli();
    let empty = Value::Array(Vec::new());
    let pinned = first_of(d, &["pinned"], line::LIST_PINNED)?.unwrap_or(&empty).clone();
    let layers = first_of(d, &["layers", "optional"], line::LIST_PINNED)?
        .unwrap_or(&empty)
        .clone();
    out.push_str(&format!(
        "PINNED  (system \u{2014} on|off via: {cli} agent context on|off pinned:agent|project|tooling)\n"
    ));
    if !py::truthy(&pinned) {
        out.push_str("  on   Role (persona)     .k2/agent/ROLE.md\n");
        out.push_str("  on   Project   .k2/PROJECT.md\n");
        out.push_str("  on   Tooling   (generated k2-cli pointer)\n");
    } else {
        let l = line::LIST_PINNED;
        for p in py_iter(&pinned, l)? {
            let label = first_of(&p, &["label", "name", "key", "id"], l)?
                .cloned()
                .unwrap_or_else(|| s("layer"));
            let label = match label {
                Value::Str(_) => py_strip(&py::str_of(&label)).to_string(),
                other => return Err(Raise::no_attribute(l, &other, "strip")),
            };
            let short = if label.is_empty() {
                "layer".to_string()
            } else {
                label
                    .split(char::is_whitespace)
                    .find(|w| !w.is_empty())
                    .unwrap_or("")
                    .to_string()
            };
            let mut path = first_of(&p, &["path", "detail"], l)?
                .map_or_else(String::new, py::str_of);
            let blank = !first_of(&p, &["path", "detail"], l)?.is_some();
            if blank {
                let generated = at(&p, "generated").is_some_and(py::truthy)
                    || at(&p, "kind").is_some_and(|k| ops::py_eq(k, &s("tooling")))
                    || label.to_lowercase().contains("tooling");
                if generated {
                    path = "(generated k2-cli pointer)".to_string();
                }
            }
            let state = on_off(Some(at(&p, "enabled").unwrap_or(&Value::Bool(true))));
            let short9: String = short.chars().take(9).collect();
            out.push_str(&format!("  {state}  {} {path}\n", ljust(&short9, 9)));
        }
    }
    out.push('\n');
    out.push_str("OPTIONAL  (order = AGENTS.md)\n");
    if !py::truthy(&layers) {
        out.push_str(&format!(
            "  (none \u{2014} add with: {cli} agent context add <path|catalog-id>)\n"
        ));
        return Ok(());
    }
    let l = line::LIST_LAYERS;
    let mut keyed = Vec::new();
    for x in py_iter(&layers, l)? {
        let k = position_key(dget(&x, "position", l)?, l)?;
        keyed.push((k, x));
    }
    keyed.sort_by_key(|(k, _)| *k);
    for (_, x) in keyed {
        let pos = at(&x, "position").cloned().unwrap_or_else(|| s("?"));
        let state = on_off(Some(at(&x, "enabled").unwrap_or(&Value::Bool(true))));
        let mut label = py_strip(&text_or_empty(at(&x, "label"), "strip", l)?).to_string();
        if label.is_empty() {
            let src = text_or_empty(at(&x, "source"), "startswith", l)?;
            if let Some(rest) = src.strip_prefix("catalog:") {
                label = rest.to_string();
            } else {
                let path = text_or_empty(at(&x, "path"), "rsplit", l)?;
                label = if path.is_empty() {
                    match at(&x, "id").filter(|v| py::truthy(v)) {
                        None => "?".to_string(),
                        Some(Value::Str(t)) => {
                            py::str_of(&Value::Str(t.clone())).chars().take(12).collect()
                        }
                        Some(other) => {
                            return Err(Raise::at(
                                l,
                                format!(
                                    "TypeError: '{}' object is not subscriptable",
                                    py::type_name(other)
                                ),
                            ))
                        }
                    }
                } else {
                    let tail = path.rsplit('/').next().unwrap_or("");
                    match tail.rsplit_once('.') {
                        Some((head, _)) => head.to_string(),
                        None => tail.to_string(),
                    }
                };
            }
        }
        let path = at(&x, "path")
            .filter(|v| py::truthy(v))
            .map_or_else(String::new, py::str_of);
        let missing = match at(&x, "exists") {
            Some(e)
                if ops::py_eq(e, &Value::Bool(false))
                    || ops::py_eq(e, &Value::Int("0".to_string()))
                    || ops::py_eq(e, &s("false")) =>
            {
                "  MISSING"
            }
            _ => "",
        };
        let label12: String = label.chars().take(12).collect();
        out.push_str(&format!(
            "  {}  {state}  {} {path}{missing}\n",
            py::str_of(&pos),
            ljust(&label12, 12)
        ));
    }
    Ok(())
}

/// `_context_format_catalog`.
pub fn catalog_printer(d: &Value) -> Result<String, (String, Raise)> {
    let mut out = String::new();
    match catalog_lines(d, &mut out) {
        Ok(()) => Ok(out),
        Err(r) => Err((out, r)),
    }
}

fn catalog_lines(d: &Value, out: &mut String) -> Result<(), Raise> {
    let l = line::CATALOG;
    let empty = Value::Array(Vec::new());
    let items = first_of(d, &["catalog", "items"], l)?.unwrap_or(&empty).clone();
    if !py::truthy(&items) {
        out.push_str("(no context catalog items registered)\n");
        return Ok(());
    }
    let row = |a: &str, b: &str, c: &str, d: &str| {
        format!("{}  {}  {}  {d}\n", ljust(a, 18), ljust(b, 6), ljust(c, 36))
    };
    out.push_str(&row("ID", "KIND", "PATH", "LABEL"));
    for p in py_iter(&items, l)? {
        if !matches!(p, Value::Object(_)) {
            out.push_str(&py::str_of(&p));
            out.push('\n');
            continue;
        }
        let pid = at(&p, "id").filter(|v| py::truthy(v)).cloned().unwrap_or_else(|| s("?"));
        let path = first_of(&p, &["path", "defaultPath"], l)?
            .cloned()
            .unwrap_or_else(|| s(""));
        let label = first_of(&p, &["label", "name"], l)?
            .cloned()
            .unwrap_or_else(|| s(""));
        let mut kind = py_strip(&text_or_empty(at(&p, "kind"), "strip", l)?).to_string();
        if kind.is_empty() {
            let src = text_or_empty(at(&p, "source"), "endswith", l)?;
            let (Value::Str(_), Value::Str(_)) = (&pid, &path) else {
                return Err(Raise::at(
                    l,
                    "TypeError: argument of type is not iterable or has no startswith",
                ));
            };
            let (pid_s, path_s) = (py::str_of(&pid), py::str_of(&path));
            kind = if pid_s.contains("roster")
                || src.ends_with("-roster")
                || src.contains("connections-roster")
            {
                "live"
            } else if pid_s.starts_with("wiki:index")
                || pid_s.starts_with("wiki:home")
                || path_s.contains("/wiki/")
            {
                "path"
            } else if pid_s.contains("hygiene") || pid_s.contains("pack") || path_s.contains("catalog/")
            {
                "static"
            } else {
                "?"
            }
            .to_string();
        }
        out.push_str(&row(
            &py::str_of(&pid),
            &kind,
            &py::str_of(&path),
            &py::str_of(&label),
        ));
        let desc = py_strip(&text_or_empty(at(&p, "description"), "strip", l)?).to_string();
        if !desc.is_empty() {
            out.push_str(&format!("    {desc}\n"));
        }
    }
    Ok(())
}

/// `int(level)` of an outline item.
fn level_of(v: &Value, line: u32) -> Result<i128, Raise> {
    match v {
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Int(d) => Ok(d.parse().unwrap_or(i128::MAX)),
        Value::Float(f) if f.is_finite() => Ok(f.trunc() as i128),
        Value::Str(t) => {
            let w = py::str_of(&Value::Str(t.clone()));
            py_strip(&w).replace('_', "").parse().map_err(|_| {
                Raise::at(
                    line,
                    format!(
                        "ValueError: invalid literal for int() with base 10: {}",
                        py::repr(v)
                    ),
                )
            })
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

/// `_context_format_show`.
pub fn show_printer(d: &Value, outline: bool) -> Result<String, (String, Raise)> {
    let mut out = String::new();
    match show_lines(d, outline, &mut out) {
        Ok(()) => Ok(out),
        Err(r) => Err((out, r)),
    }
}

fn show_lines(d: &Value, outline: bool, out: &mut String) -> Result<(), Raise> {
    let l = line::SHOW;
    if outline {
        if let Some(items) = first_of(d, &["outline", "sections", "headings"], l)? {
            for it in py_iter(items, l)? {
                if let Value::Object(_) = it {
                    let level = first_of(&it, &["level", "depth"], l)?
                        .cloned()
                        .unwrap_or(Value::Int("1".to_string()));
                    let title = first_of(&it, &["title", "heading", "text"], l)?
                        .cloned()
                        .unwrap_or_else(|| s(""));
                    let n = (level_of(&level, l)? - 1).clamp(0, 10_000) as usize;
                    out.push_str(&format!("{}{}\n", "  ".repeat(n), py::str_of(&title)));
                } else {
                    out.push_str(&py::str_of(&it));
                    out.push('\n');
                }
            }
            return Ok(());
        }
    }
    if let Some(text) = first_of(d, &["content", "text", "body", "preview"], l)? {
        let Value::Str(_) = text else {
            return Err(Raise::no_attribute(l, text, "endswith"));
        };
        let t = py::str_of(text);
        out.push_str(&t);
        if !t.ends_with('\n') {
            out.push('\n');
        }
        return Ok(());
    }
    let rest = match d {
        Value::Object(pairs) => Value::Object(
            pairs
                .iter()
                .filter(|(k, _)| *k != Text::from("ok"))
                .cloned()
                .collect(),
        ),
        other => other.clone(),
    };
    let opts = Options {
        indent: Some(2),
        ..Options::default()
    };
    out.push_str(&json::to_string(&rest, &opts).expect("json.dumps with ensure_ascii"));
    out.push('\n');
    Ok(())
}

/// `cmd_agent_context_add`'s printer.
fn add_printer(d: &Value) -> Result<String, (String, Raise)> {
    add_line(d).map_err(|r| (String::new(), r))
}

fn add_line(d: &Value) -> Result<String, Raise> {
    let l = line::ADD;
    let layer = first_of(d, &["layer", "added"], l)?
        .cloned()
        .unwrap_or_else(|| obj(vec![]));
    let pick = |keys: &[(&Value, &str)]| -> Result<Value, Raise> {
        for (x, k) in keys {
            if let Some(v) = dget(x, k, l)?.filter(|v| py::truthy(v)) {
                return Ok(v.clone());
            }
        }
        Ok(s(""))
    };
    let path = pick(&[(&layer, "path"), (d, "path")])?;
    let lab = pick(&[(&layer, "label")])?;
    let lid = pick(&[(&layer, "id"), (d, "id")])?;
    let mut bits = vec![s("added")];
    if py::truthy(&lab) {
        bits.push(lab);
    }
    if py::truthy(&path) {
        bits.push(path);
    }
    if py::truthy(&lid) && !bits.iter().any(|b| ops::py_eq(b, &lid)) {
        bits.push(s(&format!("(id {})", py::str_of(&lid))));
    }
    let mut words = Vec::new();
    for (i, b) in bits.iter().enumerate() {
        match b {
            Value::Str(_) => words.push(py::str_of(b)),
            other => {
                return Err(Raise::at(
                    l,
                    format!(
                        "TypeError: sequence item {i}: expected str instance, {} found",
                        py::type_name(other)
                    ),
                ))
            }
        }
    }
    let mut out = words.join(" ") + "\n";
    if at(d, "regenerated") == Some(&Value::Bool(false)) {
        out.push_str(&format!(
            "(AGENTS.md not regenerated \u{2014} run: {} agent context regen)\n",
            cli()
        ));
    } else if ["regen", "regenerated", "agentsMd"]
        .iter()
        .any(|k| at(d, k).is_some_and(py::truthy))
    {
        out.push_str("AGENTS.md regenerated.\n");
    }
    Ok(out)
}

/// `cmd_agent_context_regen`'s printer.
fn regen_line(d: &Value) -> String {
    let path = ["path", "agentsMd", "agents_md"]
        .iter()
        .find_map(|k| at(d, k).filter(|v| py::truthy(v)))
        .map_or_else(|| ".k2/AGENTS.md".to_string(), py::str_of);
    let bytes = match at(d, "bytes").filter(|v| py::truthy(v)) {
        Some(b) => Some(b.clone()),
        None => at(d, "size").cloned(),
    };
    match bytes {
        Some(b) if b != Value::Null => {
            format!("regenerated {path} ({} bytes)\n", str_or(Some(&b), ""))
        }
        _ => format!("regenerated {path}\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::out::json::parse;

    #[test]
    fn the_flags_before_the_verb() {
        let a = parse_words(&["--workspace", "a", "list", "--project=b", "--json"]);
        assert_eq!(a.workspaces, vec!["a", "b"]);
        assert_eq!(a.rest, vec!["list", "--json"]);
        let a = parse_words(&["list", "--project"]);
        assert!(a.missing.is_some());
    }

    fn parse_words(w: &[&str]) -> ContextArgs {
        super::parse(w)
    }

    #[test]
    fn move_targets() {
        assert!(move_words(&["a", "up"]).is_ok());
        assert!(move_words(&["a", "007"]).is_ok());
        assert!(move_words(&["a", "x\nup"]).is_ok());
        assert!(move_words(&["a", "-1"]).is_err());
        assert!(move_words(&["a", "sideways"]).is_err());
    }

    #[test]
    fn the_layer_list() {
        let d = parse(
            r#"{"ok":true,"pinned":[{"id":"pinned:agent","label":"Agent (persona)","path":".k2/agent/ROLE.md"},{"id":"pinned:tooling","label":"Tooling","enabled":false}],
            "layers":[{"id":"b","position":1,"path":"docs/b.md"},{"id":"a","position":0,"source":"catalog:wiki-index","exists":false,"path":"wiki/x.md"}]}"#,
        )
        .unwrap();
        let text = list_printer(&d).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], "  on   Agent     .k2/agent/ROLE.md");
        assert_eq!(lines[2], "  off  Tooling   (generated k2-cli pointer)");
        assert_eq!(lines[5], "  0  on   wiki-index   wiki/x.md  MISSING");
        assert_eq!(lines[6], "  1  on   b            docs/b.md");
    }

    #[test]
    fn resolving_a_layer() {
        let d = parse(
            r#"{"pinned":[{"id":"pinned:agent","label":"Agent"}],"layers":[{"id":"L1","path":"./docs/a.md","label":"Docs","source":"preset:wiki-index"}]}"#,
        )
        .unwrap();
        assert_eq!(resolve_layer(&d, "L1").unwrap().as_deref(), Some("L1"));
        assert_eq!(resolve_layer(&d, "a.md").unwrap().as_deref(), Some("L1"));
        assert_eq!(resolve_layer(&d, "docs").unwrap().as_deref(), Some("L1"));
        assert_eq!(resolve_layer(&d, "wiki-index").unwrap().as_deref(), Some("L1"));
        assert_eq!(resolve_layer(&d, "agent").unwrap().as_deref(), Some("pinned:agent"));
        assert_eq!(resolve_layer(&d, "nope").unwrap(), None);
    }

    #[test]
    fn the_regen_line() {
        let d = parse(r#"{"ok":true,"bytes":0,"size":12}"#).unwrap();
        assert_eq!(regen_line(&d), "regenerated .k2/AGENTS.md (12 bytes)\n");
        let d = parse(r#"{"ok":true,"path":"/w/.k2/AGENTS.md"}"#).unwrap();
        assert_eq!(regen_line(&d), "regenerated /w/.k2/AGENTS.md\n");
    }

    #[test]
    fn show_falls_back_to_the_json() {
        let d = parse(r#"{"ok":true,"layers":[1]}"#).unwrap();
        assert_eq!(
            show_printer(&d, false).unwrap(),
            "{\n  \"layers\": [\n    1\n  ]\n}\n"
        );
    }
}
