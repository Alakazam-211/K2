//! `cmd_agent_set` (`cli/src/agent.sh`): the conf, then the plan and the
//! urllib calls of the block's own (K2's ruling D4): `/cli/workspace/set`,
//! the display name, the persona, and project memberships.

use super::conf::fetch;
use super::{at, cli, dumps, err_json, obj, s};
use crate::exit::Exit;
use crate::families::{ljust, py_get, py_iter, Raise, Shown};
use crate::out::json::{Text, Value};
use crate::out::pylib::{json as pyjson, ops};
use crate::out::py;
use crate::prelude::Closing;
use crate::transport::urllib::{decode, Method, Opener, Outcome, Request};
use crate::transport::Client;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);
const VALID: &str = "--mode --enabled --name --persona --add-project --remove-project --dry-run --json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetArgs {
    pub name: String,
    pub mode: String,
    pub enabled: String,
    pub new_name: String,
    pub persona: String,
    pub add: Vec<String>,
    pub remove: Vec<String>,
    pub dry: bool,
    pub json: bool,
}

/// `cmd_agent_set`'s words and checks (the `bad_usage` hint on failure).
pub fn parse(rest: &[&str]) -> Result<SetArgs, String> {
    let cli = cli();
    let mut a = SetArgs {
        name: String::new(),
        mode: String::new(),
        enabled: String::new(),
        new_name: String::new(),
        persona: String::new(),
        add: Vec::new(),
        remove: Vec::new(),
        dry: false,
        json: false,
    };
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let value = rest.get(i + 1).map(|v| v.to_string());
        let need = |hint: String| value.clone().ok_or(hint);
        match w {
            "--mode" => a.mode = need("--mode requires a value (off | custom | manager | k2)".into())?,
            "--enabled" => a.enabled = need("--enabled requires a value (true | false)".into())?,
            "--name" => a.new_name = need("--name requires a value".into())?,
            "--persona" => a.persona = need("--persona requires a file path".into())?,
            "--add-project" => a.add.push(need(format!(
                "--add-project requires a project name or id (see: {cli} project list)"
            ))?),
            "--remove-project" => a.remove.push(need(format!(
                "--remove-project requires a project name or id (see: {cli} project list)"
            ))?),
            "--dry-run" => a.dry = true,
            "--json" => a.json = true,
            _ if w.starts_with('-') => {
                return Err(format!("unknown flag '{w}' \u{2014} valid flags: {VALID}"))
            }
            _ if a.name.is_empty() => a.name = w.to_string(),
            _ => {
                return Err(format!(
                    "unexpected extra argument '{w}' \u{2014} usage: {cli} agent set <name> [flags]"
                ))
            }
        }
        if matches!(
            w,
            "--mode" | "--enabled" | "--name" | "--persona" | "--add-project" | "--remove-project"
        ) {
            i += 1;
        }
        i += 1;
    }
    if a.name.is_empty() {
        return Err(format!(
            "missing <name> \u{2014} usage: {cli} agent set <name> [flags] (valid flags: {VALID})"
        ));
    }
    if a.mode.is_empty()
        && a.enabled.is_empty()
        && a.new_name.is_empty()
        && a.persona.is_empty()
        && a.add.is_empty()
        && a.remove.is_empty()
    {
        return Err("nothing to set \u{2014} pass at least one of: --mode --enabled --name --persona --add-project --remove-project".into());
    }
    if !matches!(a.mode.as_str(), "" | "off" | "custom" | "manager" | "k2" | "k2so") {
        return Err(format!(
            "invalid --mode '{}' \u{2014} valid: off, custom, manager, k2",
            a.mode
        ));
    }
    if !matches!(a.enabled.as_str(), "" | "true" | "false") {
        return Err(format!(
            "invalid --enabled '{}' \u{2014} valid: true, false",
            a.enabled
        ));
    }
    if !a.persona.is_empty() && std::fs::File::open(&a.persona).is_err()
        || !a.persona.is_empty() && !std::path::Path::new(&a.persona).is_file()
    {
        return Err(format!(
            "persona file not found or unreadable: {}",
            a.persona
        ));
    }
    Ok(a)
}

/// `cmd_agent_set`.
pub fn set(client: &mut Client, closing: &mut Closing, a: &SetArgs) -> Exit {
    let conf = match fetch(client, closing, &a.name) {
        Ok(d) => d,
        Err(exit) => return exit,
    };
    let opener = Opener::from_client(client);
    let mut run = Run {
        opener: &opener,
        a,
        conf: &conf,
        actions: Vec::new(),
        stdout: String::new(),
        path: Value::Null,
        label: String::new(),
    };
    let shown = match run.block() {
        Ok(()) => Shown::out(std::mem::take(&mut run.stdout)),
        Err(Stop::Fail(line, exit)) => Shown {
            stdout: std::mem::take(&mut run.stdout),
            stderr: line,
            exit,
        },
        Err(Stop::Raise(r)) => Shown {
            stdout: std::mem::take(&mut run.stdout),
            stderr: r.text().replace("File \"<string>\"", "File \"<stdin>\""),
            exit: Exit::Error,
        },
    };
    shown.print()
}

enum Stop {
    /// `fail(code, hint, exit)`: its line (with the newline) and exit.
    Fail(String, Exit),
    Raise(Raise),
}

impl From<Raise> for Stop {
    fn from(r: Raise) -> Stop {
        Stop::Raise(r)
    }
}

fn fail(code: &Value, hint: &str, exit: Exit) -> Stop {
    let line = dumps(&obj(vec![(
        "error",
        obj(vec![("code", code.clone()), ("hint", s(hint))]),
    )]));
    Stop::Fail(line + "\n", exit)
}

/// One action: a dict in insertion order.
type Action = Vec<(String, Value)>;

fn put(a: &mut Action, k: &str, v: Value) {
    match a.iter_mut().find(|(key, _)| key == k) {
        Some((_, old)) => *old = v,
        None => a.push((k.to_string(), v)),
    }
}

fn pop(a: &mut Action, k: &str) {
    a.retain(|(key, _)| key != k);
}

fn aget<'a>(a: &'a Action, k: &str) -> Option<&'a Value> {
    a.iter().find(|(key, _)| key == k).map(|(_, v)| v)
}

fn status_is(a: &Action, st: &str) -> bool {
    aget(a, "status").is_some_and(|v| ops::py_eq(v, &s(st)))
}

fn act(pairs: Vec<(&str, Value)>) -> Action {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn to_value(a: &Action) -> Value {
    Value::Object(a.iter().map(|(k, v)| (Text::from(k.as_str()), v.clone())).collect())
}

struct Run<'a> {
    opener: &'a Opener,
    a: &'a SetArgs,
    conf: &'a Value,
    actions: Vec<Action>,
    stdout: String,
    path: Value,
    label: String,
}

const L: u32 = 120;

impl Run<'_> {
    /// One urllib call: the answer read as JSON, or the block's error dict.
    fn call(&self, req: Request, route: &str) -> Value {
        let req = match self.opener.token() {
            Some(t) => req.header("Authorization", format!("Bearer {t}")),
            None => req,
        };
        match self.opener.open(&req) {
            Outcome::Answer { body: Ok(b), .. } => match pyjson::loads(&decode(&b)) {
                Ok(v) => v,
                Err(f) => obj(vec![("error", s(&format!("{route}: {}", f.message)))]),
            },
            Outcome::Answer { body: Err(r), .. } | Outcome::Raised(r) => {
                obj(vec![("error", s(&format!("{route}: {}", r.text())))])
            }
            Outcome::HttpError { code, body } => {
                match body.ok().and_then(|b| pyjson::loads(&decode(&b)).ok()) {
                    Some(v) => v,
                    None => obj(vec![("error", s(&format!("HTTP {code} on {route}")))]),
                }
            }
        }
    }

    fn http_get(&self, route: &str, params: Vec<(String, String)>) -> Value {
        self.call(Request::new(Method::Get, route, TIMEOUT).query(params), route)
    }

    fn post_query(&self, route: &str, params: Vec<(String, String)>) -> Value {
        let req = Request::new(Method::Post, route, TIMEOUT)
            .query(params)
            .data(Vec::new());
        self.call(req, route)
    }

    fn post_json(&self, route: &str, body: &Value) -> Value {
        let req = Request::new(Method::Post, route, TIMEOUT)
            .data(dumps(body).into_bytes())
            .header("Content-Type", "application/json");
        self.call(req, route)
    }

    fn block(&mut self) -> Result<(), Stop> {
        let conf = self.conf;
        self.path = at(conf, "path")
            .cloned()
            .ok_or_else(|| Raise::at(L, "KeyError: 'path'"))?;
        self.label = match at(conf, "name").filter(|v| py::truthy(v)) {
            Some(n) => py::str_of(n),
            None => match &self.path {
                Value::Str(_) => {
                    let p = py::str_of(&self.path);
                    p.rsplit('/').next().unwrap_or("").to_string()
                }
                other => {
                    return Err(Raise::at(
                        L,
                        format!(
                            "TypeError: expected str, bytes or os.PathLike object, not {}",
                            py::type_name(other)
                        ),
                    )
                    .into())
                }
            },
        };
        let a = self.a;
        let req_mode = if a.mode == "k2so" { "k2" } else { a.mode.as_str() };
        if !a.mode.is_empty() {
            let from = at(conf, "mode").cloned().unwrap_or(Value::Null);
            self.actions.push(if ops::py_eq(&s(req_mode), &from) {
                act(vec![
                    ("step", s("set-mode")),
                    ("mode", s(req_mode)),
                    ("status", s("skipped")),
                    ("reason", s(&format!("already {req_mode}"))),
                ])
            } else {
                act(vec![
                    ("step", s("set-mode")),
                    ("mode", s(req_mode)),
                    ("status", s("pending")),
                    ("from", from),
                ])
            });
        }
        if !a.enabled.is_empty() {
            let want = a.enabled == "true";
            let have = at(conf, "enabled").is_some_and(py::truthy);
            self.actions.push(if want == have {
                act(vec![
                    ("step", s("set-enabled")),
                    ("enabled", Value::Bool(want)),
                    ("status", s("skipped")),
                    ("reason", s(&format!("already {}", a.enabled))),
                ])
            } else {
                act(vec![
                    ("step", s("set-enabled")),
                    ("enabled", Value::Bool(want)),
                    ("status", s("pending")),
                    ("from", Value::Bool(have)),
                ])
            });
        }
        if !a.new_name.is_empty() {
            let from = at(conf, "name").cloned().unwrap_or(Value::Null);
            self.actions.push(if ops::py_eq(&s(&a.new_name), &from) {
                act(vec![
                    ("step", s("rename")),
                    ("name", s(&a.new_name)),
                    ("status", s("skipped")),
                    ("reason", s(&format!("already named {}", a.new_name))),
                ])
            } else {
                act(vec![
                    ("step", s("rename")),
                    ("name", s(&a.new_name)),
                    ("status", s("pending")),
                    ("from", from),
                ])
            });
        }
        let mut persona_content = None;
        if !a.persona.is_empty() {
            let content = match std::fs::read(&a.persona) {
                Ok(b) => read_text(&b),
                Err(e) => {
                    let f = pyjson::os_error(&e, &a.persona);
                    return Err(fail(
                        &s("bad_usage"),
                        &format!("cannot read persona file {}: {}", a.persona, f.message),
                        Exit::Usage,
                    ));
                }
            };
            let current = match at(conf, "personaPath").filter(|v| py::truthy(v)) {
                Some(Value::Str(t)) => std::fs::read(py::str_of(&Value::Str(t.clone())))
                    .ok()
                    .map(|b| read_text(&b)),
                _ => None,
            };
            self.actions.push(if current.as_deref() == Some(content.as_str()) {
                act(vec![
                    ("step", s("write-persona")),
                    ("source", s(&a.persona)),
                    ("status", s("skipped")),
                    ("reason", s("identical content")),
                ])
            } else {
                act(vec![
                    ("step", s("write-persona")),
                    ("source", s(&a.persona)),
                    ("status", s("pending")),
                ])
            });
            persona_content = Some(content);
        }
        // Current memberships: {group id: isPoc}.
        let mut member_of: Vec<(Value, bool)> = Vec::new();
        if let Some(ps) = at(conf, "projects").filter(|v| py::truthy(v)) {
            for p in py_iter(ps, L)? {
                if py_get(&p, "id", L)?.is_some_and(py::truthy) {
                    let id = ops::index(&p, "id", L)?.clone();
                    let poc = at(&p, "isPoc").is_some_and(py::truthy);
                    match member_of.iter_mut().find(|(k, _)| ops::py_eq(k, &id)) {
                        Some(slot) => slot.1 = poc,
                        None => member_of.push((id, poc)),
                    }
                }
            }
        }
        let mut adds: Vec<(Value, Value, bool)> = Vec::new();
        for sel in &a.add {
            let d = self.resolve_project(sel)?;
            let id = ops::index(&d, "id", L)?.clone();
            if adds.iter().all(|(x, _, _)| !ops::py_eq(x, &id)) {
                let name = match at(&d, "name").filter(|v| py::truthy(v)) {
                    Some(n) => n.clone(),
                    None => s(sel),
                };
                let memberless = !at(&d, "members").is_some_and(py::truthy);
                adds.push((id, name, memberless));
            }
        }
        let mut removes: Vec<(Value, Value)> = Vec::new();
        for sel in &a.remove {
            let d = self.resolve_project(sel)?;
            let id = ops::index(&d, "id", L)?.clone();
            if removes.iter().all(|(x, _)| !ops::py_eq(x, &id)) {
                let name = match at(&d, "name").filter(|v| py::truthy(v)) {
                    Some(n) => n.clone(),
                    None => s(sel),
                };
                removes.push((id, name));
            }
        }
        for (id, name, _) in &adds {
            if removes.iter().any(|(r, _)| ops::py_eq(r, id)) {
                return Err(fail(
                    &s("bad_usage"),
                    &format!(
                        "project '{}' is in both --add-project and --remove-project",
                        py::str_of(name)
                    ),
                    Exit::Usage,
                ));
            }
        }
        let member = |id: &Value| member_of.iter().find(|(k, _)| ops::py_eq(k, id)).map(|(_, p)| *p);
        for (id, name, memberless) in &adds {
            let mut x = act(vec![
                ("step", s("join-project")),
                ("project", name.clone()),
                ("projectId", id.clone()),
            ]);
            if member(id).is_some() {
                put(&mut x, "status", s("skipped"));
                put(&mut x, "reason", s("already a member"));
            } else {
                put(&mut x, "status", s("pending"));
                if *memberless {
                    put(&mut x, "becamePoc", Value::Bool(true));
                }
            }
            self.actions.push(x);
        }
        for (id, name) in &removes {
            let mut x = act(vec![
                ("step", s("leave-project")),
                ("project", name.clone()),
                ("projectId", id.clone()),
            ]);
            match member(id) {
                None => {
                    put(&mut x, "status", s("skipped"));
                    put(&mut x, "reason", s("not a member"));
                }
                Some(poc) => {
                    put(&mut x, "status", s("pending"));
                    if poc {
                        put(
                            &mut x,
                            "note",
                            s("current PoC \u{2014} refused until a successor is named"),
                        );
                    }
                }
            }
            self.actions.push(x);
        }
        if a.dry {
            let pending = self.actions.iter().any(|x| status_is(x, "pending"));
            return self.emit(true, pending);
        }
        self.apply(persona_content)
    }

    /// `resolve_project(sel)`.
    fn resolve_project(&self, sel: &str) -> Result<Value, Stop> {
        let d = self.http_get(
            "/cli/project-group/show",
            vec![("group".to_string(), sel.to_string())],
        );
        let err = py_get(&d, "error", L)?.cloned().unwrap_or(Value::Null);
        if py::truthy(&err) {
            let err = match err {
                Value::Str(_) => obj(vec![("code", s("error")), ("hint", err)]),
                other => other,
            };
            let mut code = py_get(&err, "code", L)?.cloned().unwrap_or_else(|| s("error"));
            if ops::py_eq(&code, &s("ambiguous_id")) {
                code = s("ambiguous");
            }
            let hint = at(&err, "hint").filter(|v| py::truthy(v)).cloned();
            if ops::py_eq(&code, &s("not_found")) || ops::py_eq(&code, &s("ambiguous")) {
                let hint = hint.map_or_else(
                    || format!("no project matches '{sel}' \u{2014} see: {} project list", cli()),
                    |h| py::str_of(&h),
                );
                return Err(fail(&code, &hint, Exit::NotFound));
            }
            let why = hint.map_or_else(|| py::str_of(&code), |h| py::str_of(&h));
            return Err(fail(
                &s("daemon_error"),
                &format!("cannot resolve project '{sel}': {why}"),
                Exit::Error,
            ));
        }
        if !at(&d, "ok").is_some_and(py::truthy) || !at(&d, "id").is_some_and(py::truthy) {
            let shown: String = dumps(&d).chars().take(200).collect();
            return Err(fail(
                &s("daemon_error"),
                &format!("unexpected response resolving project '{sel}': {shown}"),
                Exit::Error,
            ));
        }
        Ok(d)
    }

    fn emit(&mut self, ok: bool, changed: bool) -> Result<(), Stop> {
        if self.a.json {
            let mut out = vec![
                ("ok", Value::Bool(ok)),
                ("changed", Value::Bool(changed)),
                ("name", at(self.conf, "name").cloned().unwrap_or(Value::Null)),
                ("path", self.path.clone()),
                (
                    "actions",
                    Value::Array(self.actions.iter().map(to_value).collect()),
                ),
            ];
            if self.a.dry {
                out.push(("dryRun", Value::Bool(true)));
            }
            self.stdout.push_str(&(dumps(&obj(out)) + "\n"));
            return Ok(());
        }
        let label = self.label.clone();
        if self.a.dry {
            self.stdout.push_str(&format!(
                "DRY RUN \u{2014} no changes made. Plan for {label}:\n"
            ));
        }
        for x in self.actions.clone() {
            let line = action_line(&x)?;
            let status = py::str_of(aget(&x, "status").unwrap_or(&Value::Null));
            let marker = match status.as_str() {
                "pending" => "would apply",
                "done" => "done",
                "skipped" => "skipped",
                "failed" => "FAILED",
                _ => return Err(Raise::at(L, format!("KeyError: {}", py::repr(&s(&status)))).into()),
            };
            self.stdout.push_str(&format!(
                "  {} {line}\n",
                ljust(&format!("[{marker}]"), 14)
            ));
        }
        if !self.a.dry {
            let n = self.actions.iter().filter(|x| status_is(x, "done")).count();
            let what = if n > 0 {
                format!("{n} change(s) applied.")
            } else {
                "no changes.".to_string()
            };
            self.stdout.push_str(&format!("{label}: {what}\n"));
        }
        Ok(())
    }

    fn fail_steps(&mut self, steps: &[&str]) {
        for x in &mut self.actions {
            let step = aget(x, "step").map(py::str_of).unwrap_or_default();
            if status_is(x, "pending") && steps.contains(&step.as_str()) {
                put(x, "status", s("failed"));
            }
        }
    }

    fn apply(&mut self, persona_content: Option<String>) -> Result<(), Stop> {
        let mut changed = false;
        // 1. mode and enabled.
        let mut fields = Vec::new();
        for x in &self.actions {
            if !status_is(x, "pending") {
                continue;
            }
            let step = aget(x, "step").map(py::str_of).unwrap_or_default();
            if step == "set-mode" {
                fields.push(("agent_mode", aget(x, "mode").cloned().unwrap_or(Value::Null)));
            } else if step == "set-enabled" {
                let on = aget(x, "enabled").is_some_and(py::truthy);
                fields.push(("agent_enabled", s(if on { "1" } else { "0" })));
            }
        }
        if !fields.is_empty() {
            let resp = self.post_json(
                "/cli/workspace/set",
                &obj(vec![("project", self.path.clone()), ("fields", obj(fields))]),
            );
            let err = py_get(&resp, "error", L)?.cloned().unwrap_or(Value::Null);
            if py::truthy(&err) || !at(&resp, "ok").is_some_and(py::truthy) {
                self.fail_steps(&["set-mode", "set-enabled"]);
                self.emit(false, changed)?;
                let hint = match &err {
                    Value::Str(_) => py::str_of(&err),
                    _ => {
                        let e = if py::truthy(&err) { err.clone() } else { obj(vec![]) };
                        match py_get(&e, "hint", L)?.filter(|v| py::truthy(v)) {
                            Some(h) => py::str_of(h),
                            None => dumps(&resp).chars().take(300).collect(),
                        }
                    }
                };
                return Err(fail(
                    &s("daemon_error"),
                    &format!("settings write failed: {hint}"),
                    Exit::Error,
                ));
            }
            let mut by_field: Vec<(Value, Value)> = Vec::new();
            let acts = at(&resp, "actions").cloned().unwrap_or(Value::Array(Vec::new()));
            for f in py_iter(&acts, L)? {
                let k = py_get(&f, "field", L)?.cloned().unwrap_or(Value::Null);
                let v = py_get(&f, "status", L)?.cloned().unwrap_or(Value::Null);
                match by_field.iter_mut().find(|(x, _)| ops::py_eq(x, &k)) {
                    Some(slot) => slot.1 = v,
                    None => by_field.push((k, v)),
                }
            }
            let field = |name: &str| {
                by_field
                    .iter()
                    .find(|(k, _)| ops::py_eq(k, &s(name)))
                    .map_or_else(|| s("done"), |(_, v)| v.clone())
            };
            for x in &mut self.actions {
                if !status_is(x, "pending") {
                    continue;
                }
                let step = aget(x, "step").map(py::str_of).unwrap_or_default();
                if step == "set-mode" {
                    put(x, "status", field("agent_mode"));
                } else if step == "set-enabled" {
                    put(x, "status", field("agent_enabled"));
                }
                if status_is(x, "done") {
                    changed = true;
                }
            }
        }
        // 2. Rename.
        for i in 0..self.actions.len() {
            let x = &self.actions[i];
            if !(aget(x, "step").is_some_and(|v| ops::py_eq(v, &s("rename"))) && status_is(x, "pending")) {
                continue;
            }
            let name = py::str_of(aget(x, "name").unwrap_or(&Value::Null));
            let resp = self.post_query(
                "/cli/workspace/set-agent-display-name",
                vec![
                    ("project_path".to_string(), py::str_of(&self.path)),
                    ("name".to_string(), name),
                ],
            );
            let err = py_get(&resp, "error", L)?;
            if err.is_some_and(py::truthy) || !at(&resp, "success").is_some_and(py::truthy) {
                let why = match at(&resp, "error") {
                    Some(e) => py::str_of(e),
                    None => dumps(&resp).chars().take(300).collect(),
                };
                put(&mut self.actions[i], "status", s("failed"));
                self.emit(false, changed)?;
                return Err(fail(&s("daemon_error"), &format!("rename failed: {why}"), Exit::Error));
            }
            put(&mut self.actions[i], "status", s("done"));
            changed = true;
        }
        // 3. Persona.
        for i in 0..self.actions.len() {
            let x = &self.actions[i];
            if !(aget(x, "step").is_some_and(|v| ops::py_eq(v, &s("write-persona"))) && status_is(x, "pending")) {
                continue;
            }
            let path = py::str_of(&self.path);
            let base = path.rsplit('/').next().unwrap_or("");
            let agent_name: String = base
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' })
                .collect();
            let agent_name = if agent_name.is_empty() { "agent".to_string() } else { agent_name };
            if !at(self.conf, "personaPath").is_some_and(py::truthy) {
                let resp = self.post_query(
                    "/cli/agents/create",
                    vec![
                        ("project_path".to_string(), path.clone()),
                        ("name".to_string(), agent_name.clone()),
                        ("role".to_string(), String::new()),
                    ],
                );
                if py_get(&resp, "error", L)?.is_some_and(py::truthy) {
                    let why = py::str_of(at(&resp, "error").unwrap_or(&Value::Null));
                    put(&mut self.actions[i], "status", s("failed"));
                    self.emit(false, changed)?;
                    return Err(fail(
                        &s("daemon_error"),
                        &format!("persona scaffold failed: {why}"),
                        Exit::Error,
                    ));
                }
            }
            let content = persona_content.clone().map_or(Value::Null, |c| s(&c));
            let resp = self.post_json(
                "/cli/agents/save-agent-md",
                &obj(vec![
                    ("project_path", self.path.clone()),
                    ("agent_name", s(&agent_name)),
                    ("content", content),
                ]),
            );
            let err = py_get(&resp, "error", L)?;
            if err.is_some_and(py::truthy) || !at(&resp, "success").is_some_and(py::truthy) {
                let why = match at(&resp, "error") {
                    Some(e) => py::str_of(e),
                    None => dumps(&resp).chars().take(300).collect(),
                };
                put(&mut self.actions[i], "status", s("failed"));
                self.emit(false, changed)?;
                return Err(fail(
                    &s("daemon_error"),
                    &format!("persona write failed: {why}"),
                    Exit::Error,
                ));
            }
            put(&mut self.actions[i], "status", s("done"));
            changed = true;
        }
        // 4. Project memberships.
        for i in 0..self.actions.len() {
            let x = &self.actions[i];
            let step = aget(x, "step").map(py::str_of).unwrap_or_default();
            if !matches!(step.as_str(), "join-project" | "leave-project") || !status_is(x, "pending") {
                continue;
            }
            let route = if step == "join-project" {
                "/cli/project-group/add-member"
            } else {
                "/cli/project-group/remove-member"
            };
            let group = aget(x, "projectId").cloned().unwrap_or(Value::Null);
            let project = aget(x, "project").cloned().unwrap_or(Value::Null);
            let resp = self.post_json(
                route,
                &obj(vec![("group", group), ("workspace", self.path.clone())]),
            );
            let err = py_get(&resp, "error", L)?.cloned().unwrap_or(Value::Null);
            if py::truthy(&err) || !at(&resp, "ok").is_some_and(py::truthy) {
                let err = match err {
                    Value::Str(_) => obj(vec![("code", s("daemon_error")), ("hint", err)]),
                    e if py::truthy(&e) => e,
                    _ => obj(vec![
                        ("code", s("daemon_error")),
                        ("hint", s(&dumps(&resp).chars().take(300).collect::<String>())),
                    ]),
                };
                put(&mut self.actions[i], "status", s("failed"));
                self.emit(false, changed)?;
                let code = match py_get(&err, "code", L)? {
                    Some(c) if py::truthy(c) => c.clone(),
                    _ => s("daemon_error"),
                };
                let mut hint = match at(&err, "hint") {
                    Some(h) if py::truthy(h) => py::str_of(h),
                    _ => py::str_of(&code),
                };
                if ops::py_eq(&code, &s("poc_successor_required")) {
                    hint.push_str(&format!(
                        " \u{2014} name a successor first: {} project poc {} <workspace>",
                        cli(),
                        py::str_of(&project)
                    ));
                }
                return Err(fail(&code, &hint, Exit::Error));
            }
            let x = &mut self.actions[i];
            if step == "join-project" && at(&resp, "alreadyMember").is_some_and(py::truthy) {
                put(x, "status", s("skipped"));
                put(x, "reason", s("already a member"));
                pop(x, "becamePoc");
            } else {
                put(x, "status", s("done"));
                pop(x, "note");
                if step == "join-project" {
                    if at(&resp, "becamePoc").is_some_and(py::truthy) {
                        put(x, "becamePoc", Value::Bool(true));
                    } else {
                        pop(x, "becamePoc");
                    }
                }
                changed = true;
            }
        }
        self.emit(true, changed)
    }
}

/// `open(p, encoding="utf-8", errors="replace").read()`.
fn read_text(b: &[u8]) -> String {
    String::from_utf8_lossy(b)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

/// One action's text.
fn action_line(x: &Action) -> Result<String, Raise> {
    let g = |k: &str| aget(x, k);
    let sv = |k: &str| ops::s_of(aget(x, k));
    let idx = |k: &str| {
        aget(x, k)
            .map(py::str_of)
            .ok_or_else(|| Raise::at(L, format!("KeyError: '{k}'")))
    };
    let skipped = status_is(x, "skipped");
    let step = idx("step")?;
    Ok(match step.as_str() {
        "set-mode" => {
            if skipped {
                format!("mode: {}", idx("mode")?)
            } else {
                format!("mode: {} \u{2192} {}", sv("from"), idx("mode")?)
            }
        }
        "set-enabled" => {
            let want = if g("enabled").is_some_and(py::truthy) { "true" } else { "false" };
            if skipped {
                format!("enabled: {want}")
            } else {
                let from = if g("from").is_some_and(py::truthy) { "true" } else { "false" };
                format!("enabled: {from} \u{2192} {want}")
            }
        }
        "rename" => {
            if skipped {
                format!("name: {}", idx("name")?)
            } else {
                format!("name: {} \u{2192} {}", sv("from"), idx("name")?)
            }
        }
        "join-project" | "leave-project" => {
            let mut line = format!("{step}: {}", idx("project")?);
            if step == "join-project" && g("becamePoc").is_some_and(py::truthy) {
                line.push_str(" (first member \u{2014} Point of Contact)");
            }
            if step == "leave-project" {
                if let Some(n) = g("note").filter(|v| py::truthy(v)) {
                    line.push_str(&format!(" ({})", py::str_of(n)));
                }
            }
            if skipped {
                if let Some(r) = g("reason").filter(|v| py::truthy(v)) {
                    line.push_str(&format!("  ({})", py::str_of(r)));
                }
            }
            line
        }
        _ => {
            if skipped {
                "persona: unchanged (identical content)".to_string()
            } else {
                format!("persona: replace from {} (previous backed up)", idx("source")?)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words() {
        assert!(parse(&["qa"]).unwrap_err().starts_with("nothing to set"));
        assert!(parse(&["qa", "--mode", "x"]).unwrap_err().starts_with("invalid --mode"));
        assert!(parse(&["qa", "--enabled"]).is_err());
        let a = parse(&["qa", "--mode", "k2so", "--add-project", "A", "--add-project", "B"]).unwrap();
        assert_eq!(a.add, vec!["A", "B"]);
        assert!(parse(&["qa", "--persona", "/no/such/file"]).unwrap_err().starts_with("persona file"));
    }

    #[test]
    fn the_lines() {
        let x = act(vec![
            ("step", s("set-mode")),
            ("mode", s("k2")),
            ("status", s("pending")),
            ("from", Value::Null),
        ]);
        assert_eq!(action_line(&x).unwrap(), "mode: None \u{2192} k2");
        let x = act(vec![
            ("step", s("join-project")),
            ("project", s("Acme")),
            ("projectId", s("pg1")),
            ("status", s("skipped")),
            ("reason", s("already a member")),
        ]);
        assert_eq!(action_line(&x).unwrap(), "join-project: Acme  (already a member)");
    }
}
