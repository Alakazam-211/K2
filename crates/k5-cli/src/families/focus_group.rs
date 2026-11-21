//! `k5 focus-group list|create|rename|add|remove` (0.46.17): the
//! sidebar's focus groups, as the shell CLI's `cmd_focus_group` answers
//! them (`cli/src/focus-group.sh`), on the routes Settings → Workspaces
//! uses:
//!
//! - `GET /cli/focus-groups/list`, over TCP (`_K2_FORCE_TCP=1`: the cell
//!   socket doesn't serve it, and the daemon admits a passport there);
//! - `POST /cli/focus-groups/create {name}`, `update {id, name}` and
//!   `assign {projectId, focusGroupId}` (null: no group), each one
//!   `cli_send_json` ([`Client::send_json`]): the cell socket first in a
//!   session, then TCP after "not served" or a failed connect.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the two gates ([`crate::prelude::gate`]).
//! 2. `focus-groups` is an `id` tool, locked: in a session (`HOOK_SOCK`
//!    set) a passport the daemon no longer knows with its socket gone, and
//!    no passport at all, are refused with exit 3
//!    ([`crate::prelude::id_tool`]). The shared Codex server check that
//!    comes first is the shell's (a hand-back).
//! 3. The words ([`parse`]): the verb (the bare noun, or a flag first, is
//!    `list`), `--json`, `--` (every word after it is a word), any other
//!    `-…` is `unknown flag`. A stop is `_fg_usage`'s line, plain or JSON
//!    as `--json` stood when it was read, exit 2.
//! 4. The verb. A group is named by its id, its name in any case, or the
//!    start of exactly one name ([`group`], from the list); a workspace
//!    by handle, name or path (`_agent_conf_fetch`,
//!    [`crate::families::agent::conf_fetch`]). `add` and `remove` with
//!    nothing to change send nothing.
//! 5. `_fg_check_answer` on each answer ([`check`]): no answer is
//!    `daemon_unreachable` (exit 1); an answer that is no JSON is
//!    `daemon_error`; a refusal or an error is the daemon's code and hint
//!    (3 for 401 and 403, 4 for 404, else 1). An agent's change is refused
//!    `focus_group_needs_person` (exit 3).
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]): the refusal's
//!    closing line (`Ask with: …`), once.
//!
//! Answers are read from the response body, never handed on in an
//! environment variable.
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`), so
//! there is one copy of each: any form with `-h` or `--help` (the help,
//! which the front sends to a page or the shell, [`crate::front`]: help
//! never acts); a word that is not UTF-8; a `HOME` that is not set or
//! not UTF-8; a port that is set but is no port curl takes; a `python3`
//! the binary can't tell; and the shared Codex server check.
//!
//! Where this differs from the shell: an answer that is not UTF-8 is read
//! with U+FFFD for each bad byte (the daemon sends none). Where the
//! shell's Python raises, the traceback is Python 3.12's, without source
//! lines. The command's name in the lines is the brand's
//! ([`k2_client::brand::CLI_NAME`]); the shell types `k2` there.

use super::{curl_port, hook_sock, py_get, py_space, raised, s, without_nuls, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::families::agent;
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::{json as pyjson, ops};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::{Client, Error};
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["focus-group", "focus-groups"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's tool policy.
pub const TOOL: &str = "focus-groups";

/// `_fg_list_raw`'s route.
pub const LIST: &str = "/cli/focus-groups/list";
/// `create`'s route.
pub const CREATE: &str = "/cli/focus-groups/create";
/// `rename`'s route.
pub const UPDATE: &str = "/cli/focus-groups/update";
/// `add` and `remove`'s route.
pub const ASSIGN: &str = "/cli/focus-groups/assign";

/// The Python lines a traceback names.
mod line {
    /// The list's printer: `g.get(...)` on a row.
    pub const LIST_ROW: u32 = 13;
    /// `_fg_group`: the first match (id or whole name).
    pub const GROUP_MATCH: u32 = 9;
    /// `_fg_group`: the start of a name.
    pub const GROUP_PREFIX: u32 = 11;
}

/// A verb and its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    List,
    Create(String),
    Rename(String, String),
    Add(String, String),
    Remove(String, String),
}

/// What the words ask for: the verb, and `--json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub verb: Verb,
    pub json: bool,
}

/// `_fg_usage <why>`: its hint and whether `--json` stood when it was
/// read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    pub why: String,
    pub json: bool,
}

impl Usage {
    /// The line on stderr: exit 2.
    pub fn print(&self) -> Exit {
        let hint = format!(
            "{} (usage: {} focus-group list | create <name> | rename <group> <new-name> | add <group> <workspace> | remove <group> <workspace>)",
            self.why,
            cli()
        );
        out::stderr_line(&err_line(self.json, &s("usage"), &s(&hint)));
        Exit::Usage
    }
}

/// The command's name, as every hint prints it.
fn cli() -> &'static str {
    k2_client::brand::CLI_NAME
}

/// `json.dumps(v)`: Python's defaults.
fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default())
        .expect("json.dumps with ensure_ascii writes any value json.loads gave")
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `_fg_err`'s line (and the blocks' own `fail`): `{"error": {"code",
/// "hint"}}` with `--json`, else `Error (<code>): <hint>`.
fn err_line(json: bool, code: &Value, hint: &Value) -> String {
    if json {
        dumps(&obj(vec![(
            "error",
            obj(vec![("code", code.clone()), ("hint", hint.clone())]),
        )]))
    } else {
        format!("Error ({}): {}", py::str_of(code), py::str_of(hint))
    }
}

/// `${word// /}` is not empty: the word holds something but spaces.
fn not_blank(word: &str) -> bool {
    word.chars().any(|c| c != ' ')
}

/// `cmd_focus_group`'s words (after the noun; `-h` and `--help` never
/// reach here).
pub fn parse(words: &[&str]) -> Result<Form, Usage> {
    let (verb, rest) = match words.split_first() {
        Some((first, rest)) if !first.is_empty() && !first.starts_with('-') => (*first, rest),
        _ => ("list", words),
    };
    let mut json = false;
    let mut args: Vec<String> = Vec::new();
    let mut ended = false;
    for w in rest {
        if ended {
            args.push((*w).to_string());
            continue;
        }
        match *w {
            "--" => ended = true,
            "--json" => json = true,
            _ if w.starts_with('-') => {
                return Err(Usage {
                    why: format!("unknown flag '{w}'"),
                    json,
                })
            }
            _ => args.push((*w).to_string()),
        }
    }
    let usage = |why: &str| Usage {
        why: why.to_string(),
        json,
    };
    let n = args.len();
    let arg = |i: usize| args[i].clone();
    let verb = match verb {
        "list" | "ls" => {
            if n != 0 {
                return Err(usage("list takes no arguments"));
            }
            Verb::List
        }
        "create" => {
            if n != 1 {
                return Err(usage("create takes one name"));
            }
            if !not_blank(&args[0]) {
                return Err(usage("the name is empty"));
            }
            Verb::Create(arg(0))
        }
        "rename" => {
            if n != 2 {
                return Err(usage("rename takes a group and its new name"));
            }
            if !not_blank(&args[1]) {
                return Err(usage("the new name is empty"));
            }
            Verb::Rename(arg(0), arg(1))
        }
        "add" => {
            if n != 2 {
                return Err(usage("add takes a group and a workspace"));
            }
            Verb::Add(arg(0), arg(1))
        }
        "remove" | "rm" => {
            if n != 2 {
                return Err(usage("remove takes a group and a workspace"));
            }
            Verb::Remove(arg(0), arg(1))
        }
        other => return Err(usage(&format!("unknown verb '{other}'"))),
    };
    Ok(Form { verb, json })
}

/// `k5 focus-group ...`: `args` is the whole command line after
/// `argv[0]`. `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    // The help, anywhere: the front's page or the shell's.
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    if !python::family_native(python::here()) {
        return None;
    }
    // `_agent_conf_fetch` reads a path from `HOME`.
    std::env::var_os("HOME")?.to_str()?;
    for name in k2_client::env_names::PROJECT_PATH {
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
    if prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    Some(run_with(args, &words, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    words: &[&str],
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let form = match parse(words) {
        Ok(f) => f,
        Err(u) => return u.print(),
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
    let mut closing = Closing::default();
    let mut cx = Cx {
        client: &mut client,
        closing: &mut closing,
        json: form.json,
    };
    let exit = match cx.verb(&form.verb) {
        Ok(shown) => shown.print(),
        Err(exit) => exit,
    };
    closing.flush(&mut client, exit);
    exit
}

/// An answer as the shell held it in `raw="$(cli_send_json …)"`: the
/// body (NULs dropped) and the status (0: no answer, curl's `000`).
pub struct Raw {
    pub body: String,
    pub status: u16,
}

/// The command under way.
struct Cx<'a> {
    client: &'a mut Client,
    closing: &'a mut Closing,
    json: bool,
}

/// What `print(x)` held in `$(…)` shows: `str(x)` without its trailing
/// newlines.
fn held(v: &Value) -> String {
    py::str_of(v).trim_end_matches('\n').to_string()
}

/// `x or default`.
fn or(x: Option<&Value>, default: Value) -> Value {
    match x {
        Some(v) if py::truthy(v) => v.clone(),
        _ => default,
    }
}

/// `(x or "").lower()` of a row's name: the `AttributeError` at `line`
/// for a name that is no string.
fn name_lower(g: &Value, line: u32) -> Result<String, Raise> {
    match or(py_get(g, "name", line)?, s("")) {
        Value::Str(t) => Ok(py::str_of(&Value::Str(t)).to_lowercase()),
        other => Err(Raise::no_attribute(line, &other, "lower")),
    }
}

/// `", ".join(sorted(names))`.
fn sorted_join(mut v: Vec<Value>) -> String {
    v.sort_by(|a, b| match (a, b) {
        (Value::Str(x), Value::Str(y)) => x.cmp(y),
        _ => py::str_of(a).cmp(&py::str_of(b)),
    });
    v.iter().map(py::str_of).collect::<Vec<_>>().join(", ")
}

/// The one group a selector names: its id and its name, as `_fg_group`
/// prints them (`{"id": … or "", "name": … or sel}`).
pub struct Group {
    pub id: Value,
    pub name: Value,
}

/// `_fg_group`'s Python on the list's body: the group `sel` names, or
/// what it printed and its exit (4 for none or more than one).
pub fn pick(body: &str, sel: &str, json: bool) -> Result<Group, Shown> {
    let groups = match pyjson::loads(body) {
        Ok(Value::Array(items)) => items,
        _ => Vec::new(),
    };
    let sel = sel.trim_matches(py_space);
    let low = sel.to_lowercase();
    let traceback = |r: Raise| raised(String::new(), String::new(), &r);
    let mut hits = Vec::new();
    for g in &groups {
        let by_id = py_get(g, "id", line::GROUP_MATCH)
            .map_err(traceback)?
            .is_some_and(|v| ops::py_eq(v, &s(sel)));
        if by_id || name_lower(g, line::GROUP_MATCH).map_err(traceback)? == low {
            hits.push(g.clone());
        }
    }
    if hits.is_empty() {
        for g in &groups {
            if name_lower(g, line::GROUP_PREFIX)
                .map_err(traceback)?
                .starts_with(&low)
            {
                hits.push(g.clone());
            }
        }
    }
    let fail = |code: &str, hint: String| {
        Shown::err(err_line(json, &s(code), &s(&hint)) + "\n", Exit::NotFound)
    };
    let repr = py::repr(&s(sel));
    if hits.is_empty() {
        return Err(fail(
            "not_found",
            format!(
                "no focus group matches {repr} (see: {} focus-group list)",
                cli()
            ),
        ));
    }
    if hits.len() > 1 {
        let names = hits
            .iter()
            .map(|g| {
                let name = super::get(object(g), "name").filter(|v| py::truthy(v));
                or(name.or(super::get(object(g), "id")), s("?"))
            })
            .collect();
        return Err(fail(
            "ambiguous",
            format!(
                "{repr} matches more than one focus group: {} (pass the id)",
                sorted_join(names)
            ),
        ));
    }
    let g = object(&hits[0]);
    Ok(Group {
        id: or(super::get(g, "id"), s("")),
        name: or(super::get(g, "name"), s(sel)),
    })
}

/// A row the match took: a dict for sure (`g.get` ran on it).
fn object(g: &Value) -> &[(Text, Value)] {
    match g {
        Value::Object(pairs) => pairs,
        _ => &[],
    }
}

/// `_fg_check_answer <what>` on `raw`: the body when the answer is a good
/// one, else its line on stderr and its exit.
pub fn check(raw: &Raw, what: &str, json: bool) -> Result<(), Shown> {
    let fail = |code: &str, hint: String| {
        Shown::err(err_line(json, &s(code), &s(&hint)) + "\n", Exit::Error)
    };
    if raw.status == 0 {
        return Err(fail(
            "daemon_unreachable",
            format!(
                "no answer from the {} daemon ({what})",
                k2_client::brand::PRODUCT_NAME
            ),
        ));
    }
    let Ok(d) = pyjson::loads(&raw.body) else {
        let head: String = raw.body.chars().take(300).collect();
        return Err(fail(
            "daemon_error",
            format!("unparseable answer (HTTP {}): {head}", raw.status),
        ));
    };
    let err = match &d {
        Value::Object(pairs) => super::get(pairs, "error").cloned(),
        _ => None,
    };
    let err_true = err.as_ref().is_some_and(py::truthy);
    if raw.status >= 300 || err_true {
        let (code, hint) = match &err {
            Some(Value::Object(pairs)) => {
                let code = or(super::get(pairs, "code"), s("error"));
                let hint = match super::get(pairs, "hint").filter(|v| py::truthy(v)) {
                    Some(h) => h.clone(),
                    None => or(super::get(pairs, "message"), s("")),
                };
                (code, hint)
            }
            other => {
                let hint = match other {
                    Some(e) if py::truthy(e) => py::str_of(e),
                    _ => format!("HTTP {}", raw.status),
                };
                (s("error"), s(&hint))
            }
        };
        let line = if json {
            dumps(&d)
        } else {
            format!("Error ({}): {}", py::str_of(&code), py::str_of(&hint))
        };
        let exit = match raw.status {
            401 | 403 => Exit::Refused,
            404 => Exit::NotFound,
            _ => Exit::Error,
        };
        return Err(Shown::err(line + "\n", exit));
    }
    Ok(())
}

/// The list's printer (`cmd_focus_group list`'s Python) on its body.
pub fn list_text(body: &str, json: bool) -> Shown {
    let groups = match pyjson::loads(body) {
        Ok(Value::Array(items)) => items,
        _ => Vec::new(),
    };
    if json {
        return Shown::out(
            dumps(&obj(vec![
                ("ok", Value::Bool(true)),
                ("focusGroups", Value::Array(groups)),
            ])) + "\n",
        );
    }
    if groups.is_empty() {
        return Shown::out(format!(
            "No focus groups. Create one: {} focus-group create <name>\n",
            cli()
        ));
    }
    let mut stdout = String::new();
    for g in &groups {
        let row = py_get(g, "name", line::LIST_ROW).and_then(|name| {
            let name = or(name, s("?"));
            let id = or(py_get(g, "id", line::LIST_ROW)?, s(""));
            Ok((name, id))
        });
        match row {
            Ok((name, id)) => stdout.push_str(&format!(
                "  {} {}\n",
                super::py_left(&name, 24),
                py::str_of(&id)
            )),
            Err(r) => return raised(stdout, String::new(), &r),
        }
    }
    Shown::out(stdout)
}

/// `_fg_assign`'s plan: the request's body, its line, and whether the
/// workspace is there already (nothing to send).
pub struct Plan {
    pub skip: bool,
    pub body: String,
    pub line: String,
}

/// The workspace as `_fg_workspace` reads its conf.
pub struct Workspace {
    pub id: Value,
    pub name: Value,
    pub group: Value,
}

impl Workspace {
    /// `_fg_workspace`'s Python on the conf.
    pub fn from_conf(d: &Value) -> Workspace {
        let pairs = object(d);
        let name = super::get(pairs, "name").filter(|v| py::truthy(v));
        Workspace {
            id: or(super::get(pairs, "id"), s("")),
            name: or(name.or(super::get(pairs, "path")), s("")),
            group: super::get(pairs, "focusGroupId")
                .cloned()
                .unwrap_or(Value::Null),
        }
    }

    /// `_fg_assign <ws> <gid> <label>`'s plan. The line is what `sed -n
    /// 3p` takes of the plan: up to the first newline.
    pub fn plan(&self, gid: &str, label: &str) -> Plan {
        let gid = if gid.is_empty() { Value::Null } else { s(gid) };
        let now = or(Some(&self.group), Value::Null);
        let skip = ops::py_eq(&now, &gid);
        let body = dumps(&obj(vec![
            ("projectId", self.id.clone()),
            ("focusGroupId", gid.clone()),
        ]));
        let when = if skip { " already" } else { " now" };
        let line = if matches!(gid, Value::Null) {
            format!("{} is in no focus group{when}.", py::str_of(&self.name))
        } else {
            format!(
                "{} is in focus group {label}{when}.",
                py::str_of(&self.name)
            )
        };
        let line = line.split('\n').next().unwrap_or("").to_string();
        Plan { skip, body, line }
    }
}

/// `printf '{"ok":true,"changed":false}\n'`.
const UNCHANGED: &str = "{\"ok\":true,\"changed\":false}\n";

impl Cx<'_> {
    /// `raw="$(cli_send_json …)"`; for the list, `_K2_FORCE_TCP=1`. A
    /// line that ended the command (a POST the cell socket may have run,
    /// or a session that would send the owner token) is printed here, and
    /// its exit is the error.
    fn send(&mut self, post: bool, path: &str, payload: &str, tcp: bool) -> Result<Raw, Exit> {
        let passport = self.client.passport().is_some();
        let before = self.client.force_tcp;
        self.client.force_tcp = before || tcp;
        let r = self.client.send_json(post, path, payload);
        self.client.force_tcp = before;
        match r {
            Ok(r) => {
                self.closing.answer(&r, passport);
                Ok(Raw {
                    body: String::from_utf8_lossy(&without_nuls(&r.body)).into_owned(),
                    status: r.status,
                })
            }
            Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
                out::stderr_line(&e.to_string());
                Err(Exit::Error)
            }
            Err(_) => Ok(Raw {
                body: String::new(),
                status: 0,
            }),
        }
    }

    /// The groups' list, checked: its body.
    fn list_body(&mut self) -> Result<String, Exit> {
        let raw = self.send(false, LIST, "", true)?;
        check(&raw, LIST, self.json).map_err(|shown| shown.print())?;
        Ok(raw.body)
    }

    /// `_fg_group <sel>`.
    fn group(&mut self, sel: &str) -> Result<Group, Exit> {
        let body = self.list_body()?;
        pick(&body, sel, self.json).map_err(|shown| shown.print())
    }

    /// `_fg_workspace <workspace>`.
    fn workspace(&mut self, token: &str) -> Result<Workspace, Exit> {
        let d = agent::conf_fetch(self.client, self.closing, token)?;
        Ok(Workspace::from_conf(&d))
    }

    /// `_fg_send <route> <body> <line>`.
    fn post(&mut self, route: &str, body: &str, line: &str) -> Result<Shown, Exit> {
        let raw = self.send(true, route, body, false)?;
        check(&raw, route, self.json).map_err(|shown| shown.print())?;
        Ok(Shown::out(if self.json {
            format!("{}\n", raw.body)
        } else {
            format!("{line}\n")
        }))
    }

    /// `_fg_assign <ws> <gid> <label>`.
    fn assign(&mut self, ws: &Workspace, gid: &str, label: &str) -> Result<Shown, Exit> {
        let plan = ws.plan(gid, label);
        if plan.skip {
            return Ok(Shown::out(if self.json {
                UNCHANGED.to_string()
            } else {
                format!("{}\n", plan.line)
            }));
        }
        self.post(ASSIGN, &plan.body, &plan.line)
    }

    fn verb(&mut self, verb: &Verb) -> Result<Shown, Exit> {
        match verb {
            Verb::List => {
                let body = self.list_body()?;
                Ok(list_text(&body, self.json))
            }
            Verb::Create(name) => {
                let body = dumps(&obj(vec![("name", s(name))]));
                self.post(CREATE, &body, &format!("Created focus group {name}."))
            }
            Verb::Rename(sel, to) => {
                let g = self.group(sel)?;
                let body = dumps(&obj(vec![("id", g.id), ("name", s(to))]));
                let line = format!("Renamed focus group {} to {to}.", held(&g.name));
                self.post(UPDATE, &body, &line)
            }
            Verb::Add(sel, token) => {
                let g = self.group(sel)?;
                let ws = self.workspace(token)?;
                self.assign(&ws, &held(&g.id), &held(&g.name))
            }
            Verb::Remove(sel, token) => {
                let g = self.group(sel)?;
                let ws = self.workspace(token)?;
                let gid = held(&g.id);
                let now = held(&or(Some(&ws.group), s("")));
                if now != gid {
                    return Ok(Shown::out(if self.json {
                        UNCHANGED.to_string()
                    } else {
                        format!(
                            "{} is not in focus group {}; nothing changed.\n",
                            held(&ws.name),
                            held(&g.name)
                        )
                    }));
                }
                self.assign(&ws, "", "")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(words: &[&str]) -> Usage {
        parse(words).expect_err("a usage line")
    }

    #[test]
    fn the_words() {
        assert_eq!(
            parse(&[]).unwrap(),
            Form {
                verb: Verb::List,
                json: false
            }
        );
        assert_eq!(
            parse(&["--json"]).unwrap(),
            Form {
                verb: Verb::List,
                json: true
            }
        );
        assert_eq!(parse(&["ls"]).unwrap().verb, Verb::List);
        assert_eq!(
            parse(&["create", "--", "--json"]).unwrap(),
            Form {
                verb: Verb::Create("--json".into()),
                json: false
            }
        );
        assert_eq!(
            parse(&["rm", "a", "b", "--json"]).unwrap(),
            Form {
                verb: Verb::Remove("a".into(), "b".into()),
                json: true
            }
        );
        // An empty first word is no verb: the list, with a word.
        assert_eq!(usage(&[""]).why, "list takes no arguments");
        assert_eq!(
            usage(&["create", "x", "--bogus", "--json"]),
            Usage {
                why: "unknown flag '--bogus'".into(),
                json: false
            }
        );
        assert!(usage(&["--json", "-"]).json);
        assert_eq!(usage(&["create", "  "]).why, "the name is empty");
        assert!(parse(&["create", "\t"]).is_ok());
        assert_eq!(usage(&["bogus", "--json"]).why, "unknown verb 'bogus'");
        assert!(usage(&["bogus", "--json"]).json);
    }

    #[test]
    fn the_check() {
        let raw = |body: &str, status| Raw {
            body: body.into(),
            status,
        };
        assert!(check(&raw("[]", 200), LIST, false).is_ok());
        let e = check(&raw("", 0), LIST, false).unwrap_err();
        assert_eq!(e.exit, Exit::Error);
        assert!(e.stderr.contains("daemon_unreachable"));
        let e = check(
            &raw(r#"{"error":{"code":"x","message":"m"}}"#, 404),
            LIST,
            false,
        )
        .unwrap_err();
        assert_eq!(
            (e.stderr.as_str(), e.exit),
            ("Error (x): m\n", Exit::NotFound)
        );
        let e = check(&raw(r#"{"oops":1}"#, 500), LIST, false).unwrap_err();
        assert_eq!(e.stderr, "Error (error): HTTP 500\n");
        let e = check(&raw(r#"{"error":"no"}"#, 403), LIST, true).unwrap_err();
        assert_eq!(
            (e.stderr.as_str(), e.exit),
            ("{\"error\": \"no\"}\n", Exit::Refused)
        );
    }

    #[test]
    fn the_pick() {
        let body = r#"[{"id":"g1","name":"Infra"},{"id":"g2","name":"Client A"},{"id":"g3","name":"Client B"}]"#;
        let g = pick(body, " infra ", false).ok().unwrap();
        assert_eq!(py::str_of(&g.id), "g1");
        assert_eq!(
            py::str_of(&pick(body, "g3", false).ok().unwrap().name),
            "Client B"
        );
        let e = pick(body, "Client", false).err().unwrap();
        assert!(e.stderr.contains("Client A, Client B"), "{}", e.stderr);
        assert_eq!(e.exit, Exit::NotFound);
        let e = pick(body, "zz", true).err().unwrap();
        assert!(e.stderr.starts_with("{\"error\": {\"code\": \"not_found\""));
        let e = pick(r#"[1]"#, "x", false).err().unwrap();
        assert!(e
            .stderr
            .ends_with("AttributeError: 'int' object has no attribute 'get'\n"));
    }

    #[test]
    fn the_plan() {
        let ws = Workspace::from_conf(
            &pyjson::loads(r#"{"ok":true,"id":"w1","name":"a\nb","focusGroupId":"g1"}"#).unwrap(),
        );
        let p = ws.plan("g1", "Infra");
        assert!(p.skip);
        assert_eq!(p.line, "a");
        let p = ws.plan("", "");
        assert!(!p.skip);
        assert_eq!(p.body, r#"{"projectId": "w1", "focusGroupId": null}"#);
    }
}
