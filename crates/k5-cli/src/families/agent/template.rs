//! `cmd_agent_template` (`cli/src/agent-template.sh`): hireable agent
//! types. `list` and `show` are `cli_request` GETs on the left of a pipe
//! into `_agent_template_print`; `save`, `add`, `remove`, `upgrade` and
//! `diff` are `cli_post_json` POSTs (the session's passport in a session,
//! `_k2_project_token`) into `_agent_template_write_print`.

use super::{cli, obj, s};
use crate::exit::Exit;
use crate::families::{
    end_in_pipe, ljust, piped_body, pipefail, py_get, py_iter, request_piped_tcp, Piped, Raise,
    Shown,
};
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::{json as pyjson, ops};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::client::{Call, Channel, Error};
use crate::transport::encode::quote;
use crate::transport::urllib::decode;
use crate::transport::Client;

const LIST: &str = "/cli/agent-templates";
const SHOW: &str = "/cli/agent-templates/show";

/// The readers' lines (`<string>`).
mod line {
    pub const ERR: u32 = 9;
    pub const LIST: u32 = 27;
    pub const SHOW: u32 = 40;
    pub const WRITE: u32 = 30;
}

/// One template verb and its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    List { all: bool },
    Show { id: String, files: bool },
    Save {
        ws: String,
        id: String,
        name: String,
        summary: Option<String>,
        wiki: Vec<String>,
        dry: bool,
    },
    Add { dir: String, dry: bool },
    Remove { id: String, version: String },
    /// `upgrade <ws>` or `diff <ws>`.
    Workspace { diff: bool, ws: String, dry: bool },
    /// `diff <id> <vA> <vB>`: the versions with one leading `v` off.
    Versions { id: String, a: String, b: String },
}

/// The words after `template`: the verb and `--json`, or the usage line
/// (exit 2); `None` for what the shell answers itself (the help page).
pub fn parse(words: &[&str]) -> Option<Result<(Verb, bool), String>> {
    let cli = cli();
    let (sub, rest) = match words.split_first() {
        None => return None,
        Some((s, r)) => (*s, r),
    };
    let unknown_flag = |w: &str| format!("Unknown flag: {w} (see: {cli} agent template --help)");
    let mut json = false;
    let verb = match sub {
        "" | "help" => return None,
        "list" => {
            let mut all = false;
            for w in rest {
                match *w {
                    "--all" => all = true,
                    "--json" => json = true,
                    _ => {
                        return Some(Err(format!(
                            "Unknown argument: {w} (see: {cli} agent template --help)"
                        )))
                    }
                }
            }
            Verb::List { all }
        }
        "show" => {
            let (mut id, mut files) = (String::new(), false);
            for w in rest {
                match *w {
                    "--files" => files = true,
                    "--json" => json = true,
                    _ if w.starts_with('-') => return Some(Err(unknown_flag(w))),
                    _ if !id.is_empty() => {
                        return Some(Err(format!(
                            "Usage: {cli} agent template show <id> (one id)"
                        )))
                    }
                    _ => id = (*w).to_string(),
                }
            }
            if id.is_empty() {
                return Some(Err(format!(
                    "Usage: {cli} agent template show <id>  (ids: {cli} agent template list)"
                )));
            }
            Verb::Show { id, files }
        }
        "save" => {
            let (mut ws, mut id, mut name) = (String::new(), String::new(), String::new());
            let (mut summary, mut wiki, mut dry) = (None, Vec::new(), false);
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i];
                let value = |need: &str| rest.get(i + 1).map(|v| v.to_string()).ok_or_else(|| need.to_string());
                match w {
                    "--id" => match value("--id needs a template id") {
                        Ok(v) => {
                            id = v;
                            i += 1;
                        }
                        Err(e) => return Some(Err(e)),
                    },
                    "--name" => match value("--name needs a name") {
                        Ok(v) => {
                            name = v;
                            i += 1;
                        }
                        Err(e) => return Some(Err(e)),
                    },
                    "--summary" => match value("--summary needs a line of text") {
                        Ok(v) => {
                            summary = Some(v);
                            i += 1;
                        }
                        Err(e) => return Some(Err(e)),
                    },
                    "--wiki" => match value("--wiki needs a note name") {
                        Ok(v) => {
                            wiki.push(v);
                            i += 1;
                        }
                        Err(e) => return Some(Err(e)),
                    },
                    "--dry-run" => dry = true,
                    "--json" => json = true,
                    _ if w.starts_with('-') => return Some(Err(unknown_flag(w))),
                    _ if !ws.is_empty() => {
                        return Some(Err(format!(
                            "Usage: {cli} agent template save <ws> --id <id> (one workspace)"
                        )))
                    }
                    _ => ws = w.to_string(),
                }
                i += 1;
            }
            if ws.is_empty() || id.is_empty() {
                return Some(Err(format!(
                    "Usage: {cli} agent template save <ws> --id <id> [--name <n>] [--summary <s>] [--wiki <note>]... [--dry-run]  (<ws>: a folder or handle; . for this one)"
                )));
            }
            Verb::Save {
                ws,
                id,
                name,
                summary,
                wiki,
                dry,
            }
        }
        "add" => {
            let (mut dir, mut dry) = (String::new(), false);
            for w in rest {
                match *w {
                    "--dry-run" => dry = true,
                    "--json" => json = true,
                    _ if w.starts_with('-') => return Some(Err(unknown_flag(w))),
                    _ if !dir.is_empty() => {
                        return Some(Err(format!(
                            "Usage: {cli} agent template add <dir> (one folder)"
                        )))
                    }
                    _ => dir = (*w).to_string(),
                }
            }
            if dir.is_empty() {
                return Some(Err(format!(
                    "Usage: {cli} agent template add <dir> [--dry-run]  (a folder with template.toml and ROLE.md)"
                )));
            }
            Verb::Add { dir, dry }
        }
        "remove" | "rm" => {
            let (mut id, mut version) = (String::new(), String::new());
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i];
                match w {
                    "--version" => {
                        let Some(v) = rest.get(i + 1) else {
                            return Some(Err("--version needs a number".to_string()));
                        };
                        version = v.strip_prefix('v').unwrap_or(v).to_string();
                        i += 1;
                    }
                    "--json" => json = true,
                    _ if w.starts_with('-') => return Some(Err(unknown_flag(w))),
                    _ if !id.is_empty() => {
                        return Some(Err(format!(
                            "Usage: {cli} agent template remove <id> (one id)"
                        )))
                    }
                    _ => id = w.to_string(),
                }
                i += 1;
            }
            if id.is_empty() {
                return Some(Err(format!(
                    "Usage: {cli} agent template remove <id> [--version N]"
                )));
            }
            if !version.bytes().all(|b| b.is_ascii_digit()) {
                return Some(Err("--version takes a number (1, 2, ...)".to_string()));
            }
            Verb::Remove { id, version }
        }
        "upgrade" | "diff" => {
            let diff = sub == "diff";
            let (mut args, mut dry) = (Vec::new(), false);
            for w in rest {
                match *w {
                    "--dry-run" if diff => {
                        return Some(Err(
                            "diff writes nothing; --dry-run is for upgrade".to_string()
                        ))
                    }
                    "--dry-run" => dry = true,
                    "--json" => json = true,
                    _ if w.starts_with('-') => return Some(Err(unknown_flag(w))),
                    _ => args.push((*w).to_string()),
                }
            }
            match (args.len(), diff) {
                (1, _) => Verb::Workspace {
                    diff,
                    ws: args.remove(0),
                    dry,
                },
                (3, true) => {
                    let off = |v: &str| v.strip_prefix('v').unwrap_or(v).to_string();
                    Verb::Versions {
                        id: args[0].clone(),
                        a: off(&args[1]),
                        b: off(&args[2]),
                    }
                }
                (_, true) => return Some(Err(format!(
                    "Usage: {cli} agent template diff <ws> | <id> <vA> <vB>  (<ws>: a folder or handle; . for this one)"
                ))),
                (_, false) => return Some(Err(format!(
                    "Usage: {cli} agent template upgrade <ws> [--dry-run]  (<ws>: a folder or handle; . for this one)"
                ))),
            }
        }
        other => {
            return Some(Err(format!(
                "Unknown: {cli} agent template {other} (see: {cli} agent template --help)"
            )))
        }
    };
    Some(Ok((verb, json)))
}

/// The verb, past the gate.
pub fn run(client: &mut Client, closing: &mut Closing, verb: &Verb, as_json: bool) -> Exit {
    match verb {
        Verb::List { all } => {
            let args: &[&str] = if *all { &["all=1"] } else { &[] };
            let left = request_piped_tcp(client, closing, LIST, args);
            let shown = read_print(&decode(&left.body), Mode::List, as_json, false);
            pipefail(left.exit, shown.print())
        }
        Verb::Show { id, files } => {
            let q = format!("id={}", quote(id));
            let mut args = vec![q.as_str()];
            if *files {
                args.push("files=1");
            }
            let left = request_piped_tcp(client, closing, SHOW, &args);
            let shown = read_print(&decode(&left.body), Mode::Show, as_json, *files);
            pipefail(left.exit, shown.print())
        }
        Verb::Save {
            ws,
            id,
            name,
            summary,
            wiki,
            dry,
        } => {
            let ws = template_ws(client, ws);
            let mut body = vec![
                ("workspace", s(&ws)),
                ("id", s(id)),
                ("wiki", Value::Array(wiki.iter().map(|w| s(w)).collect())),
                ("dryRun", Value::Bool(*dry)),
            ];
            if !name.is_empty() {
                body.push(("name", s(name)));
            }
            if let Some(sum) = summary {
                body.push(("summary", s(sum)));
            }
            post(client, closing, "save", &obj(body), as_json)
        }
        Verb::Add { dir, dry } => {
            if !std::path::Path::new(dir).is_dir() {
                out::stderr_line(&format!("Error: no_folder: {dir} is not a folder"));
                return Exit::NotFound;
            }
            let dir = crate::families::sim::cd_pwd(&crate::families::sim::logical_pwd(), dir)
                .unwrap_or_default();
            let body = obj(vec![("dir", s(&dir)), ("dryRun", Value::Bool(*dry))]);
            post(client, closing, "add", &body, as_json)
        }
        Verb::Remove { id, version } => {
            let mut body = vec![("id", s(id))];
            if !version.is_empty() {
                let digits = version.trim_start_matches('0');
                let digits = if digits.is_empty() { "0" } else { digits };
                body.push(("version", Value::Int(digits.to_string())));
            }
            post(client, closing, "remove", &obj(body), as_json)
        }
        Verb::Workspace { diff, ws, dry } => {
            let ws = template_ws(client, ws);
            let body = obj(vec![("workspace", s(&ws)), ("dryRun", Value::Bool(*dry))]);
            post(client, closing, if *diff { "diff" } else { "upgrade" }, &body, as_json)
        }
        Verb::Versions { id, a, b } => {
            let (Some(a), Some(b)) = (py_int(a), py_int(b)) else {
                out::stderr_line(&format!(
                    "versions are numbers: {} agent template diff <id> 1 2",
                    cli()
                ));
                return Exit::Usage;
            };
            let body = obj(vec![
                ("id", s(id)),
                ("from", Value::Int(a)),
                ("to", Value::Int(b)),
            ]);
            post(client, closing, "diff", &body, as_json)
        }
    }
}

/// Python's `int(text)` for a version: its digits, or `None` for what
/// raises `ValueError`.
fn py_int(text: &str) -> Option<String> {
    let t = super::py_strip(text);
    let (neg, body) = match t.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if body.is_empty()
        || body.starts_with('_')
        || body.ends_with('_')
        || body.contains("__")
        || !body.bytes().all(|b| b.is_ascii_digit() || b == b'_')
    {
        return None;
    }
    let digits: String = body.chars().filter(|c| *c != '_').collect();
    let digits = digits.trim_start_matches('0');
    Some(match (digits.is_empty(), neg) {
        (true, _) => "0".to_string(),
        (false, true) => format!("-{digits}"),
        (false, false) => digits.to_string(),
    })
}

/// `_agent_template_ws <ws>`: `.` is this workspace (the passport's own
/// in a session), a folder is made absolute, a handle passes through.
fn template_ws(client: &mut Client, ws: &str) -> String {
    if ws == "." {
        crate::families::use_passport_project(client);
        return client.project.clone();
    }
    if std::path::Path::new(ws).is_dir() {
        return crate::families::sim::cd_pwd(&crate::families::sim::logical_pwd(), ws)
            .unwrap_or_default();
    }
    ws.to_string()
}

/// `_agent_template_post /cli/agent-templates/<mode> <body>`: the POST on
/// the left of the pipe, the write reader on the right.
fn post(client: &mut Client, closing: &mut Closing, mode: &str, body: &Value, as_json: bool) -> Exit {
    let route = format!("/cli/agent-templates/{mode}");
    let body = super::dumps(body);
    let passport = client.passport().is_some();
    let left = match client.call(Channel::Tcp, &route, Call::PostJson(body.as_bytes())) {
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
    };
    let shown = write_print(&decode(&left.body), mode, as_json);
    pipefail(left.exit, shown.print())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    List,
    Show,
}

fn indent2(v: &Value) -> String {
    let opts = Options {
        indent: Some(2),
        ..Options::default()
    };
    json::to_string(v, &opts).expect("json.dumps with ensure_ascii") + "\n"
}

/// `python3 -c` raised at `line`: what it printed, the traceback.
fn fail_at(stdout: String, stderr: String, r: &Raise) -> Shown {
    Shown {
        stdout,
        stderr: stderr + &r.text(),
        exit: Exit::Error,
    }
}

/// `"%s" % x` of what `x.get(k)` gave.
fn sv(v: Option<&Value>) -> String {
    ops::s_of(v)
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(py::truthy)
}

fn eq(v: Option<&Value>, w: &str) -> bool {
    v.is_some_and(|x| ops::py_eq(x, &s(w)))
}

/// `(code + ": ") if code else ""`.
fn code_head(code: &Value, l: u32) -> Result<String, Raise> {
    if !py::truthy(code) {
        return Ok(String::new());
    }
    match code {
        Value::Str(_) => Ok(format!("{}: ", py::str_of(code))),
        other => Err(Raise::at(
            l,
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                py::type_name(other)
            ),
        )),
    }
}

/// `"token" in hint.lower()`.
fn names_token(hint: &Value, l: u32) -> Result<bool, Raise> {
    match hint {
        Value::Str(_) => Ok(py::str_of(hint).to_lowercase().contains("token")),
        other => Err(Raise::no_attribute(l, other, "lower")),
    }
}

fn code_in(code: &Value, set: &[&str]) -> bool {
    set.iter().any(|c| ops::py_eq(code, &s(c)))
}

/// `_agent_template_print` on `raw`.
fn read_print(raw: &str, mode: Mode, as_json: bool, files: bool) -> Shown {
    let Ok(d) = pyjson::loads(raw) else {
        return Shown::err(
            format!("Error: daemon answered: {}\n", super::cut(raw, 300)),
            Exit::Error,
        );
    };
    let l = line::ERR;
    let err = match &d {
        Value::Object(_) => super::at(&d, "error").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    };
    if py::truthy(&err) {
        let (code, hint) = match &err {
            Value::Object(_) => (
                super::at(&err, "code").cloned().unwrap_or_else(|| s("")),
                super::at(&err, "hint").cloned().unwrap_or_else(|| s("")),
            ),
            other => (s(""), s(&py::str_of(other))),
        };
        let head = match code_head(&code, l) {
            Ok(h) => h,
            Err(r) => return fail_at(String::new(), String::new(), &r),
        };
        let line = format!("Error: {head}{}\n", py::str_of(&hint));
        let token = match names_token(&hint, l) {
            Ok(t) => t,
            Err(r) => return fail_at(String::new(), line, &r),
        };
        let exit = if code_in(&code, &["forbidden", "role_required"]) || token {
            Exit::Refused
        } else if code_in(&code, &["not_found", "unknown_template"]) {
            Exit::NotFound
        } else if code_in(&code, &["usage", "bad_template"]) {
            Exit::Usage
        } else {
            Exit::Error
        };
        return Shown::err(line, exit);
    }
    if as_json {
        return Shown::out(indent2(&d));
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    let done = match mode {
        Mode::List => list_lines(&d, &mut stdout, &mut stderr),
        Mode::Show => show_lines(&d, files, &mut stdout),
    };
    match done {
        Ok(()) => Shown {
            stdout,
            stderr,
            exit: Exit::Ok,
        },
        Err(r) => fail_at(stdout, stderr, &r),
    }
}

/// `join(", ", t[key])` as the printers write it.
fn joined(v: &Value, l: u32) -> Result<String, Raise> {
    ops::join(v, ", ", l)
}

fn list_lines(d: &Value, out: &mut String, err: &mut String) -> Result<(), Raise> {
    let l = line::LIST;
    let ts = match py_get(d, "templates", l)? {
        Some(v) if py::truthy(v) => v.clone(),
        _ => Value::Array(Vec::new()),
    };
    let ts = py_iter(&ts, l)?;
    let mut k2 = Vec::new();
    for t in &ts {
        if !eq(py_get(t, "origin", l)?, "library") {
            k2.push(t.clone());
        }
    }
    let mut mine = Vec::new();
    for t in &ts {
        if eq(py_get(t, "origin", l)?, "library") {
            mine.push(t.clone());
        }
    }
    let row = |t: &Value| -> Result<String, Raise> {
        let mut extra = Vec::new();
        if truthy(py_get(t, "aliases", l)?) {
            extra.push(format!("old id: {}", joined(super::at(t, "aliases").unwrap_or(&Value::Null), l)?));
        }
        if eq(super::at(t, "kind"), "system") {
            extra.push("system".to_string());
        }
        if eq(super::at(t, "origin"), "library") {
            extra.push(format!(
                "by {}, v{}",
                sv(super::at(t, "author")),
                sv(super::at(t, "version"))
            ));
        }
        let tail = if extra.is_empty() {
            String::new()
        } else {
            format!("  ({})", extra.join("; "))
        };
        let id = super::at(t, "id").ok_or_else(|| Raise::at(l, "KeyError: 'id'"))?;
        let name = super::at(t, "name").cloned().unwrap_or_else(|| s(""));
        let summary = super::at(t, "summary").cloned().unwrap_or_else(|| s(""));
        Ok(format!(
            "  {} {} {}{tail}\n",
            ljust(&py::str_of(id), 14),
            ljust(&py::str_of(&name), 12),
            py::str_of(&summary)
        ))
    };
    out.push_str(&format!(
        "Agent types (hire one: {} agent hire --template <id> --name <name>):\n",
        cli()
    ));
    for t in &k2 {
        out.push_str(&row(t)?);
    }
    if !mine.is_empty() {
        out.push('\n');
        out.push_str("Yours (saved on this server):\n");
        for t in &mine {
            out.push_str(&row(t)?);
        }
    }
    if let Some(problems) = super::at(d, "problems").filter(|v| py::truthy(v)) {
        for p in py_iter(problems, l)? {
            let dir = sv(py_get(&p, "dir", l)?);
            let e = sv(py_get(&p, "error", l)?);
            err.push_str(&format!("warning: {dir}: {e}\n"));
        }
    }
    Ok(())
}

fn show_lines(d: &Value, files: bool, out: &mut String) -> Result<(), Raise> {
    let l = line::SHOW;
    let empty = || obj(vec![]);
    let t = match py_get(d, "template", l)? {
        Some(v) if py::truthy(v) => v.clone(),
        _ => empty(),
    };
    let g = |k: &str| py_get(&t, k, l);
    out.push_str(&format!(
        "{} ({}) v{}, by {}\n",
        sv(g("name")?),
        sv(g("id")?),
        sv(g("version")?),
        sv(g("author")?)
    ));
    out.push_str(&format!(
        "  {}\n",
        g("summary")?.map_or_else(String::new, py::str_of)
    ));
    if truthy(g("aliases")?) {
        out.push_str(&format!("  Old id: {}\n", joined(g("aliases")?.unwrap_or(&Value::Null), l)?));
    }
    let ag = match g("agent")? {
        Some(v) if py::truthy(v) => v.clone(),
        _ => empty(),
    };
    let model = match py_get(&ag, "model", l)? {
        Some(m) if py::truthy(m) => match m {
            Value::Str(_) => format!("   Model: {}", py::str_of(m)),
            other => {
                return Err(Raise::at(
                    l,
                    format!(
                        "TypeError: can only concatenate str (not \"{}\") to str",
                        py::type_name(other)
                    ),
                ))
            }
        },
        _ => String::new(),
    };
    out.push_str(&format!(
        "  Role: {}   Tool: {}{model}\n",
        sv(py_get(&ag, "role", l)?),
        sv(py_get(&ag, "tool", l)?)
    ));
    if let Some(params) = g("params")?.filter(|v| py::truthy(v)) {
        let Value::Object(pairs) = params else {
            return Err(Raise::no_attribute(l, params, "items"));
        };
        out.push_str("  Params (--param key=value):\n");
        for (k, p) in pairs {
            out.push_str(&format!(
                "    {}: {} (up to {} characters{})\n",
                py::str_of(&Value::Str(k.clone())),
                sv(py_get(p, "label", l)?),
                sv(py_get(p, "max", l)?),
                if truthy(py_get(p, "required", l)?) { ", required" } else { "" }
            ));
        }
    }
    if let Some(qs) = g("questions")?.filter(|v| py::truthy(v)) {
        out.push_str("  Questions (answer at hire with --answer id=text; the agent asks its person the rest):\n");
        for q in py_iter(qs, l)? {
            let f = match py_get(&q, "fills", l)? {
                Some(v) if py::truthy(v) => v.clone(),
                _ => s(""),
            };
            let where_ = if ops::py_eq(&f, &s("role")) {
                "ROLE.md > What I know about my work".to_string()
            } else {
                match &f {
                    Value::Str(_) => {
                        let text = py::str_of(&f);
                        match text.strip_prefix("wiki:") {
                            Some(rest) => rest.replace('#', " > "),
                            None => text,
                        }
                    }
                    other => return Err(Raise::no_attribute(l, other, "startswith")),
                }
            };
            out.push_str(&format!(
                "    {}: {}  -> {where_}\n",
                sv(py_get(&q, "id", l)?),
                sv(py_get(&q, "ask", l)?)
            ));
        }
    }
    for (label, key) in [
        ("Wiki notes", "wiki"),
        ("Context layers", "catalog"),
        ("Own layers", "layers"),
        ("Skills", "skills"),
    ] {
        if let Some(v) = g(key)?.filter(|v| py::truthy(v)) {
            out.push_str(&format!("  {label}: {}\n", joined(v, l)?));
        }
    }
    if let Some(hb) = g("heartbeats")?.filter(|v| py::truthy(v)) {
        let mut bits = Vec::new();
        for h in py_iter(hb, l)? {
            let name = ops::index(&h, "name", l)?;
            let freq = ops::index(&h, "frequency", l)?;
            bits.push(format!("{} ({})", py::str_of(name), py::str_of(freq)));
        }
        out.push_str(&format!("  Heartbeats: {}\n", bits.join(", ")));
    }
    let perm = match g("permissions")? {
        Some(v) if py::truthy(v) => v.clone(),
        _ => empty(),
    };
    let mut sugg: Vec<Value> = match py_get(&perm, "expanded", l)? {
        Some(v) if py::truthy(v) => py_iter(v, l)?,
        _ => Vec::new(),
    };
    if truthy(py_get(&perm, "cards", l)?) {
        sugg.insert(0, s("permission cards"));
    }
    let sugg = if sugg.is_empty() {
        "nothing".to_string()
    } else {
        joined(&Value::Array(sugg), l)?
    };
    out.push_str(&format!("  Suggests: {sugg}\n"));
    out.push_str("    (suggestions only: a person turns a switch on, or the agent asks on a card)\n");
    if let Some(hello) = g("hello")?.filter(|v| py::truthy(v)) {
        let Value::Str(_) = hello else {
            return Err(Raise::no_attribute(l, hello, "replace"));
        };
        out.push_str(&format!(
            "  Hello: {}\n",
            py::str_of(hello).replace('\n', "\n         ")
        ));
    }
    if let Some(vs) = g("versions")?.filter(|v| py::truthy(v)) {
        let bits: Vec<String> = py_iter(vs, l)?
            .iter()
            .map(|v| format!("v{}", py::str_of(v)))
            .collect();
        out.push_str(&format!("  Versions: {}\n", bits.join(", ")));
    }
    out.push_str(&format!("  Hash: {}\n", sv(g("hash")?)));
    if files {
        let fs = match g("files")? {
            Some(v) if py::truthy(v) => v.clone(),
            _ => empty(),
        };
        let Value::Object(mut pairs) = fs else {
            return Err(Raise::no_attribute(l, &fs, "items"));
        };
        pairs.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (path, body) in pairs {
            let Value::Str(_) = body else {
                return Err(Raise::no_attribute(l, &body, "rstrip"));
            };
            out.push('\n');
            out.push_str(&format!(
                "\u{2500}\u{2500} {} \u{2500}\u{2500}\n",
                py::str_of(&Value::Str(path))
            ));
            out.push_str(py::str_of(&body).trim_end_matches('\n'));
            out.push('\n');
        }
    }
    Ok(())
}

/// `_agent_template_write_print <mode>` on `raw`.
fn write_print(raw: &str, mode: &str, as_json: bool) -> Shown {
    let Ok(d) = pyjson::loads(raw) else {
        return Shown::err(
            format!("Error: daemon answered: {}\n", super::cut(raw, 300)),
            Exit::Error,
        );
    };
    let l = line::WRITE;
    let err = match &d {
        Value::Object(_) => super::at(&d, "error").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    };
    if py::truthy(&err) {
        let err = match err {
            e @ Value::Object(_) => e,
            other => obj(vec![("code", s("")), ("hint", s(&py::str_of(&other)))]),
        };
        let code = super::at(&err, "code").cloned().unwrap_or_else(|| s(""));
        let hint = ["hint", "message"]
            .iter()
            .find_map(|k| super::at(&err, k).filter(|v| py::truthy(v)).cloned())
            .unwrap_or_else(|| s(""));
        let head = match code_head(&code, l) {
            Ok(h) => h,
            Err(r) => return fail_at(String::new(), String::new(), &r),
        };
        let line = format!("Error: {head}{}\n", py::str_of(&hint));
        let exit = if code_in(&code, &["usage", "bad_id", "bad_template", "no_charter"]) {
            Exit::Usage
        } else if code_in(&code, &["id_reserved", "id_taken", "secret_found", "too_large"]) {
            Exit::Error
        } else {
            let refused = code_in(
                &code,
                &[
                    "forbidden",
                    "role_required",
                    "own_workspace_only",
                    "not_author",
                    "owner_only",
                    "template_conflict",
                ],
            );
            let refused = refused
                || match names_token(&hint, l) {
                    Ok(t) => t,
                    Err(r) => return fail_at(String::new(), line, &r),
                };
            if refused {
                Exit::Refused
            } else if code_in(
                &code,
                &[
                    "not_found",
                    "unknown_template",
                    "no_workspace",
                    "no_folder",
                    "no_note",
                    "no_template",
                ],
            ) {
                Exit::NotFound
            } else {
                Exit::Error
            }
        };
        return Shown::err(line, exit);
    }
    if as_json {
        return Shown::out(indent2(&d));
    }
    let mut stdout = String::new();
    match write_lines(&d, mode, &mut stdout) {
        Ok(()) => Shown::out(stdout),
        Err(r) => fail_at(stdout, String::new(), &r),
    }
}

/// `", ".join("v%s" % v for v in x or [])`.
fn versions(v: Option<&Value>, l: u32) -> Result<String, Raise> {
    let items = match v {
        Some(x) if py::truthy(x) => py_iter(x, l)?,
        _ => Vec::new(),
    };
    Ok(items
        .iter()
        .map(|v| format!("v{}", py::str_of(v)))
        .collect::<Vec<_>>()
        .join(", "))
}

fn write_lines(d: &Value, mode: &str, out: &mut String) -> Result<(), Raise> {
    let l = line::WRITE;
    let has = |k: &str| match d {
        Value::Object(pairs) => pairs.iter().any(|(key, _)| *key == Text::from(k)),
        Value::Array(items) => items.iter().any(|x| ops::py_eq(x, &s(k))),
        _ => false,
    };
    if mode == "remove" {
        let gone = versions(py_get(d, "removed", l)?, l)?;
        let left = versions(py_get(d, "remaining", l)?, l)?;
        let still = if left.is_empty() {
            String::new()
        } else {
            format!("; still saved: {left}")
        };
        out.push_str(&format!("Removed {}: {gone}{still}\n", sv(py_get(d, "id", l)?)));
        return Ok(());
    }
    if mode == "save" || mode == "add" {
        let g = |k: &str| py_get(d, k, l);
        let dry = truthy(g("dryRun")?);
        out.push_str(&format!(
            "{} {} v{} (author {}) {} {}\n",
            if dry { "Would save" } else { "Saved" },
            sv(g("id")?),
            sv(g("version")?),
            sv(g("author")?),
            if dry { "to" } else { "at" },
            sv(g("path")?)
        ));
        out.push_str("  Files:\n");
        if let Some(fs) = g("files")?.filter(|v| py::truthy(v)) {
            for f in py_iter(fs, l)? {
                out.push_str(&format!(
                    "    {} ({} bytes)\n",
                    sv(py_get(&f, "path", l)?),
                    sv(py_get(&f, "bytes", l)?)
                ));
            }
        }
        if let Some(c) = g("catalog")?.filter(|v| py::truthy(v)) {
            out.push_str(&format!("  Context layers: {}\n", joined(c, l)?));
        }
        let tool = match g("tool")? {
            Some(v) if py::truthy(v) => py::str_of(v),
            _ => "default".to_string(),
        };
        out.push_str(&format!("  Tool: {tool}\n"));
        if let Some(h) = g("heartbeats")?.filter(|v| py::truthy(v)) {
            out.push_str(&format!("  Heartbeats: {}\n", joined(h, l)?));
        }
        let sugg = match g("suggest")? {
            Some(v) if py::truthy(v) => joined(v, l)?,
            _ => String::new(),
        };
        let sugg = if sugg.is_empty() { "nothing".to_string() } else { sugg };
        out.push_str(&format!(
            "  Suggests: {sugg}  (suggestions only: a hire never gets a switch from a template)\n"
        ));
        if let Some(sk) = g("skipped")?.filter(|v| py::truthy(v)) {
            for x in py_iter(sk, l)? {
                out.push_str(&format!(
                    "  Not taken: {} {} ({})\n",
                    sv(py_get(&x, "what", l)?),
                    sv(py_get(&x, "name", l)?),
                    sv(py_get(&x, "why", l)?)
                ));
            }
        }
        if dry {
            out.push_str("Nothing written (dry run). Run it again without --dry-run to save.\n");
        } else {
            out.push_str(&format!(
                "Hire from it: {} agent hire --template {} --name <name>\n",
                cli(),
                sv(g("id")?)
            ));
        }
        return Ok(());
    }
    if has("rows") {
        let g = |k: &str| py_get(d, k, l);
        let (a, b) = (
            g("from")?.cloned().unwrap_or(Value::Null),
            g("to")?.cloned().unwrap_or(Value::Null),
        );
        let same = ops::py_eq(&a, &b);
        if same {
            out.push_str(&format!(
                "{} template: already on v{} ({})\n",
                sv(g("name")?),
                py::str_of(&a),
                sv(g("id")?)
            ));
        } else {
            out.push_str(&format!(
                "{} template: v{} -> v{} ({}){}\n",
                sv(g("name")?),
                py::str_of(&a),
                py::str_of(&b),
                sv(g("id")?),
                if truthy(g("dryRun")?) { "  [dry run]" } else { "" }
            ));
        }
        if let Some(rows) = g("rows")?.filter(|v| py::truthy(v)) {
            for r in py_iter(rows, l)? {
                if eq(py_get(&r, "status", l)?, "unchanged") {
                    continue;
                }
                let reason = match py_get(&r, "reason", l)? {
                    Some(v) if py::truthy(v) => match v {
                        Value::Str(_) => format!("  ({})", py::str_of(v)),
                        other => {
                            return Err(Raise::at(
                                l,
                                format!(
                                    "TypeError: can only concatenate str (not \"{}\") to str",
                                    py::type_name(other)
                                ),
                            ))
                        }
                    },
                    _ => String::new(),
                };
                out.push_str(&format!(
                    "  {} {} {}{reason}\n",
                    ljust(&sv(super::at(&r, "status")), 9),
                    ljust(&sv(super::at(&r, "step")), 10),
                    sv(super::at(&r, "target"))
                ));
                if let Some(p) = super::at(&r, "proposed").filter(|v| py::truthy(v)) {
                    out.push_str(&format!("            new text: {}\n", py::str_of(p)));
                }
                if let Some(diff) = super::at(&r, "diff").filter(|v| py::truthy(v)) {
                    let Value::Str(_) = diff else {
                        return Err(Raise::no_attribute(l, diff, "splitlines"));
                    };
                    for line in ops::splitlines(&py::str_of(diff)) {
                        out.push_str(&format!("            {line}\n"));
                    }
                }
            }
        }
        if truthy(g("dryRun")?) {
            let up = if same {
                String::new()
            } else {
                format!(" Upgrade: {} agent template upgrade <ws>", cli())
            };
            out.push_str(&format!("Nothing written.{up}\n"));
        } else if truthy(g("changed")?) {
            out.push_str("Done. The agent has an inbox note saying what changed.\n");
        } else {
            out.push_str("Nothing to change.\n");
        }
        return Ok(());
    }
    if has("files") && mode == "diff" {
        let g = |k: &str| py_get(d, k, l);
        out.push_str(&format!(
            "{}: v{} -> v{}\n",
            sv(g("id")?),
            sv(g("from")?),
            sv(g("to")?)
        ));
        if let Some(fs) = g("files")?.filter(|v| py::truthy(v)) {
            for f in py_iter(fs, l)? {
                out.push_str(&format!(
                    "  {} {}\n",
                    ljust(&sv(py_get(&f, "status", l)?), 8),
                    sv(py_get(&f, "path", l)?)
                ));
                if let Some(diff) = py_get(&f, "diff", l)?.filter(|v| py::truthy(v)) {
                    let Value::Str(_) = diff else {
                        return Err(Raise::no_attribute(l, diff, "splitlines"));
                    };
                    for line in ops::splitlines(&py::str_of(diff)) {
                        out.push_str(&format!("           {line}\n"));
                    }
                }
            }
        }
        return Ok(());
    }
    out.push_str(&indent2(d));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::out::json::parse as jparse;

    #[test]
    fn the_words() {
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&["help"]), None);
        assert!(matches!(parse(&["show"]), Some(Err(_))));
        assert!(matches!(parse(&["diff", "ws", "--dry-run"]), Some(Err(_))));
        assert!(matches!(
            parse(&["remove", "x", "--version", "v2"]),
            Some(Ok((Verb::Remove { .. }, false)))
        ));
        assert!(matches!(parse(&["remove", "x", "--version", "2a"]), Some(Err(_))));
        assert!(matches!(
            parse(&["diff", "id", "v1", "2", "--json"]),
            Some(Ok((Verb::Versions { .. }, true)))
        ));
    }

    #[test]
    fn python_ints() {
        assert_eq!(py_int(" 007 ").as_deref(), Some("7"));
        assert_eq!(py_int("1_0").as_deref(), Some("10"));
        assert_eq!(py_int("-0").as_deref(), Some("0"));
        assert_eq!(py_int("x"), None);
        assert_eq!(py_int(""), None);
    }

    #[test]
    fn the_list() {
        let raw = r#"{"templates":[{"id":"chief-of-staff","name":"Chief","summary":"Runs things","kind":"system","aliases":["cos"]},{"id":"qa-bot","name":"QA","origin":"library","author":"ana","version":2}],"problems":[{"dir":"/x","error":"bad toml"}]}"#;
        let shown = read_print(raw, Mode::List, false, false);
        assert_eq!(
            shown.stdout,
            format!("Agent types (hire one: {} agent hire --template <id> --name <name>):\n  chief-of-staff Chief        Runs things  (old id: cos; system)\n\nYours (saved on this server):\n  qa-bot         QA           (by ana, v2)\n", cli())
        );
        assert_eq!(shown.stderr, "warning: /x: bad toml\n");
    }

    #[test]
    fn the_errors() {
        let r = read_print(r#"{"error":{"code":"unknown_template","hint":"no such"}}"#, Mode::Show, false, false);
        assert_eq!((r.stderr.as_str(), r.exit), ("Error: unknown_template: no such\n", Exit::NotFound));
        let r = write_print(r#"{"error":"bad token here"}"#, "save", false);
        assert_eq!((r.stderr.as_str(), r.exit), ("Error: bad token here\n", Exit::Refused));
        let r = write_print("", "save", false);
        assert_eq!(r.stderr, "Error: daemon answered: \n");
    }

    #[test]
    fn an_upgrade_plan() {
        let d = jparse(r#"{"name":"QA","id":"qa","from":1,"to":2,"dryRun":true,"rows":[{"status":"unchanged"},{"status":"update","step":"role","target":"ROLE.md","reason":"edited","diff":"-a\n+b"}]}"#).unwrap();
        let mut out = String::new();
        write_lines(&d, "upgrade", &mut out).unwrap();
        assert_eq!(
            out,
            format!("QA template: v1 -> v2 (qa)  [dry run]\n  update    role       ROLE.md  (edited)\n            -a\n            +b\nNothing written. Upgrade: {} agent template upgrade <ws>\n", cli())
        );
    }
}
