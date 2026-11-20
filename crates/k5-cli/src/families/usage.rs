//! `k5 usage`, as the shell CLI's `cmd_usage` answers it
//! (`cli/src/usage.sh`): token usage (`tokens`), the requests one by one
//! (`turns`) and the subscription limits (`limits`, `--refresh`), each
//! with `--json`, and the help.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings, the virtual desktop's guard (not with `-h` or
//!    `--help` anywhere) and the connection gate (not for the help).
//! 2. `usage` is an `id` tool: with `HOOK_SOCK` set, the stale-passport or
//!    require-passport lines, exit 3, the help too.
//! 3. The words ([`parse`]), then the checks each verb makes ([`check`]):
//!    one `usage:` line, exit 2.
//! 4. One `_sidecar_send`: a GET of `/cli/usage/<verb>` with the query,
//!    or for `limits --refresh` a POST of `/cli/usage/limits/refresh`.
//! 5. A status that is not 200 is `_sidecar_fail`'s
//!    ([`super::sessions::fail_line`]), with its exit and the refusal's
//!    closing line ([`crate::prelude::Closing`]).
//! 6. `--json` prints the body as it came; else the shell's Python lays it
//!    out ([`render`]). `tokens` asks in this computer's zone
//!    (`_usage_local_tz`, [`local_tz`]) unless `--tz` names one; `turns`
//!    prints times by `TZ` as Python's `time.localtime` does; `limits`
//!    reads times with Python's `datetime.fromisoformat` (the extended
//!    forms; a form only Python's basic-format reader takes is "never
//!    checked" here) and counts minutes from now.
//!
//! The shell CLI answers these itself ([`run`] returns `None`): a word
//! that is not UTF-8, `HOOK_SOCK` with a `CODEX_` variable exported, a
//! port curl won't take (for a form that asks), and a `python3` the
//! binary can't tell. Where the shell's Python raises, this prints a
//! Python 3.12 traceback, as [`super::sessions`] says.

use super::sessions::{exit_for, fail_line, held};
use super::{curl_port, get, ljust, py_get, py_iter, py_left, py_space, raised, s, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::offline::{self, Env};
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{permission, Channel, Response};
use k2_client::brand::CLI_NAME as CLI;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["usage"];
/// The binary answers every form ([`run`] may still hand one to the
/// shell CLI).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: the whole row is native.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy.
pub const TOOL: &str = "usage";

/// The value flags, in the shell's order.
const VALUE_FLAGS: [&str; 11] = [
    "--workspace",
    "--harness",
    "--login",
    "--model",
    "--since",
    "--until",
    "--by",
    "--tz",
    "--limit",
    "--before",
    "--tool",
];

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    pub sub: String,
    pub json: bool,
    pub refresh: bool,
    pub here: bool,
    pub outside: bool,
    /// The value flags, by [`VALUE_FLAGS`]' order; empty for none.
    pub values: [String; 11],
}

impl Ask {
    fn v(&self, flag: &str) -> &str {
        let i = VALUE_FLAGS.iter().position(|f| *f == flag).unwrap_or(0);
        &self.values[i]
    }
}

/// What `cmd_usage` does with its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Help,
    Call(Box<Ask>),
    /// A `usage:` line, exit 2.
    Usage(String),
}

/// `cmd_usage`'s words (after `usage`): the help, then the flags.
pub fn parse(words: &[&str]) -> Parsed {
    let sub = words.first().copied().unwrap_or("");
    if matches!(sub, "" | "help" | "--help" | "-h") {
        return Parsed::Help;
    }
    let rest = &words[1..];
    if rest.iter().any(|a| matches!(*a, "--help" | "-h")) {
        return Parsed::Help;
    }
    let mut ask = Ask {
        sub: sub.to_string(),
        ..Ask::default()
    };
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        match a {
            "--json" => ask.json = true,
            "--refresh" => ask.refresh = true,
            "--here" => ask.here = true,
            "--outside" => ask.outside = true,
            _ if VALUE_FLAGS.contains(&a) => {
                let value = rest.get(i + 1).copied().unwrap_or("");
                if value.is_empty() {
                    return Parsed::Usage(format!("usage: {a} needs a value"));
                }
                let at = VALUE_FLAGS.iter().position(|f| *f == a).unwrap_or(0);
                ask.values[at] = value.to_string();
                i += 1;
            }
            _ if a.starts_with("--") => {
                return Parsed::Usage(format!("usage: unknown flag '{a}' ({CLI} usage --help)"))
            }
            _ => {
                return Parsed::Usage(format!(
                    "usage: unexpected argument '{a}' ({CLI} usage --help)"
                ))
            }
        }
        i += 1;
    }
    Parsed::Call(Box::new(ask))
}

/// The request a verb makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// A GET of `/cli/usage/<route>` with the query.
    Get(&'static str, String),
    /// `limits --refresh`: the POST's JSON body.
    Refresh(String),
}

/// The checks after the words, in the shell's order; `here_ws` is
/// `${K2_PROJECT_PATH:-$PWD}`, `local_tz` this computer's zone. `Ok`: the
/// request; `Err`: the `usage:` line.
pub fn check(
    ask: &Ask,
    here_ws: impl FnOnce() -> String,
    local_tz: impl FnOnce() -> String,
) -> Result<Request, String> {
    let mut ws = ask.v("--workspace").to_string();
    if ask.here {
        if !ws.is_empty() {
            return Err("usage: --here and --workspace are the same thing; pick one".into());
        }
        ws = here_ws();
    }
    if !ws.is_empty() && ask.outside {
        return Err("usage: --workspace and --outside cannot both be set".into());
    }
    let one_of = |v: &str, list: &[&str]| v.is_empty() || list.contains(&v);
    if !one_of(ask.v("--harness"), &["claude", "codex", "grok"]) {
        return Err("usage: --harness must be claude, codex, or grok".into());
    }
    if !one_of(ask.v("--tool"), &["claude", "codex", "grok"]) {
        return Err("usage: --tool must be claude, codex, or grok".into());
    }
    if !one_of(ask.v("--by"), &["workspace", "model", "login", "harness"]) {
        return Err("usage: --by must be workspace, model, login, or harness".into());
    }
    let limit = ask.v("--limit");
    let limit_ok = limit.is_empty()
        || (limit.len() <= 3
            && limit.bytes().all(|b| b.is_ascii_digit())
            && !limit.starts_with('0'));
    if !limit_ok {
        return Err("usage: --limit must be 1-200".into());
    }
    let mut q: Vec<String> = Vec::new();
    let mut uq = |k: &str, v: &str| {
        if !v.is_empty() {
            q.push(format!("{k}={}", quote(v)));
        }
    };
    let v = |f: &str| ask.v(f).to_string();
    match ask.sub.as_str() {
        "tokens" => {
            if ask.refresh || !(v("--before") + &v("--tool") + &v("--limit")).is_empty() {
                return Err(format!("usage: {CLI} usage tokens takes no --refresh / --before / --tool / --limit ({CLI} usage --help)"));
            }
            let mut tz = v("--tz");
            if tz.is_empty() {
                tz = local_tz();
            }
            uq("workspace", &ws);
            uq("harness", ask.v("--harness"));
            uq("account", ask.v("--login"));
            uq("model", ask.v("--model"));
            uq("since", ask.v("--since"));
            uq("until", ask.v("--until"));
            uq("group_by", ask.v("--by"));
            uq("tz", &tz);
            if ask.outside {
                uq("outside", "1");
            }
            Ok(Request::Get("tokens", q.join("&")))
        }
        "turns" => {
            let other = v("--tool") + &v("--by") + &v("--tz") + &v("--until") + &v("--model");
            if ask.refresh || ask.outside || !other.is_empty() {
                return Err(format!("usage: {CLI} usage turns [--limit N] [--workspace PATH|--here] [--harness H] [--login ID] [--since T] [--before CURSOR] [--json]"));
            }
            uq("workspace", &ws);
            uq("harness", ask.v("--harness"));
            uq("account", ask.v("--login"));
            uq("since", ask.v("--since"));
            uq("limit", ask.v("--limit"));
            uq("before", ask.v("--before"));
            Ok(Request::Get("turns", q.join("&")))
        }
        "limits" => {
            let other = ws.clone()
                + &v("--harness")
                + &v("--model")
                + &v("--since")
                + &v("--until")
                + &v("--by")
                + &v("--tz")
                + &v("--limit")
                + &v("--before");
            if ask.outside || !other.is_empty() {
                return Err(format!("usage: {CLI} usage limits [--refresh] [--tool claude|codex|grok] [--login ID] [--json]"));
            }
            if !ask.refresh {
                return Ok(Request::Get("limits", String::new()));
            }
            let mut o: Vec<(Text, Value)> = Vec::new();
            if !ask.v("--tool").is_empty() {
                o.push((Text::from("tool"), s(ask.v("--tool"))));
            }
            if !ask.v("--login").is_empty() {
                o.push((Text::from("accountId"), s(ask.v("--login"))));
            }
            let body = json::to_string(&Value::Object(o), &Options::default()).unwrap_or_default();
            Ok(Request::Refresh(body))
        }
        other => Err(format!(
            "usage: unknown usage verb '{other}' ({CLI} usage --help)"
        )),
    }
}

/// `_usage_local_tz`: `TZ` (a leading `:` dropped), else the zone the
/// `/etc/localtime` link names, else `/etc/timezone`'s first line; `UTC`
/// for none, a path, or one with a space.
pub fn local_tz(tz: Option<&str>, link: Option<String>, file: Option<String>) -> String {
    let mut z = tz.unwrap_or("").to_string();
    if let Some(rest) = z.strip_prefix(':') {
        z = rest.to_string();
    }
    if z.is_empty() {
        if let Some(target) = link {
            z = match target.rfind("zoneinfo/") {
                Some(at) => target[at + "zoneinfo/".len()..].to_string(),
                None => target,
            };
        }
    }
    if z.is_empty() {
        if let Some(text) = file {
            z = text.split('\n').next().unwrap_or("").to_string();
        }
    }
    if z.is_empty() || z.starts_with('/') || z.contains(' ') {
        "UTC".into()
    } else {
        z
    }
}

/// [`local_tz`] for this computer.
fn this_tz() -> String {
    let tz = std::env::var("TZ").ok();
    let etc = Path::new("/etc/localtime");
    let link = etc
        .symlink_metadata()
        .ok()
        .filter(|m| m.file_type().is_symlink())
        .and_then(|_| std::fs::read_link(etc).ok())
        .map(|p| p.to_string_lossy().into_owned());
    let file = std::fs::read("/etc/timezone")
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned());
    local_tz(tz.as_deref(), link, file)
}

/// `${K2_PROJECT_PATH:-$PWD}`.
fn here() -> String {
    match std::env::var("K2_PROJECT_PATH") {
        Ok(p) if !p.is_empty() => p,
        _ => offline::shell_pwd(&Env::current())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// `k5 usage ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let all: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    let parsed = parse(all.get(1..).unwrap_or_default());
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
    let help = parsed == Parsed::Help;
    if !help
        && prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    let offline = all.iter().any(|w| matches!(*w, "-h" | "--help"));
    Some(run_with(
        args,
        &parsed,
        offline,
        hook_sock.as_deref(),
        settings,
    ))
}

fn run_with(
    args: &[OsString],
    parsed: &Parsed,
    offline: bool,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    match prelude::gate(&settings) {
        Some(Gate::VirtualDesktop) if !offline => return Gate::VirtualDesktop.stop(),
        Some(_) if *parsed != Parsed::Help => return Gate::CannotConnect.stop(),
        _ => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let ask = match parsed {
        Parsed::Help => return Shown::out(crate::text::help("usage").unwrap_or_default()).print(),
        Parsed::Usage(line) => {
            out::stderr_line(line);
            return Exit::Usage;
        }
        Parsed::Call(ask) => ask,
    };
    let request = match check(ask, here, this_tz) {
        Ok(r) => r,
        Err(line) => {
            out::stderr_line(&line);
            return Exit::Usage;
        }
    };
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let answer = match &request {
        Request::Get(route, q) => client.sidecar_get(&format!("/cli/usage/{route}"), q),
        Request::Refresh(body) => client.sidecar_post("/cli/usage/limits/refresh", body),
    };
    if let Err(e @ crate::transport::Error::MaybeApplied { .. }) = &answer {
        out::stderr_line(&e.to_string());
        return Exit::Error;
    }
    let (status, body) = held(answer);
    let mut closing = Closing::default();
    let exit = if status != "200" {
        out::stderr_line(&fail_line(&status, &body));
        let r = Response {
            status: status.parse().unwrap_or(0),
            permission: permission::note(&body),
            body,
            channel: Channel::Tcp,
            server_version: None,
        };
        closing.answer(&r, client.passport().is_some());
        exit_for(&status)
    } else if ask.json {
        let mut text = body;
        text.push(b'\n');
        match out::write_flushed(&mut std::io::stdout().lock(), &text) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        }
    } else {
        let now_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_micros() as i64);
        render(ask, &body, now_us).print()
    };
    closing.flush(&mut client, exit);
    exit
}

/// The lines of the shell's Python that raise (`tests` pins some).
pub mod line {
    pub const LOAD: u32 = 3;
    pub const T_N: u32 = 5;
    pub const T_FMT: u32 = 8;
    pub const T_COST_GET: u32 = 11;
    pub const T_USD: u32 = 14;
    pub const T_UNPRICED: u32 = 16;
    pub const T_LINE: u32 = 20;
    pub const T_LINE_COST: u32 = 21;
    pub const T_SCOPE: u32 = 22;
    pub const T_TOTAL: u32 = 25;
    pub const T_C: u32 = 26;
    pub const T_EST: u32 = 28;
    pub const T_HARN: u32 = 31;
    pub const T_GROUP: u32 = 35;
    pub const T_DAYS: u32 = 36;
    pub const T_DAY: u32 = 40;
    pub const U_ROWS: u32 = 4;
    pub const U_FOR: u32 = 7;
    pub const U_MS: u32 = 8;
    pub const U_WHEN: u32 = 9;
    pub const U_WS: u32 = 10;
    pub const U_COST: u32 = 12;
    pub const U_ROW: u32 = 14;
    pub const U_NEXT: u32 = 16;
    pub const L_AIRGAP: u32 = 28;
    pub const L_R: u32 = 31;
    pub const L_TOOLS: u32 = 34;
    pub const L_TOOL: u32 = 35;
    pub const L_LOGINS: u32 = 37;
    pub const L_FOR: u32 = 45;
    pub const L_MARK: u32 = 46;
    pub const L_WINS: u32 = 48;
    pub const L_PCT: u32 = 49;
    pub const L_USED: u32 = 51;
    pub const L_UNTIL: u32 = 52;
    pub const L_ROUND: u32 = 53;
    pub const L_STATE: u32 = 54;
    pub const L_NOTE: u32 = 55;
    pub const L_ERROR: u32 = 57;
    pub const L_SRC: u32 = 59;
    pub const L_LABEL: u32 = 60;
    pub const L_AGO: u32 = 61;
    pub const PARSE: u32 = 8;
    pub const AGO_M: u32 = 15;
    pub const UNTIL_M: u32 = 21;
}

/// The shell's Python on a 200's body. `now_us`: the time now, in
/// microseconds since the epoch (`limits`).
pub fn render(ask: &Ask, body: &[u8], now_us: i64) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let v = match json::parse(&raw) {
        Ok(v) => v,
        Err(e) => {
            let why = if raw.trim_matches(py_space).is_empty() {
                "Expecting value: line 1 column 1 (char 0)".to_string()
            } else {
                e.to_string()
            };
            return raised(
                String::new(),
                String::new(),
                &Raise::at(line::LOAD, format!("json.decoder.JSONDecodeError: {why}")),
            );
        }
    };
    let mut o = String::new();
    let r = match ask.sub.as_str() {
        "tokens" => tokens_lines(&v, ask.v("--by"), &mut o),
        "turns" => turns_lines(&v, &mut o),
        _ => limits_lines(&v, ask.v("--tool"), ask.v("--login"), now_us, &mut o),
    };
    match r {
        Ok(()) => Shown::out(o),
        Err(r) => raised(o, String::new(), &r),
    }
}

fn push_line(o: &mut String, text: &str) {
    o.push_str(text);
    o.push('\n');
}

/// `x or default`.
fn or(v: Option<&Value>, default: Value) -> Value {
    v.filter(|x| py::truthy(x)).cloned().unwrap_or(default)
}

/// `d.get(key)` where `d` is a dict for sure.
fn at(d: &Value, key: &str) -> Option<Value> {
    match d {
        Value::Object(pairs) => get(pairs, key).cloned(),
        _ => None,
    }
}

/// `str(d.get(key))` where `d` is a dict for sure.
fn str_at(d: &Value, key: &str) -> String {
    at(d, key).map_or_else(|| "None".into(), |v| py::str_of(&v))
}

/// `s.rjust(width)`.
fn rjust(text: &str, width: usize) -> String {
    let n = text.chars().count();
    if n < width {
        format!("{}{text}", " ".repeat(width - n))
    } else {
        text.to_string()
    }
}

/// A Python number as a float, for `%f`, `/` and `float()`.
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Int(d) => d.parse().ok(),
        Value::Float(f) => Some(*f),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

/// `"%.<places>f" % v`.
fn fixed(v: &Value, places: usize, at: Vec<(u32, &'static str)>) -> Result<String, Raise> {
    match number(v) {
        Some(f) if f.is_nan() => Ok("nan".into()),
        Some(f) if f.is_infinite() => Ok(if f > 0.0 { "inf" } else { "-inf" }.into()),
        Some(f) => Ok(format!("{f:.places$}")),
        None => Err(Raise {
            frames: at,
            exception: format!("TypeError: must be real number, not {}", py::type_name(v)),
        }),
    }
}

/// `x[:n]` on a value Python must slice.
fn head(v: &Value, n: usize, at: Vec<(u32, &'static str)>) -> Result<Value, Raise> {
    match v {
        Value::Str(t) => Ok(s(&t
            .code_points()
            .into_iter()
            .take(n)
            .map(|cp| py::str_of(&Value::Str(super::code_point(cp))))
            .collect::<String>())),
        Value::Array(items) => Ok(Value::Array(items.iter().take(n).cloned().collect())),
        Value::Object(_) => Err(Raise {
            frames: at,
            exception: format!("KeyError: slice(None, {n}, None)"),
        }),
        other => Err(Raise {
            frames: at,
            exception: format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        }),
    }
}

/// Python's `int(text)` for a string: spaces around, a sign, digits with
/// single underscores between them.
fn int_of_text(text: &str) -> Option<i128> {
    let t = text.trim_matches(py_space);
    let (neg, digits) = match t.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    let n: i128 = clean.parse().ok()?;
    Some(if neg { -n } else { n })
}

/// The `tokens` reader's `n(x)`, called from `from`.
fn n(x: Option<Value>, from: u32) -> Result<String, Raise> {
    let inside = |line: u32, e: String| Raise {
        frames: vec![(from, "<module>"), (line::T_LINE, "line"), (line, "n")],
        exception: e,
    };
    let x = x.filter(py::truthy).unwrap_or(Value::Int("0".into()));
    let v: i128 = match &x {
        Value::Int(d) => d.parse().unwrap_or(i128::MAX),
        Value::Bool(b) => i128::from(*b),
        Value::Float(f) if f.is_nan() => {
            return Err(inside(
                line::T_N,
                "ValueError: cannot convert float NaN to integer".into(),
            ))
        }
        Value::Float(f) if f.is_infinite() => {
            return Err(inside(
                line::T_N,
                "OverflowError: cannot convert float infinity to integer".into(),
            ))
        }
        Value::Float(f) => f.trunc() as i128,
        Value::Str(_) => match int_of_text(&py::str_of(&x)) {
            Some(v) => v,
            None => {
                return Err(inside(
                    line::T_N,
                    format!(
                        "ValueError: invalid literal for int() with base 10: {}",
                        py::repr(&x)
                    ),
                ))
            }
        },
        other => {
            return Err(inside(
                line::T_N,
                format!(
                    "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
                    py::type_name(other)
                ),
            ))
        }
    };
    for (unit, size) in [("B", 1e9), ("M", 1e6), ("k", 1e3)] {
        if v as f64 >= size {
            return Ok(format!("{:.1}{unit}", v as f64 / size));
        }
    }
    Ok(v.to_string())
}

/// The `tokens` reader's `line(label, t)`, called from `from`.
fn usage_line(label: &Value, t: &Value, from: u32) -> Result<String, Raise> {
    let here = |line: u32| vec![(from, "<module>"), (line, "line")];
    let label = head(label, 32, here(line::T_LINE))?;
    let Value::Object(_) = t else {
        return Err(Raise {
            frames: here(line::T_LINE),
            exception: format!(
                "AttributeError: '{}' object has no attribute 'get'",
                py::type_name(t)
            ),
        });
    };
    let req = n(at(t, "requests"), from)?;
    let inp = n(at(t, "input_tokens"), from)?;
    let outp = n(at(t, "output_tokens"), from)?;
    let cr = n(at(t, "cache_read_tokens"), from)?;
    let cw = n(at(t, "cache_write_tokens"), from)?;
    let cost = match at(t, "cost").filter(py::truthy) {
        None => String::new(),
        Some(c) => {
            let in_cost = |line: u32| {
                vec![
                    (from, "<module>"),
                    (line::T_LINE_COST, "line"),
                    (line, "cost"),
                ]
            };
            if !matches!(c, Value::Object(_)) {
                return Err(Raise {
                    frames: in_cost(line::T_USD),
                    exception: format!(
                        "AttributeError: '{}' object has no attribute 'get'",
                        py::type_name(&c)
                    ),
                });
            }
            let usd = at(&c, "usd").unwrap_or(Value::Int("0".into()));
            let mut text = format!("${}", fixed(&usd, 2, in_cost(line::T_USD))?);
            if let Some(u) = at(&c, "unpriced_requests").filter(py::truthy) {
                let d = py::percent_d(&u).map_err(|e| Raise {
                    frames: in_cost(line::T_UNPRICED),
                    exception: e,
                })?;
                text.push_str(&format!(" (+{d} unpriced)"));
            }
            text
        }
    };
    Ok(format!(
        "  {} {} req  in {}  out {}  cache r {}  w {}  {cost}",
        py_left(&label, 32),
        rjust(&req, 8),
        rjust(&inp, 7),
        rjust(&outp, 7),
        rjust(&cr, 7),
        rjust(&cw, 7)
    ))
}

/// `tokens`' reader; `by` is `--by` (its `sys.argv[1]`).
fn tokens_lines(r: &Value, by: &str, o: &mut String) -> Result<(), Raise> {
    let scope = or(py_get(r, "workspace", line::T_SCOPE)?, s("this server"));
    push_line(
        o,
        &format!(
            "Usage on {} (days in {})",
            py::str_of(&scope),
            str_at(r, "tz")
        ),
    );
    let t = or(at(r, "total").as_ref(), Value::Object(Vec::new()));
    push_line(o, &usage_line(&s("Total"), &t, line::T_TOTAL)?);
    if let Some(c) = py_get(&t, "cost", line::T_C)?.filter(|c| py::truthy(c)) {
        if !matches!(c, Value::Object(_)) {
            return Err(Raise::no_attribute(line::T_EST, c, "get"));
        }
        let usd = at(c, "usd").unwrap_or(Value::Int("0".into()));
        let usd = fixed(&usd, 2, vec![(line::T_EST, "<module>")])?;
        push_line(
            o,
            &format!(
                "  Estimated API-equivalent cost: ${usd} \u{2014} an estimate, not your bill."
            ),
        );
    }
    let groups = match at(r, "groups") {
        None | Some(Value::Null) => {
            let hs = or(at(r, "harnesses").as_ref(), Value::Array(Vec::new()));
            let mut out = Vec::new();
            for h in py_iter(&hs, line::T_HARN)? {
                let Value::Object(pairs) = &h else {
                    return Err(Raise::at(
                        line::T_HARN,
                        format!(
                            "TypeError: '{}' object is not subscriptable",
                            py::type_name(&h)
                        ),
                    ));
                };
                let Some(name) = get(pairs, "harness") else {
                    return Err(Raise::at(line::T_HARN, "KeyError: 'harness'"));
                };
                let mut merged = vec![(Text::from("label"), name.clone())];
                for (k, v) in pairs {
                    match merged.iter_mut().find(|(mk, _)| mk == k) {
                        Some(slot) => slot.1 = v.clone(),
                        None => merged.push((k.clone(), v.clone())),
                    }
                }
                out.push(Value::Object(merged));
            }
            Value::Array(out)
        }
        Some(g) => g,
    };
    if py::truthy(&groups) {
        let by = if by.is_empty() { "harness" } else { by };
        push_line(o, &format!("By {by}:"));
        for g in py_iter(&groups, line::T_GROUP)? {
            let label = match py_get(&g, "label", line::T_GROUP)?.filter(|x| py::truthy(x)) {
                Some(l) => l.clone(),
                None => or(at(&g, "key").as_ref(), s("-")),
            };
            push_line(o, &usage_line(&label, &g, line::T_GROUP)?);
        }
    }
    let days = or(at(r, "days").as_ref(), Value::Array(Vec::new()));
    let days = match days {
        Value::Array(items) => {
            let from = items.len().saturating_sub(14);
            Value::Array(items[from..].to_vec())
        }
        Value::Str(t) => {
            let cps = t.code_points();
            let from = cps.len().saturating_sub(14);
            s(&cps[from..]
                .iter()
                .map(|cp| py::str_of(&Value::Str(super::code_point(*cp))))
                .collect::<String>())
        }
        Value::Object(_) => {
            return Err(Raise::at(line::T_DAYS, "KeyError: slice(-14, None, None)"))
        }
        other => {
            return Err(Raise::at(
                line::T_DAYS,
                format!(
                    "TypeError: '{}' object is not subscriptable",
                    py::type_name(&other)
                ),
            ))
        }
    };
    if py::truthy(&days) {
        push_line(o, "Days:");
        for d in py_iter(&days, line::T_DAY)? {
            let day = match &d {
                Value::Object(pairs) => match get(pairs, "day") {
                    Some(x) => x.clone(),
                    None => return Err(Raise::at(line::T_DAY, "KeyError: 'day'")),
                },
                Value::Str(_) => {
                    return Err(Raise::at(
                        line::T_DAY,
                        "TypeError: string indices must be integers, not 'str'",
                    ))
                }
                other => {
                    return Err(Raise::at(
                        line::T_DAY,
                        format!(
                            "TypeError: '{}' object is not subscriptable",
                            py::type_name(other)
                        ),
                    ))
                }
            };
            push_line(o, &usage_line(&day, &d, line::T_DAY)?);
        }
    }
    push_line(o, "Input is uncached; output includes reasoning.");
    Ok(())
}

/// `turns`' `when` for a truthy `ms`.
fn when(ms: &Value) -> Result<String, Raise> {
    let secs = match ms {
        Value::Int(d) => d.parse::<f64>().unwrap_or(f64::INFINITY) / 1000.0,
        Value::Float(f) => f / 1000.0,
        Value::Bool(b) => f64::from(u8::from(*b)) / 1000.0,
        other => {
            return Err(Raise::at(
                line::U_WHEN,
                format!(
                    "TypeError: unsupported operand type(s) for /: '{}' and 'int'",
                    py::type_name(other)
                ),
            ))
        }
    };
    if secs.is_nan() {
        return Err(Raise::at(
            line::U_WHEN,
            "ValueError: Invalid value NaN (not a number)",
        ));
    }
    let w = secs.floor();
    if !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&w) {
        return Err(Raise::at(
            line::U_WHEN,
            "OverflowError: timestamp out of range for platform time_t",
        ));
    }
    crate::out::pylib::time::strftime_local(w as i64, "%Y-%m-%d %H:%M")
        .map_err(|e| Raise::at(line::U_WHEN, e))
}

/// `turns`' reader.
fn turns_lines(p: &Value, o: &mut String) -> Result<(), Raise> {
    let rows = or(py_get(p, "rows", line::U_ROWS)?, Value::Array(Vec::new()));
    if !py::truthy(&rows) {
        push_line(o, "(no requests)");
    }
    for r in py_iter(&rows, line::U_FOR)? {
        let ms = py_get(&r, "recorded_ms", line::U_MS)?.cloned();
        let when = match ms.filter(py::truthy) {
            Some(ms) => when(&ms)?,
            None => "-".into(),
        };
        let wsv = match at(&r, "workspace").filter(py::truthy) {
            Some(w) => w,
            None => or(at(&r, "workspace_path").as_ref(), s("-")),
        };
        let Value::Str(t) = &wsv else {
            return Err(Raise::no_attribute(line::U_WS, &wsv, "rstrip"));
        };
        let text = py::str_of(&Value::Str(t.clone()));
        let last = text.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        let ws = if last.is_empty() { "-" } else { last };
        let ws: String = ws.chars().take(18).collect();
        let cost = match at(&r, "cost_usd") {
            None | Some(Value::Null) => "unpriced".to_string(),
            Some(c) => format!("${}", fixed(&c, 4, vec![(line::U_COST, "<module>")])?),
        };
        let model = or(at(&r, "model").as_ref(), s("-"));
        let model = head(&model, 24, vec![(line::U_ROW, "<module>")])?;
        let val = |k: &str| at(&r, k).unwrap_or(Value::Null);
        push_line(
            o,
            &format!(
                "{when}  {} {} {} in {} out {} cr {} cw {} {cost}",
                py_left(&val("harness"), 6),
                py_left(&model, 24),
                ljust(&ws, 18),
                py_left(&val("input_tokens"), 7),
                py_left(&val("output_tokens"), 6),
                py_left(&val("cache_read_tokens"), 8),
                py_left(&val("cache_write_tokens"), 6),
            ),
        );
    }
    if let Some(c) = at(p, "next_cursor").filter(py::truthy) {
        push_line(
            o,
            &format!("older: {CLI} usage turns --before {}", py::str_of(&c)),
        );
    }
    Ok(())
}

/// A time `datetime.fromisoformat` reads (after `Z` became `+00:00`):
/// microseconds since the epoch, and whether it named an offset. `None`
/// for a text it refuses (`ValueError`).
pub fn iso_time(text: &str) -> Option<(i64, bool)> {
    let b = text.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let part = b.get(from..from + len)?;
        part.iter()
            .all(u8::is_ascii_digit)
            .then(|| std::str::from_utf8(part).ok()?.parse().ok())
            .flatten()
    };
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days_in = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&mo) || d < 1 || d > days_in[(mo - 1) as usize] || y < 1 {
        return None;
    }
    let (mut h, mut mi, mut sec, mut us, mut off, mut aware) = (0, 0, 0, 0i64, 0i64, false);
    let rest = &text[10..];
    if !rest.is_empty() {
        // One separator character, then the time.
        let mut chars = rest.chars();
        chars.next()?;
        let t = chars.as_str();
        let tb = t.as_bytes();
        let cut = tb
            .iter()
            .position(|c| *c == b'+' || *c == b'-')
            .unwrap_or(tb.len());
        let (clock, zone) = t.split_at(cut);
        let cb = clock.as_bytes();
        let two = |s: &[u8], at: usize| -> Option<i64> {
            let p = s.get(at..at + 2)?;
            p.iter()
                .all(u8::is_ascii_digit)
                .then(|| i64::from(p[0] - b'0') * 10 + i64::from(p[1] - b'0'))
        };
        h = two(cb, 0)?;
        match cb.len() {
            2 => {}
            5 if cb[2] == b':' => mi = two(cb, 3)?,
            n if n >= 8 && cb[2] == b':' && cb[5] == b':' => {
                mi = two(cb, 3)?;
                sec = two(cb, 6)?;
                if n > 8 {
                    if cb[8] != b'.' && cb[8] != b',' {
                        return None;
                    }
                    let frac = &cb[9..];
                    if frac.is_empty() || !frac.iter().all(u8::is_ascii_digit) {
                        return None;
                    }
                    let mut six: Vec<u8> = frac.iter().take(6).copied().collect();
                    six.resize(6, b'0');
                    us = std::str::from_utf8(&six).ok()?.parse().ok()?;
                }
            }
            _ => return None,
        }
        if h > 23 || mi > 59 || sec > 59 {
            return None;
        }
        if !zone.is_empty() {
            let zb = zone.as_bytes();
            let sign = if zb[0] == b'-' { -1 } else { 1 };
            let z = &zb[1..];
            let (zh, zm, zs) = match z.len() {
                2 => (two(z, 0)?, 0, 0),
                5 if z[2] == b':' => (two(z, 0)?, two(z, 3)?, 0),
                8 if z[2] == b':' && z[5] == b':' => (two(z, 0)?, two(z, 3)?, two(z, 6)?),
                _ => return None,
            };
            if zh > 23 || zm > 59 || zs > 59 {
                return None;
            }
            off = sign * (zh * 3600 + zm * 60 + zs);
            aware = true;
        }
    }
    // Days since the epoch (proleptic Gregorian).
    let (yy, mm) = if mo <= 2 {
        (y - 1, mo + 9)
    } else {
        (y, mo - 3)
    };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * mm + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86400 + h * 3600 + mi * 60 + sec - off;
    Some((secs * 1_000_000 + us, aware))
}

/// `limits`' `parse(s)` from `ago` or `until` (called at `from`, inside
/// `fun` at `fun_line`): the time, `None` for none.
fn parse_time(
    v: Option<Value>,
    from: u32,
    fun: &'static str,
    fun_line: u32,
) -> Result<Option<(i64, bool)>, Raise> {
    match v.filter(py::truthy) {
        None => Ok(None),
        Some(Value::Str(t)) => {
            let text = py::str_of(&Value::Str(t)).replace('Z', "+00:00");
            Ok(iso_time(&text))
        }
        Some(other) => Err(Raise {
            frames: vec![(from, "<module>"), (fun_line, fun), (line::PARSE, "parse")],
            exception: format!(
                "AttributeError: '{}' object has no attribute 'replace'",
                py::type_name(&other)
            ),
        }),
    }
}

fn naive(from: u32, fun: &'static str, at: u32) -> Raise {
    Raise {
        frames: vec![(from, "<module>"), (at, fun)],
        exception: "TypeError: can't subtract offset-naive and offset-aware datetimes".into(),
    }
}

/// `"%dm" % m if m < 120 else "%dh" % (m // 60)`.
fn minutes(m: i64) -> String {
    if m < 120 {
        format!("{m}m")
    } else {
        format!("{}h", m.div_euclid(60))
    }
}

/// Python's `round(x)` of a float: halves to even.
fn round_half_even(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 {
        2.0 * (x / 2.0).round()
    } else {
        r
    }
}

/// `limits`' reader; `want_tool` and `want_login` are `--tool` and
/// `--login`.
fn limits_lines(
    d: &Value,
    want_tool: &str,
    want_login: &str,
    now_us: i64,
    o: &mut String,
) -> Result<(), Raise> {
    if py_get(d, "airgap", line::L_AIRGAP)?.is_some_and(py::truthy) {
        push_line(o, "Air-gap mode is on: limits are not checked.");
    }
    let r = or(at(d, "refresh").as_ref(), Value::Object(Vec::new()));
    if let Some(after) = py_get(&r, "retryAfterSecs", line::L_R)?.filter(|x| py::truthy(x)) {
        let why = if at(&r, "outcome") == Some(s("too_soon")) {
            "read less than a minute ago"
        } else {
            "backing off after a failed check"
        };
        push_line(
            o,
            &format!("Not re-read: {why} (try in {}s).", py::str_of(after)),
        );
    }
    let tools = or(at(d, "tools").as_ref(), Value::Array(Vec::new()));
    for t in py_iter(&tools, line::L_TOOLS)? {
        if !want_tool.is_empty() && py_get(&t, "tool", line::L_TOOL)?.cloned() != Some(s(want_tool))
        {
            continue;
        }
        let all = or(
            py_get(&t, "logins", line::L_LOGINS)?,
            Value::Array(Vec::new()),
        );
        let mut logins = Vec::new();
        for l in py_iter(&all, line::L_LOGINS)? {
            let keep = want_login.is_empty()
                || py_get(&l, "accountId", line::L_LOGINS)?.cloned() == Some(s(want_login))
                || at(&l, "label") == Some(s(want_login));
            if keep {
                logins.push(l);
            }
        }
        if !at(&t, "supported").is_some_and(|x| py::truthy(&x)) {
            if !logins.is_empty() {
                push_line(o, &format!("{}: no limits API", str_at(&t, "display")));
            }
            continue;
        }
        push_line(o, &format!("{}:", str_at(&t, "display")));
        if logins.is_empty() {
            push_line(o, "  (not signed in)");
        }
        for l in &logins {
            if !matches!(l, Value::Object(_)) {
                return Err(Raise::no_attribute(line::L_MARK, l, "get"));
            }
            let mark = if at(l, "isDefault").is_some_and(|x| py::truthy(&x)) {
                "*"
            } else {
                " "
            };
            let mut wins: Vec<String> = Vec::new();
            let ws = or(at(l, "windows").as_ref(), Value::Array(Vec::new()));
            for w in py_iter(&ws, line::L_WINS)? {
                let pct = match py_get(&w, "usedPercent", line::L_PCT)? {
                    None | Some(Value::Null) => {
                        let used = or(at(&w, "used").as_ref(), Value::Int("0".into()));
                        let f = match &used {
                            Value::Str(_) => {
                                py::str_of(&used).trim_matches(py_space).parse::<f64>().ok()
                            }
                            other => number(other),
                        };
                        let Some(f) = f else {
                            return Err(Raise::at(
                                line::L_USED,
                                match &used {
                                    Value::Str(_) => format!(
                                        "ValueError: could not convert string to float: {}",
                                        py::repr(&used)
                                    ),
                                    other => format!(
                                        "TypeError: float() argument must be a string or a real number, not '{}'",
                                        py::type_name(other)
                                    ),
                                },
                            ));
                        };
                        Value::Float(f * 100.0)
                    }
                    Some(p) => p.clone(),
                };
                let u = match parse_time(at(&w, "resetsAt"), line::L_UNTIL, "until", 18)? {
                    None => String::new(),
                    Some((_, false)) => return Err(naive(line::L_UNTIL, "until", line::UNTIL_M)),
                    Some((t, true)) => {
                        let m = (t - now_us).div_euclid(60_000_000);
                        if m <= 0 {
                            "reset \u{2014} re-checking".into()
                        } else if m < 120 {
                            format!("resets in {m}m")
                        } else if m < 2880 {
                            format!("resets in {}h", m.div_euclid(60))
                        } else {
                            format!("resets in {}d", m.div_euclid(1440))
                        }
                    }
                };
                let rounded = match &pct {
                    Value::Int(d) => d.clone(),
                    Value::Bool(b) => u8::from(*b).to_string(),
                    Value::Float(f) if f.is_nan() => {
                        return Err(Raise::at(
                            line::L_ROUND,
                            "ValueError: cannot convert float NaN to integer",
                        ))
                    }
                    Value::Float(f) if f.is_infinite() => {
                        return Err(Raise::at(
                            line::L_ROUND,
                            "OverflowError: cannot convert float infinity to integer",
                        ))
                    }
                    Value::Float(f) => format!("{}", round_half_even(*f) as i128),
                    other => {
                        return Err(Raise::at(
                            line::L_ROUND,
                            format!(
                                "TypeError: type {} doesn't define __round__ method",
                                py::type_name(other)
                            ),
                        ))
                    }
                };
                let tail = if u.is_empty() {
                    String::new()
                } else {
                    format!(" ({u})")
                };
                wins.push(format!("{} {rounded}%{tail}", str_at(&w, "label")));
            }
            let state = or(at(l, "state").as_ref(), s("ok"));
            let mut note = match &state {
                Value::Str(_) => match py::str_of(&state).as_str() {
                    "not_signed_in" => "not signed in".to_string(),
                    "expired" => format!("sign-in needs a refresh ({CLI} llm tokens login)"),
                    "probe_failed" => "last check failed".into(),
                    "unsupported" => "no limits for this login".into(),
                    "airgap" => "off in air-gap mode".into(),
                    "no_window" => "no usage window".into(),
                    _ => String::new(),
                },
                Value::Array(_) | Value::Object(_) => {
                    return Err(Raise::at(
                        line::L_NOTE,
                        format!("TypeError: unhashable type: '{}'", py::type_name(&state)),
                    ))
                }
                _ => String::new(),
            };
            if state == s("probe_failed") {
                if let Some(e) = at(l, "error").filter(py::truthy) {
                    let Value::Str(t) = &e else {
                        return Err(Raise::at(
                            line::L_ERROR,
                            format!(
                                "TypeError: can only concatenate str (not \"{}\") to str",
                                py::type_name(&e)
                            ),
                        ));
                    };
                    note.push_str(": ");
                    note.push_str(&py::str_of(&Value::Str(t.clone())));
                }
            }
            let pinned = if at(l, "pinned").is_some_and(|x| py::truthy(&x)) {
                " [pinned]"
            } else {
                ""
            };
            let src = match or(at(l, "source").as_ref(), s("")) {
                Value::Str(t) => {
                    if py::str_of(&Value::Str(t)).starts_with("live") {
                        " \u{b7} live"
                    } else {
                        ""
                    }
                }
                other => return Err(Raise::no_attribute(line::L_SRC, &other, "startswith")),
            };
            let label = or(at(l, "label").as_ref(), s("-"));
            let label = head(&label, 22, vec![(line::L_LABEL, "<module>")])?;
            let joined = wins.join(" \u{b7} ");
            let joined = if joined.is_empty() {
                "-".to_string()
            } else {
                joined
            };
            push_line(
                o,
                &format!("{mark} {} {joined}{pinned}", py_left(&label, 22)),
            );
            let ago = match parse_time(at(l, "checkedAt"), line::L_AGO, "ago", 12)? {
                None => "never checked".to_string(),
                Some((_, false)) => return Err(naive(line::L_AGO, "ago", line::AGO_M)),
                Some((t, true)) => {
                    format!(
                        "updated {} ago",
                        minutes((now_us - t).div_euclid(60_000_000))
                    )
                }
            };
            let note = if note.is_empty() {
                String::new()
            } else {
                format!(" \u{b7} {note}")
            };
            push_line(o, &format!("    {ago}{src}{note}"));
        }
    }
    push_line(o, "* = the server default login.");
    Ok(())
}

#[cfg(test)]
mod tests;
