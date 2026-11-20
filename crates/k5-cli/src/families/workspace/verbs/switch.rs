//! The per-workspace switches: `workspace api-guest-policy`,
//! `api-skip-permissions`, `default-model`, `force-model-on-resume`,
//! `host-session-cell-cap` (`api-host-session-cap`)
//! (`cli/src/talk-and-workspace.sh`), `design-tab` (`design-mode`) and
//! `mobile-tab` (`cli/src/browser.sh`), each `get <ws>` or `set <ws> ...`.
//!
//! `get`: the lookup ([`super::Sent::resolve`]), then `cli_request
//! /cli/settings project=<path> | python3 …` ([`get_text`]). `set`: the
//! lookup, then `cli_post_json /cli/workspace/set {"project": <path>,
//! "fields": {<field>: <value>}} | python3 …` ([`set_text`]). Each reader
//! prints the daemon's error on stderr and ends with 1.
//!
//! The words are checked before anything is sent, but for `design-tab`
//! and `mobile-tab`, whose `on|off` word is read after the lookup. `get`
//! and `set` with no workspace, a word that is no switch value, and an
//! unknown verb are usage lines, exit 2. `api-skip-permissions set ... on`
//! from an agent's session (`HOOK_SOCK` and a passport) is
//! `_cli_refuse_agent_self_grant`'s refusal, exit 3, nothing sent.
//!
//! The shell answers the help forms (no verb, `help`) and these:
//! `api-guest-policy set --file` with a file that is not UTF-8 or holds a
//! NUL (bash warns about the NUL), and a `host-session-cell-cap` value
//! with a line break in it or too long for bash's `[ -gt ]`.

use super::{
    dumps, error_text, is_str, obj, py_get, py_in, py_index, py_strip, repr_head, stop, Ask as Top,
    Py, Sent,
};
use crate::exit::Exit;
use crate::families::{pipefail, s, Piped, Raise, Shown};
use crate::out::json::{self, Value};
use crate::out::py;
use k2_client::brand::PRODUCT_NAME;
use k2_client::cli_name;

/// Which switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    GuestPolicy,
    SkipPermissions,
    DefaultModel,
    ForceModel,
    CellCap,
    Design,
    Mobile,
}

impl Kind {
    /// The `/cli/workspace/set` field.
    pub fn field(self) -> &'static str {
        match self {
            Kind::GuestPolicy => "api_guest_policy",
            Kind::SkipPermissions => "api_skip_permissions",
            Kind::DefaultModel => "default_model",
            Kind::ForceModel => "force_model_on_resume",
            Kind::CellCap => "host_session_cell_cap",
            Kind::Design => "design_mode_enabled",
            Kind::Mobile => "mobile_tab_enabled",
        }
    }

    /// The name in the shell's usage lines.
    fn name(self) -> &'static str {
        match self {
            Kind::GuestPolicy => "api-guest-policy",
            Kind::SkipPermissions => "api-skip-permissions",
            Kind::DefaultModel => "default-model",
            Kind::ForceModel => "force-model-on-resume",
            Kind::CellCap => "host-session-cell-cap",
            Kind::Design | Kind::Mobile => "",
        }
    }
}

/// `api-guest-policy set`'s text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Text {
    Given(String),
    /// `--file -`: `$(cat)`, read when the command runs.
    Stdin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub kind: Kind,
    /// `get`, else `set`.
    pub get: bool,
    pub target: String,
    /// `set`'s value as sent (for `design-tab` and `mobile-tab`, the word
    /// as typed, read after the lookup).
    pub value: Text,
    /// The verb as typed (`design-tab` or `design-mode`).
    pub name: String,
}

fn usage(line: String) -> Top {
    Top::Stop(super::Stop {
        lines: vec![line],
        exit: Exit::Usage,
    })
}

fn unknown(kind: Kind, sub: &str) -> Top {
    Top::Stop(super::Stop {
        lines: vec![
            format!("Unknown {} subcommand: {sub}", kind.name()),
            format!(
                concat!(
                    "Usage: ",
                    cli_name!(),
                    " workspace {} get|set <workspace> …"
                ),
                kind.name()
            ),
        ],
        exit: Exit::Usage,
    })
}

fn usage_of(kind: Kind, tail: &str) -> String {
    format!(
        concat!("Usage: ", cli_name!(), " workspace {} {}"),
        kind.name(),
        tail
    )
}

/// The words after the verb.
pub fn parse(verb: &str, words: &[&str]) -> Top {
    let kind = match verb {
        "api-guest-policy" => Kind::GuestPolicy,
        "api-skip-permissions" => Kind::SkipPermissions,
        "default-model" => Kind::DefaultModel,
        "force-model-on-resume" => Kind::ForceModel,
        "design-tab" | "design-mode" => Kind::Design,
        "mobile-tab" => Kind::Mobile,
        _ => Kind::CellCap,
    };
    let sub = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or(&[]);
    let arg = |i: usize| rest.get(i).copied().unwrap_or("");
    let target = arg(0).to_string();
    let ask = |get: bool, value: Text| {
        Top::Switch(Ask {
            kind,
            get,
            target: target.clone(),
            value,
            name: verb.to_string(),
        })
    };
    if matches!(kind, Kind::Design | Kind::Mobile) {
        let words = if kind == Kind::Mobile {
            "on|off|auto"
        } else {
            "on|off"
        };
        return match sub {
            "get" | "set" if target.is_empty() => usage(format!(
                concat!(
                    "Usage: ",
                    cli_name!(),
                    " workspace {} get|set <workspace> [{}]"
                ),
                verb, words
            )),
            "get" => ask(true, Text::Given(String::new())),
            "set" => ask(false, Text::Given(arg(1).to_string())),
            "" | "help" | "-h" | "--help" => Top::Shell,
            _ => usage(format!(
                concat!(
                    "Usage: ",
                    cli_name!(),
                    " workspace {} get|set <workspace> [{}]"
                ),
                verb, words
            )),
        };
    }
    match sub {
        "get" if target.is_empty() => usage(usage_of(kind, "get <workspace>")),
        "get" => ask(true, Text::Given(String::new())),
        "set" => set(kind, rest, &ask),
        "" | "help" | "-h" | "--help" => Top::Shell,
        _ => unknown(kind, sub),
    }
}

fn set(kind: Kind, rest: &[&str], ask: &dyn Fn(bool, Text) -> Top) -> Top {
    let target = rest.first().copied().unwrap_or("");
    let second = rest.get(1).copied().unwrap_or("");
    match kind {
        Kind::GuestPolicy => {
            if target.is_empty() {
                return usage(usage_of(
                    kind,
                    "set <workspace> <text|--file PATH|--default>",
                ));
            }
            let after = &rest[1..];
            let text = match after.first().copied() {
                Some("--default" | "--clear") => String::new(),
                Some("--file") => {
                    let fpath = after.get(1).copied().unwrap_or("");
                    if fpath.is_empty() {
                        return usage(usage_of(kind, "set <workspace> --file <path>"));
                    }
                    if fpath == "-" {
                        return ask(false, Text::Stdin);
                    }
                    if !std::path::Path::new(fpath).is_file() {
                        return stop(&[&format!("Error: file not found: {fpath}")], Exit::Error);
                    }
                    let Ok(bytes) = std::fs::read(fpath) else {
                        return Top::Shell;
                    };
                    if bytes.contains(&0) {
                        return Top::Shell;
                    }
                    let Ok(text) = String::from_utf8(bytes) else {
                        return Top::Shell;
                    };
                    text.trim_end_matches('\n').to_string()
                }
                None => {
                    return Top::Stop(super::Stop {
                        lines: vec![
                            usage_of(kind, "set <workspace> <text|--file PATH|--default>"),
                            concat!(
                                "  (or pipe text: ",
                                cli_name!(),
                                " workspace api-guest-policy set <ws> --file -)"
                            )
                            .to_string(),
                        ],
                        exit: Exit::Usage,
                    })
                }
                Some(_) => after.join(" "),
            };
            ask(false, Text::Given(text))
        }
        Kind::SkipPermissions | Kind::ForceModel => {
            if target.is_empty() || second.is_empty() {
                return usage(usage_of(kind, "set <workspace> on|off|1|0"));
            }
            match on_off(second) {
                Some(v) => ask(false, Text::Given(v.to_string())),
                None => stop(
                    &[&format!("Error: expected on|off|1|0, got: {second}")],
                    Exit::Usage,
                ),
            }
        }
        Kind::DefaultModel => {
            if target.is_empty() || second.is_empty() {
                return usage(usage_of(kind, "set <workspace> <model|clear>"));
            }
            let v = match second.to_ascii_lowercase().as_str() {
                "clear" | "unset" | "none" | "" => "",
                _ => second,
            };
            ask(false, Text::Given(v.to_string()))
        }
        _ => {
            if target.is_empty() || second.is_empty() {
                return usage(usage_of(kind, "set <workspace> <n|default>"));
            }
            if second.contains('\n') {
                return Top::Shell;
            }
            let v = match second.to_ascii_lowercase().as_str() {
                "default" | "null" | "" => "default".to_string(),
                _ => {
                    let digits = second.bytes().all(|b| b.is_ascii_digit());
                    if !digits || second.starts_with('0') {
                        return stop(
                            &[&format!(
                                "Error: expected positive integer 1..512 or 'default', got: {second}"
                            )],
                            Exit::Usage,
                        );
                    }
                    if second.len() > 18 {
                        return Top::Shell;
                    }
                    if !second.parse::<u64>().is_ok_and(|n| n <= 512) {
                        return stop(
                            &[&format!(
                                "Error: host_session_cell_cap max is 512 (daemon ceiling), got: {second}"
                            )],
                            Exit::Usage,
                        );
                    }
                    second.to_string()
                }
            };
            ask(false, Text::Given(v))
        }
    }
}

/// `on|1|true|yes` is `1`, `off|0|false|no` is `0` (ASCII case folded, as
/// `tr '[:upper:]' '[:lower:]'` does).
fn on_off(word: &str) -> Option<&'static str> {
    match word.to_ascii_lowercase().as_str() {
        "on" | "1" | "true" | "yes" => Some("1"),
        "off" | "0" | "false" | "no" => Some("0"),
        _ => None,
    }
}

/// The agent refusal of `_cli_refuse_agent_self_grant` (`cli/src/agent.sh`).
fn self_grant(field: &str, where_: &str) -> String {
    let hint = format!(
        "An agent can't grant itself '{field}'. Ask a human (owner or admin) to change it: {where_} — or run this from a terminal outside a {PRODUCT_NAME} agent session."
    );
    dumps(&obj(&[(
        "error",
        obj(&[("code", s("owner_only")), ("hint", s(&hint))]),
    )]))
}

/// The lookup, then the switch's request and its reader.
pub fn send(w: &mut Sent, a: &Ask) -> Exit {
    if a.kind == Kind::SkipPermissions
        && !a.get
        && a.value == Text::Given("1".into())
        && w.settings.session
        && w.client.passport().is_some()
    {
        crate::out::stderr_line(&self_grant(
            "api_skip_permissions",
            concat!(
                "`",
                cli_name!(),
                " workspace api-skip-permissions set <workspace> on` from their own terminal"
            ),
        ));
        return Exit::Refused;
    }
    let value = match &a.value {
        Text::Given(v) => v.clone(),
        Text::Stdin => {
            use std::io::Read;
            let mut b = Vec::new();
            let _ = std::io::stdin().read_to_end(&mut b);
            b.retain(|c| *c != 0);
            String::from_utf8_lossy(&b)
                .trim_end_matches('\n')
                .to_string()
        }
    };
    let path = match w.resolve(&a.target) {
        Ok(p) => p,
        Err(exit) => return exit,
    };
    if a.get {
        let piped = w.settings_of(&path);
        let shown = get_text(a.kind, &piped);
        return pipefail(piped.exit, shown.print());
    }
    let value = match a.kind {
        Kind::Design | Kind::Mobile => {
            let v = match value.to_ascii_lowercase().as_str() {
                "on" | "1" | "true" | "yes" => "1",
                "off" | "0" | "false" | "no" => "0",
                "auto" | "default" if a.kind == Kind::Mobile => "auto",
                _ => "",
            };
            if v.is_empty() {
                let words = if a.kind == Kind::Mobile {
                    "on|off|auto"
                } else {
                    "on|off"
                };
                let got = if value.is_empty() {
                    "<nothing>"
                } else {
                    &value
                };
                crate::out::stderr_line(&format!("Error: expected {words}, got: {got}"));
                return Exit::Usage;
            }
            v.to_string()
        }
        _ => value,
    };
    let body = dumps(&obj(&[
        ("project", s(&path)),
        ("fields", obj(&[(a.kind.field(), s(&value))])),
    ]));
    let r = w.post_json(super::SET, &body);
    let piped = w.piped(r);
    let shown = set_text(a.kind, &value, &piped);
    pipefail(piped.exit, shown.print())
}

/// The Python lines a traceback names.
pub mod line {
    /// The get readers: `d.get(...)` on what is not a dict.
    pub const POLICY_GET: u32 = 16;
    pub const POLICY_ENDS: u32 = 20;
    pub const SKIP_HAS: u32 = 17;
    pub const SKIP_GET: u32 = 18;
    pub const SKIP_GET_SNAKE: u32 = 20;
    pub const MODEL_GET: u32 = 16;
    pub const MODEL_GET_SNAKE: u32 = 18;
    pub const FORCE_HAS: u32 = 16;
    pub const FORCE_GET: u32 = 17;
    pub const FORCE_GET_SNAKE: u32 = 19;
    pub const CAP_HAS: u32 = 16;
    pub const CAP_GET: u32 = 17;
    pub const CAP_GET_SNAKE: u32 = 19;
    pub const CAP_INT: u32 = 28;
    pub const TAB_ERROR: u32 = 12;
    pub const TAB_DESIGN_HAS: u32 = 15;
    pub const TAB_DESIGN: u32 = 18;
    pub const TAB_MOBILE_HAS: u32 = 20;
    pub const TAB_WORD: u32 = 23;
    pub const TAB_SHOWN: u32 = 24;
    /// The set readers.
    pub const SET_ERROR: u32 = 9;
    pub const SET_FOR: u32 = 17;
    pub const SET_ROW: u32 = 18;
    pub const POLICY_STRIP: u32 = 24;
    pub const CAP_FOR: u32 = 18;
    pub const CAP_ROW: u32 = 19;
    pub const TAB_SET_ERROR: u32 = 10;
    pub const TAB_FOR: u32 = 17;
    pub const TAB_ROW: u32 = 18;
}

type R<T> = Result<T, Raise>;

/// The reader's start: `d = json.loads(raw)`, or its error line.
fn loaded(raw: &str, py: &mut Py) -> Option<Value> {
    match json::parse(raw) {
        Ok(d) => Some(d),
        Err(_) => {
            py.eprint(&format!(
                "Error: daemon returned an unparseable response: {}",
                repr_head(raw, 200)
            ));
            None
        }
    }
}

/// The get readers' `cli_request /cli/settings ... | python3 …`.
pub fn get_text(kind: Kind, piped: &Piped) -> Shown {
    let raw = String::from_utf8_lossy(&piped.body).into_owned();
    let mut py = Py::default();
    let Some(d) = loaded(&raw, &mut py) else {
        return py.done(Exit::Error);
    };
    if let Value::Object(pairs) = &d {
        if let Some(err) = crate::families::get(pairs, "error").filter(|e| py::truthy(e)) {
            py.eprint(&format!("Error: {}", error_text(err)));
            return py.done(Exit::Error);
        }
    }
    match get_body(kind, &d, &mut py) {
        Ok(exit) => py.done(exit),
        Err(r) => py.raised(&r),
    }
}

fn attr(line: u32) -> impl Fn(String) -> Raise {
    move |e| Raise::at(line, format!("AttributeError: {e}"))
}

fn type_err(line: u32) -> impl Fn(String) -> Raise {
    move |e| Raise::at(line, format!("TypeError: {e}"))
}

/// `v is True or v == 1 or v == "1" or v.lower() in (...)`: `Some(true)`
/// on, `Some(false)` off, `None` neither.
fn on_value(v: &Value) -> Option<bool> {
    let lower = match v {
        Value::Str(_) => py::str_of(v).to_lowercase(),
        _ => String::new(),
    };
    let one = match v {
        Value::Bool(true) => true,
        Value::Int(d) => d == "1",
        Value::Float(f) => *f == 1.0,
        Value::Str(_) => lower == "1" || matches!(lower.as_str(), "on" | "true" | "yes"),
        _ => false,
    };
    if one {
        return Some(true);
    }
    let zero = match v {
        Value::Bool(false) => true,
        Value::Int(d) => d == "0",
        Value::Float(f) => *f == 0.0,
        Value::Str(_) => lower == "0" || matches!(lower.as_str(), "off" | "false" | "no"),
        _ => false,
    };
    zero.then_some(false)
}

/// The field under its camelCase or snake_case name, as `"x" in d` then
/// `d.get("x")` read it: `None` when neither is there.
fn either(
    d: &Value,
    camel: &str,
    snake: &str,
    has: u32,
    get: u32,
    get_snake: u32,
) -> R<Option<Value>> {
    if py_in(camel, d).map_err(type_err(has))? {
        return Ok(Some(
            py_get(d, camel)
                .map_err(attr(get))?
                .cloned()
                .unwrap_or(Value::Null),
        ));
    }
    if py_in(snake, d).map_err(type_err(has + 2))? {
        return Ok(Some(
            py_get(d, snake)
                .map_err(attr(get_snake))?
                .cloned()
                .unwrap_or(Value::Null),
        ));
    }
    Ok(None)
}

fn get_body(kind: Kind, d: &Value, py: &mut Py) -> R<Exit> {
    match kind {
        Kind::GuestPolicy => {
            let policy = py_get(d, "apiGuestPolicy")
                .map_err(attr(line::POLICY_GET))?
                .cloned()
                .unwrap_or(Value::Null);
            if policy == Value::Null {
                py.eprint("Error: settings response missing apiGuestPolicy");
                return Ok(Exit::Error);
            }
            let Value::Str(_) = policy else {
                return Err(Raise::no_attribute(line::POLICY_ENDS, &policy, "endswith"));
            };
            let p = py::str_of(&policy);
            py.out.push_str(&p);
            if !p.ends_with('\n') {
                py.out.push('\n');
            }
        }
        Kind::SkipPermissions => {
            let Some(v) = either(
                d,
                "apiSkipPermissions",
                "api_skip_permissions",
                line::SKIP_HAS,
                line::SKIP_GET,
                line::SKIP_GET_SNAKE,
            )?
            else {
                py.print("on  (default; settings response did not include api_skip_permissions)");
                return Ok(Exit::Ok);
            };
            match on_value(&v) {
                Some(true) => py.print("on"),
                Some(false) => py.print("off"),
                None if v == Value::Null => py.print("on  (default)"),
                None => py.print(if py::truthy(&v) { "on" } else { "off" }),
            }
        }
        Kind::ForceModel => {
            let Some(v) = either(
                d,
                "forceModelOnResume",
                "force_model_on_resume",
                line::FORCE_HAS,
                line::FORCE_GET,
                line::FORCE_GET_SNAKE,
            )?
            else {
                py.print("off  (default)");
                return Ok(Exit::Ok);
            };
            match on_value(&v) {
                Some(true) => py.print("on"),
                Some(false) => py.print("off"),
                None => py.print(if py::truthy(&v) { "on" } else { "off" }),
            }
        }
        Kind::DefaultModel => {
            let mut v = py_get(d, "defaultModel")
                .map_err(attr(line::MODEL_GET))?
                .cloned()
                .unwrap_or(Value::Null);
            if v == Value::Null {
                v = py_get(d, "default_model")
                    .map_err(attr(line::MODEL_GET_SNAKE))?
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            let unset = match &v {
                Value::Null => true,
                Value::Str(_) => py_strip(&py::str_of(&v)).is_empty(),
                _ => false,
            };
            if unset {
                py.print("(unset)");
            } else {
                py.print(&py::str_of(&v));
            }
        }
        Kind::CellCap => {
            let Some(v) = either(
                d,
                "hostSessionCellCap",
                "host_session_cell_cap",
                line::CAP_HAS,
                line::CAP_GET,
                line::CAP_GET_SNAKE,
            )?
            else {
                py.print("15 (default; settings response did not include host_session_cell_cap)");
                return Ok(Exit::Ok);
            };
            match &v {
                Value::Null => py.print("15 (default)"),
                Value::Int(digits) => py.print(digits),
                Value::Float(f) => {
                    if f.is_nan() {
                        return Err(Raise::at(
                            line::CAP_INT,
                            "ValueError: cannot convert float NaN to integer",
                        ));
                    }
                    if f.is_infinite() {
                        return Err(Raise::at(
                            line::CAP_INT,
                            "OverflowError: cannot convert float infinity to integer",
                        ));
                    }
                    let t = f.trunc();
                    py.print(&if t == 0.0 {
                        "0".to_string()
                    } else {
                        format!("{t:.0}")
                    });
                }
                Value::Str(_) => {
                    let t = py::str_of(&v);
                    let t = py_strip(&t);
                    if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
                        py.print(t);
                    } else {
                        py.print("15 (default)");
                    }
                }
                _ => py.print("15 (default)"),
            }
        }
        Kind::Design => {
            if !py_in("designModeEnabled", d).map_err(type_err(line::TAB_DESIGN_HAS))? {
                py.eprint("Error: this daemon does not know the Design tab setting (update it)");
                return Ok(Exit::Error);
            }
            let v =
                py_index(d, "designModeEnabled").map_err(|e| index_err(line::TAB_DESIGN, d, e))?;
            py.print(if py::truthy(v) { "on" } else { "off" });
        }
        Kind::Mobile => {
            let has_tab = py_in("mobileTab", d).map_err(type_err(line::TAB_MOBILE_HAS))?;
            if !has_tab || !py_in("mobileTabShown", d).map_err(type_err(line::TAB_MOBILE_HAS))? {
                py.eprint("Error: this daemon does not know the Mobile tab setting (update it)");
                return Ok(Exit::Error);
            }
            let word = py_index(d, "mobileTab").map_err(|e| index_err(line::TAB_WORD, d, e))?;
            let shown =
                py_index(d, "mobileTabShown").map_err(|e| index_err(line::TAB_SHOWN, d, e))?;
            let w = py::str_of(word);
            if py::truthy(shown) {
                py.print(&format!("{w} (shown)"));
            } else if is_str(word, "off") {
                py.print("off (not shown)");
            } else {
                py.print(&format!("{w} (not shown: this server cannot run a phone)"));
            }
        }
    }
    Ok(Exit::Ok)
}

/// `d[key]`'s exception: a missing key is a `KeyError`.
fn index_err(line: u32, d: &Value, e: String) -> Raise {
    if let Value::Object(_) = d {
        Raise::at(line, format!("KeyError: {e}"))
    } else {
        Raise::at(line, format!("TypeError: {e}"))
    }
}

/// The set readers' `cli_post_json /cli/workspace/set ... | python3 …`;
/// `value` is what was sent.
pub fn set_text(kind: Kind, value: &str, piped: &Piped) -> Shown {
    let raw = String::from_utf8_lossy(&piped.body).into_owned();
    let mut py = Py::default();
    let Some(d) = loaded(&raw, &mut py) else {
        return py.done(Exit::Error);
    };
    match set_body(kind, value, &raw, &d, &mut py) {
        Ok(exit) => py.done(exit),
        Err(r) => py.raised(&r),
    }
}

fn set_body(kind: Kind, value: &str, raw: &str, d: &Value, py: &mut Py) -> R<Exit> {
    let tab = matches!(kind, Kind::Design | Kind::Mobile);
    let err_line = if tab {
        line::TAB_SET_ERROR
    } else {
        line::SET_ERROR
    };
    let err = py_get(d, "error").map_err(attr(err_line))?.cloned();
    let failed = err.as_ref().is_some_and(py::truthy)
        || py_get(d, "ok").map_err(attr(err_line))? == Some(&Value::Bool(false));
    if failed {
        let err = err.unwrap_or(Value::Null);
        let shown = match &err {
            // `err.get("hint") or err.get("code") or err`, then `err or
            // raw[:200]`: an empty dict is the answer's head.
            Value::Object(p) if tab && p.is_empty() => super::head(raw, 200),
            Value::Object(_) => error_text(&err),
            e if py::truthy(e) => py::str_of(e),
            _ => super::head(raw, 200),
        };
        py.eprint(&format!("Error: {shown}"));
        return Ok(Exit::Error);
    }
    let (for_line, row_line) = match kind {
        Kind::CellCap => (line::CAP_FOR, line::CAP_ROW),
        Kind::Design | Kind::Mobile => (line::TAB_FOR, line::TAB_ROW),
        _ => (line::SET_FOR, line::SET_ROW),
    };
    let actions = py_get(d, "actions").map_err(attr(for_line))?;
    let actions = match actions.filter(|a| py::truthy(a)) {
        Some(a) => crate::families::py_iter(a, for_line)?,
        None => Vec::new(),
    };
    let field = kind.field();
    let state_of = |v: &Value| {
        if ["1", "true", "True"].contains(&py::str_of(v).as_str()) {
            "on"
        } else {
            "off"
        }
    };
    match kind {
        Kind::GuestPolicy => {
            let mut val = s("");
            for a in &actions {
                let f = py_get(a, "field").map_err(attr(row_line))?;
                if f.is_some_and(|f| is_str(f, field)) {
                    let v = py_get(a, "value").map_err(attr(row_line + 1))?;
                    val = crate::families::remind::shared::or(v, s(""));
                    let st = py_get(a, "status").map_err(attr(row_line + 2))?;
                    if st.is_some_and(|st| is_str(st, "skipped")) {
                        py.print("API guest policy already set (unchanged).");
                        return Ok(Exit::Ok);
                    }
                }
            }
            let Value::Str(_) = val else {
                return Err(Raise::no_attribute(line::POLICY_STRIP, &val, "strip"));
            };
            let text = py::str_of(&val);
            if py_strip(&text).is_empty() {
                py.print("API guest policy cleared (platform default will be used on inject).");
            } else {
                py.print(&format!(
                    "API guest policy set ({} chars).",
                    text.chars().count()
                ));
            }
        }
        Kind::SkipPermissions | Kind::ForceModel => {
            let mut val = Value::Null;
            for a in &actions {
                let f = py_get(a, "field").map_err(attr(row_line))?;
                if f.is_some_and(|f| is_str(f, field)) {
                    val = py_get(a, "value")
                        .map_err(attr(row_line + 1))?
                        .cloned()
                        .unwrap_or(Value::Null);
                    let st = py_get(a, "status").map_err(attr(row_line + 2))?;
                    if st.is_some_and(|st| is_str(st, "skipped")) {
                        py.print(&format!("{field} already {} (unchanged).", state_of(&val)));
                        return Ok(Exit::Ok);
                    }
                }
            }
            if val == Value::Null {
                val = s("");
            }
            py.print(&format!("{field} set to {}.", state_of(&val)));
        }
        Kind::DefaultModel => {
            let mut val = Value::Null;
            for a in &actions {
                let f = py_get(a, "field").map_err(attr(row_line))?;
                if f.is_some_and(|f| is_str(f, field)) {
                    val = py_get(a, "value")
                        .map_err(attr(row_line + 1))?
                        .cloned()
                        .unwrap_or(Value::Null);
                    let st = py_get(a, "status").map_err(attr(row_line + 2))?;
                    if st.is_some_and(|st| is_str(st, "skipped")) {
                        let shown = if py::truthy(&val) {
                            py::str_of(&val)
                        } else {
                            "(unset)".to_string()
                        };
                        py.print(&format!("default_model already {shown} (unchanged)."));
                        return Ok(Exit::Ok);
                    }
                }
            }
            if py::truthy(&val) {
                py.print(&format!("default_model set to {}.", py::str_of(&val)));
            } else {
                py.print("default_model cleared.");
            }
        }
        Kind::CellCap => {
            let (mut val, mut status) = (Value::Null, Value::Null);
            for a in &actions {
                let f = py_get(a, "field").map_err(attr(row_line))?;
                if f.is_some_and(|f| is_str(f, field)) {
                    val = py_get(a, "value")
                        .map_err(attr(row_line + 1))?
                        .cloned()
                        .unwrap_or(Value::Null);
                    status = py_get(a, "status")
                        .map_err(attr(row_line + 2))?
                        .cloned()
                        .unwrap_or(Value::Null);
                    break;
                }
            }
            if is_str(&status, "skipped") {
                let default = matches!(val, Value::Null)
                    || ["", "default", "null"].iter().any(|w| is_str(&val, w))
                    || (matches!(val, Value::Str(_))
                        && ["default", "null"].contains(&py::str_of(&val).to_lowercase().as_str()));
                if default {
                    py.print("host_session_cell_cap already default (inherit; unchanged).");
                } else {
                    py.print(&format!(
                        "host_session_cell_cap already {} (unchanged).",
                        py::str_of(&val)
                    ));
                }
                return Ok(Exit::Ok);
            }
            if val == Value::Null {
                py.print("host_session_cell_cap updated.");
                return Ok(Exit::Ok);
            }
            if ["default", "null", ""].contains(&py::str_of(&val).to_lowercase().as_str()) {
                py.print("host_session_cell_cap cleared → inherit daemon default (15 or env).");
            } else {
                py.print(&format!(
                    "host_session_cell_cap set to {}.",
                    py::str_of(&val)
                ));
            }
        }
        Kind::Design | Kind::Mobile => {
            let label = if kind == Kind::Mobile {
                "Mobile tab"
            } else {
                "Design tab"
            };
            let state = match value {
                "1" => "on",
                "0" => "off",
                _ => "auto",
            };
            for a in &actions {
                let f = py_get(a, "field").map_err(attr(row_line))?;
                if f.is_some_and(|f| is_str(f, field)) {
                    let st = py_get(a, "status").map_err(attr(row_line))?;
                    if st.is_some_and(|st| is_str(st, "skipped")) {
                        py.print(&format!("{label} already {state} (unchanged)."));
                        return Ok(Exit::Ok);
                    }
                }
            }
            py.print(&format!("{label} {state}."));
        }
    }
    Ok(Exit::Ok)
}
