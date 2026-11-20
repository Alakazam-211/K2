//! `k5 preset list|show|add|set|remove`: agent presets, as the shell
//! CLI's `preset)` arm and its `cmd_preset_*` functions answer them
//! (`cli/src/router.sh`, `cli/src/preset.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the two gates ([`crate::prelude`]). `preset` is
//!    an Open tool.
//! 2. The verb's words ([`parse`]): a word it can't take is one line of
//!    compact JSON on stderr (`_preset_err "usage" …`), exit 2, nothing
//!    sent.
//! 3. `_preset_py`, a `python3 -` block with its own urllib client
//!    ([`crate::transport::urllib`], K2's ruling D4): `GET
//!    /cli/presets/list` (every verb but `add` resolves its preset there
//!    first), then the verb's POST, each `urlopen(…, timeout=30)` once.
//!    Its errors are compact JSON on stderr: exit 4 for `not_found` and
//!    `ambiguous`, 2 for `usage`, else 1.
//!
//! The credential is the shell's `$TOKEN`: the passport, else the owner
//! token. In a session the shell sends the owner token from disk when
//! there is one (COMPAT-58, an Open tool) and the binary the passport:
//! `scripts/cli-parity.sh`'s named difference `owner-token-with-passport`.
//!
//! Where the shell's Python raises, this prints a traceback whose last
//! line is Python 3.12's.
//!
//! Handed to the shell, before anything is printed or sent: a word that
//! is not UTF-8, `-h` or `--help` anywhere (the front's), a port curl
//! won't take, and a `python3` the binary can't tell.

use super::heartbeat::py::{dget_or, dget_v, or, Py};
use super::{curl_port, s, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude;
use crate::python;
use crate::transport::urllib::{decode, Method, Opener, Outcome, Request};
use k2_client::{cli_name, product_name};
use std::ffi::OsString;
use std::time::Duration;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["preset"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

pub const LIST: &str = "/cli/presets/list";
pub const CREATE: &str = "/cli/presets/create";
pub const UPDATE: &str = "/cli/presets/update";
pub const DELETE: &str = "/cli/presets/delete";

const TIMEOUT: Duration = Duration::from_secs(30);

/// The flags `add` and `set` send.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fields {
    pub id: String,
    pub label: String,
    pub command: String,
    pub icon: String,
    pub readiness: String,
    pub danger: Vec<String>,
    pub env: Vec<String>,
    pub inject_flow: String,
    pub enable: String,
    pub clear_danger: bool,
    pub clear_env: bool,
    pub clear_ready: bool,
    pub clear_inject: bool,
}

/// What a command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    Help,
    /// `_preset_err "usage" <hint>`, exit 2.
    Usage(String),
    List {
        json: bool,
    },
    Show {
        selector: String,
        json: bool,
    },
    Add {
        f: Fields,
        json: bool,
    },
    Set {
        selector: String,
        f: Fields,
        json: bool,
    },
    Remove {
        selector: String,
        json: bool,
    },
}

/// The words after `preset`.
pub fn parse(words: &[&str]) -> Form {
    let c = cli_name!();
    let rest = words.get(1..).unwrap_or_default();
    match words.first().copied().unwrap_or("") {
        "list" => {
            for a in rest {
                if *a != "--json" {
                    return Form::Usage(format!(
                        "unexpected argument '{a}' \u{2014} usage: {c} preset list [--json]"
                    ));
                }
            }
            Form::List {
                json: !rest.is_empty(),
            }
        }
        verb @ ("show" | "remove") => {
            let mut selector = String::new();
            let mut json = false;
            for a in rest {
                match *a {
                    "--json" => json = true,
                    a if a.starts_with('-') => {
                        return Form::Usage(format!(
                            "unknown flag '{a}' \u{2014} usage: {c} preset {verb} <preset> [--json]"
                        ))
                    }
                    a if selector.is_empty() => selector = a.to_string(),
                    a => return Form::Usage(format!("unexpected extra argument '{a}'")),
                }
            }
            if selector.is_empty() {
                return Form::Usage(format!(
                    "{verb} requires a <preset> (id, label, or command token)"
                ));
            }
            if verb == "show" {
                Form::Show { selector, json }
            } else {
                Form::Remove { selector, json }
            }
        }
        "add" => add(rest),
        "set" => set(rest),
        "" | "--help" | "-h" => Form::Help,
        other => Form::Usage(format!(
            "unknown command '{other}' \u{2014} see {c} preset --help"
        )),
    }
}

/// A value flag of `add` and `set`: `Err` with its usage hint when it is
/// the last word.
fn value<'a>(rest: &[&'a str], i: usize, hint: &str) -> Result<&'a str, String> {
    rest.get(i + 1)
        .copied()
        .ok_or_else(|| hint.to_string())
}

/// The flags both verbs take; `Ok(false)`: not one of them.
fn shared(f: &mut Fields, rest: &[&str], i: usize, set: bool) -> Result<bool, String> {
    let dflag = if set {
        "--danger-flag requires a flag value"
    } else {
        "--danger-flag requires a flag value (e.g. --danger-flag --yes-always)"
    };
    match rest[i] {
        "--command" => f.command = value(rest, i, "--command requires a value (quote it)")?.into(),
        "--label" => f.label = value(rest, i, "--label requires a value")?.into(),
        "--icon" => f.icon = value(rest, i, "--icon requires a value")?.into(),
        "--danger-flag" => f.danger.push(value(rest, i, dflag)?.into()),
        "--env" => f.env.push(value(rest, i, "--env requires K=V")?.into()),
        "--readiness" => {
            f.readiness = value(
                rest,
                i,
                "--readiness requires 'bracketed-paste' or 'settle:<ms>'",
            )?
            .into()
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// `cmd_preset_add`.
fn add(rest: &[&str]) -> Form {
    let c = cli_name!();
    let mut f = Fields::default();
    let mut json = false;
    let mut i = 0;
    while i < rest.len() {
        match shared(&mut f, rest, i, false) {
            Err(hint) => return Form::Usage(hint),
            Ok(true) => {
                i += 2;
                continue;
            }
            Ok(false) => {}
        }
        match rest[i] {
            "--id" => match value(rest, i, "--id requires a value") {
                Ok(v) => {
                    f.id = v.into();
                    i += 2;
                    continue;
                }
                Err(hint) => return Form::Usage(hint),
            },
            "--json" => json = true,
            a => {
                return Form::Usage(format!(
                    "unknown argument '{a}' \u{2014} see: {c} preset add --help"
                ))
            }
        }
        i += 1;
    }
    if f.id.is_empty() || f.command.is_empty() {
        return Form::Usage(format!(
            "add requires --id <slug> and --command '<argv string>' (see: {c} preset add --help)"
        ));
    }
    Form::Add { f, json }
}

/// `cmd_preset_set`.
fn set(rest: &[&str]) -> Form {
    let c = cli_name!();
    let mut f = Fields::default();
    let (mut selector, mut json) = (String::new(), false);
    let mut i = 0;
    while i < rest.len() {
        match shared(&mut f, rest, i, true) {
            Err(hint) => return Form::Usage(hint),
            Ok(true) => {
                i += 2;
                continue;
            }
            Ok(false) => {}
        }
        match rest[i] {
            "--inject-flow" => match value(rest, i, "--inject-flow requires a JSON array") {
                Ok(v) => {
                    f.inject_flow = v.into();
                    i += 2;
                    continue;
                }
                Err(hint) => return Form::Usage(hint),
            },
            "--enable" => f.enable = "1".into(),
            "--disable" => f.enable = "0".into(),
            "--clear-danger-flags" => f.clear_danger = true,
            "--clear-env" => f.clear_env = true,
            "--clear-readiness" => f.clear_ready = true,
            "--clear-inject-flow" => f.clear_inject = true,
            "--json" => json = true,
            a if a.starts_with('-') => {
                return Form::Usage(format!(
                    "unknown flag '{a}' \u{2014} see: {c} preset set --help"
                ))
            }
            a if selector.is_empty() => selector = a.into(),
            a => return Form::Usage(format!("unexpected extra argument '{a}'")),
        }
        i += 1;
    }
    if selector.is_empty() {
        return Form::Usage("set requires a <preset> (id, label, or command token)".into());
    }
    if (!f.danger.is_empty() && f.clear_danger)
        || (!f.env.is_empty() && f.clear_env)
        || (!f.readiness.is_empty() && f.clear_ready)
        || (!f.inject_flow.is_empty() && f.clear_inject)
    {
        return Form::Usage("a --clear-* flag and its value flag are mutually exclusive".into());
    }
    Form::Set { selector, f, json }
}

/// `dumps(obj)`: compact, `ensure_ascii=False`.
fn dumps(v: &Value) -> String {
    json::to_string(
        v,
        &Options {
            compact: true,
            ensure_ascii: false,
            ..Options::default()
        },
    )
    .unwrap_or_default()
}

/// `{"error": {"code": code, "hint": hint}}`, compact.
fn err_line(code: &str, hint: &Value) -> String {
    dumps(&Value::Object(vec![(
        Text::from("error"),
        Value::Object(vec![
            (Text::from("code"), s(code)),
            (Text::from("hint"), hint.clone()),
        ]),
    )]))
}

/// `k5 preset ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
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
    Some(run_with(args, form, settings))
}

fn run_with(args: &[OsString], form: Form, mut settings: Settings) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    match &form {
        Form::Help => {
            let text = crate::text::help("preset").unwrap_or_default();
            return Shown::out(text).print();
        }
        Form::Usage(hint) => {
            out::stderr_line(&err_line("usage", &s(hint)));
            return Exit::Usage;
        }
        _ => {}
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let client = match settings.client(passport::current(args).would_refuse()) {
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
    let mut b = Block {
        opener,
        p: Py::default(),
    };
    let r = b.verb(form);
    let p = b.p;
    match r {
        Ok(()) => p.caught(Ok(Exit::Ok)),
        Err(Halt::Exit(e)) => p.caught(Ok(e)),
        Err(Halt::Raise(e)) => p.raised(Err(e), 1),
    }
    .print()
}

/// How the block stops before its end.
enum Halt {
    Exit(Exit),
    Raise(String),
}

impl From<String> for Halt {
    fn from(e: String) -> Halt {
        Halt::Raise(e)
    }
}

type R<T> = Result<T, Halt>;

struct Block {
    opener: Opener,
    p: Py,
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

fn st(v: &Value) -> String {
    py::str_of(v)
}

/// `body[key] = v`: a key there keeps its place.
fn put(body: &mut Vec<(Text, Value)>, key: &str, v: Value) {
    let k = Text::from(key);
    match body.iter_mut().find(|(x, _)| *x == k) {
        Some(slot) => slot.1 = v,
        None => body.push((k, v)),
    }
}

/// Python's `str.split()`.
fn split(text: &str) -> Vec<&str> {
    text.split(super::py_space)
        .filter(|w| !w.is_empty())
        .collect()
}

/// `(v or "")` as a string, the exception for one that isn't.
fn text_or_empty(v: Value, method: &str) -> R<String> {
    match or(v, s("")) {
        x @ Value::Str(_) => Ok(st(&x)),
        other => Err(format!(
            "AttributeError: '{}' object has no attribute '{method}'",
            py::type_name(&other)
        )
        .into()),
    }
}

impl Block {
    /// `die(code, hint)`.
    fn die(&mut self, code: &str, hint: &Value) -> Halt {
        self.p.err(err_line(code, hint));
        Halt::Exit(match code {
            "not_found" | "ambiguous" => Exit::NotFound,
            "usage" => Exit::Usage,
            _ => Exit::Error,
        })
    }

    /// `request(method, route, body=body)`: `None` for any exception.
    fn request(
        &mut self,
        method: Method,
        route: &str,
        body: Option<&Value>,
    ) -> (Option<u16>, Option<Value>) {
        let mut req = Request::new(method, route, TIMEOUT);
        if let Some(b) = body {
            req = req
                .data(dumps(b).into_bytes())
                .header("Content-Type", "application/json");
        }
        if let Some(t) = self.opener.token() {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        match self.opener.open(&req) {
            Outcome::Answer {
                status,
                body: Ok(b),
            } => (Some(status), json::parse(&decode(&b)).ok()),
            Outcome::HttpError { code, body } => {
                let v = body
                    .ok()
                    .and_then(|b| json::parse(&decode(&b)).ok())
                    .unwrap_or_else(|| obj(vec![("error", s(&format!("HTTP {code} on {route}")))]));
                (Some(code), Some(v))
            }
            _ => (None, None),
        }
    }

    /// `call(method, route, body=body)`.
    fn call(&mut self, method: Method, route: &str, body: Option<&Value>) -> R<Value> {
        let (status, d) = self.request(method, route, body);
        let Some(d) = d else {
            let hint = s(&format!(
                "no response from the {} daemon (is it running?)",
                product_name!()
            ));
            return Err(self.die("daemon_unreachable", &hint));
        };
        if let Value::Object(_) = d {
            let err = dget_v(&d, "error")?;
            if py::truthy(&err) {
                let hint = match &err {
                    Value::Str(_) => err.clone(),
                    Value::Object(_) => dget_or(&err, "hint", s(&dumps(&err)))?,
                    other => {
                        return Err(format!(
                            "AttributeError: '{}' object has no attribute 'get'",
                            py::type_name(other)
                        )
                        .into())
                    }
                };
                if status == Some(403) {
                    let h = s("this verb needs the owner token or an Owner/Admin session");
                    return Err(self.die("forbidden", &h));
                }
                return Err(self.die("daemon_error", &hint));
            }
        }
        Ok(d)
    }

    /// `fetch_presets()`.
    fn presets(&mut self) -> R<Vec<Value>> {
        let d = self.call(Method::Get, LIST, None)?;
        match d {
            Value::Array(items) => Ok(items),
            other => {
                let head: String = dumps(&other).chars().take(200).collect();
                let hint = s(&format!("unexpected /cli/presets/list response: {head}"));
                Err(self.die("daemon_error", &hint))
            }
        }
    }

    /// `resolve(selector)`.
    fn resolve(&mut self, selector: &str) -> R<Value> {
        let presets = self.presets()?;
        let want = selector.trim_matches(super::py_space);
        let mut by_id = Vec::new();
        for p in &presets {
            if dget_v(p, "id")? == s(want) {
                by_id.push(p.clone());
            }
        }
        if let Some(p) = by_id.into_iter().next() {
            return Ok(p);
        }
        let mut by_label = Vec::new();
        for p in &presets {
            let l = text_or_empty(dget_v(p, "label")?, "lower")?;
            if l.to_lowercase() == want.to_lowercase() {
                by_label.push(p.clone());
            }
        }
        if by_label.len() == 1 {
            return Ok(by_label.remove(0));
        }
        if by_label.len() > 1 {
            let ids = self.ids(&by_label)?;
            let hint = s(&format!(
                "label '{want}' matches {} presets: {ids} \u{2014} pass the preset id",
                by_label.len()
            ));
            return Err(self.die("ambiguous", &hint));
        }
        let mut by_tok = Vec::new();
        for p in &presets {
            let cmd = text_or_empty(dget_v(p, "command")?, "split")?;
            if split(&cmd).first().copied().unwrap_or("") == want {
                by_tok.push(p.clone());
            }
        }
        if by_tok.len() == 1 {
            return Ok(by_tok.remove(0));
        }
        if by_tok.len() > 1 {
            let ids = self.ids(&by_tok)?;
            let hint = s(&format!(
                "command token '{want}' matches {} presets: {ids} \u{2014} pass the preset id",
                by_tok.len()
            ));
            return Err(self.die("ambiguous", &hint));
        }
        let hint = s(&format!(
            "no preset matches '{want}' \u{2014} run `{} preset list`",
            cli_name!()
        ));
        Err(self.die("not_found", &hint))
    }

    /// `", ".join(p.get("id") or "?" for p in ps)`.
    fn ids(&mut self, ps: &[Value]) -> R<String> {
        let mut out = Vec::new();
        for (i, p) in ps.iter().enumerate() {
            match or(dget_v(p, "id")?, s("?")) {
                v @ Value::Str(_) => out.push(st(&v)),
                other => {
                    return Err(format!(
                        "TypeError: sequence item {i}: expected str instance, {} found",
                        py::type_name(&other)
                    )
                    .into())
                }
            }
        }
        Ok(out.join(", "))
    }

    fn verb(&mut self, form: Form) -> R<()> {
        match form {
            Form::List { json } => self.list(json),
            Form::Show { selector, json } => self.show(&selector, json),
            Form::Add { f, json } => self.write(None, f, json),
            Form::Set { selector, f, json } => self.write(Some(selector), f, json),
            Form::Remove { selector, json } => self.remove(&selector, json),
            Form::Help | Form::Usage(_) => Ok(()),
        }
    }

    fn list(&mut self, json: bool) -> R<()> {
        let presets = self.presets()?;
        if json {
            let v = obj(vec![
                ("ok", Value::Bool(true)),
                ("presets", Value::Array(presets)),
            ]);
            self.p.out(dumps(&v));
            return Ok(());
        }
        let mut rows: Vec<[String; 6]> = Vec::new();
        for p in &presets {
            let cmd = or(dget_v(p, "command")?, s(""));
            let mut cmd = match cmd {
                Value::Str(_) => st(&cmd),
                other => {
                    return Err(format!(
                        "TypeError: object of type '{}' has no len()",
                        py::type_name(&other)
                    )
                    .into())
                }
            };
            if cmd.chars().count() > 44 {
                cmd = cmd.chars().take(41).collect::<String>() + "...";
            }
            let mut meta: Vec<Value> = Vec::new();
            if py::truthy(&dget_v(p, "danger_flags")?) {
                meta.push(s("flags"));
            }
            if py::truthy(&dget_v(p, "env")?) {
                meta.push(s("env"));
            }
            if py::truthy(&dget_v(p, "readiness")?) {
                meta.push(dget_v(p, "readiness")?);
            }
            let mut words = Vec::new();
            for (i, m) in meta.iter().enumerate() {
                match m {
                    Value::Str(_) => words.push(st(m)),
                    other => {
                        return Err(format!(
                            "TypeError: sequence item {i}: expected str instance, {} found",
                            py::type_name(other)
                        )
                        .into())
                    }
                }
            }
            let meta = words.join(",");
            let cell = |v: Value| -> R<String> {
                match v {
                    x @ Value::Str(_) => Ok(st(&x)),
                    other => Err(format!(
                        "TypeError: object of type '{}' has no len()",
                        py::type_name(&other)
                    )
                    .into()),
                }
            };
            rows.push([
                cell(or(dget_v(p, "id")?, s("?")))?,
                cell(or(dget_v(p, "label")?, s("?")))?,
                cmd,
                (if py::truthy(&dget_v(p, "enabled")?) {
                    "yes"
                } else {
                    "no"
                })
                .into(),
                (if py::truthy(&dget_v(p, "is_built_in")?) {
                    "built-in"
                } else {
                    "custom"
                })
                .into(),
                if meta.is_empty() { "-".into() } else { meta },
            ]);
        }
        let head = ["ID", "LABEL", "COMMAND", "ENABLED", "SOURCE", "METADATA"].map(String::from);
        let mut widths = [0usize; 6];
        for r in rows.iter().chain(std::iter::once(&head)) {
            for (i, c) in r.iter().enumerate() {
                widths[i] = widths[i].max(c.chars().count());
            }
        }
        let line = |r: &[String; 6]| -> String {
            r.iter()
                .enumerate()
                .map(|(i, c)| super::ljust(c, widths[i]))
                .collect::<Vec<_>>()
                .join("  ")
        };
        self.p.out(line(&head));
        for r in &rows {
            self.p.out(line(r));
        }
        Ok(())
    }

    fn show(&mut self, selector: &str, json: bool) -> R<()> {
        let p = self.resolve(selector)?;
        if json {
            self.p
                .out(dumps(&obj(vec![("ok", Value::Bool(true)), ("preset", p)])));
            return Ok(());
        }
        let parse = |raw: Value, default: Value| -> Value {
            match raw {
                Value::Null => default,
                Value::Str(_) => json::parse(&st(&raw))
                    .unwrap_or_else(|_| s(&format!("MALFORMED: {}", st(&raw)))),
                other => s(&format!("MALFORMED: {}", st(&other))),
            }
        };
        let flags = parse(dget_v(&p, "danger_flags")?, Value::Array(Vec::new()));
        let env = parse(dget_v(&p, "env")?, Value::Object(Vec::new()));
        let readiness = dget_v(&p, "readiness")?;
        let f = |k: &str| -> R<String> { Ok(st(&dget_v(&p, k)?)) };
        self.p.out(format!("id:        {}", f("id")?));
        self.p.out(format!("label:     {}", f("label")?));
        self.p.out(format!("command:   {}", f("command")?));
        let yes = if py::truthy(&dget_v(&p, "enabled")?) {
            "yes"
        } else {
            "no"
        };
        self.p.out(format!("enabled:   {yes}"));
        let src = if py::truthy(&dget_v(&p, "is_built_in")?) {
            "built-in"
        } else {
            "custom"
        };
        self.p.out(format!("source:    {src}"));
        if py::truthy(&dget_v(&p, "icon")?) {
            self.p.out(format!("icon:      {}", f("icon")?));
        }
        let r = or(readiness, s("(unknown \u{2014} default injection profile)"));
        self.p.out(format!("readiness: {}", st(&r)));
        let flags_text = match &flags {
            Value::Array(items) if !items.is_empty() => {
                let mut words = Vec::new();
                for (i, x) in items.iter().enumerate() {
                    match x {
                        Value::Str(_) => words.push(st(x)),
                        other => {
                            return Err(format!(
                                "TypeError: sequence item {i}: expected str instance, {} found",
                                py::type_name(other)
                            )
                            .into())
                        }
                    }
                }
                words.join(" ")
            }
            Value::Str(_) => st(&flags),
            _ => "(none declared \u{2014} API host sessions strip only the known floor)".into(),
        };
        self.p.out(format!("danger-flags: {flags_text}"));
        match &env {
            Value::Object(pairs) if !pairs.is_empty() => {
                self.p.out("env:");
                let mut keys: Vec<&(Text, Value)> = pairs.iter().collect();
                keys.sort_by(|a, b| a.0.cmp(&b.0));
                for (k, v) in keys {
                    self.p
                        .out(format!("  {}={}", st(&Value::Str(k.clone())), st(v)));
                }
            }
            Value::Str(_) => self.p.out(format!("env:       {}", st(&env))),
            _ => {}
        }
        let flow = or(
            dget_v(&p, "inject_flow")?,
            s("(default \u{2014} paste / return / return)"),
        );
        self.p.out(format!("inject_flow: {}", st(&flow)));
        Ok(())
    }

    /// `add` (`selector` `None`) and `set`.
    fn write(&mut self, selector: Option<String>, f: Fields, json: bool) -> R<()> {
        let mut body: Vec<(Text, Value)> = Vec::new();
        if !f.label.is_empty() {
            put(&mut body, "label", s(&f.label));
        }
        if !f.command.is_empty() {
            put(&mut body, "command", s(&f.command));
        }
        if !f.icon.is_empty() {
            put(&mut body, "icon", s(&f.icon));
        }
        if !f.danger.is_empty() {
            let v = Value::Array(f.danger.iter().map(|d| s(d)).collect());
            put(&mut body, "dangerFlags", s(&dumps(&v)));
        }
        if !f.env.is_empty() {
            let mut env: Vec<(Text, Value)> = Vec::new();
            for pair in &f.env {
                let Some((k, v)) = pair.split_once('=') else {
                    let hint = s(&format!("--env expects K=V, got {}", py::repr(&s(pair))));
                    return Err(self.die("usage", &hint));
                };
                if k.is_empty() {
                    let hint = s(&format!(
                        "--env expects a non-empty variable name, got {}",
                        py::repr(&s(pair))
                    ));
                    return Err(self.die("usage", &hint));
                }
                put(&mut env, k, s(v));
            }
            put(&mut body, "env", s(&dumps(&Value::Object(env))));
        }
        if !f.readiness.is_empty() {
            put(&mut body, "readiness", s(&f.readiness));
        }
        if !f.inject_flow.is_empty() {
            let Ok(flow) = json::parse(&f.inject_flow) else {
                let hint = s("--inject-flow must be a JSON array of {key, waitMs} steps");
                return Err(self.die("usage", &hint));
            };
            put(&mut body, "injectFlow", s(&dumps(&flow)));
        }
        let Some(selector) = selector else {
            put(&mut body, "id", s(&f.id));
            if !body.iter().any(|(k, _)| *k == Text::from("label")) {
                put(&mut body, "label", s(&f.id));
            }
            let d = self.call(Method::Post, CREATE, Some(&Value::Object(body)))?;
            if json {
                self.p
                    .out(dumps(&obj(vec![("ok", Value::Bool(true)), ("preset", d)])));
            } else {
                let id = st(&dget_v(&d, "id")?);
                let cmd = st(&dget_v(&d, "command")?);
                self.p.out(format!("Created preset '{id}' ({cmd})."));
                self.p.out(format!(
                    "Use it: {} agent hire <dir> --agent {id}   (or Settings \u{2192} Language Models \u{2192} Providers / Models \u{2192} Launch presets)",
                    cli_name!()
                ));
            }
            return Ok(());
        };
        let p = self.resolve(&selector)?;
        put(&mut body, "id", dget_v(&p, "id")?);
        if f.clear_danger {
            put(&mut body, "dangerFlags", s(""));
        }
        if f.clear_env {
            put(&mut body, "env", s(""));
        }
        if f.clear_ready {
            put(&mut body, "readiness", s(""));
        }
        if f.clear_inject {
            put(&mut body, "injectFlow", s(""));
        }
        if !f.enable.is_empty() {
            let n = if f.enable == "1" { "1" } else { "0" };
            put(&mut body, "enabled", Value::Int(n.into()));
        }
        if body.len() == 1 {
            let hint = s(&format!(
                "nothing to set \u{2014} pass at least one flag (see: {} preset set --help)",
                cli_name!()
            ));
            return Err(self.die("usage", &hint));
        }
        let d = self.call(Method::Post, UPDATE, Some(&Value::Object(body)))?;
        if json {
            self.p
                .out(dumps(&obj(vec![("ok", Value::Bool(true)), ("preset", d)])));
        } else {
            self.p
                .out(format!("Updated preset '{}'.", st(&dget_v(&d, "id")?)));
        }
        Ok(())
    }

    fn remove(&mut self, selector: &str, json: bool) -> R<()> {
        let p = self.resolve(selector)?;
        let id = dget_v(&p, "id")?;
        if py::truthy(&dget_v(&p, "is_built_in")?) {
            let hint = s(&format!(
                "'{}' is a built-in preset \u{2014} built-ins can't be deleted (disable it: {} preset set {} --disable; or Restore built-in presets in Settings \u{2192} Language Models \u{2192} Providers / Models)",
                st(&dget_v(&p, "label")?),
                cli_name!(),
                st(&id)
            ));
            return Err(self.die("refused", &hint));
        }
        self.call(Method::Post, DELETE, Some(&obj(vec![("id", id.clone())])))?;
        if json {
            self.p.out(dumps(&obj(vec![
                ("ok", Value::Bool(true)),
                ("removed", id),
            ])));
        } else {
            self.p.out(format!("Removed preset '{}'.", st(&id)));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forms() {
        assert_eq!(parse(&[]), Form::Help);
        assert_eq!(parse(&["list", "--json"]), Form::List { json: true });
        assert!(matches!(parse(&["list", "x"]), Form::Usage(_)));
        assert!(matches!(parse(&["add", "--id"]), Form::Usage(h) if h == "--id requires a value"));
        assert!(matches!(
            parse(&["set", "a", "--env", "K=V", "--clear-env"]),
            Form::Usage(_)
        ));
        let Form::Add { f, json } = parse(&[
            "add",
            "--id",
            "x",
            "--command",
            "x --y",
            "--danger-flag",
            "--yes",
            "--json",
        ]) else {
            panic!("not add");
        };
        assert!(json);
        assert_eq!(f.danger, vec!["--yes".to_string()]);
    }

    #[test]
    fn the_error_line() {
        assert_eq!(
            err_line("usage", &s("caf\u{e9}")),
            "{\"error\":{\"code\":\"usage\",\"hint\":\"caf\u{e9}\"}}"
        );
    }
}
