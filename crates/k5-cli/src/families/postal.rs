//! `k5 postal ...` (Beta): an agent's friends among agents on other
//! machines, as the shell CLI's `cmd_postal` answers it
//! (`cli/src/postal.sh`, `_postal_send_teach` in `cli/src/remote-target.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the gates ([`crate::prelude`]). The virtual
//!    desktop's guard always; the connection gate not for `postal` alone,
//!    `help`, `send`, `route` and `adopt` (`_skip_conn_gate`).
//! 2. The credential: `postal` is an open tool, so in a session with the
//!    owner token on disk the shell sends the owner token, a passport or
//!    not (COMPAT-58), and so does this ([`super::feedback::write::open_tool`]).
//! 3. No verb or `help`: the usage ([`usage`]). Then every word after the
//!    verb ([`parse`]): `--workspace`, `--why`, `--code` and `--friend`
//!    take a value; `--json`, `--no-wake`, `--clear` and `--token-stdin`
//!    are flags; any other dash word is `unknown flag`, exit 2; the rest
//!    are the verb's words.
//! 4. The verb: a `cli_request` or `cli_post_json` (its body as
//!    `_postal_body` writes it) on the left of a pipe into
//!    `_postal_print` ([`print`]), which reads the answer from stdin. No
//!    answer: the transport's line, then death by SIGPIPE
//!    ([`super::end_in_pipe`]). The command ends with the reader's code
//!    when it is not 0, else the call's (`send`: the reader's).
//!
//! Handed to the shell, before anything is printed or sent: a word that is
//! not UTF-8, `-h` or `--help` anywhere (the front's), `send` or `route`
//! with no port (the shell sends them to `http://HOST:`, a bash bug), a
//! port curl won't take, and a `python3` the binary can't tell.

use super::feedback::write::open_tool;
use super::{curl_port, end_in_pipe, get, piped_body, py_d, Piped, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{Auth, Call, Channel, Client, Error};
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};
use std::ffi::OsString;
use std::io::BufRead as _;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["postal"];
/// Every form is the family's; a line with `-h` or `--help` is the
/// front's (a help page, or the shell's).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `_postal_usage`, with the command's and the product's names filled in.
const USAGE: &str = "Usage: {cli} postal <command> [--workspace <ws>] [--json]

Postal (Beta): friends for an agent here among agents on other machines.
Addresses look like handle::label.postal.bot: the handle is the agent, the
label is its server's name on Postal (ask your person which label is
yours). One address per workspace; sidecars and subagents share it. A
friendship needs both sides: a person on each machine accepts, typing
the 6-digit code shown on the other machine.

Getting started (an agent; each step is a card your person answers):
  1. {cli} postal enable [--why \"\u{2026}\"]     ask the server's owner to turn Postal on
                                      (off by default; until then the other
                                      commands answer postal_off, exit 3, so
                                      wait for this answer before step 2)
  2. {cli} postal claim <address> [--why \"\u{2026}\"]
                                      ask for this workspace's Postal address
                                      (an Owner or Admin allows it)
  3. {cli} postal invite <address> --why \"what you want to do together\"
                                      ask to be friends with that agent (an Owner
                                      or Admin allows it)
  Each prints \"Asked: \u{2026}\" and exits 2: the card is on your person's Thread,
  and the answer comes back to you as a [permission:] turn. Asking again
  while it waits finds the same card. An \"Ask with: {cli} permission request
  postal.\u{2026}\" line under it names that same card: you don't need to run it.

Every day:
  {cli} postal status                   your address; counts of friends and requests
  {cli} postal friends                  friends and requests (in, out, history)
  {cli} postal accept <request id>      ask your person to accept a request: puts
                                      its card up again (you can't accept one; the
                                      Owner does, or an Admin if the Owner allows it)
  {cli} postal reject <request id>      turn an incoming request down yourself
  {cli} postal remove <address>         end a friendship or withdraw a request
  {cli} postal block <address>          refuse that address from now on
  {cli} msg <address> \"text\" [--no-wake] [--json]
                                      Postal mail to a friend, from this
                                      workspace's address (exit 0 queued, held or
                                      delivered; 3 not a friend or the text holds a
                                      vault secret's value)
  {cli} postal route [--friend <address>] terminal|inbox|inbox+pointer|thread:<person>\u{2026} [--no-wake]
                                      where Postal mail lands (default: terminal,
                                      waking you); you may only make yours quieter
  {cli} postal route --clear [--friend <address>]   back to the default

People (an Owner or Admin) do the same directly, or in the workspace's Postal tab:
  {cli} postal claim <address> --workspace <ws>
  {cli} postal code <request id>        this server's 6-digit code: read it to the
                                      other side
  {cli} postal accept <request id> --code <their code> [--workspace <ws>]
                                      accept, typing the code shown on the OTHER
                                      side's screen (this server checks it, then
                                      signs; a code that doesn't match fails the
                                      request on both sides)
  {cli} postal owner-link [--token-stdin]
                                      the Owner, once: link Postal approvals for
                                      this server (opens a page to approve; or hand
                                      over a {token} token approved elsewhere)
  {cli} postal enable | disable         the server's switch (the Owner)
  {cli} postal admins-may-accept on|off let Admins answer incoming requests too (the
                                      Owner); claim and invite are already an
                                      Owner's or Admin's
  {cli} postal settings                 show both

p5, Postal's own client on this machine (works while Postal is off):
  {cli} postal link-p5                  a card: the owner's Allow makes a key that can
                                      only message this workspace and hands it to p5

Not available yet:
  live delivery to this server        mail comes by the Postal network's hold
                                      (within about a minute) until the network
                                      routes this server's name to it
  {cli} postal adopt                    taking over p5's handles

Incoming requests never wake you: {product} puts a card on your person's Thread,
and its answer doesn't wake you either ({cli} postal friends shows it).
Postal mail arrives as [postal <address> \u{b7} p_<code>] with every line quoted
(long or busy: in your inbox with an [inbox:<id>] pointer); check one with
{cli} msg verify <code> --text-file f.
Exit codes: 0 done; 2 asked (stdout starts \"Asked:\") or a usage error (an
\"Error:\" or \"Usage:\" line on stderr); 3 refused (the lines under the error
say what to do); 4 not found; 1 anything else.
{cli} study postal says more.
";

/// The Postal Connect token's prefix, a wire value.
const TOKEN_PREFIX: &str = "k2c_pd_";

/// The usage text.
pub fn usage() -> String {
    USAGE
        .replace("{cli}", CLI_NAME)
        .replace("{product}", PRODUCT_NAME)
        .replace("{token}", TOKEN_PREFIX)
}

/// The words after the verb.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Words {
    pub workspace: String,
    pub why: String,
    pub code: String,
    pub friend: String,
    pub json: bool,
    pub nowake: bool,
    pub clear: bool,
    pub token_stdin: bool,
    pub args: Vec<String>,
}

/// The flag loop: the words, or the line that stops it (exit 2).
pub fn parse(rest: &[&str]) -> Result<Words, String> {
    let mut w = Words::default();
    let mut i = 0;
    while let Some(&a) = rest.get(i) {
        let value = |what: &str| -> Result<String, String> {
            rest.get(i + 1)
                .map(|v| (*v).to_string())
                .ok_or_else(|| format!("{CLI_NAME} postal: {a} needs {what}"))
        };
        match a {
            "--workspace" => w.workspace = value("a name")?,
            "--why" => w.why = value("the reason")?,
            "--code" => w.code = value("the 6 digits shown on the other side")?,
            "--friend" => w.friend = value("an address")?,
            "--json" | "--no-wake" | "--clear" | "--token-stdin" => {
                match a {
                    "--json" => w.json = true,
                    "--no-wake" => w.nowake = true,
                    "--clear" => w.clear = true,
                    _ => w.token_stdin = true,
                }
                i += 1;
                continue;
            }
            f if f.starts_with('-') => {
                return Err(format!(
                    "{CLI_NAME} postal: unknown flag '{f}' ({CLI_NAME} postal --help)"
                ))
            }
            other => {
                w.args.push(other.to_string());
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    Ok(w)
}

/// `_is_postal_address`: `handle::label.postal.bot`, letters in any case.
pub fn is_postal_address(a: &str) -> bool {
    let a = a.to_ascii_lowercase();
    let Some((handle, rest)) = a.split_once("::") else {
        return false;
    };
    let Some(label) = rest.strip_suffix(".postal.bot") else {
        return false;
    };
    let hb = handle.as_bytes();
    let lb = label.as_bytes();
    let alnum = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    !hb.is_empty()
        && alnum(&hb[0])
        && hb[1..].iter().all(|b| alnum(b) || *b == b'_' || *b == b'-')
        && (3..=63).contains(&lb.len())
        && alnum(&lb[0])
        && alnum(&lb[lb.len() - 1])
        && lb[1..lb.len() - 1].iter().all(|b| alnum(b) || *b == b'-')
}

/// `_postal_body k=v…`: a JSON object of the pairs whose value is not
/// empty, `__true` and `__false` as booleans, as `json.dumps` writes it.
pub fn body(pairs: &[(&str, &str)]) -> String {
    let mut out: Vec<(json::Text, Value)> = Vec::new();
    for (k, v) in pairs {
        if v.is_empty() {
            continue;
        }
        let v = match *v {
            "__true" => Value::Bool(true),
            "__false" => Value::Bool(false),
            other => super::s(other),
        };
        out.push((json::Text::from(*k), v));
    }
    json::to_string(&Value::Object(out), &Options::default()).unwrap_or_default()
}

/// What runs for the verb.
enum Call2 {
    Get(String, String),
    Post(String, String),
}

/// `k5 postal ...`: `args` is the whole command line after `argv[0]`.
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
    if !python::family_native(python::here()) {
        return None;
    }
    let sub = words.first().copied().unwrap_or("");
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let offline = matches!(sub, "" | "help" | "send" | "route" | "adopt");
    match prelude::gate(&settings) {
        Some(g @ Gate::VirtualDesktop) => return Some(g.stop()),
        Some(g @ Gate::CannotConnect) if !offline => return Some(g.stop()),
        _ => {}
    }
    let port = settings.port_text.as_deref().unwrap_or("");
    let asks = !matches!(sub, "" | "help" | "adopt");
    if asks && (curl_port(port).is_none() || settings.auth == Auth::None) {
        // No port, one curl won't take, or no credential: the shell's.
        return None;
    }
    if matches!(sub, "" | "help") {
        return Some(Shown::out(usage()).print());
    }
    Some(run_verb(args, sub, &words[1..], settings))
}

fn stop(line: &str, exit: Exit) -> Exit {
    out::stderr_line(line);
    exit
}

fn need(one: &str, sub: &str, what: &str) -> Result<(), Exit> {
    if one.is_empty() {
        Err(stop(
            &format!("Usage: {CLI_NAME} postal {sub} {what}  ({CLI_NAME} postal --help)"),
            Exit::Usage,
        ))
    } else {
        Ok(())
    }
}

const IDS: &str = "<request id>  ({cli} postal friends shows the ids)";

fn run_verb(args: &[OsString], sub: &str, rest: &[&str], mut settings: Settings) -> Exit {
    let w = match parse(rest) {
        Ok(w) => w,
        Err(l) => return stop(&l, Exit::Usage),
    };
    let ws = if w.workspace.is_empty() {
        settings.project.clone()
    } else {
        w.workspace.clone()
    };
    let one = w.args.first().cloned().unwrap_or_default();
    let ids = IDS.replace("{cli}", CLI_NAME);
    let wsq = format!("workspace={}", quote(&ws));
    let (call, view): (Call2, &str) = match sub {
        "status" | "friends" => (Call2::Get(format!("/cli/postal/{sub}"), wsq), sub),
        "claim" => {
            if let Err(e) = need(&one, sub, "<handle::label.postal.bot> [--why \"\u{2026}\"]") {
                return e;
            }
            let b = body(&[("workspace", &ws), ("address", &one), ("why", &w.why)]);
            (Call2::Post("/cli/postal/claim".into(), b), "claim")
        }
        "invite" => {
            if let Err(e) = need(
                &one,
                sub,
                "<handle::label.postal.bot> --why \"what you want to do together\"",
            ) {
                return e;
            }
            let b = body(&[("workspace", &ws), ("to", &one), ("why", &w.why)]);
            (Call2::Post("/cli/postal/invite".into(), b), "invite")
        }
        "accept" | "reject" => {
            if let Err(e) = need(&one, sub, &ids) {
                return e;
            }
            let b = body(&[
                ("workspace", &ws),
                ("id", &one),
                ("decision", sub),
                ("code", &w.code),
            ]);
            (Call2::Post("/cli/postal/decide".into(), b), "decide")
        }
        "code" => {
            if let Err(e) = need(&one, sub, &ids) {
                return e;
            }
            (
                Call2::Get(
                    "/cli/postal/code".into(),
                    format!("{wsq}&id={}", quote(&one)),
                ),
                "code",
            )
        }
        "route" => {
            let targets = w.args.join(" ");
            let b = if w.clear {
                body(&[("workspace", &ws), ("friend", &w.friend), ("clear", "__true")])
            } else {
                if targets.is_empty() {
                    return stop(
                        &format!("Usage: {CLI_NAME} postal route [--friend <address>] terminal|inbox|inbox+pointer|thread:<person>\u{2026} [--no-wake] | --clear"),
                        Exit::Usage,
                    );
                }
                let wake = if w.nowake { "__false" } else { "__true" };
                body(&[
                    ("workspace", &ws),
                    ("friend", &w.friend),
                    ("targets", &targets),
                    ("wake", wake),
                ])
            };
            (Call2::Post("/cli/postal/route".into(), b), "route")
        }
        "owner-link" => {
            let b = if w.token_stdin {
                // `IFS= read -r tok || true`: one line, its newline dropped.
                let mut line = String::new();
                let _ = std::io::stdin().lock().read_line(&mut line);
                let tok = line.strip_suffix('\n').unwrap_or(&line).to_string();
                if tok.is_empty() {
                    return stop(
                        &format!("{CLI_NAME} postal owner-link: no token on stdin"),
                        Exit::Usage,
                    );
                }
                body(&[("token", &tok)])
            } else {
                "{}".to_string()
            };
            (Call2::Post("/cli/postal/owner-link".into(), b), "owner-link")
        }
        "send" => {
            if let Err(e) = need(&one, sub, "<handle::label.postal.bot> \"text\"") {
                return e;
            }
            if !is_postal_address(&one) {
                return stop(
                    &format!("{CLI_NAME} postal send: '{one}' isn't a Postal address (handle::label.postal.bot)"),
                    Exit::Usage,
                );
            }
            let text = w.args[1..].join(" ");
            if text.is_empty() {
                return stop(
                    &format!("Usage: {CLI_NAME} msg {one} \"text\" [--no-wake] [--json]"),
                    Exit::Usage,
                );
            }
            let wake = if w.nowake { "__false" } else { "__true" };
            let b = body(&[
                ("workspace", &settings.project),
                ("to", &one),
                ("text", &text),
                ("wake", wake),
            ]);
            (Call2::Post("/cli/postal/send".into(), b), "send")
        }
        "remove" | "block" => {
            if let Err(e) = need(&one, sub, "<handle::label.postal.bot>") {
                return e;
            }
            let b = body(&[("workspace", &ws), ("peer", &one)]);
            (Call2::Post(format!("/cli/postal/{sub}"), b), sub)
        }
        "enable" => (
            Call2::Post("/cli/postal/enable".into(), body(&[("why", &w.why)])),
            "settings",
        ),
        "disable" => (
            Call2::Post(
                "/cli/postal/settings".into(),
                body(&[("enabled", "__false")]),
            ),
            "settings",
        ),
        "admins-may-accept" => {
            let v = match one.as_str() {
                "on" => "__true",
                "off" => "__false",
                _ => {
                    return stop(
                        &format!("Usage: {CLI_NAME} postal admins-may-accept on|off"),
                        Exit::Usage,
                    )
                }
            };
            (
                Call2::Post(
                    "/cli/postal/settings".into(),
                    body(&[("adminsMayAccept", v)]),
                ),
                "settings",
            )
        }
        "settings" => (
            Call2::Get("/cli/postal/settings".into(), String::new()),
            "settings",
        ),
        "link-p5" => (
            Call2::Post("/cli/postal/link-p5".into(), body(&[("workspace", &ws)])),
            "link",
        ),
        "adopt" => {
            return stop(
                &format!("{CLI_NAME} postal adopt: not available yet: p5 keeps its own handles; {CLI_NAME} postal link-p5 lets it deliver to this workspace."),
                Exit::Error,
            )
        }
        other => {
            return stop(
                &format!("{CLI_NAME} postal: unknown command '{other}' ({CLI_NAME} postal --help)"),
                Exit::Usage,
            )
        }
    };
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let would_refuse = passport::current(args).would_refuse();
    let mut client = match open_tool(&settings).client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let piped = match &call {
        Call2::Get(ep, q) => {
            let qs: Vec<&str> = if q.is_empty() {
                vec![]
            } else {
                vec![q.as_str()]
            };
            piped_call(&mut client, &mut closing, ep, Call::Request(&qs))
        }
        Call2::Post(ep, b) => {
            piped_call(&mut client, &mut closing, ep, Call::PostJson(b.as_bytes()))
        }
    };
    let shown = print(&piped.body, view, w.json, local_minute);
    let reader = shown.print();
    let exit = if sub == "send" {
        reader
    } else {
        super::pipefail(piped.exit, reader)
    };
    closing.flush(&mut client, exit);
    exit
}

/// One call on the left of a pipe, over TCP: what the reader reads, and
/// the call's code. No answer: its line, then [`end_in_pipe`].
fn piped_call(client: &mut Client, closing: &mut Closing, endpoint: &str, call: Call<'_>) -> Piped {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, endpoint, call) {
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

/// `time.strftime("%Y-%m-%d %H:%M", time.localtime(t))`, or the
/// exception's last line.
pub fn local_minute(t: &Value) -> Result<String, String> {
    let secs = match t {
        Value::Int(d) => d
            .parse::<i64>()
            .map_err(|_| "OverflowError: timestamp out of range for platform time_t".to_string())?,
        Value::Float(f) if f.is_finite() => f.floor() as i64,
        Value::Float(_) => return Err("ValueError: Invalid value NaN (not a number)".into()),
        Value::Bool(b) => i64::from(*b),
        other => {
            return Err(format!(
                "TypeError: 'float' object cannot be interpreted as an integer, not {}",
                py::type_name(other)
            ))
        }
    };
    local_tm(secs).ok_or_else(|| "OverflowError: timestamp out of range for platform time_t".into())
}

#[cfg(unix)]
fn local_tm(secs: i64) -> Option<String> {
    // `time_t` is narrower than `i64` on some targets.
    #[allow(clippy::useless_conversion)]
    let t: libc::time_t = secs.try_into().ok()?;
    // SAFETY: localtime_r writes only into `tm`, which outlives the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    ok.then(|| {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            i64::from(tm.tm_year) + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min
        )
    })
}

#[cfg(not(unix))]
fn local_tm(_secs: i64) -> Option<String> {
    None
}

/// What `_postal_print` printed so far.
#[derive(Default)]
struct Out {
    stdout: String,
    stderr: String,
}

impl Out {
    fn line(&mut self, t: &str) {
        self.stdout.push_str(t);
        self.stdout.push('\n');
    }
}

/// `v.get(k)` on what Python must find a dict.
fn at(v: &Value, k: &str) -> Result<Value, String> {
    match v {
        Value::Object(p) => Ok(get(p, k).cloned().unwrap_or(Value::Null)),
        other => Err(format!(
            "AttributeError: '{}' object has no attribute 'get'",
            py::type_name(other)
        )),
    }
}

/// `v.get(k, default)`.
fn at_or(v: &Value, k: &str, default: Value) -> Result<Value, String> {
    match v {
        Value::Object(p) => Ok(get(p, k).cloned().unwrap_or(default)),
        other => at(other, k),
    }
}

fn t(v: &Value) -> bool {
    py::truthy(v)
}

fn st(v: &Value) -> String {
    py::str_of(v)
}

/// `a + b` for two strings, or the `TypeError`.
fn concat(a: &Value, b: &str) -> Result<String, String> {
    match a {
        Value::Str(_) => Ok(format!("{}{b}", st(a))),
        other => Err(format!(
            "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
            py::type_name(other)
        )),
    }
}

/// `b + a` for a string `b`.
fn prefixed(b: &str, a: &Value) -> Result<String, String> {
    match a {
        Value::Str(_) => Ok(format!("{b}{}", st(a))),
        other => Err(format!(
            "TypeError: can only concatenate str (not \"{}\") to str",
            py::type_name(other)
        )),
    }
}

/// `sep.join(v)` for a list of strings, or the `TypeError`.
fn join_str(v: &Value, sep: &str) -> Result<String, String> {
    let items = super::py_iter(v, 1).map_err(|r| r.exception)?;
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match item {
            Value::Str(_) => out.push(st(item)),
            other => {
                return Err(format!(
                    "TypeError: sequence item {i}: expected str instance, {} found",
                    py::type_name(other)
                ))
            }
        }
    }
    Ok(out.join(sep))
}

/// `hint[0].upper() + hint[1:] + "."`.
fn sentence(hint: &Value) -> Result<String, String> {
    match hint {
        Value::Str(_) => {
            let s = st(hint);
            let mut c = s.chars();
            match c.next() {
                Some(f) => Ok(format!("{}{}.", f.to_uppercase(), c.as_str())),
                None => Err("IndexError: string index out of range".into()),
            }
        }
        other => Err(format!(
            "TypeError: '{}' object is not subscriptable",
            py::type_name(other)
        )),
    }
}

/// `x in (…)` for a tuple of strings.
fn among(v: &Value, list: &[&str]) -> bool {
    list.iter().any(|w| *v == super::s(w))
}

/// `_postal_print <view> <json>` on what it read from stdin.
pub fn print(
    raw: &[u8],
    view: &str,
    as_json: bool,
    when: impl Fn(&Value) -> Result<String, String>,
) -> Shown {
    let mut o = Out::default();
    let r = print_lines(raw, view, as_json, &when, &mut o);
    match r {
        Ok(exit) => Shown {
            stdout: o.stdout,
            stderr: o.stderr,
            exit,
        },
        Err(e) => super::raised(o.stdout, o.stderr, &Raise::at(1, e)),
    }
}

const PERM: &[&str] = &[
    "owner_only",
    "forbidden",
    "postal_off",
    "postal_people_only",
    "postal_people_decide",
    "postal_owner_decides",
    "postal_own_workspace_only",
    "role_required",
    "postal_not_friends",
    "postal_secret_in_text",
    "postal_route_louder",
    "postal_owner_link_missing",
    "postal_rekey_pending",
];

fn print_lines(
    raw: &[u8],
    v: &str,
    as_json: bool,
    when: &dyn Fn(&Value) -> Result<String, String>,
    o: &mut Out,
) -> Result<Exit, String> {
    let text = String::from_utf8_lossy(raw);
    let Ok(d) = json::parse(&text) else {
        let head: String = text.chars().take(300).collect();
        o.stderr
            .push_str(&format!("Error: daemon answered: {head}\n"));
        return Ok(Exit::Error);
    };
    let err = match &d {
        Value::Object(p) => get(p, "error").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    };
    if t(&err) {
        let (code, hint) = match &err {
            Value::Object(_) => (
                at_or(&err, "code", super::s(""))?,
                at_or(&err, "hint", super::s(""))?,
            ),
            other => (super::s(""), super::s(&st(other))),
        };
        let lead = if t(&code) {
            concat(&code, ": ")?
        } else {
            String::new()
        };
        o.stderr.push_str(&format!("Error: {lead}{}\n", st(&hint)));
        if matches!(code, Value::Array(_) | Value::Object(_)) {
            return Err(format!(
                "TypeError: unhashable type: '{}'",
                py::type_name(&code)
            ));
        }
        let exit = if among(&code, PERM) {
            Exit::Refused
        } else if among(&code, &["not_found", "postal_no_address", "postal_no_code"]) {
            Exit::NotFound
        } else if among(
            &code,
            &[
                "usage",
                "bad_postal_address",
                "postal_code_needed",
                "too_large",
            ],
        ) {
            Exit::Usage
        } else {
            Exit::Error
        };
        return Ok(exit);
    }
    if as_json {
        let opts = Options {
            indent: Some(2),
            ..Options::default()
        };
        o.line(&json::to_string(&d, &opts).unwrap_or_default());
        return Ok(Exit::Ok);
    }
    if t(&at(&d, "asked")?) {
        let which = {
            let r = at(&d, "request")?;
            if t(&r) {
                r
            } else {
                at(&d, "card")?
            }
        };
        let waiting = if t(&at(&d, "waiting")?) {
            ", waiting already"
        } else {
            ""
        };
        o.line(&format!(
            "Asked: a card is on your person's Thread ({}){waiting}.",
            st(&which)
        ));
        let hint = at(&d, "hint")?;
        let hint = if t(&hint) {
            hint
        } else {
            super::s(
                "a person answers the card; the answer comes back to you as a [permission:] turn",
            )
        };
        o.line(&sentence(&hint)?);
        return Ok(Exit::Usage);
    }
    if v == "settings" {
        o.line(&format!(
            "Postal: {}",
            if t(&at(&d, "enabled")?) { "on" } else { "off" }
        ));
        o.line(&format!(
            "Admins may accept friend requests: {}",
            if t(&at(&d, "adminsMayAccept")?) {
                "yes"
            } else {
                "no (the Owner only)"
            }
        ));
        return Ok(Exit::Ok);
    }
    let h = at(&d, "handle")?;
    let wsv = at(&d, "workspace")?;
    let wsh = if t(&wsv) {
        at(&wsv, "handle")?
    } else {
        Value::Null
    };
    let ws = if t(&wsh) {
        st(&wsh)
    } else {
        "this workspace".into()
    };
    if v == "send" {
        let s = at(&d, "status")?;
        let failed = s == super::s("failed");
        let reason = at(&d, "reason")?;
        let why = if t(&reason) && failed {
            prefixed(" (", &reason)? + ")"
        } else {
            String::new()
        };
        o.line(&format!(
            "{} to {}: {}{why}",
            if failed { "Not sent" } else { "Postal mail" },
            st(&at(&d, "to")?),
            st(&s)
        ));
        let badge = at(&d, "badge")?;
        if t(&badge) {
            o.line(&format!("Badge:    {}", st(&badge)));
        }
        let rec = at(&d, "deliveryId")?;
        if t(&rec) {
            o.line(&format!(
                "Record:   {} ({CLI_NAME} msg verify {})",
                st(&rec),
                st(&rec)
            ));
        }
        if s == super::s("queued") {
            let hint = at(&d, "hint")?;
            o.line(&if t(&hint) {
                st(&hint)
            } else {
                "Queued on this server; it goes out at the next try.".into()
            });
        }
        if s == super::s("held") {
            o.line("Held on the Postal network until their server picks it up (up to 7 days).");
        }
        if s == super::s("delivered") {
            o.line("Their server stored it.");
        }
        return Ok(if failed { Exit::Error } else { Exit::Ok });
    }
    if v == "code" {
        o.line(&format!("Your code: {}", st(&at(&d, "code")?)));
        o.line("Read it to the other side, and type the code shown on their screen:");
        o.line(&format!(
            "  {CLI_NAME} postal accept {} --code <their code>",
            st(&at(&d, "id")?)
        ));
        return Ok(Exit::Ok);
    }
    if v == "route" {
        let r = at(&d, "route")?;
        let friend = at(&d, "friend")?;
        let who = if t(&friend) {
            st(&friend)
        } else {
            "this workspace".into()
        };
        let lands = if t(&r) {
            let targets = at_or(&r, "targets", Value::Array(Vec::new()))?;
            let joined = join_str(&targets, " + ")?;
            let wake = at_or(&r, "wake", Value::Bool(true))?;
            format!("{joined}{}", if t(&wake) { "" } else { ", no wake" })
        } else {
            "default (terminal, waking)".into()
        };
        o.line(&format!("Postal mail from {who} lands: {lands}"));
        return Ok(Exit::Ok);
    }
    if v == "owner-link" {
        if t(&at(&d, "linked")?) {
            o.line(&format!(
                "Postal approvals linked for {}.",
                st(&at(&d, "label")?)
            ));
            return Ok(Exit::Ok);
        }
        o.line(&format!(
            "Open this page and approve (as the owner of {}):",
            st(&at(&d, "label")?)
        ));
        o.line(&format!("  {}", st(&at(&d, "url")?)));
        o.line(&format!(
            "Code on the page: {}. This server picks the approval up by itself.",
            st(&at(&d, "userCode")?)
        ));
        return Ok(Exit::Ok);
    }
    if v == "status" || v == "claim" {
        if t(&at(&d, "already")?) {
            o.line("This workspace has that address already.");
        }
        if t(&h) {
            let status = at(&h, "status")?;
            let where_ = if status == super::s("local") {
                "kept here; it goes on the Postal network once this server is connected".into()
            } else if status == super::s("claimed") {
                "on the Postal network".into()
            } else {
                st(&status)
            };
            o.line(&format!("Address:  {} ({where_})", st(&at(&h, "address")?)));
        } else {
            o.line(&format!(
                "Address:  none yet for {ws} (a person claims one: {CLI_NAME} postal claim <handle::label.postal.bot>)"
            ));
        }
        let c = at(&d, "counts")?;
        if t(&c) {
            let zero = || Value::Int("0".into());
            let n = |k: &str| -> Result<String, String> {
                py_d(&at_or(&c, k, zero())?, 1).map_err(|r| r.exception)
            };
            o.line(&format!(
                "Friends:  {}   requests in: {}   out: {}",
                n("accepted")?,
                n("pendingIn")?,
                n("pendingOut")?
            ));
        }
        if at(&d, "ownerLinked")? == Value::Bool(false) {
            o.line(&format!(
                "Approvals: not linked yet; the owner runs {CLI_NAME} postal owner-link once"
            ));
        }
        let plane = at(&d, "plane")?;
        let note = if t(&plane) {
            at(&plane, "note")?
        } else {
            Value::Null
        };
        if t(&note) {
            o.line(&st(&note));
        }
        return Ok(Exit::Ok);
    }
    if v == "friends" {
        let rows = at(&d, "friends")?;
        let rows = if t(&rows) {
            rows
        } else {
            Value::Array(Vec::new())
        };
        if !t(&h) {
            o.line(&format!("{ws} has no Postal address yet."));
            return Ok(Exit::Ok);
        }
        o.line(&format!("Address: {}", st(&at(&h, "address")?)));
        let list = super::py_iter(&rows, 1).map_err(|r| r.exception)?;
        let groups: [(&str, &[&str]); 4] = [
            ("Friends", &["accepted", "rekey_pending", "blocked"]),
            ("Requests in (a person answers)", &["pending_in"]),
            ("Requests out (waiting)", &["pending_out"]),
            ("History", &["rejected", "revoked"]),
        ];
        for (title, sts) in groups {
            let mut g = Vec::new();
            for r in &list {
                if among(&at(r, "status")?, sts) {
                    g.push(r.clone());
                }
            }
            if g.is_empty() {
                continue;
            }
            o.line(&format!("{title}:"));
            for r in &g {
                let status = at(r, "status")?;
                let mut extra = Vec::new();
                if status == super::s("blocked") {
                    extra.push("blocked".to_string());
                }
                if status == super::s("rekey_pending") {
                    extra.push("key changed: waits for a person".into());
                }
                let owner = at(r, "ownerName")?;
                if t(&owner) {
                    extra.push(prefixed("owner ", &owner)?);
                }
                if among(&status, &["pending_in", "pending_out", "rekey_pending"]) {
                    let id = match r {
                        Value::Object(p) => get(p, "id")
                            .cloned()
                            .ok_or_else(|| "KeyError: 'id'".to_string())?,
                        _ => Value::Null,
                    };
                    extra.push(prefixed("id ", &id)?);
                }
                if t(&at(r, "codeWaiting")?) {
                    extra.push("pairing code waiting: a person types theirs".into());
                }
                if status == super::s("pending_out") && t(&at(r, "decidedBy")?) {
                    extra.push("you accepted; waiting for their side".into());
                }
                let u = at(r, "updatedAt")?;
                let when = if t(&u) { when(&u)? } else { "-".into() };
                let tail = if extra.is_empty() {
                    String::new()
                } else {
                    format!("  ({})", extra.join(", "))
                };
                o.line(&format!("  {}  {when}{tail}", st(&at(r, "peer")?)));
            }
        }
        if !t(&rows) {
            o.line("No friends or requests yet.");
        }
        return Ok(Exit::Ok);
    }
    let f = at(&d, "friend")?;
    if t(&f) {
        let fstatus = at(&f, "status")?;
        o.line(&format!("{}: {}", st(&at(&f, "peer")?), st(&fstatus)));
        let hint = at(&d, "hint")?;
        if t(&at(&d, "friends")?) {
            o.line("Friends now.");
        } else if t(&hint) {
            o.line(&sentence(&hint)?);
        } else if fstatus == super::s("pending_out") && v == "invite" {
            o.line(if t(&at(&d, "sent")?) {
                "Sent over the Postal network."
            } else {
                "Recorded; it goes out as soon as this server is connected to the Postal network."
            });
        }
        return Ok(Exit::Ok);
    }
    o.line(&json::to_string(&d, &Options::default()).unwrap_or_default());
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert!(is_postal_address("Scout::Acme.postal.bot"));
        assert!(is_postal_address("a::abc.postal.bot"));
        assert!(!is_postal_address("a::ab.postal.bot"));
        assert!(!is_postal_address("_a::acme.postal.bot"));
        assert!(!is_postal_address("a::-acme.postal.bot"));
        assert!(!is_postal_address("a:acme.postal.bot"));
    }

    #[test]
    fn bodies() {
        assert_eq!(
            body(&[
                ("workspace", "/w"),
                ("why", ""),
                ("wake", "__false"),
                ("t", "é")
            ]),
            "{\"workspace\": \"/w\", \"wake\": false, \"t\": \"\\u00e9\"}"
        );
    }

    #[test]
    fn the_words() {
        assert!(parse(&["--why"])
            .unwrap_err()
            .contains("--why needs the reason"));
        assert!(parse(&["-x"]).unwrap_err().contains("unknown flag '-x'"));
        let w = parse(&["a", "--json", "b", "--friend", "f"]).unwrap();
        assert_eq!((w.args.len(), w.json, w.friend.as_str()), (2, true, "f"));
    }

    #[test]
    fn the_usage_names_the_command() {
        let u = usage();
        assert!(u.starts_with(&format!("Usage: {CLI_NAME} postal <command>")));
        assert!(!u.contains("{cli}") && !u.contains("{product}"));
    }
}
