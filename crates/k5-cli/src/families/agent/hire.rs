//! `cmd_agent_hire` (`cli/src/agent.sh`): the words and their checks in
//! the shell's order, then its Python block (`python3 -`, its lines are
//! the `<stdin>` lines the tracebacks name), step by step:
//!
//! 1. With `--template` and no folder: the hire core's dry run names the
//!    folder ([`plan`]).
//! 2. The look-ups that fail fast: `--agent` (`/cli/presets/list`),
//!    `--project` (`/cli/project-group/show`), `--color`, `--focus-group`
//!    (`/cli/focus-groups/list`), then the conf (`/cli/agent/conf`) and,
//!    for a workspace already there, its layers (`/cli/context/layers`).
//! 3. The plan: one action per step, each `pending` or `skipped`, and the
//!    connect targets resolved by the daemon (`/cli/connections`).
//! 4. A dry run asks the hire core the same question and prints the plan.
//! 5. Else each step in turn ([`apply`]): the hire core
//!    (`/cli/agents/hire`), the settings (`/cli/workspace/set`), the name,
//!    color and focus group of the new workspace, the connections, the
//!    projects, the context layers, the launch, the hello, the onboard
//!    message and the App's handover. A failed step is reported and the
//!    rest go on, unless the workspace itself couldn't be made.
//! 6. Every step's line ([`emit`]), or the JSON; exit non-zero when a
//!    step failed.
//!
//! Every call is the block's own urllib call (K2's ruling D4) with the
//! shell's credential: `$TOKEN` for every route, and, for the hire core,
//! the handover and the project membership, the session passport in a
//! session with one. That one case (a cell socket and a passport) is the
//! shell's still: the shell sends the owner token from disk on the other
//! routes, and the binary never does (PRD 13.9 #4), so [`super::run`]
//! hands it back before anything is printed or sent. Everywhere else the
//! token is one and the same.
//!
//! Also the shell's, before anything is printed or sent: a `--brief` file
//! with a NUL byte in it (bash drops it with a warning naming its own
//! line), one it can't read, and a value above what one environment
//! variable may hold (the brief, or the JSON of the lists).

mod apply;
mod color;
mod emit;
mod http;
mod plan;

use super::{cli, dumps, obj, s};
use crate::exit::Exit;
use crate::families::{Raise, Shown};
use crate::out::json::{Text, Value};
use crate::out::py;
use crate::prelude::Closing;
use crate::transport::urllib::Opener;
use crate::transport::Client;

/// The most one environment variable may hold on Linux
/// (`MAX_ARG_STRLEN`, with its NUL): above it the shell's `python3`
/// doesn't start.
const ENV_MAX: usize = 128 * 1024 - 1;

/// `cmd_agent_hire`'s words, after the shell's checks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HireArgs {
    pub dir: String,
    pub name: String,
    /// `--template` as typed (the shell tests it so); the block strips it.
    pub template: String,
    pub persona: String,
    pub mode: String,
    pub enabled: String,
    pub onboard: String,
    pub agent_preset: String,
    pub launch: bool,
    pub dry_run: bool,
    pub json: bool,
    pub no_wiki: bool,
    pub no_agents_md: bool,
    pub fanout: bool,
    pub db_access: String,
    pub color: String,
    pub focus_group: String,
    /// `--take-app` as typed; the block strips it.
    pub take_app: String,
    /// The `--brief` file's text, as `$(cat file)` gives it, read the way
    /// the block reads its environment (`surrogateescape`).
    pub brief: Option<Text>,
    pub connects: Vec<String>,
    pub projects: Vec<String>,
    pub contexts: Vec<String>,
    /// `dict(a.split("=", 1) for a in params)`: a key given again keeps
    /// its place and takes the later value.
    pub params: Vec<(String, String)>,
    /// `--answer`: the key stripped, each once.
    pub answers: Vec<(String, String)>,
}

/// The value after a flag, or the shell's line for its absence.
fn value<'a>(rest: &[&'a str], i: usize, hint: &str) -> Result<&'a str, String> {
    rest.get(i + 1).copied().ok_or_else(|| hint.to_string())
}

/// `[ -f "$p" ]`.
fn is_file(p: &str) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file())
}

/// The words, in the shell's order of checks. `None`: the shell's (a
/// brief it reads otherwise, or a value too large for its environment).
pub fn parse(rest: &[&str]) -> Option<Result<HireArgs, String>> {
    match parse_words(rest) {
        Ok(Some(a)) => Some(Ok(a)),
        Ok(None) => None,
        Err(hint) => Some(Err(hint)),
    }
}

fn parse_words(rest: &[&str]) -> Result<Option<HireArgs>, String> {
    let cli = cli();
    let mut a = HireArgs::default();
    let mut brief_file = String::new();
    let mut params: Vec<String> = Vec::new();
    let mut answers: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let mut two = true;
        match w {
            "--name" => a.name = value(rest, i, "--name requires a value")?.to_string(),
            "--agent" => {
                a.agent_preset = value(
                    rest,
                    i,
                    &format!("--agent requires a preset id/label (see: {cli} preset list)"),
                )?
                .to_string()
            }
            "--template" => {
                a.template = value(
                    rest,
                    i,
                    &format!("--template requires a template id (see: {cli} agent template list)"),
                )?
                .to_string()
            }
            "--param" => {
                let v = value(
                    rest,
                    i,
                    &format!("--param requires key=value (see: {cli} agent template show <id>)"),
                )?;
                if !v.contains('=') {
                    return Err(format!("--param takes key=value, got '{v}'"));
                }
                params.push(v.to_string());
            }
            "--answer" => {
                let v = value(
                    rest,
                    i,
                    &format!(
                        "--answer requires question-id=text (see: {cli} agent template show <id>)"
                    ),
                )?;
                // `?*=*`: an `=` after the first character.
                if !v.chars().skip(1).any(|c| c == '=') {
                    return Err(format!(
                        "--answer takes question-id=text, got '{v}' (see: {cli} agent template show <id>)"
                    ));
                }
                answers.push(v.to_string());
            }
            "--persona" => a.persona = value(rest, i, "--persona requires a file path")?.to_string(),
            "--mode" => {
                a.mode = value(
                    rest,
                    i,
                    "--mode requires a value (off | custom | manager | k2)",
                )?
                .to_string()
            }
            "--enabled" => {
                a.enabled = value(rest, i, "--enabled requires a value (true | false)")?.to_string()
            }
            "--connect" => a
                .connects
                .push(value(rest, i, "--connect requires an agent name")?.to_string()),
            "--project" => a.projects.push(
                value(
                    rest,
                    i,
                    &format!("--project requires a project name or id (see: {cli} project list)"),
                )?
                .to_string(),
            ),
            "--focus-group" => {
                a.focus_group = value(
                    rest,
                    i,
                    "--focus-group requires a group name or id (or none)",
                )?
                .to_string()
            }
            "--color" => {
                a.color = value(rest, i, "--color requires a hex or rgb value")?.to_string()
            }
            "--context" => a.contexts.push(
                value(
                    rest,
                    i,
                    &format!(
                        "--context requires a path or catalog id (see: {cli} agent context catalog)"
                    ),
                )?
                .to_string(),
            ),
            "--take-app" => {
                a.take_app = value(
                    rest,
                    i,
                    &format!("--take-app requires an App id ({cli} app list)"),
                )?
                .to_string()
            }
            "--brief" => {
                brief_file = value(
                    rest,
                    i,
                    "--brief requires a file (your notes for the new agent)",
                )?
                .to_string()
            }
            "--onboard" => a.onboard = value(rest, i, "--onboard requires a message")?.to_string(),
            "--db-access" => {
                a.db_access = value(rest, i, "--db-access requires off|read|write")?.to_string()
            }
            _ => {
                two = false;
                match w {
                    "--no-wiki" => a.no_wiki = true,
                    "--no-agents-md" => a.no_agents_md = true,
                    "--fanout" => a.fanout = true,
                    "--launch" => a.launch = true,
                    "--dry-run" => a.dry_run = true,
                    "--json" => a.json = true,
                    _ if w.starts_with('-') => {
                        return Err(format!(
                            "unknown flag '{w}' \u{2014} see: {cli} agent hire --help"
                        ))
                    }
                    _ if a.dir.is_empty() => a.dir = w.to_string(),
                    _ => {
                        return Err(format!(
                            "unexpected extra argument '{w}' \u{2014} usage: {cli} agent hire <dir> [flags]"
                        ))
                    }
                }
            }
        }
        i += if two { 2 } else { 1 };
    }
    if a.dir.is_empty() && a.template.is_empty() {
        return Err(format!(
            "missing <dir> \u{2014} usage: {cli} agent hire <dir> [flags], or {cli} agent hire --template <id> --name <n> to hire under the hire root (see: {cli} agent hire --help)"
        ));
    }
    if !a.template.is_empty() && !a.persona.is_empty() {
        return Err(
            "--template and --persona are mutually exclusive \u{2014} pass one or the other"
                .to_string(),
        );
    }
    if !params.is_empty() && a.template.is_empty() {
        return Err("--param goes with --template <id>".to_string());
    }
    if !answers.is_empty() && a.template.is_empty() {
        return Err("--answer goes with --template <id>".to_string());
    }
    // `[ ! -f ] || [ ! -r ]`.
    if !a.persona.is_empty() && !(is_file(&a.persona) && std::fs::File::open(&a.persona).is_ok())
    {
        return Err(format!(
            "persona file not found or unreadable: {}",
            a.persona
        ));
    }
    if !matches!(
        a.mode.as_str(),
        "" | "off" | "custom" | "manager" | "k2" | "k2so"
    ) {
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
    if !matches!(a.db_access.as_str(), "" | "off" | "read" | "write") {
        return Err(format!(
            "invalid --db-access '{}' \u{2014} valid: off, read, write",
            a.db_access
        ));
    }
    // The self-grant check is a session's with a passport: the shell's.
    if !a.onboard.is_empty() {
        a.launch = true;
    }
    if !brief_file.is_empty() {
        if a.take_app.is_empty() {
            return Err("--brief goes with --take-app".to_string());
        }
        if !is_file(&brief_file) {
            return Err(format!("brief file not found: {brief_file}"));
        }
        let Ok(mut bytes) = std::fs::read(&brief_file) else {
            return Ok(None);
        };
        if bytes.contains(&0) {
            return Ok(None);
        }
        while bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        if bytes.len() > ENV_MAX {
            return Ok(None);
        }
        a.brief = Some(escaped(&bytes));
    }
    for p in &params {
        let (k, v) = p.split_once('=').unwrap_or((p, ""));
        match a.params.iter_mut().find(|(key, _)| key == k) {
            Some(slot) => slot.1 = v.to_string(),
            None => a.params.push((k.to_string(), v.to_string())),
        }
    }
    for p in &answers {
        let (k, v) = p.split_once('=').unwrap_or((p, ""));
        let k = super::py_strip(k);
        if a.answers.iter().any(|(key, _)| key == k) {
            return Err(format!("--answer {k} given twice"));
        }
        a.answers.push((k.to_string(), v.to_string()));
    }
    // The lists go to the block in its environment, one variable each.
    let too_big = [
        dumps(&strs(&a.connects)),
        dumps(&strs(&a.projects)),
        dumps(&strs(&a.contexts)),
        dumps(&pairs(&a.params)),
        dumps(&pairs(&a.answers)),
    ]
    .iter()
    .any(|j| j.len() > ENV_MAX);
    if too_big {
        return Ok(None);
    }
    Ok(Some(a))
}

/// A list of strings as a JSON value.
fn strs(v: &[String]) -> Value {
    Value::Array(v.iter().map(|x| s(x)).collect())
}

/// Pairs as a JSON object, in their order.
fn pairs(v: &[(String, String)]) -> Value {
    Value::Object(
        v.iter()
            .map(|(k, x)| (Text::from(k.as_str()), s(x)))
            .collect(),
    )
}

/// Bytes as Python reads them from its environment (`surrogateescape`):
/// each byte that is not part of UTF-8 is U+DC00 plus the byte.
fn escaped(bytes: &[u8]) -> Text {
    if let Ok(t) = std::str::from_utf8(bytes) {
        return Text::from(t);
    }
    let mut json = String::from("\"");
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(t) => {
                push_json(&mut json, t);
                break;
            }
            Err(e) => {
                let n = e.valid_up_to();
                push_json(&mut json, std::str::from_utf8(&rest[..n]).unwrap_or_default());
                let bad = e.error_len().unwrap_or(rest.len() - n);
                for b in &rest[n..n + bad] {
                    json.push_str(&format!("\\udc{b:02x}"));
                }
                rest = &rest[n + bad..];
            }
        }
    }
    json.push('"');
    match crate::out::json::parse(&json) {
        Ok(Value::Str(t)) => t,
        _ => Text::from(String::from_utf8_lossy(bytes).into_owned()),
    }
}

/// `t` inside a JSON string.
fn push_json(out: &mut String, t: &str) {
    for c in t.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
}

/// How the block stops: `sys.exit(n)` after its lines, or an exception.
pub(super) enum Stop {
    Exit(Exit),
    Raise(Raise),
}

impl From<Raise> for Stop {
    fn from(r: Raise) -> Stop {
        Stop::Raise(r)
    }
}

pub(super) type R<T> = Result<T, Stop>;

/// A frame of the traceback: the line and the function.
pub(super) type Frame = (u32, &'static str);

/// An exception raised with these frames, outermost first.
pub(super) fn raise(frames: &[Frame], exception: impl Into<String>) -> Raise {
    Raise {
        frames: frames.to_vec(),
        exception: exception.into(),
    }
}

/// `v.get(key)` on a value from the daemon: `AttributeError` when it is
/// not a dict.
pub(super) fn pget<'v>(v: &'v Value, key: &str, fr: &[Frame]) -> Result<Option<&'v Value>, Raise> {
    match v {
        Value::Object(pairs) => Ok(crate::families::get(pairs, key)),
        other => Err(raise(
            fr,
            format!(
                "AttributeError: '{}' object has no attribute 'get'",
                py::type_name(other)
            ),
        )),
    }
}

/// `v.get(key)` truthy.
pub(super) fn ptrue(v: &Value, key: &str, fr: &[Frame]) -> Result<bool, Raise> {
    Ok(pget(v, key, fr)?.is_some_and(py::truthy))
}

/// `x or default`.
pub(super) fn or(x: Option<&Value>, default: Value) -> Value {
    match x {
        Some(v) if py::truthy(v) => v.clone(),
        _ => default,
    }
}

/// `x.lower()` on a value that must be a string.
pub(super) fn lower(v: &Value, fr: &[Frame]) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(py::str_of(v).to_lowercase()),
        other => Err(raise(
            fr,
            format!(
                "AttributeError: '{}' object has no attribute 'lower'",
                py::type_name(other)
            ),
        )),
    }
}

/// `json.dumps(v)[:n]`.
pub(super) fn dumps_head(v: &Value, n: usize) -> String {
    dumps(v).chars().take(n).collect()
}

/// An action: a dict of the block's own.
pub(super) mod act {
    use crate::out::json::{Text, Value};

    pub fn get<'a>(a: &'a Value, key: &str) -> Option<&'a Value> {
        match a {
            Value::Object(p) => crate::families::get(p, key),
            _ => None,
        }
    }

    /// `a[key] = v`: in place when the key is there, else last.
    pub fn set(a: &mut Value, key: &str, v: Value) {
        if let Value::Object(p) = a {
            let k = Text::from(key);
            match p.iter_mut().find(|(x, _)| *x == k) {
                Some(slot) => slot.1 = v,
                None => p.push((k, v)),
            }
        }
    }

    /// `a.pop(key, None)`.
    pub fn pop(a: &mut Value, key: &str) {
        if let Value::Object(p) = a {
            let k = Text::from(key);
            p.retain(|(x, _)| *x != k);
        }
    }

    /// `a[key] == text`.
    pub fn is(a: &Value, key: &str, text: &str) -> bool {
        matches!(get(a, key), Some(Value::Str(t)) if t.as_str() == Some(text))
    }

    /// `a["step"]` (always one of the block's names).
    pub fn step(a: &Value) -> String {
        get(a, "step")
            .map(crate::out::py::str_of)
            .unwrap_or_default()
    }

    pub fn pending(a: &Value) -> bool {
        is(a, "status", "pending")
    }
}

/// The block's state as it runs.
pub(super) struct Hire<'a> {
    pub op: Opener,
    pub a: &'a HireArgs,
    /// `TEMPLATE`, stripped.
    pub template: String,
    /// `TAKE_APP`, stripped.
    pub take_app: String,
    pub stdout: String,
    pub stderr: String,
    /// `permission_lines`.
    pub perm: Vec<String>,
    pub actions: Vec<Value>,
    pub path: String,
    pub display_name: Value,
    pub changed: bool,
    pub conf: Value,
    pub registered: bool,
    pub resolved_preset: Option<Value>,
    pub persona_content: Option<String>,
}

impl Hire<'_> {
    /// `fail(code, hint, exit_code)`: the error line, the permission
    /// lines, then `sys.exit`.
    pub fn fail(&mut self, code: Value, hint: Value, exit: Exit) -> Stop {
        let line = dumps(&obj(vec![(
            "error",
            obj(vec![("code", code), ("hint", hint)]),
        )]));
        self.stderr.push_str(&line);
        self.stderr.push('\n');
        for l in &self.perm {
            self.stderr.push_str(l);
            self.stderr.push('\n');
        }
        Stop::Exit(exit)
    }

    /// `note_permission(resp)`.
    pub fn note_permission(&mut self, resp: &Value) {
        let Value::Object(pairs) = resp else {
            return;
        };
        let Some(Value::Object(p)) = crate::families::get(pairs, "permission") else {
            return;
        };
        if !self.perm.is_empty() {
            return;
        }
        let p = Value::Object(p.clone());
        let text = |k: &str| match act::get(&p, k) {
            Some(Value::Str(t)) if py::truthy(&Value::Str(t.clone())) => {
                Some(py::str_of(&Value::Str(t.clone())))
            }
            _ => None,
        };
        if let Some(cmd) = text("command") {
            self.perm.push(format!("Ask with: {cmd}"));
        } else if matches!(act::get(&p, "requestable"), Some(Value::Bool(false))) {
            if let Some(hint) = text("hint") {
                self.perm.push(hint);
            }
            self.perm.push("It can't be requested.".to_string());
        }
    }

    /// The first action of `step` (`one(step)`), by its place.
    pub fn one(&self, step: &str) -> Option<usize> {
        self.actions.iter().position(|a| act::step(a) == step)
    }

    /// Every action of `step`, by place.
    pub fn all(&self, step: &str) -> Vec<usize> {
        (0..self.actions.len())
            .filter(|&i| act::step(&self.actions[i]) == step)
            .collect()
    }

    /// The `template_body(extra)` of the hire core.
    pub fn template_body(&self, extra: Vec<(&str, Value)>) -> Value {
        let mut b = obj(vec![
            ("template", s(&self.template)),
            ("params", pairs(&self.a.params)),
            ("noWiki", Value::Bool(self.a.no_wiki)),
            ("noAgentsMd", Value::Bool(self.a.no_agents_md)),
            ("fanout", Value::Bool(self.a.fanout)),
        ]);
        if !self.a.answers.is_empty() {
            act::set(&mut b, "answers", pairs(&self.a.answers));
        }
        if !self.a.name.is_empty() {
            act::set(&mut b, "name", s(&self.a.name));
        }
        for (k, v) in extra {
            act::set(&mut b, k, v);
        }
        b
    }
}

/// `exit_code` as the block's `sys.exit` gives it.
pub(super) fn exit_of(code: i64) -> Exit {
    match code {
        0 => Exit::Ok,
        2 => Exit::Usage,
        3 => Exit::Refused,
        4 => Exit::NotFound,
        _ => Exit::Error,
    }
}

/// `cmd_agent_hire`'s Python block.
pub fn hire(client: &mut Client, _closing: &mut Closing, a: &HireArgs) -> Exit {
    let mut h = Hire {
        op: Opener::from_client(client),
        a,
        template: super::py_strip(&a.template).to_string(),
        take_app: super::py_strip(&a.take_app).to_string(),
        stdout: String::new(),
        stderr: String::new(),
        perm: Vec::new(),
        actions: Vec::new(),
        path: String::new(),
        display_name: Value::Null,
        changed: false,
        conf: Value::Null,
        registered: false,
        resolved_preset: None,
        persona_content: None,
    };
    let exit = match run_block(&mut h) {
        Ok(()) => Exit::Ok,
        Err(Stop::Exit(e)) => e,
        Err(Stop::Raise(r)) => {
            h.stderr
                .push_str(&crate::families::feedback::write::traceback_stdin(&r));
            Exit::Error
        }
    };
    Shown {
        stdout: h.stdout,
        stderr: h.stderr,
        exit,
    }
    .print()
}

fn run_block(h: &mut Hire) -> R<()> {
    plan::plan(h)?;
    if h.a.dry_run {
        return plan::dry_run(h);
    }
    apply::apply(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(w: &[&str]) -> Result<HireArgs, String> {
        parse(w).expect("native")
    }

    #[test]
    fn the_checks_come_in_the_shells_order() {
        assert_eq!(
            words(&["--name"]).unwrap_err(),
            "--name requires a value"
        );
        assert_eq!(
            words(&["--persona", "x", "--template", "t"]).unwrap_err(),
            "--template and --persona are mutually exclusive \u{2014} pass one or the other"
        );
        assert_eq!(
            words(&["d", "--param", "a=1"]).unwrap_err(),
            "--param goes with --template <id>"
        );
        assert_eq!(
            words(&["--answer", "=x"]).unwrap_err(),
            format!(
                "--answer takes question-id=text, got '=x' (see: {} agent template show <id>)",
                cli()
            )
        );
        assert_eq!(
            words(&["--template", "t", "--answer", " a=1", "--answer", "a =2"]).unwrap_err(),
            "--answer a given twice"
        );
    }

    #[test]
    fn params_keep_their_first_place() {
        let a = words(&["--template", "t", "--param", "a=1", "--param", "b=2", "--param", "a=3=4"])
            .unwrap();
        assert_eq!(
            a.params,
            vec![("a".to_string(), "3=4".to_string()), ("b".to_string(), "2".to_string())]
        );
    }

    #[test]
    fn empty_words_wait_for_the_folder() {
        let a = words(&["", "", "d"]).unwrap();
        assert_eq!(a.dir, "d");
        assert!(words(&["--onboard", "hi", "d"]).unwrap().launch);
    }

    #[test]
    fn a_brief_is_read_as_the_environment_reads_it() {
        assert_eq!(escaped(b"caf\xc3\xa9").code_points(), "caf\u{e9}".chars().map(u32::from).collect::<Vec<_>>());
        assert_eq!(escaped(b"a\xffb").code_points(), vec![0x61, 0xdcff, 0x62]);
    }
}
