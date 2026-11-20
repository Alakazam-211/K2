//! `k5 remote-session`: Remote Sessions (the Layer 0 switch, grants and a
//! grant's shell), as the shell CLI's `cmd_remote_session` answers it
//! (`cli/src/remote-session.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]) and the two gates
//!    ([`crate::prelude`]); `remote-session` is a locked tool, so with
//!    `HOOK_SOCK` set the stale-passport or require-passport lines, exit 3
//!    ([`crate::prelude::id_tool`]).
//! 2. No verb, or `help`: the help page on stdout, exit 0.
//! 3. The verb's words ([`parse`]). A flag it doesn't take is its line
//!    (`status` adds the help page on stderr), exit 2; a value flag as the
//!    last word ends the command with 1 and no line (`shift 2` under
//!    `set -e`, a bash bug, copied).
//! 4. `grant`'s `--ttl` (`30m`, `1h`, `90s`, bare seconds), then the
//!    target ([`apply_target`]): `--token <t>` or `--token-stdin` (one
//!    line; no echo on a terminal) is the Bearer for the calls in place of
//!    the owner token.
//! 5. One call over TCP, held in `$(…)` (nothing when it failed; no answer
//!    is the transport's line and exit 1):
//!    - `status [--json]`, `grants [--json]`: a GET;
//!    - `enable`, `disable`, `grant`, `revoke <id>`, `shell [--cwd D]`: a
//!      POST of JSON;
//!    - `write <sid> "text"`: `/cli/terminal/write` (a POST with the words
//!      in the query); `read <sid> [--lines N]`: `/cli/terminal/read`.
//!
//!    An answer that is an object with `"ok": true` goes to the verb's
//!    reader; any other to the shared error printer ([`print_err`]), which
//!    is exit 3 for a refusal, 2 for a usage error and 1 else.
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The readers print times with Python's `utcfromtimestamp`, which Python
//! 3.12 warns about once per line of the reader; that line is printed too.
//!
//! Still the shell's: a word that is not UTF-8, a command line with `-h`
//! or `--help` (the front sends those on; the router prints the help page
//! and acts on nothing), `HOOK_SOCK` set with a `CODEX_` variable
//! exported, a port that is set but is no port curl takes, a `python3` the
//! binary can't tell, `--host` (a remote server over Connect: its own TLS
//! client is not here yet), a grant token with a session passport, and a
//! `read --lines` that is not a plain word (the shell puts it in the URL
//! as it is).

use super::{get, py_iter, py_len, py_space, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::families::tunnel::Out;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::prompt;
use crate::python;
use crate::transport::{Auth, Call, Channel, Client, Error};
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["remote-session"];
/// The binary's, the whole row.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// None: the row is native whole.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy.
pub const TOOL: &str = "remote-session";

pub const STATUS: &str = "/cli/remote-session/status";
pub const GRANT: &str = "/cli/remote-session/grant";
pub const GRANTS: &str = "/cli/remote-session/grants";
pub const REVOKE: &str = "/cli/remote-session/revoke";
pub const SPAWN: &str = "/cli/remote-session/shell/spawn";
pub const WRITE: &str = "/cli/terminal/write";
pub const READ: &str = "/cli/terminal/read";

pub const BOTH: &str = concat!(
    cli_name!(),
    " remote-session: --token-stdin or --token, not both"
);
pub const NO_TOKEN: &str = concat!(
    cli_name!(),
    " remote-session: --token-stdin: no token on stdin"
);
pub const REVOKE_USAGE: &str = concat!("Usage: ", cli_name!(), " remote-session revoke <grant-id>");
pub const WRITE_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " remote-session write <session-id> \"text\" [--token-stdin | --token k2rs_\u{2026}] [--host HOST] (a token on the command line is visible to other users on this machine)"
);
pub const READ_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " remote-session read <session-id> [--lines N] [--token-stdin | --token k2rs_\u{2026}] [--host HOST] (a token on the command line is visible to other users on this machine)"
);

/// Python 3.12's warning for `utcfromtimestamp`, after `<string>:<line>: `.
pub const UTC_WARNING: &str = "DeprecationWarning: datetime.datetime.utcfromtimestamp() is deprecated and scheduled for removal in a future version. Use timezone-aware objects to represent datetimes in UTC: datetime.datetime.fromtimestamp(timestamp, datetime.UTC).";
/// The reader lines that call it.
pub const STATUS_UTC_LINE: u32 = 19;
pub const GRANT_UTC_LINE: u32 = 9;
pub const GRANTS_UTC_LINE: u32 = 9;
/// The `grants` reader's line that raises (`w_st = …`).
pub const GRANTS_WIDTH_LINE: u32 = 28;

/// `--host`, `--token`, `--token-stdin`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Target {
    pub host: String,
    pub token: String,
    pub stdin: bool,
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    Help,
    Status {
        json: bool,
    },
    Switch {
        on: bool,
    },
    /// `ttl` in seconds, as bash's arithmetic gives it.
    Grant {
        scope: String,
        ttl: i64,
        label: String,
    },
    Grants {
        json: bool,
    },
    Revoke {
        id: String,
    },
    Shell {
        cwd: String,
    },
    Write {
        sid: String,
        text: String,
    },
    Read {
        sid: String,
        lines: String,
    },
    /// Lines on stderr (the help page after them when `page`), the exit.
    Stop {
        lines: Vec<String>,
        page: bool,
        exit: Exit,
    },
}

fn stop(line: impl Into<String>) -> Form {
    Form::Stop {
        lines: vec![line.into()],
        page: false,
        exit: Exit::Usage,
    }
}

/// `shift 2` with one word left: the command ends with 1, no line.
fn silent() -> Form {
    Form::Stop {
        lines: Vec::new(),
        page: false,
        exit: Exit::Error,
    }
}

/// The words after `remote-session`, and the target.
pub fn parse(words: &[&str]) -> (Form, Target) {
    let mut t = Target::default();
    let sub = words.first().copied().unwrap_or("");
    if matches!(sub, "" | "--help" | "-h" | "help") {
        return (Form::Help, t);
    }
    let rest = words.get(1..).unwrap_or_default();
    // Flag -> which value it sets; the verb's own after the target's.
    let mut json = false;
    let (mut scope, mut ttl, mut label, mut cwd) = (
        "shell".to_string(),
        "30m".to_string(),
        String::new(),
        String::new(),
    );
    let mut lines = "50".to_string();
    let mut pos: Vec<String> = Vec::new();
    let positional = match sub {
        "revoke" | "read" => 1,
        "write" => 2,
        "status" | "enable" | "disable" | "grant" | "grants" | "shell" => 0,
        other => {
            return (
                Form::Stop {
                    lines: vec![format!("Unknown remote-session subcommand: {other}")],
                    page: true,
                    exit: Exit::Usage,
                },
                t,
            )
        }
    };
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let value = rest.get(i + 1).map(|v| v.to_string());
        let slot: Option<&mut String> = match (sub, w) {
            (_, "--host") => Some(&mut t.host),
            (_, "--token") => Some(&mut t.token),
            ("grant", "--scope") => Some(&mut scope),
            ("grant", "--ttl") => Some(&mut ttl),
            ("grant", "--label") => Some(&mut label),
            ("shell", "--cwd") => Some(&mut cwd),
            ("read", "--lines") => Some(&mut lines),
            _ => None,
        };
        if let Some(slot) = slot {
            let Some(v) = value else {
                return (silent(), t);
            };
            *slot = v;
            i += 2;
            continue;
        }
        match w {
            "--token-stdin" => t.stdin = true,
            "--json" if matches!(sub, "status" | "grants") => json = true,
            "--help" | "-h" => return (Form::Help, t),
            _ if positional == 0 => {
                let page = sub == "status";
                return (
                    Form::Stop {
                        lines: vec![format!("Unknown flag: {w}")],
                        page,
                        exit: Exit::Usage,
                    },
                    t,
                );
            }
            _ if w.starts_with('-') => return (stop(format!("Unknown flag: {w}")), t),
            _ if pos.len() < positional => pos.push(w.to_string()),
            _ => return (stop(format!("Unexpected argument: {w}")), t),
        }
        i += 1;
    }
    let first = pos.first().cloned().unwrap_or_default();
    let form = match sub {
        "status" => Form::Status { json },
        "enable" | "disable" => Form::Switch {
            on: sub == "enable",
        },
        "grant" => match parse_ttl(&ttl) {
            Some(secs) => Form::Grant {
                scope,
                ttl: secs,
                label,
            },
            None => stop(format!(
                "Error: invalid --ttl '{ttl}' (use 30m, 1h, 90s, or bare seconds)"
            )),
        },
        "grants" => Form::Grants { json },
        "revoke" if first.is_empty() => stop(REVOKE_USAGE),
        "revoke" => Form::Revoke { id: first },
        "shell" => Form::Shell { cwd },
        "write" => {
            let text = pos.get(1).cloned().unwrap_or_default();
            if first.is_empty() || text.is_empty() {
                stop(WRITE_USAGE)
            } else {
                Form::Write { sid: first, text }
            }
        }
        _ if first.is_empty() => stop(READ_USAGE),
        _ => Form::Read { sid: first, lines },
    };
    (form, t)
}

/// `_rs_parse_ttl`: seconds, or `None` for a word it refuses. Bash's
/// 64-bit arithmetic wraps.
pub fn parse_ttl(raw: &str) -> Option<i64> {
    if raw.is_empty() {
        return Some(1800);
    }
    if !raw
        .bytes()
        .all(|b| b.is_ascii_digit() || b"smhSMH".contains(&b))
    {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    let (n, mult) = match lower.as_bytes().last() {
        Some(b'h') => (&lower[..lower.len() - 1], 3600),
        Some(b'm') => (&lower[..lower.len() - 1], 60),
        Some(b's') => (&lower[..lower.len() - 1], 1),
        _ => (lower.as_str(), 1),
    };
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let v = n.bytes().fold(0i64, |acc, b| {
        acc.wrapping_mul(10).wrapping_add(i64::from(b - b'0'))
    });
    Some(v.wrapping_mul(mult))
}

/// `k5 remote-session ...`: `args` is the whole command line after
/// `argv[0]`. `None`: the shell CLI answers it (see the module's notes).
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
    let (form, target) = parse(&words);
    if !target.host.is_empty() {
        return None;
    }
    if let Form::Read { lines, .. } = &form {
        if !lines
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
        {
            return None;
        }
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let grant_token = !target.token.is_empty() || target.stdin;
    if grant_token && matches!(settings.auth, Auth::Passport { .. }) {
        return None;
    }
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    let port = settings.port_text.as_deref().and_then(super::curl_port)?;
    Some(run_with(
        args,
        form,
        target,
        settings,
        port,
        hook_sock.as_deref(),
    ))
}

fn run_with(
    args: &[OsString],
    form: Form,
    target: Target,
    settings: Settings,
    port: u16,
    hook_sock: Option<&Path>,
) -> Exit {
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    match &form {
        Form::Help => {
            let page = crate::text::help("remote-session").unwrap_or_default();
            return match out::stdout(page) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            };
        }
        Form::Stop { lines, page, exit } => {
            for l in lines {
                out::stderr_line(l);
            }
            if *page {
                let page = crate::text::help("remote-session").unwrap_or_default();
                let _ = out::write_flushed(&mut std::io::stderr().lock(), page.as_bytes());
            }
            return *exit;
        }
        _ => {}
    }
    let token = match apply_target(&target) {
        Ok(t) => t,
        Err(exit) => return exit,
    };
    let mut client = settings.client_on(port, would_refuse);
    if let Some(t) = token {
        client.auth = Auth::Owner(t);
        client.owner_from_disk = false;
    }
    let mut closing = Closing::default();
    let exit = match answer(&mut client, &mut closing, form) {
        Ok(e) | Err(e) => e,
    };
    closing.flush(&mut client, exit);
    exit
}

/// `_rs_apply_target` (no `--host`): the grant token to send in place of
/// the owner token, if one was given; its lines and exit 2 for both forms
/// or an empty token on stdin.
pub fn apply_target(t: &Target) -> Result<Option<String>, Exit> {
    let mut token = t.token.clone();
    if t.stdin {
        if !token.is_empty() {
            out::stderr_line(BOTH);
            return Err(Exit::Usage);
        }
        let secret = prompt::read_secret_stdin("Token: ");
        token = String::from_utf8_lossy(secret.expose()).into_owned();
        if token.is_empty() {
            out::stderr_line(NO_TOKEN);
            return Err(Exit::Usage);
        }
    }
    Ok((!token.is_empty()).then_some(token))
}

/// `raw=$(cli_… <endpoint> …) || raw=""` over TCP.
fn held_call(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    call: Call<'_>,
) -> Result<Vec<u8>, Exit> {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, endpoint, call) {
        Ok(r) => {
            closing.answer(&r, passport);
            Ok(crate::families::users::held(&r.body))
        }
        Err(
            e @ (Error::NoAnswer(_) | Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. }),
        ) => {
            out::stderr_line(&e.to_string());
            Err(Exit::Error)
        }
        Err(_) => Ok(Vec::new()),
    }
}

fn dumps(pairs: Vec<(&str, Value)>) -> String {
    crate::confirm::dumps(&crate::confirm::object(pairs))
}

fn s(t: &str) -> Value {
    Value::Str(Text::from(t))
}

/// A verb's reader of an answer that was `ok`.
type Reader = fn(&Value, &mut Out) -> Result<(), Raise>;

fn answer(client: &mut Client, closing: &mut Closing, form: Form) -> Result<Exit, Exit> {
    let (raw, reader): (Vec<u8>, Reader) = match &form {
        Form::Status { json } => {
            let raw = held_call(client, closing, STATUS, Call::Request(&[]))?;
            if *json && ok(&raw) {
                return Ok(pretty(&raw));
            }
            (raw, status_lines)
        }
        Form::Switch { on } => {
            let route = format!(
                "/cli/remote-session/{}",
                if *on { "enable" } else { "disable" }
            );
            (
                held_call(client, closing, &route, Call::PostJson(b"{}"))?,
                switch_lines,
            )
        }
        Form::Grant { scope, ttl, label } => {
            let mut b = vec![
                ("scope", s(if scope.is_empty() { "shell" } else { scope })),
                ("ttlSeconds", Value::Int(ttl.to_string())),
            ];
            let lab = label.trim_matches(py_space);
            if !lab.is_empty() {
                b.push(("label", s(lab)));
            }
            let body = dumps(b);
            (
                held_call(client, closing, GRANT, Call::PostJson(body.as_bytes()))?,
                grant_lines,
            )
        }
        Form::Grants { json } => {
            let raw = held_call(client, closing, GRANTS, Call::Request(&[]))?;
            if *json && ok(&raw) {
                return Ok(pretty(&raw));
            }
            (raw, grants_lines)
        }
        Form::Revoke { id } => {
            let body = dumps(vec![("id", s(id))]);
            (
                held_call(client, closing, REVOKE, Call::PostJson(body.as_bytes()))?,
                revoke_lines,
            )
        }
        Form::Shell { cwd } => {
            let body = if cwd.is_empty() {
                "{}".to_string()
            } else {
                dumps(vec![("cwd", s(cwd))])
            };
            (
                held_call(client, closing, SPAWN, Call::PostJson(body.as_bytes()))?,
                shell_lines,
            )
        }
        Form::Write { sid, text } => {
            let id = format!("id={}", crate::transport::encode::quote(sid));
            let msg = format!("message={}", crate::transport::encode::quote(text));
            let raw = held_call(client, closing, WRITE, Call::RequestPost(&[&id, &msg]))?;
            return Ok(write_reader(&raw, sid).print());
        }
        Form::Read { sid, lines } => {
            let id = format!("id={}", crate::transport::encode::quote(sid));
            let n = format!("lines={lines}");
            let raw = held_call(client, closing, READ, Call::Request(&[&id, &n]))?;
            return Ok(read_reader(&raw).print());
        }
        Form::Help | Form::Stop { .. } => return Ok(Exit::Error),
    };
    if !ok(&raw) {
        return Ok(print_err(&raw).print());
    }
    let d = json::parse(&text(&raw)).unwrap_or(Value::Null);
    let mut o = Out::default();
    let r = reader(&d, &mut o);
    Ok(o.done(r.map(|()| Exit::Ok)).print())
}

fn text(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

/// The `ok` check: `json.loads(raw or "{}")` is an object whose `ok` is
/// `true`.
pub fn ok(raw: &[u8]) -> bool {
    let t = text(raw);
    let t = if t.is_empty() { "{}".to_string() } else { t };
    match json::parse(&t) {
        Ok(Value::Object(p)) => get(&p, "ok") == Some(&Value::Bool(true)),
        _ => false,
    }
}

/// `python3 -m json.tool`: four spaces, ASCII only, and a newline.
fn pretty(raw: &[u8]) -> Exit {
    let opts = Options {
        indent: Some(4),
        ..Options::default()
    };
    let mut t = json::reprint(&text(raw), &opts).unwrap_or_default();
    t.push('\n');
    Shown::out(t).print()
}

/// `_rs_print_err`: the daemon's error, taught.
pub fn print_err(raw: &[u8]) -> Shown {
    let full = text(raw);
    let raw = full.trim_matches(py_space);
    if raw.is_empty() {
        return Shown::err(
            "Error: empty response from daemon (is it running?)\n",
            Exit::Error,
        );
    }
    let head: String = raw.chars().take(200).collect();
    let Ok(d) = json::parse(raw) else {
        return Shown::err(format!("Error: {head}\n"), Exit::Error);
    };
    let Value::Object(p) = &d else {
        return Shown::err(
            format!("Error: unexpected response: {}\n", py::repr(&s(&head))),
            Exit::Error,
        );
    };
    let mut err = get(p, "error").cloned().unwrap_or(Value::Null);
    if err == Value::Null && get(p, "ok") == Some(&Value::Bool(false)) {
        err = d.clone();
    }
    let mut o = Out::default();
    let r = match &err {
        Value::Object(ep) => dict_err(&mut o, ep, &err),
        Value::Str(_) => {
            let e = py::str_of(&err);
            o.err(&format!("Error: {e}"));
            let low = e.to_lowercase();
            if ["token", "forbidden", "owner"]
                .iter()
                .any(|w| low.contains(w))
            {
                o.err("  \u{2192} requires owner/admin token (or a live k2rs_ grant for shell/write/read)");
                Ok(Exit::Refused)
            } else {
                Ok(Exit::Error)
            }
        }
        _ => {
            o.err(&format!("Error: unexpected response: {head}"));
            Ok(Exit::Error)
        }
    };
    o.done(r)
}

/// `(x or "").strip()`, where `x` must be a string when it is true.
fn stripped(x: Option<&Value>, line: u32) -> Result<String, Raise> {
    match x {
        Some(v) if py::truthy(v) => match v {
            Value::Str(_) => Ok(py::str_of(v).trim_matches(py_space).to_string()),
            other => Err(Raise::no_attribute(line, other, "strip")),
        },
        _ => Ok(String::new()),
    }
}

/// What each code teaches.
pub fn teach(code: &str) -> Option<String> {
    let cli = cli_name!();
    let mint = format!("Mint a new one: {cli} remote-session grant [--ttl 30m]");
    Some(match code {
        "REMOTE_SESSIONS_DISABLED" => {
            format!("Remote Sessions are OFF. Enable with: {cli} remote-session enable")
        }
        "NO_GRANT" => format!("No live grant covers this principal. Mint one: {cli} remote-session grant [--ttl 30m] [--label \"...\"]"),
        "GRANT_EXPIRED" => format!("Grant expired. {mint}"),
        "GRANT_REVOKED" => format!("Grant was revoked. {mint}"),
        "owner_only" | "forbidden" | "gated" => "requires owner/admin \u{2014} ask your human".into(),
        _ => return None,
    })
}

fn dict_err(o: &mut Out, ep: &[(Text, Value)], err: &Value) -> Result<Exit, Raise> {
    let code = stripped(get(ep, "code"), 25)?;
    let hint_v = match get(ep, "hint") {
        Some(h) if py::truthy(h) => Some(h),
        _ => get(ep, "message"),
    };
    let hint = stripped(hint_v, 26)?;
    if code.is_empty() && hint.is_empty() {
        o.err(&format!("Error: {}", py::str_of(err)));
        return Ok(Exit::Error);
    }
    if let Some(t) = teach(&code) {
        let base = if hint.is_empty() {
            t.clone()
        } else {
            hint.clone()
        };
        o.err(&format!("Error: {base}"));
        if !hint.is_empty() && !hint.contains(&t) {
            o.err(&format!("  \u{2192} {t}"));
        }
        if code == "REMOTE_SESSIONS_DISABLED" && !hint.to_lowercase().contains("enable") {
            o.err(&format!("  \u{2192} {} remote-session enable", cli_name!()));
        }
        return Ok(Exit::Refused);
    }
    let msg = if !hint.is_empty() {
        hint
    } else if !code.is_empty() {
        code.clone()
    } else {
        py::str_of(err)
    };
    o.err(&format!("Error: {msg}"));
    Ok(if code == "usage" || code == "bad_request" {
        Exit::Usage
    } else {
        Exit::Error
    })
}

/// `datetime.datetime.utcfromtimestamp(int(ts)).strftime("%Y-%m-%d
/// %H:%M:%SZ")`, or `str(ts)` where it raises. `warned`: the line's
/// warning was printed (Python prints it once per line).
fn utc(o: &mut Out, ts: Option<&Value>, line: u32, warned: &mut bool) -> String {
    let null = Value::Null;
    let ts = ts.unwrap_or(&null);
    let Some(secs) = int_of(ts) else {
        return py::str_of(ts);
    };
    if !*warned {
        o.err(&format!("<string>:{line}: {UTC_WARNING}"));
        *warned = true;
    }
    secs.and_then(civil).unwrap_or_else(|| py::str_of(ts))
}

/// Python's `int(v)`: `None` where it raises; `Some(None)` for an integer
/// past what a timestamp holds.
fn int_of(v: &Value) -> Option<Option<i64>> {
    match v {
        Value::Bool(b) => Some(Some(i64::from(*b))),
        Value::Int(d) => Some(d.parse().ok()),
        Value::Float(f) if f.is_finite() => Some(if f.abs() < 9.2e18 {
            Some(f.trunc() as i64)
        } else {
            None
        }),
        Value::Str(_) => {
            let w = py::str_of(v);
            let w = w.trim_matches(py_space);
            let digits = w.strip_prefix(['+', '-']).unwrap_or(w);
            let ok = !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
                && !digits.starts_with('_')
                && !digits.ends_with('_')
                && !digits.contains("__");
            if !ok {
                return None;
            }
            Some(w.replace('_', "").parse().ok())
        }
        _ => None,
    }
}

/// Seconds since 1970 as `YYYY-MM-DD HH:MM:SSZ`, for years 1 to 9999.
pub fn civil(secs: i64) -> Option<String> {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    if !(1..=9999).contains(&y) {
        return None;
    }
    Some(format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    ))
}

fn at<'a>(d: &'a Value, key: &str, line: u32) -> Result<Option<&'a Value>, Raise> {
    super::py_get(d, key, line)
}

fn or(x: Option<&Value>, default: Value) -> Value {
    match x {
        Some(v) if py::truthy(v) => v.clone(),
        _ => default,
    }
}

/// `v[:n]` for a list or a string.
fn first(v: &Value, n: usize, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Array(items) => Ok(Value::Array(items.iter().take(n).cloned().collect())),
        Value::Str(t) => Ok(Value::Str(
            t.code_points()
                .into_iter()
                .take(n)
                .map(|cp| py::str_of(&Value::Str(super::code_point(cp))))
                .collect::<String>()
                .into(),
        )),
        Value::Object(_) => Err(Raise::at(line, "TypeError: unhashable type: 'slice'")),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// `status`'s lines.
pub fn status_lines(d: &Value, o: &mut Out) -> Result<(), Raise> {
    let on = at(d, "enabled", 4)?.is_some_and(py::truthy);
    o.out(&format!(
        "Remote Sessions: {}",
        if on { "ON" } else { "OFF" }
    ));
    let grants = at(d, "activeGrants", 5)?
        .cloned()
        .unwrap_or(Value::Int("0".into()));
    o.out(&format!("Active grants:   {}", py::str_of(&grants)));
    let sessions = or(at(d, "activeSessions", 6)?, Value::Array(Vec::new()));
    o.out(&format!("Active shells:   {}", py_len(&sessions, 7)?));
    for x in py_iter(&sessions, 8)? {
        if let Value::Object(p) = &x {
            let g = |k: &str| get(p, k).cloned().unwrap_or_else(|| s("?"));
            o.out(&format!(
                "  session={}  grant={}",
                py::str_of(&g("sessionId")),
                py::str_of(&g("grantId"))
            ));
        }
    }
    let denials = or(at(d, "recentDenials", 11)?, Value::Array(Vec::new()));
    if !py::truthy(&denials) {
        o.out("Recent denials:  none");
        return Ok(());
    }
    o.out(&format!("Recent denials ({}):", py_len(&denials, 13)?));
    let mut warned = false;
    for ev in py_iter(&first(&denials, 10, 14)?, 14)? {
        let Value::Object(p) = &ev else { continue };
        let ts = or(get(p, "createdAt"), Value::Int("0".into()));
        let tstr = utc(o, Some(&ts), STATUS_UTC_LINE, &mut warned);
        let code = or(get(p, "code"), s("-"));
        let who = or(get(p, "principalLabel"), s("?"));
        let payload = first(&or(get(p, "payload"), s("")), 80, 27)?;
        o.out(&format!(
            "  [{tstr}] {}  {}  {}",
            py::str_of(&code),
            py::str_of(&who),
            py::str_of(&payload)
        ));
    }
    Ok(())
}

/// `enable` and `disable`'s lines.
pub fn switch_lines(d: &Value, o: &mut Out) -> Result<(), Raise> {
    let on = at(d, "enabled", 4)?.is_some_and(py::truthy);
    o.out(&format!(
        "Remote Sessions: {}",
        if on { "ON" } else { "OFF" }
    ));
    if let Some(k) = at(d, "killedSessions", 5)?.filter(|k| **k != Value::Null) {
        o.out(&format!("Killed sessions: {}", py::str_of(k)));
    }
    Ok(())
}

/// `grant`'s lines: the token is shown once, here.
pub fn grant_lines(d: &Value, o: &mut Out) -> Result<(), Raise> {
    let g = or(at(d, "grant", 4)?, Value::Object(Vec::new()));
    let tok = or(at(d, "token", 5)?, s(""));
    let gid = or(at(&g, "id", 6)?, s("?"));
    let exp = or(
        at(&g, "expiresAt", 7)?,
        at(&g, "expires_at", 7)?.cloned().unwrap_or(Value::Null),
    );
    let mut warned = false;
    let exp_s = if py::truthy(&exp) {
        utc(o, Some(&exp), GRANT_UTC_LINE, &mut warned)
    } else {
        "?".to_string()
    };
    o.out(&format!("Grant id: {}", py::str_of(&gid)));
    o.out(&format!(
        "Token (save now, shown once): {}",
        py::str_of(&tok)
    ));
    o.out(&format!("Expires: {exp_s}"));
    if let Some(l) = at(&g, "label", 15)?.filter(|l| py::truthy(l)) {
        o.out(&format!("Label: {}", py::str_of(l)));
    }
    o.out(&format!(
        "Scope: {}",
        py::str_of(&or(at(&g, "scope", 17)?, s("shell")))
    ));
    Ok(())
}

/// `grants`' table: the reader raises before it prints one (see below).
pub fn grants_lines(d: &Value, o: &mut Out) -> Result<(), Raise> {
    let grants = or(at(d, "grants", 4)?, Value::Array(Vec::new()));
    if !py::truthy(&grants) {
        o.out("No grants.");
        return Ok(());
    }
    let mut warned = false;
    let mut rows = 0;
    for g in py_iter(&grants, 13)? {
        let Value::Object(p) = &g else { continue };
        // The row's other cells can't raise; its time may warn.
        utc(o, get(p, "expiresAt"), GRANTS_UTC_LINE, &mut warned);
        rows += 1;
    }
    if rows == 0 {
        o.out("No grants.");
        return Ok(());
    }
    // The width of STATE indexes the one-word tuple `("STATE",)`: the
    // shell's reader raises here whenever there is a grant (a bash bug,
    // `cli/src/remote-session.sh:353`, copied), so the table is never
    // printed.
    Err(Raise::inside(
        GRANTS_WIDTH_LINE,
        GRANTS_WIDTH_LINE,
        "<genexpr>",
        "IndexError: tuple index out of range",
    ))
}

/// `revoke`'s lines.
pub fn revoke_lines(d: &Value, o: &mut Out) -> Result<(), Raise> {
    let g = or(at(d, "grant", 4)?, Value::Object(Vec::new()));
    o.out(&format!(
        "Revoked grant {}.",
        py::str_of(&or(at(&g, "id", 5)?, s("?")))
    ));
    if let Some(k) = at(d, "killedSessions", 6)?.filter(|k| **k != Value::Null) {
        o.out(&format!("Killed sessions: {}", py::str_of(k)));
    }
    Ok(())
}

/// `shell`'s lines: the session and what to run next.
pub fn shell_lines(d: &Value, o: &mut Out) -> Result<(), Raise> {
    let sid = py::str_of(&or(at(d, "sessionId", 4)?, s("?")));
    o.out(&format!("sessionId: {sid}"));
    o.out(&format!(
        "grantId:   {}",
        py::str_of(&or(at(d, "grantId", 6)?, s("?")))
    ));
    let ready = at(d, "ready", 7)?.is_some_and(py::truthy);
    o.out(&format!(
        "ready:     {}",
        if ready { "true" } else { "false" }
    ));
    o.out("");
    o.out("Next:");
    let cli = cli_name!();
    o.out(&format!(
        "  {cli} remote-session write {sid} \"command\" --token-stdin"
    ));
    o.out(&format!(
        "  {cli} remote-session read  {sid} [--lines 50] --token-stdin"
    ));
    Ok(())
}

/// `write`'s two checks and their lines.
pub fn write_reader(raw: &[u8], sid: &str) -> Shown {
    let wrote = Shown::out(format!("wrote to {sid}\n"));
    if raw.is_empty() {
        return wrote;
    }
    let full = text(raw);
    let t = full.trim_matches(py_space);
    if t.is_empty() {
        return wrote;
    }
    let shown = Shown::out(format!("{t}\n"));
    let Ok(d) = json::parse(t) else {
        return shown;
    };
    let Value::Object(p) = &d else {
        return shown;
    };
    let ok = get(p, "ok") == Some(&Value::Bool(true));
    if get(p, "error").is_some_and(|e| *e != Value::Null) && !ok {
        return print_err(raw);
    }
    if ok || get(p, "success").is_some() {
        return wrote;
    }
    if get(p, "lines").is_some() {
        return Shown::out("");
    }
    shown
}

/// `read`'s lines, or the error printer.
pub fn read_reader(raw: &[u8]) -> Shown {
    let t = text(raw);
    let d = if t.trim_matches(py_space).is_empty() {
        Value::Object(Vec::new())
    } else {
        match json::parse(&t) {
            Ok(d) => d,
            Err(_) => return print_err(raw),
        }
    };
    let Value::Object(p) = &d else {
        return print_err(raw);
    };
    if get(p, "error").is_some_and(|e| *e != Value::Null) && get(p, "lines").is_none() {
        return print_err(raw);
    }
    let lines = or(get(p, "lines"), Value::Array(Vec::new()));
    let items = match py_iter(&lines, 13) {
        Ok(i) => i,
        // The check's Python raised (its stderr is dropped): the printer.
        Err(_) => return print_err(raw),
    };
    let mut out = String::new();
    for l in items {
        out.push_str(&py::str_of(&l));
        out.push('\n');
    }
    Shown::out(out)
}

#[cfg(test)]
mod tests;
