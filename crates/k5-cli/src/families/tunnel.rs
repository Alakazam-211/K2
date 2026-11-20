//! `k5 tunnel`: this device's Connect tunnel, as the shell CLI's
//! `cmd_tunnel` answers it (`cli/src/usage.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]) and the two gates
//!    ([`crate::prelude`]). `tunnel` is an open tool: no session checks.
//! 2. The action, `${1:-status}`:
//!    - `start [--subdomain <s>]`, `stop`, `disable`: one `cli_post`
//!      ([`Client::post_query`]) piped into its reader;
//!    - `enable [--subdomain <s>] [--auto-start|--no-auto-start]`: the
//!      air-gap refusal, then the same;
//!    - `switch <subdomain> [--token-stdin | --token <t>]
//!      [--start|--no-start]`: the air-gap refusal, the words (exit 2 on a
//!      bad one), the token read from stdin, then one `cli_post_json`
//!      piped into its reader;
//!    - `release` and `forget`: without `--confirm`, what they would do
//!      (release reads `tunnel.json`'s subdomain), exit 1; with it, one
//!      `cli_post` whose answer, held in `$(…)`, goes to one reader;
//!    - `status [--json]` and `subdomains [--json]`: `raw=$(cli_request
//!      …)`, printed as it came with `--json`, else read (`subdomains`
//!      asks for the workspaces' names too, [`status`]);
//!    - `boot-status`: what strangers see (a `cli_request` piped into its
//!      reader), or `ghost`/`compat` (a `cli_post_json` to the settings),
//!      or the usage lines, exit 2;
//!    - `help` (also `--help` and `-h`, which the front answers): the
//!      help page, exit 0;
//!    - anything else: the usage lines, exit 1.
//! 3. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! Every reader's failure to read its input (an empty answer, a refused
//! connect, a reset: `cli_post`'s caller sees no line of its own) is the
//! reader's `Failed to parse … : <Python's JSONDecodeError>` and exit 1
//! ([`pyjson`]). Where the shell's Python raises on an answer of a shape
//! it doesn't expect, this prints a Python 3.12 traceback.
//!
//! Still the shell's: a value flag as the last word (`--subdomain`,
//! `--token`: the shell stops on `$2: unbound variable`, K2's D6 class), a
//! word that is not UTF-8, a command line with `-h` or `--help` that is
//! not a help page ([`crate::front`]), `switch --token-stdin` with a
//! terminal on stdin (the no-echo read is the vault lane's), a port that
//! is set but is no port curl takes, and a `python3` the binary can't
//! tell.

pub mod pyjson;
pub mod status;

use super::{curl_port, get, py_get, py_space, raised, Raise, Shown};
use crate::cards::Decode;
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::offline::Env;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::{Call, Channel, Client, Error, Response};
use k2_client::hosts::ZONE_SUFFIX;
use k2_client::{cli_name, product_name};
use std::ffi::OsString;
use std::io::{IsTerminal, Read};

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["tunnel"];
/// The binary's, the whole row.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// None: the row is native whole.
pub const VERBS: &[crate::route::Verb] = &[];

/// The routes.
pub const START: &str = "/cli/tunnel/start";
pub const STOP: &str = "/cli/tunnel/stop";
pub const DISABLE: &str = "/cli/tunnel/disable";
pub const ENABLE: &str = "/cli/tunnel/enable";
pub const SWITCH: &str = "/cli/tunnel/switch";
pub const RELEASE: &str = "/cli/tunnel/release";
pub const STATUS: &str = "/cli/tunnel/status";
pub const SUBDOMAINS: &str = "/cli/tunnel/subdomains";
pub const PROJECTS: &str = "/cli/companion/projects";
pub const SETTINGS_UPDATE: &str = "/cli/settings/update";

/// `cmd_tunnel`'s `*)` arm, on stderr, then exit 1.
pub const UNKNOWN: [&str; 5] = [
    concat!(
        "Usage: ",
        cli_name!(),
        " tunnel <start [--subdomain <s>]|stop|disable|enable [--subdomain <s>]|release --confirm|status [--json]|subdomains [--json]|boot-status [ghost|compat]>"
    ),
    "  disable  pause the tunnel, persisted on disk (survives restarts/reboots)",
    "  enable   undo disable and bring the tunnel up",
    "  release  permanently unpair THIS DEVICE from its subdomain (destructive)",
    concat!(
        "  See: ",
        cli_name!(),
        " tunnel --help for product boundary vs publish/dns"
    ),
];

/// `boot-status` with a word it doesn't take, on stderr, then exit 2.
pub const BOOT_USAGE: [&str; 3] = [
    concat!("Usage: ", cli_name!(), " tunnel boot-status [ghost|compat]"),
    "  ghost   strangers see only phase, protocol range and restart id",
    "  compat  strangers also see version + features (older phone apps need them; the 0.46.3 default)",
];

/// `switch`'s lines on stderr, each then exit 2.
pub const SWITCH_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " tunnel switch <subdomain> [--token-stdin | --token <tunnel token>] [--start|--no-start]"
);
pub const SWITCH_PREFIX: &str = concat!(cli_name!(), " tunnel switch: ");
pub const SWITCH_BOTH: &str = "--token-stdin or --token, not both";
pub const SWITCH_NO_TOKEN: &str = "--token-stdin: no token on stdin";

/// `forget` without `--confirm`, on stderr, then exit 1.
pub const FORGET_LINES: [&str; 3] = [
    concat!(
        cli_name!(),
        " tunnel forget deletes this device's tunnel config (tunnel.json, frpc.toml,"
    ),
    concat!(
        "the certificate) and tells ",
        product_name!(),
        " Connect NOTHING \u{2014} the subdomain and whichever"
    ),
    "device holds it now are untouched. Re-run with --confirm.",
];

/// `release` without `--confirm`, on stderr, then exit 1: its first line;
/// then [`release_sub_line`] or [`RELEASE_NO_SUB`]; then [`RELEASE_REST`].
pub const RELEASE_HEAD: &str = concat!(
    cli_name!(),
    " tunnel release unpairs THIS DEVICE from its subdomain:"
);
pub const RELEASE_NO_SUB: &str = "  - this device's tunnel identity is deleted";
pub const RELEASE_REST: [&str; 6] = [
    concat!(
        "  - ",
        product_name!(),
        " Connect is told only if this device's tunnel is connected (it holds the name)"
    ),
    "  - your ACCOUNT keeps the subdomain \u{2014} pair it on any device any time",
    concat!(
        "  - this is NOT a pause; for that use: ",
        cli_name!(),
        " tunnel disable"
    ),
    concat!(
        "  - stale config from another machine? use: ",
        cli_name!(),
        " tunnel forget --confirm"
    ),
    concat!(
        "  - moving to another subdomain? use: ",
        cli_name!(),
        " tunnel switch <subdomain> --token-stdin"
    ),
    concat!(
        "Re-run with --confirm to proceed: ",
        cli_name!(),
        " tunnel release --confirm"
    ),
];

/// `release`'s line for the subdomain `tunnel.json` names.
pub fn release_sub_line(sub: &str) -> String {
    format!("  - {sub}{ZONE_SUFFIX} stops routing here; this copy of the identity is refused")
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    /// `start`: the query pair for `--subdomain`, if any.
    Start(Option<String>),
    Stop,
    Disable,
    /// `enable`: its query pairs.
    Enable(String),
    Switch(Switch),
    /// `release`: `--confirm` given.
    Release(bool),
    /// `forget`: `--confirm` given.
    Forget(bool),
    /// `status`: `--json` first.
    Status(bool),
    /// `subdomains`: `--json` first.
    Subdomains(bool),
    Help,
    Boot(Boot),
    Unknown,
}

/// `switch`'s words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Switch {
    /// Ready to read the token (when `stdin`) and send.
    Ask {
        subdomain: String,
        token: String,
        stdin: bool,
        start: Option<bool>,
    },
    /// The line that stops it, exit 2.
    Stop(String),
}

/// `boot-status`'s words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boot {
    Show,
    Set(&'static str),
    Usage,
}

/// The words after `tunnel`: the form, or `None` when the shell answers
/// (a value flag as the last word, `-h` or `--help`).
pub fn parse(words: &[&str]) -> Option<Form> {
    let (sub, rest) = match words.split_first() {
        None => ("status", &[][..]),
        Some((&"", rest)) => ("status", rest),
        Some((w, rest)) => (*w, rest),
    };
    let form = match sub {
        "start" => {
            let mut subdomain = None;
            let mut i = 0;
            while i < rest.len() {
                if rest[i] == "--subdomain" {
                    subdomain = Some(rest.get(i + 1)?.to_string());
                    i += 2;
                } else {
                    i += 1;
                }
            }
            Form::Start(subdomain.filter(|s| !s.is_empty()).map(|s| sub_pair(&s)))
        }
        "stop" => Form::Stop,
        "disable" => Form::Disable,
        "enable" => {
            let (mut subdomain, mut auto) = (String::new(), "1");
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    "--subdomain" => {
                        subdomain = rest.get(i + 1)?.to_string();
                        i += 2;
                    }
                    "--no-auto-start" => {
                        auto = "0";
                        i += 1;
                    }
                    "--auto-start" => {
                        auto = "1";
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            let mut extra = format!("auto_start={auto}");
            if !subdomain.is_empty() {
                extra.push('&');
                extra.push_str(&sub_pair(&subdomain));
            }
            Form::Enable(extra)
        }
        "switch" => Form::Switch(switch_words(rest)?),
        "release" => Form::Release(rest.contains(&"--confirm")),
        "forget" => Form::Forget(rest.contains(&"--confirm")),
        "status" => Form::Status(rest.first() == Some(&"--json")),
        "subdomains" => Form::Subdomains(rest.first() == Some(&"--json")),
        "help" => Form::Help,
        "--help" | "-h" => return None,
        "boot-status" => Form::Boot(match rest.first().copied().unwrap_or("") {
            "" => Boot::Show,
            "ghost" => Boot::Set("ghost"),
            "compat" => Boot::Set("compat"),
            _ => Boot::Usage,
        }),
        _ => Form::Unknown,
    };
    Some(form)
}

fn switch_words(rest: &[&str]) -> Option<Switch> {
    let (mut subdomain, mut token, mut stdin, mut start) = ("", "", false, None);
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "--token" => {
                token = rest.get(i + 1)?;
                i += 2;
            }
            "--token-stdin" => {
                stdin = true;
                i += 1;
            }
            "--start" => {
                start = Some(true);
                i += 1;
            }
            "--no-start" => {
                start = Some(false);
                i += 1;
            }
            w if w.starts_with('-') => {
                return Some(Switch::Stop(format!("{SWITCH_PREFIX}unknown flag {w}")))
            }
            w => {
                subdomain = w;
                i += 1;
            }
        }
    }
    if subdomain.is_empty() {
        return Some(Switch::Stop(SWITCH_USAGE.into()));
    }
    if stdin && !token.is_empty() {
        return Some(Switch::Stop(format!("{SWITCH_PREFIX}{SWITCH_BOTH}")));
    }
    Some(Switch::Ask {
        subdomain: subdomain.into(),
        token: token.into(),
        stdin,
        start,
    })
}

/// `subdomain=$(urlencode "$s")`.
fn sub_pair(s: &str) -> String {
    format!("subdomain={}", crate::transport::encode::quote(s))
}

/// `k5 tunnel ...`: `args` is the whole command line after `argv[0]`.
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
    // The prelude refuses `tunnel enable` under air-gap before its gates
    // (`cli/src/cli-tools-policy.sh`); `switch` is refused in `cmd_tunnel`,
    // after them.
    if words.first() == Some(&"enable") {
        if let Some(exit) = airgap_refusal()? {
            return Some(exit);
        }
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
    if words.first() == Some(&"switch") {
        if let Some(exit) = airgap_refusal()? {
            return Some(exit);
        }
    }
    let form = parse(&words)?;
    // The no-echo read is not here yet: a terminal on stdin is the shell's,
    // and so is a locale the binary can't read Python's stdin by.
    let mut decode = Decode::Strict;
    if let Form::Switch(Switch::Ask { stdin: true, .. }) = form {
        if std::io::stdin().is_terminal() {
            return None;
        }
        decode = crate::cards::decode(&Env::current())?;
    }
    Some(run_with(args, form, settings, port, decode))
}

/// `_k2_refuse_if_airgap`: its line, then exit 1, when air-gap is on;
/// `Some(None)` when it is off. `None`: the shell's (no `HOME`).
fn airgap_refusal() -> Option<Option<Exit>> {
    let (state, file) = crate::airgap::state(&Env::current())?;
    Some(crate::airgap::refusal(state, &file).map(|line| {
        let mut bytes = line;
        bytes.push(b'\n');
        let _ = out::write_flushed(&mut std::io::stderr().lock(), &bytes);
        Exit::Error
    }))
}

/// Lines on stderr, then `exit`.
fn lines(list: &[&str], exit: Exit) -> Exit {
    for l in list {
        out::stderr_line(l);
    }
    exit
}

fn run_with(
    args: &[OsString],
    form: Form,
    mut settings: Settings,
    port: u16,
    decode: Decode,
) -> Exit {
    match &form {
        Form::Unknown => return lines(&UNKNOWN, Exit::Error),
        Form::Boot(Boot::Usage) => return lines(&BOOT_USAGE, Exit::Usage),
        Form::Forget(false) => return lines(&FORGET_LINES, Exit::Error),
        Form::Release(false) => {
            out::stderr_line(RELEASE_HEAD);
            match tunnel_json_subdomain(&settings.home.join("tunnel.json")) {
                Some(sub) => out::stderr_line(&release_sub_line(&sub)),
                None => out::stderr_line(RELEASE_NO_SUB),
            }
            return lines(&RELEASE_REST, Exit::Error);
        }
        Form::Switch(Switch::Stop(line)) => return lines(&[line.as_str()], Exit::Usage),
        Form::Help => {
            let page = crate::text::help("tunnel").unwrap_or_default();
            return match out::stdout(page) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            };
        }
        _ => {}
    }
    // The token, read before anything is sent.
    let body = match &form {
        Form::Switch(Switch::Ask {
            subdomain,
            token,
            stdin,
            start,
        }) => {
            let token = if *stdin {
                match read_token(decode) {
                    Ok(None) => {
                        out::stderr_line(&format!("{SWITCH_PREFIX}{SWITCH_NO_TOKEN}"));
                        return Exit::Usage;
                    }
                    Ok(Some(t)) => t,
                    Err(shown) => return shown.print(),
                }
            } else {
                Text::from(token.as_str())
            };
            Some(switch_body(subdomain, &token, *start))
        }
        _ => None,
    };
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
    let exit = match form {
        Form::Start(pair) => {
            let pairs: Vec<&str> = pair.iter().map(String::as_str).collect();
            posted(&mut client, &mut closing, START, &pairs, start_reader)
        }
        Form::Stop => posted(&mut client, &mut closing, STOP, &[], stop_reader),
        Form::Disable => posted(&mut client, &mut closing, DISABLE, &[], disable_reader),
        Form::Enable(extra) => posted(
            &mut client,
            &mut closing,
            ENABLE,
            &[extra.as_str()],
            enable_reader,
        ),
        Form::Switch(_) => {
            let body = body.unwrap_or_default();
            match post_json_piped(&mut client, &mut closing, SWITCH, body.as_bytes()) {
                Ok(left) => pipefail(left.1, switch_reader(&left.0).print()),
                Err(exit) => exit,
            }
        }
        Form::Release(_) => released(&mut client, &mut closing, &[]),
        Form::Forget(_) => released(&mut client, &mut closing, &["mode=forget"]),
        Form::Status(json) => status::status(&mut client, &mut closing, json),
        Form::Subdomains(json) => status::subdomains(&mut client, &mut closing, json),
        Form::Boot(Boot::Show) => {
            let left = super::request_piped_tcp(&mut client, &mut closing, STATUS, &[]);
            pipefail(left.exit, boot_show_reader(&left.body).print())
        }
        Form::Boot(Boot::Set(mode)) => {
            let body = format!("{{\"bootStatusPublic\":\"{mode}\"}}");
            match post_json_piped(&mut client, &mut closing, SETTINGS_UPDATE, body.as_bytes()) {
                Ok((raw, call)) => {
                    let reader = boot_set_reader(&raw, mode).print();
                    // `… || exit 1`.
                    if pipefail(call, reader) == Exit::Ok {
                        Exit::Ok
                    } else {
                        Exit::Error
                    }
                }
                Err(exit) => exit,
            }
        }
        Form::Help | Form::Unknown | Form::Boot(Boot::Usage) => Exit::Error,
    };
    closing.flush(&mut client, exit);
    exit
}

/// `set -o pipefail`.
fn pipefail(call: Exit, reader: Exit) -> Exit {
    super::pipefail(call, reader)
}

/// `cli_post <endpoint> <pairs> | python3 -c …`: `cli_post` prints the
/// body whatever happened (nothing when the call failed), and the reader
/// reads it. Its code is lost to the reader's when the reader fails.
fn posted(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    pairs: &[&str],
    reader: fn(&[u8]) -> Shown,
) -> Exit {
    let (body, call) = query_post(client, closing, endpoint, pairs);
    pipefail(call, reader(&body).print())
}

/// One `cli_post`: what it prints (NULs dropped) and its code.
fn query_post(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    pairs: &[&str],
) -> (Vec<u8>, Exit) {
    let passport = client.passport().is_some();
    match client.post_query(endpoint, pairs) {
        Ok(r) => {
            closing.answer(&r, passport);
            (super::without_nuls(&r.body), Exit::Ok)
        }
        Err(e) => (Vec::new(), Exit::from_curl(e.curl_code())),
    }
}

/// `_k2_tunnel_release_print "$(cli_post /cli/tunnel/release confirm=1…)"`:
/// the body as `$(…)` holds it, whatever the call's code.
fn released(client: &mut Client, closing: &mut Closing, extra: &[&str]) -> Exit {
    let mut pairs = vec!["confirm=1"];
    pairs.extend_from_slice(extra);
    let joined = pairs.join("&");
    let (mut body, _) = query_post(client, closing, RELEASE, &[joined.as_str()]);
    while body.last() == Some(&b'\n') {
        body.pop();
    }
    release_reader(&body).print()
}

/// `cli_post_json <endpoint> <body>` on the left of a pipe, over TCP:
/// what the reader reads and the call's code. No answer: its line, then
/// death by SIGPIPE ([`super::end_in_pipe`]).
fn post_json_piped(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    body: &[u8],
) -> Result<(Vec<u8>, Exit), Exit> {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, endpoint, Call::PostJson(body)) {
        Ok(r) => {
            closing.answer(&r, passport);
            Ok((super::without_nuls(&r.body), Exit::Ok))
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            super::end_in_pipe()
        }
        Err(e) => Ok((Vec::new(), Exit::from_curl(e.curl_code()))),
    }
}

/// `_cli_read_secret_stdin` when stdin is not a terminal (`IFS= read -r`:
/// the first line, without its newline; NULs dropped), then the Python
/// that reads it (`sys.stdin.read()`): bytes that are not UTF-8 stop the
/// command in a UTF-8 locale (exit 1), and are kept as stand-ins
/// (`surrogateescape`) in C or POSIX. `None`: no token.
fn read_token(decode: Decode) -> Result<Option<Text>, Shown> {
    let mut bytes = Vec::new();
    let mut stdin = std::io::stdin().lock();
    let mut one = [0u8; 1];
    loop {
        match stdin.read(&mut one) {
            Ok(0) => break,
            Ok(_) if one[0] == b'\n' => break,
            Ok(_) if one[0] == 0 => {}
            Ok(_) => bytes.push(one[0]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    match String::from_utf8(bytes) {
        Ok(t) => Ok(Some(Text::from(t))),
        Err(e) if decode == Decode::Strict => {
            let at = e.utf8_error().valid_up_to();
            let byte = e.as_bytes()[at];
            Err(raised(
                String::new(),
                String::new(),
                &Raise::at(
                    4,
                    format!("UnicodeDecodeError: 'utf-8' codec can't decode byte 0x{byte:02x} in position {at}: invalid start byte"),
                ),
            ))
        }
        Err(e) => Ok(Some(escaped(e.as_bytes()))),
    }
}

/// Bytes read with `surrogateescape`: each byte that is not part of UTF-8
/// becomes the lone surrogate U+DC00 plus the byte.
fn escaped(bytes: &[u8]) -> Text {
    let mut lit = String::from("\"");
    let mut rest = bytes;
    while !rest.is_empty() {
        let (good, bad) = match std::str::from_utf8(rest) {
            Ok(all) => (all, None),
            Err(e) => {
                let n = e.valid_up_to();
                let bad = e.error_len().unwrap_or(rest.len() - n);
                (
                    std::str::from_utf8(&rest[..n]).unwrap_or_default(),
                    Some(&rest[n..n + bad]),
                )
            }
        };
        let quoted =
            json::to_string(&Value::Str(Text::from(good)), &Options::default()).unwrap_or_default();
        lit.push_str(&quoted[1..quoted.len() - 1]);
        let used = good.len() + bad.map_or(0, <[u8]>::len);
        for b in bad.unwrap_or_default() {
            lit.push_str(&format!("\\udc{b:02x}"));
        }
        rest = &rest[used..];
    }
    lit.push('"');
    match json::parse(&lit) {
        Ok(Value::Str(t)) => t,
        _ => Text::from(String::from_utf8_lossy(bytes).into_owned()),
    }
}

/// The body `switch` sends: `json.dumps(b)`.
pub fn switch_body(subdomain: &str, token: &Text, start: Option<bool>) -> String {
    let mut pairs = vec![(Text::from("subdomain"), Value::Str(Text::from(subdomain)))];
    if !token.code_points().is_empty() {
        pairs.push((Text::from("token"), Value::Str(token.clone())));
    }
    if let Some(start) = start {
        pairs.push((Text::from("start"), Value::Bool(start)));
    }
    json::to_string(&Value::Object(pairs), &Options::default()).unwrap_or_default()
}

/// The subdomain `release`'s Python reads from `tunnel.json`: `None` for
/// its `""` (no file, not JSON, not an object, no subdomain, one that is
/// not a string or is only spaces, one Python can't print).
pub fn tunnel_json_subdomain(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    // Text mode reads `\r\n` and `\r` as `\n`.
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let Ok(Value::Object(pairs)) = json::parse(&text) else {
        return None;
    };
    let v = get(&pairs, "subdomain")?;
    if !py::truthy(v) {
        return None;
    }
    let Value::Str(t) = v else { return None };
    let s = t.as_str()?;
    let s = s.trim_matches(py_space);
    (!s.is_empty()).then(|| s.to_string())
}

/// `json.loads(sys.stdin.read())`: the value, or `str(e)`.
pub fn load(raw: &[u8]) -> Result<Value, String> {
    let text = String::from_utf8_lossy(raw);
    json::parse(&text).map_err(|e| pyjson::decode_error(&text).unwrap_or_else(|| e.to_string()))
}

/// What a reader prints.
#[derive(Default)]
pub struct Out {
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn out(&mut self, line: &str) {
        self.stdout.push_str(line);
        self.stdout.push('\n');
    }
    pub fn err(&mut self, line: &str) {
        self.stderr.push_str(line);
        self.stderr.push('\n');
    }
    pub fn done(self, r: Result<Exit, Raise>) -> Shown {
        match r {
            Ok(exit) => Shown {
                stdout: self.stdout,
                stderr: self.stderr,
                exit,
            },
            Err(r) => raised(self.stdout, self.stderr, &r),
        }
    }
}

/// `print(f"{prefix}{e}", file=sys.stderr); sys.exit(1)` for a text the
/// reader can't read.
fn unread(prefix: &str, e: &str) -> Shown {
    Shown::err(format!("{prefix}{e}\n"), Exit::Error)
}

/// `"<text>" + v`: v must be a string.
pub fn plus(text: &str, v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(format!("{text}{}", py::str_of(v))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

/// `bool(x)` for a `.get` that may have found nothing.
pub fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(py::truthy)
}

/// `d.get(key) or default`, as a value.
pub fn or(v: Option<&Value>, default: &str) -> Value {
    match v {
        Some(x) if py::truthy(x) => x.clone(),
        _ => Value::Str(Text::from(default)),
    }
}

/// `x is True` / `x is False`.
pub fn is(v: Option<&Value>, b: bool) -> bool {
    v == Some(&Value::Bool(b))
}

/// `start`'s reader.
pub fn start_reader(raw: &[u8]) -> Shown {
    let d = match load(raw) {
        Ok(d) => d,
        Err(e) => return unread("Failed to parse daemon response: ", &e),
    };
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let err = py_get(&d, "error", 7)?;
        if truthy(err) {
            o.err(&format!(
                "Failed to start tunnel: {}",
                py::str_of(err.unwrap())
            ));
            return Ok(Exit::Error);
        }
        let url = or(py_get(&d, "public_url", 10)?, "");
        if let Some(rep) = py_get(&d, "repaired", 13)?.filter(|v| py::truthy(v)) {
            o.out(&plus("Tunnel was running but not serving: ", rep, 14)?);
            o.out("Re-rendered the frpc config and restarted frpc.");
        }
        if py::truthy(&url) {
            o.out(&format!("Tunnel started. Public URL: {}", py::str_of(&url)));
        } else {
            o.out("Tunnel started. (subdomain will be assigned by the server)");
        }
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// `stop`'s reader.
pub fn stop_reader(raw: &[u8]) -> Shown {
    let d = match load(raw) {
        Ok(d) => d,
        Err(e) => return unread("Failed to parse daemon response: ", &e),
    };
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        if truthy(py_get(&d, "ok", 7)?) {
            o.out("Tunnel stopped.");
            return Ok(Exit::Ok);
        }
        let err = or(py_get(&d, "error", 10)?, "unknown error");
        o.err(&format!("Failed to stop tunnel: {}", py::str_of(&err)));
        Ok(Exit::Error)
    })();
    o.done(r)
}

/// `disable`'s reader.
pub fn disable_reader(raw: &[u8]) -> Shown {
    let d = match load(raw) {
        Ok(d) => d,
        Err(e) => return unread("Failed to parse daemon response: ", &e),
    };
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let err = py_get(&d, "error", 7)?;
        if truthy(err) {
            o.err(&format!(
                "Failed to disable tunnel: {}",
                py::str_of(err.unwrap())
            ));
            return Ok(Exit::Error);
        }
        o.out("Tunnel disabled (persisted \u{2014} survives daemon restarts and reboots).");
        o.out(&format!("Re-enable with: {} tunnel enable", cli_name!()));
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// `enable`'s reader.
pub fn enable_reader(raw: &[u8]) -> Shown {
    let d = match load(raw) {
        Ok(d) => d,
        Err(e) => return unread("Failed to parse daemon response: ", &e),
    };
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let err = py_get(&d, "error", 7)?;
        if truthy(err) {
            o.err(&format!(
                "Failed to enable tunnel: {}",
                py::str_of(err.unwrap())
            ));
            return Ok(Exit::Error);
        }
        if truthy(py_get(&d, "running", 10)?) {
            let url = or(
                py_get(&d, "public_url", 11)?,
                "(subdomain assigned by the server)",
            );
            o.out(&format!(
                "Tunnel enabled and running. Public URL: {}",
                py::str_of(&url)
            ));
        } else {
            o.out("Tunnel enabled (persisted). Not running yet \u{2014} choose an address in");
            o.out(&format!(
                "Settings \u{2192} Network \u{2192} Tunnel (or run: {} tunnel start).",
                cli_name!()
            ));
        }
        let auto = py_get(&d, "autoStart", 16)?;
        if is(auto, true) {
            o.out("Auto-start: on (comes back by itself after a daemon restart or reboot).");
        } else if is(auto, false) {
            o.out(&format!(
                "Auto-start: off (after a restart, run {} tunnel enable again).",
                cli_name!()
            ));
        }
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// `s.get(key, default)` on a step Python must find a dict.
fn step_get(s: &Value, key: &str, default: &str, line: u32) -> Result<String, Raise> {
    Ok(match py_get(s, key, line)? {
        Some(v) => py::str_of(v),
        None => default.to_string(),
    })
}

/// `switch`'s reader.
pub fn switch_reader(raw: &[u8]) -> Shown {
    let d = match load(raw) {
        Ok(d) => d,
        Err(e) => return unread("Failed to parse daemon response: ", &e),
    };
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let err = py_get(&d, "error", 7)?;
        let Value::Object(pairs) = &d else {
            unreachable!("d.get passed")
        };
        if truthy(err) && get(pairs, "steps").is_none() {
            o.err(&format!("Switch refused: {}", py::str_of(err.unwrap())));
            return Ok(Exit::Error);
        }
        let steps = or(get(pairs, "steps"), "");
        let steps = if py::truthy(&steps) {
            super::py_iter(&steps, 10)?
        } else {
            Vec::new()
        };
        for s in &steps {
            let mark = if truthy(py_get(s, "ok", 11)?) {
                "ok "
            } else {
                "ERR"
            };
            let step = step_get(s, "step", "", 12)?;
            let detail = step_get(s, "detail", "", 13)?;
            o.out(&format!("  [{mark}] {step}: {detail}"));
        }
        if truthy(get(pairs, "ok")) {
            let sub = or(get(pairs, "subdomain"), "");
            o.out(&format!("Switched to {}{ZONE_SUFFIX}.", py::str_of(&sub)));
            return Ok(Exit::Ok);
        }
        let err = or(get(pairs, "error"), "unknown error");
        o.err(&format!("Switch failed: {}", py::str_of(&err)));
        if truthy(get(pairs, "rolledBack")) {
            o.err("Rolled back: the previous subdomain is still in place.");
        }
        Ok(Exit::Error)
    })();
    o.done(r)
}

/// `_k2_tunnel_release_print`'s reader (release and forget).
pub fn release_reader(raw: &[u8]) -> Shown {
    let d = match load(raw) {
        Ok(d) => d,
        Err(e) => return unread("Failed to parse daemon response: ", &e),
    };
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let err = py_get(&d, "error", 7)?;
        if truthy(err) {
            o.err(&format!(
                "Failed to release tunnel identity: {}",
                py::str_of(err.unwrap())
            ));
            return Ok(Exit::Error);
        }
        let Value::Object(pairs) = &d else {
            unreachable!("d.get passed")
        };
        let sub = match or(get(pairs, "subdomain"), "") {
            Value::Str(t) => py::str_of(&Value::Str(t))
                .trim_matches(py_space)
                .to_string(),
            other => return Err(Raise::no_attribute(10, &other, "strip")),
        };
        let label = if sub.is_empty() {
            "its subdomain".to_string()
        } else {
            format!("{sub}{ZONE_SUFFIX}")
        };
        if truthy(get(pairs, "alreadyReleased")) {
            o.out(&format!("This device was already released from {label}."));
        } else if truthy(get(pairs, "localOnly")) {
            o.out(&format!(
                "Forgotten on this device: {label} no longer routes here."
            ));
        } else {
            o.out(&format!("Released: this device is unpaired from {label}."));
        }
        if truthy(get(pairs, "upstreamReported")) {
            o.out(&format!(
                "{} Connect recorded the release.",
                product_name!()
            ));
        } else if truthy(get(pairs, "queued")) {
            o.out(&format!("{} Connect was unreachable; the release is queued and replays on the next daemon boot.", product_name!()));
        }
        if let Some(note) = get(pairs, "note").filter(|n| py::truthy(n)) {
            o.out(&format!("Note: {}", py::str_of(note)));
        }
        o.out("Your account still owns the subdomain; pair it on any device any time.");
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// `boot-status`'s reader of the tunnel's status.
pub fn boot_show_reader(raw: &[u8]) -> Shown {
    let d = match load(raw) {
        Ok(d) => d,
        Err(e) => return unread("Failed to parse tunnel status: ", &e),
    };
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let err = py_get(&d, "error", 7)?;
        if truthy(err) {
            o.err(&format!("Error: {}", py::str_of(err.unwrap())));
            return Ok(Exit::Error);
        }
        let Value::Object(pairs) = &d else {
            unreachable!("d.get passed")
        };
        let bs = get(pairs, "bootStatus").filter(|v| py::truthy(v));
        let Some(bs) = bs else {
            o.out("This daemon predates boot-status views (0.46.3): every caller gets the full /boot-status.");
            return Ok(Exit::Ok);
        };
        let note = if truthy(get(pairs, "bootStatusExplicit")) {
            ""
        } else {
            " (the default)"
        };
        o.out(&format!("Boot status: {}{note}", py::str_of(bs)));
        if *bs == Value::Str(Text::from("ghost")) {
            o.out("Strangers see: phase, protocol range, restart id. /ping says k2-daemon (no version).");
        } else {
            o.out("Strangers see: phase, protocol range, restart id, version, features, serverId, publicHost, airgap.enabled.");
        }
        o.out("Signed-in callers and this computer always get the full status.");
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// `boot-status ghost|compat`'s reader: `raw` is all it read.
pub fn boot_set_reader(raw: &[u8], mode: &str) -> Shown {
    let text = String::from_utf8_lossy(raw).into_owned();
    let d = match json::parse(&text) {
        Ok(d) => d,
        Err(_) => {
            let head: String = text.chars().take(200).collect();
            return Shown::err(
                format!(
                    "Error: daemon returned an unparseable response: {}\n",
                    py::repr(&Value::Str(Text::from(head)))
                ),
                Exit::Error,
            );
        }
    };
    let mut o = Out::default();
    let r = (|| -> Result<Exit, Raise> {
        let has_error = match &d {
            Value::Object(pairs) => get(pairs, "error").is_some(),
            Value::Array(items) => items.contains(&Value::Str(Text::from("error"))),
            Value::Str(t) => py::str_of(&Value::Str(t.clone())).contains("error"),
            other => {
                return Err(Raise::at(
                    8,
                    format!(
                        "TypeError: argument of type '{}' is not iterable",
                        py::type_name(other)
                    ),
                ))
            }
        };
        if has_error {
            let Value::Object(pairs) = &d else {
                let index = if matches!(d, Value::Array(_)) {
                    "list indices must be integers or slices, not str"
                } else {
                    "string indices must be integers, not 'str'"
                };
                return Err(Raise::at(9, format!("TypeError: {index}")));
            };
            let err = get(pairs, "error").cloned().unwrap_or(Value::Null);
            o.err(&format!("Error: {}", py::str_of(&err)));
            return Ok(Exit::Error);
        }
        let set = or(py_get(&d, "bootStatusPublic", 10)?, mode);
        o.out(&format!(
            "Boot status: {} (live now, no restart)",
            py::str_of(&set)
        ));
        Ok(Exit::Ok)
    })();
    o.done(r)
}

/// A response's body as `$(…)` holds it: NULs and trailing newlines gone.
pub fn held(r: &Response) -> Vec<u8> {
    let mut b = super::without_nuls(&r.body);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    b
}

#[cfg(test)]
mod tests;
