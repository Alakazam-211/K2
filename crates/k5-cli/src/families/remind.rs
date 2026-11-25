//! `k5 remind ...` and `k5 reminders ...`: one-time reminders (a
//! heartbeat that fires once), as the shell CLI's `cmd_heartbeat_remind`
//! does them (`cli/src/remind.sh`). `reminders` is `remind list`.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]).
//! 2. `remind` and `reminders` are the `heartbeat` tool, an `id` tool: in a
//!    session (`HOOK_SOCK` set) a stale passport and no passport at all
//!    are refused with exit 3 ([`crate::prelude::id_tool`]).
//! 3. The form ([`parse`]): `snooze` first; then, when no word before a
//!    bare `--` is `--in`, `--at` or `--on`, the verbs `list`, `show` and
//!    `cancel`; anything else sets a reminder, whose words after a bare
//!    `--` are its text, as typed. Each reads its words as the shell's
//!    loop does; a stop is its line (and for `add` and `list`, the usage
//!    line), exit 2.
//! 4. One request with `cli_send` (the cell socket first with a passport,
//!    `/cli/heartbeat/` being a route it serves): `add`, `snooze` and
//!    `cancel` are `cli_request_post`, `list` and `show` `cli_request`.
//!    The answer goes through `_remind_show_result` ([`checked`]): an
//!    empty answer, one that is not JSON, and an object with an `error`
//!    or `"ok": false` are `_hb_print_err`'s line and exit; the rest is
//!    the answer. Under `set -o pipefail` the reader's code wins.
//! 5. `--json` prints the answer and a newline; else the verb's Python
//!    ([`text`]); `cancel` says `Reminder <name> cancelled.`.
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! A request with no answer, and a POST on the cell socket that may have
//! been applied, is the transport's line, then `_k2_end_command` from
//! inside the pipe: the command dies of SIGPIPE ([`super::end_in_pipe`]).
//!
//! `remind` alone, or with an empty first word and no time, is the help
//! page (`cmd_help_remind`), exit 0, after the gates and the `id` tool's
//! checks ([`super::heartbeat::remind_alone`]).
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`): the
//! other help forms (`help`, and any `-h` or `--help`, which the front
//! sends to a page or the shell); a word that is not UTF-8; a port curl
//! won't take; a `python3` the binary can't tell; the shared Codex server
//! check; and a `--workspace` folder the binary can't resolve as bash's
//! `cd` would ([`super::workspace::register::path`]).
//!
//! Where this differs from the shell: as [`super::organization`] says
//! (an answer that is not UTF-8, a lone surrogate, Python 3.12's
//! tracebacks).

use self::shared::{at_line, dget, or, post_piped, sub};
use super::workspace::register::{self, Verb as RegisterVerb};
use super::{
    curl_port, get, hook_sock, pipefail, py_iter, py_left, raised, request_piped, Piped, Raise,
    Shown,
};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::encode::quote;
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["remind"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's tool policy.
pub const TOOL: &str = "heartbeat";

pub const ADD: &str = "/cli/heartbeat/reminder/add";
pub const LIST: &str = "/cli/heartbeat/reminder/list";
pub const SHOW: &str = "/cli/heartbeat/reminder/show";
pub const SNOOZE: &str = "/cli/heartbeat/reminder/snooze";
pub const CANCEL: &str = "/cli/heartbeat/reminder/cancel";

/// `_remind_add`'s usage line.
pub const ADD_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " heartbeat remind \"<text>\" (--in 45m | --at 13:00 | --on 2026-10-30 [--at 09:00]) [--tz Area/City] [--name n] [--file path] [--session pinned|own] [--json]"
);
pub const LIST_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " heartbeat remind list [--all] [--workspace <ws>] [--json]"
);
pub const SHOW_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " heartbeat remind show <name> [--json]"
);
pub const SNOOZE_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " heartbeat remind snooze <name> <10m> (or --in/--at/--on [--tz Area/City])"
);
pub const CANCEL_USAGE: &str = concat!("Usage: ", cli_name!(), " heartbeat remind cancel <name>");

/// The folder a `--workspace` value names, as `_remind_workspace_param`
/// resolves it (`None`: the shell's).
pub type Resolve<'a> = &'a dyn Fn(&str) -> Option<String>;

/// What the words ask for: the request, or the lines that stop it (exit 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    /// The help: the shell's (or the front's page).
    Help,
    Send(Ask),
    Stop(Vec<String>),
}

/// One request and how its answer is printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub verb: Kind,
    pub post: bool,
    pub endpoint: &'static str,
    /// `cli_request`'s arguments (each already escaped), after `project=`.
    pub params: Vec<String>,
    pub json: bool,
    /// `cancel`'s name, for its line.
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Add,
    List,
    Show,
    Snooze,
    Cancel,
}

fn stop(lines: &[&str]) -> Form {
    Form::Stop(lines.iter().map(|l| (*l).to_string()).collect())
}

/// `printf 'workspace=%s' "$(urlencode "$w")"`, `w` resolved.
fn workspace(w: &str, resolve: Resolve) -> Option<String> {
    Some(format!("workspace={}", quote(&resolve(w)?)))
}

/// The words after `remind` (for `reminders`, `list` first), as
/// `cmd_heartbeat_remind` reads them. `None`: the shell's (a
/// `--workspace` it can't resolve).
pub fn parse(words: &[&str], resolve: Resolve) -> Option<Form> {
    let first = words.first().copied().unwrap_or("");
    if first == "snooze" {
        return snooze(&words[1..], resolve);
    }
    // The time scan stops at a bare `--`: the words after it are text.
    let has_when = crate::dashdash::before(words)
        .iter()
        .any(|w| matches!(*w, "--in" | "--at" | "--on"));
    if !has_when {
        match first {
            "" | "-h" | "--help" | "help" => return Some(Form::Help),
            "list" => return list(&words[1..], resolve),
            "show" => return show(&words[1..], resolve),
            "cancel" => return cancel(&words[1..], resolve),
            _ => {}
        }
    }
    add(words, resolve)
}

fn add(words: &[&str], resolve: Resolve) -> Option<Form> {
    let (mut text, mut in_, mut at, mut on, mut tz, mut name, mut file, mut session) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    let mut ws = String::new();
    let mut json = false;
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "-h" | "--help" => return Some(Form::Help),
            "--in" | "--at" | "--on" | "--tz" | "--name" | "--file" | "--session"
            | "--workspace" | "--project" => {
                let Some(v) = words.get(i + 1) else {
                    return Some(stop(&[&format!("Error: {w} needs a value"), ADD_USAGE]));
                };
                let v = (*v).to_string();
                match w {
                    "--in" => in_ = v,
                    "--at" => at = v,
                    "--on" => on = v,
                    "--tz" => tz = v,
                    "--name" => name = v,
                    "--file" => file = v,
                    "--session" => session = v,
                    _ => ws = workspace(&v, resolve)?,
                }
                i += 2;
                continue;
            }
            "--json" => json = true,
            // The rest is the reminder's text, as typed.
            "--" => {
                for w in &words[i + 1..] {
                    super::msg::postal::join(&mut text, w);
                }
                break;
            }
            w if w.starts_with("--") => {
                return Some(stop(&[&format!("Error: unknown flag: {w}"), ADD_USAGE]))
            }
            w => super::msg::postal::join(&mut text, w),
        }
        i += 1;
    }
    if text.is_empty() && file.is_empty() {
        return Some(stop(&[
            "Error: a reminder needs some text, or --file",
            ADD_USAGE,
        ]));
    }
    if in_.is_empty() && at.is_empty() && on.is_empty() {
        return Some(stop(&[
            "Error: say when: --in 45m, --at 13:00 or --on 2026-10-30",
            ADD_USAGE,
        ]));
    }
    let mut p = format!("text={}", quote(&text));
    for (key, v) in [
        ("in", &in_),
        ("at", &at),
        ("on", &on),
        ("tz", &tz),
        ("name", &name),
        ("file", &file),
        ("session", &session),
    ] {
        if !v.is_empty() {
            p.push_str(&format!("&{key}={}", quote(v)));
        }
    }
    if !ws.is_empty() {
        p.push('&');
        p.push_str(&ws);
    }
    Some(Form::Send(Ask {
        verb: Kind::Add,
        post: true,
        endpoint: ADD,
        params: vec![p],
        json,
        name: String::new(),
    }))
}

fn list(words: &[&str], resolve: Resolve) -> Option<Form> {
    let (mut json, mut all, mut ws) = (false, false, String::new());
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "-h" | "--help" => return Some(Form::Help),
            "--json" => json = true,
            "--all" => all = true,
            w @ ("--workspace" | "--project") => {
                let Some(v) = words.get(i + 1) else {
                    return Some(stop(&[&format!("Error: {w} needs a value")]));
                };
                ws = workspace(v, resolve)?;
                i += 2;
                continue;
            }
            w => {
                return Some(stop(&[
                    &format!("Error: unknown argument: {w}"),
                    LIST_USAGE,
                ]))
            }
        }
        i += 1;
    }
    let mut params = Vec::new();
    if all {
        params.push("all=1".to_string());
    }
    if !ws.is_empty() {
        params.push(ws);
    }
    Some(Form::Send(Ask {
        verb: Kind::List,
        post: false,
        endpoint: LIST,
        params,
        json,
        name: String::new(),
    }))
}

/// `show` and `cancel`: the first word is the name, `--json` and
/// `--workspace` (or `--project`) the flags.
fn named(words: &[&str], resolve: Resolve) -> Option<Result<(String, bool, String), Form>> {
    let (mut name, mut json, mut ws) = (String::new(), false, String::new());
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "-h" | "--help" => return Some(Err(Form::Help)),
            "--json" => json = true,
            w @ ("--workspace" | "--project") => {
                let Some(v) = words.get(i + 1) else {
                    return Some(Err(stop(&[&format!("Error: {w} needs a value")])));
                };
                ws = workspace(v, resolve)?;
                i += 2;
                continue;
            }
            w if w.starts_with("--") => {
                return Some(Err(stop(&[&format!("Error: unknown flag: {w}")])))
            }
            w => {
                if name.is_empty() {
                    name = w.to_string();
                }
            }
        }
        i += 1;
    }
    Some(Ok((name, json, ws)))
}

fn show(words: &[&str], resolve: Resolve) -> Option<Form> {
    let (name, json, ws) = match named(words, resolve)? {
        Ok(n) => n,
        Err(form) => return Some(form),
    };
    if name.is_empty() {
        return Some(stop(&[SHOW_USAGE]));
    }
    let mut params = vec![format!("name={}", quote(&name))];
    if !ws.is_empty() {
        params.push(ws);
    }
    Some(Form::Send(Ask {
        verb: Kind::Show,
        post: false,
        endpoint: SHOW,
        params,
        json,
        name,
    }))
}

fn cancel(words: &[&str], resolve: Resolve) -> Option<Form> {
    let (name, json, ws) = match named(words, resolve)? {
        Ok(n) => n,
        Err(form) => return Some(form),
    };
    if name.is_empty() {
        return Some(stop(&[CANCEL_USAGE]));
    }
    let mut p = format!("name={}", quote(&name));
    if !ws.is_empty() {
        p.push('&');
        p.push_str(&ws);
    }
    Some(Form::Send(Ask {
        verb: Kind::Cancel,
        post: true,
        endpoint: CANCEL,
        params: vec![p],
        json,
        name,
    }))
}

fn snooze(words: &[&str], resolve: Resolve) -> Option<Form> {
    let (mut name, mut when, mut in_, mut at, mut on, mut tz, mut ws) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    let mut json = false;
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "-h" | "--help" => return Some(Form::Help),
            "--in" | "--at" | "--on" | "--tz" | "--workspace" | "--project" => {
                let Some(v) = words.get(i + 1) else {
                    return Some(stop(&[&format!("Error: {w} needs a value")]));
                };
                let v = (*v).to_string();
                match w {
                    "--in" => in_ = v,
                    "--at" => at = v,
                    "--on" => on = v,
                    "--tz" => tz = v,
                    _ => ws = workspace(&v, resolve)?,
                }
                i += 2;
                continue;
            }
            "--json" => json = true,
            w if w.starts_with("--") => return Some(stop(&[&format!("Error: unknown flag: {w}")])),
            w => {
                if name.is_empty() {
                    name = w.to_string();
                } else if when.is_empty() {
                    when = w.to_string();
                } else {
                    return Some(stop(&[&format!("Error: unexpected argument: {w}")]));
                }
            }
        }
        i += 1;
    }
    if in_.is_empty() {
        in_ = when;
    }
    if name.is_empty() || (in_.is_empty() && at.is_empty() && on.is_empty()) {
        return Some(stop(&[SNOOZE_USAGE]));
    }
    let mut p = format!("name={}", quote(&name));
    for (key, v) in [("in", &in_), ("at", &at), ("on", &on), ("tz", &tz)] {
        if !v.is_empty() {
            p.push_str(&format!("&{key}={}", quote(v)));
        }
    }
    if !ws.is_empty() {
        p.push('&');
        p.push_str(&ws);
    }
    Some(Form::Send(Ask {
        verb: Kind::Snooze,
        post: true,
        endpoint: SNOOZE,
        params: vec![p],
        json,
        name,
    }))
}

/// `k5 remind ...` and `k5 reminders ...`: `args` is the whole command
/// line after `argv[0]`. `None`: the shell CLI answers it (see the
/// module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let all: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    let (noun, rest) = all.split_first()?;
    let mut words: Vec<&str> = Vec::new();
    if *noun == "reminders" {
        words.push("list");
    }
    words.extend(rest);
    let pwd = daemon::logical_cwd(std::env::var_os("PWD")).ok()?;
    let pwd = pwd.to_str()?.to_string();
    let resolve = |w: &str| register::path(RegisterVerb::Open, w, &pwd, &super::sim::cd_moved);
    let form = parse(&words, &resolve)?;
    // `remind` alone (or with an empty first word, and no time) is the
    // help page, after the gates ([`run_with`]), as `heartbeat remind`
    // alone is; every other help form is the front's page or the shell's.
    if form == Form::Help && !super::heartbeat::remind_alone(&all) {
        return None;
    }
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
    Some(run_with(args, form, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    form: Form,
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
    let ask = match form {
        Form::Send(ask) => ask,
        Form::Stop(lines) => {
            for l in lines {
                out::stderr_line(&l);
            }
            return Exit::Usage;
        }
        // `remind` alone (`run` hands every other help form on):
        // `cmd_help_remind`, exit 0, nothing sent.
        Form::Help => {
            return match crate::text::help("remind") {
                Some(page) => Shown::out(page).print(),
                None => Exit::Usage,
            }
        }
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
    let params: Vec<&str> = ask.params.iter().map(String::as_str).collect();
    let piped = if ask.post {
        post_piped(&mut client, &mut closing, ask.endpoint, &params)
    } else {
        request_piped(&mut client, &mut closing, ask.endpoint, &params)
    };
    let exit = answer(&ask, &piped).print();
    closing.flush(&mut client, exit);
    exit
}

/// What `$(…)` holds of a body: trailing newlines gone (the NULs went
/// before).
fn held(body: &[u8]) -> String {
    let mut b = body.to_vec();
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    String::from_utf8_lossy(&b).into_owned()
}

/// `_remind_show_result` on what the reader read: `Ok(resp)` to go on, or
/// `_hb_print_err`'s output.
pub fn checked(body: &[u8]) -> Result<String, Shown> {
    let resp = held(body);
    if resp.is_empty() {
        return Err(print_err(&resp));
    }
    let failed = match json::parse(&resp) {
        Err(_) => true,
        Ok(Value::Object(pairs)) => {
            get(&pairs, "error").is_some() || get(&pairs, "ok") == Some(&Value::Bool(false))
        }
        Ok(_) => false,
    };
    if failed {
        Err(print_err(&resp))
    } else {
        Ok(resp)
    }
}

/// `_hb_print_err`'s lines (`cli/src/heartbeat.sh`) a traceback names.
pub mod err_line {
    pub const HINT_CHECK: u32 = 16;
    pub const HINT_JOIN: u32 = 17;
    pub const PRINT_HINT: u32 = 18;
}

/// `_hb_print_err` on `raw`: its line on stderr and its exit.
pub fn print_err(raw: &str) -> Shown {
    let raw = raw.trim_matches(super::py_space);
    if raw.is_empty() {
        return Shown::err(
            "Error: empty response from daemon (is it running?)\n",
            Exit::Error,
        );
    }
    let d = match json::parse(raw) {
        Ok(d) => d,
        Err(_) => {
            let head: String = raw.chars().take(200).collect();
            return Shown::err(format!("Error: {head}\n"), Exit::Error);
        }
    };
    let err = match &d {
        Value::Object(pairs) => get(pairs, "error").cloned().unwrap_or_else(|| d.clone()),
        _ => super::s(raw),
    };
    if let Value::Object(ep) = &err {
        let code = get(ep, "code")
            .filter(|c| py::truthy(c))
            .cloned()
            .unwrap_or_else(|| super::s("error"));
        let mut hint = [get(ep, "hint"), get(ep, "message")]
            .into_iter()
            .flatten()
            .find(|x| py::truthy(x))
            .cloned()
            .unwrap_or_else(|| code.clone());
        let gated = ["owner_only", "forbidden", "gated"]
            .iter()
            .any(|w| code == super::s(w));
        if gated {
            let has = match &hint {
                Value::Str(_) => py::str_of(&hint).contains("ask your human"),
                Value::Array(items) => items.contains(&super::s("ask your human")),
                Value::Object(pairs) => get(pairs, "ask your human").is_some(),
                other => {
                    return raised(
                        String::new(),
                        String::new(),
                        &Raise::at(
                            err_line::HINT_CHECK,
                            format!(
                                "TypeError: argument of type '{}' is not iterable",
                                py::type_name(other)
                            ),
                        ),
                    )
                }
            };
            if !has {
                let Value::Str(_) = hint else {
                    let e = if matches!(hint, Value::Array(_)) {
                        "TypeError: can only concatenate list (not \"str\") to list".to_string()
                    } else {
                        format!(
                            "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                            py::type_name(&hint)
                        )
                    };
                    return raised(
                        String::new(),
                        String::new(),
                        &Raise::at(err_line::HINT_JOIN, e),
                    );
                };
                hint = super::s(&format!(
                    "{} — requires owner/admin — ask your human",
                    py::str_of(&hint)
                ));
            }
        }
        let Value::Str(_) = hint else {
            return raised(
                String::new(),
                String::new(),
                &Raise::at(
                    err_line::PRINT_HINT,
                    format!(
                        "TypeError: can only concatenate str (not \"{}\") to str",
                        py::type_name(&hint)
                    ),
                ),
            );
        };
        let exit = if gated {
            Exit::Refused
        } else if code == super::s("usage") {
            Exit::Usage
        } else {
            Exit::Error
        };
        return Shown::err(format!("Error: {}\n", py::str_of(&hint)), exit);
    }
    let exit = match &err {
        Value::Str(_) if py::str_of(&err).to_lowercase().contains("token") => Exit::Refused,
        _ => Exit::Error,
    };
    Shown::err(format!("Error: {}\n", py::str_of(&err)), exit)
}

/// The command's output once the request is done: `piped` is the left
/// side of the pipe.
pub fn answer(ask: &Ask, piped: &Piped) -> Shown {
    let resp = match checked(&piped.body) {
        Ok(resp) => resp,
        Err(mut shown) => {
            shown.exit = pipefail(piped.exit, shown.exit);
            return shown;
        }
    };
    // The reader printed it and ended with 0: the call's code.
    if piped.exit != Exit::Ok {
        return Shown {
            stdout: String::new(),
            stderr: String::new(),
            exit: piped.exit,
        };
    }
    if ask.json {
        return Shown::out(format!("{resp}\n"));
    }
    if ask.verb == Kind::Cancel {
        return Shown::out(format!("Reminder {} cancelled.\n", ask.name));
    }
    text(ask.verb, &resp)
}

/// The verbs' Python lines a traceback names.
pub mod line {
    pub const ADD_LOAD: u32 = 3;
    pub const ADD_NAME: u32 = 4;
    pub const ADD_FIRES: u32 = 5;
    pub const LIST_FOR: u32 = 7;
    pub const LIST_FIRE: u32 = 8;
    pub const LIST_TEXT: u32 = 10;
    pub const LIST_LEN: u32 = 11;
    pub const LIST_ROW: u32 = 12;
    pub const SHOW_HEAD: u32 = 4;
    pub const SHOW_FIRES: u32 = 5;
    pub const SHOW_TEXT: u32 = 6;
    pub const SHOW_BY: u32 = 7;
    pub const SHOW_FIRED: u32 = 8;
    pub const SHOW_WAIT: u32 = 9;
    pub const SHOW_HISTORY: u32 = 10;
    pub const SHOW_FOR: u32 = 13;
    pub const SHOW_ROW: u32 = 14;
    pub const SNOOZE_ROW: u32 = 4;
}

type R<T> = Result<T, Raise>;

/// `v.get(key)` on a dict, `None` (null) when it has no such key.
fn field(v: &Value, key: &str) -> Value {
    match v {
        Value::Object(pairs) => get(pairs, key).cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// The verb's Python on `resp` (what `$(…)` held).
pub fn text(verb: Kind, resp: &str) -> Shown {
    let mut out = String::new();
    let r = (|| -> R<()> {
        // `_remind_show_result` passed it on: it is JSON.
        let d = json::parse(resp).map_err(|_| {
            Raise::at(
                line::ADD_LOAD,
                "json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)",
            )
        })?;
        match verb {
            Kind::Add => {
                let n = sub(&d, "name", &at_line(line::ADD_NAME))?;
                let fires = sub(&d, "fires", &at_line(line::ADD_FIRES))?;
                let n = py::str_of(n);
                out.push_str(&format!("Reminder {n} set: {}.\n", py::str_of(fires)));
                out.push_str(&format!(
                    concat!(
                        "  Cancel: ",
                        cli_name!(),
                        " heartbeat remind cancel {}   Snooze: ",
                        cli_name!(),
                        " heartbeat remind snooze {} <when>\n"
                    ),
                    n, n
                ));
            }
            Kind::Snooze => {
                let n = sub(&d, "name", &at_line(line::SNOOZE_ROW))?;
                let fires = sub(&d, "fires", &at_line(line::SNOOZE_ROW))?;
                out.push_str(&format!(
                    "Reminder {} now fires {}.\n",
                    py::str_of(n),
                    py::str_of(fires)
                ));
            }
            Kind::List => {
                if !py::truthy(&d) {
                    out.push_str(concat!(
                        "No reminders. Set one: ",
                        cli_name!(),
                        " heartbeat remind \"text\" --in 45m\n"
                    ));
                    return Ok(());
                }
                for r in py_iter(&d, line::LIST_FOR)? {
                    let when = or(
                        dget(&r, "fireAt", &at_line(line::LIST_FIRE))?,
                        super::s("?"),
                    );
                    let state = field(&r, "state");
                    let t = or(Some(&field(&r, "text")), super::s(""));
                    let t = match &t {
                        Value::Str(_) => py::str_of(&t).replace('\n', " "),
                        other => {
                            return Err(Raise::no_attribute(line::LIST_TEXT, other, "replace"))
                        }
                    };
                    let t = if t.chars().count() > 70 {
                        let head: String = t.chars().take(67).collect();
                        format!("{head}...")
                    } else {
                        t
                    };
                    let name = sub(&r, "name", &at_line(line::LIST_ROW))?;
                    out.push_str(&format!(
                        "{} {} {}  {t}\n",
                        py_left(name, 22),
                        py_left(&state, 10),
                        py::str_of(&when)
                    ));
                }
            }
            Kind::Show => {
                let name = sub(&d, "name", &at_line(line::SHOW_HEAD))?;
                let state = sub(&d, "state", &at_line(line::SHOW_HEAD))?;
                out.push_str(&format!(
                    "Reminder {} ({})\n",
                    py::str_of(name),
                    py::str_of(state)
                ));
                let g = |k: &str| field(&d, k);
                let tz = if py::truthy(&g("tz")) {
                    format!(
                        " ({})",
                        py::str_of(sub(&d, "tz", &at_line(line::SHOW_FIRES))?)
                    )
                } else {
                    String::new()
                };
                out.push_str(&format!("  fires:   {}{tz}\n", py::str_of(&g("fireAt"))));
                out.push_str(&format!(
                    "  text:    {}\n",
                    py::str_of(&or(Some(&g("text")), super::s("")))
                ));
                out.push_str(&format!(
                    "  set by:  {}\n",
                    py::str_of(&or(Some(&g("createdBy")), super::s("?")))
                ));
                if py::truthy(&g("firedAt")) {
                    out.push_str(&format!("  fired:   {}\n", py::str_of(&g("firedAt"))));
                }
                if py::truthy(&g("waitReason")) {
                    let detail = if py::truthy(&g("waitDetail")) {
                        format!(" ({})", py::str_of(&g("waitDetail")))
                    } else {
                        String::new()
                    };
                    out.push_str(&format!(
                        "  waiting: {}{detail}\n",
                        py::str_of(&g("waitReason"))
                    ));
                }
                let fires = or(Some(&g("fires")), Value::Array(Vec::new()));
                if py::truthy(&fires) {
                    out.push_str("  history:\n");
                    for f in py_iter(&fires, line::SHOW_FOR)? {
                        let fallback = dget(&f, "fired_at", &at_line(line::SHOW_ROW))?
                            .cloned()
                            .unwrap_or_else(|| super::s(""));
                        let at = dget(&f, "firedAt", &at_line(line::SHOW_ROW))?
                            .cloned()
                            .unwrap_or(fallback);
                        let decision = dget(&f, "decision", &at_line(line::SHOW_ROW))?
                            .cloned()
                            .unwrap_or_else(|| super::s(""));
                        let reason =
                            or(dget(&f, "reason", &at_line(line::SHOW_ROW))?, super::s(""));
                        out.push_str(&format!(
                            "    {}  {}  {}\n",
                            py::str_of(&at),
                            py::str_of(&decision),
                            py::str_of(&reason)
                        ));
                    }
                }
            }
            Kind::Cancel => {}
        }
        Ok(())
    })();
    match r {
        Ok(()) => Shown::out(out),
        Err(e) => raised(out, String::new(), &e),
    }
}

pub mod shared;

#[cfg(test)]
mod tests;
