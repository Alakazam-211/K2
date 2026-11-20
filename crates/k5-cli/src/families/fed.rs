//! `k5 fed`: federation, this server's links to other servers, as the
//! shell CLI's `cmd_fed` answers it (`cli/src/federation.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at
//!    once, and the two gates ([`crate::prelude`]). `fed` is an open tool.
//! 2. In a session (a passport): `pair-*`, `confirm` and `outbox` are the
//!    owner's, three lines and exit 3 ([`OWNER_ONLY`]).
//! 3. Outside one, the owner token is `daemon.token` in the K2 home, every
//!    space taken out (`tr -d '[:space:]'`); `heartbeat.token` is not read.
//! 4. The verb (its first word):
//!    - `send <peer>::<workspace>::<agent> "message"`: the passport, else
//!      the owner token, as Bearer on one POST of `{to, text,
//!      from_workspace}` ([`Client::own_curl`], at most 60 s); what came
//!      back is printed as `$(…)` held it, and a newline; exit 0;
//!    - `pair-start [--subdomain S] [--url U] [--label L]`, `pair-join`,
//!      `pair-finish`, `pair-verify` (`--token-stdin`, `-` or the token):
//!      the owner token on one POST; the answer's reader prints the next
//!      token ([`pair`]). A failed call ends the command with curl's code;
//!    - `pair-request --pubkey <file> …`: one POST with no credential, the
//!      key file's text in it; the answer as it came, and a newline;
//!    - `confirm <fingerprint> <sas>`: the owner token on one POST; the
//!      answer as it came, and a newline;
//!    - `pubkey [--pem|--fingerprint|--json]`: openssl on the key file
//!      ([`pubkey`]);
//!    - `enable`, `disable`: a `cli_post_json` of the switch, piped into
//!      its reader; `status`: a `cli_request` of the settings, likewise;
//!    - `peers`: `cli_send cli_request`, piped into its reader ([`peers`]);
//!    - `outbox`: the owner token on one GET, piped into its reader;
//!    - anything else (no verb too): the usage lines, exit 2.
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! Still the shell's: a word that is not UTF-8, a command line with `-h`
//! or `--help` ([`crate::front`]; the shell has no help page for `fed`:
//! it prints the usage on stdout, exit 0, and sends nothing), a token on stdin from a terminal (the no-echo read is
//! the vault lane's), `pubkey` where `shasum` or `awk` is missing (the
//! shell's "command not found" line names its script), a port that is
//! set but is no port curl takes, and a `python3` the binary can't tell.

pub mod pair;
pub mod peers;
pub mod pubkey;

use super::{curl_port, without_nuls, Shown};
use crate::cards::Decode;
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::offline::Env;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::{Client, OwnCurl, Response};
use k2_client::cli_name;
use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["fed"];
/// The binary's, the whole row.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// None: the row is native whole.
pub const VERBS: &[crate::route::Verb] = &[];

/// The routes.
pub const SEND: &str = "/cli/federation/send";
pub const PAIR_REQUEST: &str = "/cli/federation/pair/request";
pub const CONFIRM: &str = "/cli/federation/pair/confirm";
pub const PEERS: &str = "/cli/federation/peers";
pub const OUTBOX: &str = "/cli/federation/outbox";
pub const SETTINGS_UPDATE: &str = "/cli/settings/update";
pub const SETTINGS_GET: &str = "/cli/settings/get";

/// `send`'s `--max-time 60`; every other call's is 30.
pub const SEND_MAX_TIME: Duration = Duration::from_secs(60);
pub const MAX_TIME: Duration = Duration::from_secs(30);

/// `_fed_owner_only`'s lines, after its first, on stderr, then exit 3.
pub const OWNER_ONLY: [&str; 2] = [
    concat!(
        "This shell is an agent session (it carries a session passport), so ",
        cli_name!(),
        " doesn't use the owner token here."
    ),
    "Ask the owner to run it from their own terminal.",
];

/// The `*)` arm's lines, on stderr, then exit 2.
pub const USAGE: [&str; 8] = [
    concat!(
        "usage: ",
        cli_name!(),
        " fed <enable|disable|status|peers|pubkey|send|outbox|pair-start|pair-join|pair-finish|pair-verify|pair-request|confirm> ..."
    ),
    "  enable/disable/status: flip the persisted federation switch (headless-friendly)",
    concat!(
        "  peers: list pinned servers (use their host with ",
        cli_name!(),
        " msg agent::host)"
    ),
    "  pair-start [--subdomain S] [--url http://host:port] [--label L]: start a pairing (prints a token for the other owner)",
    "  pair-join / pair-finish / pair-verify --token-stdin | <token>: the next steps; both owners then compare codes and confirm (a token on the command line is visible to other users on this machine)",
    "  pair-request: the pre-0.45.2 pairing; refused for a server not paired yet",
    "  send/outbox/pair-*/confirm need federation ON; pubkey works anytime",
    "  outbox: queued + dead-lettered cross-server messages per peer",
];

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    Send {
        to: String,
        text: String,
    },
    /// `pair-start`'s fields, or the code it stops with (a line or none).
    PairStart(Result<[String; 3], (Exit, Option<String>)>),
    /// `pair-join`, `pair-finish` or `pair-verify`: the step and the token
    /// word (`--token-stdin` or `-` read stdin).
    PairToken {
        step: String,
        token: String,
    },
    /// `pair-request`'s fields (label, subdomain, url, key file), or the
    /// code it stops with.
    PairRequest(Result<[String; 4], (Exit, Option<String>)>),
    Confirm {
        fp: String,
        sas: String,
    },
    Pubkey(String),
    /// `enable` (true) or `disable`.
    Switch(bool),
    Status,
    Peers,
    Outbox,
    Usage,
}

impl Form {
    fn owner_only(&self) -> Option<&'static str> {
        match self {
            Form::PairStart(_) | Form::PairToken { .. } => {
                Some("it pairs this server with another one")
            }
            Form::Confirm { .. } => Some("it trusts another server"),
            Form::Outbox => Some("it lists every workspace's queued messages"),
            _ => None,
        }
    }

    /// A token read from stdin.
    fn reads_stdin(&self) -> bool {
        matches!(self, Form::PairToken { token, .. } if token == "--token-stdin" || token == "-")
    }
}

/// The value after a flag: `${2:-}`, and `shift 2` failing under `set -e`
/// when it is the last word (exit 1, nothing printed).
fn value<'a>(rest: &[&'a str], i: usize) -> Result<&'a str, (Exit, Option<String>)> {
    rest.get(i + 1).copied().ok_or((Exit::Error, None))
}

/// `pair-start`'s and `pair-request`'s flags into `fields` (label,
/// subdomain, url, pubkey); `None` for a word the verb doesn't take.
fn flags(verb: &str, rest: &[&str], pubkey: bool) -> Result<[String; 4], (Exit, Option<String>)> {
    let mut f: [String; 4] = Default::default();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let (slot, step) = match w {
            "--pubkey" if pubkey => (3, Some(value(rest, i)?)),
            "--label" => (0, Some(value(rest, i)?)),
            "--subdomain" => (1, Some(value(rest, i)?)),
            "--url" | "--base-url" | "--baseUrl" => (2, Some(value(rest, i)?)),
            _ if pubkey && w.starts_with("--pubkey=") => (3, None),
            _ if w.starts_with("--label=") => (0, None),
            _ if w.starts_with("--subdomain=") => (1, None),
            _ if ["--url=", "--base-url=", "--baseUrl="]
                .iter()
                .any(|p| w.starts_with(p)) =>
            {
                (2, None)
            }
            _ => {
                return Err((
                    Exit::Usage,
                    Some(format!("{} fed {verb}: unknown arg: {w}", cli_name!())),
                ))
            }
        };
        match step {
            Some(v) => {
                f[slot] = v.to_string();
                i += 2;
            }
            None => {
                f[slot] = w.split_once('=').map_or("", |(_, v)| v).to_string();
                i += 1;
            }
        }
    }
    Ok(f)
}

/// The words after `fed`.
pub fn parse(words: &[&str]) -> Form {
    let sub = words.first().copied().unwrap_or("");
    let rest = words.get(1..).unwrap_or_default();
    let arg = |i: usize| rest.get(i).copied().unwrap_or("").to_string();
    match sub {
        "send" => Form::Send {
            to: arg(0),
            text: arg(1),
        },
        "pair-start" => Form::PairStart(flags(sub, rest, false).map(|[l, s, u, _]| [l, s, u])),
        "pair-join" | "pair-finish" | "pair-verify" => Form::PairToken {
            step: sub.to_string(),
            token: arg(0),
        },
        "pair-request" => Form::PairRequest(flags(sub, rest, true)),
        "confirm" => Form::Confirm {
            fp: arg(0),
            sas: arg(1),
        },
        "pubkey" => Form::Pubkey(arg(0)),
        "enable" => Form::Switch(true),
        "disable" => Form::Switch(false),
        "status" => Form::Status,
        "peers" => Form::Peers,
        "outbox" => Form::Outbox,
        _ => Form::Usage,
    }
}

/// `k5 fed ...`: `args` is the whole command line after `argv[0]`.
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
    let port = settings.port_text.as_deref().and_then(curl_port)?;
    let form = parse(&words);
    let mut decode = Decode::Strict;
    if form.reads_stdin() {
        if std::io::stdin().is_terminal() {
            return None;
        }
        decode = crate::cards::decode(&Env::current())?;
    }
    if let Form::Pubkey(_) = form {
        let key = pubkey::key_file(&settings.home);
        if key.is_file()
            && pubkey::on_path("openssl").is_some()
            && (pubkey::on_path("shasum").is_none() || pubkey::on_path("awk").is_none())
        {
            return None;
        }
    }
    Some(run_with(args, form, settings, port, decode))
}

/// Lines on stderr, then `exit`.
fn lines(list: &[&str], exit: Exit) -> Exit {
    for l in list {
        out::stderr_line(l);
    }
    exit
}

/// `$(cat "$K2_HOME/daemon.token" | tr -d '[:space:]')` when it is a file.
pub fn owner_token(home: &Path) -> String {
    let f = home.join("daemon.token");
    if !f.is_file() {
        return String::new();
    }
    let bytes = std::fs::read(&f).unwrap_or_default();
    let kept: Vec<u8> = bytes
        .into_iter()
        .filter(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r'))
        .collect();
    String::from_utf8_lossy(&kept).into_owned()
}

/// `json.dumps(d)` of string fields.
fn dumps(pairs: &[(&str, Value)]) -> String {
    let v = Value::Object(
        pairs
            .iter()
            .map(|(k, v)| (Text::from(*k), v.clone()))
            .collect(),
    );
    json::to_string(&v, &Options::default()).unwrap_or_default()
}

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

fn run_with(
    args: &[OsString],
    form: Form,
    mut settings: Settings,
    port: u16,
    decode: Decode,
) -> Exit {
    settings.port = Some(port);
    let would_refuse = passport::current(args).would_refuse();
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let exit = answer(&mut client, &mut closing, &settings.home, form, decode);
    closing.flush(&mut client, exit);
    exit
}

/// `$(…)`: NULs and trailing newlines dropped.
fn held(r: &Response) -> Vec<u8> {
    let mut b = without_nuls(&r.body);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    b
}

/// Bytes on stdout; the exit, or what a broken stdout ends with.
fn print_bytes(bytes: &[u8], exit: Exit) -> Exit {
    match out::write_flushed(&mut std::io::stdout().lock(), bytes) {
        Ok(()) => exit,
        Err(e) => out::after_stdout_error(&e),
    }
}

fn answer(
    client: &mut Client,
    closing: &mut Closing,
    home: &Path,
    form: Form,
    decode: Decode,
) -> Exit {
    let cli = cli_name!();
    let passport = client.passport().map(|p| p.as_str().to_string());
    let sub_word = match &form {
        Form::PairStart(_) => "pair-start",
        Form::PairToken { step, .. } => step.as_str(),
        Form::Confirm { .. } => "confirm",
        Form::Outbox => "outbox",
        _ => "",
    }
    .to_string();
    if passport.is_some() {
        if let Some(why) = form.owner_only() {
            out::stderr_line(&format!(
                "Error: {cli} fed {sub_word} is the server owner's: {why}."
            ));
            return lines(&OWNER_ONLY, Exit::Refused);
        }
    }
    let owner = if passport.is_none() {
        owner_token(home)
    } else {
        String::new()
    };
    let tok_file = home.join("daemon.token");
    let no_owner = |verb: &str| {
        out::stderr_line(&format!(
            "{cli} fed {verb}: no owner token (need {})",
            tok_file.display()
        ));
        Exit::Error
    };
    match form {
        Form::Usage => lines(&USAGE, Exit::Usage),
        Form::Send { to, text } => {
            if to.is_empty() || text.is_empty() {
                out::stderr_line(&format!(
                    "usage: {cli} fed send <peer>::<workspace>::<agent> \"message\""
                ));
                return Exit::Usage;
            }
            let token = passport.clone().unwrap_or_else(|| owner.clone());
            if token.is_empty() {
                return no_owner("send");
            }
            let body = dumps(&[
                ("to", s(&to)),
                ("text", s(&text)),
                ("from_workspace", s(&client.project.clone())),
            ]);
            let how = own(true, Some(&token), SEND_MAX_TIME, true);
            let mut shown = match client.own_curl(SEND, how, body.as_bytes()) {
                Ok(r) => {
                    closing.answer(&r, passport.is_some());
                    held(&r)
                }
                Err(_) => Vec::new(),
            };
            shown.push(b'\n');
            print_bytes(&shown, Exit::Ok)
        }
        Form::PairStart(fields) => {
            if owner.is_empty() {
                return no_owner("pair-start");
            }
            let [label, subdomain, url] = match fields {
                Ok(f) => f,
                Err((exit, line)) => return stop(exit, line),
            };
            let body = dumps(&[
                ("label", s(&label)),
                ("subdomain", s(&subdomain)),
                ("baseUrl", s(&url)),
            ]);
            pair_call(client, "pair-start", "pair/v2/init", &owner, &body)
        }
        Form::PairToken { step, token } => {
            if owner.is_empty() {
                return no_owner(&step);
            }
            let bytes = if token == "--token-stdin" || token == "-" {
                super::tunnel::first_line()
            } else {
                token.into_bytes()
            };
            if bytes.is_empty() {
                out::stderr_line(&format!("usage: {cli} fed {step} --token-stdin | <token from the other server's owner> (a token on the command line is visible to other users on this machine)"));
                return Exit::Usage;
            }
            let body = match pair::decode_stdin(&bytes, decode)
                .and_then(|t| pair::token_body(&step, &t))
            {
                Ok(b) => b,
                // Python's own exit, then `|| exit 2`.
                Err(shown) => {
                    let _ = shown.print();
                    return Exit::Usage;
                }
            };
            let route = match step.as_str() {
                "pair-join" => "pair/v2/respond",
                "pair-finish" => "pair/v2/reveal",
                _ => "pair/v2/verify",
            };
            pair_call(client, &step, route, &owner, &body)
        }
        Form::PairRequest(fields) => {
            let [label, subdomain, url, file] = match fields {
                Ok(f) => f,
                Err((exit, line)) => return stop(exit, line),
            };
            if file.is_empty() || !Path::new(&file).is_file() {
                out::stderr_line(&format!("usage: {cli} fed pair-request --pubkey <spki.pem> [--label L] [--subdomain S] [--url http://host:port]"));
                return Exit::Usage;
            }
            let pem = match read_text(&file) {
                Ok(t) => t,
                Err(shown) => return shown.print(),
            };
            let body = dumps(&[
                ("label", s(&label)),
                ("subdomain", s(&subdomain)),
                ("baseUrl", s(&url)),
                ("public_key_pem", s(&pem)),
            ]);
            let how = own(true, None, MAX_TIME, false);
            as_it_came(client.own_curl(PAIR_REQUEST, how, body.as_bytes()))
        }
        Form::Confirm { fp, sas } => {
            if fp.is_empty() || sas.is_empty() {
                out::stderr_line(&format!(
                    "usage: {cli} fed confirm <fingerprint> <sas-code>"
                ));
                return Exit::Usage;
            }
            if owner.is_empty() {
                return no_owner("confirm");
            }
            let body = dumps(&[("fingerprint", s(&fp)), ("sas", s(&sas))]);
            let how = own(true, Some(&owner), MAX_TIME, false);
            as_it_came(client.own_curl(CONFIRM, how, body.as_bytes()))
        }
        Form::Pubkey(want) => pubkey::run(home, &want).print(),
        Form::Switch(on) => {
            let body = format!("{{\"federationEnabled\": {on}}}");
            match super::tunnel::post_json_piped(client, closing, SETTINGS_UPDATE, body.as_bytes())
            {
                Ok((raw, call)) => super::pipefail(call, switch_reader(&raw, "").print()),
                Err(exit) => exit,
            }
        }
        Form::Status => {
            let left = super::request_piped_tcp(client, closing, SETTINGS_GET, &[]);
            super::pipefail(left.exit, switch_reader(&left.body, " status").print())
        }
        Form::Peers => {
            let left = super::request_piped(client, closing, PEERS, &[]);
            super::pipefail(left.exit, peers::peers_reader(&left.body).print())
        }
        Form::Outbox => {
            if owner.is_empty() {
                return no_owner("outbox");
            }
            let how = own(false, Some(&owner), MAX_TIME, false);
            let (raw, call) = match client.own_curl(OUTBOX, how, &[]) {
                Ok(r) => (r.body, Exit::Ok),
                Err(e) => (Vec::new(), Exit::from_curl(e.curl_code())),
            };
            super::pipefail(call, peers::outbox_reader(&raw).print())
        }
    }
}

/// How a call of the verb's own goes ([`Client::own_curl`]).
fn own(post: bool, credential: Option<&str>, max_time: Duration, note: bool) -> OwnCurl<'_> {
    OwnCurl {
        post,
        credential,
        max_time,
        note,
    }
}

/// A verb's own stop: its line, if any, then `exit`.
fn stop(exit: Exit, line: Option<String>) -> Exit {
    if let Some(l) = line {
        out::stderr_line(&l);
    }
    exit
}

/// `_k2_curl … ; echo`: the answer as it came, then a newline; a failed
/// call ends the command with curl's code (`set -e`).
fn as_it_came(r: Result<Response, crate::transport::Error>) -> Exit {
    match r {
        Ok(r) => {
            let mut b = r.body;
            b.push(b'\n');
            print_bytes(&b, Exit::Ok)
        }
        Err(e) => Exit::from_curl(e.curl_code()),
    }
}

/// `resp=$(_k2_curl …)` with the owner token, then the pairing reader.
fn pair_call(client: &mut Client, step: &str, route: &str, owner: &str, body: &str) -> Exit {
    let how = own(true, Some(owner), MAX_TIME, false);
    let endpoint = format!("/cli/federation/{route}");
    match client.own_curl(&endpoint, how, body.as_bytes()) {
        Ok(r) => pair::reader(step, &held(&r)).print(),
        Err(e) => Exit::from_curl(e.curl_code()),
    }
}

/// `open(path).read()` in the shell's Python: UTF-8, universal newlines;
/// a failure is its traceback (line 2) and exit 1.
fn read_text(path: &str) -> Result<String, Shown> {
    let raise = |e: String| super::raised(String::new(), String::new(), &super::Raise::at(2, e));
    let bytes = std::fs::read(path).map_err(|e| {
        let why = if e.kind() == std::io::ErrorKind::PermissionDenied {
            "PermissionError: [Errno 13] Permission denied"
        } else {
            "OSError: [Errno 5] Input/output error"
        };
        raise(format!("{why}: {}", py::repr(&s(path))))
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|e| {
        raise(format!(
            "UnicodeDecodeError: {}",
            pair::utf8_error(&bytes, &e)
        ))
    })?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// The switch's readers (`enable`, `disable`: `verb` is ""; `status`:
/// " status"): the line, or the error line and exit 1.
pub fn switch_reader(raw: &[u8], verb: &str) -> Shown {
    let route = if verb.is_empty() {
        SETTINGS_UPDATE
    } else {
        SETTINGS_GET
    };
    let d = std::str::from_utf8(raw)
        .ok()
        .and_then(|t| json::parse(t).ok());
    match d {
        Some(Value::Object(pairs)) => {
            let on = super::get(&pairs, "federationEnabled").is_some_and(py::truthy);
            Shown::out(format!(
                "Connect agents across servers: {} (this server)\n",
                if on { "on" } else { "off" }
            ))
        }
        _ => Shown::err(
            format!(
                "{} fed{verb}: unexpected response from {route}\n",
                cli_name!()
            ),
            Exit::Error,
        ),
    }
}

#[cfg(test)]
mod tests;
