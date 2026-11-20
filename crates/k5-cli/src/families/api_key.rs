//! `k5 api-key`, the public /v1 API's keys, as the shell CLI's
//! `cmd_api_key` answers it (`cli/src/talk-and-workspace.sh`): `create`
//! (and `create --status`), `list`, `revoke`, `disable` and `enable`.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the two gates ([`crate::prelude`]). `api-key` is an
//!    `id` tool: with `HOOK_SOCK` set, a stale passport or none at all is
//!    refused (exit 3).
//! 2. The words. A value flag as the last word stops on its failed
//!    `shift 2` under `set -e`: exit 1 with nothing printed. A bare or
//!    unknown verb prints the usage ([`USAGE`]), exit 1.
//! 3. `list`: `cli_request /cli/api-keys/list` on the left of a pipe into
//!    the shell's Python. `revoke`, `disable`, `enable`: `cli_post_json`
//!    of `{"id": …}` on the left of a pipe. No answer is the transport's
//!    line, then death by SIGPIPE ([`super::end_in_pipe`]).
//! 4. `create`: the LLM key from stdin with `--key-stdin` (a hidden prompt
//!    on a terminal, else one line), or the old argv forms with their
//!    warning; the body as the shell's Python builds it ([`create_body`]),
//!    sent once with `cli_post_json_stdin` (the body on stdin, never
//!    argv), then the printer. `--status`: a read-only status key, its body
//!    `{"label": …, "scope": "status"}` and a newline, any failure exit 1.
//! 5. Every reader but `--status`'s catches Python's exceptions and prints
//!    `Error: <message>` (exit 1); the message is Python 3.12's.
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The shell answers itself (`run` returns `None`): `create --hire` and
//! `hire-ceiling` (and its alias `app-access`), which ask for the owner's
//! confirm over the vault socket (`_vault_confirm_token`, not yet in the
//! binary); a line with `-h` or `--help`; a word that is not UTF-8;
//! `HOOK_SOCK` with a `CODEX_` variable exported; a port curl won't take;
//! and a `python3` the binary can't tell.

use super::skin_token::py::{self, at, at_or, dict, join, obj, or, s};
use super::{
    curl_port, end_in_pipe, piped_body, pipefail, raised, request_piped_tcp, Piped, Raise, Shown,
};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::{self, py as pyout};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::{permission, Call, Channel, Client, Error};
use k2_client::cli_name;
use std::ffi::OsString;
use std::io::Read;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["api-key"];
/// The shell answers it until the flip.
pub const OWNER: crate::route::Owner = crate::route::Owner::Bash;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "api-key";
/// The routes.
pub const CREATE: &str = "/cli/api-keys/create";
pub const LIST: &str = "/cli/api-keys/list";
pub const REVOKE: &str = "/cli/api-keys/revoke";
pub const DISABLE: &str = "/cli/api-keys/disable";
pub const ENABLE: &str = "/cli/api-keys/enable";

/// A bare or unknown verb: on stderr, exit 1.
pub const USAGE: [&str; 33] = [
    concat!("Usage: ", cli_name!(), " api-key <create|list|hire-ceiling|revoke|disable|enable>"),
    "",
    "  create  [--label L] [--key-stdin]   (LLM key read from stdin; never an argument)",
    "          [--provider anthropic|openai|google|xai] [--base-url U]",
    "          [--workspaces '*'|ws1,ws2,...]",
    "          [--allow host-sessions,canonical-message,sandboxes,db]",
    "          [--deny  host-sessions,canonical-message,sandboxes,db]",
    "          [--hire [--max-per-day N] [--max-live N] [--max-per-team N] [--hire-db off|read|write]",
    "                  [--link] [--link-targets ws1,ws2] [--retire] [--reach none|message|host-sessions]",
    "                  [--folder SUB] [--app-login APP[:ROOMS[:ROLES]]]...]   (asks for the owner confirm)",
    "  create --status [--label L]   A read-only status key (fleet monitor): reads this",
    "                 server's full /boot-status and opens nothing else (not /v1, not /cli)",
    "  list",
    "  hire-ceiling <id> (--app-login APP[:ROOMS[:ROLE]]... | --none)",
    "                 A hire key's App access limit: the App, rooms (self = the hired agent's own)",
    "                 and highest role its agents may get. Widening asks for the owner confirm.",
    "                 Also app-access.",
    "  disable <id>   Soft emergency kill (re-enableable)",
    "  enable  <id>   Clear soft-disable",
    "  revoke  <id>   Permanent (cannot re-enable same secret)",
    "",
    "Capabilities (Phase 0) gate which /v1 doors a key may open:",
    "  host-sessions      \u{2192} /v1/w/<ws>/host-sessions*   (default ON for new keys)",
    "  canonical-message  \u{2192} POST /v1/w/<ws>/message     (default OFF)",
    "  sandboxes          \u{2192} /v1/sandboxes* + sessions*  (default OFF)",
    "  db                 \u{2192} /v1/w/<ws>/db*              (default OFF)",
    "  hire (--hire)      \u{2192} /v1/agents/* + /v1/agent-teams/* and new workspaces on POST /v1/w",
    "                       (hire-only unless --allow names other doors; defaults 20/day,",
    "                        100 live, 12 per team, db off, no link/retire/reach)",
    "",
    "Wiki public-chat recipe:",
    concat!("  ", cli_name!(), " api-key create --label wiki-public-chat \\"),
    "    --workspaces '<wiki-ws-slug>' --allow host-sessions",
];

/// `--status` with another flag: exit 2.
pub const STATUS_ONLY: &str = "Error: a status key is read-only: --status takes --label only.";
/// `--key-stdin` with a key on argv: exit 2.
pub const BOTH_KEYS: &str = "Error: give the LLM key with --key-stdin or --llm-key, not both.";
/// `--key-stdin` and nothing read: exit 2.
pub const NO_KEY: &str = "Error: --key-stdin given but no key on stdin.";
/// `_cli_read_secret_stdin`'s prompt.
pub const PROMPT: &str = "Paste the LLM API key: ";
/// `_cli_warn_key_on_argv`.
pub const ARGV_WARNING: &str = "warning: passing a key on the command line is deprecated and will be refused in the next release; use --key-stdin";
/// What `create` prints after the key.
pub const STORE_NOW: &str =
    "STORE THIS KEY NOW \u{2014} it is shown only once and cannot be retrieved again.";
/// A key minted with no `--workspaces`: on stderr.
pub const NO_GRANT: [&str; 3] = [
    "NOTE: minted WITHOUT --workspaces \u{2014} this key authorizes ZERO",
    "workspaces, so every /v1/w/<ws>/* call will 404. Re-mint with",
    "--workspaces '*' (all) or --workspaces ws1,ws2 to grant access.",
];

/// A verb that takes one id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdVerb {
    Revoke,
    Disable,
    Enable,
}

impl IdVerb {
    fn of(word: &str) -> Option<IdVerb> {
        match word {
            "revoke" => Some(IdVerb::Revoke),
            "disable" => Some(IdVerb::Disable),
            "enable" => Some(IdVerb::Enable),
            _ => None,
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            IdVerb::Revoke => REVOKE,
            IdVerb::Disable => DISABLE,
            IdVerb::Enable => ENABLE,
        }
    }

    /// No id: exit 1.
    pub fn usage(self) -> &'static str {
        match self {
            IdVerb::Revoke => concat!("Usage: ", cli_name!(), " api-key revoke <id>"),
            IdVerb::Disable => concat!("Usage: ", cli_name!(), " api-key disable <id>"),
            IdVerb::Enable => concat!("Usage: ", cli_name!(), " api-key enable <id>"),
        }
    }

    /// What the answer's `success` picks.
    pub fn lines(self) -> (&'static str, &'static str) {
        match self {
            IdVerb::Revoke => (
                "API key revoked.",
                "No matching active key (unknown or already revoked).",
            ),
            IdVerb::Disable => (
                concat!(
                    "API key disabled (soft \u{2014} re-enable with ",
                    cli_name!(),
                    " api-key enable <id>)."
                ),
                "No matching key (unknown, revoked, or already disabled).",
            ),
            IdVerb::Enable => (
                "API key enabled.",
                "No matching key (unknown, revoked, or already enabled).",
            ),
        }
    }
}

/// `create`'s words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Create {
    pub label: String,
    pub status: bool,
    pub key_stdin: bool,
    /// The key from `--anthropic-key` or `--llm-key`, the last one given.
    pub argv_key: Option<String>,
    pub provider: String,
    pub base_url: String,
    pub workspaces: String,
    pub allow: String,
    pub deny: String,
    /// A flag other than `--label` and `--status`.
    pub other_flags: bool,
    /// A hire limit without `--hire`: `--link`, `--retire` or
    /// `--app-login` given, or a value flag's last value not empty.
    pub hire_limits: bool,
    /// The hire value flags' last values (`--max-per-day` and the others).
    pub limit_values: Vec<(String, String)>,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Create(Create),
    List,
    Id(IdVerb, String),
    /// Lines on stderr and the exit.
    Stop(Vec<String>, Exit),
}

/// The words after `api-key`. `None`: the shell's (`create --hire`,
/// `hire-ceiling`).
pub fn parse(words: &[&str]) -> Option<Ask> {
    let sub = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or_default();
    if let Some(v) = IdVerb::of(sub) {
        return Some(match rest.first() {
            Some(id) if !id.is_empty() => Ask::Id(v, id.to_string()),
            _ => Ask::Stop(vec![v.usage().to_string()], Exit::Error),
        });
    }
    match sub {
        "create" => parse_create(rest),
        "list" => Some(Ask::List),
        "hire-ceiling" | "app-access" => None,
        _ => Some(Ask::Stop(
            USAGE.iter().map(|l| l.to_string()).collect(),
            Exit::Error,
        )),
    }
}

fn parse_create(rest: &[&str]) -> Option<Ask> {
    let mut c = Create::default();
    let mut i = 0;
    while i < rest.len() {
        let flag = rest[i];
        if flag != "--label" && flag != "--status" {
            c.other_flags = true;
        }
        let value = rest.get(i + 1).map(|v| v.to_string());
        let mut takes = true;
        match flag {
            "--hire" => return None,
            "--status" => {
                c.status = true;
                takes = false;
            }
            "--key-stdin" => {
                c.key_stdin = true;
                takes = false;
            }
            "--link" | "--retire" => {
                c.hire_limits = true;
                takes = false;
            }
            "--label" | "--max-per-day" | "--max-live" | "--max-per-team" | "--hire-db"
            | "--link-targets" | "--reach" | "--folder" | "--app-login" | "--anthropic-key"
            | "--llm-key" | "--provider" | "--base-url" | "--workspaces" | "--allow"
            | "--deny" => {
                // `x="${2:-}"; shift 2`: no value, and `shift 2` fails.
                let Some(v) = value else {
                    return Some(Ask::Stop(Vec::new(), Exit::Error));
                };
                match flag {
                    "--label" => c.label = v,
                    "--anthropic-key" | "--llm-key" => c.argv_key = Some(v),
                    "--provider" => c.provider = v,
                    "--base-url" => c.base_url = v,
                    "--workspaces" => c.workspaces = v,
                    "--allow" => c.allow = v,
                    "--deny" => c.deny = v,
                    // `h_apps` gains `v` and a newline: never empty.
                    "--app-login" => c.hire_limits = true,
                    _ => {
                        c.limit_values.retain(|(f, _)| f != flag);
                        c.limit_values.push((flag.to_string(), v));
                    }
                }
            }
            other => {
                return Some(Ask::Stop(vec![format!("Unknown flag: {other}")], Exit::Error));
            }
        }
        i += if takes { 2 } else { 1 };
    }
    c.hire_limits |= c.limit_values.iter().any(|(_, v)| !v.is_empty());
    Some(Ask::Create(c))
}

/// `create`'s body, as its Python prints it (the hire part never: a hire
/// key is the shell's). `key`: the LLM key as Python reads it.
pub fn create_body(c: &Create, key: &str) -> String {
    let mut b = vec![("label", s(&c.label))];
    let k = py::strip(key);
    if !k.is_empty() {
        b.push(("anthropicKey", s(k)));
    }
    if !py::strip(&c.provider).is_empty() {
        b.push(("provider", s(&c.provider)));
    }
    if !py::strip(&c.base_url).is_empty() {
        b.push(("baseUrl", s(&c.base_url)));
    }
    let w = py::strip(&c.workspaces);
    if w == "*" {
        b.push(("workspaces", s("*")));
    } else if !w.is_empty() {
        let slugs = py::comma_list(w);
        if !slugs.is_empty() {
            b.push(("workspaces", Value::Array(slugs)));
        }
    }
    for (key, text) in [("allow", &c.allow), ("deny", &c.deny)] {
        let t = py::strip(text);
        if !t.is_empty() {
            b.push((key, Value::Array(py::comma_list(t))));
        }
    }
    py::dumps(&obj(b))
}

/// `k5 api-key ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
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
    let ask = parse(&words)?;
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
    Some(run_with(args, ask, hook_sock.as_deref(), settings))
}

fn run_with(args: &[OsString], ask: Ask, hook_sock: Option<&Path>, mut settings: Settings) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    if let Ask::Stop(lines, exit) = &ask {
        for l in lines {
            out::stderr_line(l);
        }
        return *exit;
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let exit = match ask {
        Ask::List => {
            let left = request_piped_tcp(&mut client, &mut closing, LIST, &[]);
            let shown = list_reader(&String::from_utf8_lossy(&left.body));
            pipefail(left.exit, shown.print())
        }
        Ask::Id(verb, id) => {
            let body = py::dumps(&obj(vec![("id", s(&id))]));
            let left = post_piped(&mut client, &mut closing, verb.endpoint(), &body);
            let shown = id_reader(&String::from_utf8_lossy(&left.body), verb);
            pipefail(left.exit, shown.print())
        }
        Ask::Create(c) => create(&mut client, &mut closing, &c),
        Ask::Stop(_, exit) => exit,
    };
    closing.flush(&mut client, exit);
    exit
}

/// `cli_post_json <endpoint> <body>` on the left of a pipe.
fn post_piped(client: &mut Client, closing: &mut Closing, endpoint: &str, body: &str) -> Piped {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, endpoint, Call::PostJson(body.as_bytes())) {
        Ok(r) => {
            closing.answer(&r, passport);
            Piped {
                body: piped_body(&r),
                exit: Exit::Ok,
            }
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            end_in_pipe()
        }
        Err(e) => Piped {
            body: Vec::new(),
            exit: Exit::from_curl(e.curl_code()),
        },
    }
}

/// `cli_post_json_stdin <endpoint>` with `body` on its stdin, on the left
/// of a pipe: sent once, no line; its refusal's `permission` noted.
fn post_stdin_piped(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    body: &[u8],
) -> Piped {
    let passport = client.passport().is_some();
    match client.post_json_stdin(endpoint, body) {
        Ok(mut r) => {
            r.permission = permission::note(&r.body);
            closing.answer(&r, passport);
            Piped {
                body: piped_body(&r),
                exit: Exit::Ok,
            }
        }
        Err(e) => Piped {
            body: Vec::new(),
            exit: Exit::from_curl(e.curl_code()),
        },
    }
}

/// `create`, after the gates.
fn create(client: &mut Client, closing: &mut Closing, c: &Create) -> Exit {
    if c.status {
        if c.other_flags {
            out::stderr_line(STATUS_ONLY);
            return Exit::Usage;
        }
        let mut body = py::dumps(&obj(vec![("label", s(&c.label)), ("scope", s("status"))]));
        body.push('\n');
        let left = post_stdin_piped(client, closing, CREATE, body.as_bytes());
        let shown = status_reader(&String::from_utf8_lossy(&left.body));
        return match pipefail(left.exit, shown.print()) {
            Exit::Ok => Exit::Ok,
            _ => Exit::Error,
        };
    }
    let key = if c.key_stdin {
        if c.argv_key.is_some() {
            out::stderr_line(BOTH_KEYS);
            return Exit::Usage;
        }
        let key = read_secret(PROMPT);
        if key.is_empty() {
            out::stderr_line(NO_KEY);
            return Exit::Usage;
        }
        key
    } else {
        let key = c.argv_key.clone().unwrap_or_default();
        if !key.is_empty() {
            out::stderr_line(ARGV_WARNING);
        }
        key
    };
    if c.hire_limits {
        out::stderr_line(concat!(
            "Error: hire limits (--max-*, --hire-db, --link, --retire, --reach, --folder, ",
            "--app-login) need --hire."
        ));
        return Exit::Usage;
    }
    let body = create_body(c, &key);
    let left = post_stdin_piped(client, closing, CREATE, body.as_bytes());
    let shown = create_reader(&String::from_utf8_lossy(&left.body), &c.workspaces);
    pipefail(left.exit, shown.print())
}

/// `_cli_read_secret_stdin <prompt>`: on a terminal, the prompt on stderr
/// and one line read with echo off, then a newline on stderr; else one
/// line from stdin (the part before an end with no newline too).
pub fn read_secret(prompt: &str) -> String {
    let tty = is_tty();
    let echo = if tty {
        let _ = out::write_flushed(&mut std::io::stderr().lock(), prompt.as_bytes());
        echo_off()
    } else {
        None
    };
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    let mut stdin = std::io::stdin().lock();
    while let Ok(1) = stdin.read(&mut byte) {
        if byte[0] == b'\n' {
            break;
        }
        // bash's `read` drops a NUL.
        if byte[0] != 0 {
            line.push(byte[0]);
        }
    }
    if tty {
        restore_echo(echo);
        out::stderr_line("");
    }
    String::from_utf8_lossy(&line).into_owned()
}

#[cfg(unix)]
fn is_tty() -> bool {
    // SAFETY: isatty only reads the descriptor's state.
    unsafe { libc::isatty(0) == 1 }
}

#[cfg(not(unix))]
fn is_tty() -> bool {
    false
}

#[cfg(unix)]
fn echo_off() -> Option<libc::termios> {
    // SAFETY: tcgetattr and tcsetattr on fd 0 with a termios we own.
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) != 0 {
            return None;
        }
        let saved = t;
        t.c_lflag &= !libc::ECHO;
        libc::tcsetattr(0, libc::TCSANOW, &t);
        Some(saved)
    }
}

#[cfg(unix)]
fn restore_echo(saved: Option<libc::termios>) {
    if let Some(t) = saved {
        // SAFETY: puts back the state read before.
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &t);
        }
    }
}

#[cfg(not(unix))]
fn echo_off() -> Option<()> {
    None
}

#[cfg(not(unix))]
fn restore_echo(_saved: Option<()>) {}

/// A reader inside `try: … except Exception as e: print("Error: %s" % e,
/// file=sys.stderr); sys.exit(1)`.
fn caught(raw: &str, lines: impl FnOnce(&Value, &mut Out) -> Result<Exit, Raise>) -> Shown {
    let mut o = Out::default();
    let r = py::load(raw, 0).and_then(|d| lines(&d, &mut o));
    match r {
        Ok(exit) => Shown {
            stdout: o.stdout,
            stderr: o.stderr,
            exit,
        },
        Err(e) => Shown {
            stdout: o.stdout,
            stderr: format!("{}Error: {}\n", o.stderr, py::message(&e)),
            exit: Exit::Error,
        },
    }
}

#[derive(Default)]
struct Out {
    stdout: String,
    stderr: String,
}

impl Out {
    fn out(&mut self, line: &str) {
        self.stdout.push_str(line);
        self.stdout.push('\n');
    }
    fn err(&mut self, line: &str) {
        self.stderr.push_str(line);
        self.stderr.push('\n');
    }
}

/// `"error" in d` then `d["error"]`, for any `d`: the error, if there is
/// one, or Python's exception.
fn error_of(d: &Value) -> Result<Option<Value>, Raise> {
    if !py::py_in("error", d, 0)? {
        return Ok(None);
    }
    match d {
        Value::Object(_) => Ok(Some(at_or(d, "error", Value::Null))),
        Value::Array(_) => Err(Raise::at(
            0,
            "TypeError: list indices must be integers or slices, not str",
        )),
        _ => Err(Raise::at(
            0,
            "TypeError: string indices must be integers, not 'str'",
        )),
    }
}

/// The key's doors that are on: `caps.get(…)` for each, by name.
fn doors(caps: &Value) -> Result<Vec<Value>, Raise> {
    let caps = dict(caps, 0)?;
    Ok([
        ("host-sessions", "hostSessions"),
        ("canonical-message", "canonicalMessage"),
        ("sandboxes", "sandboxes"),
        ("db", "db"),
        ("hire", "hire"),
    ]
    .iter()
    .filter(|(_, key)| at(caps, key).is_some_and(pyout::truthy))
    .map(|(name, _)| s(name))
    .collect())
}

/// `for x in v`, or Python's `TypeError`.
fn items(v: &Value) -> Result<Vec<Value>, Raise> {
    super::py_iter(v, 0)
}

/// `list`'s reader.
pub fn list_reader(raw: &str) -> Shown {
    caught(raw, |d, o| {
        if matches!(d, Value::Object(_)) {
            if let Some(e) = at(d, "error") {
                o.err(&format!("Error: {}", pyout::str_of(e)));
                return Ok(Exit::Error);
            }
        }
        let keys = at_or(dict(d, 0)?, "keys", Value::Array(vec![]));
        if !pyout::truthy(&keys) {
            o.out("No API keys.");
            return Ok(Exit::Ok);
        }
        for k in items(&keys)? {
            let k = dict(&k, 0)?;
            let state = if at(k, "revokedAt").is_some_and(pyout::truthy) {
                "revoked"
            } else if at(k, "disabledAt").is_some_and(pyout::truthy) {
                "disabled"
            } else {
                "active"
            };
            let prov = or(at(k, "provider"), s("anthropic"));
            let cred = if at(k, "anthropicKeySet").is_some_and(pyout::truthy) {
                format!("{}-key", pyout::str_of(&prov))
            } else {
                "no-llm-key".to_string()
            };
            let mut line = format!(
                "{}  [{state}]  {}  {cred}  scope={}",
                pyout::str_of(&at_or(k, "id", s(""))),
                pyout::str_of(&or(at(k, "label"), s("-"))),
                pyout::str_of(&at_or(k, "scope", s(""))),
            );
            if let Some(u) = at(k, "baseUrl").filter(|v| pyout::truthy(v)) {
                line.push_str(&format!("  base-url={}", pyout::str_of(u)));
            }
            let caps = or(at(k, "capabilities"), Value::Object(vec![]));
            let on = doors(&caps)?;
            let on = if on.is_empty() {
                "none".to_string()
            } else {
                join(",", &Value::Array(on), 0)?
            };
            line.push_str(&format!("  caps={on}"));
            if at(k, "legacyNewWorkspace").is_some_and(pyout::truthy) {
                line.push_str("  legacy-new-workspace");
            }
            o.out(&line);
            let hire = at(&caps, "hire").is_some_and(pyout::truthy);
            let limits = at(k, "hireLimits").filter(|v| **v != Value::Null);
            if hire && limits.is_some() {
                let hl = or(limits, Value::Object(vec![]));
                let apps = or(at(dict(&hl, 0)?, "apps"), Value::Array(vec![]));
                if !pyout::truthy(&apps) {
                    o.out("    App access: none (hired agents get no App login)");
                }
                for a in items(&apps)? {
                    let a = dict(&a, 0)?;
                    let roles = or(at(a, "roles"), Value::Array(vec![]));
                    let rooms = join(",", &or(at(a, "rooms"), Value::Array(vec![])), 0)?;
                    let role = if pyout::truthy(&roles) {
                        format!("up-to-role={}", join(",", &roles, 0)?)
                    } else {
                        "no-role".to_string()
                    };
                    o.out(&format!(
                        "    App access: {} rooms={rooms} {role}",
                        pyout::str_of(&at_or(a, "app", Value::Null))
                    ));
                }
            }
        }
        Ok(Exit::Ok)
    })
}

/// `revoke`, `disable` and `enable`'s reader.
pub fn id_reader(raw: &str, verb: IdVerb) -> Shown {
    caught(raw, |d, o| {
        if let Some(e) = error_of(d)? {
            o.err(&format!("Error: {}", pyout::str_of(&e)));
            return Ok(Exit::Error);
        }
        let (yes, no) = verb.lines();
        let d = dict(d, 0)?;
        o.out(if at(d, "success").is_some_and(pyout::truthy) {
            yes
        } else {
            no
        });
        Ok(Exit::Ok)
    })
}

/// `create`'s reader. `workspaces`: `--workspaces` as given.
pub fn create_reader(raw: &str, workspaces: &str) -> Shown {
    caught(raw, |d, o| {
        if let Some(e) = error_of(d)? {
            let e = match &e {
                Value::Object(_) => s(&format!(
                    "{}: {}",
                    pyout::str_of(&at_or(&e, "code", s(""))),
                    pyout::str_of(&at_or(&e, "hint", s("")))
                )),
                _ => e,
            };
            o.err(&format!("Error: {}", pyout::str_of(&e)));
            return Ok(Exit::Error);
        }
        let d = dict(d, 0)?;
        o.out(&format!(
            "API key created (id: {}).",
            pyout::str_of(&at_or(d, "id", s("")))
        ));
        let caps = or(at(d, "capabilities"), Value::Object(vec![]));
        if pyout::truthy(&caps) {
            let on = doors(&caps)?;
            let on = if on.is_empty() {
                "(none)".to_string()
            } else {
                join(", ", &Value::Array(on), 0)?
            };
            o.out(&format!("Capabilities: {on}"));
        }
        if let Some(hl) = at(d, "hireLimits").filter(|v| pyout::truthy(v)) {
            let hl = dict(hl, 0)?;
            let g = |k: &str| pyout::str_of(&at_or(hl, k, Value::Null));
            let onoff = |k: &str| {
                if at(hl, k).is_some_and(pyout::truthy) {
                    "on"
                } else {
                    "off"
                }
            };
            let folder = match at(hl, "folder").filter(|v| pyout::truthy(v)) {
                Some(f) => format!(", folder {}", pyout::str_of(f)),
                None => String::new(),
            };
            o.out(&format!(
                "Hire limits: {}/day, {} live, {} per team, db {}, link {}, retire {}, reach {}{folder}",
                g("maxMembersPerDay"),
                g("maxLiveMembers"),
                g("maxMembersPerTeam"),
                g("db"),
                onoff("link"),
                onoff("retire"),
                g("reach"),
            ));
            for a in items(&or(at(hl, "apps"), Value::Array(vec![])))? {
                let a = dict(&a, 0)?;
                let rooms = join(",", &or(at(a, "rooms"), Value::Array(vec![])), 0)?;
                let rooms = if rooms.is_empty() { "-".to_string() } else { rooms };
                let roles = match at(a, "roles").filter(|v| pyout::truthy(v)) {
                    Some(r) => format!(", roles {}", join(",", r, 0)?),
                    None => String::new(),
                };
                o.out(&format!(
                    "App logins on {}: rooms {rooms}{roles} (whole rooms until folder grants exist)",
                    pyout::str_of(&at_or(a, "app", Value::Null))
                ));
            }
        }
        o.out("");
        o.out(&format!("  {}", pyout::str_of(&at_or(d, "key", s("")))));
        o.out("");
        o.out(STORE_NOW);
        if py::strip(workspaces).is_empty() {
            o.out("");
            for l in NO_GRANT {
                o.err(l);
            }
        }
        Ok(Exit::Ok)
    })
}

/// `create --status`'s reader (no catch-all: Python's raise is a
/// traceback).
pub fn status_reader(raw: &str) -> Shown {
    let Ok(d) = crate::out::json::parse(raw) else {
        return Shown::err("Error: no answer from the daemon\n", Exit::Error);
    };
    let mut stdout = String::new();
    let r = (|| -> Result<Exit, Raise> {
        if py::py_in("error", &d, line::status::IN)? {
            let e = match &d {
                Value::Object(_) => at_or(&d, "error", Value::Null),
                Value::Array(_) => {
                    return Err(Raise::at(
                        line::status::INDEX,
                        "TypeError: list indices must be integers or slices, not str",
                    ))
                }
                _ => {
                    return Err(Raise::at(
                        line::status::INDEX,
                        "TypeError: string indices must be integers, not 'str'",
                    ))
                }
            };
            return Err(Raise {
                frames: Vec::new(),
                exception: format!("Error: {}", pyout::str_of(&e)),
            });
        }
        let d = dict(&d, line::status::ID)?;
        stdout.push_str(&format!(
            "Status key created (id: {}). It reads this server's full /boot-status and nothing else.\n\n  {}\n\n",
            pyout::str_of(&at_or(d, "id", s(""))),
            pyout::str_of(&at_or(d, "key", s(""))),
        ));
        stdout.push_str(concat!(
            "STORE THIS KEY NOW \u{2014} it is shown only once. Send it as: Authorization: Bearer <key>\n",
            "Rotate: mint a new one, switch the monitor, then ",
            cli_name!(),
            " api-key revoke <old id>.\n"
        ));
        Ok(Exit::Ok)
    })();
    match r {
        Ok(exit) => Shown {
            stdout,
            stderr: String::new(),
            exit,
        },
        // The daemon's error: its line, exit 1.
        Err(Raise { frames, exception }) if frames.is_empty() => {
            Shown::err(format!("{exception}\n"), Exit::Error)
        }
        Err(r) => raised(stdout, String::new(), &r),
    }
}

/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    /// `create --status`'s reader.
    pub mod status {
        pub const IN: u32 = 6;
        pub const INDEX: u32 = 7;
        pub const ID: u32 = 8;
    }
}

#[cfg(test)]
mod tests;
