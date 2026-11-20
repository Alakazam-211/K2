//! `k5 front`, as the shell CLI's `cmd_front` answers it
//! (`cli/src/desktop.sh`, with `_front_render` in `cli/src/front.sh`):
//! the box's :443 front. Every form: `status` (also no verb), `enable
//! [--mail]`, `disable`, `http on|off`, `passthrough [list|ls]`,
//! `passthrough add`, `passthrough rm|remove`, each with `--json`, and
//! the help.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at
//!    once, then the virtual desktop's guard and the connection gate
//!    ([`crate::prelude::gate`]). A command line with `-h` or `--help`
//!    anywhere skips the guard; nothing skips the connection gate, the
//!    help neither (the shell's prelude has no `front` row).
//! 2. `front` is an `id` tool (`dns` in the shell's policy), so with
//!    `HOOK_SOCK` set: the stale-passport or require-passport lines, exit
//!    3 ([`crate::prelude::id_tool`]), the help too.
//! 3. The words ([`parse`]): `passthrough` and `http` read their own,
//!    any other verb the main loop's; then each verb's checks
//!    ([`request`]). Each problem is one `usage:` line, exit 2.
//! 4. One `_sidecar_send`, as [`super::job`] sends it.
//! 5. No answer is `daemon unreachable`, exit 1. Then:
//!    - `status`, `enable`, `disable`, `http`: `_front_render`. With
//!      `--json` the body as it came, exit 0 for 200, 3 for 403, else 1;
//!      else its Python ([`front_lines`]), which says what a refusal or a
//!      200 with `"ok": false` (or an `error` string and no `serving`)
//!      means, exit 3 for 403, else 1.
//!    - `passthrough`: with `--json` the body as it came and
//!      `_sidecar_exit_for`'s exit; a refusal is `_firewall_render`'s
//!      lines ([`super::firewall::error_lines`], which name `firewall`);
//!      else its Python ([`passthrough_lines`]).
//!
//!    No closing line and no bulletin: neither renderer notes anything.
//!
//! The shell CLI answers these itself ([`run`] returns `None`): a word
//! that is not UTF-8, `HOOK_SOCK` set with a `CODEX_` variable exported,
//! a port that is set but is no port curl takes (for a form that asks),
//! and a `python3` the binary can't tell.
//!
//! The shell hands the answer to its Python in an environment variable
//! (`K2_FR_BODY`, `K2_FP_BODY`), which Linux caps at 128 KiB: above it
//! the shell stops on python3's "Argument list too long", exit 126. This
//! prints the lines Python would have printed.
//!
//! Where the shell's Python raises, this prints a Python 3.12 traceback,
//! as [`super::sessions`] says; bytes that are not UTF-8 and half of a
//! surrogate pair are as [`super::inbox`] says.

use super::firewall::{self, at, dict_get, dumps, or, plus_str, push, Rendered};
use super::job::held;
use super::{curl_port, get, ljust, py_get, py_iter, py_space, raised, s, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Gate};
use crate::python;
use k2_client::brand::CLI_NAME as CLI;
use k2_client::{cli_name, product_name};
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["front"];
/// The binary answers every form ([`run`] may still hand one to the
/// shell CLI).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: the whole row is native.
pub const VERBS: &[crate::route::Verb] = &[];

/// The routes, under `/cli/front/`.
pub const ROUTE: &str = "/cli/front/";
/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "dns";
/// No answer.
pub const UNREACHABLE: &str = concat!(cli_name!(), " front: daemon unreachable");
/// What `_front_render`'s Python starts each error line with.
pub const ERROR: &str = concat!(cli_name!(), " front: error: ");
/// `http` with a word it doesn't take, or no `on` or `off`.
pub const HTTP_USAGE: &str = concat!("usage: ", cli_name!(), " front http on|off [--json]");
/// `passthrough add` with no host or no `--to`.
pub const ADD_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " front passthrough add <host>|'*' --to 127.0.0.1:<port> [--proxy-v2] [--label <why>]"
);
/// `passthrough rm` with no host.
pub const RM_USAGE: &str = concat!("usage: ", cli_name!(), " front passthrough rm <host>|'*'");
/// `passthrough` with a verb it doesn't have.
pub const PASSTHROUGH_USAGE: &str =
    concat!("usage: ", cli_name!(), " front passthrough [list|add|rm]");
/// `--to` at the end of the line.
pub const TO_USAGE: &str = "usage: --to 127.0.0.1:<port>";
/// `--label` at the end of the line.
pub const LABEL_USAGE: &str = "usage: --label <why>";
/// `disable --mail`.
pub const MAIL_USAGE: &str = "usage: --mail is for enable (disable takes everything back)";
/// The passthrough list with no names.
pub const NO_NAMES: &str = concat!(
    "no passthrough names (",
    cli_name!(),
    " front passthrough add <host> --to 127.0.0.1:<port>)"
);

/// A word the main loop doesn't take.
pub fn unknown_argument(word: &str) -> String {
    format!("usage: unknown argument '{word}' ({CLI} front --help)")
}

/// A flag `passthrough` doesn't have.
pub fn unknown_flag(flag: &str) -> String {
    format!("usage: unknown flag '{flag}' ({CLI} front --help)")
}

/// A verb `front` doesn't have.
pub fn unknown_verb(sub: &str) -> String {
    format!("usage: unknown front verb '{sub}' (status|enable|disable|http|passthrough)")
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `status`, `enable`, `disable`, or a verb `front` doesn't have.
    Front { sub: String, json: bool, mail: bool },
    /// `http`: the last `on` or `off`, empty for none.
    Http { mode: String, json: bool },
    /// `passthrough`: its verb (`list` for none, or a flag first), the
    /// last word that is no flag, and the flags.
    Passthrough {
        verb: String,
        json: bool,
        to: String,
        proxy: bool,
        label: String,
        host: String,
    },
}

/// What `cmd_front` does with its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// `_front_usage`: the help, exit 0.
    Help,
    /// What to print on stderr (with its newline), exit 2.
    Early(String),
    /// The verb's checks to make.
    Call(Ask),
}

fn early(line: &str) -> Parsed {
    Parsed::Early(format!("{line}\n"))
}

/// `cmd_front`'s words (after `front`).
pub fn parse(words: &[&str]) -> Parsed {
    let first = words.first().copied().unwrap_or("");
    let sub = if first.is_empty() { "status" } else { first };
    let rest = words.get(1..).unwrap_or_default();
    match sub {
        "help" | "--help" | "-h" => return Parsed::Help,
        "passthrough" => return passthrough_words(rest),
        "http" => return http_words(rest),
        _ => {}
    }
    let (mut json, mut mail) = (false, false);
    for a in rest {
        match *a {
            "--json" => json = true,
            "--mail" => mail = true,
            "-h" | "--help" => return Parsed::Help,
            _ => return early(&unknown_argument(a)),
        }
    }
    Parsed::Call(Ask::Front {
        sub: sub.to_string(),
        json,
        mail,
    })
}

/// `_front_http`'s words.
fn http_words(rest: &[&str]) -> Parsed {
    let (mut mode, mut json) = ("", false);
    for a in rest {
        match *a {
            "on" | "off" => mode = *a,
            "--json" => json = true,
            "-h" | "--help" => return Parsed::Help,
            _ => return early(HTTP_USAGE),
        }
    }
    Parsed::Call(Ask::Http {
        mode: mode.to_string(),
        json,
    })
}

/// `_front_passthrough`'s words: a first word that is a flag is `list`
/// and stays a word.
fn passthrough_words(rest: &[&str]) -> Parsed {
    let first = rest.first().copied().unwrap_or("");
    let (verb, rest) = if first.starts_with('-') {
        ("list", rest)
    } else {
        (
            if first.is_empty() { "list" } else { first },
            rest.get(1..).unwrap_or_default(),
        )
    };
    let (mut json, mut proxy) = (false, false);
    let (mut to, mut label, mut host) = (String::new(), String::new(), String::new());
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        match a {
            "--json" => json = true,
            "--proxy-v2" => proxy = true,
            "--to" | "--label" => {
                // `shift 2` fails with no value after the flag.
                let Some(value) = rest.get(i + 1) else {
                    return early(if a == "--to" { TO_USAGE } else { LABEL_USAGE });
                };
                if a == "--to" {
                    to = value.to_string();
                } else {
                    label = value.to_string();
                }
                i += 1;
            }
            "-h" | "--help" => return Parsed::Help,
            _ => {
                if let Some(v) = a.strip_prefix("--to=") {
                    to = v.to_string();
                } else if a.starts_with('-') {
                    return early(&unknown_flag(a));
                } else {
                    host = a.to_string();
                }
            }
        }
        i += 1;
    }
    Parsed::Call(Ask::Passthrough {
        verb: verb.to_string(),
        json,
        to,
        proxy,
        label,
        host,
    })
}

/// The one request a verb sends, and how its answer shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub get: bool,
    pub path: String,
    /// The query for a GET (none here), the JSON body for a POST.
    pub payload: String,
    pub json: bool,
    /// `passthrough`'s answer, read its own way.
    pub passthrough: bool,
}

impl Request {
    fn of(get: bool, route: &str, payload: impl Into<String>, json: bool, pt: bool) -> Request {
        Request {
            get,
            path: format!("{ROUTE}{route}"),
            payload: payload.into(),
            json,
            passthrough: pt,
        }
    }
}

/// The verb's checks, in the shell's order, and its request. `Err`: the
/// `usage:` line (with its newline), exit 2.
pub fn request(ask: &Ask) -> Result<Request, String> {
    let usage = |line: &str| Err(format!("{line}\n"));
    Ok(match ask {
        Ask::Front { sub, json, mail } => match sub.as_str() {
            "status" => Request::of(true, "status", "", *json, false),
            "enable" => {
                let body = if *mail {
                    r#"{"mail":true}"#
                } else {
                    r#"{"mail":false}"#
                };
                Request::of(false, "enable", body, *json, false)
            }
            "disable" => {
                if *mail {
                    return usage(MAIL_USAGE);
                }
                Request::of(false, "disable", "{}", *json, false)
            }
            _ => return usage(&unknown_verb(sub)),
        },
        Ask::Http { mode, json } => {
            let body = match mode.as_str() {
                "" => return usage(HTTP_USAGE),
                "on" => r#"{"on":true}"#,
                _ => r#"{"on":false}"#,
            };
            Request::of(false, "http", body, *json, false)
        }
        Ask::Passthrough {
            verb,
            json,
            to,
            proxy,
            label,
            host,
        } => match verb.as_str() {
            "list" | "ls" => Request::of(true, "passthrough", "", *json, true),
            "add" => {
                if host.is_empty() || to.is_empty() {
                    return usage(ADD_USAGE);
                }
                let body = dumps(vec![
                    ("host", s(host)),
                    ("to", s(to)),
                    ("proxyV2", Value::Bool(*proxy)),
                    ("label", s(label)),
                ]);
                Request::of(false, "passthrough/add", body, *json, true)
            }
            "rm" | "remove" => {
                if host.is_empty() {
                    return usage(RM_USAGE);
                }
                let body = dumps(vec![("host", s(host))]);
                Request::of(false, "passthrough/rm", body, *json, true)
            }
            _ => return usage(PASSTHROUGH_USAGE),
        },
    })
}

/// Whether a command line skips the virtual desktop's guard: `-h` or
/// `--help` anywhere in it (`_k2_offline_verb`).
fn offline_verb(words: &[&str]) -> bool {
    words.iter().any(|w| matches!(*w, "-h" | "--help"))
}

/// `k5 front ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
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
    let gate = prelude::gate(&settings);
    if !help && gate.is_none() && settings.port_text.as_deref().and_then(curl_port).is_none() {
        return None;
    }
    Some(run_with(
        args,
        &parsed,
        offline_verb(&all),
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
    // The guard, unless `-h` or `--help` is anywhere; then the connection
    // gate, the help's too.
    match prelude::gate(&settings) {
        Some(Gate::VirtualDesktop) if !offline => return Gate::VirtualDesktop.stop(),
        Some(_) => return Gate::CannotConnect.stop(),
        None => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let ask = match parsed {
        Parsed::Help => return Shown::out(crate::text::help("front").unwrap_or_default()).print(),
        Parsed::Early(text) => return Shown::err(text.clone(), Exit::Usage).print(),
        Parsed::Call(ask) => ask,
    };
    let req = match request(ask) {
        Ok(r) => r,
        Err(text) => return Shown::err(text, Exit::Usage).print(),
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
    let answer = if req.get {
        client.sidecar_get(&req.path, &req.payload)
    } else {
        client.sidecar_post(&req.path, &req.payload)
    };
    match held(answer) {
        Ok((status, body)) => render(&req, &status, &body).print(),
        Err(line) => {
            out::stderr_line(&line);
            Exit::Error
        }
    }
}

/// `_front_render`'s exit for a status that is not 200: 3 for 403, else
/// 1.
pub fn refusal_exit(status: &str) -> Exit {
    if status == "403" {
        Exit::Refused
    } else {
        Exit::Error
    }
}

/// What the request's answer prints: `_front_render`, or
/// `_front_passthrough`'s own end, on `_sidecar_send`'s status (`000`
/// for none) and body.
pub fn render(req: &Request, status: &str, body: &[u8]) -> Rendered {
    if status == "000" {
        return Rendered {
            stdout: Vec::new(),
            stderr: format!("{UNREACHABLE}\n"),
            exit: Exit::Error,
        };
    }
    if req.json {
        let mut stdout = body.to_vec();
        stdout.push(b'\n');
        let exit = match status {
            "200" => Exit::Ok,
            _ if req.passthrough => firewall::exit_for(status),
            _ => refusal_exit(status),
        };
        return Rendered {
            stdout,
            stderr: String::new(),
            exit,
        };
    }
    if !req.passthrough {
        return Rendered::of_shown(front_lines(status, body));
    }
    if status != "200" {
        return firewall::refusal(false, status, body);
    }
    Rendered::of_shown(passthrough_lines(body))
}

/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    /// `_front_render`'s Python.
    pub const R_OK: u32 = 8;
    pub const R_ERROR: u32 = 9;
    pub const R_MESSAGE: u32 = 11;
    pub const R_ASK: u32 = 15;
    pub const R_SOCKET: u32 = 21;
    pub const R_HTTP: u32 = 27;
    pub const R_HELPER: u32 = 28;
    pub const R_ROUTES: u32 = 31;
    pub const R_ROUTE: u32 = 32;
    pub const R_HOLDER: u32 = 34;
    pub const R_HOLDER_LINE: u32 = 35;
    pub const R_PASSTHROUGH: u32 = 38;
    pub const R_GUARD: u32 = 41;
    pub const R_CLOSED: u32 = 46;
    pub const R_STATE: u32 = 51;
    pub const R_UNIT: u32 = 52;
    pub const R_JOURNAL: u32 = 53;
    pub const R_WARNINGS: u32 = 68;
    /// `_front_passthrough`'s Python.
    pub const P_LOAD: u32 = 3;
    pub const P_ROWS: u32 = 4;
    pub const P_FOR: u32 = 7;
    pub const P_ROW: u32 = 8;
    pub const P_WARNINGS: u32 = 9;
    pub const P_WARNING: u32 = 10;
    pub const P_NOTE: u32 = 12;
}

/// `print(*parts)`: the parts with a space between them.
fn print(o: &mut String, parts: &[&str]) {
    push(o, &parts.join(" "));
}

fn on_off(v: Option<&Value>) -> &'static str {
    if v.is_some_and(py::truthy) {
        "on"
    } else {
        "off"
    }
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(py::truthy)
}

fn str_of(v: Option<&Value>) -> String {
    v.map_or_else(|| "None".into(), py::str_of)
}

/// The JSON decoder's traceback at `line`, as [`super::job`] prints it.
fn not_json(raw: &str, err: &json::Error, at: u32) -> Raise {
    let why = if raw.trim_matches(py_space).is_empty() {
        "Expecting value: line 1 column 1 (char 0)".to_string()
    } else {
        err.to_string()
    };
    Raise::at(at, format!("json.decoder.JSONDecodeError: {why}"))
}

/// `_front_render`'s Python on a status and its body: what a refusal
/// says (on stderr), else the front, its :80 or what a change did.
pub fn front_lines(status: &str, body: &[u8]) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let Ok(d) = json::parse(&raw) else {
        let head: String = raw.trim_matches(py_space).chars().take(200).collect();
        return Shown::err(
            format!("{ERROR}HTTP {status}: {head}\n"),
            refusal_exit(status),
        );
    };
    let (mut o, mut e) = (String::new(), String::new());
    match front_python(status, &d, &mut o, &mut e) {
        Ok(exit) => Shown {
            stdout: o,
            stderr: e,
            exit,
        },
        Err(r) => Shown {
            stdout: o,
            stderr: e + &r.text(),
            exit: Exit::Error,
        },
    }
}

fn front_python(status: &str, d: &Value, o: &mut String, e: &mut String) -> Result<Exit, Raise> {
    use line::*;
    let refused = status != "200" || {
        let ok = py_get(d, "ok", R_OK)?;
        ok == Some(&Value::Bool(false))
            || (matches!(dict_get(d, "error"), Some(Value::Str(_)))
                && dict_get(d, "serving").is_none())
    };
    if refused {
        let error = at(d, "error", R_ERROR)?;
        let hint = match &error {
            Value::Object(pairs) => get(pairs, "hint"),
            _ => None,
        };
        let message = [dict_get(d, "message"), hint]
            .into_iter()
            .flatten()
            .find(|m| py::truthy(m));
        let what = match &error {
            Value::Object(pairs) => str_of(get(pairs, "code")),
            other => py::str_of(other),
        };
        let tail = match message {
            Some(m) => format!(": {}", plus_str(m, R_MESSAGE)?),
            None => String::new(),
        };
        push(e, format!("{ERROR}{what}{tail}").as_str());
        if let Some(Value::Object(p)) = dict_get(d, "permission") {
            if let Some(command) = get(p, "command").filter(|c| py::truthy(c)) {
                push(
                    e,
                    format!("Ask with: {}", plus_str(command, R_ASK)?).as_str(),
                );
            } else if get(p, "requestable") == Some(&Value::Bool(false)) {
                push(e, "It can't be requested.");
            }
        }
        return Ok(refusal_exit(status));
    }
    let enabled = truthy(dict_get(d, "enabled"));
    let mail = if truthy(dict_get(d, "mail")) {
        "behind the front"
    } else {
        "not behind the front"
    };
    if dict_get(d, "serving").is_some() {
        let serving = truthy(dict_get(d, "serving"));
        let socket = if serving {
            let sock = or(dict_get(d, "socket"), Value::Object(Vec::new()));
            format!(
                "(socket from {})",
                py::str_of(&at(&sock, "source", R_SOCKET)?)
            )
        } else {
            String::new()
        };
        print(
            o,
            &[
                "front      :",
                on_off(dict_get(d, "enabled")),
                if serving {
                    "\u{b7} serving"
                } else {
                    "\u{b7} not serving"
                },
                socket.as_str(),
            ],
        );
        if truthy(dict_get(d, "restartNeeded")) {
            push(
                o,
                "note       : enabled but not serving: the daemon takes the socket at its next start",
            );
        }
        print(o, &["mail       :", mail]);
        let ht = or(dict_get(d, "http"), Value::Object(Vec::new()));
        if py::truthy(&ht) {
            let on = if py::truthy(&at(&ht, "on", R_HTTP)?) {
                "on"
            } else {
                "off"
            };
            let serving = if py::truthy(&at(&ht, "serving", R_HTTP)?) {
                "\u{b7} serving"
            } else {
                "\u{b7} not serving"
            };
            let next = at(&ht, "next", R_HTTP)?;
            let next = if py::truthy(&next) {
                format!("({})", plus_str(&next, R_HTTP)?)
            } else {
                String::new()
            };
            print(o, &[":80        :", on, serving, next.as_str()]);
            let helper = at(&ht, "helper", R_HELPER)?;
            if py::truthy(&helper) {
                print(o, &["note       :", py::str_of(&helper).as_str()]);
            }
        }
        print(
            o,
            &[
                "cap        :",
                str_of(dict_get(d, "cap")).as_str(),
                " (E2E listener",
                format!("{}; RLIMIT_NOFILE", str_of(dict_get(d, "e2eCap"))).as_str(),
                format!("{})", str_of(dict_get(d, "nofileSoftLimit"))).as_str(),
            ],
        );
        let routes = or(dict_get(d, "routes"), Value::Array(Vec::new()));
        for r in py_iter(&routes, R_ROUTES)? {
            let host = at(&r, "host", R_ROUTE)?;
            let kind = at(&r, "kind", R_ROUTE)?;
            let backend = at(&r, "backend", R_ROUTE)?;
            let proxy = if py::truthy(&at(&r, "proxyV2", R_ROUTE)?) {
                "PROXY v2"
            } else {
                "no PROXY"
            };
            print(
                o,
                &[
                    "route      :",
                    py::str_of(&host).as_str(),
                    "->",
                    py::str_of(&kind).as_str(),
                    py::str_of(&backend).as_str(),
                    proxy,
                ],
            );
        }
        let h = or(dict_get(d, "port443"), Value::Object(Vec::new()));
        let holder = at(&h, "holder", R_HOLDER)?;
        if py::truthy(&holder) && holder != s("free") {
            let message = at(&h, "message", R_HOLDER_LINE)?;
            print(
                o,
                &[
                    ":443       :",
                    py::str_of(&holder).as_str(),
                    "-",
                    py::str_of(&message).as_str(),
                ],
            );
        }
        if let Some(Value::Array(pt)) = dict_get(d, "passthrough") {
            if !pt.is_empty() && !enabled {
                let mut names = Vec::with_capacity(pt.len());
                for x in pt {
                    names.push(passthrough_name(x)?);
                }
                print(
                    o,
                    &[
                        "passthrough:",
                        names.join(", ").as_str(),
                        "(routes once the front is on)",
                    ],
                );
            }
        }
        let g = or(dict_get(d, "guard"), Value::Object(Vec::new()));
        if py::truthy(&g) {
            let refused = at(&g, "perIpRefused", R_GUARD)?;
            let limited = at(&g, "guardLimited", R_GUARD)?;
            print(
                o,
                &[
                    "App guard  :",
                    "refused connections",
                    py::str_of(&refused).as_str(),
                    "\u{b7} limited writes",
                    py::str_of(&limited).as_str(),
                ],
            );
        }
        if let Some(c @ Value::Object(_)) = dict_get(d, "connections") {
            let closed = or(dict_get(c, "closed"), Value::Object(Vec::new()));
            print(
                o,
                &[
                    "conns      : accepted",
                    str_of(dict_get(c, "accepted")).as_str(),
                    "\u{b7} live",
                    str_of(dict_get(c, "live")).as_str(),
                    "\u{b7} over cap",
                    str_of(dict_get(c, "overCap")).as_str(),
                ],
            );
            let Value::Object(pairs) = &closed else {
                return Err(Raise::no_attribute(R_CLOSED, &closed, "items"));
            };
            let parts: Vec<String> = pairs
                .iter()
                .filter(|(_, v)| py::truthy(v))
                .map(|(k, v)| format!("{} {}", py::str_of(&Value::Str(k.clone())), py::str_of(v)))
                .collect();
            print(o, &["closed     :", parts.join(", ").as_str()]);
        }
        if let Some(helper) = dict_get(d, "helper").filter(|v| py::truthy(v)) {
            print(o, &["helper     :", py::str_of(helper).as_str()]);
        }
        let su = or(dict_get(d, "socketUnit"), Value::Object(Vec::new()));
        if py::truthy(&su) {
            let st = or(Some(&at(&su, "state", R_STATE)?), Value::Object(Vec::new()));
            let active = at(&st, "ActiveState", R_UNIT)?;
            let sub = or(Some(&at(&st, "SubState", R_UNIT)?), s(""));
            let result = at(&st, "Result", R_UNIT)?;
            print(
                o,
                &[
                    "socket unit:",
                    py::str_of(&active).as_str(),
                    py::str_of(&sub).as_str(),
                    format!("({})", py::str_of(&result)).as_str(),
                ],
            );
            let journal = or(dict_get(&su, "journal"), Value::Array(Vec::new()));
            for line in py_iter(&journal, R_JOURNAL)? {
                print(o, &["  systemd  :", py::str_of(&line).as_str()]);
            }
        }
        return Ok(Exit::Ok);
    }
    if dict_get(d, "on").is_some() {
        print(o, &[":80 front  :", on_off(dict_get(d, "on"))]);
        if let Some(next) = dict_get(d, "next").filter(|v| py::truthy(v)) {
            print(o, &["next       :", py::str_of(next).as_str()]);
        }
        return Ok(Exit::Ok);
    }
    print(o, &["front      :", if enabled { "on" } else { "off" }]);
    if enabled {
        print(o, &["mail       :", mail]);
        if truthy(dict_get(d, "restarting")) {
            push(
                o,
                concat!(
                    "restarting : ",
                    product_name!(),
                    " will restart its daemon once to take port 443"
                ),
            );
        } else if truthy(dict_get(d, "restartNeeded")) {
            push(o, "restart    : needed to take port 443 (see warning)");
        }
    }
    let warnings = or(dict_get(d, "warnings"), Value::Array(Vec::new()));
    for w in py_iter(&warnings, R_WARNINGS)? {
        print(o, &["warning    :", py::str_of(&w).as_str()]);
    }
    if let Some(next) = dict_get(d, "next").filter(|v| py::truthy(v)) {
        print(o, &["next       :", py::str_of(next).as_str()]);
    }
    Ok(Exit::Ok)
}

/// One name of the passthrough list while the front is off,
/// `x.get("host") + " -> " + x.get("to")`, inside the generator.
fn passthrough_name(x: &Value) -> Result<String, Raise> {
    let inside = |exception: String| {
        Raise::inside(
            line::R_PASSTHROUGH,
            line::R_PASSTHROUGH,
            "<genexpr>",
            exception,
        )
    };
    let Value::Object(pairs) = x else {
        return Err(inside(format!(
            "AttributeError: '{}' object has no attribute 'get'",
            py::type_name(x)
        )));
    };
    let host = match get(pairs, "host") {
        Some(h @ Value::Str(_)) => py::str_of(h),
        Some(Value::Array(_)) => {
            return Err(inside(
                "TypeError: can only concatenate list (not \"str\") to list".into(),
            ))
        }
        other => {
            return Err(inside(format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                py::type_name(other.unwrap_or(&Value::Null))
            )))
        }
    };
    let to = get(pairs, "to").cloned().unwrap_or(Value::Null);
    let to = plus_str(&to, line::R_PASSTHROUGH).map_err(|r| inside(r.exception))?;
    Ok(format!("{host} -> {to}"))
}

/// `_front_passthrough`'s Python on a 200's body: the names, each with
/// where it goes, then the warnings and what comes next.
pub fn passthrough_lines(body: &[u8]) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let d = match json::parse(&raw) {
        Ok(v) => v,
        Err(err) => {
            return raised(
                String::new(),
                String::new(),
                &not_json(&raw, &err, line::P_LOAD),
            )
        }
    };
    let mut o = String::new();
    match passthrough_python(&d, &mut o) {
        Ok(()) => Shown::out(o),
        Err(r) => raised(o, String::new(), &r),
    }
}

fn passthrough_python(d: &Value, o: &mut String) -> Result<(), Raise> {
    use line::*;
    let rows = or(py_get(d, "passthrough", P_ROWS)?, Value::Array(Vec::new()));
    if !py::truthy(&rows) {
        push(o, NO_NAMES);
    }
    for r in py_iter(&rows, P_FOR)? {
        let host = at(&r, "host", P_ROW)?;
        let to = at(&r, "to", P_ROW)?;
        let proxy = if py::truthy(&at(&r, "proxyV2", P_ROW)?) {
            "  PROXY v2"
        } else {
            ""
        };
        let label = at(&r, "label", P_ROW)?;
        let label = if py::truthy(&label) {
            format!("  ({})", plus_str(&label, P_ROW)?)
        } else {
            String::new()
        };
        push(
            o,
            &format!(
                "{} -> {}{proxy}{label}",
                ljust(&py::str_of(&host), 32),
                py::str_of(&to)
            ),
        );
    }
    let warnings = or(dict_get(d, "warnings"), Value::Array(Vec::new()));
    for w in py_iter(&warnings, P_WARNINGS)? {
        push(o, &format!("warning: {}", plus_str(&w, P_WARNING)?));
    }
    if let Some(next) = dict_get(d, "next").filter(|v| py::truthy(v)) {
        if *next != s("routed now") {
            push(o, &format!("note: {}", plus_str(next, P_NOTE)?));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
