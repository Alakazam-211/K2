//! `k5 sim …`: the workspace's mobile device, as the shell CLI's `cmd_sim`
//! answers it (`cli/src/sim.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings, a re-minted passport taken at once, and the two gates
//!    ([`crate::prelude`]). `sim` alone, `help`, `-h` or `--help` second,
//!    or `-h` or `--help` before any bare `--` skip the connection gate;
//!    the virtual desktop's guard stops all but a line with `-h` or
//!    `--help` before any `--` in it.
//! 2. `sim` is an `id` tool: with `HOOK_SOCK` set, the stale-passport or
//!    require-passport lines, exit 3.
//! 3. `-h` or `--help` before any `--`, no verb or `help`: the help page on
//!    stdout. An unknown verb: its line and the help page on stderr,
//!    exit 2.
//! 4. `_sim_common`: `--json` and `--workspace` come out of the words
//!    ([`common`]), then the workspace ([`Run::target`]): a folder as
//!    `cd <it> && pwd` names it, else the daemon's
//!    `/cli/workspace/resolve` (`cli_request`, its errors unseen), else the
//!    word itself; with no `--workspace`, `PROJECT` outside a session with
//!    a passport, nothing inside one (the daemon takes the passport's).
//! 5. The verb's words ([`parse`]); a bad one is `_sim_die`: `Error:
//!    <hint>` (not with `--json`) and the JSON error object, on stderr,
//!    exit 2.
//! 6. `_sim_call`: `cli_send_json` (the cell socket first with a passport
//!    and a live socket, then TCP; a POST again over TCP only after a
//!    refused connect or "not served"). A failed call is status `000`; a
//!    POST that went out on the socket and got no answer is the
//!    transport's line and exit 1. A status that is not 2xx is the error
//!    reader ([`refusal`]): exit 1 for none or 5xx, 2 for 400 or held, 3
//!    otherwise.
//! 7. What the verb prints ([`Render`]), then the EXIT trap's lines.
//!
//! Hand-backs to the shell, decided before anything is printed or sent:
//! a word that is not UTF-8, `HOOK_SOCK` with a `CODEX_` variable, a port
//! curl won't take, a `python3` the binary can't tell; and three of
//! bash's own edges, where bash prints its own error line: a
//! `--workspace` folder `cd` reads as options it takes or one `CDPATH`
//! moves ([`crate::shellpath::cd_to_shell`]; one named as an option `cd`
//! doesn't take is `cd`'s usage error here), an install path whose folder
//! `CDPATH` would move, a duration with
//! other than ASCII in it (Python's `\d` and `float` take any digits), and
//! a number past bash's 64-bit range. An answer that is not UTF-8 is read
//! with U+FFFD for each bad byte, as [`super::inbox`].
//!
//! Where the binary differs on purpose (`sim-install-dash-path`, a native
//! refusal for input bash only half-handles): an install path starting with `-` reaches bash's `dirname`
//! and `basename` as their options, and the run stops with the system
//! tools' own usage lines (GNU's or BSD's words), exit 1. The binary
//! refuses it in its own words, exit 1 too. Neither sends anything.

use super::{curl_port, get, py_d, py_get, py_iter, py_left, py_space, raised, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::ops::{py_eq, py_in};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{Auth, Call, Channel, Client, Error};
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["sim"];
/// The binary answers it; [`run`] hands the edge cases back.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy.
pub const TOOL: &str = "sim";
pub const RESOLVE: &str = "/cli/workspace/resolve";

/// `_sim_die CODE HINT [RC]`: a refusal the CLI makes itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Die {
    pub code: &'static str,
    pub hint: String,
    pub exit: Exit,
    /// `_SIM_JSON` when it ran.
    pub json: bool,
}

pub fn die(code: &'static str, hint: impl Into<String>) -> Die {
    Die {
        code,
        hint: hint.into(),
        exit: Exit::Usage,
        json: false,
    }
}

fn usage(hint: impl Into<String>) -> Die {
    die("usage", hint)
}

impl Die {
    /// Its lines on stderr; its exit.
    pub fn print(&self) -> Exit {
        let mut err = String::new();
        if !self.json {
            err.push_str(&format!("Error: {}\n", self.hint));
        }
        let obj = Value::Object(vec![(
            Text::from("error"),
            Value::Object(vec![
                (Text::from("code"), s(self.code)),
                (Text::from("hint"), s(&self.hint)),
            ]),
        )]);
        err.push_str(&dumps(&obj));
        err.push('\n');
        Shown::err(err, self.exit).print()
    }
}

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).expect("a value from JSON or words always prints")
}

/// What `_sim_common` took out of the words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Common {
    pub json: bool,
    pub workspace: String,
    pub rest: Vec<String>,
}

/// `_sim_common`'s loop.
pub fn common(words: &[&str]) -> Result<Common, Die> {
    let mut c = Common {
        json: false,
        workspace: String::new(),
        rest: Vec::new(),
    };
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--json" => c.json = true,
            "--workspace" | "-w" => {
                let Some(v) = words.get(i + 1) else {
                    return Err(Die {
                        json: c.json,
                        ..usage("--workspace needs a path or name")
                    });
                };
                c.workspace = (*v).to_string();
                i += 2;
                continue;
            }
            _ => match w.strip_prefix("--workspace=") {
                Some(v) => c.workspace = v.into(),
                None => c.rest.push(w.to_string()),
            },
        }
        i += 1;
    }
    Ok(c)
}

/// One of `_sim_body`'s `kind:key=value` words.
#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    /// `s:`: a string, left out when empty.
    S(&'static str, String),
    /// `b:`: `true`, left out when false.
    B(&'static str, bool),
    /// `n:`: digits, a float when they have a `.`.
    N(&'static str, String),
    /// `j:`: a value.
    J(&'static str, Value),
}

/// `_sim_body`: the workspace (when set), then each field, a dotted key
/// nested.
pub fn body(ws: &str, fields: &[Field]) -> String {
    let mut d: Vec<(Text, Value)> = Vec::new();
    if !ws.is_empty() {
        d.push((Text::from("workspace"), s(ws)));
    }
    for f in fields {
        let (key, v) = match f {
            Field::S(_, v) if v.is_empty() => continue,
            Field::B(_, false) => continue,
            Field::S(k, v) => (*k, s(v)),
            Field::B(k, true) => (*k, Value::Bool(true)),
            Field::N(k, v) => (*k, number(v)),
            Field::J(k, v) => (*k, v.clone()),
        };
        put(&mut d, key, v);
    }
    dumps(&Value::Object(d))
}

/// `float(val) if "." in val else int(val)`, on the shell's checked
/// digits (`-?[0-9]+(\.[0-9]+)?`).
fn number(v: &str) -> Value {
    if v.contains('.') {
        return Value::Float(v.parse().expect("checked digits are a float"));
    }
    let (neg, digits) = match v.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, v),
    };
    let digits = digits.trim_start_matches('0');
    match (neg, digits) {
        (_, "") => Value::Int("0".into()),
        (true, d) => Value::Int(format!("-{d}")),
        (false, d) => Value::Int(d.into()),
    }
}

/// `put(key, v)`: `d.setdefault` down the dotted path, then the last key
/// set (a key that is there keeps its place).
fn put(d: &mut Vec<(Text, Value)>, key: &str, v: Value) {
    let mut parts: Vec<&str> = key.split('.').collect();
    let last = parts.pop().expect("split gives one part at least");
    let mut t = d;
    for k in parts {
        let k = Text::from(k);
        let at = match t.iter().position(|(x, _)| *x == k) {
            Some(at) => at,
            None => {
                t.push((k, Value::Object(Vec::new())));
                t.len() - 1
            }
        };
        let Value::Object(inner) = &mut t[at].1 else {
            unreachable!("only the body's own objects are nested into");
        };
        t = inner;
    }
    let last = Text::from(last);
    match t.iter_mut().find(|(x, _)| *x == last) {
        Some((_, slot)) => *slot = v,
        None => t.push((last, v)),
    }
}

/// `_sim_query`: `workspace=` (when set), then each pair that is not
/// empty, every value escaped.
pub fn query(ws: &str, pairs: &[(&str, String)]) -> String {
    let mut out: Vec<String> = Vec::new();
    if !ws.is_empty() {
        out.push(format!("workspace={}", quote(ws)));
    }
    for (k, v) in pairs {
        if !v.is_empty() {
            out.push(format!("{k}={}", quote(v)));
        }
    }
    out.join("&")
}

/// `_sim_ms`: a duration in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ms {
    Value(i64),
    /// Python's regex or `float` refuses it: `return 1`.
    Bad,
    /// Bash's own error, or Python's traceback, would come out: the shell
    /// answers.
    Shell,
}

/// `_sim_ms`'s Python: `\s*(\d+(?:\.\d+)?)\s*(ms|s|m|h)?\s*`, then
/// `int(float(n) * unit)`.
pub fn ms(text: &str) -> Ms {
    if !text.is_ascii() {
        return Ms::Shell;
    }
    let t = text.trim_matches(py_space);
    let digits_end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    if digits_end == 0 {
        return Ms::Bad;
    }
    let mut end = digits_end;
    let rest = &t[digits_end..];
    if let Some(frac) = rest.strip_prefix('.') {
        let n = frac
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(frac.len());
        if n > 0 {
            end += 1 + n;
        }
    }
    let number = &t[..end];
    let unit = t[end..].trim_start_matches(py_space);
    let mult: f64 = match unit {
        "" | "s" => 1000.0,
        "ms" => 1.0,
        "m" => 60_000.0,
        "h" => 3_600_000.0,
        _ => return Ms::Bad,
    };
    let n: f64 = number.parse().expect("checked digits are a float");
    let v = n * mult;
    if !v.is_finite() {
        return Ms::Shell;
    }
    let v = v.trunc();
    if v >= 9_223_372_036_854_775_808.0 {
        return Ms::Shell;
    }
    Ms::Value(v as i64)
}

/// `_sim_is_id`: `^(t[0-9]+:)?e[0-9]+$`.
pub fn is_id(w: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let rest = match w.split_once(':') {
        Some((t, rest)) => {
            if !t.strip_prefix('t').is_some_and(digits) {
                return false;
            }
            rest
        }
        None => w,
    };
    rest.strip_prefix('e').is_some_and(digits)
}

/// `^-?[0-9]+(\.[0-9]+)?$` (`neg`: the `-?`).
fn decimal(w: &str, neg: bool) -> bool {
    let w = if neg {
        w.strip_prefix('-').unwrap_or(w)
    } else {
        w
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match w.split_once('.') {
        Some((a, b)) => digits(a) && digits(b),
        None => digits(w),
    }
}

/// `x,y` with each side [`decimal`]; the two sides as `${v%,*}` and
/// `${v#*,}` cut them.
fn pair(w: &str, neg: bool) -> Option<(String, String)> {
    let (a, b) = w.split_once(',')?;
    (decimal(a, neg) && decimal(b, neg)).then(|| (a.to_string(), b.to_string()))
}

/// What a verb prints of a 2xx answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Render {
    /// `_sim_print <field> <fallback>`.
    Field(&'static str, String),
    Status,
    Devices,
    Boot,
    Logs,
}

/// The request a verb's words make.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    Get {
        route: &'static str,
        pairs: Vec<(&'static str, String)>,
        render: Render,
    },
    Post {
        route: String,
        fields: Vec<Field>,
        max_time: Option<u64>,
        render: Render,
        /// `erase` without `--yes`: ask on the terminal first.
        confirm: bool,
    },
}

fn post(route: impl Into<String>, fields: Vec<Field>, render: Render) -> Plan {
    Plan::Post {
        route: route.into(),
        fields,
        max_time: None,
        render,
        confirm: false,
    }
}

fn line(fallback: impl Into<String>) -> Render {
    Render::Field("line", fallback.into())
}

fn act(fields: Vec<Field>) -> Plan {
    post("/cli/sim/act", fields, line("ok"))
}

/// What a verb's words come to: a request, the shell, or a refusal.
#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Plan(Plan),
    Shell,
    Die(Die),
}

impl From<Die> for Parsed {
    fn from(d: Die) -> Self {
        Parsed::Die(d)
    }
}

/// A value flag's word after it: `Err` is `_sim_die usage <hint>`.
fn value<'a>(rest: &'a [String], i: usize, hint: &str) -> Result<&'a str, Die> {
    rest.get(i + 1)
        .map(String::as_str)
        .ok_or_else(|| usage(hint))
}

fn name() -> &'static str {
    cli_name!()
}

/// `${choices// /|}` and `${choices// /, }`.
fn choices(list: &[&str], sep: &str) -> String {
    list.join(sep)
}

const KEYS: &[&str] = &[
    "enter",
    "backspace",
    "delete",
    "tab",
    "escape",
    "arrow-up",
    "arrow-down",
    "arrow-left",
    "arrow-right",
];
const BUTTONS: &[&str] = &[
    "home",
    "back",
    "lock",
    "volume-up",
    "volume-down",
    "app-switch",
    "dev-menu",
];

/// The verb's words after `_sim_common` (`rest`), as its function reads
/// them. `exists`: `[ -e <path> ]`; `cd_moved`: `CDPATH` moves `cd <dir>`
/// ([`crate::shellpath::cd_unmoved`]).
pub fn parse(
    sub: &str,
    rest: &[String],
    exists: &dyn Fn(&str) -> bool,
    cd_moved: &dyn Fn(&str) -> bool,
) -> Parsed {
    match parse_verb(sub, rest, exists, cd_moved) {
        Ok(p) => p,
        Err(d) => Parsed::Die(d),
    }
}

fn no_args(verb: &str, rest: &[String]) -> Result<(), Die> {
    if rest.is_empty() {
        Ok(())
    } else {
        Err(usage(format!(
            "{} sim {verb} takes no arguments: {}",
            name(),
            rest.join(" ")
        )))
    }
}

fn platform_ok(p: &str) -> Result<(), Die> {
    match p {
        "" | "android" | "ios" => Ok(()),
        _ => Err(usage(format!("--platform is android or ios, not {p}"))),
    }
}

fn parse_verb(
    sub: &str,
    rest: &[String],
    exists: &dyn Fn(&str) -> bool,
    cd_moved: &dyn Fn(&str) -> bool,
) -> Result<Parsed, Die> {
    let n = name();
    let plan = match sub {
        "status" => {
            no_args("status", rest)?;
            Plan::Get {
                route: "/cli/sim/status",
                pairs: Vec::new(),
                render: Render::Status,
            }
        }
        "list-devices" | "devices" => {
            let mut platform = String::new();
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i].as_str();
                if w == "--platform" {
                    platform = value(rest, i, "--platform needs android or ios")?.into();
                    i += 2;
                    continue;
                } else if let Some(v) = w.strip_prefix("--platform=") {
                    platform = v.into();
                } else {
                    return Err(usage(format!(
                        "unknown argument for {n} sim list-devices: {w}"
                    )));
                }
                i += 1;
            }
            platform_ok(&platform)?;
            Plan::Get {
                route: "/cli/sim/devices",
                pairs: vec![("platform", platform)],
                render: Render::Devices,
            }
        }
        "boot" | "start" => {
            let (mut platform, mut device, mut notab) = (String::new(), String::new(), false);
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i].as_str();
                if let Some(v) = w.strip_prefix("--platform=") {
                    platform = v.into();
                    i += 1;
                    continue;
                }
                if let Some(v) = w.strip_prefix("--device=") {
                    device = v.into();
                    i += 1;
                    continue;
                }
                match w {
                    "--platform" => {
                        platform = value(rest, i, "--platform needs android or ios")?.into();
                        i += 2;
                        continue;
                    }
                    "--device" => {
                        device = value(
                            rest,
                            i,
                            &format!("--device needs a profile ({n} sim list-devices)"),
                        )?
                        .into();
                        i += 2;
                        continue;
                    }
                    "--no-tab" => notab = true,
                    _ => return Err(usage(format!("unknown argument for {n} sim boot: {w}"))),
                }
                i += 1;
            }
            platform_ok(&platform)?;
            Plan::Post {
                route: "/cli/sim/boot".into(),
                fields: vec![
                    Field::S("platform", platform),
                    Field::S("device", device),
                    Field::B("noTab", notab),
                ],
                max_time: Some(330),
                render: Render::Boot,
                confirm: false,
            }
        }
        "stop" => {
            no_args("stop", rest)?;
            post(
                "/cli/sim/stop",
                Vec::new(),
                line(format!(
                    "Stopped. Its data is kept; {n} sim boot starts it again."
                )),
            )
        }
        "erase" => {
            let mut yes = false;
            for a in rest {
                match a.as_str() {
                    "--yes" | "-y" => yes = true,
                    _ => return Err(usage(format!("unknown argument for {n} sim erase: {a}"))),
                }
            }
            Plan::Post {
                route: "/cli/sim/erase".into(),
                fields: Vec::new(),
                max_time: None,
                render: line(format!(
                    "Erased. The next {n} sim boot starts a fresh device."
                )),
                confirm: !yes,
            }
        }
        "install" => {
            let [p] = rest else {
                return Err(usage(format!("usage: {n} sim install <app.apk|App.app>")));
            };
            if !exists(p.as_str()) {
                return Err(die(
                    "not_found",
                    format!("no such file: {p} (build the app in your terminal first)"),
                ));
            }
            let dir = dirname(p);
            // `dirname` and `basename` would read the path as their
            // options and fail in the system's own words: refused here
            // instead (the named difference sim-install-dash-path).
            if p.starts_with('-') || dir.starts_with('-') {
                // Exit 1, as bash stops there.
                return Err(Die {
                    exit: Exit::Error,
                    ..usage(format!(
                        "{n} sim install: a path starting with \"-\" can't be read safely; use ./{p} instead"
                    ))
                });
            }
            if cd_moved(&dir) {
                return Ok(Parsed::Shell);
            }
            // The absolute path is made once the plan runs (`cd`).
            Plan::Post {
                route: "/cli/sim/install".into(),
                fields: vec![Field::S("path", p.clone())],
                max_time: Some(330),
                render: line(String::new()),
                confirm: false,
            }
        }
        "launch" | "terminate" | "kill" => {
            let verb = if sub == "launch" {
                "launch"
            } else {
                "terminate"
            };
            let [app] = rest else {
                return Err(usage(format!("usage: {n} sim {verb} <package|bundleId>")));
            };
            post(
                format!("/cli/sim/{verb}"),
                vec![Field::S("app", app.clone())],
                line(format!("ok {verb} {app}")),
            )
        }
        "tree" => {
            let (mut all, mut find, mut boxes) = ("", String::new(), "");
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i].as_str();
                if let Some(v) = w.strip_prefix("--find=") {
                    find = v.into();
                    i += 1;
                    continue;
                }
                match w {
                    "--all" => all = "1",
                    "--boxes" => boxes = "1",
                    "--find" => {
                        find = value(rest, i, "--find needs text")?.into();
                        i += 2;
                        continue;
                    }
                    _ => return Err(usage(format!("unknown argument for {n} sim tree: {w}"))),
                }
                i += 1;
            }
            Plan::Get {
                route: "/cli/sim/tree",
                pairs: vec![("all", all.into()), ("find", find), ("boxes", boxes.into())],
                render: Render::Field("text", "(the daemon sent no tree)".into()),
            }
        }
        "tap" => {
            let (mut el, mut at) = (String::new(), String::new());
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i].as_str();
                if w == "--at" {
                    at = value(rest, i, "--at needs x,y (dp)")?.into();
                    i += 2;
                    continue;
                } else if let Some(v) = w.strip_prefix("--at=") {
                    at = v.into();
                } else if w.starts_with('-') {
                    return Err(usage(format!("unknown flag for {n} sim tap: {w}")));
                } else {
                    if !el.is_empty() {
                        return Err(usage(format!("{n} sim tap takes one element")));
                    }
                    el = w.into();
                }
                i += 1;
            }
            if !at.is_empty() {
                if !el.is_empty() {
                    return Err(usage(format!(
                        "{n} sim tap takes an element OR --at x,y, not both"
                    )));
                }
                let Some((x, y)) = pair(&at, false) else {
                    return Err(usage("--at is x,y in dp, like 120,300"));
                };
                act(vec![
                    Field::S("action.verb", "tap".into()),
                    Field::N("action.x", x),
                    Field::N("action.y", y),
                ])
            } else {
                if el.is_empty() {
                    return Err(usage(format!(
                        "usage: {n} sim tap e5 | {n} sim tap --at 120,300"
                    )));
                }
                if !is_id(&el) {
                    return Err(die(
                        "bad_element_id",
                        format!(
                            "\"{el}\" is not an element id (use one from {n} sim tree, like e5)"
                        ),
                    ));
                }
                act(vec![
                    Field::S("action.verb", "tap".into()),
                    Field::S("action.element", el),
                ])
            }
        }
        "type" => {
            let (mut into, mut secret, mut text) = (String::new(), String::new(), String::new());
            let mut have_text = false;
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i].as_str();
                if let Some(v) = w.strip_prefix("--into=") {
                    into = v.into();
                    i += 1;
                    continue;
                }
                if let Some(v) = w.strip_prefix("--secret=") {
                    secret = v.into();
                    i += 1;
                    continue;
                }
                match w {
                    "--into" => {
                        into = value(rest, i, "--into needs an element id")?.into();
                        i += 2;
                        continue;
                    }
                    "--secret" => {
                        secret = value(rest, i, "--secret needs a vault name")?.into();
                        i += 2;
                        continue;
                    }
                    "--" => {
                        if let Some(t) = rest.get(i + 1) {
                            text = t.clone();
                            have_text = true;
                            i += 2;
                        } else {
                            i += 1;
                        }
                        continue;
                    }
                    _ if w.starts_with('-') => {
                        return Err(usage(format!("unknown flag for {n} sim type: {w}")))
                    }
                    _ => {
                        if have_text {
                            return Err(usage(format!("{n} sim type takes one text (quote it)")));
                        }
                        text = w.into();
                        have_text = true;
                    }
                }
                i += 1;
            }
            if !into.is_empty() && !is_id(&into) {
                return Err(die(
                    "bad_element_id",
                    format!("\"{into}\" is not an element id (use one from {n} sim tree, like e3)"),
                ));
            }
            if !secret.is_empty() {
                if have_text {
                    return Err(usage(format!(
                        "{n} sim type takes text OR --secret <vault-name>, not both"
                    )));
                }
                act(vec![
                    Field::S("action.verb", "type".into()),
                    Field::S("action.into", into),
                    Field::S("action.secret", secret),
                ])
            } else {
                if !have_text {
                    return Err(usage(format!(
                        "usage: {n} sim type [--into e3] \"text\" | --secret <vault-name>"
                    )));
                }
                act(vec![
                    Field::S("action.verb", "type".into()),
                    Field::S("action.into", into),
                    Field::J("action.text", s(&text)),
                ])
            }
        }
        "swipe" => {
            let (mut dir, mut from) = (String::new(), String::new());
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i].as_str();
                if let Some(v) = w.strip_prefix("--from=") {
                    from = v.into();
                    i += 1;
                    continue;
                }
                match w {
                    "--from" => {
                        from = value(rest, i, "--from needs an element id")?.into();
                        i += 2;
                        continue;
                    }
                    "up" | "down" | "left" | "right" => {
                        if !dir.is_empty() {
                            return Err(usage("one direction"));
                        }
                        dir = w.into();
                    }
                    _ => {
                        return Err(usage(format!(
                            "{n} sim swipe up|down|left|right [--from e7] (not: {w})"
                        )))
                    }
                }
                i += 1;
            }
            if dir.is_empty() {
                return Err(usage(format!(
                    "usage: {n} sim swipe up|down|left|right [--from e7]"
                )));
            }
            if !from.is_empty() && !is_id(&from) {
                return Err(die(
                    "bad_element_id",
                    format!("\"{from}\" is not an element id"),
                ));
            }
            act(vec![
                Field::S("action.verb", "swipe".into()),
                Field::S("action.direction", dir),
                Field::S("action.from", from),
            ])
        }
        "key" => choice("key", "action.key", KEYS, rest)?,
        "button" => choice("button", "action.button", BUTTONS, rest)?,
        "rotate" => choice("rotate", "action.turn", &["left", "right"], rest)?,
        "appearance" => choice("appearance", "action.appearance", &["dark", "light"], rest)?,
        "open-url" | "open" => {
            let [u] = rest else {
                return Err(usage(format!("usage: {n} sim open-url <url or deep link>")));
            };
            if !u.contains(':') {
                return Err(usage(
                    "open-url needs a URL with a scheme (https://…, myapp://…)",
                ));
            }
            act(vec![
                Field::S("action.verb", "open-url".into()),
                Field::S("action.url", u.clone()),
            ])
        }
        "location" => {
            let [ll] = rest else {
                return Err(usage(format!("usage: {n} sim location <lat,lon>")));
            };
            let Some((lat, lon)) = pair(ll, true) else {
                return Err(usage("location is lat,lon, like 37.33,-122.03"));
            };
            act(vec![
                Field::S("action.verb", "location".into()),
                Field::N("action.lat", lat),
                Field::N("action.lon", lon),
            ])
        }
        "screenshot" | "shot" => {
            let mut region = String::new();
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i].as_str();
                if w == "--region" {
                    region = value(rest, i, "--region needs an element id")?.into();
                    i += 2;
                    continue;
                } else if let Some(v) = w.strip_prefix("--region=") {
                    region = v.into();
                } else {
                    return Err(usage(format!(
                        "unknown argument for {n} sim screenshot: {w}"
                    )));
                }
                i += 1;
            }
            if !region.is_empty() && !is_id(&region) {
                return Err(die(
                    "bad_element_id",
                    format!("\"{region}\" is not an element id"),
                ));
            }
            post(
                "/cli/sim/screenshot",
                vec![Field::S("region", region)],
                Render::Field("path", "(the daemon sent no path)".into()),
            )
        }
        "logs" | "log" => {
            let (mut app, mut since, mut grep, mut lines) =
                (String::new(), String::new(), String::new(), String::new());
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i].as_str();
                let (slot, hint) = match w {
                    "--app" => (&mut app, "--app needs a package or bundle id"),
                    "--since" => (&mut since, "--since needs a duration (2m)"),
                    "--grep" => (&mut grep, "--grep needs text"),
                    "--lines" => (&mut lines, "--lines needs a number"),
                    _ => return Err(usage(format!("unknown argument for {n} sim logs: {w}"))),
                };
                *slot = value(rest, i, hint)?.into();
                i += 2;
            }
            let mut secs = String::new();
            if !since.is_empty() {
                match ms(&since) {
                    Ms::Value(m) => secs = (m.wrapping_add(999) / 1000).to_string(),
                    Ms::Bad => return Err(usage("--since is a duration like 30s, 2m or 1h")),
                    Ms::Shell => return Ok(Parsed::Shell),
                }
            }
            if !lines.is_empty() {
                let digits = lines.bytes().all(|b| b.is_ascii_digit());
                if digits && lines.parse::<i64>().is_err() {
                    return Ok(Parsed::Shell);
                }
                if !digits || lines.parse::<i64>().is_ok_and(|l| l < 1) {
                    return Err(usage("--lines is a positive number"));
                }
            }
            Plan::Get {
                route: "/cli/sim/logs",
                pairs: vec![
                    ("app", app),
                    ("sinceSecs", secs),
                    ("grep", grep),
                    ("maxLines", lines),
                ],
                render: Render::Logs,
            }
        }
        "wait" => {
            let (mut for_, mut timeout) = (String::new(), "10s".to_string());
            let mut i = 0;
            while i < rest.len() {
                let w = rest[i].as_str();
                if let Some(v) = w.strip_prefix("--for=") {
                    for_ = v.into();
                    i += 1;
                    continue;
                }
                if let Some(v) = w.strip_prefix("--timeout=") {
                    timeout = v.into();
                    i += 1;
                    continue;
                }
                match w {
                    "--for" => {
                        for_ = value(rest, i, "--for needs text")?.into();
                        i += 2;
                        continue;
                    }
                    "--timeout" => {
                        timeout = value(rest, i, "--timeout needs a duration (10s)")?.into();
                        i += 2;
                        continue;
                    }
                    _ => return Err(usage(format!("unknown argument for {n} sim wait: {w}"))),
                }
            }
            let m = match ms(&timeout) {
                Ms::Value(m) => m,
                Ms::Bad => return Err(usage("--timeout is a duration like 10s or 2m")),
                Ms::Shell => return Ok(Parsed::Shell),
            };
            if m > 120_000 {
                return Err(usage("--timeout is at most 2m (wait again if you must)"));
            }
            Plan::Post {
                route: "/cli/sim/wait".into(),
                fields: vec![Field::S("for", for_), Field::N("timeoutMs", m.to_string())],
                max_time: Some(u64::try_from(m / 1000 + 30).expect("at most 150 s")),
                render: line("ok wait"),
                confirm: false,
            }
        }
        _ => unreachable!("cmd_sim's verbs only"),
    };
    Ok(Parsed::Plan(plan))
}

fn choice(verb: &str, key: &'static str, list: &[&str], rest: &[String]) -> Result<Plan, Die> {
    let n = name();
    let [v] = rest else {
        return Err(usage(format!(
            "usage: {n} sim {verb} {}",
            choices(list, "|")
        )));
    };
    if !list.contains(&v.as_str()) {
        return Err(usage(format!(
            "{n} sim {verb} takes {} (not {v})",
            choices(list, ", ")
        )));
    }
    Ok(act(vec![
        Field::S("action.verb", verb.to_string()),
        Field::S(key, v.clone()),
    ]))
}

/// The verbs `cmd_sim` knows.
pub const SUBCOMMANDS: &[&str] = &[
    "status",
    "list-devices",
    "devices",
    "boot",
    "start",
    "stop",
    "erase",
    "install",
    "launch",
    "terminate",
    "kill",
    "tree",
    "tap",
    "type",
    "swipe",
    "key",
    "button",
    "rotate",
    "appearance",
    "open-url",
    "open",
    "location",
    "screenshot",
    "shot",
    "logs",
    "log",
    "wait",
];

/// POSIX `dirname`.
pub fn dirname(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.is_empty() { ".".into() } else { "/".into() };
    }
    match t.rfind('/') {
        None => ".".into(),
        Some(at) => {
            let d = t[..at].trim_end_matches('/');
            if d.is_empty() {
                "/".into()
            } else {
                d.into()
            }
        }
    }
}

/// POSIX `basename`.
pub fn basename(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.is_empty() {
            String::new()
        } else {
            "/".into()
        };
    }
    t.rsplit('/').next().unwrap_or(t).to_string()
}

/// Whether `CDPATH` moves `cd <dir>`: an entry has it
/// ([`crate::shellpath::cdpath`]; a `CDPATH` the binary can't read counts
/// too). Where it doesn't, `cd` goes as with no `CDPATH`.
pub fn cd_moved(dir: &str) -> bool {
    !crate::shellpath::cd_unmoved(dir)
}

/// `cd <dir> && pwd` from the logical folder `pwd`: the path by its words
/// (`.` gone, `..` taken back, each `..` only from a folder), else the
/// folder as the system names it.
#[cfg(not(unix))]
pub fn cd_pwd(pwd: &Path, dir: &str) -> Option<String> {
    crate::out::pylib::path::cd_pwd(pwd, dir)
}

/// [`cd_pwd`] on Unix.
#[cfg(unix)]
pub fn cd_pwd(pwd: &Path, dir: &str) -> Option<String> {
    let full = if dir.starts_with('/') {
        dir.to_string()
    } else {
        format!("{}/{dir}", pwd.to_string_lossy())
    };
    let mut parts: Vec<&str> = Vec::new();
    let mut ok = true;
    for c in full.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                let cur = format!("/{}", parts.join("/"));
                if !Path::new(&cur).is_dir() {
                    ok = false;
                    break;
                }
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let canon = format!("/{}", parts.join("/"));
    if ok && Path::new(&canon).is_dir() {
        return Some(canon);
    }
    std::fs::canonicalize(&full)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------
// The readers.

/// What Python reads of the answer on stdin.
fn text_of(body: &[u8]) -> String {
    String::from_utf8_lossy(body).into_owned()
}

/// `_sim_print <field> <fallback>` without `--json`.
pub fn print_field(body: &[u8], field: &str, fallback: &str) -> Shown {
    let raw = text_of(body);
    let raw = if raw.is_empty() {
        "{}".to_string()
    } else {
        raw
    };
    let v = json::parse(&raw).unwrap_or(Value::Object(Vec::new()));
    let out = match &v {
        Value::Object(p) => get(p, field),
        _ => None,
    };
    match out {
        Some(Value::Str(t)) if !t.code_points().is_empty() => {
            let s = py::str_of(&Value::Str(t.clone()));
            Shown::out(if s.ends_with('\n') { s } else { s + "\n" })
        }
        _ => Shown::out(format!("{fallback}\n")),
    }
}

/// `json.load(sys.stdin)` at line 3 of a reader.
fn load(body: &[u8]) -> Result<Value, Raise> {
    crate::out::pylib::json::loads(&text_of(body)).map_err(|e| Raise::at(3, e.last_line()))
}

/// The output a reader printed before it raised, if it did.
#[derive(Default)]
struct Lines {
    out: String,
    err: String,
}

impl Lines {
    fn p(&mut self, line: impl AsRef<str>) {
        self.out.push_str(line.as_ref());
        self.out.push('\n');
    }

    fn end(self, r: Result<(), Raise>) -> Shown {
        match r {
            Ok(()) => Shown {
                stdout: self.out,
                stderr: self.err,
                exit: Exit::Ok,
            },
            Err(e) => raised(self.out, self.err, &e),
        }
    }
}

/// `x or default` on what `.get` gave.
fn or_default(x: Option<&Value>, default: Value) -> Value {
    match x {
        Some(v) if py::truthy(v) => v.clone(),
        _ => default,
    }
}

/// `d.get(key, default)` on a dict.
fn get_or(v: &Value, key: &str, default: &str, line: u32) -> Result<String, Raise> {
    Ok(py_get(v, key, line)?.map_or_else(|| default.to_string(), py::str_of))
}

/// `"%s" % d.get(key)`.
fn get_s(v: &Value, key: &str, line: u32) -> Result<String, Raise> {
    Ok(py_get(v, key, line)?.map_or_else(|| "None".to_string(), py::str_of))
}

fn truthy_get(v: &Value, key: &str, line: u32) -> Result<bool, Raise> {
    Ok(py_get(v, key, line)?.is_some_and(py::truthy))
}

/// `": " + x` (or any `str + x`): `x` must be a string.
fn concat(x: &Value, line: u32) -> Result<String, Raise> {
    match x {
        Value::Str(_) => Ok(py::str_of(x)),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

/// `d[key]` on a dict that has it.
fn index<'a>(v: &'a Value, key: &str) -> &'a Value {
    match v {
        Value::Object(p) => get(p, key).expect("checked with .get first"),
        _ => unreachable!("a dict, checked with .get first"),
    }
}

/// `_sim_status`'s reader.
pub fn render_status(body: &[u8]) -> Shown {
    let mut o = Lines::default();
    let r = status_lines(body, &mut o);
    o.end(r)
}

fn status_lines(body: &[u8], o: &mut Lines) -> Result<(), Raise> {
    let n = name();
    let v = load(body)?;
    let lanes = or_default(py_get(&v, "lanes", 4)?, Value::Array(Vec::new()));
    let lanes = py_iter(&lanes, 5)?;
    for l in &lanes {
        let setup = or_default(py_get(l, "setup", 6)?, Value::Object(Vec::new()));
        let word = if truthy_get(l, "ready", 7)? {
            "ready"
        } else if truthy_get(&setup, "running", 7)? {
            "setting up"
        } else if truthy_get(l, "usable", 7)? {
            "needs a step"
        } else {
            "not available"
        };
        let mut line = format!(
            "{} {word}",
            py_left(&py_get(l, "lane", 8)?.cloned().unwrap_or_else(|| s("?")), 9)
        );
        if truthy_get(l, "detail", 9)? {
            line += &format!(" ({})", py::str_of(index(l, "detail")));
        }
        o.p(format!("lane {line}"));
        let needs = or_default(py_get(l, "needs", 12)?, Value::Array(Vec::new()));
        let needs = py_iter(&needs, 13)?;
        let mut licences = 0usize;
        for need in &needs {
            if py_get(need, "kind", 13)?.is_some_and(|k| py_eq(k, &s("licence"))) {
                licences += 1;
            }
        }
        let accepted = or_default(
            py_get(&setup, "licencesAccepted", 14)?,
            Value::Int("0".into()),
        );
        for need in &needs {
            let k = py_get(need, "kind", 16)?.cloned().unwrap_or(Value::Null);
            if py_eq(&k, &s("licence")) {
                let id = py_get(need, "id", 18)?.cloned();
                let title = match py_get(need, "title", 18)? {
                    Some(t) if py::truthy(t) => t.clone(),
                    _ => id.clone().unwrap_or(Value::Null),
                };
                let lid = or_default(id.as_ref(), s(""));
                let tail = if py_in(&lid, &title, 19)? {
                    String::new()
                } else {
                    format!(" [{}]", py::str_of(&lid))
                };
                o.p(format!(
                    "  the owner must accept the {}{tail} (a card; never you)",
                    py::str_of(&title)
                ));
            } else if py_eq(&k, &s("install")) {
                if !truthy_get(&setup, "error", 21)? {
                    o.p(format!(
                        "  first use downloads {}",
                        get_s(need, "what", 22)?
                    ));
                }
            } else if py_eq(&k, &s("owner-step")) {
                o.p(format!(
                    "  owner step: {}: {}",
                    get_s(need, "what", 24)?,
                    get_s(need, "command", 24)?
                ));
            }
        }
        if licences > 1 {
            o.p(format!(
                "  one card covers all {licences} licences; the owner accepts them together"
            ));
        }
        if truthy_get(&setup, "running", 27)? {
            if py::truthy(&accepted) {
                let what = if py_eq(&accepted, &Value::Int("1".into())) {
                    "the licence".to_string()
                } else {
                    format!("all {} licences", py_d(&accepted, 29)?)
                };
                o.p(format!(
                    "  the owner accepted {what}; nothing left to accept"
                ));
            }
            o.p(format!(
                "  installing now; devices can boot when it finishes. {n} sim status again to follow it"
            ));
        } else if truthy_get(&setup, "error", 31)? && licences == 0 {
            let kept = if py::truthy(&accepted) {
                "; the licences stay accepted"
            } else {
                ""
            };
            o.p(format!(
                "  setup failed{kept}; the owner of this server runs it again (Retry setup on the Mobile tab), never you"
            ));
        }
    }
    let sim = py_get(&v, "sim", 33)?.cloned().unwrap_or(Value::Null);
    let stopped = !py::truthy(&sim) || {
        let st = py_get(&sim, "state", 34)?.cloned().unwrap_or(Value::Null);
        py_eq(&st, &s("stopped")) || py_eq(&st, &s("failed"))
    };
    if stopped {
        let mut ready = false;
        for l in &lanes {
            if truthy_get(l, "ready", 35)? {
                ready = true;
                break;
            }
        }
        let mut tail = String::new();
        if py::truthy(&sim) && py_get(&sim, "state", 37)?.is_some_and(|st| py_eq(st, &s("failed")))
        {
            let reason = if truthy_get(&sim, "reason", 38)? {
                format!(": {}", concat(index(&sim, "reason"), 38)?)
            } else {
                String::new()
            };
            tail = format!(" (last: failed{reason})");
        }
        let boot = if ready {
            format!("; {n} sim boot")
        } else {
            String::new()
        };
        o.p(format!("device: none running{tail}{boot}"));
    } else {
        let p = or_default(py_get(&sim, "profile", 41)?, Value::Object(Vec::new()));
        let size = if truthy_get(&p, "widthPt", 42)? {
            format!(
                " · {}x{} pt",
                get_s(&p, "widthPt", 42)?,
                get_s(&p, "heightPt", 42)?
            )
        } else {
            String::new()
        };
        let app = if truthy_get(&sim, "appId", 43)? {
            format!(" · {}", concat(index(&sim, "appId"), 43)?)
        } else {
            String::new()
        };
        o.p(format!(
            "device: sim {} · {} · {} · {}{size}{app}",
            get_s(&sim, "sim", 44)?,
            get_s(&sim, "lane", 44)?,
            get_or(&p, "name", "?", 44)?,
            get_s(&sim, "state", 44)?
        ));
        let h = or_default(py_get(&sim, "hold", 45)?, Value::Object(Vec::new()));
        let kind = py_get(&h, "kind", 46)?.cloned().unwrap_or(Value::Null);
        let by = |h: &Value| -> String {
            let b = or_default(py_get(h, "by", 47).ok().flatten(), s("a person"));
            py::str_of(&b)
        };
        if py_eq(&kind, &s("person")) {
            o.p(format!("held: {} is using the device", by(&h)));
        } else if py_eq(&kind, &s("takeover")) {
            o.p(format!("held: {} has taken over (until Hand back)", by(&h)));
        }
    }
    let allowed = if truthy_get(&v, "agentsAllowed", 50)? {
        "allowed (Allow mobile devices is on)"
    } else {
        "not allowed (Allow mobile devices is off)"
    };
    o.p(format!("agents: {allowed}"));
    Ok(())
}

/// `_sim_list_devices`'s reader.
pub fn render_devices(body: &[u8]) -> Shown {
    let mut o = Lines::default();
    let r = (|| -> Result<(), Raise> {
        let v = load(body)?;
        let ds = or_default(py_get(&v, "devices", 4)?, Value::Array(Vec::new()));
        if !py::truthy(&ds) {
            o.p(format!(
                "No device profiles on this server ({} sim status says why).",
                name()
            ));
        }
        for d in py_iter(&ds, 7)? {
            let id = get_or(&d, "id", "?", 8)?;
            let nm = get_or(&d, "name", "?", 8)?;
            let w = get_or(&d, "widthPt", "?", 8)?;
            let h = get_or(&d, "heightPt", "?", 8)?;
            let os = get_or(&d, "os", "", 8)?;
            let def = if truthy_get(&d, "default", 8)? {
                "  (default)"
            } else {
                ""
            };
            o.p(format!(
                "{} {} {w}x{h} pt  {os}{def}",
                super::ljust(&id, 28),
                super::ljust(&nm, 18)
            ));
        }
        Ok(())
    })();
    o.end(r)
}

/// `_sim_boot`'s reader.
pub fn render_boot(body: &[u8]) -> Shown {
    let mut o = Lines::default();
    let r = (|| -> Result<(), Raise> {
        let v = or_default(Some(&load(body)?), Value::Object(Vec::new()));
        let state = py_get(&v, "state", 4)?.cloned().unwrap_or_else(|| s("?"));
        let profile = or_default(py_get(&v, "profile", 5)?, Value::Object(Vec::new()));
        let nm = get_or(&profile, "name", "?", 5)?;
        let n = name();
        if py_eq(&state, &s("running")) {
            o.p(format!(
                "Running: sim {} · {} · {nm}. Next: {n} sim install <app>, or {n} sim tree",
                get_s(&v, "sim", 7)?,
                get_s(&v, "lane", 7)?
            ));
        } else {
            o.p(format!(
                "Booting: sim {} · {} · {nm} ({}). Next: {n} sim wait, then {n} sim tree",
                get_s(&v, "sim", 9)?,
                get_s(&v, "lane", 9)?,
                py::str_of(&state)
            ));
        }
        Ok(())
    })();
    o.end(r)
}

/// `_sim_logs`'s reader.
pub fn render_logs(body: &[u8]) -> Shown {
    let mut o = Lines::default();
    let r = (|| -> Result<(), Raise> {
        let v = load(body)?;
        let lines = or_default(py_get(&v, "lines", 4)?, Value::Array(Vec::new()));
        for l in py_iter(&lines, 5)? {
            o.p(py::str_of(&l));
        }
        if !py::truthy(&lines) {
            o.p("(no log lines)");
        }
        if truthy_get(&v, "truncated", 9)? {
            o.err
                .push_str("(cut at the line/byte cap; narrow with --since, --app or --grep)\n");
        }
        Ok(())
    })();
    o.end(r)
}

/// `_sim_call`'s error reader: a status that is not 2xx (`000` for no
/// answer), the answer's body and `--json`.
pub fn refusal(status: &str, body: &[u8], as_json: bool) -> Shown {
    let raw = text_of(body);
    let mut code = s(match status {
        "400" => "bad_request",
        "401" => "unauthorized",
        "403" => "forbidden",
        "404" => "not_found",
        "405" => "method_not_allowed",
        "409" => "conflict",
        "000" => "daemon_unreachable",
        _ => "error",
    });
    let mut hint = s("");
    match json::parse(&raw) {
        Ok(Value::Object(v)) => {
            let e = get(&v, "error").cloned();
            if let Some(Value::Object(e)) = &e {
                if let Some(c) = get(e, "code").filter(|c| py::truthy(c)) {
                    code = c.clone();
                }
                hint = get(e, "hint")
                    .filter(|h| py::truthy(h))
                    .or_else(|| get(e, "message").filter(|m| py::truthy(m)))
                    .cloned()
                    .unwrap_or_else(|| s(""));
            } else {
                hint = match get(&v, "message").filter(|m| py::truthy(m)) {
                    Some(m) => m.clone(),
                    None => match &e {
                        Some(Value::Null) | None => s(""),
                        Some(e) => s(&py::str_of(e)),
                    },
                };
                code = match get(&v, "code").filter(|c| py::truthy(c)) {
                    Some(c) => c.clone(),
                    None => match &e {
                        Some(Value::Str(t))
                            if !py::str_of(&Value::Str(t.clone())).contains(' ') =>
                        {
                            Value::Str(t.clone())
                        }
                        _ => code,
                    },
                };
            }
        }
        _ => {
            let stripped = raw.trim_matches(py_space);
            if !stripped.is_empty() {
                hint = s(&stripped.chars().take(300).collect::<String>());
            }
        }
    }
    if status == "000" {
        hint = s(concat!(
            "no answer from the ",
            k2_client::product_name!(),
            " daemon"
        ));
    }
    let held = py_eq(&code, &s("sim_held")) || py_eq(&code, &s("held"));
    if held && !py::truthy(&hint) {
        hint = s("held: a person is using the device");
    }
    if !py::truthy(&hint) {
        hint = s(&format!("the daemon refused (HTTP {status})"));
    }
    let mut err = String::new();
    if !as_json {
        err.push_str(&format!("Error: {}\n", py::str_of(&hint)));
        if held {
            let n = name();
            err.push_str(&format!(
                "Run `{n} sim wait`, then `{n} sim tree` before acting again.\n"
            ));
        }
    }
    let number = status.trim_start_matches('0');
    let obj = Value::Object(vec![(
        Text::from("error"),
        Value::Object(vec![
            (Text::from("code"), code),
            (Text::from("hint"), hint),
            (
                Text::from("status"),
                Value::Int(if number.is_empty() { "0" } else { number }.into()),
            ),
        ]),
    )]);
    err.push_str(&dumps(&obj));
    err.push('\n');
    let exit = if status == "000" || status.starts_with('5') {
        Exit::Error
    } else if status == "400" || held {
        Exit::Usage
    } else {
        Exit::Refused
    };
    Shown::err(err, exit)
}

/// `_sim_target`'s Python on what `/cli/workspace/resolve` answered.
pub fn resolved(body: &[u8]) -> String {
    let raw = text_of(body);
    let raw = if raw.is_empty() {
        "{}".to_string()
    } else {
        raw
    };
    let d = json::parse(&raw).unwrap_or(Value::Object(Vec::new()));
    let Value::Object(p) = &d else {
        return String::new();
    };
    let pick = get(p, "path")
        .filter(|v| py::truthy(v))
        .or_else(|| get(p, "projectPath").filter(|v| py::truthy(v)));
    let mut out = pick.map_or_else(String::new, py::str_of);
    while out.ends_with('\n') {
        out.pop();
    }
    out.replace('\0', "")
}

// ---------------------------------------------------------------------
// The run.

fn help_page() -> &'static str {
    crate::text::help("sim").expect("the sim help page is embedded")
}

/// The words after `sim` that `cmd_sim` reads as help: `-h` or `--help`
/// before the first bare `--` (`_wants_help`), no verb, an empty one or
/// `help`.
pub fn wants_help(words: &[&str]) -> bool {
    crate::dashdash::wants_help(words) || matches!(words.first(), None | Some(&("" | "help")))
}

/// `k5 sim …`: `args` is the whole command line after `argv[0]`. `None`:
/// the shell CLI answers it.
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
    let help = wants_help(&words);
    if !help {
        if SUBCOMMANDS.contains(&words[0]) && handed_back(&words) {
            return None;
        }
        if prelude::gate(&settings).is_none()
            && settings.port_text.as_deref().and_then(curl_port).is_none()
        {
            return None;
        }
    }
    Some(run_with(args, &words, hook_sock.as_deref(), settings))
}

/// Whether the shell answers this line: an edge where bash prints its own
/// error (see the module's notes).
fn handed_back(words: &[&str]) -> bool {
    let Ok(c) = common(&words[1..]) else {
        return false;
    };
    let ws = &c.workspace;
    if !ws.is_empty() && Path::new(ws).is_dir() && crate::shellpath::cd_to_shell(ws) {
        return true;
    }
    matches!(
        parse(words[0], &c.rest, &|p| Path::new(p).exists(), &cd_moved),
        Parsed::Shell
    )
}

fn run_with(
    args: &[OsString],
    words: &[&str],
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    // `_k2_offline_verb` and the gate's skip: a help word before `--`.
    let offline = crate::dashdash::wants_help(words);
    let skip = matches!(words.first(), None | Some(&("" | "help" | "--help" | "-h"))) || offline;
    let no_token = settings.auth == Auth::None;
    if settings.no_owner_fallback && no_token && !offline {
        return Gate::VirtualDesktop.stop();
    }
    if !skip && (settings.port_text.as_deref().is_none_or(str::is_empty) || no_token) {
        return Gate::CannotConnect.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    if wants_help(words) {
        return match out::stdout(help_page()) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        };
    }
    let sub = words[0];
    if !SUBCOMMANDS.contains(&sub) {
        out::stderr_line(&format!(
            "Error: unknown {} sim command: {sub} (status, list-devices, boot, stop, erase, install, launch, terminate, tree, tap, type, swipe, key, button, rotate, open-url, location, appearance, screenshot, logs, wait)",
            name()
        ));
        let _ = out::write_flushed(&mut std::io::stderr().lock(), help_page().as_bytes());
        return Exit::Usage;
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut run = Run {
        in_cell: hook_sock.is_some() && client.passport().is_some() && !client.force_tcp,
        client,
        closing: Closing::default(),
        project: settings.project.clone(),
        json: false,
    };
    let exit = run.verb(sub, &words[1..]);
    let Run {
        mut client,
        closing,
        ..
    } = run;
    closing.flush(&mut client, exit);
    exit
}

struct Run {
    client: Client,
    closing: Closing,
    project: String,
    /// `_changes_in_cell`: a session with a passport, not a remote host.
    in_cell: bool,
    /// `_SIM_JSON`.
    json: bool,
}

impl Run {
    fn verb(&mut self, sub: &str, words: &[&str]) -> Exit {
        let c = match common(words) {
            Ok(c) => c,
            Err(d) => return d.print(),
        };
        self.json = c.json;
        let parsed = parse(sub, &c.rest, &|p| Path::new(p).exists(), &cd_moved);
        // `_sim_target`'s `cd` with a folder named as an option it doesn't
        // take: its error, and `set -e` ends the run.
        if !c.workspace.is_empty() && Path::new(&c.workspace).is_dir() {
            if let Some(exit) = crate::shellpath::cd_refused(&c.workspace) {
                return exit;
            }
        }
        let ws = self.target(&c.workspace);
        let plan = match parsed {
            Parsed::Plan(p) => p,
            Parsed::Die(d) => {
                return Die {
                    json: self.json,
                    ..d
                }
                .print()
            }
            Parsed::Shell => unreachable!("handed back before the run"),
        };
        match plan {
            Plan::Get {
                route,
                pairs,
                render,
            } => self.call(false, route, &query(&ws, &pairs), None, &render),
            Plan::Post {
                route,
                mut fields,
                max_time,
                mut render,
                confirm,
            } => {
                if confirm {
                    if let Some(stop) = self.confirm_erase() {
                        return stop;
                    }
                }
                if route == "/cli/sim/install" {
                    let Some(Field::S(_, p)) = fields.first().cloned() else {
                        unreachable!("install's one field");
                    };
                    let dir = cd_pwd(&logical_pwd(), &dirname(&p)).unwrap_or_default();
                    let abs = format!("{dir}/{}", basename(&p));
                    render = line(format!("Installed {}", basename(&abs)));
                    fields = vec![Field::S("path", abs)];
                }
                self.call(true, &route, &body(&ws, &fields), max_time, &render)
            }
        }
    }

    /// `_sim_erase`'s question, when stdin and stdout are terminals; else
    /// `confirm_required`.
    fn confirm_erase(&mut self) -> Option<Exit> {
        if !(is_tty(0) && is_tty(1)) {
            return Some(
                Die {
                    json: self.json,
                    ..die(
                        "confirm_required",
                        format!(
                            "{} sim erase deletes the device's apps and data; pass --yes",
                            name()
                        ),
                    )
                }
                .print(),
            );
        }
        let _ = out::write_flushed(
            &mut std::io::stderr().lock(),
            b"Erase this workspace's device? Its apps and data are deleted. [y/N] ",
        );
        let reply = read_line();
        if matches!(reply.trim_matches([' ', '\t']), "y" | "Y" | "yes" | "YES") {
            return None;
        }
        out::stderr_line("Not erased.");
        Some(Exit::Usage)
    }

    /// `_sim_target <ws>`: the `workspace` to send.
    fn target(&mut self, ws: &str) -> String {
        if !ws.is_empty() {
            if Path::new(ws).is_dir() {
                return cd_pwd(&logical_pwd(), ws).unwrap_or_default();
            }
            let q = format!("q={}", quote(ws));
            let passport = self.client.passport().is_some();
            let got = match self
                .client
                .call(Channel::Tcp, RESOLVE, Call::Request(&[q.as_str()]))
            {
                Ok(r) => {
                    self.closing.answer(&r, passport);
                    resolved(&super::piped_body(&r))
                }
                // Its line went to /dev/null; `_k2_end_command` still ends
                // the command from inside the pipe.
                Err(Error::NoAnswer(_)) => super::end_in_pipe(),
                Err(_) => String::new(),
            };
            return if got.is_empty() { ws.to_string() } else { got };
        }
        if self.in_cell {
            String::new()
        } else {
            self.project.clone()
        }
    }

    /// `_sim_call`, then the verb's output.
    fn call(
        &mut self,
        post: bool,
        route: &str,
        payload: &str,
        max_time: Option<u64>,
        render: &Render,
    ) -> Exit {
        self.client.send_max_time = max_time.map(Duration::from_secs);
        let passport = self.client.passport().is_some();
        let sent = self.client.send_json(post, route, payload);
        self.client.send_max_time = None;
        let (status, body) = match sent {
            Ok(r) => {
                self.closing.answer(&r, passport);
                (format!("{:03}", r.status), super::without_nuls(&r.body))
            }
            Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
                out::stderr_line(&e.to_string());
                return Exit::Error;
            }
            Err(_) => ("000".to_string(), Vec::new()),
        };
        if !status.starts_with('2') || status.len() != 3 {
            return refusal(&status, &body, self.json).print();
        }
        if self.json {
            let mut b = body;
            b.push(b'\n');
            return match out::write_flushed(&mut std::io::stdout().lock(), &b) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            };
        }
        match render {
            Render::Field(field, fallback) => print_field(&body, field, fallback),
            Render::Status => render_status(&body),
            Render::Devices => render_devices(&body),
            Render::Boot => render_boot(&body),
            Render::Logs => render_logs(&body),
        }
        .print()
    }
}

pub fn logical_pwd() -> std::path::PathBuf {
    daemon::logical_cwd(std::env::var_os("PWD")).unwrap_or_default()
}

#[cfg(unix)]
pub fn is_tty(fd: i32) -> bool {
    // SAFETY: isatty only reads the descriptor's state.
    unsafe { libc::isatty(fd) == 1 }
}

#[cfg(not(unix))]
pub fn is_tty(_fd: i32) -> bool {
    false
}

/// `read -r reply || true`: one line from stdin, without its newline.
pub fn read_line() -> String {
    use std::io::Read;
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    let mut stdin = std::io::stdin().lock();
    while let Ok(1) = stdin.read(&mut byte) {
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
    }
    String::from_utf8_lossy(&line).into_owned()
}

#[cfg(test)]
mod tests;
