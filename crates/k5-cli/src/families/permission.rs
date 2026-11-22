//! `k5 permission ...`, the permission cards family (Beta), as the shell
//! CLI's `cmd_permission` answers it (`cli/src/permission.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]). `permission` is no tool in
//!    the shell's policy, so it is an open one: no passport check.
//! 2. Whether the family exists for this caller
//!    (`_permission_family_available`):
//!    - with a passport, the pages' probe ([`crate::cards`]): one `GET
//!      /cli/permission/enabled` on the cell socket, else over TCP;
//!    - without one, `beta` exists with nothing asked;
//!    - else the same GET over TCP with the owner token ([`OwnerProbe`]).
//!
//!    On only for a 200 whose JSON says `"enabled": true` (a refused try
//!    first is off, as the cards probe reads it). Off, the command is
//!    unknown: `Unknown command: permission` on stderr, then the daily tier
//!    on stdout with cards off (the shell's cached answer) and the
//!    send-feedback block as `_sf_agents_off_here` decides
//!    ([`crate::feedback_optin`]), exit 1.
//! 3. In a session with a passport, the passport is the TCP token too (the
//!    shell's `TOKEN="$SCOPED_TOKEN"`): the client never holds both.
//! 4. The verb ([`parse`]), then its requests: `cli_send_json`
//!    ([`Client::send_json`]), the cell socket first with a passport. A
//!    POST that went out there and got no usable answer is the shell's
//!    "may have been applied" line, never sent again over TCP.
//! 5. `_permission_print` ([`print_answer`]) on `<body>\n<status>`, or
//!    the verb's own reader (`beta status`, `approvers`, the `pending`
//!    lookup of `approve` and `deny`).
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The shell's Python reads the answer with its locale; this reads bytes
//! that are not UTF-8 with U+FFFD for each, and prints half of a surrogate
//! pair as `\udXXX` where Python's `print` fails. Python's `int()` takes
//! digits of any script; this takes ASCII digits only. The daemon sends
//! none of these.
//!
//! What goes to the shell: a word that is not UTF-8, a port that is set
//! but is no port curl takes, a `python3` the binary can't tell, and a
//! probe the binary can't be sure to send as the shell does (the rules of
//! [`crate::cards::ask`]).

use super::feedback::py_int;
use super::{curl_port, get, py_space, Raise, Shown};
use crate::cards::{self, Decode, Reader};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::feedback_optin;
use crate::offline::{self, Env};
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python::{self, Python3};
use crate::transport::encode::quote;
use crate::transport::{Auth, Client, Error, Response};
use k2_client::{cli_name, product_name};
use std::ffi::OsString;
use std::net::Ipv4Addr;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["permission"];
/// Every form is the family's; a line with `-h` or `--help` is the
/// front's (a help page, or the shell's).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The routes.
pub const REQUEST: &str = "/cli/permission/request";
pub const UPDATE: &str = "/cli/permission/update";
pub const STATUS: &str = "/cli/permission/status";
pub const CANCEL: &str = "/cli/permission/cancel";
pub const LIST: &str = "/cli/permission/list";
pub const PENDING: &str = "/cli/permission/pending";
pub const ANSWER: &str = "/cli/thread/answer";
pub const SETTINGS_UPDATE: &str = "/cli/settings/update";
pub const SETTINGS_GET: &str = "/cli/settings/get";
pub const AGENT_ACCESS: &str = "/cli/agent-access/set";

/// `_permission_usage`.
pub const USAGE: &str = concat!(
    "Usage: ", cli_name!(), " permission <command> (Beta)\n",
    "\n",
    "Ask a person to allow what ",
    product_name!(),
    " refused you. Every refusal ends with the\n",
    "exact command to run (\"Ask with: \u{2026}\"), or \"It can't be requested.\"\n",
    "\n",
    "Agents (in a ",
    product_name!(),
    " terminal):\n",
    "  ", cli_name!(), " permission request <action> [key=value \u{2026}] --why \"<one line>\" [--to <login>[,<login>\u{2026}]] [--dry-run]\n",
    "                                                   --to: only these people are asked (owners or\n",
    "                                                   admins who may answer, or owner); else the\n",
    "                                                   owner's default, else everyone who may answer\n",
    "                                                   (--assign is the same flag, as on ", cli_name!(), " app)\n",
    "  ", cli_name!(), " permission update <id> --why \"<new facts>\"   once, on a denied request\n",
    "  ", cli_name!(), " permission status [<id>]                      check once; never loop\n",
    "  ", cli_name!(), " permission cancel <id>\n",
    "  ", cli_name!(), " permission list [--area <area>] [--search <word>]\n",
    "\n",
    "People (a terminal outside ",
    product_name!(),
    "):\n",
    "  ", cli_name!(), " permission pending                            what waits on you\n",
    "  ", cli_name!(), " permission approve <id>\n",
    "  ", cli_name!(), " permission deny <id> [--reason \"\u{2026}\"]\n",
    "  ", cli_name!(), " permission beta on|off|status                 the server switch (owner; on by default)\n",
    "  ", cli_name!(), " permission approvers [<login>[,<login>\u{2026}]|everyone]\n",
    "                                                   who agents' cards ask when an agent names no\n",
    "                                                   one (owner to change; everyone by default)\n",
    "  ", cli_name!(), " permission agent <workspace> on|off           one agent's \"Let this agent ask for\n",
    "                                                   permission\" (owner or admin; off until\n",
    "                                                   someone turns it on)\n",
    "\n",
    "Rules: ", cli_name!(), " study permissions\n",
);

/// `_permission_print`'s line for status `000`.
pub const NO_ANSWER: &str = concat!("Error: no answer from the ", product_name!(), " daemon");

/// The lines of the shell's Python that raise (`tests` pins them to
/// `cli/src/permission.sh`). `_permission_print`'s Python is `python3 -c
/// '...'` whose text starts with a newline: its first line is line 2.
pub mod line {
    pub const P_DENIED_BY: u32 = 41;
    pub const P_PEOPLE_FOR: u32 = 27;
    pub const P_PEOPLE_JOIN: u32 = 34;
    pub const P_DRY_SHOWN: u32 = 58;
    pub const P_ASKED: u32 = 67;
    pub const P_THREAD: u32 = 68;
    /// `beta status`'s Python.
    pub const B_GET: u32 = 8;
    /// `approvers`' Python.
    pub const A_GET: u32 = 8;
    pub const A_JOIN: u32 = 9;
}

/// The verb and what it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `_permission_usage` on stdout, exit 0.
    Usage,
    /// Lines on stderr, and the exit (2 for a usage error, 1 for `shift
    /// 2` with one word left under `set -e`, 3 for a person's verb in a
    /// session).
    Stop { lines: Vec<String>, exit: Exit },
    /// A `_permission_print` verb: one `cli_send_json`.
    Send {
        verb: &'static str,
        post: bool,
        path: &'static str,
        payload: String,
        json: bool,
    },
    /// `approve` or `deny`: the `pending` lookup, then the answer.
    Answer {
        approve: bool,
        id: String,
        reason: String,
    },
    /// `beta status`.
    BetaStatus,
    /// `approvers` alone.
    Approvers,
}

fn stop(line: impl Into<String>, exit: Exit) -> Ask {
    Ask::Stop {
        lines: vec![line.into()],
        exit,
    }
}

/// A value flag as the last word: the shell's `_need_value` line, exit 2.
fn short_shift(flag: &str) -> Ask {
    Ask::Stop {
        lines: vec![crate::out::need_value(flag)],
        exit: Exit::Usage,
    }
}

fn unknown_flag(flag: &str) -> Ask {
    let mut lines = vec![format!("Error: unknown flag '{flag}'")];
    lines.extend(USAGE.trim_end_matches('\n').split('\n').map(str::to_string));
    Ask::Stop {
        lines,
        exit: Exit::Usage,
    }
}

/// `_permission_people_only <verb>`: refused with a passport.
fn people_only(verb: &str) -> Ask {
    stop(
        format!(
            concat!(
                "Error [person_only]: ",
                cli_name!(),
                " permission {} is for people, from a terminal outside ",
                product_name!(),
                " (or the ",
                product_name!(),
                " app). An agent asks with ",
                cli_name!(),
                " permission request."
            ),
            verb
        ),
        Exit::Refused,
    )
}

/// The words after `permission` as `_permission_cmd` reads them.
/// `passport`: `SCOPED_TOKEN` is set (a person's verb is refused).
///
/// Help wins (`_wants_help`): `-h` or `--help` anywhere before `--` is
/// the usage, before any verb reads its words. The family's probe in
/// [`run`] is still sent first.
pub fn parse(words: &[&str], passport: bool) -> Ask {
    if wants_help(words) {
        return Ask::Usage;
    }
    let sub = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or_default();
    match sub {
        "" | "help" | "-h" | "--help" => Ask::Usage,
        "request" | "ask" => parse_request(rest),
        "update" => {
            let (id, rest) = first_id(rest);
            let mut why = String::new();
            let mut i = 0;
            while let Some(w) = rest.get(i) {
                if *w != "--why" {
                    return stop(format!("Error: unexpected argument '{w}'"), Exit::Usage);
                }
                why = rest.get(i + 1).copied().unwrap_or("").to_string();
                if i + 2 > rest.len() {
                    return short_shift(w);
                }
                i += 2;
            }
            if id.is_empty() || why.is_empty() {
                return stop(
                    concat!(
                        "Usage: ",
                        cli_name!(),
                        " permission update <id> --why \"<new facts>\""
                    ),
                    Exit::Usage,
                );
            }
            send(
                "update",
                true,
                UPDATE,
                dumps(&obj(vec![("id", s(id)), ("why", s(&why))])),
            )
        }
        "status" => {
            let payload = match rest.first() {
                Some(id) if !id.is_empty() => format!("id={}", quote(id)),
                _ => String::new(),
            };
            send("status", false, STATUS, payload)
        }
        "cancel" => match rest.first() {
            Some(id) if !id.is_empty() => {
                send("cancel", true, CANCEL, dumps(&obj(vec![("id", s(id))])))
            }
            _ => stop(
                concat!("Usage: ", cli_name!(), " permission cancel <id>"),
                Exit::Usage,
            ),
        },
        "list" => {
            let (mut area, mut search) = ("", "");
            let mut i = 0;
            while let Some(w) = rest.get(i) {
                let value = rest.get(i + 1).copied().unwrap_or("");
                match *w {
                    "--area" => area = value,
                    "--search" => search = value,
                    w => return stop(format!("Error: unexpected argument '{w}'"), Exit::Usage),
                }
                if i + 2 > rest.len() {
                    return short_shift(w);
                }
                i += 2;
            }
            let mut q = String::new();
            if !area.is_empty() {
                q = format!("area={}", quote(area));
            }
            if !search.is_empty() {
                let amp = if q.is_empty() { "" } else { "&" };
                q = format!("{q}{amp}search={}", quote(search));
            }
            send("list", false, LIST, q)
        }
        "pending" if passport => people_only("pending"),
        "pending" => send("pending", false, PENDING, String::new()),
        "approve" | "deny" if passport => people_only(sub),
        "approve" | "deny" => {
            let (id, rest) = first_id(rest);
            let mut reason = String::new();
            let mut i = 0;
            while let Some(w) = rest.get(i) {
                if *w != "--reason" {
                    return stop(format!("Error: unexpected argument '{w}'"), Exit::Usage);
                }
                reason = rest.get(i + 1).copied().unwrap_or("").to_string();
                if i + 2 > rest.len() {
                    return short_shift(w);
                }
                i += 2;
            }
            if id.is_empty() {
                return stop(
                    format!(concat!("Usage: ", cli_name!(), " permission {} <id>"), sub),
                    Exit::Usage,
                );
            }
            Ask::Answer {
                approve: sub == "approve",
                id: id.to_string(),
                reason,
            }
        }
        "beta" if passport => people_only("beta"),
        // `${1:-status}`: an empty word is `status` too.
        "beta" => match rest.first().copied().unwrap_or("") {
            "on" | "off" => send(
                "beta",
                true,
                SETTINGS_UPDATE,
                format!(
                    "{{\"permissionCards\":{}}}",
                    if rest[0] == "on" { "true" } else { "false" }
                ),
            ),
            "status" | "" => Ask::BetaStatus,
            _ => stop(
                concat!("Usage: ", cli_name!(), " permission beta on|off|status"),
                Exit::Usage,
            ),
        },
        "approvers" if passport => people_only("approvers"),
        "approvers" => match rest {
            [] => Ask::Approvers,
            [one] => send(
                "beta",
                true,
                SETTINGS_UPDATE,
                dumps(&obj(vec![(
                    "permissionApprovers",
                    Value::Array(approvers(one).into_iter().map(Value::Str).collect()),
                )])),
            ),
            _ => stop(
                concat!(
                    "Usage: ",
                    cli_name!(),
                    " permission approvers [<login>[,<login>\u{2026}]|everyone]"
                ),
                Exit::Usage,
            ),
        },
        "agent" if passport => people_only("agent"),
        "agent" => {
            let ws = rest.first().copied().unwrap_or("");
            let onoff = rest.get(1).copied().unwrap_or("");
            if !matches!(onoff, "on" | "off") {
                return stop(
                    concat!(
                        "Usage: ",
                        cli_name!(),
                        " permission agent <workspace> on|off"
                    ),
                    Exit::Usage,
                );
            }
            send(
                "agent",
                true,
                AGENT_ACCESS,
                dumps(&obj(vec![
                    ("workspace", s(ws)),
                    ("toggle", s("permission_cards")),
                    ("value", Value::Bool(onoff == "on")),
                ])),
            )
        }
        other => {
            let mut lines = vec![format!(
                concat!("Error: unknown ", cli_name!(), " permission command: {}"),
                other
            )];
            lines.extend(USAGE.trim_end_matches('\n').split('\n').map(str::to_string));
            Ask::Stop {
                lines,
                exit: Exit::Usage,
            }
        }
    }
}

/// `_wants_help "$@"`: `-h` or `--help` before the first `--`.
fn wants_help(words: &[&str]) -> bool {
    words
        .iter()
        .take_while(|w| **w != "--")
        .any(|w| matches!(*w, "-h" | "--help"))
}

/// `local id="${1:-}"; [ -n "$id" ] && shift`: the id and the words after
/// it (an empty first word is not taken off).
fn first_id<'a>(rest: &'a [&'a str]) -> (&'a str, &'a [&'a str]) {
    match rest.split_first() {
        Some((id, after)) if !id.is_empty() => (id, after),
        _ => ("", rest),
    }
}

fn send(verb: &'static str, post: bool, path: &'static str, payload: String) -> Ask {
    Ask::Send {
        verb,
        post,
        path,
        payload,
        json: false,
    }
}

/// `_permission_request`'s words.
fn parse_request(rest: &[&str]) -> Ask {
    let (mut action, mut why, mut to) = (String::new(), String::new(), String::new());
    let (mut dry, mut json_out) = (false, false);
    let mut kv: Vec<&str> = Vec::new();
    // `$1 needs …`: 0.46.17's `--assign` is the same flag as `--to`.
    let needs = |flag: &str| {
        stop(
            format!("Error: {flag} needs one or more logins, comma-separated ({flag} ada,owner)"),
            Exit::Usage,
        )
    };
    let mut i = 0;
    while let Some(w) = rest.get(i) {
        let w = *w;
        match w {
            "--why" => {
                let Some(v) = rest.get(i + 1) else {
                    return stop("Error: --why needs a value", Exit::Usage);
                };
                why = (*v).to_string();
                i += 2;
                continue;
            }
            w if w.starts_with("--why=") => why = w["--why=".len()..].to_string(),
            "--to" | "--assign" => match rest.get(i + 1) {
                Some(v) if !v.is_empty() => {
                    to = (*v).to_string();
                    i += 2;
                    continue;
                }
                _ => return needs(w),
            },
            w if w.starts_with("--to=") || w.starts_with("--assign=") => {
                let (flag, value) = w.split_once('=').unwrap_or((w, ""));
                to = value.to_string();
                if to.is_empty() {
                    return needs(flag);
                }
            }
            "--dry-run" => dry = true,
            "--json" => json_out = true,
            "-h" | "--help" => return Ask::Usage,
            w if w.starts_with("--") => return unknown_flag(w),
            w if w.contains('=') => kv.push(w),
            w if action.is_empty() => action = w.to_string(),
            w => {
                return stop(
                    format!("Error: unexpected argument '{w}' (values are key=value)"),
                    Exit::Usage,
                )
            }
        }
        i += 1;
    }
    if action.is_empty() {
        return stop(
            concat!(
                "Usage: ",
                cli_name!(),
                " permission request <action> [key=value \u{2026}] --why \"<one line>\""
            ),
            Exit::Usage,
        );
    }
    if why.is_empty() {
        return stop(
            "Error [bad_why]: --why is required: one line, at most 200 characters, saying what you need it for",
            Exit::Usage,
        );
    }
    // `params[k] = v`: a key that comes twice keeps its first place and
    // its last value.
    let mut params: Vec<(Text, Value)> = Vec::new();
    for a in kv {
        let (k, v) = a.split_once('=').unwrap_or((a, ""));
        match params.iter_mut().find(|(key, _)| key.as_str() == Some(k)) {
            Some(slot) => slot.1 = s(v),
            None => params.push((Text::from(k), s(v))),
        }
    }
    let mut body = vec![
        ("action", s(&action)),
        ("params", Value::Object(params)),
        ("why", s(&why)),
    ];
    if !to.is_empty() {
        body.push(("to", s(&to)));
    }
    if dry {
        body.push(("dryRun", Value::Bool(true)));
    }
    Ask::Send {
        verb: "request",
        post: true,
        path: REQUEST,
        payload: dumps(&obj(body)),
        json: json_out,
    }
}

/// `approvers <list>`'s names: none for `everyone` (any case, Python's
/// whitespace stripped), else each comma-separated name stripped, the
/// empty ones left out.
pub fn approvers(raw: &str) -> Vec<Text> {
    let raw = raw.trim_matches(py_space);
    // `raw.lower() == "everyone"`: no character but an ASCII one lowers
    // to one of these letters.
    if raw.eq_ignore_ascii_case("everyone") {
        return Vec::new();
    }
    raw.split(',')
        .map(|n| n.trim_matches(py_space))
        .filter(|n| !n.is_empty())
        .map(Text::from)
        .collect()
}

// ---------------------------------------------------------------------
// The run.

/// How the family's existence is asked, decided before anything is sent.
enum Probe {
    /// `beta` with no passport: it exists, nothing asked.
    None,
    /// The pages' probe, with the passport ([`cards::ask`]).
    Cards(cards::Ask),
    /// The owner's probe over TCP.
    Owner(OwnerProbe),
}

/// `_permission_probe "$TOKEN"` outside a session's passport: one `GET
/// /cli/permission/enabled` over TCP, the owner token as Bearer, connect
/// 1 s, at most 1 s.
pub struct OwnerProbe {
    client: Box<Client>,
    reader: Reader,
}

impl OwnerProbe {
    /// The probe for `settings`, or `None` where the binary can't be sure
    /// to send it as the shell does (the rules of [`cards::ask`]).
    fn new(settings: &Settings, env: &Env, would_refuse: bool) -> Option<OwnerProbe> {
        let Auth::Owner(token) = &settings.auth else {
            return None;
        };
        if !cards::plain(token) {
            return None;
        }
        let port = cards::plain_port(settings.port_text.as_deref()?)?;
        settings.host.parse::<Ipv4Addr>().ok()?;
        let reader = reader(env)?;
        Some(OwnerProbe {
            client: Box::new(settings.client_on(port, would_refuse)),
            reader,
        })
    }

    fn on(mut self) -> bool {
        match self.client.permission_enabled() {
            Ok(r) => {
                r.status == 200
                    && match self.reader {
                        Reader::Python(decode) => cards::enabled(&r.body, decode),
                        Reader::ShellWithout => false,
                        Reader::Own => cards::enabled(&r.body, Decode::Lossy),
                    }
            }
            Err(_) => false,
        }
    }
}

/// Who reads the probe's answer, as [`cards::ask`] decides it.
fn reader(env: &Env) -> Option<Reader> {
    match python::in_env(env) {
        Python3::Present => Some(Reader::Python(cards::decode(env)?)),
        Python3::Missing if python::NATIVE_WITHOUT_PYTHON3 => Some(Reader::Own),
        Python3::Missing => {
            cards::decode(env)?;
            Some(Reader::ShellWithout)
        }
        Python3::Unknown => None,
    }
}

/// `k5 permission ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
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
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    // A port curl won't take is the shell's.
    let port = curl_port(settings.port_text.as_deref()?)?;
    let would_refuse = || passport::current(args).would_refuse();
    let env = Env::current();
    let pre = offline::shell_pwd(&env)
        .and_then(|cwd| offline::prelude(&env, &cwd))
        .ok()?;
    let has_passport = matches!(settings.auth, Auth::Passport { .. });
    let probe = if has_passport {
        Probe::Cards(cards::ask(&env, &pre, would_refuse)?)
    } else if words.first() == Some(&"beta") {
        Probe::None
    } else {
        Probe::Owner(OwnerProbe::new(&settings, &env, would_refuse())?)
    };
    // The help the shell prints when the family doesn't exist.
    let feedback = feedback_optin::ask(&env, &pre, would_refuse)?;
    crate::pages::daily_text(false, false)?;
    let on = match probe {
        Probe::None => true,
        Probe::Cards(ask) => cards::on(ask),
        Probe::Owner(p) => p.on(),
    };
    if !on {
        out::stderr_line("Unknown command: permission");
        let hidden = feedback_optin::hidden(feedback);
        let text = crate::pages::daily_text(false, hidden)?;
        let _ = out::stdout(&text);
        return Some(Exit::Error);
    }
    let ask = parse(&words, has_passport);
    let settings = Settings {
        port: Some(port),
        ..settings
    };
    Some(run_ask(args, ask, &settings))
}

fn run_ask(args: &[OsString], ask: Ask, settings: &Settings) -> Exit {
    match &ask {
        Ask::Usage => {
            return match out::stdout(USAGE) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Ask::Stop { lines, exit } => {
            for l in lines {
                out::stderr_line(l);
            }
            return *exit;
        }
        _ => {}
    }
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let exit = match ask {
        Ask::Send {
            verb,
            post,
            path,
            payload,
            json,
        } => match fetch(&mut client, &mut closing, post, path, &payload) {
            Fetched::Raw(status, raw) => print_answer(verb, status, &raw, json, now()).print(),
            Fetched::Stopped(exit) => exit,
        },
        Ask::Answer {
            approve,
            id,
            reason,
        } => answer(&mut client, &mut closing, approve, &id, &reason),
        Ask::BetaStatus => match fetch(&mut client, &mut closing, false, SETTINGS_GET, "") {
            Fetched::Raw(_, raw) => beta_status(&raw).print(),
            Fetched::Stopped(exit) => exit,
        },
        Ask::Approvers => match fetch(&mut client, &mut closing, false, SETTINGS_GET, "") {
            Fetched::Raw(_, raw) => approvers_shown(&raw).print(),
            Fetched::Stopped(exit) => exit,
        },
        Ask::Usage | Ask::Stop { .. } => unreachable!("answered above"),
    };
    closing.flush(&mut client, exit);
    exit
}

/// What `cli_send_json` printed, as the caller splits it.
enum Fetched {
    /// The status (`0` for the shell's `000`) and the body as `$(…)` held
    /// it: the NULs dropped, with `\n000` in front for each refused
    /// connect `_k2_curl` tried again before the answer (curl's `-w` wrote
    /// one for each).
    Raw(u16, Vec<u8>),
    /// The send ended the command (the shell's own line, then exit 1).
    Stopped(Exit),
}

fn fetch(
    client: &mut Client,
    closing: &mut Closing,
    post: bool,
    path: &str,
    payload: &str,
) -> Fetched {
    let passport = client.passport().is_some();
    match client.send_json(post, path, payload) {
        Ok(r) => {
            closing.answer(&r, passport);
            Fetched::Raw(r.status, held(client, &r))
        }
        Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
            out::stderr_line(&e.to_string());
            Fetched::Stopped(Exit::Error)
        }
        Err(_) => Fetched::Raw(0, Vec::new()),
    }
}

fn held(client: &Client, r: &Response) -> Vec<u8> {
    let mut raw = b"\n000".repeat(client.refused_before_answer() as usize);
    raw.extend(r.body.iter().copied().filter(|b| *b != 0));
    raw
}

/// `approve <id>` / `deny <id>`: the `pending` lookup, the shell's Python
/// on it, then the answer through `/cli/thread/answer`.
fn answer(
    client: &mut Client,
    closing: &mut Closing,
    approve: bool,
    id: &str,
    reason: &str,
) -> Exit {
    let raw = match fetch(client, closing, false, PENDING, "") {
        Fetched::Raw(_, raw) => raw,
        Fetched::Stopped(exit) => return exit,
    };
    let body = match lookup(&raw, id, approve, reason) {
        Lookup::Body(b) => b,
        Lookup::Confirm => {
            out::stderr_line(&format!(
                concat!(
                    "Error [vault_confirm_required]: {} needs the same confirm Settings takes. Approve it in the ",
                    product_name!(),
                    " app, on your phone, or in Settings."
                ),
                id
            ));
            return Exit::Refused;
        }
        Lookup::NotFound => {
            out::stderr_line(&format!(
                concat!(
                    "Error [not_found]: no request {} waits on you (",
                    cli_name!(),
                    " permission pending)"
                ),
                id
            ));
            return Exit::NotFound;
        }
    };
    let verb = if approve { "approve" } else { "deny" };
    match fetch(client, closing, true, ANSWER, &body) {
        Fetched::Raw(status, raw) => print_answer(verb, status, &raw, false, now()).print(),
        Fetched::Stopped(exit) => exit,
    }
}

/// What the `pending` lookup's Python printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// The answer's JSON.
    Body(String),
    /// `CONFIRM`: the request needs the vault's confirm.
    Confirm,
    /// It failed or raised (its stderr is dropped): no such request.
    NotFound,
}

/// The `pending` lookup's Python on the answer's body.
pub fn lookup(raw: &[u8], id: &str, approve: bool, reason: &str) -> Lookup {
    let Ok(v) = json::parse(&String::from_utf8_lossy(raw)) else {
        return Lookup::NotFound;
    };
    let Value::Object(pairs) = &v else {
        return Lookup::NotFound;
    };
    let requests = get(pairs, "requests").filter(|r| py::truthy(r)).cloned();
    let items = match requests {
        None => Vec::new(),
        Some(r) => match super::py_iter(&r, 0) {
            Ok(items) => items,
            Err(_) => return Lookup::NotFound,
        },
    };
    for r in items {
        let Value::Object(r) = r else {
            return Lookup::NotFound;
        };
        if !matches!(get(&r, "requestId"), Some(Value::Str(t)) if t.as_str() == Some(id)) {
            continue;
        }
        if approve {
            let kind = match get(&r, "proof").filter(|p| py::truthy(p)) {
                None => None,
                Some(Value::Object(p)) => get(p, "kind").cloned(),
                Some(_) => return Lookup::NotFound,
            };
            if matches!(kind, Some(Value::Str(t)) if t.as_str() == Some("confirm")) {
                return Lookup::Confirm;
            }
        }
        let mut out = vec![
            ("addr", get(&r, "addr").cloned().unwrap_or(Value::Null)),
            ("id", get(&r, "cardId").cloned().unwrap_or(Value::Null)),
            ("answer", s(if approve { "Allow" } else { "Deny" })),
        ];
        if !reason.is_empty() {
            out.push(("reason", s(reason)));
        }
        return Lookup::Body(dumps(&obj(out)));
    }
    Lookup::NotFound
}

/// `beta status`'s Python on the body (its status is not read).
pub fn beta_status(raw: &[u8]) -> Shown {
    let settings = match settings_of(raw) {
        Ok(s) => s,
        Err(shown) => return shown,
    };
    let on = match &settings {
        Value::Object(pairs) => get(pairs, "permissionCards") == Some(&Value::Bool(true)),
        other => {
            return raise_shown(
                String::new(),
                &Raise::no_attribute(line::B_GET, other, "get"),
            )
        }
    };
    Shown::out(format!(
        "Let agents ask with permission cards: {}\n",
        if on { "on" } else { "off" }
    ))
}

/// `approvers` alone: the shell's Python on the body.
pub fn approvers_shown(raw: &[u8]) -> Shown {
    let settings = match settings_of(raw) {
        Ok(s) => s,
        Err(shown) => return shown,
    };
    let names = match &settings {
        Value::Object(pairs) => get(pairs, "permissionApprovers").cloned(),
        other => {
            return raise_shown(
                String::new(),
                &Raise::no_attribute(line::A_GET, other, "get"),
            )
        }
    };
    let names = names.filter(py::truthy);
    let who = match names {
        None => "everyone who may answer (an owner or admin)".to_string(),
        Some(n) => match join(&n, line::A_JOIN) {
            Ok(j) => j,
            Err(r) => return raise_shown(String::new(), &r),
        },
    };
    Shown::out(format!("Cards from agents ask: {who}\n"))
}

/// `json.load(sys.stdin)` then `v.get("settings", v) if isinstance(v,
/// dict) else {}`; not JSON: the no-answer line, exit 1.
fn settings_of(raw: &[u8]) -> Result<Value, Shown> {
    let Ok(v) = json::parse(&String::from_utf8_lossy(raw)) else {
        return Err(Shown::err(format!("{NO_ANSWER}\n"), Exit::Error));
    };
    Ok(match &v {
        Value::Object(pairs) => get(pairs, "settings").cloned().unwrap_or(v),
        _ => Value::Object(Vec::new()),
    })
}

/// `", ".join(v)`, or the `TypeError` at `line`.
fn join(v: &Value, line: u32) -> Result<String, Raise> {
    let items = super::py_iter(v, line)
        .map_err(|_| Raise::at(line, "TypeError: can only join an iterable"))?;
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        match item {
            Value::Str(_) => out.push(py::str_of(item)),
            other => {
                return Err(Raise::at(
                    line,
                    format!(
                        "TypeError: sequence item {i}: expected str instance, {} found",
                        py::type_name(other)
                    ),
                ))
            }
        }
    }
    Ok(out.join(", "))
}

/// A Python run that raised: what it printed, the traceback, exit 1.
fn raise_shown(stdout: String, r: &Raise) -> Shown {
    super::raised(stdout, String::new(), r)
}

// ---------------------------------------------------------------------
// `_permission_print`.

/// `_permission_print <verb> "<body>\n<status>" [json]`: `status` 0 is the
/// shell's `000`. `now`: `time.time()`.
pub fn print_answer(verb: &str, status: u16, raw: &[u8], json_out: bool, now: i64) -> Shown {
    if status == 0 {
        return Shown::err(format!("{NO_ANSWER}\n"), Exit::Error);
    }
    let exit = match status {
        200..=299 => Exit::Ok,
        400 | 413 => Exit::Usage,
        404 => Exit::NotFound,
        403 | 409 | 429 => Exit::Refused,
        _ => Exit::Error,
    };
    let text = String::from_utf8_lossy(raw).into_owned();
    if json_out && (200..300).contains(&status) {
        return Shown {
            stdout: format!("{text}\n"),
            stderr: String::new(),
            exit: Exit::Ok,
        };
    }
    let mut shown = printer(verb, status, &text, now);
    // `|| true`: the status decides the exit, a traceback too.
    shown.exit = exit;
    shown
}

/// What `_permission_print`'s Python prints (its exit is not used).
fn printer(verb: &str, status: u16, raw: &str, now: i64) -> Shown {
    let v = json::parse(raw).ok();
    let dict = matches!(v, Some(Value::Object(_)));
    let mut stdout = String::new();
    let mut stderr = String::new();
    if status >= 300 || !dict {
        let v = v.unwrap_or(Value::Null);
        let e = at(&v, "error").cloned();
        let code = match &e {
            Some(Value::Object(_)) => at(e.as_ref().unwrap_or(&Value::Null), "code")
                .cloned()
                .unwrap_or(Value::Null),
            Some(Value::Str(t)) => Value::Str(t.clone()),
            _ => s("error"),
        };
        let hint = match &e {
            Some(e @ Value::Object(_)) => or(at(e, "hint"), at(e, "message")),
            _ if dict => or(at(&v, "hint"), at(&v, "message")),
            _ => None,
        };
        if is_str(&code, "recently_denied") && dict {
            let d = match at(&v, "denied").filter(|d| py::truthy(d)) {
                Some(d) => d.clone(),
                None => match &e {
                    Some(e @ Value::Object(_)) => e.clone(),
                    _ => Value::Object(Vec::new()),
                },
            };
            if !matches!(d, Value::Object(_)) {
                return Shown {
                    stdout,
                    stderr: Raise::no_attribute(line::P_DENIED_BY, &d, "get").text(),
                    exit: Exit::Error,
                };
            }
            let mut msg = match at(&d, "by").filter(|b| py::truthy(b)) {
                Some(by) => format!(
                    "Denied by {} at {}",
                    py::str_of(by),
                    when(at(&d, "at").unwrap_or(&Value::Null))
                ),
                None => "Denied".to_string(),
            };
            if let Some(reason) = at(&d, "reason").filter(|r| py::truthy(r)) {
                msg.push_str(&format!(": \"{}\"", py::str_of(reason)));
            }
            stderr.push_str(&format!("{msg}.\n"));
            let until = or(at(&d, "askAgainAt"), at(&d, "until")).filter(py::truthy);
            let rid = or(at(&d, "id"), at(&v, "id"))
                .filter(py::truthy)
                .map_or_else(|| "<id>".to_string(), |r| py::str_of(&r));
            let after = match &until {
                Some(u) => when(u),
                None => "24 hours".to_string(),
            };
            stderr.push_str(&format!(
                concat!(
                    "Ask again after {after}, or add facts: ",
                    cli_name!(),
                    " permission update {rid} --why \"\u{2026}\"\n"
                ),
                after = after,
                rid = rid
            ));
        } else {
            let detail = match hint.filter(py::truthy) {
                Some(h) => py::str_of(&h),
                None => {
                    let stripped: Vec<char> =
                        raw.trim_matches(py_space).chars().take(300).collect();
                    if stripped.is_empty() {
                        format!("status {status}")
                    } else {
                        stripped.into_iter().collect()
                    }
                }
            };
            stderr.push_str(&format!("Error [{}]: {detail}\n", py::str_of(&code)));
        }
        return Shown {
            stdout,
            stderr,
            exit: Exit::Ok,
        };
    }
    let v = v.unwrap_or(Value::Null);
    let st = at(&v, "status").cloned();
    let id_or_q = || at(&v, "id").cloned().unwrap_or_else(|| s("?"));
    let has = |k: &str| at(&v, k).is_some();
    let truthy = |k: &str| at(&v, k).is_some_and(py::truthy);
    let is = |x: Option<&Value>, want: &str| x.is_some_and(|x| is_str(x, want));
    if verb == "request" {
        if truthy("dryRun") || (has("text") && !has("id")) {
            stdout.push_str("Dry run: nothing was sent.\n");
            let text = at(&v, "text")
                .filter(|t| py::truthy(t))
                .map_or_else(String::new, py::str_of);
            stdout.push_str(&format!("Card: {text}\n"));
            if let Some(c) = at(&v, "consequences").filter(|c| py::truthy(c)) {
                stdout.push_str(&format!("What it means: {}\n", py::str_of(c)));
            }
            let routed = or(at(&v, "routedTo"), at(&v, "approvers"));
            match people(routed.as_ref(), line::P_DRY_SHOWN) {
                Ok(p) => stdout.push_str(&format!("Shown to: {p}\n")),
                Err(r) => return raised(stdout, &r),
            }
            if let Some(e) = at(&v, "expiresIn").filter(|e| py::truthy(e)) {
                stdout.push_str(&format!("Expires: {}\n", py::str_of(e)));
            }
        } else if is(st.as_ref(), "already_pending") || is(at(&v, "code"), "already_pending") {
            stdout.push_str(&format!(
                "Already asked: {} (pending). Nothing new sent.\n",
                py::str_of(&id_or_q())
            ));
        } else if is(st.as_ref(), "already_done") || is(at(&v, "code"), "already_done") {
            stdout.push_str("Already on: nothing to ask. Run your command again.\n");
        } else {
            let exp = expires_in(at(&v, "expiresAt").unwrap_or(&Value::Null), now);
            let state = st
                .filter(py::truthy)
                .map_or_else(|| "pending".to_string(), |s| py::str_of(&s));
            let exp = if exp.is_empty() {
                String::new()
            } else {
                format!("; {exp}")
            };
            let shown = match people(at(&v, "routedTo"), line::P_ASKED) {
                Ok(p) => p,
                Err(r) => return raised(stdout, &r),
            };
            stdout.push_str(&format!(
                "Asked: {} ({state}{exp}). Shown to: {shown}.\n",
                py::str_of(&id_or_q())
            ));
            let addr = match at(&v, "addr").filter(|a| py::truthy(a)) {
                None => String::new(),
                Some(Value::Str(t)) => format!(" ({})", py::str_of(&Value::Str(t.clone()))),
                Some(other) => {
                    return raised(
                        stdout,
                        &Raise::at(
                            line::P_THREAD,
                            format!(
                                "TypeError: can only concatenate str (not \"{}\") to str",
                                py::type_name(other)
                            ),
                        ),
                    )
                }
            };
            stdout.push_str(&format!(
                "Card raised on your Thread{addr}: they answer it there (desktop, phone or web); you get one [permission:{}] turn with the answer.\n",
                py::str_of(&id_or_q())
            ));
            if let Some(t) = at(&v, "text").filter(|t| py::truthy(t)) {
                let lasts = match at(&v, "duration").filter(|d| py::truthy(d)) {
                    Some(d) => format!(" Lasts: {}.", py::str_of(d)),
                    None => String::new(),
                };
                stdout.push_str(&format!("Card: {}{lasts}\n", py::str_of(t)));
            }
        }
    } else if matches!(
        verb,
        "update" | "cancel" | "approve" | "deny" | "beta" | "agent"
    ) {
        let said = or(at(&v, "message"), at(&v, "hint"))
            .filter(py::truthy)
            .map_or_else(|| "OK.".to_string(), |m| py::str_of(&m));
        stdout.push_str(&format!("{said}\n"));
    } else {
        let v = if verb == "list" { named_actions(v) } else { v };
        let options = Options {
            indent: Some(2),
            ..Options::default()
        };
        let text = json::to_string(&v, &options)
            .expect("json.dumps with ensure_ascii writes any value json.loads gave");
        stdout.push_str(&format!("{text}\n"));
    }
    Shown {
        stdout,
        stderr,
        exit: Exit::Ok,
    }
}

/// `list`'s answer with each action's `command` this binary's own name, a
/// space and the action's `tail`, where it holds a non-empty string one
/// (white-label C5); an action with none is as sent. The shell's
/// `[a.update(command="k2 <tail>"...) for a in v["actions"] ...]`.
fn named_actions(v: Value) -> Value {
    let Value::Object(mut pairs) = v else {
        return v;
    };
    for (k, x) in pairs.iter_mut() {
        let (Some("actions"), Value::Array(items)) = (k.as_str(), x) else {
            continue;
        };
        for a in items.iter_mut() {
            if let Value::Object(p) = a {
                let named = match crate::transport::permission::named_pairs(p) {
                    std::borrow::Cow::Owned(n) => Some(n),
                    std::borrow::Cow::Borrowed(_) => None,
                };
                if let Some(n) = named {
                    *p = n;
                }
            }
        }
    }
    Value::Object(pairs)
}

fn raised(stdout: String, r: &Raise) -> Shown {
    Shown {
        stdout,
        stderr: r.text(),
        exit: Exit::Error,
    }
}

/// `people(r)`, called from `from`: the names joined, or `nobody yet`;
/// the `TypeError` where Python raises one.
fn people(r: Option<&Value>, from: u32) -> Result<String, Raise> {
    let items = match r.filter(|r| py::truthy(r)) {
        None => Vec::new(),
        Some(r) => super::py_iter(r, line::P_PEOPLE_FOR).map_err(|e| Raise {
            frames: vec![(from, "<module>"), (line::P_PEOPLE_FOR, "people")],
            exception: e.exception,
        })?,
    };
    let mut out: Vec<Value> = Vec::with_capacity(items.len());
    for p in items {
        match &p {
            Value::Object(pairs) => {
                let n = or(get(pairs, "login"), get(pairs, "name"))
                    .filter(py::truthy)
                    .unwrap_or_else(|| s("?"));
                match get(pairs, "role").filter(|r| py::truthy(r)) {
                    Some(role) => {
                        out.push(s(&format!("{} ({})", py::str_of(&n), py::str_of(role))))
                    }
                    None => out.push(n),
                }
            }
            other => out.push(s(&py::str_of(other))),
        }
    }
    let mut names = Vec::with_capacity(out.len());
    for (i, n) in out.iter().enumerate() {
        match n {
            Value::Str(_) => names.push(py::str_of(n)),
            other => {
                return Err(Raise {
                    frames: vec![(from, "<module>"), (line::P_PEOPLE_JOIN, "people")],
                    exception: format!(
                        "TypeError: sequence item {i}: expected str instance, {} found",
                        py::type_name(other)
                    ),
                })
            }
        }
    }
    let joined = names.join(", ");
    Ok(if joined.is_empty() {
        "nobody yet".to_string()
    } else {
        joined
    })
}

/// `when(ts)`: the local time of `int(ts)` as `%Y-%m-%d %H:%M`, else
/// `str(ts)`.
pub fn when(ts: &Value) -> String {
    py_int(ts)
        .ok()
        .and_then(|digits| {
            super::activity::stamp(&Value::Int(digits), 0)
                .ok()
                .and_then(|t| t.get(..t.len().checked_sub(3)?).map(str::to_string))
        })
        .unwrap_or_else(|| py::str_of(ts))
}

/// `expires_in(ts)`: `""` when `int(ts)` fails, `expired` at or before
/// `now`, else the whole days, or the whole hours (at least 1).
pub fn expires_in(ts: &Value, now: i64) -> String {
    let Ok(digits) = py_int(ts) else {
        return String::new();
    };
    let (days, hours) = match digits.parse::<i128>() {
        Ok(n) => {
            let s = n - i128::from(now);
            if s <= 0 {
                return "expired".into();
            }
            ((s / 86_400).to_string(), (s % 86_400) / 3600)
        }
        // Past i128: negative is long expired; else divide the digits.
        Err(_) if digits.starts_with('-') => return "expired".into(),
        Err(_) => {
            let s = sub_small(&digits, now.max(0).unsigned_abs());
            let (q, r) = div_small(&s, 86_400);
            (q, i128::from(r / 3600))
        }
    };
    if days != "0" {
        let plural = if days == "1" { "" } else { "s" };
        return format!("expires in {days} day{plural}");
    }
    let h = hours.max(1);
    format!("expires in {h} hour{}", if h == 1 { "" } else { "s" })
}

/// A long run of decimal digits less `k` (the digits are larger).
fn sub_small(digits: &str, k: u64) -> String {
    let mut out: Vec<u8> = digits.bytes().rev().map(|b| b - b'0').collect();
    let mut borrow = 0i64;
    let mut k = k;
    for d in out.iter_mut() {
        let mut v = i64::from(*d) - (k % 10) as i64 - borrow;
        k /= 10;
        borrow = if v < 0 {
            v += 10;
            1
        } else {
            0
        };
        *d = v as u8;
    }
    while out.len() > 1 && out.last() == Some(&0) {
        out.pop();
    }
    out.iter().rev().map(|d| char::from(b'0' + d)).collect()
}

/// A long run of decimal digits divided by `k`: the quotient's digits and
/// the remainder.
fn div_small(digits: &str, k: u64) -> (String, u64) {
    let mut q = String::with_capacity(digits.len());
    let mut r: u64 = 0;
    for b in digits.bytes() {
        let cur = r * 10 + u64::from(b - b'0');
        q.push(char::from(b'0' + (cur / k) as u8));
        r = cur % k;
    }
    let q = q.trim_start_matches('0');
    (if q.is_empty() { "0".into() } else { q.into() }, r)
}

/// `int(time.time())`.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

// ---------------------------------------------------------------------
// Python's values.

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

fn is_str(v: &Value, want: &str) -> bool {
    matches!(v, Value::Str(t) if t.as_str() == Some(want))
}

/// `json.dumps(v)` with Python's defaults.
fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default())
        .expect("json.dumps with ensure_ascii writes any value json.loads gave")
}

/// `d.get(key)` where `d` is a dict for sure (else `None`).
fn at<'a>(d: &'a Value, key: &str) -> Option<&'a Value> {
    match d {
        Value::Object(pairs) => get(pairs, key),
        _ => None,
    }
}

/// `a or b`: `a` when it is truthy, else `b` as it is (`None` when
/// missing). A caller that goes on with `or <default>` filters the
/// result with [`py::truthy`].
fn or(a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    match a {
        Some(a) if py::truthy(a) => Some(a.clone()),
        _ => b.cloned(),
    }
}

#[cfg(test)]
mod tests;
