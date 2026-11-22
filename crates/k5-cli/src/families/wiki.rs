//! `k5 wiki …`: the workspace's knowledge base, as the shell CLI's
//! `cmd_wiki` answers it (`cli/src/wiki.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the two gates ([`crate::prelude`]); `wiki chat on`
//!    skips the connection gate (it never asks the daemon), not the
//!    virtual desktop's guard. `wiki` is an open tool: no session checks.
//!    In a session with the owner token on disk the shell sends the owner
//!    token over TCP (COMPAT-58); the binary sends the passport
//!    (`owner-token-with-passport`, PRD 13.9 #4).
//! 2. The verb and its words ([`parse`]): `_wiki_parse_args` (`--json`,
//!    `--port`, `--label`, `--workspace`/`--project`, `--`), or `note`'s
//!    own loop. A bad word is `_wiki_err usage …`, exit 2: `Error: <hint>`
//!    on stderr, or with `--json` (or `AS_JSON=true` in the environment)
//!    Python's JSON error object there. Nothing is sent before it.
//! 3. `_wiki_apply_project`: a workspace given as a folder is `cd <it> &&
//!    pwd`; any other word is what the daemon's `/cli/workspace/resolve`
//!    says (`cli_request … 2>/dev/null | python3 …`), else the word itself.
//! 4. The verb's requests, each `raw=$(cli_request …)` or `raw=$(cli_post_form
//!    …)` over TCP: a failed call ends the command with curl's code, one
//!    with no answer prints the transport's line and exits 1. `status`
//!    then reads the index with its errors hidden (`2>/dev/null || true`):
//!    a failed call is an empty index, one with no answer still ends the
//!    command (exit 1, its line unseen). `unpublish` reads the serve status
//!    the same way.
//! 5. The shell's Python on the answers ([`read`]), then the EXIT trap's
//!    lines ([`crate::prelude::Closing`]).
//!
//! `status`'s Python gets both answers in environment variables. An
//! answer bigger than the system lets one variable be (128 KiB on Linux,
//! about 1 MiB in all on macOS) stops bash there ("Argument list too
//! long", exit 126); the binary reads the answers itself and prints the
//! status. That is the named difference `env-answer-large`
//! (`scripts/cli-parity.sh`).
//!
//! `publish` ([`Run::publish`], [`publish`]): serve on and its port, the
//! claim check (`cli_send cli_post`), then the Connect steps through the
//! daemon (`_connect_curl_daemon`, [`Client::connect_proxy`]): create,
//! else point, each followed by the attribution stamp and the refresh
//! nudge. Their failures are quiet, as the shell hides them; a Connect
//! request that may have reached the daemon ends the command with 1.
//!
//! With no daemon to make a Connect call, the control plane itself with
//! `tunnel.json`'s token as its `Authorization` header (TLS for an
//! `https://` base, [`crate::transport::tls`]).
//!
//! `K2_CONNECT_BASE` with no scheme, or plain `http://` to a host that is
//! not this computer, is refused as publish starts, before any request:
//! one line, exit 2 (the shell does the same). Through the
//! person's proxy when one is set for an `https://` base
//! ([`crate::transport::proxy`]).
//!
//! Still the shell's ([`run`] returns `None`): `wiki publish` with a token
//! in `tunnel.json`, air-gap off, and a proxy setting the binary doesn't
//! read or a control-plane base it doesn't send to itself, a word that is
//! not UTF-8, a command line with `-h` or `--help` ([`crate::front`]),
//! a `--workspace` folder that `cd` reads as an option or that `CDPATH`
//! would move (bash's own edges), a port that is set but is no port
//! curl takes, and a `python3` the binary can't tell. An answer that is
//! not UTF-8 is read with U+FFFD for each bad byte, as
//! [`super::inbox`].

pub mod publish;
pub mod read;

use super::{curl_port, without_nuls};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{Auth, Call, Channel, Client, Error, Response};
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["wiki"];
/// The binary answers it; [`run`] hands `publish` and the edge cases back.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The routes.
pub const STATUS: &str = "/cli/wiki/serve/status";
pub const INDEX: &str = "/cli/wiki/index";
pub const NOTE: &str = "/cli/wiki/note";
pub const SEED: &str = "/cli/wiki/seed";
pub const SERVE: &str = "/cli/wiki/serve";
pub const CHAT: &str = "/cli/wiki/chat";
pub const RESOLVE: &str = "/cli/workspace/resolve";
/// `_subdomain_attrib_post claim`'s route (the ownership check and the
/// attribution stamp).
pub const CLAIM: &str = "/cli/publish/subdomain/claim";
/// `_nudge_subdomain_refresh`'s route.
pub const REFRESH: &str = "/cli/tunnel/subdomains/refresh";

/// `wiki chat on`'s refusal, exit 3.
pub const CHAT_UNAVAILABLE: &str = concat!(
    "Public wiki chat is coming back with the ",
    k2_client::product_name!(),
    " runtime. It is off on every workspace for now; hosting is being rebuilt on the runtime."
);

/// What `_wiki_parse_args` took out of the words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Opts {
    pub json: bool,
    /// `--workspace`, else the first word left.
    pub workspace: String,
    pub port: String,
    pub label: String,
}

/// The verbs that ask the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    Status,
    Index,
    Note(String),
    Seed,
    Serve(bool),
    ChatOff,
    Unpublish,
    Publish,
}

/// What a command line does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    /// `wiki`, `wiki help`, `wiki ""`: the help page, exit 0.
    Help,
    /// A help file on stdout (`serve` or `chat` with no mode: exit 2;
    /// with `help`: 0).
    Page(&'static str, Exit),
    /// An unknown verb: its line and the usage page on stderr, exit 1.
    Unknown(String),
    /// `_wiki_err usage <hint>`, exit 2. `json`: as JSON.
    Usage {
        json: bool,
        hint: String,
    },
    /// `wiki chat on`: the refusal, exit 3.
    ChatOn {
        json: bool,
    },
    Ask(Verb, Opts),
}

/// `${AS_JSON:-false} = true` (or `as_json` not yet set by the verb).
fn env_json(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| v == "true")
}

fn usage(json: bool, hint: impl Into<String>) -> Form {
    Form::Usage {
        json,
        hint: hint.into(),
    }
}

/// `_wiki_parse_args`: the options and the words left, or the usage form.
pub fn parse_args(words: &[&str]) -> Result<(Opts, Vec<String>), Form> {
    let mut o = Opts::default();
    let mut pos = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let next = words.get(i + 1).copied().unwrap_or("");
        match w {
            "--json" => o.json = true,
            "--port" | "--label" => {
                if next.is_empty() {
                    return Err(usage(o.json, format!("{w} requires a value")));
                }
                if w == "--port" {
                    o.port = next.into();
                } else {
                    o.label = next.into();
                }
                i += 2;
                continue;
            }
            "--workspace" | "--project" => {
                if next.is_empty() {
                    return Err(usage(o.json, format!("{w} requires a workspace path/name")));
                }
                o.workspace = next.into();
                i += 2;
                continue;
            }
            "--" => {
                pos.extend(words[i + 1..].iter().map(|s| s.to_string()));
                break;
            }
            _ => {
                if let Some(v) = w.strip_prefix("--port=") {
                    o.port = v.into();
                } else if let Some(v) = w.strip_prefix("--label=") {
                    o.label = v.into();
                } else if let Some(v) = w
                    .strip_prefix("--workspace=")
                    .or_else(|| w.strip_prefix("--project="))
                {
                    o.workspace = v.into();
                } else if w.starts_with('-') {
                    return Err(usage(o.json, format!("unknown flag '{w}'")));
                } else {
                    pos.push(w.to_string());
                }
            }
        }
        i += 1;
    }
    Ok((o, pos))
}

/// `_wiki_parse_args`, then the trailing workspace.
fn opts(words: &[&str]) -> Result<Opts, Form> {
    let (mut o, pos) = parse_args(words)?;
    if o.workspace.is_empty() {
        if let Some(p) = pos.first() {
            o.workspace = p.clone();
        }
    }
    Ok(o)
}

/// `cmd_wiki_note`'s own loop.
fn note_words(words: &[&str]) -> Form {
    let note_usage = format!("usage: {} wiki note <id> [--json] [project]", cli_name!());
    let mut o = Opts::default();
    let mut id = String::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--json" => o.json = true,
            "--workspace" | "--project" => {
                let v = words.get(i + 1).copied().unwrap_or("");
                if v.is_empty() {
                    return usage(o.json, format!("{w} requires a value"));
                }
                o.workspace = v.into();
                i += 2;
                continue;
            }
            _ => {
                if let Some(v) = w
                    .strip_prefix("--workspace=")
                    .or_else(|| w.strip_prefix("--project="))
                {
                    o.workspace = v.into();
                } else if w.starts_with('-') {
                    return usage(o.json, format!("unknown flag '{w}' \u{2014} {note_usage}"));
                } else if id.is_empty() {
                    id = w.into();
                } else if o.workspace.is_empty() {
                    o.workspace = w.into();
                } else {
                    return usage(o.json, format!("unexpected argument '{w}'"));
                }
            }
        }
        i += 1;
    }
    if id.is_empty() {
        return usage(o.json, note_usage);
    }
    Form::Ask(Verb::Note(id), o)
}

/// The command line's form (the words after `wiki`; no `-h` or `--help`
/// in them, the front's).
pub fn parse(words: &[&str]) -> Form {
    let sub = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or(&[]);
    let ask = |verb: Verb, words: &[&str]| match opts(words) {
        Ok(o) => Form::Ask(verb, o),
        Err(f) => f,
    };
    match sub {
        "status" => ask(Verb::Status, rest),
        "index" => ask(Verb::Index, rest),
        "note" => note_words(rest),
        "seed" => ask(Verb::Seed, rest),
        "publish" => ask(Verb::Publish, rest),
        "unpublish" => ask(Verb::Unpublish, rest),
        "serve" => {
            let mode = rest.first().copied().unwrap_or("");
            let after = rest.get(1..).unwrap_or(&[]);
            match mode {
                "on" | "start" | "true" | "1" => ask(Verb::Serve(true), after),
                "off" | "stop" | "false" | "0" => ask(Verb::Serve(false), after),
                "" => Form::Page("wiki/serve", Exit::Usage),
                "help" => Form::Page("wiki/serve", Exit::Ok),
                _ => usage(
                    env_json("as_json"),
                    format!(
                        "usage: {} wiki serve on|off [--port N] [project]",
                        cli_name!()
                    ),
                ),
            }
        }
        "chat" => {
            let mode = rest.first().copied().unwrap_or("");
            let after = rest.get(1..).unwrap_or(&[]);
            match mode {
                "on" | "enable" | "true" | "1" => match parse_args(after) {
                    Ok((o, _)) => Form::ChatOn { json: o.json },
                    Err(f) => f,
                },
                "off" | "disable" | "false" | "0" => ask(Verb::ChatOff, after),
                "" => Form::Page("wiki/chat", Exit::Usage),
                "help" => Form::Page("wiki/chat", Exit::Ok),
                _ => usage(
                    env_json("as_json"),
                    format!("usage: {} wiki chat off [project]", cli_name!()),
                ),
            }
        }
        "" | "help" => Form::Help,
        other => Form::Unknown(other.to_string()),
    }
}

/// `k5 wiki ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let form = parse(&words);
    if let Form::Ask(_, o) = &form {
        if handed_back(&o.workspace) {
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
    // The prelude skips the connection gate for `wiki chat on` (whatever
    // follows), not the virtual desktop's guard.
    let chat_on = words.first() == Some(&"chat")
        && matches!(words.get(1), Some(&("on" | "enable" | "true" | "1")));
    if chat_on {
        if settings.no_owner_fallback && settings.auth == Auth::None {
            return Some(Gate::VirtualDesktop.stop());
        }
        return Some(run_with(args, form, settings, Connect::default()));
    }
    if let Some(gate) = prelude::gate(&settings) {
        return Some(gate.stop());
    }
    // A port curl won't take is the shell's.
    settings.port_text.as_deref().and_then(curl_port)?;
    let mut connect = Connect::default();
    if matches!(form, Form::Ask(Verb::Publish, _)) {
        // The rules on the control plane's base, as publish starts and
        // before any request: no scheme, or plain http to another machine,
        // is refused.
        if let Some(line) = base_refusal(&connect_base()) {
            out::stderr_line(&line);
            return Some(Exit::Usage);
        }
        let (state, _) = crate::airgap::state(&crate::offline::Env::current())?;
        connect.airgap_off = state == crate::airgap::State::Off;
        // With no daemon to make the call, the shell calls the control
        // plane itself with `tunnel.json`'s token, through the person's
        // proxy when one is set for it. A base or a proxy setting the
        // binary doesn't read is the shell's.
        if connect.airgap_off {
            if let Some(token) = publish::tunnel_token(&settings.home.join("tunnel.json")) {
                let url = connect_base();
                if url.ends_with('/') {
                    return None;
                }
                connect.control = Some((crate::transport::remote::routed(&url)?, token));
            }
        }
    }
    Some(run_with(args, form, settings, connect))
}

/// What `wiki publish`'s Connect steps may use.
#[derive(Default)]
struct Connect {
    /// `_k2_airgap_state` is off: `_connect_curl` may phone Connect.
    airgap_off: bool,
    /// The control plane's base and `tunnel.json`'s token, for when no
    /// daemon makes the call. The token is only ever sent as the
    /// `Authorization` header.
    control: Option<(crate::transport::remote::Base, String)>,
}

/// `K2_CONNECT_BASE` (its K5_, K2_ and K2SO_ names, empty counts as
/// unset), else the hosted control plane.
fn connect_base() -> String {
    k2_client::env_names::CONNECT_BASE
        .iter()
        .filter_map(std::env::var_os)
        .find(|v| !v.is_empty())
        .map_or_else(
            || k2_client::hosts::CONNECT_BASE.to_string(),
            |v| v.to_string_lossy().into_owned(),
        )
}

/// The rules on `K2_CONNECT_BASE` (the shell checks the same, first):
/// a base with no scheme is refused (curl would guess `http://` and send
/// the token there), and so is plain `http://` to a host that is not a
/// loopback literal (a token goes to another machine over https only;
/// judged by the URL as written, never a lookup).
fn base_refusal(url: &str) -> Option<String> {
    let plain = url.starts_with("http://");
    if !plain && !url.starts_with("https://") {
        return Some(format!(
            "Error: K2_CONNECT_BASE must start with https:// (got: {url})"
        ));
    }
    let host = crate::transport::proxy::url_host(url);
    (plain && !crate::transport::remote::literal_loopback(host))
        .then(|| crate::transport::remote::plain_http_line(url))
}

/// A `--workspace` folder bash's `cd` reads otherwise: an option (a
/// leading `-`), or a relative name `CDPATH` would look for elsewhere.
fn handed_back(ws: &str) -> bool {
    let cdpath = std::env::var_os("CDPATH").is_some_and(|v| !v.is_empty());
    !ws.is_empty()
        && Path::new(ws).is_dir()
        && (ws.starts_with('-') || (cdpath && super::sim::cd_searches(ws)))
}

/// `_wiki_err CODE HINT`: its line on stderr.
fn wiki_err(json: bool, code: &str, hint: &str) {
    let text = if json || env_json("AS_JSON") {
        read::err_json(code, hint)
    } else {
        format!("Error: {hint}\n")
    };
    let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
}

/// A help file on stdout; `exit`.
fn page(key: &str, exit: Exit) -> Exit {
    let text = crate::text::help(key).unwrap_or_default();
    match out::stdout(text) {
        Ok(()) => exit,
        Err(e) => out::after_stdout_error(&e),
    }
}

fn run_with(args: &[OsString], form: Form, mut settings: Settings, connect: Connect) -> Exit {
    let (verb, o) = match form {
        Form::Help => return page("wiki", Exit::Ok),
        Form::Page(key, exit) => return page(key, exit),
        Form::Unknown(sub) => {
            out::stderr_line(&format!("Unknown '{} wiki' subcommand: {sub}", cli_name!()));
            let text = crate::text::help("wiki/_unknown").unwrap_or_default();
            let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
            return Exit::Error;
        }
        Form::Usage { json, hint } => {
            wiki_err(json, "usage", &hint);
            return Exit::Usage;
        }
        Form::ChatOn { json } => {
            wiki_err(json, "public_chat_unavailable", CHAT_UNAVAILABLE);
            return Exit::Refused;
        }
        Form::Ask(verb, o) => (verb, o),
    };
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut run = Run {
        client,
        closing: Closing::default(),
        connect,
    };
    run.apply_project(&o.workspace);
    let exit = run.verb(&verb, &o);
    let Run {
        mut client,
        closing,
        ..
    } = run;
    closing.flush(&mut client, exit);
    exit
}

/// A body as `$(…)` holds it: NULs and trailing newlines gone.
fn held(r: &Response) -> Vec<u8> {
    let mut b = without_nuls(&r.body);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    b
}

struct Run {
    client: Client,
    closing: Closing,
    connect: Connect,
}

impl Run {
    /// `_wiki_apply_project`.
    fn apply_project(&mut self, sel: &str) {
        if sel.is_empty() {
            return;
        }
        if Path::new(sel).is_dir() {
            self.client.project =
                super::sim::cd_pwd(&super::sim::logical_pwd(), sel).unwrap_or_default();
            return;
        }
        let q = format!("q={}", quote(sel));
        let passport = self.client.passport().is_some();
        let got = match self
            .client
            .call(Channel::Tcp, RESOLVE, Call::Request(&[q.as_str()]))
        {
            Ok(r) => {
                self.closing.answer(&r, passport);
                read::resolved(&without_nuls(&r.body))
            }
            // Its line went to /dev/null; `_k2_end_command` still ends the
            // command from inside the pipe.
            Err(Error::NoAnswer(_)) => super::end_in_pipe(),
            Err(_) => String::new(),
        };
        self.client.project = if got.is_empty() { sel.to_string() } else { got };
    }

    /// `raw=$(cli_request <endpoint> <args>)`. `quiet`: `2>/dev/null ||
    /// true` (a failed call is an empty answer; one with no answer still
    /// ends the command, its line unseen).
    fn get(&mut self, endpoint: &str, args: &[&str], quiet: bool) -> Result<Vec<u8>, Exit> {
        let sent = self
            .client
            .call(Channel::Tcp, endpoint, Call::Request(args));
        self.took(sent, quiet)
    }

    /// `raw=$(cli_post_form <endpoint> <pairs>)`.
    fn post(&mut self, endpoint: &str, pairs: &[&str]) -> Result<Vec<u8>, Exit> {
        let sent = self
            .client
            .call(Channel::Tcp, endpoint, Call::PostForm(pairs));
        self.took(sent, false)
    }

    fn took(&mut self, sent: Result<Response, Error>, quiet: bool) -> Result<Vec<u8>, Exit> {
        let passport = self.client.passport().is_some();
        match sent {
            Ok(r) => {
                self.closing.answer(&r, passport);
                Ok(held(&r))
            }
            Err(Error::NoAnswer(n)) => {
                if !quiet {
                    out::stderr_line(&n.to_string());
                }
                Err(Exit::Error)
            }
            Err(_) if quiet => Ok(Vec::new()),
            Err(e) => Err(Exit::from_curl(e.curl_code())),
        }
    }

    fn verb(&mut self, verb: &Verb, o: &Opts) -> Exit {
        match self.answer(verb, o) {
            Ok(exit) | Err(exit) => exit,
        }
    }

    fn answer(&mut self, verb: &Verb, o: &Opts) -> Result<Exit, Exit> {
        Ok(match verb {
            Verb::Status => {
                let raw = self.get(STATUS, &[], false)?;
                let idx = self.get(INDEX, &[], true)?;
                read::status(&raw, &idx, &self.client.project, o.json).print()
            }
            Verb::Index => {
                let mut raw = self.get(INDEX, &[], false)?;
                if o.json {
                    // `<<<"$raw"` adds a newline.
                    raw.push(b'\n');
                    read::as_json(&raw).print()
                } else {
                    read::index(&raw).print()
                }
            }
            Verb::Note(id) => {
                let pair = format!("id={}", quote(id));
                let raw = self.get(NOTE, &[pair.as_str()], false)?;
                if o.json {
                    read::as_json(&raw).print()
                } else {
                    read::note(&raw).print()
                }
            }
            Verb::Seed => read::seed(&self.post(SEED, &[])?).print(),
            Verb::Serve(on) => {
                let enabled = format!("enabled={on}");
                let port = format!("port={}", quote(&o.port));
                let mut pairs = vec![enabled.as_str()];
                if !o.port.is_empty() {
                    pairs.push(port.as_str());
                }
                read::serve(&self.post(SERVE, &pairs)?, *on).print()
            }
            Verb::ChatOff => read::chat_off(&self.post(CHAT, &["enabled=false"])?).print(),
            Verb::Unpublish => self.unpublish(o)?,
            Verb::Publish => self.publish(o)?,
        })
    }

    /// Text on stdout; a broken stdout ends the command.
    fn say(&self, text: &str) -> Result<(), Exit> {
        out::stdout(text).map_err(|e| out::after_stdout_error(&e))
    }

    /// `cmd_wiki_publish`: serve on, read the port, then the Connect
    /// steps when this workspace may take the label.
    fn publish(&mut self, o: &Opts) -> Result<Exit, Exit> {
        let label = if o.label.is_empty() {
            publish::default_label(&self.client.project)
        } else {
            o.label.clone()
        };
        if let Some(line) = publish::reserved_refusal(&label) {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), line.as_bytes());
            return Ok(Exit::Error);
        }
        let port = format!("port={}", quote(&o.port));
        let mut pairs = vec!["enabled=true"];
        if !o.port.is_empty() {
            pairs.push(port.as_str());
        }
        let sent = self
            .client
            .call(Channel::Tcp, SERVE, Call::PostForm(&pairs));
        let serve = self.took(sent, true)?;
        let status = self.get(STATUS, &[], true)?;
        let (port_num, url) = publish::port_and_url(&serve, &status);
        // `port_num=$(… python3 …)` under `set -e`: a reader that raises
        // ends the command there with 1, its traceback on stderr.
        let raised = |raise: super::Raise| {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), raise.text().as_bytes());
            Exit::Error
        };
        let port_num = match port_num {
            Ok(p) => p,
            Err(raise) => return Ok(raised(raise)),
        };
        let url = match url {
            Ok(u) => u,
            Err(raise) => return Ok(raised(raise)),
        };
        if port_num.is_empty() {
            wiki_err(
                o.json,
                "serve_failed",
                "could not start wiki serve or read port from daemon status",
            );
            let name = cli_name!();
            out::stderr_line(&format!(
                "  Check: {name} wiki serve on && {name} wiki status"
            ));
            return Ok(Exit::Error);
        }
        let target = format!("127.0.0.1:{port_num}");
        let site = if url.is_empty() {
            format!("http://127.0.0.1:{port_num}")
        } else {
            url.clone()
        };
        self.say(&format!(
            "Wiki local site: {site}\n  target for publish: {target}\n  label:              {label}\n\n"
        ))?;
        let mut made = false;
        if self.preflight(&label)? {
            let body = publish::create_body(&label, &target);
            if let Some(raw) = self.connect("POST", "/subdomains", &body)? {
                if publish::made(&raw) {
                    if let Some(lines) = publish::created(&raw) {
                        self.say(&lines)?;
                        made = true;
                        self.stamp(&label)?;
                        self.nudge()?;
                    }
                }
            }
            if !made {
                let body = publish::point_body(&target);
                let ep = format!("/subdomains/{}", quote(&label));
                if let Some(raw) = self.connect("PUT", &ep, &body)? {
                    if publish::made(&raw) {
                        if let Some(lines) = publish::pointed(&raw) {
                            self.say(&lines)?;
                            made = true;
                            self.stamp(&label)?;
                            self.nudge()?;
                        }
                    }
                }
            }
        }
        self.say(&publish::closing_lines(&label, &target, made))?;
        if o.json {
            self.say(&publish::json_line(&label, &target, &url, &port_num, made))?;
        }
        Ok(Exit::Ok)
    }

    /// `resp=$(_subdomain_attrib_post claim <label> "" <pairs>) ||
    /// resp=""`: `cli_send cli_post`. A send that may have run ends the
    /// command with its line; any other failure is an empty answer.
    fn attrib(&mut self, pairs: &[&str]) -> Result<Vec<u8>, Exit> {
        let passport = self.client.passport().is_some();
        match self.client.send(CLAIM, Call::QueryPost(pairs)) {
            Ok(r) => {
                self.closing.answer(&r, passport);
                Ok(held(&r))
            }
            Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
                out::stderr_line(&e.to_string());
                Err(Exit::Error)
            }
            Err(_) => Ok(Vec::new()),
        }
    }

    /// `_subdomain_label_preflight <label> 0`: may this workspace take it?
    fn preflight(&mut self, label: &str) -> Result<bool, Exit> {
        let pair = format!("label={}", quote(label));
        let resp = self.attrib(&[pair.as_str(), "check=1"])?;
        Ok(match publish::preflight(&resp) {
            Ok(()) => true,
            Err(lines) => {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), lines.as_bytes());
                false
            }
        })
    }

    /// `_stamp_subdomain_attrib claim <label> || true`.
    fn stamp(&mut self, label: &str) -> Result<(), Exit> {
        let pair = format!("label={}", quote(label));
        let resp = self.attrib(&[pair.as_str()])?;
        if let Err(lines) = publish::stamp(&resp, label) {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), lines.as_bytes());
        }
        Ok(())
    }

    /// `_nudge_subdomain_refresh`: `cli_post_form` with everything hidden.
    fn nudge(&mut self) -> Result<(), Exit> {
        let sent = self.client.call(Channel::Tcp, REFRESH, Call::PostForm(&[]));
        self.took(sent, true).map(|_| ())
    }

    /// `cp_raw=$(_connect_curl <method> <endpoint> <body> 2>/dev/null)`:
    /// the answer as `<status>\n<body>`, or `None` when it failed (its
    /// lines unseen). The air-gap refuses before any request. The daemon
    /// makes the call; with none to make it, the control plane itself
    /// with `tunnel.json`'s token (none: that fails too). A request that
    /// may have reached the daemon ends the command with 1.
    fn connect(
        &mut self,
        method: &str,
        endpoint: &str,
        body: &str,
    ) -> Result<Option<String>, Exit> {
        if !self.connect.airgap_off {
            return Ok(None);
        }
        let req = publish::proxy_request(method, endpoint, body);
        match self.client.connect_proxy(method == "GET", req.as_bytes()) {
            Ok(r) => match publish::proxied(r.status, &r.body) {
                publish::Proxied::Relayed(raw) => return Ok(Some(raw)),
                publish::Proxied::Failed => return Ok(None),
                publish::Proxied::NoDaemon => {}
            },
            Err(Error::MaybeApplied { .. }) => return Err(Exit::Error),
            Err(_) => {}
        }
        let Some((base, token)) = &self.connect.control else {
            return Ok(None);
        };
        let mut cp = Client::new(&base.host, base.port, "", Auth::Owner(token.clone()));
        crate::transport::remote::apply(&mut cp, base, token.clone());
        Ok(
            match cp.control_plane(method == "PUT", endpoint, body.as_bytes()) {
                Ok(r) => Some(publish::control_plane_raw(r.status, &r.body)),
                Err(_) => None,
            },
        )
    }

    fn unpublish(&mut self, o: &Opts) -> Result<Exit, Exit> {
        let status = self.get(STATUS, &[], true)?;
        let mut label = read::published_label(&status);
        if !o.label.is_empty() {
            label = o.label.clone();
        }
        let raw = self.post(SERVE, &["enabled=false"])?;
        // `|| true`: the reader's code is not the command's.
        let shown = read::serve_off(&raw);
        if !shown.stdout.is_empty() {
            if let Err(e) = out::stdout(&shown.stdout) {
                return Ok(out::after_stdout_error(&e));
            }
        }
        if !shown.stderr.is_empty() {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), shown.stderr.as_bytes());
        }
        let name = cli_name!();
        let mut text = String::from("\n");
        if label.is_empty() {
            text.push_str("If you created a nested subdomain for this brain, remove it with:\n");
            text.push_str(&format!("  {name} publish subdomain list\n"));
            text.push_str(&format!("  {name} publish subdomain rm <label>\n"));
        } else {
            text.push_str("To remove the nested subdomain advertisement:\n");
            text.push_str(&format!("  {name} publish subdomain rm {label}\n"));
            text.push_str(&format!(
                "  {name} publish subdomain unclaim {label}   # drop workspace attribution\n"
            ));
        }
        text.push('\n');
        text.push_str("Notes under .k2/wiki/ are kept (unpublish does not delete the vault).\n");
        if o.json {
            text.push_str(&read::unpublished_json(&label));
        }
        Ok(match out::stdout(&text) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        })
    }
}

#[cfg(test)]
mod tests;
