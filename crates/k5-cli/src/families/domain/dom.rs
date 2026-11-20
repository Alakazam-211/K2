//! What `k5 cert`, `k5 domain` and `k5 dns` share: the shell CLI's frame
//! around one request whose answer goes to a Python reader through the
//! environment (`cli/src/domains-certs.sh` and `cli/src/dns.sh`), and the
//! reader `cert` and `domain` share, `_dom_handle_resp`.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]).
//! 2. All three are the locked `dns` tool, an `id` tool, so with
//!    `HOOK_SOCK` set: the stale-passport or require-passport lines, exit 3
//!    ([`crate::prelude::id_tool`]).
//! 3. The words ([`Act`]): a help page, or the usage line
//!    (`_dom_err usage <hint>`, `_dns_err usage <hint>`: one line of
//!    compact JSON on stderr, exit 2, nothing sent), or the call.
//! 4. One `RESP=$(cli_send <fn> <endpoint> ...) || RESP=""`: the cell socket
//!    first where it serves the route, then TCP. `$(…)` drops the answer's
//!    NULs and trailing newlines ([`held`]). A call with no answer is the
//!    transport's line and exit 1 (`_k2_end_command`); any other failure
//!    leaves `RESP` empty for the reader.
//! 5. The reader on `RESP`, which the shell hands to `python3` in an
//!    environment variable.
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! A command line with `-h` or `--help` is the front's ([`crate::front`]).
//! A word that is not UTF-8, `HOOK_SOCK` set with a `CODEX_` variable
//! exported, a port that is set but is no port curl takes, and a
//! `python3` the binary can't tell are handed to the shell, as for
//! [`crate::families::connections`].
//!
//! # Where the binary differs
//!
//! - **A large answer.** The shell puts the answer in the environment of
//!   its `python3`. Linux takes at most 128 KiB in one variable, so there
//!   an answer above that stops the shell's `python3` before it starts
//!   (`Argument list too long`, exit 126). The binary never puts an
//!   answer in its environment or its arguments and prints the whole
//!   list, as the shell does on macOS, which has no such limit.
//! - An answer that is not UTF-8 is read here with U+FFFD for each bad
//!   byte (Python reads the variable with surrogate escapes), and a string
//!   with half of a surrogate pair is printed as JSON escapes where
//!   Python writes them out raw or fails. The daemon sends neither.
//! - Where Python raises, the traceback is Python 3.12's, without source
//!   lines.

use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::families::{curl_port, get, py_iter, py_space, raised, without_nuls, Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::text;
use crate::transport::{Call, Error};
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};
use k2_client::hosts::ZONE;
use std::ffi::OsString;
use std::path::Path;

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`):
/// `dns`, `domain` and `cert` are all `dns`.
pub const TOOL: &str = "dns";

/// What the words ask for, once the gates have passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// `cmd_help_<…>; exit 0`: the page `cli/text/help/<key>.txt`.
    Help(&'static str),
    /// `_dom_err usage "<hint>"; exit 2` (or `_dns_err`): nothing is sent.
    Usage(String),
    /// A form only the shell answers: handed to it before anything is
    /// printed or sent.
    Shell,
    /// `cli_send cli_request <endpoint> <query…>`.
    Get {
        endpoint: &'static str,
        query: Vec<String>,
        json: bool,
        verb: &'static str,
    },
    /// `cli_send cli_post_json <endpoint> "$body"`.
    Post {
        endpoint: &'static str,
        body: String,
        json: bool,
        verb: &'static str,
    },
}

/// A family's Python reader: `RESP` as Python reads it, `AS_JSON` and
/// `D_VERB`.
pub type Reader = fn(raw: &str, as_json: bool, verb: &str) -> Shown;

/// `cmd_<family> <words>`: `args` is the whole command line after
/// `argv[0]`, `parse` reads the words after the family's own. `None`:
/// the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString], parse: fn(&[&str]) -> Act, read: Reader) -> Option<Exit> {
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
    let act = parse(&words);
    if act == Act::Shell || !python::family_native(python::here()) {
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
    Some(run_with(args, act, read, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    act: Act,
    read: Reader,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let (endpoint, get, body, as_json, verb) = match act {
        Act::Help(key) => return help(key),
        Act::Shell => unreachable!("run hands it to the shell"),
        Act::Usage(hint) => {
            out::stderr_line(&err_line("usage", &hint));
            return Exit::Usage;
        }
        Act::Get {
            endpoint,
            query,
            json,
            verb,
        } => (endpoint, Some(query), String::new(), json, verb),
        Act::Post {
            endpoint,
            body,
            json,
            verb,
        } => (endpoint, None, body, json, verb),
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
    let mut closing = Closing::default();
    let passport = client.passport().is_some();
    let answer = match &get {
        Some(query) => {
            let refs: Vec<&str> = query.iter().map(String::as_str).collect();
            client.send(endpoint, Call::Request(&refs))
        }
        None => client.send(endpoint, Call::PostJson(body.as_bytes())),
    };
    let exit = match answer {
        Ok(r) => {
            closing.answer(&r, passport);
            read(&String::from_utf8_lossy(&held(&r.body)), as_json, verb).print()
        }
        Err(
            e @ (Error::NoAnswer(_) | Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. }),
        ) => {
            out::stderr_line(&e.to_string());
            Exit::Error
        }
        Err(_) => read("", as_json, verb).print(),
    };
    closing.flush(&mut client, exit);
    exit
}

/// `cat <<'EOF'` of a help page, exit 0.
fn help(key: &str) -> Exit {
    match text::help(key) {
        Some(page) => match out::stdout(page) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        },
        None => {
            out::stderr_line(&format!("Error: no help page {key}"));
            Exit::Error
        }
    }
}

/// What `RESP=$(…)` holds of an answer: NULs and trailing newlines gone.
pub fn held(body: &[u8]) -> Vec<u8> {
    let mut b = without_nuls(body);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    b
}

/// `_dom_err` and `_dns_err`: `{"error":{"code":…,"hint":…}}`, compact,
/// as Python writes it with `ensure_ascii=False`.
pub fn err_line(code: &str, hint: &str) -> String {
    dumps(&obj(vec![(
        "error",
        obj(vec![("code", s(code)), ("hint", s(hint))]),
    )]))
}

/// `json.dumps({"<key>": <value>})`: Python's default layout (`, ` and
/// `: `, `ensure_ascii`), the body the shell's `python3 -c` prints.
pub fn body(pairs: Vec<(&str, Value)>) -> String {
    json::to_string(&obj(pairs), &Options::default()).unwrap_or_default()
}

/// The readers' `dumps`: compact, `ensure_ascii=False`. A lone surrogate
/// is written as its JSON escape (see the module's notes).
pub fn dumps(v: &Value) -> String {
    let plain = Options {
        compact: true,
        ensure_ascii: false,
        ..Options::default()
    };
    json::to_string(v, &plain)
        .or_else(|_| {
            json::to_string(
                v,
                &Options {
                    compact: true,
                    ..Options::default()
                },
            )
        })
        .unwrap_or_default()
}

/// A string value.
pub fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// An object with these keys, in order.
pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `d.get(key)` where `d` is a dict for sure; `None` for anything else.
pub fn at<'a>(d: &'a Value, key: &str) -> Option<&'a Value> {
    match d {
        Value::Object(pairs) => get(pairs, key),
        _ => None,
    }
}

/// `bool(d.get(key))`.
pub fn truthy_at(d: &Value, key: &str) -> bool {
    at(d, key).is_some_and(py::truthy)
}

/// `str(d.get(key) or default)`.
pub fn or_s(v: Option<&Value>, default: &str) -> String {
    match v {
        Some(v) if py::truthy(v) => py::str_of(v),
        _ => default.to_string(),
    }
}

/// `d.get(key) == "<text>"`.
pub fn is_str(v: Option<&Value>, text: &str) -> bool {
    matches!(v, Some(Value::Str(t)) if t.as_str() == Some(text))
}

/// `str.strip()`.
pub fn strip(text: &str) -> &str {
    text.trim_matches(py_space)
}

/// `x in frozenset({...})` for the code a reader's `die` got: `None` when
/// Python can't hash it (a list or a dict), with the type's name.
pub fn code_in(code: &Value, set: &[&str]) -> Result<bool, &'static str> {
    match code {
        Value::Array(_) => Err("list"),
        Value::Object(_) => Err("dict"),
        Value::Str(t) => Ok(t.as_str().is_some_and(|c| set.contains(&c))),
        _ => Ok(false),
    }
}

/// What a reader printed, line by line.
#[derive(Debug, Default)]
pub struct Out {
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn line(&mut self, text: &str) {
        self.stdout.push_str(text);
        self.stdout.push('\n');
    }

    /// The end: what was printed and `exit`, or the traceback after it.
    pub fn end(self, r: Result<Exit, Raise>) -> Shown {
        match r {
            Ok(exit) => Shown {
                stdout: self.stdout,
                stderr: self.stderr,
                exit,
            },
            Err(r) => {
                let mut shown = raised(self.stdout, self.stderr, &r);
                // `python3 -` reads its script from stdin.
                shown.stderr = shown
                    .stderr
                    .replace("  File \"<string>\"", "  File \"<stdin>\"");
                shown
            }
        }
    }
}

/// The lines of `_dom_handle_resp`'s Python, as `python3 -` numbers them
/// (`tests` pins them).
pub mod line {
    pub const DIE_CODE: u32 = 29;
    pub const ERR: u32 = 47;
    pub const DIE_ERR: u32 = 53;
    pub const PENDING_NS: u32 = 85;
    pub const ADD_PENDING: u32 = 133;
    pub const REFRESH_ROWS: u32 = 137;
    pub const REFRESH_PENDING: u32 = 142;
    pub const LIST_FOR: u32 = 154;
    pub const LIST_PENDING: u32 = 163;
    pub const LIST_NAMES: u32 = 164;
    pub const LIST_CERT: u32 = 167;
    pub const CERTS_FOR: u32 = 177;
    pub const CERTS_CERT: u32 = 181;
}

const GATED: &[&str] = &[
    "gated",
    "forbidden",
    "dns_manage_disabled",
    "owner_only",
    "dns_zone_not_attached",
];
const USAGE: &[&str] = &[
    "usage",
    "invalid_domain",
    "invalid_role",
    "hostname_not_under_apex",
];

/// The daemon's answer to nothing at all.
pub const UNREACHABLE: &str = concat!(
    "no response from the ",
    k2_client::product_name!(),
    " daemon (is it running?)"
);

/// `die(err)` of `_dom_handle_resp`, called from line `from`: the error
/// line on stderr and the exit (3 for a gate, 2 for usage, else 1), or
/// the `TypeError` when the code can't be hashed.
fn die(o: &mut Out, err: &Value, from: u32) -> Result<Exit, Raise> {
    let code = at(err, "code").cloned().unwrap_or_else(|| s("error"));
    let hint = at(err, "hint").cloned().unwrap_or_else(|| s(""));
    o.stderr.push_str(&dumps(&obj(vec![(
        "error",
        obj(vec![("code", code.clone()), ("hint", hint)]),
    )])));
    o.stderr.push('\n');
    let unhashable = |t: &str| {
        Raise::inside(
            from,
            line::DIE_CODE,
            "die",
            format!("TypeError: unhashable type: '{t}'"),
        )
    };
    if code_in(&code, GATED).map_err(unhashable)? {
        return Ok(Exit::Refused);
    }
    if code_in(&code, USAGE).map_err(unhashable)? {
        return Ok(Exit::Usage);
    }
    Ok(Exit::Error)
}

/// The steps every reader takes first (`_dom_handle_resp` and
/// `_dns_handle_resp` have them the same, but `die`): an empty answer,
/// one that is not JSON, the daemon's error, `"ok": false`. `Ok(Ok(d))`:
/// the answer, read on.
pub fn first_steps(
    o: &mut Out,
    raw: &str,
    die: fn(&mut Out, &Value, u32) -> Result<Exit, Raise>,
    lines: (u32, u32),
) -> Result<Result<Value, Exit>, Raise> {
    let (err_line, die_err) = lines;
    if strip(raw).is_empty() {
        let e = obj(vec![
            ("code", s("daemon_unreachable")),
            ("hint", s(UNREACHABLE)),
        ]);
        return die(o, &e, 0).map(Err);
    }
    let Ok(d) = json::parse(raw) else {
        let head: String = raw.chars().take(200).collect();
        let hint = format!(
            "daemon returned an unparseable response: {}",
            py::repr(&Value::Str(Text::from(head)))
        );
        let e = obj(vec![("code", s("bad_response")), ("hint", s(&hint))]);
        return die(o, &e, 0).map(Err);
    };
    let Value::Object(pairs) = &d else {
        return Err(Raise::no_attribute(err_line, &d, "get"));
    };
    match get(pairs, "error") {
        None | Some(Value::Null) => {}
        Some(err) => {
            let err = match err {
                Value::Object(_) => err.clone(),
                Value::Str(_) => obj(vec![("code", s("error")), ("hint", err.clone())]),
                other => obj(vec![("code", s("error")), ("hint", s(&py::str_of(other)))]),
            };
            return die(o, &err, die_err).map(Err);
        }
    }
    if matches!(get(pairs, "ok"), Some(Value::Bool(false))) {
        let text: String = dumps(&d).chars().take(200).collect();
        let e = obj(vec![
            ("code", s("error")),
            ("hint", s(&format!("unexpected daemon response: {text}"))),
        ]);
        return die(o, &e, 0).map(Err);
    }
    Ok(Ok(d))
}

/// `_dom_handle_resp` (`cli/src/domains-certs.sh`) on `raw`, for `cert`
/// and `domain`.
pub fn read(raw: &str, as_json: bool, verb: &str) -> Shown {
    let mut o = Out::default();
    let r = read_into(&mut o, raw, as_json, verb);
    o.end(r)
}

fn read_into(o: &mut Out, raw: &str, as_json: bool, verb: &str) -> Result<Exit, Raise> {
    let d = match first_steps(o, raw, die, (line::ERR, line::DIE_ERR))? {
        Ok(d) => d,
        Err(exit) => return Ok(exit),
    };
    if as_json {
        o.line(&dumps(&d));
        return Ok(Exit::Ok);
    }
    match verb {
        "add" => {
            let row = match at(&d, "domain") {
                Some(r @ Value::Object(_)) => r.clone(),
                _ => Value::Object(Vec::new()),
            };
            o.line(&format!(
                "attached {} \u{2014} {}",
                or_s(at(&row, "apex"), "?"),
                state_words(&row)
            ));
            pending_lines(o, &row, line::ADD_PENDING)?;
        }
        "refresh" => {
            let all = py_iter(&or_list(at(&d, "domains")), line::REFRESH_ROWS)?;
            let rows: Vec<&Value> = all
                .iter()
                .filter(|r| matches!(r, Value::Object(_)))
                .collect();
            if rows.is_empty() {
                o.line("(no pending domains to check)");
            }
            for row in rows {
                o.line(&format!(
                    "{}  {}",
                    crate::families::ljust(&or_s(at(row, "apex"), "?"), 28),
                    state_words(row)
                ));
                pending_lines(o, row, line::REFRESH_PENDING)?;
            }
            if matches!(at(&d, "checked"), Some(Value::Bool(false))) {
                o.line(&format!(
                    "not checked: {}",
                    or_s(at(&d, "hint"), NO_TUNNEL_TOKEN)
                ));
            }
        }
        "list" => {
            let domains = at(&d, "domains").filter(|v| py::truthy(v));
            let Some(domains) = domains else {
                o.line("(no custom domains attached to this server)");
                o.line(NO_DOMAINS_HINT);
                return Ok(Exit::Ok);
            };
            o.line(&format!(
                "{}  {}  ZONE",
                crate::families::ljust("APEX", 28),
                crate::families::ljust("DNS", 8)
            ));
            for row in py_iter(domains, line::LIST_FOR)? {
                if !matches!(row, Value::Object(_)) {
                    o.line(&py::str_of(&row));
                    continue;
                }
                o.line(&format!(
                    "{}  {}  {}",
                    crate::families::ljust(&or_s(at(&row, "apex"), "?"), 28),
                    crate::families::ljust(dns_label(&row), 8),
                    or_s(at(&row, "zoneId"), "-")
                ));
                pending_lines(o, &row, line::LIST_PENDING)?;
                for n in py_iter(&or_list(at(&row, "names")), line::LIST_NAMES)? {
                    if !matches!(n, Value::Object(_)) {
                        continue;
                    }
                    let cert = cert_state(&n, line::LIST_CERT)?;
                    o.line(&format!(
                        "  {}  {}  cert {cert}{}{}",
                        crate::families::ljust(&or_s(at(&n, "hostname"), "?"), 26),
                        crate::families::ljust(&or_s(at(&n, "role"), "other"), 8),
                        renew_suffix(&n),
                        app_suffix(&n)
                    ));
                }
            }
        }
        "certs" => {
            let certs = at(&d, "certs").filter(|v| py::truthy(v));
            let Some(certs) = certs else {
                o.line("(no attached hostnames)");
                return Ok(Exit::Ok);
            };
            o.line(&format!(
                "{}  {}  CERT",
                crate::families::ljust("HOSTNAME", 32),
                crate::families::ljust("ROLE", 8)
            ));
            for c in py_iter(certs, line::CERTS_FOR)? {
                if !matches!(c, Value::Object(_)) {
                    o.line(&py::str_of(&c));
                    continue;
                }
                let cert = cert_state(&c, line::CERTS_CERT)?;
                o.line(&format!(
                    "{}  {}  {cert}{}{}",
                    crate::families::ljust(&or_s(at(&c, "hostname"), "?"), 32),
                    crate::families::ljust(&or_s(at(&c, "role"), "other"), 8),
                    renew_suffix(&c),
                    app_suffix(&c)
                ));
            }
        }
        "remove" | "name-add" | "name-remove" | "issue" | "renew" => {
            let Value::Object(pairs) = &d else {
                unreachable!("first_steps took only a dict");
            };
            let rest: Vec<(Text, Value)> = pairs
                .iter()
                .filter(|(k, _)| k.as_str() != Some("ok"))
                .cloned()
                .collect();
            if rest.is_empty() {
                o.line(&dumps(&d));
            } else {
                o.line(&dumps(&Value::Object(rest)));
            }
        }
        _ => o.line(&dumps(&d)),
    }
    Ok(Exit::Ok)
}

/// `refresh`'s line when the server didn't ask the zone's host.
const NO_TUNNEL_TOKEN: &str = concat!("no ", k2_client::product_name!(), " Connect tunnel token");

/// `list`'s second line when no domain is attached.
const NO_DOMAINS_HINT: &str = concat!(
    "hint: ",
    k2_client::cli_name!(),
    " domain add <apex> \u{2014} attach \u{2260} nameserver transfer"
);

/// `x or []`.
fn or_list(v: Option<&Value>) -> Value {
    v.filter(|x| py::truthy(x))
        .cloned()
        .unwrap_or(Value::Array(Vec::new()))
}

/// `(n.get("cert") or {}).get("state") or "missing"`, or the
/// `AttributeError` at `line` for a `cert` that is set and not a dict.
fn cert_state(n: &Value, line: u32) -> Result<String, Raise> {
    match at(n, "cert") {
        Some(c @ Value::Object(_)) => Ok(or_s(at(c, "state"), "missing")),
        Some(c) if py::truthy(c) => Err(Raise::no_attribute(line, c, "get")),
        _ => Ok("missing".to_string()),
    }
}

/// `is_pending(row)`.
fn is_pending(row: &Value) -> bool {
    is_str(at(row, "status"), "pending_ns")
}

/// `dns_label(row)`.
fn dns_label(row: &Value) -> &'static str {
    if is_pending(row) {
        "pending"
    } else if truthy_at(row, "readOnly") {
        "readonly"
    } else if truthy_at(row, "dnsWrite") {
        "write"
    } else {
        "byo"
    }
}

/// `state_words(row)`. "<product>-hosted" is prose people read, so it
/// follows the brand like any other sentence.
fn state_words(row: &Value) -> String {
    let hosted = format!("{}-hosted DNS", PRODUCT_NAME);
    if is_pending(row) {
        "pending nameservers".to_string()
    } else if truthy_at(row, "readOnly") {
        format!(
            "{hosted} (read-only: {} on {ZONE})",
            or_s(at(row, "status"), "on hold")
        )
    } else if truthy_at(row, "dnsWrite") {
        format!("{hosted} (writable)")
    } else {
        "BYO DNS (inventory only)".to_string()
    }
}

/// `pending_lines(row)`, called from line `from`.
fn pending_lines(o: &mut Out, row: &Value, from: u32) -> Result<(), Raise> {
    let apex = or_s(at(row, "apex"), "<apex>");
    if truthy_at(row, "zoneMissing") {
        o.line(&format!(
            "  Not found on {ZONE} for this server (removed, or moved to another account) \u{2014} {CLI_NAME} domain refresh {apex}, or remove and add it again"
        ));
    }
    if truthy_at(row, "readOnly") {
        o.line(&format!(
            "  Read-only: {} on {ZONE} \u{2014} records serve, writes are refused (zone_readonly)",
            or_s(at(row, "status"), "on hold")
        ));
    }
    if !is_pending(row) {
        return Ok(());
    }
    let ns: Vec<String> = py_iter(&or_list(at(row, "nameservers")), line::PENDING_NS)
        .map_err(|r| Raise::inside(from, line::PENDING_NS, "pending_lines", r.exception))?
        .iter()
        .filter(|n| py::truthy(n))
        .map(py::str_of)
        .collect();
    let to = if ns.is_empty() {
        format!("the {ZONE} nameservers (see your {ZONE} dashboard)")
    } else {
        ns.join(", ")
    };
    o.line(&format!("  Pending: point nameservers to {to}"));
    if truthy_at(row, "created") {
        o.line(&format!("  Auto-added to your {ZONE} account"));
    }
    o.line(&format!(
        "  This server re-checks with {ZONE} by itself; to check now: {CLI_NAME} domain refresh {apex}"
    ));
    Ok(())
}

/// `renew_suffix(row)`.
fn renew_suffix(row: &Value) -> String {
    let Some(r @ Value::Object(_)) = at(row, "renewal") else {
        return String::new();
    };
    if !truthy_at(r, "k2Issued") {
        return String::new();
    }
    let mut out = format!("  renew {}", or_s(at(r, "state"), "?"));
    if truthy_at(r, "failures") {
        out.push_str(&format!(
            " ({} failed; next {})",
            or_s(at(r, "failures"), ""),
            or_s(at(r, "nextAttempt"), "?")
        ));
    }
    out
}

/// `app_suffix(row)`.
fn app_suffix(row: &Value) -> String {
    let Some(a @ Value::Object(_)) = at(row, "app") else {
        return String::new();
    };
    if is_str(at(a, "error"), "unknown_app") {
        return format!("  (not served: no App {} here)", or_s(at(a, "app"), "?"));
    }
    if truthy_at(a, "external") {
        return format!(
            concat!(
                "  (served by your own web server, not by ",
                k2_client::product_name!(),
                "{})"
            ),
            if truthy_at(a, "primary") {
                "; primary"
            } else {
                ""
            }
        );
    }
    let mut out = if truthy_at(a, "notServedYet") {
        format!(
            "  (not served yet: {})",
            or_s(at(a, "state"), "waiting_dns")
        )
    } else {
        format!(
            "  (served: live{})",
            if truthy_at(a, "primary") {
                ", primary"
            } else {
                ""
            }
        )
    };
    if is_str(at(a, "served"), "elsewhere") {
        out.push_str("  :443 answers with another server's certificate");
    }
    if truthy_at(a, "warning") {
        out.push_str(&format!("  warning: {}", or_s(at(a, "warning"), "")));
    }
    out
}

/// The words of `cmd_<x>_list`: `--json` only. `Err`: the usage hint.
pub fn json_only(rest: &[&str], usage: &str) -> Result<bool, String> {
    let mut json = false;
    for w in rest {
        match *w {
            "--json" => json = true,
            w if w.starts_with('-') => {
                return Err(format!("unknown flag '{w}' \u{2014} usage: {usage}"))
            }
            w => return Err(format!("unexpected argument '{w}' \u{2014} usage: {usage}")),
        }
    }
    Ok(json)
}

/// The words of a verb that takes one positional and `--json`: the
/// positional (empty when none came) and `--json`. `Err`: the usage hint.
/// The first word that is not empty is the positional; an empty one
/// before it is not counted (`[ -z "$x" ]`).
pub fn one_arg(rest: &[&str], flag_usage: &str, extra: &str) -> Result<(String, bool), String> {
    let mut one = String::new();
    let mut json = false;
    for w in rest {
        match *w {
            "--json" => json = true,
            w if w.starts_with('-') => {
                return Err(format!("unknown flag '{w}' \u{2014} usage: {flag_usage}"))
            }
            w if one.is_empty() => one = w.to_string(),
            w => return Err(format!("unexpected extra argument '{w}'{extra}")),
        }
    }
    Ok((one, json))
}

#[cfg(test)]
pub mod tests;
