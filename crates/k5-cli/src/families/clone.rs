//! `k5 clone ...`, as the shell CLI's `cmd_clone` answers it
//! (`cli/src/clone.sh`):
//!
//! - the proposals: `proposals` (`proposal`), `propose`, `withdraw` and
//!   `deny`, each one `_compute_call` (the cell socket first in a
//!   session; `deny` over TCP) and its Python's lines;
//! - the engine ([`engine`], `_clone_py`): `preview`, `to`, `from`,
//!   `here`, `approve` and a bare `<ws> --dest`, a client of this server
//!   and a remote one (chunked transfers, the vault socket, passwords on
//!   the terminal);
//! - `clone` alone (or `help`) prints the help.
//!
//! Every verb parses as `_clone_parse` does, then, in its order: the
//! usage stops (`_clone_die`'s one JSON line on stderr, `json.dumps`,
//! Python's defaults), `--token-stdin` (one line from stdin) or `--token`
//! with its note, the plain-http warning for this server's own side, the
//! verb's count, the person guard (a passport, or any `HOOK_TOKEN`: exit
//! 3), the remote URL's rule ([`rule`]: exit 3, nothing looked up or
//! sent) and the remote token.
//!
//! Still the shell's ([`run`] returns `None`): any `-h` or `--help` (the
//! front's), a value flag as the last word (`shift 2` out of range), a
//! word that is not UTF-8, the shell's own settings in the environment,
//! a port curl won't take, a `python3` the binary can't tell; and for
//! the engine, `K2_HOST` as an `https://` URL or an IPv6 address, a
//! remote URL the binary doesn't send to itself ([`net::place`]), and a
//! remote `https://` server the person's proxy would carry (the shared
//! CONNECT path is the next step).

#[cfg(unix)]
pub mod engine;
#[cfg(unix)]
pub mod net;
pub mod rule;
#[cfg(unix)]
pub mod tty;

use super::compute::writes::{body_of, print_body, shell_settings, Cp};
use super::heartbeat::py::{dget_or, dget_v, index, iter, left, loads, str_plus, Py, R};
use super::{curl_port, s};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
#[cfg(unix)]
use engine::{EOpts, Engine, Stop, Which};
use k2_client::{cli_name, product_name};
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["clone"];
/// The binary answers the forms [`run`] takes; the rest go to the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

pub const LIST: &str = "/cli/clone-proposal/list";
pub const NEW: &str = "/cli/clone-proposal/new";
pub const WITHDRAW: &str = "/cli/clone-proposal/withdraw";
pub const DENY: &str = "/cli/clone-proposal/deny";

/// `_clone_parse`'s options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Opts {
    pub no_threads: bool,
    pub own: bool,
    pub live_only: bool,
    pub env_files: bool,
    pub yes: bool,
    pub json: bool,
    pub all: bool,
    pub copy: Vec<String>,
    pub blank: Vec<String>,
    pub skip: Vec<String>,
    pub since: Option<String>,
    pub dest: String,
    pub why: String,
    pub to: String,
    pub workspace: String,
    pub reason: String,
    /// `--host`.
    pub host: String,
    /// `--token`'s value (`_CLONE_TOKEN`), and whether `--token` came.
    pub token: String,
    pub token_flag: bool,
    pub token_stdin: bool,
    pub args: Vec<String>,
}

/// The verb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sub {
    Proposals,
    Propose,
    Withdraw,
    Deny,
    Preview,
    To,
    From,
    Here,
    Approve,
    /// A bare `<word> ...` (`here` with `--dest`): the first word.
    Bare(String),
}

impl Sub {
    fn engine(&self) -> bool {
        !matches!(
            self,
            Sub::Proposals | Sub::Propose | Sub::Withdraw | Sub::Deny
        )
    }
}

/// What a command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    Help,
    /// `_clone_die <code> <hint> <rc>`.
    Die(&'static str, String, Exit),
    Run(Sub, Box<Opts>),
}

/// `lst(k)`: the comma list's stripped, non-empty names.
fn names(v: &str) -> Vec<String> {
    v.split(',')
        .map(|n| n.trim_matches(super::py_space))
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect()
}

/// `_clone_parse`'s loop and its days check: `None` for a form the shell
/// must answer.
fn opts(words: &[&str]) -> Option<Result<Opts, Form>> {
    let mut o = Opts::default();
    let (mut copy, mut blank, mut skip) = (String::new(), String::new(), String::new());
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let next = words.get(i + 1).copied();
        let two = |slot: &mut String| -> Option<()> {
            *slot = next?.to_string();
            Some(())
        };
        let append = |acc: &mut String| -> Option<()> {
            let v = next?;
            if !acc.is_empty() {
                acc.push(',');
            }
            acc.push_str(v);
            Some(())
        };
        match w {
            "--no-threads" => o.no_threads = true,
            "--own" => o.own = true,
            "--env-files" => o.env_files = true,
            "--yes" | "-y" => o.yes = true,
            "--mine" => {}
            "--live-only" => o.live_only = true,
            "--json" => o.json = true,
            "--all" => o.all = true,
            "--token-stdin" => o.token_stdin = true,
            "--threads-since" | "--since-days" => {
                let mut v = String::new();
                two(&mut v)?;
                o.since = Some(v);
                i += 2;
                continue;
            }
            "--copy" | "--blank" | "--skip" => {
                append(match w {
                    "--copy" => &mut copy,
                    "--blank" => &mut blank,
                    _ => &mut skip,
                })?;
                i += 2;
                continue;
            }
            "--dest" | "--to" | "--why" | "--reason" | "--workspace" | "--host" | "--token" => {
                two(match w {
                    "--dest" => &mut o.dest,
                    "--to" => &mut o.to,
                    "--why" => &mut o.why,
                    "--reason" => &mut o.reason,
                    "--workspace" => &mut o.workspace,
                    "--host" => &mut o.host,
                    _ => &mut o.token,
                })?;
                if w == "--token" {
                    o.token_flag = true;
                }
                i += 2;
                continue;
            }
            "-h" | "--help" => return None,
            w if w.starts_with('-') => {
                return Some(Err(Form::Die(
                    "usage",
                    format!("unknown flag '{w}' ({} clone --help)", cli_name!()),
                    Exit::Usage,
                )))
            }
            w => o.args.push(w.to_string()),
        }
        i += 1;
    }
    if let Some(since) = &o.since {
        if since.is_empty() || !since.bytes().all(|b| b.is_ascii_digit()) {
            return Some(Err(Form::Die(
                "usage",
                "--threads-since takes a number of days".into(),
                Exit::Usage,
            )));
        }
    }
    o.copy = names(&copy);
    o.blank = names(&blank);
    o.skip = names(&skip);
    Some(Ok(o))
}

/// The words after `clone`; `None`: the shell's.
pub fn parse(words: &[&str]) -> Option<Form> {
    let first = words.first().copied().unwrap_or("");
    if matches!(first, "" | "-h" | "--help" | "help") {
        return Some(Form::Help);
    }
    let (sub, rest) = match first {
        "proposals" | "proposal" => (Sub::Proposals, &words[1..]),
        "propose" => (Sub::Propose, &words[1..]),
        "withdraw" => (Sub::Withdraw, &words[1..]),
        "deny" => (Sub::Deny, &words[1..]),
        "preview" => (Sub::Preview, &words[1..]),
        "to" => (Sub::To, &words[1..]),
        "from" => (Sub::From, &words[1..]),
        "here" => (Sub::Here, &words[1..]),
        "approve" => (Sub::Approve, &words[1..]),
        w => (Sub::Bare(w.to_string()), words),
    };
    Some(match opts(rest)? {
        Ok(o) => Form::Run(sub, Box::new(o)),
        Err(f) => f,
    })
}

/// `_clone_die`'s line: `json.dumps({"error": code, "hint": hint})`.
fn die_line(code: &str, hint: &str) -> String {
    let v = Value::Object(vec![
        (Text::from("error"), s(code)),
        (Text::from("hint"), s(hint)),
    ]);
    json::to_string(&v, &Options::default()).unwrap_or_default()
}

fn die(code: &str, hint: &str, exit: Exit) -> Exit {
    out::stderr_line(&die_line(code, hint));
    exit
}

/// `_clone_remote`'s URL for a host as typed: the URL less one trailing
/// `/`, or `https://<host>`.
pub fn remote_url(host: &str) -> String {
    if host.starts_with("http://") || host.starts_with("https://") {
        host.strip_suffix('/').unwrap_or(host).to_string()
    } else {
        format!("https://{host}")
    }
}

/// Does the binary send to `url` itself? A form [`net::place`] reads, and
/// for `https://` to another computer, no proxy the person set
/// (Python's `ProxyHandler` reads `https_proxy`, then `HTTPS_PROXY`).
fn sends_to(url: &str) -> bool {
    let Some(pl) = rule::place(url) else {
        return false;
    };
    if !pl.tls || k2_client::proxy_rule::direct(&format!("{url}/")) {
        return true;
    }
    !["https_proxy", "HTTPS_PROXY"]
        .iter()
        .any(|n| std::env::var_os(n).is_some_and(|v| !v.is_empty()))
}

/// The remote URL a verb reaches its engine with (before any stop that
/// can't be told without a request), for the hand-off check.
fn remote_of(sub: &Sub, o: &Opts) -> Option<String> {
    match sub {
        Sub::To if o.args.len() == 2 => Some(remote_url(&o.args[0])),
        Sub::From if o.args.len() == 1 => o.args[0].rsplit_once(':').map(|(h, _)| remote_url(h)),
        Sub::Preview if o.args.len() == 1 && !o.host.is_empty() => Some(remote_url(&o.host)),
        _ => None,
    }
}

/// `k5 clone ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let form = parse(&words)?;
    if shell_settings(std::env::vars_os()) || std::env::var_os("_K2_ARGS_JSON").is_some() {
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
    if let Form::Run(sub, o) = &form {
        if sub.engine() {
            if !cfg!(unix) {
                return None;
            }
            let h = settings.host.to_ascii_lowercase();
            if h.starts_with("https://") || settings.host.contains(':') {
                return None;
            }
            if let Some(url) = remote_of(sub, o) {
                if rule::url_refusal(&url, false).is_none() && !sends_to(&url) {
                    return None;
                }
            }
            let proxy = ["https_proxy", "HTTPS_PROXY"]
                .iter()
                .any(|n| std::env::var_os(n).is_some_and(|v| !v.is_empty()));
            if *sub == Sub::Approve && proxy {
                return None;
            }
        }
    }
    Some(run_with(args, form, settings))
}

/// `_clone_person_guard`: a passport (`_compute_in_session`) or any
/// `HOOK_TOKEN` refuses.
fn person_guard(settings: &Settings) -> Option<Exit> {
    let hook = k2_client::env_names::first_nonempty_os(k2_client::env_names::HOOK_TOKEN).is_some();
    if !hook && !super::compute::writes::in_session(&settings.auth) {
        return None;
    }
    Some(die(
        "clone_agents_propose",
        &format!(
            "agents can't clone; propose it and a person approves: {} clone propose --to <host|here> --why \"\u{2026}\"",
            cli_name!()
        ),
        Exit::Refused,
    ))
}

/// `_clone_local_plain_http_warn`: this server's side, when the
/// prelude's own line didn't come (it says it once for both).
fn local_plain_http_warn(settings: &Settings, args: &[OsString]) {
    let first = |names: &[&'static str]| {
        k2_client::env_names::first_nonempty_os(names)
            .map(|(_, v)| v.to_string_lossy().into_owned())
    };
    if first(k2_client::env_names::PLAIN_HTTP_OK).is_some_and(|v| v == "1") {
        return;
    }
    let host = first(k2_client::env_names::HOST)
        .unwrap_or_else(|| crate::daemon::DEFAULT_HOST.to_string());
    let rest: Vec<OsString> = args.iter().skip(1).cloned().collect();
    let prelude = crate::prelude::plain_http::step(&rest, &host, false, String::new);
    if matches!(prelude, crate::prelude::plain_http::Step::Warn(_)) {
        return;
    }
    if !rule::local_warns(&host) {
        return;
    }
    let _ = settings;
    out::stderr_line(&format!(
        "warning: {} is sending this server's owner token to {host} over plain http, unencrypted. That's fine on a private network such as a tailnet; elsewhere use https.",
        cli_name!()
    ));
}

fn run_with(args: &[OsString], form: Form, mut settings: Settings) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let (sub, o) = match form {
        Form::Help => {
            let text = crate::text::help("clone").unwrap_or_default();
            return super::Shown::out(text).print();
        }
        Form::Die(code, hint, exit) => return die(code, &hint, exit),
        Form::Run(sub, o) => (sub, *o),
    };
    // The remote token off argv: --token-stdin reads one line from stdin.
    let mut stdin_token = crate::prompt::Secret::default();
    if o.token_stdin {
        if o.token_flag {
            return die("usage", "--token-stdin or --token, not both", Exit::Usage);
        }
        let mut t = crate::prompt::read_secret_stdin("Token: ");
        let b = t.expose();
        let b = b.strip_suffix(b"\r").unwrap_or(b).to_vec();
        t = crate::prompt::Secret::new(b);
        if t.is_empty() {
            return die("usage", "--token-stdin: no token on stdin", Exit::Usage);
        }
        stdin_token = t;
    } else if o.token_flag {
        out::stderr_line("note: --token is visible to every account on this computer while the command runs; use --token-stdin");
    }
    let token_given = !stdin_token.is_empty() || !o.token.is_empty();
    local_plain_http_warn(&settings, args);
    let c = cli_name!();
    let usage = |hint: String| die("usage", &hint, Exit::Usage);
    match &sub {
        Sub::Preview if o.args.len() != 1 => {
            return usage(format!("usage: {c} clone preview <ws> [--host H]"))
        }
        Sub::To if o.args.len() != 2 => {
            return usage(format!("usage: {c} clone to <host> <ws> [--dest DIR]"))
        }
        Sub::From if o.args.len() != 1 => {
            return usage(format!("usage: {c} clone from <host>:<ws> [--dest DIR]"))
        }
        Sub::From if !o.args[0].contains(':') => {
            return usage(format!("usage: {c} clone from <host>:<ws>"))
        }
        Sub::Here if o.args.len() != 1 => {
            return usage(format!("usage: {c} clone here <ws> [--dest DIR]"))
        }
        Sub::Approve if o.args.len() != 1 => {
            return usage(format!("usage: {c} clone approve <id>"))
        }
        Sub::Approve if token_given => {
            return usage(format!("approve takes only the token saved for the proposal's host (signed in to in {} with Remember on, or in connect-tokens.json); drop --token / --token-stdin", product_name!()))
        }
        Sub::Bare(w) if o.dest.is_empty() => {
            return usage(format!(
                "unknown '{c} clone {w}' ({c} clone --help); a bare workspace needs --dest"
            ))
        }
        Sub::Bare(_) if o.args.len() != 1 => {
            return usage(format!("usage: {c} clone <ws> --dest DIR"))
        }
        Sub::Propose if !o.args.is_empty() => {
            return usage(format!(
                "usage: {c} clone propose --to <host|here> --why \"\u{2026}\""
            ))
        }
        Sub::Withdraw if o.args.len() != 1 => return usage(format!("usage: {c} clone withdraw <id>")),
        Sub::Deny if o.args.len() != 1 => {
            return usage(format!("usage: {c} clone deny <id> [--reason TEXT]"))
        }
        _ => {}
    }
    if sub.engine() || sub == Sub::Deny {
        if let Some(stop) = person_guard(&settings) {
            return stop;
        }
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    if sub.engine() {
        let explicit = if stdin_token.is_empty() {
            crate::prompt::Secret::new(o.token.clone().into_bytes())
        } else {
            stdin_token
        };
        return run_engine(&sub, &o, &settings, &client, explicit);
    }
    let mut cp = Cp {
        client: &mut client,
        closing: Closing::default(),
        project: settings.project.clone(),
        as_json: false,
    };
    let exit = answer(&mut cp, sub, o);
    let Cp { closing, .. } = cp;
    closing.flush(&mut client, exit);
    exit
}

#[cfg(unix)]
/// `_clone_remote host [saved]`: the URL, its rule, then the token (the
/// one typed, `*_REMOTE_TOKEN`, the K2 home's files, the keychain; with
/// `saved`, only the one saved for that host). `Err`: the stop.
fn clone_remote(
    host: &str,
    saved: bool,
    explicit: &crate::prompt::Secret,
    k2_home: &std::path::Path,
) -> Result<(String, crate::prompt::Secret), Exit> {
    let url = remote_url(host);
    if let Some(line) = rule::url_refusal(&url, false) {
        out::stderr_line(&line);
        return Err(Exit::Refused);
    }
    use crate::transport::remote;
    let tok = if saved {
        let homes = remote::homes(
            k2_home,
            std::env::var_os("HOME").map(std::path::PathBuf::from),
        );
        let t = match remote::lookup(host, &homes) {
            Some(remote::Found::Token(t)) => Some(t),
            Some(remote::Found::HostId(id)) => remote::keychain(&id),
            None => None,
        };
        match t.filter(|t| !t.is_empty()) {
            Some(t) => t,
            None => {
                return Err(die(
                    "no_saved_token",
                    &format!(
                        "no saved token for '{host}': sign in to it in {p} with Remember on (or add it to {}/connect-tokens.json), then approve again; a proposal never takes --token or $K2_REMOTE_TOKEN",
                        k2_home.display(),
                        p = product_name!()
                    ),
                    Exit::Refused,
                ))
            }
        }
    } else {
        let given = String::from_utf8_lossy(explicit.expose()).into_owned();
        remote::token_for_host(host, &given, k2_home).unwrap_or_default()
    };
    if tok.is_empty() {
        return Err(die(
            "no_remote_token",
            &format!(
                "no token for '{host}': sign in to it in {p} with Remember on, pass --token-stdin, set $K2_REMOTE_TOKEN, or add it to {}/connect-tokens.json",
                k2_home.display(),
                p = product_name!()
            ),
            Exit::Refused,
        ));
    }
    Ok((url, crate::prompt::Secret::new(tok.into_bytes())))
}

#[cfg(unix)]
/// The engine's options from the command line (`K2C_OPTS`).
fn eopts(o: &Opts) -> EOpts {
    let list = |v: &[String]| v.iter().map(|x| s(x)).collect();
    let since = o
        .since
        .as_deref()
        .filter(|v| !v.is_empty())
        .map(|v| {
            let n = v.trim_start_matches('0');
            Value::Int(if n.is_empty() { "0".into() } else { n.into() })
        })
        .unwrap_or(Value::Null);
    EOpts {
        no_threads: o.no_threads,
        own: o.own,
        live_only: o.live_only,
        env_files: o.env_files,
        yes: o.yes,
        json: o.json,
        copy: list(&o.copy),
        blank: list(&o.blank),
        skip: list(&o.skip),
        threads_since: since,
        dest: if o.dest.is_empty() {
            Value::Null
        } else {
            s(&o.dest)
        },
        proposal: Value::Null,
        proposal_summary: Value::Null,
    }
}

#[cfg(unix)]
/// `main()`'s end for a stop: the line, and the exit.
fn stopped(stop: Stop) -> Exit {
    match stop {
        Stop::Fail {
            code,
            hint,
            rc,
            plain,
        } => {
            if plain {
                out::stderr_line(&py::str_of(&hint));
            } else {
                let v = Value::Object(vec![
                    (Text::from("error"), code),
                    (Text::from("hint"), hint),
                ]);
                out::stderr_line(&json::to_string(&v, &Options::default()).unwrap_or_default());
            }
            Exit::from_curl(rc)
        }
        Stop::Cancelled => {
            out::stderr_line("{\"error\": \"cancelled\", \"hint\": \"stopped\"}");
            Exit::Interrupted
        }
        Stop::Raised(e) => {
            out::stderr_line(super::Raise::at(1, e).text().trim_end_matches('\n'));
            Exit::Error
        }
        Stop::Exit(n) => Exit::from_curl(n),
    }
}

#[cfg(not(unix))]
fn run_engine(
    _sub: &Sub,
    _o: &Opts,
    _settings: &Settings,
    _client: &crate::transport::client::Client,
    _explicit: crate::prompt::Secret,
) -> Exit {
    Exit::Error
}

#[cfg(unix)]
fn run_engine(
    sub: &Sub,
    o: &Opts,
    settings: &Settings,
    client: &crate::transport::client::Client,
    explicit: crate::prompt::Secret,
) -> Exit {
    let _int = tty::Caught::start(libc::SIGINT);
    let opener = crate::transport::urllib::Opener::from_client(client);
    let eo = eopts(o);
    let tty_on = crate::prompt::stderr_is_tty() && !eo.yes && tty::has_tty();
    let mut e = Engine {
        local_url: opener.base(),
        opener,
        remote_url: String::new(),
        remote: None,
        remote_token: crate::prompt::Secret::default(),
        vsock: engine::vault_sock_path(&settings.home),
        k2_home: settings.home.clone(),
        tty: tty_on,
        opts: eo,
        clone_id: engine::uuid4(),
        approved: Vec::new(),
        version: client.version.clone(),
    };
    let with_remote = |e: &mut Engine, host: &str, saved: bool| -> Result<(), Exit> {
        let (url, tok) = clone_remote(host, saved, &explicit, &settings.home)?;
        e.remote = rule::place(&url);
        if e.remote.is_none() {
            return Err(die(
                "usage",
                &format!("{url} isn't an address {} sends to itself", cli_name!()),
                Exit::Usage,
            ));
        }
        e.remote_url = url;
        e.remote_token = tok;
        Ok(())
    };
    let arg = |i: usize| o.args.get(i).cloned().unwrap_or_default();
    let ran = match sub {
        Sub::Preview => {
            if !o.host.is_empty() {
                if let Err(x) = with_remote(&mut e, &o.host.clone(), false) {
                    return x;
                }
            }
            main(&mut e, "preview", &arg(0))
        }
        Sub::To => {
            if let Err(x) = with_remote(&mut e, &arg(0), false) {
                return x;
            }
            main(&mut e, "push", &arg(1))
        }
        Sub::From => {
            let spec = arg(0);
            let (host, ws) = spec.rsplit_once(':').unwrap_or(("", ""));
            if let Err(x) = with_remote(&mut e, host, false) {
                return x;
            }
            main(&mut e, "pull", ws)
        }
        Sub::Here | Sub::Bare(_) => main(&mut e, "here", &arg(0)),
        Sub::Approve => {
            let id = arg(0);
            let target = match e.find_proposal(&id) {
                Ok(pr) => match index(&pr, "target") {
                    Ok(t) => py::str_of(&t).trim_end_matches('\n').to_string(),
                    Err(x) => return stopped(Stop::Raised(x)),
                },
                Err(stop) => return stopped(stop),
            };
            if target != "here" {
                if let Err(x) = with_remote(&mut e, &target, true) {
                    return x;
                }
            }
            main(&mut e, "approve", &id)
        }
        _ => Ok(()),
    };
    match ran {
        Ok(()) => Exit::Ok,
        Err(stop) => stopped(stop),
    }
}

#[cfg(unix)]
/// The engine's `main()` for one verb.
fn main(e: &mut Engine, verb: &str, arg: &str) -> engine::E<()> {
    if !e.remote_url.is_empty() {
        e.https_gate(Which::Remote, false)?;
    }
    let res = match verb {
        "preview" => {
            let w = if e.remote_url.is_empty() {
                Which::Local
            } else {
                Which::Remote
            };
            let out = e.preview(w, arg, &Value::Null)?;
            if e.opts.json {
                return engine::print_indented(&out);
            }
            return engine::print_preview(e, &out);
        }
        "push" => e.push(arg)?,
        "pull" => e.pull(arg)?,
        "here" => e.here(arg)?,
        _ => e.approve(arg)?,
    };
    engine::print_result(&res, e.opts.json)
}

fn answer(cp: &mut Cp, sub: Sub, o: Opts) -> Exit {
    match sub {
        Sub::Proposals => {
            let q = if o.all { "status=all" } else { "status=open" };
            let body = match cp.call(false, LIST, q, false) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if o.json {
                return print_body(&body);
            }
            proposals(&String::from_utf8_lossy(&body)).print()
        }
        Sub::Propose => {
            if o.to.is_empty() {
                out::stderr_line("usage: --to <host|here>");
                return Exit::Usage;
            }
            let list = |v: &[String]| Value::Array(v.iter().map(|x| s(x)).collect());
            let mut b = vec![
                (Text::from("to"), s(&o.to)),
                (Text::from("threads"), Value::Bool(!o.no_threads)),
                (Text::from("allHistory"), Value::Bool(!o.live_only)),
                (Text::from("copy"), list(&o.copy)),
                (Text::from("blank"), list(&o.blank)),
                (Text::from("skip"), list(&o.skip)),
            ];
            for (k, v) in [
                ("why", &o.why),
                ("dest", &o.dest),
                ("workspace", &o.workspace),
            ] {
                if !v.is_empty() {
                    b.push((Text::from(k), s(v)));
                }
            }
            if let Some(n) = &o.since {
                let n = n.trim_start_matches('0');
                if !n.is_empty() {
                    b.push((Text::from("sinceDays"), Value::Int(n.to_string())));
                }
            }
            let body = json::to_string(&Value::Object(b), &Options::default()).unwrap_or_default();
            let answer = match cp.call(true, NEW, &body, false) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if o.json {
                return print_body(&answer);
            }
            proposed(&String::from_utf8_lossy(&answer)).print()
        }
        Sub::Withdraw => {
            let body = body_of(&[("proposal", &o.args[0])]);
            match cp.call(true, WITHDRAW, &body, false) {
                Ok(b) => decided(&String::from_utf8_lossy(&b)).print(),
                Err(e) => e,
            }
        }
        Sub::Deny => {
            let reason = o.reason.trim_end_matches('\n').to_string();
            let body = body_of(&[("proposal", &o.args[0]), ("reason", &reason)]);
            match cp.call(true, DENY, &body, true) {
                Ok(b) => decided(&String::from_utf8_lossy(&b)).print(),
                Err(e) => e,
            }
        }
        _ => Exit::Ok,
    }
}

fn st(v: &Value) -> String {
    py::str_of(v)
}

/// The `proposals` reader.
pub fn proposals(raw: &str) -> super::Shown {
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let v = loads(raw)?;
        let rs = super::heartbeat::py::or(dget_v(&v, "proposals")?, Value::Array(Vec::new()));
        if !py::truthy(&rs) {
            p.out("No clone proposals.");
            return Ok(Exit::Ok);
        }
        for r in iter(&rs)? {
            let id = index(&r, "id")?;
            let status = index(&r, "status")?;
            let summary = index(&r, "summary")?;
            p.out(format!(
                "{}  {} {}",
                st(&id),
                left(&s(&st(&status)), 9)?,
                st(&summary)
            ));
            let by = index(&r, "proposedBy")?;
            let why = if py::truthy(&dget_v(&r, "why")?) {
                str_plus(": ", &index(&r, "why")?)?
            } else {
                String::new()
            };
            p.out(format!("    by {}{why}", st(&by)));
            if py::truthy(&dget_v(&r, "decidedBy")?) {
                p.out(format!(
                    "    {} by {}",
                    st(&index(&r, "status")?),
                    st(&index(&r, "decidedBy")?)
                ));
            }
        }
        Ok(Exit::Ok)
    })();
    p.raised(r, 3)
}

/// The `propose` reader.
pub fn proposed(raw: &str) -> super::Shown {
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let v = loads(raw)?;
        let prop = index(&v, "proposal")?;
        p.out(format!(
            "{} held: {}",
            st(&index(&prop, "id")?),
            st(&index(&prop, "summary")?)
        ));
        let hint = dget_or(&v, "hint", s(""))?;
        p.out(str_plus("  ", &hint)?);
        Ok(Exit::Ok)
    })();
    p.raised(r, 4)
}

/// The `withdraw` and `deny` reader.
pub fn decided(raw: &str) -> super::Shown {
    let mut p = Py::default();
    let r = (|| -> R<Exit> {
        let prop = index(&loads(raw)?, "proposal")?;
        p.out(format!(
            "{} {}",
            st(&index(&prop, "id")?),
            st(&index(&prop, "status")?)
        ));
        Ok(Exit::Ok)
    })();
    p.raised(r, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_of(f: Option<Form>) -> (Sub, Opts) {
        match f {
            Some(Form::Run(sub, o)) => (sub, *o),
            other => panic!("not a run: {other:?}"),
        }
    }

    #[test]
    fn the_forms() {
        assert_eq!(parse(&[]), Some(Form::Help));
        assert_eq!(parse(&["proposals", "--why"]), None);
        assert_eq!(parse(&["to", "--token"]), None);
        assert_eq!(parse(&["here", "w", "-h"]), None);
        let (sub, o) = run_of(parse(&["preview", "w", "--host", "h"]));
        assert_eq!((sub, o.host.as_str(), o.args.len()), (Sub::Preview, "h", 1));
        let (sub, o) = run_of(parse(&["w1", "--dest", "/d"]));
        assert_eq!(
            (sub, o.args),
            (Sub::Bare("w1".into()), vec!["w1".to_string()])
        );
        let (sub, o) = run_of(parse(&["--json"]));
        assert_eq!((sub, o.json), (Sub::Bare("--json".into()), true));
        let (_, o) = run_of(parse(&["to", "h", "w", "--token", "", "--token-stdin"]));
        assert!(o.token_flag && o.token_stdin && o.token.is_empty());
        assert!(matches!(
            parse(&["withdraw", "--bogus"]),
            Some(Form::Die("usage", _, Exit::Usage))
        ));
        let (_, o) = run_of(parse(&[
            "propose", "--to", "h", "--copy", "a, b", "--copy", ",c",
        ]));
        assert_eq!(o.copy, vec!["a", "b", "c"]);
        assert_eq!(
            die_line("usage", "x \u{2026}"),
            "{\"error\": \"usage\", \"hint\": \"x \\u2026\"}"
        );
    }

    #[test]
    fn remote_urls() {
        assert_eq!(remote_url("h.example/"), "https://h.example/");
        assert_eq!(remote_url("http://127.0.0.1:9/"), "http://127.0.0.1:9");
        let o = run_of(parse(&["from", "https://h.example:ws1"])).1;
        assert_eq!(
            remote_of(&Sub::From, &o).as_deref(),
            Some("https://h.example")
        );
        let pl = rule::place("http://[::1]:8080/k2").unwrap();
        assert_eq!(
            (pl.host.as_str(), pl.port, pl.prefix.as_str()),
            ("::1", 8080, "/k2")
        );
        assert!(rule::place("https://h.example:x").is_none());
        assert!(rule::place("https://h.example/?q").is_none());
    }
}
