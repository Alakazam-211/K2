//! `k5 desktop`, as the shell CLI's `cmd_desktop` answers it
//! (`cli/src/desktop.sh`): this computer's screen in a Desktop tab, for
//! people, and an agent's own virtual desktop (`env`). Every form:
//! `status` (also no verb, or `--json` first), `list`, `start`, `stop`
//! (`close`), `setup`, `enable` (`on`), `ask`, `disable` (`off`),
//! `policy`, `grant`, `revoke`, `grants`, `viewers`, `kick`, `audit`,
//! `open`, `displays`, `env`, `allow`, each with `--json`, and the help.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), then the virtual desktop's guard
//!    and the connection gate ([`crate::prelude::gate`]); `-h` or
//!    `--help` anywhere skips the guard, nothing skips the gate.
//! 2. `desktop env` is an `id` tool (`desktop`), so with `HOOK_SOCK` set:
//!    the stale-passport or require-passport lines, exit 3. The other
//!    verbs are no tool's.
//! 3. The words ([`parse`]), then the verb's checks ([`request`]): each
//!    problem is one `usage:` line, exit 2.
//! 4. `_sidecar_send` (as [`super::job`] sends it), then `_desktop_render`
//!    ([`render`]): no answer is `daemon unreachable`, exit 1; a refusal
//!    is its Python's lines ([`refusal_lines`]) and `_sidecar_exit_for`'s
//!    exit, with the body after them for `--json`; a 200 is the body for
//!    `--json`, else its Python ([`view_lines`]).
//! 5. `setup` and `env` wait as the shell does ([`setup_wait`],
//!    [`env_loop`]): another call every 2 s while the server installs or
//!    the desktop is on its way, up to `--timeout`.
//!
//! The shell CLI answers these itself ([`run`] returns `None`): a word
//! that is not UTF-8, `HOOK_SOCK` set with a `CODEX_` variable exported,
//! a port that is set but is no port curl takes, a `python3` the binary
//! can't tell, and a `--timeout` of more than 12 digits that count, which
//! its arithmetic reads. A `--timeout` that isn't digits and one unit
//! letter is the usage line, exit 2, before anything is sent
//! ([`timeout_secs`], 0f25406366).
//!
//! The renderers' Python reads the answer from an environment variable,
//! which Linux caps at 128 KiB: above it the shell stops on python3's
//! "Argument list too long". This prints the lines Python would have
//! printed. With `--json` no Python reads it, so both print it whole.

use super::compute::lines::py_line;
use super::firewall::{at, dict_get, dumps, or, plus_str, push, Rendered};
use super::job::held;
use super::{curl_port, py_d, py_iter, py_left, py_space, raised, s, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::pylib::json::loads;
use crate::out::pylib::ops::{contains, index, join};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Gate};
use crate::python;
use crate::transport::{encode, Client};
use k2_client::brand::CLI_NAME as CLI;
use k2_client::{cli_name, product_name};
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["desktop"];
/// The binary answers every form ([`run`] may still hand one to the
/// shell CLI).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: the whole row is native.
pub const VERBS: &[crate::route::Verb] = &[];

/// The routes, under `/cli/desktop/`.
pub const ROUTE: &str = "/cli/desktop/";
/// `desktop env`'s tool in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "desktop";
/// No answer.
pub const UNREACHABLE: &str = concat!(cli_name!(), " desktop: daemon unreachable");
/// What the refusal's Python starts each error line with.
pub const ERROR: &str = concat!(cli_name!(), " desktop: error: ");
/// No answer while `setup` waits.
pub const UNREACHABLE_WAITING: &str = concat!(
    cli_name!(),
    " desktop: daemon unreachable while waiting for the install (",
    cli_name!(),
    " desktop status shows how it ends)"
);
/// `setup`'s first line on stderr while it waits.
pub const INSTALLING: &str =
    "Installing the desktop packages on the server (this can take a few minutes)\u{2026}";
/// `setup --no-wait` (or `setup` that does not wait) while it installs.
pub const INSTALLING_NO_WAIT: &str = concat!(
    "Installing the desktop packages on the server. ",
    cli_name!(),
    " desktop status says setting_up until it ends."
);
/// `setup`, installed.
pub const INSTALLED: &str = concat!("Installed. ", cli_name!(), " desktop start starts it.");

/// The usage line of each flag `shift 2` finds with no value.
fn value_usage(flag: &str) -> String {
    match flag {
        "--desktop" => format!(
            "usage: --desktop <id> (virtual, virtual:<workspace id>, console; {CLI} desktop list)"
        ),
        "--size" => "usage: --size WxH (1280x720 to 1920x1080)".into(),
        "--timeout" => {
            format!("usage: --timeout 10m|90s|600 (how long {CLI} desktop setup waits)")
        }
        "--open" => format!("usage: --open screen|accessibility ({CLI} desktop ask)"),
        "--workspace" => format!("usage: --workspace <path|name> ({CLI} desktop allow)"),
        "--since" => "usage: --since 1d|2h|30m|<RFC 3339 time>".into(),
        "--limit" => "usage: --limit N".into(),
        "--display" => format!("usage: --display <id> ({CLI} desktop displays)"),
        _ => "usage: --hide-app <bundle id>".into(),
    }
}

/// The flags that take a value, as `--flag value` or `--flag=value`.
const VALUE_FLAGS: &[&str] = &[
    "--desktop",
    "--size",
    "--timeout",
    "--open",
    "--workspace",
    "--since",
    "--limit",
    "--display",
    "--hide-app",
];

/// What the words ask for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ask {
    /// The verb as typed (`status` for none, or `--json` first).
    pub sub: String,
    pub json: bool,
    pub desk: String,
    pub size: String,
    pub wait_for: String,
    pub no_wait: bool,
    pub release: bool,
    pub pane: String,
    pub check: bool,
    pub ws: String,
    pub since: String,
    pub limit: String,
    pub display: String,
    pub hides: Vec<String>,
    pub no_hide: bool,
    pub args: Vec<String>,
}

/// What `cmd_desktop` does with its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// `_desktop_usage`: the help, exit 0.
    Help,
    /// The `usage:` line (no newline), exit 2.
    Early(String),
    /// The verb's checks to make.
    Call(Box<Ask>),
}

/// `cmd_desktop`'s words (after `desktop`).
pub fn parse(words: &[&str]) -> Parsed {
    let first = words.first().copied();
    let mut sub = first.unwrap_or("status").to_string();
    let mut rest: Vec<&str> = words.get(1..).unwrap_or_default().to_vec();
    match sub.as_str() {
        "help" | "--help" | "-h" => return Parsed::Help,
        "--json" => {
            sub = "status".into();
            rest.insert(0, "--json");
        }
        _ => {}
    }
    let mut a = Ask {
        sub,
        ..Ask::default()
    };
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        if VALUE_FLAGS.contains(&w) {
            let Some(v) = rest.get(i + 1) else {
                return Parsed::Early(value_usage(w));
            };
            set_value(&mut a, w, v);
            i += 2;
            continue;
        }
        if let Some((flag, v)) = w.split_once('=') {
            if VALUE_FLAGS.contains(&flag) {
                set_value(&mut a, flag, v);
                i += 1;
                continue;
            }
        }
        match w {
            "--json" => a.json = true,
            "--no-wait" => a.no_wait = true,
            "--release" => a.release = true,
            "--check" => a.check = true,
            "--no-hide-apps" => a.no_hide = true,
            "--view" => {}
            "--control" => {
                return Parsed::Early(
                    "usage: grants are view only: Members never drive (Owners and Admins drive without a grant). Use --view."
                        .into(),
                )
            }
            _ if w == "--ttl" || w.starts_with("--ttl=") => {
                return Parsed::Early(format!(
                    "usage: grants have no length: they last until {CLI} desktop revoke <login>"
                ))
            }
            _ if ["--consent", "--active-window"]
                .iter()
                .any(|f| w == *f || w.starts_with(&format!("{f}="))) =>
            {
                return Parsed::Early(format!(
                    "usage: there is no consent policy in this version (no Allow click at the screen); {CLI} desktop policy takes --hide-app"
                ))
            }
            "-h" | "--help" => return Parsed::Help,
            _ if w.starts_with('-') => {
                return Parsed::Early(format!("usage: unknown flag '{w}' ({CLI} desktop --help)"))
            }
            _ => a.args.push(w.to_string()),
        }
        i += 1;
    }
    Parsed::Call(Box::new(a))
}

fn set_value(a: &mut Ask, flag: &str, v: &str) {
    let v = v.to_string();
    match flag {
        "--desktop" => a.desk = v,
        "--size" => a.size = v,
        "--timeout" => a.wait_for = v,
        "--open" => a.pane = v,
        "--workspace" => a.ws = v,
        "--since" => a.since = v,
        "--limit" => a.limit = v,
        "--display" => a.display = v,
        _ => a.hides.push(v),
    }
}

/// What `_desktop_secs` makes of a `--timeout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timeout {
    /// Digits and one unit letter or none, read as decimal (`10#`).
    Secs(u64),
    /// Anything else: the verb's usage line, exit 2, before anything is
    /// sent.
    Bad,
    /// More than 12 digits that count: the binary leaves it to the shell's
    /// arithmetic.
    Long,
}

/// `_desktop_secs`: a `--timeout` of digits and one unit letter or none
/// (`^[0-9]+[smh]?$`), read as decimal.
pub fn timeout_secs(v: &str) -> Timeout {
    let (digits, mult) = match v.as_bytes().last() {
        Some(b'h') => (&v[..v.len() - 1], 3600),
        Some(b'm') => (&v[..v.len() - 1], 60),
        Some(b's') => (&v[..v.len() - 1], 1),
        _ => (v, 1),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Timeout::Bad;
    }
    let counted = digits.trim_start_matches('0');
    if counted.len() > 12 {
        return Timeout::Long;
    }
    // All zeros is 0.
    Timeout::Secs(counted.parse::<u64>().unwrap_or(0) * mult)
}

/// `setup`'s usage line for a bad `--timeout`.
pub const SETUP_TIMEOUT_USAGE: &str = "usage: --timeout 10m|90s|600";
/// `env`'s usage line for a bad `--timeout`.
pub const ENV_TIMEOUT_USAGE: &str = "usage: --timeout 2m|90s|120";

/// What a verb sends, and how its answer shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// One call, then `_desktop_render <verb>`.
    One {
        get: bool,
        path: String,
        payload: String,
        verb: &'static str,
    },
    /// `setup`'s POST, then [`setup_wait`].
    Setup,
    /// [`env_loop`]'s POSTs, with this body.
    Env(String),
}

fn one(get: bool, route: &str, payload: impl Into<String>, verb: &'static str) -> Request {
    Request::One {
        get,
        path: format!("{ROUTE}{route}"),
        payload: payload.into(),
        verb,
    }
}

/// `_desktop_body <json> <desktop>`: the object with `"desktop"` added
/// (Python's `json.dumps`), or the JSON as it was with no desktop.
fn with_desk(pairs: Vec<(&str, Value)>, literal: &str, desk: &str) -> String {
    if desk.is_empty() {
        return if literal.is_empty() {
            dumps(pairs)
        } else {
            literal.to_string()
        };
    }
    let mut pairs = pairs;
    pairs.push(("desktop", s(desk)));
    dumps(pairs)
}

/// `int(word)` of a word of digits, as `json.dumps` writes it.
fn int_of(digits: &str) -> Value {
    let t = digits.trim_start_matches('0');
    Value::Int(if t.is_empty() { "0".into() } else { t.into() })
}

/// The verb's checks, in the shell's order, and its request. `Err`: the
/// `usage:` line (no newline), exit 2. `project`: `$PROJECT`.
pub fn request(a: &Ask, project: &str) -> Result<Request, String> {
    let sub = a.sub.as_str();
    let dq = if a.desk.is_empty() {
        String::new()
    } else {
        format!("desktop={}", encode::quote(&a.desk))
    };
    if !a.size.is_empty() && sub != "start" {
        return Err(format!("usage: --size goes with {CLI} desktop start"));
    }
    if a.no_wait && sub != "setup" {
        return Err(format!("usage: --no-wait goes with {CLI} desktop setup"));
    }
    if !a.wait_for.is_empty() && sub != "setup" && sub != "env" {
        return Err(format!(
            "usage: --timeout goes with {CLI} desktop setup or {CLI} desktop env"
        ));
    }
    if a.release && sub != "env" {
        return Err(format!("usage: --release goes with {CLI} desktop env"));
    }
    if !a.ws.is_empty() && sub != "allow" {
        return Err(format!("usage: --workspace goes with {CLI} desktop allow"));
    }
    if (!a.pane.is_empty() || a.check) && sub != "ask" {
        return Err(format!(
            "usage: --open and --check go with {CLI} desktop ask"
        ));
    }
    let empty = || Vec::<(&str, Value)>::new();
    Ok(match sub {
        "status" => one(true, "status", dq, "status"),
        "list" => one(true, "list", "", "list"),
        "start" => {
            let mut pairs = empty();
            if !a.size.is_empty() {
                let lower = a.size.to_ascii_lowercase();
                let parts: Vec<&str> = lower.split('x').collect();
                let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
                if parts.len() != 2 || !digits(parts[0]) || !digits(parts[1]) {
                    return Err(
                        "usage: --size WxH, e.g. 1600x900 (1280x720 to 1920x1080)".into()
                    );
                }
                pairs = vec![("width", int_of(parts[0])), ("height", int_of(parts[1]))];
            }
            let literal = if pairs.is_empty() { "{}" } else { "" };
            one(false, "start", with_desk(pairs, literal, &a.desk), "status_start")
        }
        "stop" | "close" => one(false, "stop", with_desk(empty(), "{}", &a.desk), "stop"),
        "setup" => Request::Setup,
        "enable" | "on" => one(false, "enable", with_desk(empty(), "{}", &a.desk), "enable"),
        "ask" => {
            if !a.args.is_empty() {
                return Err(format!(
                    "usage: {CLI} desktop ask [--open screen|accessibility | --check]"
                ));
            }
            if !a.pane.is_empty() && a.check {
                return Err(format!(
                    "usage: {CLI} desktop ask takes --open or --check, not both"
                ));
            }
            let body = match a.pane.as_str() {
                "" if a.check => r#"{"check":true}"#.to_string(),
                "" => "{}".to_string(),
                "screen" | "accessibility" => format!(r#"{{"open":"{}"}}"#, a.pane),
                _ => return Err("usage: --open screen|accessibility".into()),
            };
            one(false, "ask", body, "ask")
        }
        "disable" | "off" => one(false, "disable", with_desk(empty(), "{}", &a.desk), "disable"),
        "policy" => {
            if a.no_hide && !a.hides.is_empty() {
                return Err("usage: --no-hide-apps or --hide-app, not both".into());
            }
            if !a.no_hide && a.hides.is_empty() {
                return Err(format!(
                    "usage: {CLI} desktop policy --hide-app <bundle id> [--hide-app \u{2026}] | --no-hide-apps"
                ));
            }
            let list = Value::Array(a.hides.iter().map(|h| s(h)).collect());
            one(false, "policy", dumps(vec![("hideApps", list)]), "policy")
        }
        "grant" => {
            if a.args.len() != 1 {
                return Err(format!("usage: {CLI} desktop grant <login> [--view]"));
            }
            let pairs = vec![("user", s(&a.args[0])), ("mode", s("view"))];
            one(false, "grant", with_desk(pairs, "", &a.desk), "grant")
        }
        "revoke" => {
            if a.args.len() != 1 {
                return Err(format!("usage: {CLI} desktop revoke <login>"));
            }
            let pairs = vec![("user", s(&a.args[0]))];
            one(false, "revoke", with_desk(pairs, "", &a.desk), "revoke")
        }
        "grants" => one(true, "grants", dq, "grants"),
        "viewers" => one(true, "viewers", dq, "viewers"),
        "kick" => {
            if a.args.len() != 1 {
                return Err(format!(
                    "usage: {CLI} desktop kick <viewer-id|login> ({CLI} desktop viewers lists them)"
                ));
            }
            let who = &a.args[0];
            let key = if who.starts_with("dv_") { "viewer" } else { "user" };
            one(false, "kick", with_desk(vec![(key, s(who))], "", &a.desk), "kick")
        }
        "audit" => {
            if !a.limit.is_empty() && !a.limit.bytes().all(|b| b.is_ascii_digit()) {
                return Err("usage: --limit N".into());
            }
            let limit = if a.limit.is_empty() { "50" } else { &a.limit };
            let mut qs = format!("limit={limit}");
            if !a.since.is_empty() {
                qs.push_str(&format!("&since={}", encode::quote(&a.since)));
            }
            one(true, "audit", qs, "audit")
        }
        "open" => {
            let (pairs, literal) = if a.display.is_empty() {
                (empty(), "{}")
            } else {
                (vec![("display", s(&a.display))], "")
            };
            one(false, "open", with_desk(pairs, literal, &a.desk), "open")
        }
        "displays" => one(true, "displays", dq, "displays"),
        "env" => {
            if !a.args.is_empty() {
                return Err(format!(
                    "usage: {CLI} desktop env [--release] [--timeout 2m] [--json]"
                ));
            }
            let (pairs, literal) = if a.release {
                (vec![("release", Value::Bool(true))], r#"{"release": true}"#)
            } else {
                (empty(), "{}")
            };
            Request::Env(with_desk(pairs, literal, &a.desk))
        }
        "allow" => {
            let usage = format!("usage: {CLI} desktop allow [on|off] [--workspace <ws>]");
            if !a.desk.is_empty() {
                return Err(format!("{usage} (no --desktop)"));
            }
            if a.args.len() > 1 {
                return Err(usage);
            }
            let ws = if a.ws.is_empty() { project } else { &a.ws };
            match a.args.first().map_or("", String::as_str) {
                on @ ("on" | "off") => {
                    let body = dumps(vec![
                        ("workspace", s(ws)),
                        ("toggle", s("desktop")),
                        ("value", Value::Bool(on == "on")),
                    ]);
                    Request::One {
                        get: false,
                        path: "/cli/agent-access/set".into(),
                        payload: body,
                        verb: "allow",
                    }
                }
                "" | "status" => one(
                    true,
                    "status",
                    format!("workspace={}", encode::quote(ws)),
                    "allow",
                ),
                _ => return Err(usage),
            }
        }
        _ => {
            return Err(format!(
                "usage: unknown desktop verb '{sub}' (status|enable|disable|ask|policy|grant|revoke|grants|viewers|kick|audit|open|displays|list|start|stop|setup|allow)"
            ))
        }
    })
}

/// Whether a command line skips the virtual desktop's guard: `-h` or
/// `--help` before any bare `--` (`_k2_offline_verb`).
fn offline_verb(words: &[&str]) -> bool {
    words
        .iter()
        .take_while(|w| **w != "--")
        .any(|w| matches!(*w, "-h" | "--help"))
}

/// `k5 desktop ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let all: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    let words = all.get(1..).unwrap_or_default();
    let parsed = parse(words);
    if let Parsed::Call(a) = &parsed {
        // Past 12 digits: the shell's arithmetic.
        let waits = a.sub == "setup" || a.sub == "env";
        if waits && !a.wait_for.is_empty() && timeout_secs(&a.wait_for) == Timeout::Long {
            return None;
        }
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
    let gate = prelude::gate(&settings);
    if gate.is_none() && settings.port_text.as_deref().and_then(curl_port).is_none() {
        return None;
    }
    let env_tool = words.first() == Some(&"env");
    Some(run_with(
        args,
        &parsed,
        offline_verb(&all),
        env_tool,
        hook_sock.as_deref(),
        settings,
    ))
}

fn run_with(
    args: &[OsString],
    parsed: &Parsed,
    offline: bool,
    env_tool: bool,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    match prelude::gate(&settings) {
        Some(Gate::VirtualDesktop) if !offline => return Gate::VirtualDesktop.stop(),
        Some(_) => return Gate::CannotConnect.stop(),
        None => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if env_tool {
        if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
            return stop;
        }
    }
    let ask = match parsed {
        Parsed::Help => {
            return Shown::out(crate::text::help("desktop").unwrap_or_default()).print()
        }
        Parsed::Early(line) => return Shown::err(format!("{line}\n"), Exit::Usage).print(),
        Parsed::Call(a) => a,
    };
    let req = match request(ask, &settings.project) {
        Ok(r) => r,
        Err(line) => return Shown::err(format!("{line}\n"), Exit::Usage).print(),
    };
    // `_desktop_secs`: a bad `--timeout` is the usage line before anything
    // is sent (`setup` checks it first; `env` after its words).
    let bad_timeout = match &req {
        Request::Setup => Some(SETUP_TIMEOUT_USAGE),
        Request::Env(_) => Some(ENV_TIMEOUT_USAGE),
        Request::One { .. } => None,
    };
    if let Some(line) = bad_timeout {
        if !ask.wait_for.is_empty() && timeout_secs(&ask.wait_for) == Timeout::Bad {
            return Shown::err(format!("{line}\n"), Exit::Usage).print();
        }
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    match req {
        Request::One {
            get,
            path,
            payload,
            verb,
        } => match send(&mut client, get, &path, &payload) {
            Ok((status, body)) => render(ask.json, verb, &status, &body).print(),
            Err(exit) => exit,
        },
        Request::Setup => match send(&mut client, false, "/cli/desktop/setup", "{}") {
            Ok((status, body)) => setup_wait(&mut client, ask, &status, &body),
            Err(exit) => exit,
        },
        Request::Env(body) => env_loop(&mut client, ask, &body),
    }
}

/// `_sidecar_send` and `_sidecar_split`: the status (`000` for none) and
/// the body. `Err`: a line the send printed, exit 1.
fn send(
    client: &mut Client,
    get: bool,
    path: &str,
    payload: &str,
) -> Result<(String, Vec<u8>), Exit> {
    let answer = if get {
        client.sidecar_get(path, payload)
    } else {
        client.sidecar_post(path, payload)
    };
    held(answer).map_err(|line| {
        out::stderr_line(&line);
        Exit::Error
    })
}

/// `_sidecar_exit_for`.
pub fn exit_for(status: &str) -> Exit {
    super::firewall::exit_for(status)
}

/// `_desktop_render <as_json> <verb>` on `_sidecar_send`'s status (`000`
/// for none) and body. `status_start` is `status` and `start`.
pub fn render(as_json: bool, verb: &str, status: &str, body: &[u8]) -> Rendered {
    if status == "000" {
        return Rendered {
            stdout: Vec::new(),
            stderr: format!("{UNREACHABLE}\n"),
            exit: Exit::Error,
        };
    }
    let with_newline = || {
        let mut b = body.to_vec();
        b.push(b'\n');
        b
    };
    if status != "200" {
        // `set -e`: a refusal its Python can't read ends the command there.
        let (stderr, ok) = refusal_lines(status, body);
        return Rendered {
            stdout: if as_json && ok {
                with_newline()
            } else {
                Vec::new()
            },
            stderr,
            exit: if ok { exit_for(status) } else { Exit::Error },
        };
    }
    if as_json {
        return Rendered {
            stdout: with_newline(),
            stderr: String::new(),
            exit: Exit::Ok,
        };
    }
    Rendered::of_shown(view_lines(verb, body))
}

/// The lines of the shell's Python, found in `cli/src/desktop.sh`.
pub mod line {
    use super::py_line;
    const SRC: &str = include_str!("../../../../cli/src/desktop.sh");
    const ERR: &str = "K2_DT_CODE=\"$code\"";
    const OK: &str = "K2_DT_VERB=\"$verb\"";
    const fn e(m: &str) -> u32 {
        py_line(SRC, ERR, m)
    }
    const fn o(m: &str) -> u32 {
        py_line(SRC, OK, m)
    }
    pub const E_ERROR: u32 = e("e = d.get(\"error\")");
    pub const E_CODE: u32 = e("c = e.get(\"code\") or \"\"");
    pub const E_CLOSABLE: u32 = e("print(\"desktops you could close: \"");
    pub const E_ASK: u32 = e("print(\"Ask with: \" + p[\"command\"]");
    pub const O_LOAD: u32 = o("d = json.loads(os.environ[\"K2_DT_BODY\"])");
    pub const O_SIZE: u32 = o("print(\"size           : %s\"");
    pub const O_MEMORY: u32 = o("print(\"memory held    : %d MB\"");
    pub const O_PRESENT: u32 = o("print(\"viewing now    : %s\"");
    pub const O_HOLD: u32 = o("print(\"agent hold     : %s until %s\"");
    pub const O_WAITING: u32 = o("print(\"waiting        : place");
    pub const O_LIMITS: u32 = o("print(\"limits         :");
    pub const O_LIMITS_1: u32 = o("lim[\"desktops\"][\"used\"], lim[\"desktops\"][\"max\"]");
    pub const O_LIMITS_2: u32 = o("lim[\"memoryBytes\"][\"max\"] // (1024 * 1024)");
    pub const O_NOTE: u32 = o("print(\"note           : %s");
    pub const O_MANUAL: u32 = o("print(\"to set up      :");
    pub const O_YOU: u32 = o("y = d.get(\"you\") or {}");
    pub const O_SAME: u32 = o("if y.get(\"sameMachine\"):");
    pub const O_YOU_LINE: u32 = o("print(\"you            : %s (%s)%s%s\"");
    pub const O_VIRTUAL_CALL: u32 = o("    virtual_lines(d)\n    if y.get");
    pub const O_HIDDEN: u32 = o("print(\"hidden apps    : %s\"");
    pub const O_GRANTS_LINE: u32 = o("print(\"grants         : %s\"");
    pub const O_VIEWERS_LINE: u32 = o("print(\"viewers        : %s\"");
    pub const O_ENABLED: u32 = o("    if d.get(\"enabled\"):");
    pub const O_ASKED: u32 = o("print(said.get(d.get(\"asked\"), \"Asked.\"))");
    pub const O_POLICY: u32 = o("print(\"hidden apps: %s\"");
    pub const O_GRANT: u32 = o("g = d.get(\"grant\") or {}");
    pub const O_GRANT_LINE: u32 = o("print(\"%s may watch (%s)");
    pub const O_REVOKE: u32 = o("print(\"%s; closed %s viewer(s).\"");
    pub const O_GRANTS: u32 = o("g = d.get(\"grants\") or []");
    pub const O_GRANTS_FOR: u32 = o("    for x in g:");
    pub const O_GRANTS_ROW: u32 = o("print(\"%-20s %-8s by %s\"");
    pub const O_VIEWERS: u32 = o("v = d.get(\"viewers\") or []");
    pub const O_VIEWERS_FOR: u32 = o("    for x in v:");
    pub const O_VIEWERS_ROW: u32 = o("print(\"%-14s %-16s %-8s %-8s %s since %s\"");
    pub const O_KICK: u32 = o("print(\"closed %s viewer(s)\"");
    pub const O_AUDIT: u32 = o("lines = d.get(\"lines\") or []");
    pub const O_AUDIT_FOR: u32 = o("    for l in lines:");
    pub const O_AUDIT_EXTRA: u32 = o("extra = \" \".join(");
    pub const O_AUDIT_ROW: u32 = o("print(\"%s  %-20s %-18s %-8s %s %s\"");
    pub const O_OPEN: u32 = o("print(\"asked your ");
    pub const O_LIST: u32 = o("ds = d.get(\"desktops\") or []");
    pub const O_LIST_FOR: u32 = o("    for x in ds:\n        print(\"%-46s");
    pub const O_LIST_ROW: u32 = o("print(\"%-46s %-10s %-20s %s\"");
    pub const O_STOP: u32 = o("print(\"%s %s\" % (\"Closed.\"");
    pub const O_SETUP: u32 = o("print(d.get(\"message\") or (\"Installed.");
    pub const O_DISPLAYS: u32 = o("ds = d.get(\"displays\") or []");
    pub const O_DISPLAYS_FOR: u32 = o("    for x in ds:\n        print(\"%-8s");
    pub const O_DISPLAYS_ROW: u32 = o("print(\"%-8s %-20s %sx%s pt%s\"");
    pub const O_ALLOW: u32 = o("if \"agentsCanUseDesktop\" in d:");
    pub const O_ALLOW_ON: u32 = o("on, who = d[\"agentsCanUseDesktop\"]");

    const SETUP: &str = "K2_DT_BODY=\"$body\" python3";
    const ENV: &str = "K2_DT_BODY=\"$out\" python3";
    pub const S_LOAD: u32 = py_line(SRC, SETUP, "d = json.loads(os.environ[\"K2_DT_BODY\"])");
    pub const S_STATE: u32 = py_line(SRC, SETUP, "if d.get(\"state\") == \"not_set_up\":");
    pub const V_LOAD: u32 = py_line(SRC, ENV, "d = json.loads(os.environ[\"K2_DT_BODY\"])");
    pub const V_RELEASED: u32 = py_line(SRC, ENV, "if \"released\" in d:");
    pub const V_ENV: u32 = py_line(SRC, ENV, "env = d.get(\"env\") or {}");
    pub const V_IN: u32 = py_line(SRC, ENV, "    if k in env:");
    pub const V_EXPORT: u32 = py_line(SRC, ENV, "print(\"export %s=%s\"");
}

/// The JSON decoder's traceback at `line`.
fn not_json(raw: &str, line: u32) -> Raise {
    match loads(raw) {
        Err(f) => Raise::at(line, f.last_line()),
        Ok(_) => Raise::at(line, "json.decoder.JSONDecodeError: Expecting value"),
    }
}

/// `_desktop_render`'s Python for a status that is not 200: the error
/// line, the desktops that could be closed, and the permission line.
/// `false`: its Python raised (the lines end with the traceback).
pub fn refusal_lines(status: &str, body: &[u8]) -> (String, bool) {
    let raw = String::from_utf8_lossy(body);
    let Ok(d) = loads(&raw) else {
        let head: String = raw.trim_matches(py_space).chars().take(300).collect();
        return (format!("{ERROR}HTTP {status}: {head}\n"), true);
    };
    let mut e = String::new();
    if let Err(r) = refusal_python(status, &d, &mut e) {
        e.push_str(&r.text());
        return (e, false);
    }
    (e, true)
}

fn refusal_python(status: &str, d: &Value, e: &mut String) -> Result<(), Raise> {
    use line::*;
    let error = at(d, "error", E_ERROR)?;
    let mut c = s("");
    match &error {
        Value::Object(_) => {
            c = or(dict_get(&error, "code"), s(""));
            let hint = or(dict_get(&error, "hint"), s(""));
            let _ = E_CODE;
            push(
                e,
                &format!("{ERROR}{}: {}", py::str_of(&c), py::str_of(&hint)),
            );
        }
        Value::Null => push(e, &format!("{ERROR}HTTP {status}")),
        other => {
            c = s(&py::str_of(other));
            push(e, &format!("{ERROR}{}", py::str_of(other)));
        }
    }
    if let Some(closable) = dict_get(d, "closable").filter(|v| py::truthy(v)) {
        push(
            e,
            &format!(
                "desktops you could close: {}",
                join(closable, ", ", E_CLOSABLE)?
            ),
        );
    }
    let p = match dict_get(d, "permission") {
        Some(o @ Value::Object(_)) => Some(crate::transport::permission::named(o)),
        _ => None,
    };
    let p = p.as_deref();
    let command = p.and_then(|o| dict_get(o, "command").filter(|v| py::truthy(v)));
    let unrequestable = p.is_some_and(|o| dict_get(o, "requestable") == Some(&Value::Bool(false)));
    if let Some(command) = command {
        push(e, &format!("Ask with: {}", plus_str(command, E_ASK)?));
    } else if c == s("desktop_people_only") || unrequestable {
        push(e, "It can't be requested.");
        if p.is_some_and(|o| dict_get(o, "feedback") == Some(&Value::Bool(true))) {
            push(e, crate::transport::permission::NO_CARD_FEEDBACK);
        }
    }
    Ok(())
}

/// What a Python run printed: stdout so far, and how it ended.
struct Py {
    o: String,
}

impl Py {
    fn print(&mut self, text: &str) {
        push(&mut self.o, text);
    }
}

/// `x[key]` at `line`, as a value.
fn idx(x: &Value, key: &str, line: u32) -> Result<Value, Raise> {
    index(x, key, line).cloned()
}

/// `x.get(key, default)`.
fn get_or(x: &Value, key: &str, default: Value, line: u32) -> Result<Value, Raise> {
    match x {
        Value::Object(_) => Ok(dict_get(x, key).cloned().unwrap_or(default)),
        other => Err(Raise::no_attribute(line, other, "get")),
    }
}

/// `"%s" % v`.
fn st(v: &Value) -> String {
    py::str_of(v)
}

/// A raise in a function `name` called from `from`.
fn inside(from: u32, name: &'static str) -> impl Fn(Raise) -> Raise {
    move |mut r: Raise| {
        r.frames.insert(0, (from, "<module>"));
        if let Some(f) = r.frames.get_mut(1) {
            f.1 = name;
        }
        r
    }
}

/// A raise inside a generator expression on `line` of the frame that
/// made it, which the caller then names.
fn in_genexpr(line: u32) -> impl Fn(Raise) -> Raise {
    move |r: Raise| Raise {
        frames: vec![(line, "<module>"), (r.frames[0].0, "<genexpr>")],
        exception: r.exception,
    }
}

/// `v // (1024 * 1024)` at `line`.
fn floordiv_mb(v: &Value, line: u32) -> Result<Value, Raise> {
    const MB: i128 = 1024 * 1024;
    match v {
        Value::Bool(_) => Ok(Value::Int("0".into())),
        Value::Int(digits) => match digits.parse::<i128>() {
            Ok(n) => Ok(Value::Int(n.div_euclid(MB).to_string())),
            Err(_) => Ok(Value::Float(
                (digits.parse::<f64>().unwrap_or(0.0) / MB as f64).floor(),
            )),
        },
        Value::Float(f) => Ok(Value::Float((f / MB as f64).floor())),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for //: '{}' and 'int'",
                py::type_name(other)
            ),
        )),
    }
}

/// The seconds `datetime.datetime.fromtimestamp` reads, or `None` where
/// it raises (the shell's helpers catch that). `ms`: divide by 1000 first.
fn timestamp(v: &Value, ms: bool) -> Option<f64> {
    let x = match v {
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Int(digits) => digits.parse::<f64>().ok()?,
        Value::Float(f) => *f,
        _ => return None,
    };
    let x = if ms { x / 1000.0 } else { x };
    x.is_finite().then_some(x)
}

/// `datetime.datetime.fromtimestamp(x).strftime(fmt)`, `None` where
/// Python raises.
fn local_time(x: f64, fmt: &str) -> Option<String> {
    // Microseconds, rounded half to even, as fromtimestamp rounds.
    let us = (x * 1e6).round_ties_even();
    let secs = (us / 1e6).floor();
    if !(-62_135_596_800.0..=253_402_300_799.0 + 86_400.0).contains(&secs) {
        return None;
    }
    let secs = secs as i64;
    let year: i64 = crate::out::pylib::time::strftime_local(secs, "%Y")
        .ok()?
        .parse()
        .ok()?;
    if !(1..=9999).contains(&year) {
        return None;
    }
    crate::out::pylib::time::strftime_local(secs, fmt).ok()
}

/// The Python's `when(ms)`.
fn when(v: &Value) -> String {
    timestamp(v, true)
        .and_then(|x| local_time(x, "%Y-%m-%d %H:%M"))
        .unwrap_or_else(|| "-".into())
}

/// The Python's `at(s)`.
fn at_time(v: &Value) -> String {
    timestamp(v, false)
        .and_then(|x| local_time(x, "%Y-%m-%d %H:%M"))
        .unwrap_or_else(|| "-".into())
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(py::truthy)
}

fn s_or(v: Option<&Value>, default: &str) -> String {
    st(&or(v, s(default)))
}

/// `_desktop_render`'s Python on a 200's body, by verb.
pub fn view_lines(verb: &str, body: &[u8]) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let d = match loads(&raw) {
        Ok(v) => v,
        Err(_) => return raised(String::new(), String::new(), &not_json(&raw, line::O_LOAD)),
    };
    let mut p = Py { o: String::new() };
    match view_python(verb, &d, &mut p) {
        Ok(()) => Shown::out(p.o),
        Err(r) => raised(p.o, String::new(), &r),
    }
}

fn mac_permission_lines(d: &Value, p: &mut Py) {
    let Some(perm @ Value::Object(_)) = dict_get(d, "macPermission") else {
        return;
    };
    if dict_get(perm, "step") == Some(&s("ready")) {
        return;
    }
    let get = |k: &str| st(dict_get(perm, k).unwrap_or(&Value::Null));
    p.print(&format!(
        "permission     : Screen Recording {}, Accessibility {} (to type and click)",
        get("screen"),
        get("accessibility")
    ));
    let tail = if truthy(dict_get(perm, "checking")) {
        concat!(" ", product_name!(), " notices by itself when they do.").to_string()
    } else {
        format!(" ({CLI} desktop ask --check)")
    };
    p.print(&format!(
        "               : {}{tail}",
        s_or(dict_get(perm, "line"), "")
    ));
}

fn virtual_lines(d: &Value, p: &mut Py) -> Result<(), Raise> {
    use line::*;
    let Some(v @ Value::Object(_)) = dict_get(d, "virtual") else {
        return Ok(());
    };
    let sz = dict_get(v, "size").cloned().unwrap_or(Value::Null);
    let size = if py::truthy(&sz) {
        let w = idx(&sz, "width", O_SIZE)?;
        let h = idx(&sz, "height", O_SIZE)?;
        format!("{}x{}", st(&w), st(&h))
    } else {
        "-".into()
    };
    p.print(&format!("size           : {size}"));
    if let Some(m) = dict_get(v, "memoryBytes").filter(|m| **m != Value::Null) {
        let mb = floordiv_mb(m, O_MEMORY)?;
        p.print(&format!("memory held    : {} MB", py_d(&mb, O_MEMORY)?));
    }
    let pr = or(dict_get(v, "present"), Value::Array(Vec::new()));
    let mut users = Vec::new();
    for x in py_iter(&pr, O_PRESENT)? {
        users.push(idx(&x, "user", O_PRESENT).map_err(in_genexpr(O_PRESENT))?);
    }
    let joined = join(&Value::Array(users), ", ", O_PRESENT)?;
    p.print(&format!(
        "viewing now    : {}",
        if joined.is_empty() { "nobody" } else { &joined }
    ));
    let h = dict_get(v, "hold").cloned().unwrap_or(Value::Null);
    if py::truthy(&h) {
        let by = at(&h, "by", O_HOLD)?;
        let until = match &h {
            Value::Object(_) => at_time(dict_get(&h, "until").unwrap_or(&Value::Int("0".into()))),
            _ => "-".into(),
        };
        p.print(&format!("agent hold     : {} until {until}", st(&by)));
    }
    if let Some(n) = dict_get(v, "nextSleepAt").filter(|x| py::truthy(x)) {
        p.print(&format!("sleeps at      : {}", at_time(n)));
    }
    if let Some(n) = dict_get(v, "nextCloseAt").filter(|x| py::truthy(x)) {
        p.print(&format!("closes at      : {} (a day asleep)", at_time(n)));
    }
    if !truthy(dict_get(v, "canSleep")) {
        p.print("sleep          : this server can't pause a desktop, so it closes when nobody is viewing it");
    }
    let w = dict_get(v, "waiting").cloned().unwrap_or(Value::Null);
    if py::truthy(&w) {
        let pos = at(&w, "position", O_WAITING)?;
        let lim = at(&w, "awakeLimit", O_WAITING)?;
        p.print(&format!(
            "waiting        : place {}; this server runs {} awake at a time",
            st(&pos),
            st(&lim)
        ));
    }
    let lim = or(dict_get(v, "limits"), Value::Object(Vec::new()));
    if py::truthy(&lim) {
        let two = |a: &str, b: &str, l: u32| -> Result<Value, Raise> {
            let x = idx(&lim, a, l)?;
            idx(&x, b, l)
        };
        let du = two("desktops", "used", O_LIMITS_1)?;
        let dm = two("desktops", "max", O_LIMITS_1)?;
        let au = two("awake", "used", O_LIMITS_1)?;
        let am = two("awake", "max", O_LIMITS_1)?;
        let mm = floordiv_mb(&two("memoryBytes", "max", O_LIMITS_2)?, O_LIMITS_2)?;
        let me = floordiv_mb(&two("memoryBytes", "each", O_LIMITS_2)?, O_LIMITS_2)?;
        let cores = idx(&lim, "cpuCores", O_LIMITS_2)?;
        p.print(&format!(
            "limits         : {}/{} desktops, {}/{} awake, {} MB for all ({} MB each), {} cores",
            st(&du),
            st(&dm),
            st(&au),
            st(&am),
            py_d(&mm, O_LIMITS)?,
            py_d(&me, O_LIMITS)?,
            st(&cores)
        ));
    }
    if let Some(l) = dict_get(v, "label").filter(|x| py::truthy(x)) {
        let _ = O_NOTE;
        p.print(&format!(
            "note           : {} (until agent isolation, any agent on this server could reach it)",
            st(l)
        ));
    }
    if let Some(m) = dict_get(v, "manual").filter(|x| py::truthy(x)) {
        let _ = O_MANUAL;
        p.print(&format!(
            "to set up      : {CLI} desktop setup, or by hand: {}",
            st(m)
        ));
    }
    if truthy(dict_get(v, "suggestRecycle")) {
        p.print(&format!(
            "tip            : it has been up for a week; {CLI} desktop stop and start frees what it leaked"
        ));
    }
    Ok(())
}

/// `", ".join("<fmt>" % (x[k] ...) for x in v)` for the status's grants
/// and viewers, at `l`.
fn pairs_line(v: &Value, keys: &[&str], l: u32) -> Result<String, Raise> {
    let mut parts = Vec::new();
    for x in py_iter(v, l)? {
        let mut vals = Vec::new();
        for k in keys {
            vals.push(st(&idx(&x, k, l).map_err(in_genexpr(l))?));
        }
        parts.push(if keys.len() == 2 {
            format!("{} ({})", vals[0], vals[1])
        } else {
            format!("{} {} ({})", vals[0], vals[1], vals[2])
        });
    }
    Ok(parts.join(", "))
}

fn none_if_empty(text: String, none: &str) -> String {
    if text.is_empty() {
        none.to_string()
    } else {
        text
    }
}

fn view_python(verb: &str, d: &Value, p: &mut Py) -> Result<(), Raise> {
    use line::*;
    match verb {
        "status_start" | "status" | "start" => {
            let y = or(
                crate::families::py_get(d, "you", O_YOU)?,
                Value::Object(Vec::new()),
            );
            p.print(&format!(
                "remote desktop : {} on {}",
                if truthy(dict_get(d, "enabled")) {
                    "on"
                } else {
                    "off"
                },
                s_or(dict_get(d, "machine"), "this computer")
            ));
            p.print(&format!(
                "desktop        : {} ({}{})",
                st(dict_get(d, "desktop").unwrap_or(&Value::Null)),
                s_or(dict_get(d, "name"), "This server"),
                if dict_get(d, "kind") == Some(&s("agent")) {
                    ", an agent"
                } else {
                    ""
                }
            ));
            p.print(&format!(
                "state          : {} ({})",
                st(dict_get(d, "state").unwrap_or(&Value::Null)),
                s_or(dict_get(d, "message"), "")
            ));
            p.print(&format!(
                "backend        : {}, lane {}",
                st(dict_get(d, "backend").unwrap_or(&Value::Null)),
                st(dict_get(d, "lane").unwrap_or(&Value::Null))
            ));
            mac_permission_lines(d, p);
            virtual_lines(d, p).map_err(inside(O_VIRTUAL_CALL, "virtual_lines"))?;
            let yg = |k: &str, l: u32| at(&y, k, l);
            let same = py::truthy(&yg("sameMachine", O_SAME)?);
            let you = if same {
                "you are on this computer: you can manage it, but you watch it from another device"
            } else if py::truthy(&yg("mayControl", O_SAME + 2)?) {
                "you may watch and drive"
            } else if py::truthy(&yg("mayView", O_SAME + 4)?) {
                "you may watch (view only)"
            } else {
                "you have no access to this screen"
            };
            let role = yg("role", O_YOU_LINE)?;
            let manage = py::truthy(&yg("mayManage", O_YOU_LINE)?)
                && !py::truthy(&yg("sameMachine", O_YOU_LINE)?);
            let start = py::truthy(&yg("mayStart", O_YOU_LINE)?)
                && dict_get(d, "virtual").is_some_and(|v| *v != Value::Null);
            p.print(&format!(
                "you            : {you} ({}){}{}",
                st(&role),
                if manage { "; you manage it" } else { "" },
                if start {
                    "; you may start and close it"
                } else {
                    ""
                }
            ));
            if let Some(h) = dict_get(d, "hideApps") {
                p.print(&format!(
                    "hidden apps    : {}",
                    none_if_empty(join(h, ", ", O_HIDDEN)?, "none")
                ));
            }
            if let Some(g) = dict_get(d, "grants") {
                p.print(&format!(
                    "grants         : {}",
                    none_if_empty(pairs_line(g, &["user", "mode"], O_GRANTS_LINE)?, "none")
                ));
            }
            if let Some(v) = dict_get(d, "viewers") {
                p.print(&format!(
                    "viewers        : {}",
                    none_if_empty(
                        pairs_line(v, &["user", "mode", "ingress"], O_VIEWERS_LINE)?,
                        "none"
                    )
                ));
            }
        }
        "enable" | "disable" => {
            crate::families::py_get(d, "enabled", O_ENABLED)?;
            let already = if truthy(dict_get(d, "changed")) {
                ""
            } else {
                " (it already was)"
            };
            if truthy(dict_get(d, "enabled")) {
                p.print(&format!(
                    "Remote desktop is on{already}. State: {} ({})",
                    st(dict_get(d, "state").unwrap_or(&Value::Null)),
                    s_or(dict_get(d, "message"), "")
                ));
                mac_permission_lines(d, p);
                if let Some(card) = dict_get(d, "card").filter(|c| py::truthy(c)) {
                    p.print("");
                    p.print(&st(card));
                }
            } else {
                p.print(&format!(
                    "Remote desktop is off{already}; closed {} viewer(s).",
                    st(&get_or(d, "closed", Value::Int("0".into()), O_ENABLED)?)
                ));
            }
        }
        "ask" => {
            let asked = crate::families::py_get(d, "asked", O_ASKED)?
                .cloned()
                .unwrap_or(Value::Null);
            let said = match &asked {
                Value::Array(_) | Value::Object(_) => {
                    return Err(Raise::at(
                        O_ASKED,
                        format!("TypeError: unhashable type: '{}'", py::type_name(&asked)),
                    ))
                }
                Value::Str(t) => match t.as_str() {
                    Some("prompt") => {
                        concat!(
                            "Asked ",
                            product_name!(),
                            " Screen to show macOS's prompt at the Mac."
                        )
                    }
                    Some("open_screen") => "Opened System Settings at Screen Recording on the Mac.",
                    Some("open_accessibility") => {
                        "Opened System Settings at Accessibility on the Mac."
                    }
                    Some("check") => "Checking again.",
                    _ => "Asked.",
                },
                _ => "Asked.",
            };
            p.print(said);
            mac_permission_lines(d, p);
        }
        "policy" => {
            let h = or(
                crate::families::py_get(d, "hideApps", O_POLICY)?,
                Value::Array(Vec::new()),
            );
            p.print(&format!(
                "hidden apps: {}",
                none_if_empty(join(&h, ", ", O_POLICY)?, "none")
            ));
        }
        "grant" => {
            let g = or(
                crate::families::py_get(d, "grant", O_GRANT)?,
                Value::Object(Vec::new()),
            );
            let user = at(&g, "user", O_GRANT_LINE)?;
            let mode = at(&g, "mode", O_GRANT_LINE)?;
            p.print(&format!(
                "{} may watch ({}) until you revoke it: {CLI} desktop revoke {}",
                st(&user),
                st(&mode),
                st(&user)
            ));
        }
        "revoke" => {
            crate::families::py_get(d, "revoked", O_REVOKE)?;
            p.print(&format!(
                "{}; closed {} viewer(s).",
                if truthy(dict_get(d, "revoked")) {
                    "revoked"
                } else {
                    "there was no grant"
                },
                st(&get_or(d, "closed", Value::Int("0".into()), O_REVOKE)?)
            ));
        }
        "grants" => {
            let g = or(
                crate::families::py_get(d, "grants", O_GRANTS)?,
                Value::Array(Vec::new()),
            );
            if !py::truthy(&g) {
                p.print(&format!(
                    "no grants (Owners and Admins open it without one; {CLI} desktop grant <login> lets a Member watch)"
                ));
            }
            for x in py_iter(&g, O_GRANTS_FOR)? {
                let user = idx(&x, "user", O_GRANTS_ROW)?;
                let mode = idx(&x, "mode", O_GRANTS_ROW)?;
                let by = at(&x, "grantedBy", O_GRANTS_ROW)?;
                p.print(&format!(
                    "{} {} by {}",
                    py_left(&user, 20),
                    py_left(&mode, 8),
                    st(&by)
                ));
            }
        }
        "viewers" => {
            let v = or(
                crate::families::py_get(d, "viewers", O_VIEWERS)?,
                Value::Array(Vec::new()),
            );
            if !py::truthy(&v) {
                p.print("nobody is watching");
            }
            for x in py_iter(&v, O_VIEWERS_FOR)? {
                let l = O_VIEWERS_ROW;
                let id = idx(&x, "id", l)?;
                let user = idx(&x, "user", l)?;
                let mode = idx(&x, "mode", l)?;
                let ingress = idx(&x, "ingress", l)?;
                let ip = or(crate::families::py_get(&x, "ip", l)?, s("-"));
                let since = get_or(&x, "since", Value::Int("0".into()), l)?;
                p.print(&format!(
                    "{} {} {} {} {} since {}",
                    py_left(&id, 14),
                    py_left(&user, 16),
                    py_left(&mode, 8),
                    py_left(&ingress, 8),
                    st(&ip),
                    when(&since)
                ));
            }
        }
        "kick" => {
            p.print(&format!(
                "closed {} viewer(s)",
                st(&get_or(d, "kicked", Value::Int("0".into()), O_KICK)?)
            ));
        }
        "audit" => {
            let lines = or(
                crate::families::py_get(d, "lines", O_AUDIT)?,
                Value::Array(Vec::new()),
            );
            if !py::truthy(&lines) {
                p.print("no access log lines");
            }
            for l in py_iter(&lines, O_AUDIT_FOR)? {
                let mut extra = Vec::new();
                for k in ["target", "mode", "display", "viewer", "ip"] {
                    let v = crate::families::py_get(&l, k, O_AUDIT_EXTRA)
                        .map_err(in_genexpr(O_AUDIT_EXTRA))?;
                    if truthy(v) {
                        extra.push(format!("{k}={}", st(v.expect("truthy"))));
                    }
                }
                let r = O_AUDIT_ROW;
                let ts = get_or(&l, "ts", s("-"), r)?;
                let event = at(&l, "event", r)?;
                let who = get_or(&l, "who", s("-"), r)?;
                let ingress = get_or(&l, "ingress", s("-"), r)?;
                let outcome = get_or(&l, "outcome", s(""), r)?;
                p.print(&format!(
                    "{}  {} {} {} {} {}",
                    st(&ts),
                    py_left(&event, 20),
                    py_left(&who, 18),
                    py_left(&ingress, 8),
                    st(&outcome),
                    extra.join(" ")
                ));
            }
        }
        "open" => {
            let f = crate::families::py_get(d, "for", O_OPEN)?
                .cloned()
                .unwrap_or(Value::Null);
            p.print(&format!(
                concat!(
                    "asked your ",
                    product_name!(),
                    " client to open the Desktop tab (for {})"
                ),
                st(&f)
            ));
        }
        "list" => {
            let ds = or(
                crate::families::py_get(d, "desktops", O_LIST)?,
                Value::Array(Vec::new()),
            );
            if !py::truthy(&ds) {
                p.print(&format!(
                    "no desktops you may open (the owner grants one: {CLI} desktop grant <login> --desktop <id>)"
                ));
            }
            for x in py_iter(&ds, O_LIST_FOR)? {
                let r = O_LIST_ROW;
                let id = idx(&x, "id", r)?;
                let state = get_or(&x, "state", s("-"), r)?;
                let name = get_or(&x, "name", s("-"), r)?;
                let label = or(crate::families::py_get(&x, "label", r)?, s(""));
                p.print(&format!(
                    "{} {} {} {}",
                    py_left(&id, 46),
                    py_left(&state, 10),
                    py_left(&name, 20),
                    st(&label)
                ));
            }
        }
        "stop" => {
            let closed = crate::families::py_get(d, "closed", O_STOP)?;
            p.print(&format!(
                "{} {}",
                if truthy(closed) {
                    "Closed."
                } else {
                    "It wasn't running."
                },
                s_or(dict_get(d, "message"), "")
            ));
        }
        "setup" => {
            let message = crate::families::py_get(d, "message", O_SETUP)?;
            let text = if truthy(message) {
                st(message.expect("truthy"))
            } else if truthy(dict_get(d, "ok")) {
                INSTALLED.to_string()
            } else {
                let missing = or(dict_get(d, "missing"), Value::Array(Vec::new()));
                format!(
                    "Installed, but still missing: {}",
                    join(&missing, ", ", O_SETUP)?
                )
            };
            p.print(&text);
        }
        "displays" => {
            let ds = or(
                crate::families::py_get(d, "displays", O_DISPLAYS)?,
                Value::Array(Vec::new()),
            );
            if !py::truthy(&ds) {
                p.print(&format!("no displays yet ({CLI} desktop status says why)"));
            }
            for x in py_iter(&ds, O_DISPLAYS_FOR)? {
                let r = O_DISPLAYS_ROW;
                let id = idx(&x, "id", r)?;
                let name = idx(&x, "name", r)?;
                let w = idx(&x, "widthPt", r)?;
                let h = idx(&x, "heightPt", r)?;
                let main = at(&x, "main", r)?;
                p.print(&format!(
                    "{} {} {}x{} pt{}",
                    py_left(&id, 8),
                    py_left(&name, 20),
                    st(&w),
                    st(&h),
                    if py::truthy(&main) { "  main" } else { "" }
                ));
            }
        }
        "allow" => {
            let (on, who) = if contains(d, "agentsCanUseDesktop", O_ALLOW)? {
                let on = idx(d, "agentsCanUseDesktop", O_ALLOW_ON)?;
                let who = at(d, "workspace", O_ALLOW_ON)?;
                (on, who)
            } else {
                let on = at(d, "agentsAllowed", O_ALLOW_ON + 2)?;
                let who = or(
                    dict_get(d, "name"),
                    dict_get(d, "workspaceId").cloned().unwrap_or(Value::Null),
                );
                (on, who)
            };
            let on = py::truthy(&on);
            p.print(&format!(
                "Allow a virtual desktop is {} for {}.",
                if on { "on" } else { "off" },
                st(&who)
            ));
            if on {
                p.print(&format!(
                    "That agent can get a desktop of its own with {CLI} desktop env (separate, not private)."
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

/// `print(json.load(sys.stdin).get(key) or "")` in `$(…)`, all on line 1:
/// the text with its trailing newlines gone, or the traceback (`Err`).
fn read_field(body: &[u8], key: &str) -> Result<String, String> {
    let raw = String::from_utf8_lossy(body);
    let d = loads(&raw).map_err(|f| Raise::at(1, f.last_line()).text())?;
    let v = crate::families::py_get(&d, key, 1).map_err(|r| r.text())?;
    let mut t = st(&or(v, s("")));
    while t.ends_with('\n') {
        t.pop();
    }
    Ok(t)
}

fn sleep_2() {
    std::thread::sleep(Duration::from_secs(2));
}

/// `_desktop_setup_wait`: the install runs on the server; while the
/// answer says `setting_up`, ask the status every 2 s up to `--timeout`.
fn setup_wait(client: &mut Client, a: &Ask, status: &str, body: &[u8]) -> Exit {
    // `set -e`: a render whose Python raises ends the command with 1.
    let render_0 = |status: &str, body: &[u8]| render(a.json, "setup", status, body).print();
    if status != "200" {
        return render_0(status, body);
    }
    // `2>/dev/null || true`: no traceback here.
    let state = read_field(body, "state").unwrap_or_default();
    if state != "setting_up" || a.no_wait {
        if state == "setting_up" && !a.json {
            return Shown::out(format!("{INSTALLING_NO_WAIT}\n")).print();
        }
        return render_0(status, body);
    }
    let secs = if a.wait_for.is_empty() {
        600
    } else {
        match timeout_secs(&a.wait_for) {
            Timeout::Secs(n) => n,
            other => unreachable!("run_with stops a bad one, run hands a long one on: {other:?}"),
        }
    };
    if !a.json {
        out::stderr_line(INSTALLING);
    }
    let mut waited: u64 = 0;
    let body = loop {
        sleep_2();
        waited += 2;
        let (code, body) = match send(client, true, "/cli/desktop/status", "desktop=virtual") {
            Ok(x) => x,
            Err(exit) => return exit,
        };
        if code == "000" {
            out::stderr_line(UNREACHABLE_WAITING);
            return Exit::Error;
        }
        if code != "200" {
            return render(a.json, "status", &code, &body).print();
        }
        // `set -e`: `state="$(… python3 …)"` that raises ends the command.
        let state = match read_field(&body, "state") {
            Ok(t) => t,
            Err(tb) => {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), tb.as_bytes());
                return Exit::Error;
            }
        };
        if state != "setting_up" {
            break (state, body);
        }
        if waited >= secs {
            out::stderr_line(&format!(
                "{CLI} desktop: still installing after {waited} s. It goes on on the server: {CLI} desktop status says setting_up until it ends."
            ));
            return Exit::Error;
        }
        if !a.json && waited.is_multiple_of(10) {
            out::stderr_line(&format!("  still installing\u{2026} ({waited} s)"));
        }
    };
    let (state, body) = body;
    if a.json {
        let mut text = body.clone();
        text.push(b'\n');
        if let Err(e) = out::write_flushed(&mut std::io::stdout().lock(), &text) {
            return out::after_stdout_error(&e);
        }
        return if state == "not_set_up" {
            Exit::Error
        } else {
            Exit::Ok
        };
    }
    setup_done(&body).print()
}

/// `_desktop_setup_wait`'s last Python: installed, or why not (exit 1).
pub fn setup_done(body: &[u8]) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let d = match loads(&raw) {
        Ok(v) => v,
        Err(_) => return raised(String::new(), String::new(), &not_json(&raw, line::S_LOAD)),
    };
    let state = match crate::families::py_get(&d, "state", line::S_STATE) {
        Ok(v) => v.cloned(),
        Err(r) => return raised(String::new(), String::new(), &r),
    };
    if state == Some(s("not_set_up")) {
        let m = or(dict_get(&d, "message"), s("the packages did not install"));
        return Shown::err(format!("{ERROR}setup_failed: {}\n", st(&m)), Exit::Error);
    }
    Shown::out(format!("{INSTALLED}\n"))
}

/// `_desktop_env`: POST `/cli/desktop/env` again every 2 s while the
/// desktop is waiting, starting or waking, up to `--timeout` (2 minutes).
fn env_loop(client: &mut Client, a: &Ask, body: &str) -> Exit {
    let secs = if a.wait_for.is_empty() {
        120
    } else {
        match timeout_secs(&a.wait_for) {
            Timeout::Secs(n) => n,
            other => unreachable!("run_with stops a bad one, run hands a long one on: {other:?}"),
        }
    };
    let mut waited: u64 = 0;
    let out_body = loop {
        let (code, answer) = match send(client, false, "/cli/desktop/env", body) {
            Ok(x) => x,
            Err(exit) => return exit,
        };
        if code != "200" {
            return render(a.json, "env", &code, &answer).print();
        }
        if a.release {
            break answer;
        }
        let state = match read_field(&answer, "state") {
            Ok(t) => t,
            Err(tb) => {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), tb.as_bytes());
                return Exit::Error;
            }
        };
        if !matches!(state.as_str(), "waiting" | "starting" | "waking") {
            break answer;
        }
        if waited >= secs {
            out::stderr_line(&format!(
                "{CLI} desktop: your desktop is still {state} after {waited} s; run {CLI} desktop env again later"
            ));
            if a.json {
                let mut text = answer.clone();
                text.push(b'\n');
                if let Err(e) = out::write_flushed(&mut std::io::stdout().lock(), &text) {
                    return out::after_stdout_error(&e);
                }
            }
            return Exit::Error;
        }
        if !a.json {
            let message = read_field(&answer, "message").unwrap_or_default();
            out::stderr_line(&format!("# {state}: {message}"));
        }
        sleep_2();
        waited += 2;
    };
    if a.json {
        let mut text = out_body;
        text.push(b'\n');
        return match out::write_flushed(&mut std::io::stdout().lock(), &text) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        };
    }
    env_lines(&out_body).print()
}

/// `shlex.quote` of a string.
pub fn shlex_quote(text: &str) -> String {
    if text.is_empty() {
        return "''".into();
    }
    let safe = text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if safe {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\"'\"'"))
    }
}

/// `_desktop_env`'s last Python: the shell lines for eval and the hold.
pub fn env_lines(body: &[u8]) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let d = match loads(&raw) {
        Ok(v) => v,
        Err(_) => return raised(String::new(), String::new(), &not_json(&raw, line::V_LOAD)),
    };
    let mut p = Py { o: String::new() };
    match env_python(&d, &mut p) {
        Ok(()) => Shown::out(p.o),
        Err(r) => raised(p.o, String::new(), &r),
    }
}

fn env_python(d: &Value, p: &mut Py) -> Result<(), Raise> {
    use line::*;
    if contains(d, "released", V_RELEASED)? {
        let desk = at(d, "desktop", V_RELEASED + 1)?;
        p.print(&format!(
            "# {}: {}",
            st(&desk),
            if truthy(dict_get(d, "released")) {
                "released: it can sleep when nobody is watching"
            } else {
                "you held nothing"
            }
        ));
        return Ok(());
    }
    let env = or(
        crate::families::py_get(d, "env", V_ENV)?,
        Value::Object(Vec::new()),
    );
    for k in ["DISPLAY", "XAUTHORITY", "K2_DESKTOP_PROFILE"] {
        if contains(&env, k, V_IN)? {
            let v = idx(&env, k, V_EXPORT)?;
            let quoted = match &v {
                Value::Str(_) => shlex_quote(&st(&v)),
                other if !py::truthy(other) => "''".into(),
                other => {
                    return Err(Raise::at(
                        V_EXPORT,
                        format!(
                            "TypeError: expected string or bytes-like object, got '{}'",
                            py::type_name(other)
                        ),
                    ))
                }
            };
            p.print(&format!("export {k}={quoted}"));
        }
    }
    let h = or(dict_get(d, "hold"), Value::Object(Vec::new()));
    let until = match &h {
        Value::Object(_) => timestamp(
            dict_get(&h, "until").unwrap_or(&Value::Int("0".into())),
            false,
        )
        .and_then(|x| local_time(x, "%H:%M")),
        _ => None,
    }
    .unwrap_or_else(|| "?".into());
    let state = dict_get(d, "state").cloned().unwrap_or(Value::Null);
    p.print(&format!(
        "# {} ({}): {} Yours until {until}: run {CLI} desktop env again to keep it, {CLI} desktop env --release when done. {}: people can watch and use it in a Desktop tab.",
        st(dict_get(d, "desktop").unwrap_or(&Value::Null)),
        s_or(dict_get(d, "name"), ""),
        st(&or(dict_get(d, "message"), state)),
        s_or(dict_get(d, "label"), "Separate, not private")
    ));
    Ok(())
}

#[cfg(test)]
mod tests;
