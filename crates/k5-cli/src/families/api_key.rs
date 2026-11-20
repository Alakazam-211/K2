//! `k5 api-key`, the public /v1 API's keys, as the shell CLI's
//! `cmd_api_key` answers it (`cli/src/talk-and-workspace.sh`): `create`
//! (and `create --status` and `create --hire`), `list`, `hire-ceiling`
//! (and its alias `app-access`), `revoke`, `disable` and `enable`.
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
//!    `--hire`: first the owner's confirm over the vault socket
//!    ([`confirm::confirm_token`], `hire-key-mint`; none is exit 3), then
//!    the body with its `hire` limits and the token
//!    ([`confirm::with_confirm_token`]). A limit Python's `int()` refuses
//!    is its line and an empty body, sent all the same, as the shell's
//!    pipe does ([`create_body`]).
//! 5. `hire-ceiling <id>`: the body `{"id": …, "apps": […]}` sent with
//!    `cli_post_json_stdin` inside `$(…)` (no answer ends the command with
//!    curl's code); a `vault_confirm_required` answer asks for the owner's
//!    confirm and sends it again with the token; then the printer
//!    ([`ceiling_reader`]).
//! 6. Every reader but `--status`'s and `hire-ceiling`'s catches Python's
//!    exceptions and prints `Error: <message>` (exit 1); the message is
//!    Python 3.12's. Those two print Python's traceback.
//! 7. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The shell answers itself (`run` returns `None`): a hire limit
//! (`--max-per-day`, `--max-live`, `--max-per-team`) that is not ASCII,
//! where Python's `int()` reads other digits; a line with `-h` or
//! `--help`; a word that is not UTF-8; `HOOK_SOCK` with a `CODEX_`
//! variable exported; a port curl won't take; and a `python3` the binary
//! can't tell.

use super::skin_token::py::{self, at, at_or, dict, join, obj, or, s};
use super::{
    curl_port, end_in_pipe, piped_body, pipefail, raised, request_piped_tcp, Piped, Raise, Shown,
};
use crate::confirm::{self, Confirm};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::{self, py as pyout};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::prompt;
use crate::python;
use crate::transport::{permission, Call, Channel, Client, Error};
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["api-key"];
/// The binary answers it ([`run`] may still hand a form to the shell).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
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
pub const HIRE_CEILING: &str = "/cli/api-keys/hire-ceiling";
/// The confirm a hire key asks for (`_vault_confirm_token`'s action).
pub const HIRE_ACTION: &str = "hire-key-mint";
/// `create --hire`'s confirm label.
pub const HIRE_LABEL: &str = "API key that can hire agents";
/// `hire-ceiling`'s confirm label, when the daemon asks for one.
pub const WIDEN_LABEL: &str = "Widen an API key's App access";
/// The code that asks for the owner's confirm.
pub const CONFIRM_REQUIRED: &str = "vault_confirm_required";

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
/// `hire-ceiling` with no id, or one that starts with `-`: exit 2.
pub const CEILING_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " api-key hire-ceiling <id> (--app-login APP[:ROOMS[:ROLE]]... | --none)"
);
/// `--none` with `--app-login`: exit 2.
pub const CEILING_BOTH: &str = "Error: --none and --app-login don't mix.";
/// Neither: exit 2.
pub const CEILING_NEITHER: &str =
    "Error: name the App access (--app-login APP[:ROOMS[:ROLE]]) or --none for no App access.";
/// A hire limit given without `--hire`: exit 2.
pub const LIMITS_NEED_HIRE: &str = concat!(
    "Error: hire limits (--max-*, --hire-db, --link, --retire, --reach, --folder, ",
    "--app-login) need --hire."
);
/// The hire limits that are numbers, with their body keys, in Python's
/// order.
pub const NUMBERS: [(&str, &str); 3] = [
    ("--max-per-day", "maxMembersPerDay"),
    ("--max-live", "maxLiveMembers"),
    ("--max-per-team", "maxMembersPerTeam"),
];
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
    /// `--hire`.
    pub hire: bool,
    /// `--link`, `--retire`.
    pub link: bool,
    pub retire: bool,
    /// `h_apps`: each `--app-login` value and a newline.
    pub apps: String,
}

impl Create {
    /// A hire value flag's last value (`""` when not given).
    fn limit(&self, flag: &str) -> &str {
        self.limit_values
            .iter()
            .find(|(f, _)| f == flag)
            .map_or("", |(_, v)| v.as_str())
    }
}

/// `hire-ceiling <id>`'s words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ceiling {
    pub id: String,
    /// `c_apps`: each `--app-login` value and a newline.
    pub apps: String,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Create(Create),
    Ceiling(Ceiling),
    List,
    Id(IdVerb, String),
    /// Lines on stderr and the exit.
    Stop(Vec<String>, Exit),
}

/// The words after `api-key`. `None`: the shell's (a hire number that is
/// not ASCII).
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
        "hire-ceiling" | "app-access" => Some(parse_ceiling(rest)),
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
            "--hire" => {
                c.hire = true;
                takes = false;
            }
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
                if flag == "--link" {
                    c.link = true;
                } else {
                    c.retire = true;
                }
                takes = false;
            }
            "--label" | "--max-per-day" | "--max-live" | "--max-per-team" | "--hire-db"
            | "--link-targets" | "--reach" | "--folder" | "--app-login" | "--anthropic-key"
            | "--llm-key" | "--provider" | "--base-url" | "--workspaces" | "--allow" | "--deny" => {
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
                    "--app-login" => {
                        c.hire_limits = true;
                        c.apps.push_str(&v);
                        c.apps.push('\n');
                    }
                    _ => {
                        c.limit_values.retain(|(f, _)| f != flag);
                        c.limit_values.push((flag.to_string(), v));
                    }
                }
            }
            other => {
                return Some(Ask::Stop(
                    vec![format!("Unknown flag: {other}")],
                    Exit::Error,
                ));
            }
        }
        i += if takes { 2 } else { 1 };
    }
    c.hire_limits |= c.limit_values.iter().any(|(_, v)| !v.is_empty());
    // Python's `int()` reads digits other than ASCII ones: the shell's.
    if c.hire && NUMBERS.iter().any(|(f, _)| !c.limit(f).is_ascii()) {
        return None;
    }
    Some(Ask::Create(c))
}

/// `hire-ceiling|app-access`'s words: the id, then its flags.
fn parse_ceiling(rest: &[&str]) -> Ask {
    let usage = |line: &str| Ask::Stop(vec![line.to_string()], Exit::Usage);
    let id = rest.first().copied().unwrap_or("");
    if id.is_empty() || id.starts_with('-') {
        return usage(CEILING_USAGE);
    }
    let mut c = Ceiling {
        id: id.to_string(),
        apps: String::new(),
    };
    let mut none = false;
    let mut i = 1;
    while i < rest.len() {
        match rest[i] {
            "--app-login" => {
                // `${2:-}; shift 2`: no value, and `shift 2` fails.
                let Some(v) = rest.get(i + 1) else {
                    return Ask::Stop(Vec::new(), Exit::Error);
                };
                c.apps.push_str(v);
                c.apps.push('\n');
                i += 2;
            }
            "--none" => {
                none = true;
                i += 1;
            }
            other => return usage(&format!("Unknown flag: {other}")),
        }
    }
    match (none, c.apps.is_empty()) {
        (true, false) => usage(CEILING_BOTH),
        (false, true) => usage(CEILING_NEITHER),
        _ => Ask::Ceiling(c),
    }
}

/// The `--app-login` lines as Python reads them: `APP[:ROOMS[:ROLES]]`,
/// rooms and roles comma lists.
pub fn app_logins(apps: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for line in py::splitlines(apps) {
        let line = py::strip(line);
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split(':').collect();
        let mut a = vec![("app", s(parts[0]))];
        for (n, key) in [(1, "rooms"), (2, "roles")] {
            if let Some(p) = parts.get(n).filter(|p| !py::strip(p).is_empty()) {
                a.push((key, Value::Array(py::comma_list(p))));
            }
        }
        out.push(obj(a));
    }
    out
}

/// Python's `int(v)` of an ASCII word: optional sign, digits with single
/// `_` between them. `None`: Python's `ValueError`.
pub fn py_int(v: &str) -> Option<String> {
    let (neg, digits) = match v.as_bytes().first() {
        Some(b'-') => (true, &v[1..]),
        Some(b'+') => (false, &v[1..]),
        _ => (false, v),
    };
    let ok = !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
        && !digits.starts_with('_')
        && !digits.ends_with('_')
        && !digits.contains("__");
    if !ok {
        return None;
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    let n = clean.trim_start_matches('0');
    Some(match (neg, n.is_empty()) {
        (_, true) => "0".to_string(),
        (true, false) => format!("-{n}"),
        (false, false) => n.to_string(),
    })
}

/// `create`'s body, as its Python prints it. `key`: the LLM key as Python
/// reads it. `Err`: the line Python prints for a hire number it can't
/// read (it exits 2 and prints no body).
pub fn create_body(c: &Create, key: &str) -> Result<String, String> {
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
    if c.hire {
        b.push(("hire", hire_limits(c)?));
    }
    Ok(py::dumps(&obj(b)))
}

/// The body's `hire` dict, in Python's order.
fn hire_limits(c: &Create) -> Result<Value, String> {
    let mut h = Vec::new();
    for (flag, key) in NUMBERS {
        let v = py::strip(c.limit(flag));
        if !v.is_empty() {
            match py_int(v) {
                Some(n) => h.push((key, Value::Int(n))),
                None => return Err(format!("Error: {key} must be a number")),
            }
        }
    }
    let db = py::strip(c.limit("--hire-db"));
    if !db.is_empty() {
        h.push(("db", s(db)));
    }
    if c.link {
        h.push(("link", Value::Bool(true)));
    }
    let t = py::strip(c.limit("--link-targets"));
    if !t.is_empty() {
        h.push(("linkTargets", Value::Array(py::comma_list(t))));
    }
    if c.retire {
        h.push(("retire", Value::Bool(true)));
    }
    for (flag, key) in [("--reach", "reach"), ("--folder", "folder")] {
        let v = py::strip(c.limit(flag));
        if !v.is_empty() {
            h.push((key, s(v)));
        }
    }
    let apps = app_logins(&c.apps);
    if !apps.is_empty() {
        h.push(("apps", Value::Array(apps)));
    }
    Ok(obj(h))
}

/// `hire-ceiling`'s body (`$(…)` drops Python's newline).
pub fn ceiling_body(c: &Ceiling) -> String {
    py::dumps(&obj(vec![
        ("id", s(&c.id)),
        ("apps", Value::Array(app_logins(&c.apps))),
    ]))
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
    let sock = confirm::sock_path(&settings.home);
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
        Ask::Create(c) => create(&mut client, &mut closing, &c, &sock),
        Ask::Ceiling(c) => ceiling(&mut client, &mut closing, &c, &sock),
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

/// `tok="$(_vault_confirm_token hire-key-mint "" <label>)" || exit 3`.
/// `None`: exit 3.
fn confirm_hire(
    client: &mut Client,
    closing: &mut Closing,
    sock: &Path,
    label: &str,
) -> Option<String> {
    let passport = client.passport().is_some();
    let mut c = Confirm {
        client,
        sock,
        closing,
        passport,
    };
    confirm::confirm_token(&mut c, HIRE_ACTION, "", label)
}

/// `create`, after the gates.
fn create(client: &mut Client, closing: &mut Closing, c: &Create, sock: &Path) -> Exit {
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
        let key = prompt::read_secret_stdin(PROMPT);
        if key.is_empty() {
            out::stderr_line(NO_KEY);
            return Exit::Usage;
        }
        String::from_utf8_lossy(key.expose()).into_owned()
    } else {
        let key = c.argv_key.clone().unwrap_or_default();
        if !key.is_empty() {
            out::stderr_line(ARGV_WARNING);
        }
        key
    };
    if !c.hire && c.hire_limits {
        out::stderr_line(LIMITS_NEED_HIRE);
        return Exit::Usage;
    }
    let tok = if c.hire {
        match confirm_hire(client, closing, sock, HIRE_LABEL) {
            Some(t) => t,
            None => return Exit::Refused,
        }
    } else {
        String::new()
    };
    // The body's Python, then the group that adds the token: a body
    // Python refused is its line and nothing on the pipe, and curl sends
    // that empty body all the same.
    let (body, made) = match create_body(c, &key) {
        Ok(b) if !tok.is_empty() => match confirm::with_confirm_token(&b, &tok, None) {
            Some(with) => (with + "\n", Exit::Ok),
            None => (b, Exit::Ok),
        },
        Ok(b) => (b, Exit::Ok),
        Err(line) => {
            out::stderr_line(&line);
            (String::new(), Exit::Usage)
        }
    };
    let left = post_stdin_piped(client, closing, CREATE, body.as_bytes());
    let shown = create_reader(&String::from_utf8_lossy(&left.body), &c.workspaces);
    pipefail(pipefail(made, left.exit), shown.print())
}

/// `c_resp="$(printf '%s' "$c_body" | cli_post_json_stdin …)"`: the
/// answer as `$(…)` holds it, or the exit `set -e` ends the command with.
fn ceiling_post(client: &mut Client, closing: &mut Closing, body: &str) -> Result<String, Exit> {
    let left = post_stdin_piped(client, closing, HIRE_CEILING, body.as_bytes());
    if left.exit != Exit::Ok {
        return Err(left.exit);
    }
    // `piped_body` has dropped the NULs.
    let mut body = left.body;
    while body.last() == Some(&b'\n') {
        body.pop();
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// `hire-ceiling`, after the gates.
fn ceiling(client: &mut Client, closing: &mut Closing, c: &Ceiling, sock: &Path) -> Exit {
    let body = ceiling_body(c);
    let mut resp = match ceiling_post(client, closing, &body) {
        Ok(r) => r,
        Err(exit) => return exit,
    };
    if error_code(&resp) == CONFIRM_REQUIRED {
        let Some(tok) = confirm_hire(client, closing, sock, WIDEN_LABEL) else {
            return Exit::Refused;
        };
        let body = confirm::with_confirm_token(&body, &tok, None).unwrap_or(body);
        resp = match ceiling_post(client, closing, &body) {
            Ok(r) => r,
            Err(exit) => return exit,
        };
    }
    ceiling_reader(&resp).print()
}

/// `c_code`: the answer's `error.code` when the error is a dict, else
/// `""`, as `$(…)` holds Python's line.
pub fn error_code(resp: &str) -> String {
    let Ok(d) = crate::out::json::parse(resp) else {
        return String::new();
    };
    // `at` is `None` for anything but a dict: Python's AttributeError.
    match at(&d, "error") {
        Some(e @ Value::Object(_)) => pyout::str_of(&at_or(e, "code", s("")))
            .trim_end_matches('\n')
            .to_string(),
        _ => String::new(),
    }
}

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
                let rooms = if rooms.is_empty() {
                    "-".to_string()
                } else {
                    rooms
                };
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

/// `hire-ceiling`'s reader (Python's raise past the load is a
/// traceback).
pub fn ceiling_reader(raw: &str) -> Shown {
    let Ok(d) = crate::out::json::parse(raw) else {
        return Shown::err("Error: no answer from the daemon\n", Exit::Error);
    };
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let e = match &d {
            Value::Object(_) => at(&d, "error").filter(|e| pyout::truthy(e)),
            _ => None,
        };
        if let Some(e) = e {
            let e = match e {
                Value::Object(_) => format!(
                    "{}: {}",
                    pyout::str_of(&at_or(e, "code", s(""))),
                    pyout::str_of(&at_or(e, "hint", s("")))
                ),
                other => pyout::str_of(other),
            };
            o.err(&format!("Error: {e}"));
            return Ok(Exit::Error);
        }
        let limits = or(at(dict(&d, line::ceiling::APPS)?, "hireLimits"), obj(vec![]));
        let apps = or(at(dict(&limits, line::ceiling::APPS)?, "apps"), Value::Array(vec![]));
        let raised = at(&d, "change").is_some_and(|c| *c == s("raise"));
        o.out(&format!(
            "App access {} for key {}.",
            if raised { "widened" } else { "updated" },
            pyout::str_of(&at_or(&d, "id", s("")))
        ));
        if !pyout::truthy(&apps) {
            o.out("No App access: agents hired with this key get no App login.");
        }
        for a in super::py_iter(&apps, line::ceiling::FOR)? {
            let a = dict(&a, line::ceiling::ROLES)?;
            let roles = or(at(a, "roles"), Value::Array(vec![]));
            let app = pyout::str_of(&at_or(a, "app", Value::Null));
            let rooms = join(
                ",",
                &or(at(a, "rooms"), Value::Array(vec![])),
                line::ceiling::PRINT,
            )?;
            let role = if pyout::truthy(&roles) {
                format!("up to role {}", join(",", &roles, line::ceiling::PRINT)?)
            } else {
                "no role (Thread only in those rooms)".to_string()
            };
            o.out(&format!("  {app}: rooms {rooms}, {role}"));
        }
        Ok(Exit::Ok)
    })();
    match r {
        Ok(exit) => Shown {
            stdout: o.stdout,
            stderr: o.stderr,
            exit,
        },
        Err(r) => raised(o.stdout, o.stderr, &r),
    }
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
    /// `hire-ceiling`'s reader.
    pub mod ceiling {
        pub const APPS: u32 = 10;
        pub const FOR: u32 = 14;
        pub const ROLES: u32 = 15;
        pub const PRINT: u32 = 16;
    }
    /// `create --status`'s reader.
    pub mod status {
        pub const IN: u32 = 6;
        pub const INDEX: u32 = 7;
        pub const ID: u32 = 8;
    }
}

#[cfg(test)]
mod tests;
