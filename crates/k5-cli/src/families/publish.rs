//! `k5 publish`, as the shell CLI's `cmd_publish` answers it
//! (`cli/src/connect.sh`): the services this workspace hosts. The
//! binary answers every verb: `run`, `start`, `stop`, `rm`, `ps` (`list`,
//! `ls`), `logs`, `frame`, `visibility`, `open`, `sessions`, `subdomain
//! claim` and `unclaim`, `publish` alone or `help`, and an unknown verb
//! (the readers are [`read`]'s); `client-ip`, `status <name>`, and
//! `secret` (`secrets`) and `env`, which hand over to the vault's `grant`,
//! `revoke` and `service` ([`local`]); `status` with no name and the
//! other `subdomain` verbs, the K2 Connect control plane ([`cp`]).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), the two gates
//!    ([`crate::prelude::gate`]; no connection gate for `secret` alone or
//!    `secret help`, which only print a page); `publish` is an `id` tool,
//!    so with `HOOK_SOCK` set: the stale-passport or require-passport
//!    lines, exit 3 ([`crate::prelude::id_tool`]).
//! 2. The words ([`parse`]): the help (on stderr, exit 0), a usage line
//!    (`_publish_err usage <hint>`: `Error: <hint>`, or one line of JSON
//!    after `--json`; exit 2), or the requests.
//! 3. Each `RESP=$(cli_send <fn> <endpoint> …) || RESP=""`: the cell
//!    socket first (every `/cli/publish/*` route is one it serves), then
//!    TCP. A call with no answer is the transport's line and exit 1
//!    (`_k2_end_command`); any other failure leaves `RESP` empty.
//! 4. The reader on `RESP` ([`read::handle`], [`read::site`],
//!    [`read::claimed`]).
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! `visibility <name> pin` asks first with no PIN (`"check": true`); when
//! the daemon says go on, the PIN is read (twice with no echo at a
//! terminal, else one line from stdin) and sent in the body over TCP,
//! never on argv or in an environment. A `needs_yes` answer at a terminal
//! asks `Go ahead? [y/N]` and sends again.
//!
//! A command line with `-h` or `--help` is the front's ([`crate::front`]).
//! The shell CLI answers ([`run`] returns `None`): a control-plane verb
//! when `tunnel.json` holds a token, air-gap is off and the control
//! plane's base or the person's proxy setting is one the binary doesn't
//! send through itself (it might have to call the control plane itself,
//! [`cp::prep`]); a word that is not UTF-8; `HOOK_SOCK` set with a
//! `CODEX_` variable exported; a `python3` the binary can't tell; a port
//! that is set but is no port curl takes; a `--port` too long for bash's
//! `test` (its own error line); `~` in `--cwd` with no `HOME`.
//!
//! # Where the binary differs
//!
//! - **A large answer.** The shell hands `RESP` to its `python3` in an
//!   environment variable. Linux takes at most 128 KiB in one, so there a
//!   larger answer (`publish ps` of thousands of services) stops the
//!   shell's `python3` before it starts (`Argument list too long`, exit
//!   126). The binary never puts an answer in its environment or its
//!   arguments and prints the whole list, as the shell does on macOS.
//! - An answer that is not UTF-8 is read with U+FFFD for each bad byte.
//! - Where Python raises, the traceback is Python 3.12's, without source
//!   lines.

pub mod cp;
pub mod local;
pub mod read;

use self::read::{err_line, Mode};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::families::{curl_port, Shown};
use crate::out::json::Value;
use crate::out::{self};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::prompt::{self, AtEof, Echo};
use crate::python;
use crate::text;
use crate::transport::encode::quote;
use crate::transport::{Auth, Call, Channel, Client, Error};
use k2_client::brand::{CLI_NAME as CLI, PRODUCT_NAME as PRODUCT};
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["publish"];
/// The binary answers it ([`run`] hands some forms back to the shell).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "publish";
/// `_publish_secret_usage`'s page (`cli/text/help/publish/secret.txt`).
pub const SECRET_PAGE: &str = "publish/secret";

/// The routes.
pub const RUN: &str = "/cli/publish/run";
pub const START: &str = "/cli/publish/start";
pub const STOP: &str = "/cli/publish/stop";
pub const RM: &str = "/cli/publish/rm";
pub const LIST: &str = "/cli/publish/list";
pub const LOGS: &str = "/cli/publish/logs";
pub const FRAME: &str = "/cli/publish/frame";
pub const VISIBILITY: &str = "/cli/publish/visibility";
pub const OPEN: &str = "/cli/publish/open";
pub const SESSIONS: &str = "/cli/publish/sessions";
pub const SESSIONS_REVOKE: &str = "/cli/publish/sessions/revoke";
pub const CLAIM: &str = "/cli/publish/subdomain/claim";
pub const UNCLAIM: &str = "/cli/publish/subdomain/unclaim";

/// What `visibility <name> <mode>` sends and asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vis {
    pub label: String,
    pub mode: String,
    pub reason: String,
    pub pin_days: String,
    pub yes: bool,
    pub dev_public: bool,
    pub json: bool,
}

/// What the words ask for, once the gates have passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// The shell CLI answers it: handed over before anything is printed
    /// or sent.
    Shell,
    /// `_publish_usage; exit 0`: the help, on stderr.
    Help,
    /// `Unknown 'k2 publish' subcommand: <sub>`, the help, exit 1.
    Unknown(String),
    /// `_publish_err <code> <hint>; exit 2`: nothing is sent.
    Usage {
        code: &'static str,
        hint: String,
        json: bool,
    },
    /// Lines on stderr and an exit: nothing is sent.
    Lines(String, Exit),
    /// One request, then `_publish_handle_resp`.
    Service {
        endpoint: &'static str,
        call: Req,
        json: bool,
        mode: Mode,
        verb: &'static str,
        long: bool,
    },
    /// `visibility <name>` with no mode: the GET, then the reader.
    VisGet { label: String, json: bool },
    /// `visibility <name> <mode>`.
    VisSet(Vis),
    /// `open <name>`.
    Open {
        body: String,
        json: bool,
        no_browser: bool,
    },
    /// `sessions <name>`: the GET, or the POST with `--revoke-all`.
    Sessions {
        endpoint: &'static str,
        call: Req,
        json: bool,
    },
    /// `subdomain claim|unclaim <label> [--transfer]`.
    Claim {
        action: &'static str,
        label: String,
        transfer: bool,
    },
    /// `status <name>`: `/cli/publish/list`, then `_publish_app_status`.
    AppStatus { name: String, json: bool },
    /// `client-ip`: the list, or the switch.
    ClientIp {
        endpoint: &'static str,
        call: Req,
        json: bool,
    },
    /// A help page (`publish/secret`), on stdout or stderr, then `exit`.
    Page {
        key: &'static str,
        stderr: bool,
        exit: Exit,
    },
    /// `cmd_vault <sub> <words…>`: `secret add|remove|list` and `env`.
    Vault {
        sub: &'static str,
        words: Vec<String>,
    },
    /// A K2 Connect control-plane verb ([`cp`]).
    Connect(cp::Form),
}

/// A request's shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Req {
    /// `cli_request`: these query pairs, already encoded.
    Get(Vec<String>),
    /// `cli_post_json`: this body.
    Post(String),
}

/// `k5 publish ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = crate::families::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    // `_k2_takes_dashdash` has no publish: a bare `--` anywhere is the
    // prelude's usage error, exit 2, before every gate and nothing sent.
    if words.contains(&"--") {
        out::stderr_line(&no_dashdash());
        return Some(Exit::Usage);
    }
    if !python::family_native(python::here()) {
        return None;
    }
    // The prelude refuses `subdomain` (but `claim` and `unclaim`) under
    // air-gap before its gates (`cli/src/cli-tools-policy.sh`).
    if words.first() == Some(&"subdomain") && !matches!(words.get(1), Some(&("claim" | "unclaim")))
    {
        let (state, file) = crate::airgap::state(&crate::offline::Env::current())?;
        if let Some(mut line) = crate::airgap::refusal(state, &file) {
            line.push(b'\n');
            let _ = out::write_flushed(&mut std::io::stderr().lock(), &line);
            return Some(Exit::Error);
        }
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
    let home = std::env::var("HOME").ok();
    let act = parse(&words, &settings.project, home.as_deref());
    let prep = match &act {
        Act::Shell => return None,
        Act::Vault { sub, words } => {
            let refs: Vec<&str> = words.iter().map(String::as_str).collect();
            if !super::vault::native(sub, &refs) {
                return None;
            }
            None
        }
        Act::Connect(_) => Some(cp::prep(&settings.home)?),
        _ => None,
    };
    Some(run_with(args, act, hook_sock.as_deref(), settings, prep))
}

/// What the request and its reader need.
struct Cx {
    client: Client,
    closing: Closing,
    passport: bool,
}

impl Cx {
    /// `RESP=$(cli_send <fn> <endpoint> …) || RESP=""`: the answer as `$(…)`
    /// holds it, or the exit after the transport's no-answer line.
    fn send(&mut self, endpoint: &str, req: &Req) -> Result<String, Exit> {
        let answer = match req {
            Req::Get(query) => {
                let refs: Vec<&str> = query.iter().map(String::as_str).collect();
                self.client.send(endpoint, Call::Request(&refs))
            }
            Req::Post(body) => self.client.send(endpoint, Call::PostJson(body.as_bytes())),
        };
        self.held(answer, false)
    }

    /// `soft`: `cli_post`, which has no no-answer line of its own (only a
    /// send on the cell socket that may have run ends the command).
    fn held(
        &mut self,
        answer: Result<crate::transport::Response, Error>,
        soft: bool,
    ) -> Result<String, Exit> {
        match answer {
            Ok(r) => {
                self.closing.answer(&r, self.passport);
                let mut b = crate::families::without_nuls(&r.body);
                while b.last() == Some(&b'\n') {
                    b.pop();
                }
                Ok(String::from_utf8_lossy(&b).into_owned())
            }
            Err(Error::NoAnswer(_)) if soft => Ok(String::new()),
            Err(
                e @ (Error::NoAnswer(_)
                | Error::MaybeApplied { .. }
                | Error::NoOwnerInSession { .. }),
            ) => {
                out::stderr_line(&e.to_string());
                Err(Exit::Error)
            }
            Err(_) => Ok(String::new()),
        }
    }
}

fn run_with(
    args: &[OsString],
    act: Act,
    hook_sock: Option<&Path>,
    mut settings: Settings,
    prep: Option<cp::Prep>,
) -> Exit {
    // The shell skips its connection gate for `secret` alone or `secret
    // help` (`_skip_conn_gate`): only the page, no daemon needed. The
    // virtual-desktop guard comes first and still holds.
    let secret_page = matches!(
        act,
        Act::Page {
            key: SECRET_PAGE,
            ..
        }
    );
    match prelude::gate(&settings) {
        Some(prelude::Gate::CannotConnect) if secret_page => {}
        Some(gate) => return gate.stop(),
        None => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    match &act {
        Act::Shell => unreachable!("run hands it to the shell"),
        Act::Help => return usage_page(Exit::Ok),
        Act::Unknown(sub) => {
            out::stderr_line(&format!("Unknown '{CLI} publish' subcommand: {sub}"));
            return usage_page(Exit::Error);
        }
        Act::Usage { code, hint, json } => {
            out::stderr_line(&err_line(code, hint, *json));
            return Exit::Usage;
        }
        Act::Lines(lines, exit) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), lines.as_bytes());
            return *exit;
        }
        Act::Page { key, stderr, exit } => return page(key, *stderr, *exit),
        Act::Claim { action, .. } if no_daemon(&settings) => return claim_unreachable(action),
        _ => {}
    }
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    if let Act::Vault { sub, words } = &act {
        // `cmd_vault <sub> …` past the gates `publish` already passed.
        let mut ctx = super::vault::Ctx {
            client,
            closing: Closing::default(),
            sock: crate::confirm::sock_path(&settings.home),
            args: args.to_vec(),
        };
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        let exit = super::vault::verbs::run(&mut ctx, sub, &refs);
        ctx.closing.flush(&mut ctx.client, exit);
        return exit;
    }
    let passport = client.passport().is_some();
    let mut cx = Cx {
        client,
        closing: Closing::default(),
        passport,
    };
    let exit = match (act, prep.as_ref()) {
        (Act::Connect(form), Some(prep)) => cp::Run { cx: &mut cx, prep }.run(&form),
        (act, _) => match flow(&mut cx, act, &settings.project) {
            Ok(shown) => shown.print(),
            Err(exit) => exit,
        },
    };
    let Cx {
        mut client,
        closing,
        ..
    } = cx;
    closing.flush(&mut client, exit);
    exit
}

/// `[ -z "$PORT" ] || [ -z "$TOKEN" ]`.
fn no_daemon(settings: &Settings) -> bool {
    settings.port_text.as_deref().is_none_or(str::is_empty) || settings.auth == Auth::None
}

/// `claim`/`unclaim` with no daemon to record it on.
fn claim_unreachable(action: &str) -> Exit {
    out::stderr_line(&format!(
        "Error: '{action}' records attribution on the local {PRODUCT} daemon, which isn't reachable."
    ));
    out::stderr_line("(Connection details: ~/.k2/heartbeat.port and ~/.k2/heartbeat.token)");
    Exit::Error
}

/// A help page on stdout (or stderr), then `exit`.
fn page(key: &str, stderr: bool, exit: Exit) -> Exit {
    let Some(text) = text::help(key) else {
        out::stderr_line(&format!("Error: no help page {key}"));
        return Exit::Error;
    };
    if stderr {
        let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
        return exit;
    }
    match out::stdout(text) {
        Ok(()) => exit,
        Err(e) => out::after_stdout_error(&e),
    }
}

/// `_publish_usage`: the help page on stderr, then `exit`.
fn usage_page(exit: Exit) -> Exit {
    match text::help("publish") {
        Some(page) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), page.as_bytes());
            exit
        }
        None => {
            out::stderr_line("Error: no help page publish");
            Exit::Error
        }
    }
}

/// The requests and the reader: what it printed, or the exit it ended
/// with.
fn flow(cx: &mut Cx, act: Act, project: &str) -> Result<Shown, Exit> {
    match act {
        Act::Service {
            endpoint,
            call,
            json,
            mode,
            verb,
            long,
        } => {
            let raw = cx.send(endpoint, &call)?;
            Ok(read::handle(&raw, json, mode, verb, long))
        }
        Act::VisGet { label, json } => {
            let raw = cx.send(
                VISIBILITY,
                &Req::Get(vec![format!("label={}", quote(&label))]),
            )?;
            Ok(read::site(&raw, json, "visibility"))
        }
        Act::VisSet(v) => visibility(cx, &v, project),
        Act::Open {
            body,
            json,
            no_browser,
        } => {
            let raw = cx.send(OPEN, &Req::Post(body))?;
            let raw = if !json && !no_browser && prompt::stdout_is_tty() {
                opened(raw)
            } else {
                raw
            };
            Ok(read::site(&raw, json, "open"))
        }
        Act::Sessions {
            endpoint,
            call,
            json,
        } => {
            let raw = cx.send(endpoint, &call)?;
            Ok(read::site(&raw, json, "sessions"))
        }
        Act::Claim {
            action,
            label,
            transfer,
        } => {
            let endpoint = if action == "claim" { CLAIM } else { UNCLAIM };
            let mut pairs = vec![format!("label={}", quote(&label))];
            if transfer {
                pairs.push("transfer=1".to_string());
            }
            let refs: Vec<&str> = pairs.iter().map(String::as_str).collect();
            let answer = cx.client.send(endpoint, Call::QueryPost(&refs));
            let raw = cx.held(answer, true)?;
            Ok(read::claimed(&raw, action, &label))
        }
        Act::AppStatus { name, json } => {
            let raw = cx.send(LIST, &Req::Get(Vec::new()))?;
            Ok(local::app_status(&raw, &name, json))
        }
        Act::ClientIp {
            endpoint,
            call,
            json,
        } => {
            let raw = cx.send(endpoint, &call)?;
            Ok(local::client_ip(&raw, json))
        }
        _ => unreachable!("handled before the requests"),
    }
}

/// `open`'s browser, for a person at a terminal: the answer's `url` opened
/// (`open` on macOS, else `xdg-open` with a display), and `"opened": true`
/// added to the answer when the opener says it did.
fn opened(raw: String) -> String {
    let Ok(Value::Object(mut d)) = crate::out::pylib::json::loads(&raw) else {
        return raw;
    };
    let url = match crate::families::get(&d, "url") {
        Some(u) if crate::out::py::truthy(u) => crate::out::py::str_of(u),
        _ => return raw,
    };
    let url = url.trim_end_matches('\n').to_string();
    if url.is_empty() {
        return raw;
    }
    let opener = if cfg!(target_os = "macos") {
        Some("open")
    } else {
        let display = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
            || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
        let path = std::env::var_os("PATH").unwrap_or_default();
        (display && daemon::find_on_path("xdg-open", path).is_some()).then_some("xdg-open")
    };
    let Some(opener) = opener else {
        return raw;
    };
    let ran = std::process::Command::new(opener)
        .arg(&url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ran {
        return raw;
    }
    let key = crate::out::json::Text::from("opened");
    match d.iter_mut().find(|(k, _)| *k == key) {
        Some((_, v)) => *v = Value::Bool(true),
        None => d.push((key, Value::Bool(true))),
    }
    read::dumps(&Value::Object(d))
}

/// `visibility <name> <mode>`'s steps.
fn visibility(cx: &mut Cx, v: &Vis, project: &str) -> Result<Shown, Exit> {
    let usage_err = |code: &str, hint: &str| {
        out::stderr_line(&err_line(code, hint, v.json));
        Exit::Usage
    };
    let label = &v.label;
    if v.mode == "public" && v.dev_public {
        if prompt::stdin_is_tty() && prompt::stderr_is_tty() {
            prompt::prompt(&format!(
                "{label} is a dev server. Made public, anyone with the address can see its source maps,\nerror pages with file paths, and anything the dev server evaluates.\nType {label} to make it public: "
            ));
            let typed = prompt::read_line(Echo::Shown, AtEof::Drop);
            if typed.expose() != label.as_bytes() {
                out::stderr_line("Nothing changed.");
                return Err(Exit::Usage);
            }
        } else if !v.yes {
            return Err(usage_err(
                "needs_yes",
                "public dev needs a person at a terminal, or --yes with no terminal",
            ));
        }
    }
    let check = v.mode == "pin";
    let mut yes = v.yes;
    let mut raw;
    let mut code;
    loop {
        let body = vis_body(v, yes, check, project, None);
        raw = cx.send(VISIBILITY, &Req::Post(body))?;
        code = read::resp_code(&raw);
        if code != "needs_yes" {
            break;
        }
        if prompt::stdin_is_tty() && prompt::stderr_is_tty() {
            let _ = out::write_flushed(
                &mut std::io::stderr().lock(),
                read::needs_yes_line(&raw).as_bytes(),
            );
            let answer = prompt::ask("Go ahead? [y/N] ");
            if matches!(answer.expose(), b"y" | b"Y" | b"yes" | b"YES") {
                yes = true;
            } else {
                out::stderr_line("Nothing changed.");
                return Err(Exit::Usage);
            }
        } else {
            break;
        }
    }
    if check && code.is_empty() && read::asks_pin(&raw) {
        // `_publish_read_pin`: typed or piped, never over plain http to
        // another computer.
        if !crate::prelude::plain_http::password_transport_ok() {
            return Err(Exit::Usage);
        }
        let pin = if prompt::stdin_is_tty() {
            let first = prompt::ask_hidden(&format!("PIN for {label} (6 to 64 characters): "));
            let again = prompt::ask_hidden("The same PIN again: ");
            if first.expose() != again.expose() {
                return Err(usage_err(
                    "usage",
                    "the two PINs don't match; nothing changed",
                ));
            }
            first
        } else {
            prompt::read_line(Echo::Shown, AtEof::Keep)
        };
        if pin.is_empty() {
            return Err(usage_err(
                "pin_required",
                "no PIN on stdin; nothing changed",
            ));
        }
        // `_K2_FORCE_TCP=1`, then `cli_post_json` itself: TCP only.
        let body = vis_body(v, yes, false, project, Some(pin.expose()));
        drop(pin);
        cx.client.force_tcp = true;
        let answer = cx
            .client
            .call(Channel::Tcp, VISIBILITY, Call::PostJson(body.as_bytes()));
        raw = cx.held(answer, false)?;
    }
    Ok(read::site(&raw, v.json, "visibility"))
}

/// `_publish_vis_body <check>` with the PIN (if any) on its stdin.
fn vis_body(v: &Vis, yes: bool, check: bool, project: &str, pin: Option<&[u8]>) -> String {
    let mut pairs = vec![
        ("label", read::s(&v.label)),
        ("mode", read::s(&v.mode)),
        ("project", read::s(project)),
    ];
    if !v.reason.is_empty() {
        pairs.push(("reason", read::s(&v.reason)));
    }
    if !v.pin_days.is_empty() {
        pairs.push(("pinDays", read::s(&v.pin_days)));
    }
    if yes {
        pairs.push(("yes", Value::Bool(true)));
    }
    if v.dev_public {
        pairs.push(("devIsPublic", Value::Bool(true)));
    }
    if check {
        pairs.push(("check", Value::Bool(true)));
    }
    if let Some(p) = pin.filter(|p| !p.is_empty()) {
        pairs.push(("pin", read::s(&String::from_utf8_lossy(p))));
    }
    read::dumps(&read::obj(pairs))
}

/// The words after `publish`, with `$PROJECT` and `$HOME`.
pub fn parse(words: &[&str], project: &str, home: Option<&str>) -> Act {
    let rest = words.get(1..).unwrap_or_default();
    match words.first().copied().unwrap_or("") {
        "run" => parse_run(rest, project, home),
        "start" => named(rest, "start", START, project),
        "stop" => named(rest, "stop", STOP, project),
        "rm" => parse_rm(rest, project),
        "ps" | "list" | "ls" => parse_ps(rest),
        "logs" => parse_logs(rest),
        "frame" => parse_frame(rest, project),
        "visibility" => parse_visibility(rest),
        "open" => parse_open(rest, project),
        "sessions" => parse_sessions(rest, project),
        "subdomain" => match rest.first().copied() {
            Some("claim" | "unclaim") => parse_claim(rest),
            _ => cp::parse_subdomain(rest),
        },
        "status" => cp::parse_status(rest),
        "client-ip" => local::parse_client_ip(rest),
        "secret" | "secrets" => local::parse_secret(rest),
        "env" => local::parse_env(rest),
        "" | "help" => Act::Help,
        sub => Act::Unknown(sub.to_string()),
    }
}

/// What follows `prefix` in `w` (`${1#--flag=}`).
fn after(w: &str, prefix: &str) -> String {
    w.strip_prefix(prefix).unwrap_or_default().to_string()
}

fn usage(code: &'static str, hint: impl Into<String>, json: bool) -> Act {
    Act::Usage {
        code,
        hint: hint.into(),
        json,
    }
}

fn body(pairs: Vec<(&str, Value)>) -> String {
    read::dumps(&read::obj(pairs))
}

/// `_publish_is_port`: `None` when bash's `test` can't read it as a
/// number (it prints its own error then).
fn is_port(p: &str) -> Option<bool> {
    if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) || p == "0" {
        return Some(false);
    }
    let n: i64 = p.parse().ok()?;
    Some(n <= 65535)
}

/// The run's flags (`cmd_publish_run`).
#[derive(Default)]
struct RunLine {
    name: String,
    cmd: String,
    port: String,
    cwd: String,
    no_tunnel: bool,
    json: bool,
    skin: bool,
    skin_root: String,
    transfer: bool,
    probe_timeout: String,
    dov: String,
    dev: bool,
    idle: String,
    mem: String,
    cpus: String,
}

const RUN_USAGE: &str = concat!(
    "usage: ",
    k2_client::cli_name!(),
    " publish run <name> (--cmd <shell>|--skin [<dir>]) --port <n> [--no-tunnel] [--cwd <path>]"
);

fn parse_run(rest: &[&str], project: &str, home: Option<&str>) -> Act {
    let mut l = RunLine::default();
    let mut i = 0;
    // `--flag <value>` and `--flag=<value>`, with the line for a missing
    // value.
    let valued: &[(&str, &str)] = &[
        ("--idle", "--idle needs a duration from 5m to 24h, or off"),
        ("--mem", "--mem needs an amount, like 4G"),
        ("--cpus", "--cpus needs a number of CPUs, like 2"),
        ("--disk-override", "--disk-override needs a reason"),
        (
            "--probe-timeout",
            "--probe-timeout needs a duration (90s, 10m, at most 15m) or 'default'",
        ),
        ("--cmd", "--cmd needs a shell string"),
        ("--port", "--port needs a number"),
        ("--cwd", "--cwd needs a path"),
    ];
    while i < rest.len() {
        let w = rest[i];
        let mut took = None;
        for (flag, missing) in valued {
            if w == *flag {
                let Some(v) = rest.get(i + 1) else {
                    return usage("usage", *missing, l.json);
                };
                took = Some((*flag, (*v).to_string(), 2));
                break;
            }
            if let Some(v) = w.strip_prefix(flag).and_then(|r| r.strip_prefix('=')) {
                took = Some((*flag, v.to_string(), 1));
                break;
            }
        }
        if let Some((flag, v, n)) = took {
            let slot = match flag {
                "--idle" => &mut l.idle,
                "--mem" => &mut l.mem,
                "--cpus" => &mut l.cpus,
                "--disk-override" => &mut l.dov,
                "--probe-timeout" => &mut l.probe_timeout,
                "--cmd" => &mut l.cmd,
                "--port" => &mut l.port,
                _ => &mut l.cwd,
            };
            *slot = v;
            i += n;
            continue;
        }
        match w {
            "--dev" => l.dev = true,
            "--skin" => {
                l.skin = true;
                if let Some(next) = rest.get(i + 1) {
                    if !next.starts_with('-') {
                        l.skin_root = (*next).to_string();
                        i += 1;
                    }
                }
            }
            "--no-tunnel" => l.no_tunnel = true,
            "--transfer" => l.transfer = true,
            "--json" => l.json = true,
            _ if w.starts_with("--skin=") => {
                l.skin = true;
                l.skin_root = after(w, "--skin=");
            }
            _ if w.starts_with('-') => {
                return usage(
                    "usage",
                    format!(
                        "unknown flag '{w}' \u{2014} usage: {CLI} publish run <name> (--cmd <shell>|--skin [<dir>]) --port <n> [--no-tunnel] [--cwd <path>] [--transfer] [--probe-timeout <dur>] [--dev [--idle <dur>] [--mem <n>] [--cpus <n>]]"
                    ),
                    l.json,
                )
            }
            _ if l.name.is_empty() => l.name = w.to_string(),
            _ => return usage("usage", format!("unexpected argument '{w}'"), l.json),
        }
        i += 1;
    }
    let json = l.json;
    if l.skin && !l.cmd.is_empty() {
        return usage("usage", "--cmd and --skin are mutually exclusive", json);
    }
    if l.dev && l.skin {
        return usage(
            "dev_needs_cmd",
            "--dev runs a dev server, so it needs --cmd (--skin hosts static files)",
            json,
        );
    }
    if l.dev && l.no_tunnel {
        return usage(
            "dev_needs_tunnel",
            format!("--dev puts a dev server behind {PRODUCT}'s check at its address, so it can't use --no-tunnel. To run one for yourself only, run the command in a terminal"),
            json,
        );
    }
    if !l.dev && (!l.idle.is_empty() || !l.mem.is_empty() || !l.cpus.is_empty()) {
        return usage("usage", "--idle, --mem and --cpus go with --dev", json);
    }
    if l.skin && !l.cwd.is_empty() {
        return usage("usage", "--cwd and --skin are mutually exclusive", json);
    }
    if l.name.is_empty() || l.port.is_empty() || (!l.skin && l.cmd.is_empty()) {
        return usage("usage", RUN_USAGE, json);
    }
    match is_port(&l.port) {
        None => return Act::Shell,
        Some(false) => return usage("usage", "--port must be an integer 1-65535", json),
        Some(true) => {}
    }
    if !l.probe_timeout.is_empty() {
        if let Err(e) = read::probe_timeout(&l.probe_timeout) {
            return usage("usage", e, json);
        }
    }
    if l.skin && !l.skin_root.is_empty() {
        let r = &l.skin_root;
        let b = r.as_bytes();
        let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
        if r.starts_with('/') || r.starts_with('~') || drive {
            return usage(
                "usage",
                "skinRoot must be a workspace-relative directory",
                json,
            );
        }
        if r.contains("..") {
            return usage("usage", "skinRoot must not contain ..", json);
        }
        if !Path::new(&format!("{project}/{r}")).is_dir() {
            return usage("usage", format!("skinRoot is not a directory: {r}"), json);
        }
    }
    let mut cwd = l.cwd.clone();
    if cwd.is_empty() && !l.skin {
        cwd = project.to_string();
    }
    let b = cwd.as_bytes();
    let drive_path =
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    let resolved = cwd.is_empty() || cwd.starts_with('/') || drive_path;
    if !resolved {
        if cwd == "~" || cwd.starts_with("~/") {
            let Some(home) = home else {
                return Act::Shell;
            };
            cwd = format!("{home}{}", after(&cwd, "~"));
        } else {
            cwd = crate::out::pylib::path::abspath(&cwd);
        }
    }
    let port_digits = l.port.trim_start_matches('0');
    let mut pairs = vec![
        ("name", read::s(&l.name)),
        (
            "port",
            Value::Int(
                if port_digits.is_empty() {
                    "0"
                } else {
                    port_digits
                }
                .to_string(),
            ),
        ),
        ("project", read::s(project)),
    ];
    if l.dev {
        pairs.push(("dev", Value::Bool(true)));
        for (k, v) in [("idle", &l.idle), ("mem", &l.mem), ("cpus", &l.cpus)] {
            if !v.is_empty() {
                pairs.push((k, read::s(v)));
            }
        }
    }
    if !l.cmd.is_empty() {
        pairs.push(("cmd", read::s(&l.cmd)));
    }
    if !cwd.is_empty() {
        pairs.push(("cwd", read::s(&cwd)));
    }
    if l.no_tunnel {
        pairs.push(("noTunnel", Value::Bool(true)));
    }
    if l.transfer {
        pairs.push(("transfer", Value::Bool(true)));
    }
    let strip = |t: &str| t.trim_matches(crate::families::py_space).to_string();
    let pt = strip(&l.probe_timeout);
    if !pt.is_empty() {
        pairs.push(("probeTimeout", read::s(&pt)));
    }
    let dov = strip(&l.dov);
    if !dov.is_empty() {
        pairs.push(("diskOverride", read::s(&dov)));
    }
    if l.skin {
        pairs.push(("skin", Value::Bool(true)));
        if !l.skin_root.is_empty() {
            pairs.push(("skinRoot", read::s(&l.skin_root)));
        }
    }
    Act::Service {
        endpoint: RUN,
        call: Req::Post(body(pairs)),
        json,
        mode: Mode::Service,
        verb: "run",
        long: false,
    }
}

/// One `<name>` and `--json`; `Err`: the usage act.
fn name_and_json(
    rest: &[&str],
    unknown: &str,
    extra: &[&str],
) -> Result<(String, bool, Vec<String>), Box<Act>> {
    let mut name = String::new();
    let mut json = false;
    let mut flags = Vec::new();
    for w in rest {
        match *w {
            "--json" => json = true,
            f if extra.contains(&f) => flags.push(f.to_string()),
            f if f.starts_with('-') => {
                return Err(Box::new(usage(
                    "usage",
                    format!("unknown flag '{f}' \u{2014} {unknown}"),
                    json,
                )))
            }
            a if name.is_empty() => name = a.to_string(),
            a => {
                return Err(Box::new(usage(
                    "usage",
                    format!("unexpected argument '{a}'"),
                    json,
                )))
            }
        }
    }
    Ok((name, json, flags))
}

/// `_publish_named_post <verb> <endpoint>` (`start`, `stop`).
fn named(rest: &[&str], verb: &'static str, endpoint: &'static str, project: &str) -> Act {
    let line = format!("usage: {CLI} publish {verb} <name>");
    let (name, json, _) = match name_and_json(rest, &line, &[]) {
        Ok(x) => x,
        Err(a) => return *a,
    };
    if name.is_empty() {
        return usage("usage", line, json);
    }
    Act::Service {
        endpoint,
        call: Req::Post(body(vec![
            ("name", read::s(&name)),
            ("project", read::s(project)),
        ])),
        json,
        mode: Mode::Service,
        verb,
        long: false,
    }
}

fn parse_rm(rest: &[&str], project: &str) -> Act {
    let (name, json, flags) = match name_and_json(
        rest,
        &format!(
            "usage: {CLI} publish rm <name> [--keep-hostname]  (not: {CLI} publish subdomain rm)"
        ),
        &["--keep-hostname"],
    ) {
        Ok(x) => x,
        Err(a) => return *a,
    };
    if name.is_empty() {
        return usage(
            "usage",
            format!("usage: {CLI} publish rm <name> [--keep-hostname]  (service; for a hostname use: {CLI} publish subdomain rm <label>)"),
            json,
        );
    }
    let mut pairs = vec![("name", read::s(&name)), ("project", read::s(project))];
    if !flags.is_empty() {
        pairs.push(("keepHostname", Value::Bool(true)));
    }
    Act::Service {
        endpoint: RM,
        call: Req::Post(body(pairs)),
        json,
        mode: Mode::Service,
        verb: "rm",
        long: false,
    }
}

fn parse_ps(rest: &[&str]) -> Act {
    let line = format!("usage: {CLI} publish ps [--long] [--json]");
    let mut json = false;
    let mut long = false;
    for w in rest {
        match *w {
            "--json" => json = true,
            "--long" | "-l" => long = true,
            f if f.starts_with('-') => {
                return usage("usage", format!("unknown flag '{f}' \u{2014} {line}"), json)
            }
            a => {
                return usage(
                    "usage",
                    format!("unexpected argument '{a}' \u{2014} {line}"),
                    json,
                )
            }
        }
    }
    Act::Service {
        endpoint: LIST,
        call: Req::Get(Vec::new()),
        json,
        mode: Mode::List,
        verb: "ps",
        long,
    }
}

fn parse_logs(rest: &[&str]) -> Act {
    let line = format!("usage: {CLI} publish logs <name> [--tail]");
    let mut name = String::new();
    let mut tail = false;
    for w in rest {
        match *w {
            "--tail" => tail = true,
            f if f.starts_with('-') => {
                return usage(
                    "usage",
                    format!("unknown flag '{f}' \u{2014} {line}"),
                    false,
                )
            }
            a if name.is_empty() => name = a.to_string(),
            a => return usage("usage", format!("unexpected argument '{a}'"), false),
        }
    }
    if name.is_empty() {
        return usage("usage", line, false);
    }
    let mut query = vec![format!("name={}", quote(&name))];
    if tail {
        query.push("tail=1".to_string());
    }
    Act::Service {
        endpoint: LOGS,
        call: Req::Get(query),
        json: false,
        mode: Mode::Logs,
        verb: "logs",
        long: false,
    }
}

fn parse_frame(rest: &[&str], project: &str) -> Act {
    let line = format!(
        "usage: {CLI} publish frame <name> (--allow <origin> ... | --none | --reset) [--json]"
    );
    let mut name = String::new();
    let mut json = false;
    let mut mode = "";
    let mut allow = String::new();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let mut add = |mode: &mut &str, v: &str| -> bool {
            if !matches!(*mode, "" | "allow") {
                return false;
            }
            *mode = "allow";
            if !allow.is_empty() {
                allow.push(' ');
            }
            allow.push_str(v);
            true
        };
        match w {
            "--allow" => {
                let Some(v) = rest.get(i + 1) else {
                    return usage(
                        "usage",
                        "--allow needs an origin, e.g. https://partner.example",
                        json,
                    );
                };
                if !add(&mut mode, v) {
                    return usage("usage", line, json);
                }
                i += 2;
                continue;
            }
            _ if w.starts_with("--allow=") => {
                if !add(&mut mode, &after(w, "--allow=")) {
                    return usage("usage", line, json);
                }
            }
            "--none" | "--reset" => {
                if !mode.is_empty() {
                    return usage("usage", line, json);
                }
                mode = &w[2..];
            }
            "--json" => json = true,
            f if f.starts_with('-') => {
                return usage("usage", format!("unknown flag '{f}' \u{2014} {line}"), json)
            }
            a if name.is_empty() => name = a.to_string(),
            a => {
                return usage(
                    "usage",
                    format!("unexpected argument '{a}' \u{2014} {line}"),
                    json,
                )
            }
        }
        i += 1;
    }
    if name.is_empty() || mode.is_empty() {
        return usage("usage", line, json);
    }
    let mut pairs = vec![("name", read::s(&name)), ("project", read::s(project))];
    if mode == "allow" {
        pairs.push(("allow", read::s(&allow)));
    } else {
        pairs.push((
            if mode == "none" { "none" } else { "reset" },
            Value::Bool(true),
        ));
    }
    Act::Service {
        endpoint: FRAME,
        call: Req::Post(body(pairs)),
        json,
        mode: Mode::Service,
        verb: "frame",
        long: false,
    }
}

fn parse_visibility(rest: &[&str]) -> Act {
    let line = format!("usage: {CLI} publish visibility <name> [private|pin|public] [--reason <why>] [--pin-days N] [--yes] [--dev-is-public] [--json]");
    let mut v = Vis {
        label: String::new(),
        mode: String::new(),
        reason: String::new(),
        pin_days: String::new(),
        yes: false,
        dev_public: false,
        json: false,
    };
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        match w {
            _ if w == "--pin" || w.starts_with("--pin=") => {
                let name = if v.label.is_empty() {
                    "<name>"
                } else {
                    &v.label
                };
                return usage(
                    "pin_in_argv",
                    format!("a PIN on the command line lands in your shell history and process lists. Pipe it, or leave it out and type it when asked: {CLI} publish visibility {name} pin < pin.txt"),
                    v.json,
                );
            }
            "--reason" | "--pin-days" => {
                let Some(x) = rest.get(i + 1) else {
                    let hint = if w == "--reason" {
                        "--reason needs a few words on why"
                    } else {
                        "--pin-days needs a number from 1 to 30"
                    };
                    return usage("usage", hint, v.json);
                };
                if w == "--reason" {
                    v.reason = (*x).to_string();
                } else {
                    v.pin_days = (*x).to_string();
                }
                i += 2;
                continue;
            }
            _ if w.starts_with("--reason=") => v.reason = after(w, "--reason="),
            _ if w.starts_with("--pin-days=") => v.pin_days = after(w, "--pin-days="),
            "--yes" | "-y" => v.yes = true,
            "--dev-is-public" => v.dev_public = true,
            "--json" => v.json = true,
            f if f.starts_with('-') => {
                return usage(
                    "usage",
                    format!("unknown flag '{f}' \u{2014} {line}"),
                    v.json,
                )
            }
            a if v.label.is_empty() => v.label = a.to_string(),
            a if v.mode.is_empty() => v.mode = a.to_string(),
            a => {
                return usage(
                    "usage",
                    format!("unexpected argument '{a}' \u{2014} {line}"),
                    v.json,
                )
            }
        }
        i += 1;
    }
    if v.label.is_empty() {
        return usage("usage", line, v.json);
    }
    if v.mode.is_empty() {
        return Act::VisGet {
            label: v.label,
            json: v.json,
        };
    }
    if !matches!(v.mode.as_str(), "private" | "pin" | "public") {
        return usage(
            "usage",
            format!("the mode is private, pin or public \u{2014} {line}"),
            v.json,
        );
    }
    let days = v.pin_days.as_bytes();
    if !(days.len() <= 2 && days.iter().all(u8::is_ascii_digit)) {
        return usage("usage", "--pin-days needs a number from 1 to 30", v.json);
    }
    Act::VisSet(v)
}

fn parse_open(rest: &[&str], project: &str) -> Act {
    let line = format!("usage: {CLI} publish open <name> [--path <p>] [--no-browser] [--json]");
    let mut label = String::new();
    let mut path = String::new();
    let mut json = false;
    let mut no_browser = false;
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        match w {
            "--path" => {
                let Some(p) = rest.get(i + 1) else {
                    return usage("usage", "--path needs a path, like /docs", json);
                };
                path = (*p).to_string();
                i += 2;
                continue;
            }
            _ if w.starts_with("--path=") => path = after(w, "--path="),
            "--no-browser" => no_browser = true,
            "--json" => json = true,
            f if f.starts_with('-') => {
                return usage("usage", format!("unknown flag '{f}' \u{2014} {line}"), json)
            }
            a if label.is_empty() => label = a.to_string(),
            a => {
                return usage(
                    "usage",
                    format!("unexpected argument '{a}' \u{2014} {line}"),
                    json,
                )
            }
        }
        i += 1;
    }
    if label.is_empty() {
        return usage("usage", line, json);
    }
    let mut pairs = vec![("label", read::s(&label)), ("project", read::s(project))];
    if !path.is_empty() {
        pairs.push(("path", read::s(&path)));
    }
    Act::Open {
        body: body(pairs),
        json,
        no_browser,
    }
}

fn parse_sessions(rest: &[&str], project: &str) -> Act {
    let line = format!("usage: {CLI} publish sessions <name> [--revoke-all] [--json]");
    let mut label = String::new();
    let mut revoke = false;
    let mut json = false;
    for w in rest {
        match *w {
            "--revoke-all" => revoke = true,
            "--json" => json = true,
            f if f.starts_with('-') => {
                return usage("usage", format!("unknown flag '{f}' \u{2014} {line}"), json)
            }
            a if label.is_empty() => label = a.to_string(),
            a => {
                return usage(
                    "usage",
                    format!("unexpected argument '{a}' \u{2014} {line}"),
                    json,
                )
            }
        }
    }
    if label.is_empty() {
        return usage("usage", line, json);
    }
    if revoke {
        Act::Sessions {
            endpoint: SESSIONS_REVOKE,
            call: Req::Post(body(vec![
                ("label", read::s(&label)),
                ("project", read::s(project)),
            ])),
            json,
        }
    } else {
        Act::Sessions {
            endpoint: SESSIONS,
            call: Req::Get(vec![format!("label={}", quote(&label))]),
            json,
        }
    }
}

/// The prelude's line for a `--` on a verb that takes none
/// (`cli/src/cli-tools-policy.sh`, after `_k2_takes_dashdash`).
pub fn no_dashdash() -> String {
    format!("usage: publish takes no '--' ({CLI} help publish)")
}

/// `subdomain claim|unclaim`: the words after `subdomain`. A word
/// beginning with `-` other than `--transfer` is an unknown flag, exit 2
/// (a label never begins with one, so `-h` can't be one).
fn parse_claim(rest: &[&str]) -> Act {
    let action = match rest.first().copied() {
        Some("claim") => "claim",
        Some("unclaim") => "unclaim",
        _ => return Act::Shell,
    };
    let mut label = String::new();
    let mut transfer = false;
    for w in &rest[1..] {
        match *w {
            "--transfer" => transfer = true,
            f if f.starts_with('-') => {
                return Act::Lines(format!("Error: unknown flag '{f}'\n"), Exit::Usage)
            }
            a => {
                if label.is_empty() {
                    label = a.to_string();
                }
            }
        }
    }
    if label.is_empty() {
        let mut lines = format!("Usage: {CLI} publish subdomain {action} <label>\n");
        if action == "claim" {
            lines.push_str(&format!(
                "       {CLI} publish subdomain claim <label> --transfer   (owner/admin; audited)\n"
            ));
        }
        return Act::Lines(lines, Exit::Error);
    }
    if transfer && action != "claim" {
        return Act::Lines(
            "Error: --transfer only applies to claim/create/point/run\n".to_string(),
            Exit::Usage,
        );
    }
    Act::Claim {
        action,
        label,
        transfer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_verb_is_the_binary_s() {
        for w in [
            &["status"][..],
            &["status", "web"],
            &["subdomain"],
            &["subdomain", "list"],
            &["subdomain", "create", "a"],
            &["client-ip"],
            &["secret", "add"],
            &["secrets"],
            &["env", "x"],
            &["ps"],
            &["subdomain", "claim", "a"],
            &[],
            &["nope"],
        ] {
            assert_ne!(parse(w, "/p", None), Act::Shell, "{w:?}");
        }
    }

    /// A label never begins with `-`: every subdomain verb refuses one as
    /// an unknown flag, exit 2 (the shell's f523e1e546 and f267f23585).
    #[test]
    fn a_dash_label_is_an_unknown_flag() {
        for (w, flag) in [
            (&["subdomain", "claim", "-x"][..], "-x"),
            (&["subdomain", "claim", "--transfer", "-h"], "-h"),
            (&["subdomain", "unclaim", "-"], "-"),
            (&["subdomain", "create", "-x", "--target", "t"], "-x"),
            (&["subdomain", "create", "a", "--target", "t", "-q"], "-q"),
            (&["subdomain", "point", "--target", "t", "-x"], "-x"),
            (&["subdomain", "target", "a", "-x"], "-x"),
            (&["subdomain", "rm", "-x"], "-x"),
            (&["subdomain", "remove", "--keep"], "--keep"),
            (&["subdomain", "delete", "-x"], "-x"),
        ] {
            assert_eq!(
                parse(w, "/p", None),
                Act::Lines(format!("Error: unknown flag '{flag}'\n"), Exit::Usage),
                "{w:?}"
            );
        }
        // create and point still skip an unknown long flag, and --target
        // takes a dash value.
        assert!(matches!(
            parse(
                &["subdomain", "create", "a", "--nope", "--target", "-t"],
                "/p",
                None
            ),
            Act::Connect(cp::Form::Create { .. })
        ));
    }

    #[test]
    fn a_port_bash_cannot_read_goes_to_the_shell() {
        assert_eq!(is_port("00"), Some(true));
        assert_eq!(is_port("65536"), Some(false));
        assert_eq!(is_port("0"), Some(false));
        assert_eq!(is_port("99999999999999999999"), None);
        assert_eq!(
            parse(
                &["run", "a", "--cmd", "x", "--port", "99999999999999999999"],
                "/p",
                None
            ),
            Act::Shell
        );
    }

    #[test]
    fn the_run_body_is_python_s() {
        let Act::Service { call, .. } = parse(
            &[
                "run",
                "web",
                "--cmd",
                "npm start",
                "--port",
                "08080",
                "--cwd",
                "/srv",
                "--probe-timeout",
                " 90s ",
                "--no-tunnel",
            ],
            "/p",
            None,
        ) else {
            panic!("a request")
        };
        assert_eq!(
            call,
            Req::Post(
                r#"{"name": "web", "port": 8080, "project": "/p", "cmd": "npm start", "cwd": "/srv", "noTunnel": true, "probeTimeout": "90s"}"#
                    .to_string()
            )
        );
    }
}
