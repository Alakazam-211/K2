//! `k5 firewall`, as the shell CLI's `cmd_firewall` answers it
//! (`cli/src/firewall.sh`): the box's public ports and the owner's
//! options. Every form: `status` (also no verb, and `--json` first),
//! `ports`, `rules [--format nft|ufw]`, `ssh any|allow|port`, `open`,
//! `close`, `guard`, `upstream`, each with `--json` (which `rules`
//! ignores), and the help.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at
//!    once, then the virtual desktop's guard and the connection gate
//!    ([`crate::prelude::gate`]). A command line with `-h` or `--help`
//!    anywhere skips the guard; nothing skips the connection gate, the
//!    help neither (the shell's prelude has no `firewall` row).
//! 2. `firewall` is an `id` tool (`dns` in the shell's policy), so with
//!    `HOOK_SOCK` set: the stale-passport or require-passport lines, exit
//!    3 ([`crate::prelude::id_tool`]), the help too.
//! 3. The words ([`parse`]), then the verb's checks and its body
//!    ([`request`]): each problem is one `usage:` line, exit 2. `guard`
//!    and `upstream` make their body with the shell's Python, whose
//!    problems are exit 2 as well (a traceback too, where its `int()`
//!    raises).
//! 4. One `_sidecar_send` ([`crate::transport::Client::sidecar_get`] or
//!    [`crate::transport::Client::sidecar_post`]), as [`super::job`]
//!    sends it: a POST that went out on the cell socket is never sent
//!    again.
//! 5. `_firewall_render`: no answer is `daemon unreachable`, exit 1; a
//!    status that is not 200 is its Python's lines ([`error_lines`],
//!    `Ask with:` from the answer's own `permission`, whatever the cards
//!    setting) with `_sidecar_exit_for`'s exit ([`refusal`]; where that
//!    Python raises, the shell's `set -e` ends the command with 1 and no
//!    body); else the body as it came
//!    (`--json`, `status --json`, the text forms) or its Python's lines
//!    ([`view_lines`]). No closing line and no bulletin: the shell's
//!    `_firewall_render` notes nothing.
//!
//! The shell CLI answers these itself ([`run`] returns `None`): a word
//! that is not UTF-8, `HOOK_SOCK` set with a `CODEX_` variable exported,
//! a port that is set but is no port curl takes (for a form that asks),
//! and a `python3` the binary can't tell.
//!
//! The shell hands the answer to its Python in an environment variable
//! (`K2_FW_BODY`). Linux refuses one longer than 128 KiB, so there the
//! shell's `ports` (and every other verb's laid-out answer, and a
//! refusal) stops on python3's "Argument list too long", exit 126. This
//! prints the lines Python would have printed.
//!
//! Where the shell's Python raises, this prints a Python 3.12 traceback,
//! as [`super::sessions`] says; bytes that are not UTF-8 and half of a
//! surrogate pair are as [`super::inbox`] says.

use super::job::held;
use super::{curl_port, get, ljust, py_get, py_iter, py_space, raised, s, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Gate};
use crate::python;
use k2_client::brand::CLI_NAME as CLI;
use k2_client::{cli_name, product_name};
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["firewall"];
/// The binary answers every form ([`run`] may still hand one to the
/// shell CLI).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: the whole row is native.
pub const VERBS: &[crate::route::Verb] = &[];

/// The view's route; the others are under it.
pub const ROUTE: &str = "/cli/firewall";
/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "dns";
/// `_firewall_render` with no answer.
pub const UNREACHABLE: &str = concat!(cli_name!(), " firewall: daemon unreachable");
/// What the render's Python starts each error line with.
pub const ERROR: &str = concat!(cli_name!(), " firewall: error: ");
/// `guard`'s Python, a third word that is neither `on` nor `off`.
pub const GUARD_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " firewall guard [<app>] [on|off] [--conns-per-ip N] [--writes-per-min N] [--reset]"
);
/// `upstream`'s Python, nothing to send. The second name is the wire
/// value `managedBy` takes, in every brand.
pub const UPSTREAM_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " firewall upstream --hash <sha256:\u{2026}> --provider <name> [--applied-at <unix>] | --managed-by k2|owner|auto"
);
/// `upstream`'s Python, an `--applied-at` that is no number.
pub const APPLIED_AT_USAGE: &str = "usage: --applied-at <unix seconds>";
/// `rules` with a format it doesn't have.
pub const RULES_USAGE: &str = concat!("usage: ", cli_name!(), " firewall rules --format nft|ufw");
/// `ssh allow` with no address.
pub const SSH_ALLOW_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " firewall ssh allow <cidr>... (SSH is never closed; for anyone: ",
    cli_name!(),
    " firewall ssh any)"
);
/// `ssh port` with no number.
pub const SSH_PORT_USAGE: &str = concat!("usage: ", cli_name!(), " firewall ssh port <n>");
/// `ssh` with no form it has.
pub const SSH_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " firewall ssh any | allow <cidr>... | port <n>"
);
/// `open` with no `--label`.
pub const OPEN_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " firewall open <port>/<tcp|udp> --label \"<why>\" [--from <cidr>]..."
);

/// A `--` flag the family doesn't have.
pub fn unknown_flag(flag: &str) -> String {
    format!("usage: unknown flag '{flag}' ({CLI} firewall --help)")
}

/// `open` or `close` without exactly one port.
pub fn port_usage(sub: &str) -> String {
    format!("usage: {CLI} firewall {sub} <port>/<tcp|udp>")
}

/// A verb `firewall` doesn't have.
pub fn unknown_verb(sub: &str) -> String {
    format!(
        "usage: unknown firewall verb '{sub}' (status|ports|rules|ssh|open|close|guard|upstream)"
    )
}

/// A value flag at the end of the line: its own usage line.
pub fn flag_usage(flag: &str) -> &'static str {
    match flag {
        "--format" => "usage: --format nft|ufw",
        "--label" => "usage: --label \"<why>\"",
        "--from" => "usage: --from <cidr>",
        "--conns-per-ip" => "usage: --conns-per-ip N",
        "--writes-per-min" => "usage: --writes-per-min N",
        "--hash" => "usage: --hash sha256:\u{2026}",
        "--provider" => "usage: --provider <name>",
        "--applied-at" => "usage: --applied-at <unix>",
        // The value is the wire's, in every brand.
        _ => "usage: --managed-by k2|owner|auto",
    }
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    /// The verb: `status` for none (or an empty one) and for `--json`.
    pub sub: String,
    pub json: bool,
    pub format: String,
    pub label: String,
    pub reset: bool,
    pub conns: String,
    pub writes: String,
    pub hash: String,
    pub provider: String,
    pub applied: String,
    pub managed: String,
    /// The words that are no flag, in order.
    pub args: Vec<String>,
    /// Each `--from`'s value, in order.
    pub froms: Vec<String>,
}

/// What `cmd_firewall` does with its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// `_firewall_usage`: the help, exit 0.
    Help,
    /// What to print on stderr (each line with its newline), exit 2.
    Early(String),
    /// The verb's checks to make.
    Call(Box<Ask>),
}

/// `cmd_firewall`'s words (after `firewall`): the verb, then the flags
/// in order. A flag's problem, or a help flag, ends the reading where it
/// stands.
pub fn parse(words: &[&str]) -> Parsed {
    let first = words.first().copied().unwrap_or("");
    let mut sub = if first.is_empty() { "status" } else { first }.to_string();
    let mut rest: Vec<&str> = words.get(1..).unwrap_or_default().to_vec();
    match sub.as_str() {
        "help" | "--help" | "-h" => return Parsed::Help,
        "--json" => {
            sub = "status".into();
            rest.insert(0, "--json");
        }
        _ => {}
    }
    let mut ask = Ask {
        sub,
        ..Ask::default()
    };
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        match a {
            "--json" => ask.json = true,
            "--reset" => ask.reset = true,
            "--format" | "--label" | "--from" | "--conns-per-ip" | "--writes-per-min"
            | "--hash" | "--provider" | "--applied-at" | "--managed-by" => {
                // `shift 2` fails with no value after the flag.
                let Some(value) = rest.get(i + 1) else {
                    return Parsed::Early(format!("{}\n", flag_usage(a)));
                };
                let v = value.to_string();
                match a {
                    "--format" => ask.format = v,
                    "--label" => ask.label = v,
                    "--from" => ask.froms.push(v),
                    "--conns-per-ip" => ask.conns = v,
                    "--writes-per-min" => ask.writes = v,
                    "--hash" => ask.hash = v,
                    "--provider" => ask.provider = v,
                    "--applied-at" => ask.applied = v,
                    _ => ask.managed = v,
                }
                i += 1;
            }
            "-h" | "--help" => return Parsed::Help,
            _ => {
                if let Some(v) = a.strip_prefix("--format=") {
                    ask.format = v.to_string();
                } else if a.starts_with('-') {
                    return Parsed::Early(format!("{}\n", unknown_flag(a)));
                } else {
                    ask.args.push(a.to_string());
                }
            }
        }
        i += 1;
    }
    Parsed::Call(Box::new(ask))
}

/// How `_firewall_render` shows a 200.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The body and a newline.
    Json,
    /// The body as it came.
    Text,
    /// The body and a newline with `--json`, else its Python's lines.
    Status,
}

/// The one request a verb sends, and how its answer shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub get: bool,
    pub path: String,
    /// The query for a GET, the JSON body for a POST.
    pub payload: String,
    /// `_firewall_render`'s first word: a refusal's body is printed too.
    pub as_json: bool,
    pub mode: Mode,
}

impl Request {
    fn of_get(path: &str, query: &str, as_json: bool, mode: Mode) -> Request {
        Request {
            get: true,
            path: path.to_string(),
            payload: query.to_string(),
            as_json,
            mode,
        }
    }

    fn of_post(path: &str, body: String, as_json: bool) -> Request {
        Request {
            get: false,
            path: path.to_string(),
            payload: body,
            as_json,
            mode: Mode::Status,
        }
    }
}

/// `json.dumps(o)`, Python's defaults (`ensure_ascii` on).
pub(super) fn dumps(o: Vec<(&str, Value)>) -> String {
    let pairs = o.into_iter().map(|(k, v)| (Text::from(k), v)).collect();
    // Strings from UTF-8 words and whole numbers: nothing to refuse.
    json::to_string(&Value::Object(pairs), &Options::default()).unwrap_or_default()
}

fn strings(list: &[String]) -> Value {
    Value::Array(list.iter().map(|w| s(w)).collect())
}

/// The verb's checks, in the shell's order, and its request. `Err`: what
/// to print on stderr (each line with its newline), exit 2.
pub fn request(ask: &Ask) -> Result<Request, String> {
    let usage = |line: &str| Err(format!("{line}\n"));
    let first = ask.args.first().map_or("", String::as_str);
    Ok(match ask.sub.as_str() {
        "status" if ask.json => Request::of_get(ROUTE, "", true, Mode::Json),
        "status" => Request::of_get(ROUTE, "format=text", false, Mode::Text),
        "ports" => Request::of_get(&format!("{ROUTE}/ports"), "", ask.json, Mode::Status),
        "rules" => {
            let format = if ask.format.is_empty() {
                "nft"
            } else {
                ask.format.as_str()
            };
            if !matches!(format, "nft" | "nftables" | "ufw") {
                return usage(RULES_USAGE);
            }
            Request::of_get(
                &format!("{ROUTE}/rules"),
                &format!("format={format}"),
                false,
                Mode::Text,
            )
        }
        "ssh" => {
            let body = match first {
                "any" => r#"{"any":true}"#.to_string(),
                "allow" => {
                    if ask.args.len() < 2 {
                        return usage(SSH_ALLOW_USAGE);
                    }
                    dumps(vec![("allow", strings(&ask.args[1..]))])
                }
                "port" => {
                    let n = ask.args.get(1).map_or("", String::as_str);
                    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
                        return usage(SSH_PORT_USAGE);
                    }
                    // As typed, leading zeros and all.
                    format!("{{\"port\":{n}}}")
                }
                _ => return usage(SSH_USAGE),
            };
            Request::of_post(&format!("{ROUTE}/ssh"), body, ask.json)
        }
        "open" | "close" => {
            if ask.args.len() != 1 {
                return usage(&port_usage(&ask.sub));
            }
            let body = if ask.sub == "open" {
                if ask.label.is_empty() {
                    return usage(OPEN_USAGE);
                }
                dumps(vec![
                    ("port", s(first)),
                    ("label", s(&ask.label)),
                    ("from", strings(&ask.froms)),
                ])
            } else {
                dumps(vec![("port", s(first))])
            };
            Request::of_post(&format!("{ROUTE}/{}", ask.sub), body, ask.json)
        }
        "guard" => Request::of_post(&format!("{ROUTE}/guard"), guard_body(ask)?, ask.json),
        "upstream" => Request::of_post(&format!("{ROUTE}/upstream"), upstream_body(ask)?, ask.json),
        _ => return usage(&unknown_verb(&ask.sub)),
    })
}

/// `guard`'s Python: the words (as the shell joins them with `\x1f` and
/// Python splits them again), the two numbers, `--reset`.
pub fn guard_body(ask: &Ask) -> Result<String, String> {
    let joined = ask.args.join("\u{1f}");
    let words: Vec<&str> = if joined.is_empty() {
        Vec::new()
    } else {
        joined.split('\u{1f}').collect()
    };
    /// `b[k] = v`: a key keeps its first place.
    fn set(b: &mut Vec<(&'static str, Value)>, k: &'static str, v: Value) {
        match b.iter_mut().find(|(key, _)| *key == k) {
            Some(pair) => pair.1 = v,
            None => b.push((k, v)),
        }
    }
    let mut b: Vec<(&'static str, Value)> = Vec::new();
    for a in &words {
        if matches!(*a, "on" | "off") {
            set(&mut b, "on", Value::Bool(*a == "on"));
        } else if !b.iter().any(|(k, _)| *k == "app") {
            set(&mut b, "app", s(a));
        } else {
            return Err(format!("{GUARD_USAGE}\n"));
        }
    }
    for (k, v) in [("connsPerIp", &ask.conns), ("writesPerMin", &ask.writes)] {
        if !v.is_empty() {
            if !py_isdigit(v) {
                return Err(format!("usage: {k} takes a number\n"));
            }
            let n = py_int(v, line::G_INT).map_err(|r| r.text())?;
            set(&mut b, k, n);
        }
    }
    if ask.reset {
        set(&mut b, "reset", Value::Bool(true));
    }
    Ok(dumps(b))
}

/// `upstream`'s Python: `--hash` with `--provider` (and `--applied-at`),
/// `--managed-by`, or the usage line.
pub fn upstream_body(ask: &Ask) -> Result<String, String> {
    let mut b: Vec<(&str, Value)> = Vec::new();
    if !ask.hash.is_empty() {
        b.push(("hash", s(&ask.hash)));
        b.push(("provider", s(&ask.provider)));
        if !ask.applied.is_empty() {
            if !py_isdigit(&ask.applied) {
                return Err(format!("{APPLIED_AT_USAGE}\n"));
            }
            b.push((
                "appliedAt",
                py_int(&ask.applied, line::U_INT).map_err(|r| r.text())?,
            ));
        }
    }
    if !ask.managed.is_empty() {
        b.push(("managedBy", s(&ask.managed)));
    }
    if b.is_empty() {
        return Err(format!("{UPSTREAM_USAGE}\n"));
    }
    Ok(dumps(b))
}

/// The first code point of each run of ten decimal digits (Unicode's
/// `Nd`, 0 to 9) as Python 3.12 has them (Unicode 15.0).
const DECIMAL_ZEROS: [u32; 68] = [
    0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66,
    0xDE6, 0xE50, 0xED0, 0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90,
    0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
    0x104A0, 0x10D30, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450, 0x114D0, 0x11650,
    0x116C0, 0x11730, 0x118E0, 0x11950, 0x11C50, 0x11D50, 0x11DA0, 0x11F50, 0x16A60, 0x16AC0,
    0x16B50, 0x1D7CE, 0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0, 0x1E4F0, 0x1E950,
    0x1FBF0,
];

/// The other code points Python 3.12's `str.isdigit()` takes (Unicode's
/// `Numeric_Type=Digit`: superscripts, circled digits and the like),
/// which `int()` refuses.
const OTHER_DIGITS: [(u32, u32); 20] = [
    (0xB2, 0xB3),
    (0xB9, 0xB9),
    (0x1369, 0x1371),
    (0x19DA, 0x19DA),
    (0x2070, 0x2070),
    (0x2074, 0x2079),
    (0x2080, 0x2089),
    (0x2460, 0x2468),
    (0x2474, 0x247C),
    (0x2488, 0x2490),
    (0x24EA, 0x24EA),
    (0x24F5, 0x24FD),
    (0x24FF, 0x24FF),
    (0x2776, 0x277E),
    (0x2780, 0x2788),
    (0x278A, 0x2792),
    (0x10A40, 0x10A43),
    (0x10E60, 0x10E68),
    (0x11052, 0x1105A),
    (0x1F100, 0x1F10A),
];

/// The digit a decimal digit stands for.
fn decimal(c: char) -> Option<u32> {
    let cp = u32::from(c);
    DECIMAL_ZEROS
        .iter()
        .find(|z| (**z..**z + 10).contains(&cp))
        .map(|z| cp - z)
}

/// Python 3.12's `str.isdigit()`.
pub fn py_isdigit(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| {
            decimal(c).is_some()
                || OTHER_DIGITS
                    .iter()
                    .any(|(a, b)| (*a..=*b).contains(&u32::from(c)))
        })
}

/// Python's limit on the digits `int()` reads.
const INT_MAX_STR_DIGITS: usize = 4300;

/// `int(text)` for a text `isdigit()` took, at `line`: the number, or
/// the `ValueError` (a digit that is no decimal one, or too many).
pub fn py_int(text: &str, line: u32) -> Result<Value, Raise> {
    let digits: Option<String> = text
        .chars()
        .map(|c| decimal(c).and_then(|d| char::from_digit(d, 10)))
        .collect();
    let Some(digits) = digits else {
        // `%.200R`: the text's repr, cut at 200 code points.
        let shown: String = py::repr(&s(text)).chars().take(200).collect();
        return Err(Raise::at(
            line,
            format!("ValueError: invalid literal for int() with base 10: {shown}"),
        ));
    };
    if digits.len() > INT_MAX_STR_DIGITS {
        return Err(Raise::at(
            line,
            format!(
                "ValueError: Exceeds the limit ({INT_MAX_STR_DIGITS} digits) for integer string conversion: value has {} digits; use sys.set_int_max_str_digits() to increase the limit",
                digits.len()
            ),
        ));
    }
    let trimmed = digits.trim_start_matches('0');
    Ok(Value::Int(if trimmed.is_empty() {
        "0".into()
    } else {
        trimmed.to_string()
    }))
}

/// Whether a command line skips the virtual desktop's guard: `-h` or
/// `--help` anywhere in it (`_k2_offline_verb`).
fn offline_verb(words: &[&str]) -> bool {
    words.iter().any(|w| matches!(*w, "-h" | "--help"))
}

/// `k5 firewall ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let all: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    let parsed = parse(all.get(1..).unwrap_or_default());
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
    let help = parsed == Parsed::Help;
    let gate = prelude::gate(&settings);
    if !help && gate.is_none() && settings.port_text.as_deref().and_then(curl_port).is_none() {
        return None;
    }
    Some(run_with(
        args,
        &parsed,
        offline_verb(&all),
        hook_sock.as_deref(),
        settings,
    ))
}

fn run_with(
    args: &[OsString],
    parsed: &Parsed,
    offline: bool,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    // The guard, unless `-h` or `--help` is anywhere; then the connection
    // gate, the help's too.
    match prelude::gate(&settings) {
        Some(Gate::VirtualDesktop) if !offline => return Gate::VirtualDesktop.stop(),
        Some(_) => return Gate::CannotConnect.stop(),
        None => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let ask = match parsed {
        Parsed::Help => {
            return Shown::out(crate::text::help("firewall").unwrap_or_default()).print()
        }
        Parsed::Early(text) => return Shown::err(text.clone(), Exit::Usage).print(),
        Parsed::Call(ask) => ask,
    };
    let req = match request(ask) {
        Ok(r) => r,
        Err(text) => return Shown::err(text, Exit::Usage).print(),
    };
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let answer = if req.get {
        client.sidecar_get(&req.path, &req.payload)
    } else {
        client.sidecar_post(&req.path, &req.payload)
    };
    match held(answer) {
        Ok((status, body)) => render(req.as_json, req.mode, &status, &body).print(),
        Err(line) => {
            out::stderr_line(&line);
            Exit::Error
        }
    }
}

/// What `_firewall_render` prints: stdout's bytes (a body as it came
/// need not be UTF-8), stderr, and the exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub exit: Exit,
}

impl Rendered {
    pub(super) fn of_shown(shown: Shown) -> Rendered {
        Rendered {
            stdout: shown.stdout.into_bytes(),
            stderr: shown.stderr,
            exit: shown.exit,
        }
    }

    /// Writes stderr (the Python runs first), then stdout; the exit. A
    /// broken stdout ends as [`out::after_stdout_error`] says.
    pub fn print(&self) -> Exit {
        if !self.stderr.is_empty() {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), self.stderr.as_bytes());
        }
        if !self.stdout.is_empty() {
            if let Err(e) = out::write_flushed(&mut std::io::stdout().lock(), &self.stdout) {
                return out::after_stdout_error(&e);
            }
        }
        self.exit
    }
}

/// `_sidecar_exit_for`.
pub fn exit_for(status: &str) -> Exit {
    match status {
        "200" => Exit::Ok,
        "400" | "413" => Exit::Usage,
        "403" => Exit::Refused,
        "404" => Exit::NotFound,
        _ => Exit::Error,
    }
}

/// `_firewall_render <as_json> <mode>` on `_sidecar_send`'s status
/// (`000` for none) and body.
pub fn render(as_json: bool, mode: Mode, status: &str, body: &[u8]) -> Rendered {
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
        return refusal(as_json, status, body);
    }
    if as_json || mode == Mode::Json {
        return Rendered {
            stdout: with_newline(),
            stderr: String::new(),
            exit: Exit::Ok,
        };
    }
    if mode == Mode::Text {
        return Rendered {
            stdout: body.to_vec(),
            stderr: String::new(),
            exit: Exit::Ok,
        };
    }
    Rendered::of_shown(view_lines(body))
}

/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    /// The refusal's Python: `d.get("error")`.
    pub const E_ERROR: u32 = 8;
    /// `": " + d["message"]`.
    pub const E_MESSAGE: u32 = 12;
    /// `"Ask with: " + p["command"]`.
    pub const E_ASK: u32 = 16;
    /// The view's Python.
    pub const V_LOAD: u32 = 3;
    pub const V_PUBLIC: u32 = 4;
    pub const V_HEAD: u32 = 5;
    pub const V_PUBLIC_FOR: u32 = 6;
    pub const V_PUBLIC_ROW: u32 = 7;
    pub const V_CLOSED_FOR: u32 = 8;
    pub const V_CLOSED_ROW: u32 = 9;
    pub const V_UP: u32 = 10;
    pub const V_APPLIED_BY: u32 = 11;
    pub const V_GUARD: u32 = 12;
    pub const V_G: u32 = 13;
    pub const V_G_LINE: u32 = 14;
    pub const V_APPS: u32 = 15;
    pub const V_APP: u32 = 16;
    pub const V_UPSTREAM: u32 = 17;
    pub const V_UP_D: u32 = 18;
    pub const V_A: u32 = 19;
    pub const V_MANAGED: u32 = 20;
    pub const V_APPLIED: u32 = 22;
    pub const V_OPENED: u32 = 25;
    pub const V_SSH: u32 = 27;
    pub const V_SSH_LINE: u32 = 29;
    pub const V_NOTE: u32 = 30;
    pub const V_NOTE_LINE: u32 = 31;
    pub const V_HASH: u32 = 32;
    /// `guard`'s `int(v)`.
    pub const G_INT: u32 = 16;
    /// `upstream`'s `int(a)`.
    pub const U_INT: u32 = 10;
}

/// The refusal's Python (on stderr): `code: hint` from an `error`
/// object, else `error` and `: message`, then the answer's `Ask with:`
/// or "It can't be requested." line; an answer that is not JSON is
/// `HTTP <status>: ` and its first 300 characters, spaces trimmed. Where
/// it raises, the lines before it and the traceback.
pub fn error_lines(status: &str, body: &[u8]) -> String {
    error_text(status, body).0
}

/// [`error_lines`], and whether its Python raised.
fn error_text(status: &str, body: &[u8]) -> (String, bool) {
    let raw = String::from_utf8_lossy(body);
    let Ok(d) = json::parse(&raw) else {
        let head: String = raw.trim_matches(py_space).chars().take(300).collect();
        return (format!("{ERROR}HTTP {status}: {head}\n"), false);
    };
    let mut o = String::new();
    match error_python(&d, &mut o) {
        Ok(()) => (o, false),
        Err(r) => (o + &r.text(), true),
    }
}

/// `_firewall_render` on a status that is not 200: the refusal's
/// Python, then the body with `--json`, and `_sidecar_exit_for`'s exit.
/// Where that Python raises, the shell's `set -e` ends the command
/// there: exit 1, and no body.
pub fn refusal(as_json: bool, status: &str, body: &[u8]) -> Rendered {
    let (stderr, raised) = error_text(status, body);
    if raised {
        return Rendered {
            stdout: Vec::new(),
            stderr,
            exit: Exit::Error,
        };
    }
    let mut stdout = Vec::new();
    if as_json {
        stdout.extend_from_slice(body);
        stdout.push(b'\n');
    }
    Rendered {
        stdout,
        stderr,
        exit: exit_for(status),
    }
}

fn error_python(d: &Value, o: &mut String) -> Result<(), Raise> {
    let e = py_get(d, "error", line::E_ERROR)?
        .cloned()
        .unwrap_or(Value::Null);
    if let Value::Object(pairs) = &e {
        let code = get(pairs, "code").map_or_else(|| "None".into(), py::str_of);
        let hint = get(pairs, "hint")
            .filter(|h| py::truthy(h))
            .map_or_else(String::new, py::str_of);
        push(o, &format!("{ERROR}{code}: {hint}"));
    } else {
        let message = match dict_get(d, "message").filter(|m| py::truthy(m)) {
            Some(m) => format!(": {}", plus_str(m, line::E_MESSAGE)?),
            None => String::new(),
        };
        push(o, &format!("{ERROR}{}{message}", py::str_of(&e)));
    }
    if let Some(Value::Object(p)) = dict_get(d, "permission") {
        if let Some(command) = get(p, "command").filter(|c| py::truthy(c)) {
            push(o, &format!("Ask with: {}", plus_str(command, line::E_ASK)?));
        } else if get(p, "requestable") == Some(&Value::Bool(false)) {
            push(o, "It can't be requested.");
        }
    }
    Ok(())
}

/// `d.get(key)` where `d` is a dict for sure.
pub(super) fn dict_get<'a>(d: &'a Value, key: &str) -> Option<&'a Value> {
    match d {
        Value::Object(pairs) => get(pairs, key),
        _ => None,
    }
}

/// `"<text>" + v` at `line`: the string, or the `TypeError`.
pub(super) fn plus_str(v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(py::str_of(v)),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

pub(super) fn push(o: &mut String, text: &str) {
    o.push_str(text);
    o.push('\n');
}

/// `x or default`.
pub(super) fn or(v: Option<&Value>, default: Value) -> Value {
    v.filter(|x| py::truthy(x)).cloned().unwrap_or(default)
}

/// `v.get(key)` at `line`, `None` for a missing key.
pub(super) fn at(v: &Value, key: &str, line: u32) -> Result<Value, Raise> {
    Ok(py_get(v, key, line)?.cloned().unwrap_or(Value::Null))
}

/// `key in d` at `line`: a dict's key, a list's item, a string's part.
fn py_in(d: &Value, key: &str, line: u32) -> Result<bool, Raise> {
    Ok(match d {
        Value::Object(pairs) => get(pairs, key).is_some(),
        Value::Array(items) => items
            .iter()
            .any(|x| matches!(x, Value::Str(t) if *t == Text::from(key))),
        Value::Str(t) => {
            let hay = t.code_points();
            let needle: Vec<u32> = key.chars().map(u32::from).collect();
            hay.windows(needle.len()).any(|w| w == needle.as_slice())
        }
        other => {
            return Err(Raise::at(
                line,
                format!(
                    "TypeError: argument of type '{}' is not iterable",
                    py::type_name(other)
                ),
            ))
        }
    })
}

/// `d[key]` at `line`.
fn py_index(d: &Value, key: &str, line: u32) -> Result<Value, Raise> {
    let why = match d {
        Value::Object(pairs) => {
            return get(pairs, key)
                .cloned()
                .ok_or_else(|| Raise::at(line, format!("KeyError: {}", py::repr(&s(key)))))
        }
        Value::Array(_) => "list indices must be integers or slices, not str".to_string(),
        Value::Str(_) => "string indices must be integers, not 'str'".to_string(),
        other => format!("'{}' object is not subscriptable", py::type_name(other)),
    };
    Err(Raise::at(line, format!("TypeError: {why}")))
}

/// `", ".join(v)` at `line`.
fn py_join(v: &Value, line: u32) -> Result<String, Raise> {
    let items = match v {
        Value::Array(_) | Value::Str(_) | Value::Object(_) => py_iter(v, line)?,
        _ => return Err(Raise::at(line, "TypeError: can only join an iterable")),
    };
    let mut parts = Vec::with_capacity(items.len());
    for (i, x) in items.iter().enumerate() {
        match x {
            Value::Str(_) => parts.push(py::str_of(x)),
            other => {
                return Err(Raise::at(
                    line,
                    format!(
                        "TypeError: sequence item {i}: expected str instance, {} found",
                        py::type_name(other)
                    ),
                ))
            }
        }
    }
    Ok(parts.join(", "))
}

/// `'%<width>s' % v`: `str(v)` with spaces before it, to `width` code
/// points.
fn rjust(text: &str, width: usize) -> String {
    let n = text.chars().count();
    if n < width {
        format!("{}{text}", " ".repeat(width - n))
    } else {
        text.to_string()
    }
}

/// `{True: "on", False: "off"}.get(v, "box default")` at `line`: Python's
/// equal keys (`1` and `1.0` are `True`), and no list or dict.
fn on_off(v: &Value, line: u32) -> Result<&'static str, Raise> {
    Ok(match v {
        Value::Bool(true) => "on",
        Value::Bool(false) => "off",
        Value::Int(d) if d == "1" => "on",
        Value::Int(d) if d == "0" => "off",
        Value::Float(f) if *f == 1.0 => "on",
        Value::Float(f) if *f == 0.0 => "off",
        Value::Array(_) | Value::Object(_) => {
            return Err(Raise::at(
                line,
                format!("TypeError: unhashable type: '{}'", py::type_name(v)),
            ))
        }
        _ => "box default",
    })
}

/// The view's Python on a 200's body (`ports`, and each change's
/// answer): the public list, the App guard, the upstream record, or what
/// a change did.
pub fn view_lines(body: &[u8]) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let d = match json::parse(&raw) {
        Ok(v) => v,
        Err(e) => {
            let why = if raw.trim_matches(py_space).is_empty() {
                "Expecting value: line 1 column 1 (char 0)".to_string()
            } else {
                e.to_string()
            };
            return raised(
                String::new(),
                String::new(),
                &Raise::at(line::V_LOAD, format!("json.decoder.JSONDecodeError: {why}")),
            );
        }
    };
    let mut o = String::new();
    match view(&d, &mut o) {
        Ok(()) => Shown::out(o),
        Err(r) => raised(o, String::new(), &r),
    }
}

fn view(d: &Value, o: &mut String) -> Result<(), Raise> {
    use line::*;
    if py_in(d, "public", V_PUBLIC)? && py_in(d, "version", V_PUBLIC)? {
        let hash = at(d, "hash", V_HEAD)?;
        let version = at(d, "version", V_HEAD)?;
        push(
            o,
            &format!("view {} (v{})", py::str_of(&hash), py::str_of(&version)),
        );
        let public = or(py_get(d, "public", V_PUBLIC_FOR)?, Value::Array(Vec::new()));
        for r in py_iter(&public, V_PUBLIC_FOR)? {
            let port = py_index(&r, "port", V_PUBLIC_ROW)?;
            let proto = py_index(&r, "proto", V_PUBLIC_ROW)?;
            let source = py_index(&r, "source", V_PUBLIC_ROW)?;
            let from = or(
                py_get(&r, "allowFrom", V_PUBLIC_ROW)?,
                Value::Array(Vec::new()),
            );
            let from = py_join(&from, V_PUBLIC_ROW)?;
            let why = py_get(&r, "why", V_PUBLIC_ROW)?
                .cloned()
                .unwrap_or_else(|| s(""));
            push(
                o,
                &format!(
                    "  {}/{}  {}  from {}  {}",
                    rjust(&py::str_of(&port), 5),
                    ljust(&py::str_of(&proto), 3),
                    ljust(&py::str_of(&source), 11),
                    ljust(&from, 20),
                    py::str_of(&why)
                ),
            );
        }
        let closed = or(
            py_get(d, "knownNotPublic", V_CLOSED_FOR)?,
            Value::Array(Vec::new()),
        );
        for r in py_iter(&closed, V_CLOSED_FOR)? {
            let port = py_index(&r, "port", V_CLOSED_ROW)?;
            let proto = py_index(&r, "proto", V_CLOSED_ROW)?;
            let source = py_index(&r, "source", V_CLOSED_ROW)?;
            let why = py_get(&r, "why", V_CLOSED_ROW)?
                .cloned()
                .unwrap_or_else(|| s(""));
            push(
                o,
                &format!(
                    "  {}/{}  {}  keep closed: {}",
                    rjust(&py::str_of(&port), 5),
                    ljust(&py::str_of(&proto), 3),
                    ljust(&py::str_of(&source), 11),
                    py::str_of(&why)
                ),
            );
        }
        let up = or(py_get(d, "upstream", V_UP)?, Value::Object(Vec::new()));
        // `managedBy` is the wire's value, in every brand.
        let ours = matches!(
            py_get(&up, "managedBy", V_APPLIED_BY)?,
            Some(Value::Str(t)) if *t == Text::from("k2")
        );
        let by = if ours {
            concat!(product_name!(), " at your provider").to_string()
        } else {
            format!("nobody ({CLI} firewall rules)")
        };
        push(o, &format!("applied by: {by}"));
    } else if py_in(d, "guard", V_GUARD)? && py_in(d, "counters", V_GUARD)? {
        let g = py_index(d, "guard", V_G)?;
        let on = if py::truthy(&at(&g, "enabled", V_G_LINE)?) {
            "on"
        } else {
            "off"
        };
        let conns = at(&g, "connsPerIp", V_G_LINE)?;
        let writes = at(&g, "writesPerMin", V_G_LINE)?;
        push(
            o,
            &format!(
                "App guard: {on}, {} connections per visitor, {} writes per visitor per App a minute",
                py::str_of(&conns),
                py::str_of(&writes)
            ),
        );
        let apps = or(py_get(&g, "apps", V_APPS)?, Value::Object(Vec::new()));
        let mut pairs = match &apps {
            Value::Object(pairs) => pairs.clone(),
            other => return Err(Raise::no_attribute(V_APPS, other, "items")),
        };
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        for (app, a) in &pairs {
            let state = on_off(&at(a, "enabled", V_APP)?, V_APP)?;
            let tail = match py_get(a, "writesPerMin", V_APP)? {
                Some(w) if py::truthy(w) => format!(", {} writes a minute", py::str_of(w)),
                _ => String::new(),
            };
            push(
                o,
                &format!("  {}: {state}{tail}", py::str_of(&Value::Str(app.clone()))),
            );
        }
    } else if py_in(d, "upstream", V_UPSTREAM)? {
        let up = py_index(d, "upstream", V_UP_D)?;
        let a = or(py_get(&up, "applied", V_A)?, Value::Object(Vec::new()));
        let managed = at(&up, "managedBy", V_MANAGED)?;
        push(o, &format!("managed by: {}", py::str_of(&managed)));
        if py::truthy(&a) {
            let hash = at(&a, "hash", V_APPLIED)?;
            let provider = at(&a, "provider", V_APPLIED)?;
            let which = if py::truthy(&at(&a, "current", V_APPLIED)?) {
                "this list"
            } else {
                "an older list"
            };
            push(
                o,
                &format!(
                    "applied   : {} at {} ({which})",
                    py::str_of(&hash),
                    py::str_of(&provider)
                ),
            );
        }
    } else {
        for k in ["opened", "closed"] {
            if let Some(v) = py_get(d, k, V_OPENED)?.filter(|v| py::truthy(v)) {
                push(o, &format!("{k} {}", py::str_of(v)));
            }
        }
        if let Some(ssh) = py_get(d, "ssh", V_SSH)?.filter(|v| py::truthy(v)) {
            let port = at(ssh, "port", V_SSH_LINE)?;
            let from = or(
                py_get(ssh, "allowFrom", V_SSH_LINE)?,
                Value::Array(Vec::new()),
            );
            let from = py_join(&from, V_SSH_LINE)?;
            let from = if from.is_empty() {
                "anyone".into()
            } else {
                from
            };
            push(o, &format!("ssh: port {} from {from}", py::str_of(&port)));
        }
        if let Some(note) = py_get(d, "note", V_NOTE)?.filter(|v| py::truthy(v)) {
            push(o, &format!("note: {}", plus_str(note, V_NOTE_LINE)?));
        }
        if let Some(hash) = py_get(d, "hash", V_HASH)?.filter(|v| py::truthy(v)) {
            push(
                o,
                &format!(
                    "view hash now {} ({} Web applies it on its next pass, where it manages this server)",
                    py::str_of(hash),
                    product_name!()
                ),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
