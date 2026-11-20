//! `k5 thread export <ws> [--since-days N]`, `import <file> --to <ws>` and
//! `imports <ws>` (`--json` on each): a person moves a workspace's Threads
//! outside a clone, as the shell's `cmd_thread_transfer`
//! (`cli/src/browser.sh`) does it, through `_compute_call` over TCP
//! (compute's [`Cp::call`]).
//!
//! 1. The words ([`parse`]): the first of `export`, `import` and
//!    `imports` is the verb, a later one the workspace; a flag it doesn't
//!    know is the usage line, exit 2; `--to` or `--since-days` as the last
//!    word stops the shell's `shift 2` under `set -e`: exit 1, nothing
//!    printed.
//! 2. `export` and `import` are people's: in a session, or with a
//!    `*_HOOK_TOKEN` set, the clone refusal's JSON line, exit 3
//!    (`_clone_person_guard`).
//! 3. `export`: POST `/cli/thread/export` `{"ws", "sinceDays"}`;
//!    `import`: POST `/cli/thread/import` `{"path", "to"}`, the file's
//!    folder as `cd` and `pwd` give it; `imports`: GET
//!    `/cli/thread/imports?ws=`. Any status but 200 is `_compute_fail`'s
//!    line and exit.
//! 4. `--json`: the body; else its Python's line (or table).
//!
//! The shell answers itself: a `--since-days` that is not plain digits
//! (its Python's `int()` takes more, or raises).

use crate::exit::Exit;
use crate::families::compute::writes::{dumps, load, object, print_body, str_value, Cp};
use crate::families::{py_get, py_iter, py_left, py_len, raised, Raise, Shown};
use crate::out::json::Value;
use crate::out::py;
use crate::prelude::Closing;
use crate::transport::encode::quote;
use crate::transport::{Auth, Client};
use k2_client::{cli_name, env_names};
use std::path::Path;

/// The routes.
pub const EXPORT: &str = "/cli/thread/export";
pub const IMPORT: &str = "/cli/thread/import";
pub const IMPORTS: &str = "/cli/thread/imports";

pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " thread export <ws> [--since-days N] | import <file> --to <ws> | imports <ws>"
);

/// What the words ask for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ask {
    pub sub: String,
    pub ws: String,
    pub path: String,
    pub to: String,
    pub since: String,
    pub json: bool,
}

/// What the words come to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Ask(Ask),
    /// Stderr (may be empty) and the exit, nothing sent.
    Stop(String, Exit),
}

/// `cmd_thread_transfer`'s loop. `None`: the shell's.
pub fn parse(words: &[&str]) -> Option<Parsed> {
    let mut a = Ask::default();
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "export" | "import" | "imports" => {
                if a.sub.is_empty() {
                    a.sub = (*w).to_string();
                } else {
                    a.ws = (*w).to_string();
                }
                i += 1;
            }
            "--to" | "--since-days" | "--threads-since" => {
                let Some(v) = words.get(i + 1) else {
                    return Some(Parsed::Stop(String::new(), Exit::Error));
                };
                if *w == "--to" {
                    a.to = (*v).to_string();
                } else {
                    a.since = (*v).to_string();
                }
                i += 2;
            }
            "--json" => {
                a.json = true;
                i += 1;
            }
            w if w.starts_with('-') => return Some(Parsed::Stop(USAGE.to_string(), Exit::Usage)),
            w => {
                if a.sub == "import" {
                    a.path = w.to_string();
                } else {
                    a.ws = w.to_string();
                }
                i += 1;
            }
        }
    }
    if !a.since.is_empty() && !a.since.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(Parsed::Ask(a))
}

/// `_clone_die clone_agents_propose …`'s line.
pub fn person_refusal() -> String {
    let hint = format!(
        "agents can't clone; propose it and a person approves: {} clone propose --to <host|here> --why \"…\"",
        k2_client::brand::CLI_NAME
    );
    dumps(&object(vec![
        ("error", str_value("clone_agents_propose")),
        ("hint", str_value(&hint)),
    ]))
}

/// `$(cd "$(dirname "$path")" 2>/dev/null && pwd)/$(basename "$path")`.
pub fn abs_of(path: &str, pwd: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    let (dir, base) = if trimmed.is_empty() {
        (if path.is_empty() { "." } else { "/" }, if path.is_empty() { "" } else { "/" })
    } else {
        match trimmed.rsplit_once('/') {
            Some((d, b)) => {
                let d = d.trim_end_matches('/');
                (if d.is_empty() { "/" } else { d }, b)
            }
            None => (".", trimmed),
        }
    };
    let joined = if dir.starts_with('/') {
        dir.to_string()
    } else {
        format!("{pwd}/{dir}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for c in joined.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let logical = format!("/{}", parts.join("/"));
    let folder = if Path::new(&logical).is_dir() {
        logical
    } else {
        String::new()
    };
    format!("{folder}/{base}")
}

/// `$PWD` as bash holds it: the variable when it names this folder, else
/// the folder.
fn pwd() -> String {
    let here = std::env::current_dir().unwrap_or_default();
    match std::env::var("PWD") {
        Ok(p) if p.starts_with('/') && Path::new(&p).canonicalize().ok() == here.canonicalize().ok() => p,
        _ => here.to_string_lossy().into_owned(),
    }
}

/// `_compute_in_session || _envn_any HOOK_TOKEN`.
fn agent(client: &Client) -> bool {
    matches!(client.auth, Auth::Passport { .. })
        || env_names::first_nonempty_os_in(env_names::HOOK_TOKEN, |n| std::env::var_os(n)).is_some()
}

/// The verb's run, on the client the gates gave.
pub fn run(client: &mut Client, a: Ask) -> Exit {
    let mut cp = Cp {
        client,
        closing: Closing::default(),
        project: String::new(),
        as_json: a.json,
    };
    let exit = verb(&mut cp, &a);
    let Cp { client, closing, .. } = cp;
    closing.flush(client, exit);
    exit
}

fn usage(line: &str) -> Exit {
    crate::out::stderr_line(line);
    Exit::Usage
}

fn verb(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    let guard = |cp: &Cp<'_>| {
        agent(&*cp.client).then(|| {
            crate::out::stderr_line(&person_refusal());
            Exit::Refused
        })
    };
    match a.sub.as_str() {
        "export" => {
            if a.ws.is_empty() {
                return usage(concat!("usage: ", cli_name!(), " thread export <ws> [--since-days N]"));
            }
            if let Some(e) = guard(cp) {
                return e;
            }
            let mut pairs = vec![("ws", str_value(&a.ws))];
            if !a.since.is_empty() {
                let n = a.since.trim_start_matches('0');
                pairs.push(("sinceDays", Value::Int(if n.is_empty() { "0" } else { n }.into())));
            }
            match cp.call(true, EXPORT, &dumps(&object(pairs)), true) {
                Ok(body) if a.json => print_body(&body),
                Ok(body) => exported(&body).print(),
                Err(e) => e,
            }
        }
        "import" => {
            if a.path.is_empty() || a.to.is_empty() {
                return usage(concat!("usage: ", cli_name!(), " thread import <file> --to <ws>"));
            }
            if let Some(e) = guard(cp) {
                return e;
            }
            let abs = abs_of(&a.path, &pwd());
            let body = dumps(&object(vec![("path", str_value(&abs)), ("to", str_value(&a.to))]));
            match cp.call(true, IMPORT, &body, true) {
                Ok(body) if a.json => print_body(&body),
                Ok(body) => imported(&body).print(),
                Err(e) => e,
            }
        }
        "imports" => {
            if a.ws.is_empty() {
                return usage(concat!("usage: ", cli_name!(), " thread imports <ws>"));
            }
            match cp.call(false, IMPORTS, &format!("ws={}", quote(&a.ws)), true) {
                Ok(body) if a.json => print_body(&body),
                Ok(body) => imports(&body),
                Err(e) => e,
            }
        }
        // `cmd_thread` sends only these three here.
        _ => Exit::Ok,
    }
}

fn s_of(v: Option<&Value>) -> String {
    v.map_or_else(|| "None".to_string(), py::str_of)
}

/// `x or {}` / `x or []`: the value when truthy.
fn or_empty(v: Option<&Value>, empty: Value) -> Value {
    match v {
        Some(x) if py::truthy(x) => x.clone(),
        _ => empty,
    }
}

/// The export's Python.
pub fn exported(body: &[u8]) -> Shown {
    let go = || -> Result<String, Raise> {
        let v = load(body, 3)?;
        let s = or_empty(py_get(&v, "summary", 4)?, Value::Object(Vec::new()));
        Ok(format!(
            "{}: {} Thread(s), {} line(s) → {}\n",
            s_of(py_get(&v, "workspace", 5)?),
            s_of(py_get(&s, "conversations", 5)?),
            s_of(py_get(&s, "docs", 5)?),
            s_of(py_get(&v, "path", 5)?)
        ))
    };
    match go() {
        Ok(line) => Shown::out(line),
        Err(r) => raised(String::new(), String::new(), &r),
    }
}

/// The import's Python.
pub fn imported(body: &[u8]) -> Shown {
    let go = || -> Result<String, Raise> {
        let v = load(body, 3)?;
        let r = or_empty(py_get(&v, "report", 4)?, Value::Object(Vec::new()));
        let ws = s_of(py_get(&v, "workspace", 5)?);
        let docs = s_of(py_get(&r, "docsImported", 5)?);
        let convs = py_len(&or_empty(py_get(&r, "conversations", 6)?, Value::Array(Vec::new())), 6)?;
        let skipped = s_of(py_get(&r, "docsSkipped", 6)?);
        let pending = py_get(&r, "pending", 6)?.is_some_and(py::truthy);
        Ok(format!(
            "{ws}: imported {docs} line(s) into {convs} Thread(s) ({skipped} already there){}\n",
            if pending {
                " — main Thread waits for the first chat"
            } else {
                ""
            }
        ))
    };
    match go() {
        Ok(line) => Shown::out(line),
        Err(r) => raised(String::new(), String::new(), &r),
    }
}

/// `r[key]`: the value, or the exception at line 7.
fn index<'a>(r: &'a Value, key: &str) -> Result<&'a Value, Raise> {
    match r {
        Value::Object(pairs) => crate::families::get(pairs, key)
            .ok_or_else(|| Raise::at(7, format!("KeyError: '{key}'"))),
        Value::Str(_) => Err(Raise::at(
            7,
            "TypeError: string indices must be integers, not 'str'",
        )),
        Value::Array(_) => Err(Raise::at(
            7,
            "TypeError: list indices must be integers or slices, not str",
        )),
        other => Err(Raise::at(
            7,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// The imports table's Python: each row printed as it goes, so a row
/// that raises leaves the ones before it on stdout.
pub fn imports(body: &[u8]) -> Exit {
    let mut stdout = String::new();
    let go = |stdout: &mut String| -> Result<(), Raise> {
        let v = load(body, 3)?;
        let rows = or_empty(py_get(&v, "imports", 3)?, Value::Array(Vec::new()));
        if !py::truthy(&rows) {
            stdout.push_str("No imported Threads.\n");
            return Ok(());
        }
        for r in py_iter(&rows, 6)? {
            let id = index(&r, "localConversationId")?;
            let role = index(&r, "role")?;
            let docs = index(&r, "docCount")?;
            let handle = index(&r, "sourceHandle")?;
            let host = index(&r, "sourceHost")?;
            let name = py_get(&r, "name", 8)?.filter(|n| py::truthy(n)).cloned();
            let named = match name {
                None => String::new(),
                Some(Value::Str(t)) => format!(" ({})", py::str_of(&Value::Str(t))),
                Some(other) => {
                    return Err(Raise::at(
                        8,
                        format!(
                            "TypeError: can only concatenate str (not \"{}\") to str",
                            py::type_name(&other)
                        ),
                    ))
                }
            };
            stdout.push_str(&format!(
                "{} {} {:>5} lines  from {} on {}{named}\n",
                py_left(id, 44),
                py_left(role, 17),
                py::str_of(docs),
                py::str_of(handle),
                py::str_of(host)
            ));
        }
        Ok(())
    };
    let shown = match go(&mut stdout) {
        Ok(()) => Shown::out(stdout),
        Err(r) => raised(stdout, String::new(), &r),
    };
    shown.print()
}
