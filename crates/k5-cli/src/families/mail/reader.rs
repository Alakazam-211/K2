//! `k5 mail reader`: the mail reader for people, as the shell CLI's
//! `cmd_mail_reader` (`cli/src/mail-reader-and-help.sh`) answers it. The
//! Owner and Admins open every inbox on the server, anyone else only
//! their own mailbox; agents are refused by the daemon (exit 3).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings, the gates and the `id` tool's checks, as every `mail`
//!    verb ([`super::core::run`]): `mail reader` is the `mail` tool, so the
//!    credential is the one `_mail_py` sends ([`Opener::from_client`]): in
//!    a session its passport, never the owner token from disk.
//! 2. The words ([`parse`]): `mail reader` alone, `help` or an empty word
//!    is the help page; a mistake is `_mail_err "usage" …`, exit 2, and
//!    nothing is sent.
//! 3. The reader's own Python ([`exec`]): one `urlopen` per call (no
//!    tries), its own `die` (exit 3 for `owner_only`, `forbidden`, `gated`,
//!    `not_allowed` and `role_required`, 2 for `usage`, else 1, and no
//!    permission card), its printers.
//!
//! `--body -` reads stdin as `$(cat)` does: its trailing newlines dropped.
//! It is read only once the binary answers the line, so a line handed to
//! the shell still has it.
//!
//! Where this differs from the shell:
//!
//! - A NUL byte on stdin for `--body -` is dropped silently; bash drops it
//!   with a warning that names `cli/k2`'s path and line.
//! - A traceback (an answer the Python raises on) has the script's frames
//!   only, none from the standard library, and only the last exception of
//!   a chain.
//! - `K5_MAIL_HTTP_TIMEOUT` of 0, of infinity, past what a socket takes,
//!   or not UTF-8 is the shell's ([`parse`] hands the line back).

use super::core::{self, usage, End, Env, Plan, Py, R};
use super::py::{self as p, attr_get, iter, py_index};
use super::reads::b64decode;
use crate::exit::Exit;
use crate::out::json::{Text, Value};
use crate::out::pylib::json as pyjson;
use crate::out::pylib::time as pytime;
use crate::out::{self, py};
use crate::transport::urllib::{Method, Outcome, Request};
use k2_client::brand::CLI_NAME;
use k2_client::product_name;
use std::time::Duration;

/// `M_VERB` for the reader's Python.
pub const VERB: &str = "mail_reader";

/// The reader's verbs, in `cmd_mail_reader`'s order.
pub const SUBS: &[&str] = &[
    "inboxes",
    "folders",
    "messages",
    "show",
    "thread",
    "attachment",
    "log",
    "flag",
    "move",
    "archive",
    "delete",
    "reply",
    "draft",
    "send",
    "drafts",
    "draft-show",
    "draft-save",
    "draft-send",
    "draft-discard",
];

/// The flags that take a value.
const VALUE_FLAGS: &[&str] = &[
    "--reason",
    "--folder",
    "--query",
    "--from",
    "--since",
    "--before",
    "--limit",
    "--offset",
    "--out",
    "--inbox",
    "--actor",
    "--body",
    "--to",
    "--cc",
    "--subject",
];

/// The route prefix.
const R_: &str = "/cli/mail/reader/";

/// The codes `die` exits 3 for.
const NEEDS_HUMAN: &[&str] = &[
    "owner_only",
    "forbidden",
    "gated",
    "not_allowed",
    "role_required",
];

/// The lines of the reader's Python (line 1 is `import base64, …`) where
/// it calls or raises. `tests` pins each to its statement.
pub mod line {
    pub const CALL_DATA: u32 = 31;
    pub const CALL_HTTP_ERROR: u32 = 45;
    pub const CALL_ERROR: u32 = 50;
    pub const CALL_OK: u32 = 52;
    pub const WHO_NAME: u32 = 62;
    pub const DRAFT_AUTHOR: u32 = 69;
    pub const DRAFT_BY: u32 = 70;
    pub const DRAFT_KIND: u32 = 71;
    pub const DRAFT_EDITED: u32 = 72;
    pub const DRAFT_EDITED_BY: u32 = 74;
    pub const DRAFT_ID: u32 = 77;
    pub const DRAFT_HINT: u32 = 78;
    pub const MSG_DRAFT: u32 = 82;
    pub const MSG_HEAD: u32 = 83;
    pub const MSG_FROM: u32 = 84;
    pub const MSG_TO_IF: u32 = 85;
    pub const MSG_TO: u32 = 86;
    pub const MSG_CC_IF: u32 = 87;
    pub const MSG_CC: u32 = 88;
    pub const MSG_DATE: u32 = 89;
    pub const MSG_SUBJECT: u32 = 90;
    pub const MSG_ID_IF: u32 = 91;
    pub const MSG_ID: u32 = 92;
    pub const MSG_AUTH: u32 = 93;
    pub const MSG_ANY: u32 = 94;
    pub const MSG_CHECKS: u32 = 95;
    pub const MSG_ATTS: u32 = 96;
    pub const MSG_ATT: u32 = 97;
    pub const MSG_HTML_IF: u32 = 99;
    pub const MSG_HTML: u32 = 100;
    pub const MSG_BLOCKED: u32 = 101;
    pub const MSG_TEXT: u32 = 104;
    pub const INBOXES_CALL: u32 = 108;
    pub const INBOXES_ROWS: u32 = 111;
    pub const INBOXES_NONE: u32 = 113;
    pub const INBOXES_FOR: u32 = 115;
    pub const INBOXES_OWNER: u32 = 116;
    pub const INBOXES_TAG: u32 = 117;
    pub const INBOXES_ROW: u32 = 118;
    pub const FOLDERS_CALL: u32 = 120;
    pub const FOLDERS_FOR: u32 = 123;
    pub const FOLDERS_ROW: u32 = 124;
    pub const MESSAGES_CALL: u32 = 126;
    pub const MESSAGES_ITEMS: u32 = 131;
    pub const MESSAGES_FOR: u32 = 134;
    pub const MESSAGES_ROW: u32 = 135;
    pub const MESSAGES_NEXT: u32 = 137;
    pub const MESSAGES_MORE: u32 = 138;
    pub const SHOW_CALL: u32 = 140;
    pub const SHOW_PRINT: u32 = 143;
    pub const THREAD_CALL: u32 = 145;
    pub const THREAD_FOR: u32 = 148;
    pub const THREAD_PRINT: u32 = 151;
    pub const ATT_CALL: u32 = 160;
    pub const ATT_DATA: u32 = 161;
    pub const ATT_OPEN: u32 = 162;
    pub const ATT_PRINT: u32 = 167;
    pub const LOG_CALL: u32 = 169;
    pub const LOG_ROWS: u32 = 172;
    pub const LOG_FOR: u32 = 175;
    pub const LOG_AT: u32 = 176;
    pub const LOG_ROW: u32 = 177;
    pub const LOG_REASON: u32 = 178;
    pub const DRAFTS_CALL: u32 = 182;
    pub const DRAFTS_ROWS: u32 = 185;
    pub const DRAFTS_FOR: u32 = 188;
    pub const DRAFTS_AUTHOR: u32 = 189;
    pub const DRAFTS_EDITED: u32 = 190;
    pub const DRAFTS_ROW: u32 = 191;
    pub const DRAFTS_EDITED_BY: u32 = 192;
    pub const DRAFTS_STATE: u32 = 194;
    pub const DRAFT_SHOW_CALL: u32 = 198;
    pub const DRAFT_SHOW_PRINT: u32 = 201;
    pub const DRAFT_STATE_CALL: u32 = 215;
    pub const DRAFT_SEND_STATUS: u32 = 219;
    pub const DRAFT_SEND_PRINT: u32 = 220;
    pub const ACT_CALL: u32 = 244;
    pub const ACT_STATUS: u32 = 247;
    pub const ACT_SEND: u32 = 249;
    pub const ACT_MOVE: u32 = 251;
    pub const ACT_DRAFT_IF: u32 = 252;
    pub const ACT_DRAFT: u32 = 253;
}

/// The words after `mail reader`, as `cmd_mail_reader` reads them.
pub fn parse(_reader: &str, words: &[&str]) -> Plan {
    let sub = words.first().copied().unwrap_or("");
    if matches!(sub, "--help" | "-h" | "help" | "") {
        return Plan::Help("mail/reader");
    }
    if !SUBS.contains(&sub) {
        return usage(format!(
            "unknown command '{sub}' — see {CLI_NAME} mail reader --help"
        ));
    }
    let mut e = Env::new()
        .with("M_VERB", VERB)
        .with("MR_SUB", sub)
        .with("MR_JSON", "false")
        .with("MR_UNREAD", "false")
        .with("MR_HTML", "false")
        .with("MR_BODY_SET", "false");
    let (mut to, mut cc) = (String::new(), String::new());
    // `--body -`: the first `$(cat)` reads stdin; any later one reads
    // nothing.
    let mut stdin_read = false;
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 1;
    while let Some(w) = words.get(i).copied() {
        match w {
            "--help" | "-h" => return Plan::Help("mail/reader"),
            "--json" => e.set("MR_JSON", "true"),
            "--html" => e.set("MR_HTML", "true"),
            "--unread" if sub == "flag" => e.set("MR_READ", "false"),
            "--unread" => e.set("MR_UNREAD", "true"),
            "--read" => e.set("MR_READ", "true"),
            "--star" => e.set("MR_FLAGGED", "true"),
            "--unstar" => e.set("MR_FLAGGED", "false"),
            f if VALUE_FLAGS.contains(&f) => {
                let Some(v) = words.get(i + 1).copied() else {
                    return usage(format!(
                        "{f} needs a value — see {CLI_NAME} mail reader --help"
                    ));
                };
                match f {
                    "--body" => {
                        if v == "-" {
                            e.set("MR_BODY", "");
                            e.set("MR_BODY_STDIN", if stdin_read { "" } else { "true" });
                            stdin_read = true;
                        } else {
                            e.set("MR_BODY", v);
                            e.set("MR_BODY_STDIN", "");
                        }
                        e.set("MR_BODY_SET", "true");
                    }
                    "--to" => append(&mut to, v),
                    "--cc" => append(&mut cc, v),
                    _ => e.set(env_key(f), v),
                }
                i += 1;
            }
            f if f.starts_with("--") => {
                return usage(format!(
                    "unknown flag '{f}' for '{CLI_NAME} mail reader {sub}' — see {CLI_NAME} mail reader --help"
                ))
            }
            _ => pos.push(w),
        }
        i += 1;
    }
    e.set("MR_TO", to);
    e.set("MR_CC", cc);
    let need = match sub {
        "folders" | "messages" | "show" | "thread" | "flag" | "archive" | "delete" | "reply" => 1,
        "drafts" | "draft-show" | "draft-save" | "draft-send" | "draft-discard" => 1,
        "move" | "attachment" => 2,
        "draft" if e.get("MR_INBOX").is_empty() => 1,
        _ => 0,
    };
    if pos.len() < need {
        return usage(format!(
            "'{CLI_NAME} mail reader {sub}' needs {need} argument(s) — see {CLI_NAME} mail reader --help"
        ));
    }
    if let Some(extra) = pos.get(need) {
        return usage(format!(
            "unexpected argument '{extra}' for '{CLI_NAME} mail reader {sub}' — see {CLI_NAME} mail reader --help"
        ));
    }
    if sub == "attachment" && e.get("MR_OUT").is_empty() {
        return usage("missing --out <file> — where to save the attachment");
    }
    if sub == "flag" && e.get("MR_READ").is_empty() && e.get("MR_FLAGGED").is_empty() {
        return usage("nothing to change — pass --read, --unread, --star or --unstar");
    }
    if matches!(sub, "reply" | "draft" | "send") && !e.on("MR_BODY_SET") {
        return usage("missing --body <text> (or --body - to read stdin)");
    }
    if sub == "send" && e.get("MR_INBOX").is_empty() {
        return usage("missing --inbox <address> — the inbox to send from");
    }
    // `int(a2)` of a number Python reads and this doesn't (digits of
    // another script).
    if sub == "attachment" && !pos[1].is_ascii() {
        return Plan::Shell;
    }
    if timeout().is_err() {
        return Plan::Shell;
    }
    e.set("MR_A1", pos.first().copied().unwrap_or(""));
    e.set("MR_A2", pos.get(1).copied().unwrap_or(""));
    Plan::Run(e)
}

/// `X="${X:+$X\n}$2"`.
fn append(list: &mut String, v: &str) {
    if !list.is_empty() {
        list.push('\n');
    }
    list.push_str(v);
}

fn env_key(flag: &str) -> &'static str {
    match flag {
        "--reason" => "MR_REASON",
        "--folder" => "MR_FOLDER",
        "--query" => "MR_QUERY",
        "--from" => "MR_FROM",
        "--since" => "MR_SINCE",
        "--before" => "MR_BEFORE",
        "--limit" => "MR_LIMIT",
        "--offset" => "MR_OFFSET",
        "--out" => "MR_OUT",
        "--inbox" => "MR_INBOX",
        "--actor" => "MR_ACTOR",
        "--subject" => "MR_SUBJECT",
        other => unreachable!("not a value flag: {other}"),
    }
}

/// What `float(E("K5_MAIL_HTTP_TIMEOUT") or … or 60)` comes to: the
/// seconds, or the `ValueError`'s text `call` turns into its
/// `daemon_unreachable` line (`Ok(Err(text))`). `Err(())`: a value the
/// binary hands to the shell.
fn timeout() -> Result<Result<Duration, String>, ()> {
    let set =
        k2_client::env_names::first_nonempty_os_in(k2_client::env_names::MAIL_HTTP_TIMEOUT, |n| {
            std::env::var_os(n)
        });
    let Some((_, raw)) = set else {
        return Ok(Ok(Duration::from_secs(60)));
    };
    let raw = raw.into_string().map_err(|_| ())?;
    let Some(f) = core::py_float(&raw) else {
        return Ok(Err(format!(
            "could not convert string to float: {}",
            py::repr(&p::sv(&raw))
        )));
    };
    if f.is_nan() {
        return Ok(Err("Invalid value NaN (not a number)".into()));
    }
    if f < 0.0 {
        return Ok(Err("Timeout value out of range".into()));
    }
    // 0 is a non-blocking connect, and a value past what a socket holds
    // raises another error: both the shell's.
    if f == 0.0 || f >= 9.2e9 {
        return Err(());
    }
    Ok(Ok(Duration::from_secs_f64(f)))
}

/// How the reader's Python ended early.
enum Ex {
    /// `die` wrote its line: the exit.
    End(End),
    /// An exception: the frames (line, function), outermost first, and
    /// its last line.
    Raise(Vec<(u32, &'static str)>, String),
}

type X<T> = Result<T, Ex>;

/// An exception raised at `line` of `name`.
fn raise<T>(r: Result<T, String>, line: u32, name: &'static str) -> X<T> {
    r.map_err(|e| Ex::Raise(vec![(line, name)], e))
}

/// The caller's frame on an exception from a function it called.
fn within<T>(r: X<T>, line: u32, name: &'static str) -> X<T> {
    r.map_err(|e| match e {
        Ex::Raise(mut f, x) => {
            f.insert(0, (line, name));
            Ex::Raise(f, x)
        }
        other => other,
    })
}

const M: &str = "<module>";

/// The Python a verb runs, for `M_VERB` [`VERB`]. `None`: not the
/// reader's.
pub fn exec(py_: &mut Py, verb: &str) -> Option<R<()>> {
    if verb != VERB {
        return None;
    }
    Some(match main(py_) {
        Ok(()) => Ok(()),
        Err(Ex::End(e)) => Err(e),
        Err(Ex::Raise(frames, exception)) => {
            let mut s = String::from("Traceback (most recent call last):\n");
            for (line, name) in frames {
                s.push_str(&format!("  File \"<stdin>\", line {line}, in {name}\n"));
            }
            s.push_str(&exception);
            s.push('\n');
            let _ = out::write_flushed(&mut std::io::stderr().lock(), s.as_bytes());
            Err(End::Exit(Exit::Error))
        }
    })
}

/// `die(err)`: the error's line on stderr, and its exit.
fn die(err: &Value) -> Ex {
    let (code, hint) = match err {
        Value::Object(_) => (
            p::get_or(err, "code", p::sv("error")).unwrap_or_else(|_| p::sv("error")),
            p::get_or(err, "hint", p::sv("")).unwrap_or_else(|_| p::sv("")),
        ),
        other => (p::sv("error"), p::sv(&py::str_of(other))),
    };
    let line = core::dumps(&core::obj(vec![(
        "error",
        core::obj(vec![("code", code.clone()), ("hint", hint)]),
    )]));
    out::stderr_line(&line);
    let exit = if NEEDS_HUMAN.iter().any(|c| p::eq_str(&code, c)) {
        Exit::Refused
    } else if p::eq_str(&code, "usage") {
        Exit::Usage
    } else {
        Exit::Error
    };
    Ex::End(End::Exit(exit))
}

/// `die({"code": code, "hint": hint})`.
fn die_with(code: &str, hint: &str) -> Ex {
    die(&core::obj(vec![
        ("code", p::sv(code)),
        ("hint", p::sv(hint)),
    ]))
}

/// The body as `E("MR_BODY")` gives it: the words' text, or stdin read now
/// (`$(cat)`). `Err(bytes)`: stdin that is not UTF-8, which Python holds
/// with its bytes escaped.
fn body_text(py_: &Py) -> Result<String, Vec<u8>> {
    if !py_.env.on("MR_BODY_STDIN") {
        return Ok(py_.env("MR_BODY").to_string());
    }
    let s = crate::prompt::read_all_stdin();
    String::from_utf8(s.expose().to_vec()).map_err(|e| e.into_bytes())
}

/// `call(method, route, params, body)`, from module line `from`.
fn call(
    py_: &Py,
    method: Method,
    route: &str,
    params: &[(&str, String)],
    body: Option<&Value>,
    from: u32,
) -> X<Value> {
    within(call_inner(py_, method, route, params, body), from, M)
}

fn call_inner(
    py_: &Py,
    method: Method,
    route: &str,
    params: &[(&str, String)],
    body: Option<&Value>,
) -> X<Value> {
    const F: &str = "call";
    let q: Vec<(String, String)> = params
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect();
    let timeout = match timeout() {
        Ok(t) => t,
        Err(()) => unreachable!("parse hands such a timeout to the shell"),
    };
    let data = body.map(|b| core::dumps(b).into_bytes());
    let timeout = match timeout {
        Ok(t) => t,
        Err(e) => {
            return Err(die_with(
                "daemon_unreachable",
                &format!("{}{e}", unreachable_head()),
            ))
        }
    };
    let mut req = Request::new(method, route, timeout).query(q);
    if let Some(d) = data {
        req = req.data(d).header("Content-Type", "application/json");
    }
    if let Some(t) = py_.opener.token() {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let d = match py_.opener.open(&req) {
        Outcome::Answer { body: Ok(b), .. } => match pyjson::loads(&String::from_utf8_lossy(&b)) {
            Ok(d) => d,
            Err(f) => {
                return Err(die_with(
                    "daemon_unreachable",
                    &format!("{}{}", unreachable_head(), f.message),
                ))
            }
        },
        Outcome::Answer { body: Err(r), .. } | Outcome::Raised(r) => {
            return Err(die_with(
                "daemon_unreachable",
                &format!("{}{}", unreachable_head(), r.text()),
            ))
        }
        Outcome::HttpError { code, body } => {
            let b = body.map_err(|r| Ex::Raise(vec![(40, F)], r.traceback_line()))?;
            let d = match pyjson::loads(&String::from_utf8_lossy(&b)) {
                Ok(d) => d,
                Err(_) => {
                    return Err(die_with("error", &format!("HTTP {code} from {route}")));
                }
            };
            let e = raise(attr_get(&d, "error"), line::CALL_HTTP_ERROR, F)?;
            if let Some(Value::Str(t)) = &e {
                let code = Value::Str(t.clone());
                let hint = p::or(attr_get(&d, "message").unwrap_or(None), code.clone());
                return Err(die(&core::obj(vec![("code", code), ("hint", hint)])));
            }
            d
        }
    };
    if let Some(e @ Value::Object(_)) = raise(attr_get(&d, "error"), line::CALL_ERROR, F)? {
        return Err(die(&e));
    }
    if !p::truthy(raise(attr_get(&d, "ok"), line::CALL_OK, F)?.as_ref()) {
        let t: String = core::dumps(&d).chars().take(200).collect();
        return Err(die_with("error", &format!("unexpected daemon answer: {t}")));
    }
    Ok(d)
}

/// The `daemon_unreachable` hint before `%s` of the exception.
fn unreachable_head() -> String {
    format!(
        "no response from the {} daemon (is it running?): ",
        product_name!()
    )
}

/// `d.get(k)` at `l` of `name`.
fn g(d: &Value, k: &str, l: u32, name: &'static str) -> X<Option<Value>> {
    raise(attr_get(d, k), l, name)
}

/// `"%s" % v`.
fn s_(v: &Value) -> String {
    py::str_of(v)
}

/// `"%s" % d.get(k)`: `None` when missing.
fn s_get(d: &Value, k: &str, l: u32, name: &'static str) -> X<String> {
    Ok(s_(&g(d, k, l, name)?.unwrap_or(Value::Null)))
}

/// `d.get(k) or default` at `l`.
fn or_get(d: &Value, k: &str, default: Value, l: u32, name: &'static str) -> X<Value> {
    Ok(p::or(g(d, k, l, name)?, default))
}

fn empty_list() -> Value {
    Value::Array(Vec::new())
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

/// `who(a)`: the person's text, or the address as it came when there is
/// no name.
fn who(a: Option<&Value>) -> X<Value> {
    const F: &str = "who";
    let Some(a) = a.filter(|a| py::truthy(a)) else {
        return Ok(p::sv("(unknown)"));
    };
    let name = or_get(a, "name", p::sv(""), line::WHO_NAME, F)?;
    let n = match &name {
        Value::Str(t) => p::text(t).trim_matches(super::super::py_space).to_string(),
        other => {
            return Err(Ex::Raise(
                vec![(line::WHO_NAME, F)],
                format!(
                    "AttributeError: '{}' object has no attribute 'strip'",
                    py::type_name(other)
                ),
            ))
        }
    };
    let ad = or_get(a, "address", p::sv(""), line::WHO_NAME, F)?;
    Ok(if n.is_empty() {
        ad
    } else {
        p::sv(&format!("{n} <{}>", s_(&ad)))
    })
}

/// `", ".join(who(x) for x in v)` at `l` of `name`.
fn who_all(v: &Value, l: u32, name: &'static str) -> X<String> {
    let mut all = Vec::new();
    for x in raise(iter(v), l, name)? {
        all.push(within(within(who(Some(&x)), l, "<genexpr>"), l, name)?);
    }
    raise(p::join(", ", &Value::Array(all)), l, name)
}

/// `v[:n]`: a string's first `n` code points, a list's first `n` items.
fn head(v: &Value, n: usize) -> Result<Value, String> {
    match v {
        Value::Str(t) => Ok(Value::Str(Text::from(
            p::text(t).chars().take(n).collect::<String>().as_str(),
        ))),
        Value::Array(items) => Ok(Value::Array(items.iter().take(n).cloned().collect())),
        Value::Object(_) => Err("TypeError: unhashable type: 'slice'".into()),
        other => Err(format!(
            "TypeError: '{}' object is not subscriptable",
            py::type_name(other)
        )),
    }
}

/// `time.strftime("%Y-%m-%d %H:%M", time.localtime(v or 0))`.
fn local_minute(v: Option<Value>) -> Result<String, String> {
    let v = p::or(v, Value::Int("0".into()));
    let secs = pytime::seconds(&v, 0)?;
    pytime::strftime_local(secs, "%Y-%m-%d %H:%M")
}

/// `print_draft_head(m)`.
fn draft_head(py_: &mut Py, m: &Value) -> X<()> {
    const F: &str = "print_draft_head";
    let a = or_get(m, "author", empty_dict(), line::DRAFT_AUTHOR, F)?;
    let by = or_get(&a, "name", p::sv("?"), line::DRAFT_BY, F)?;
    let kind = g(&a, "kind", line::DRAFT_KIND, F)?;
    let kind = if kind.as_ref().is_some_and(|k| p::eq_str(k, "agent")) {
        "agent"
    } else {
        "person"
    };
    let edited = raise(
        local_minute(g(m, "editedAt", line::DRAFT_EDITED, F)?),
        line::DRAFT_EDITED,
        F,
    )?;
    let mut l = format!(
        "DRAFT, NOT SENT — drafted by {} ({kind}), last edited {edited}",
        s_(&by)
    );
    if let Some(e) = g(m, "editedBy", line::DRAFT_EDITED_BY, F)?.filter(py::truthy) {
        l.push_str(&format!(" by {}", s_(&e)));
    }
    py_.print(&l);
    let id = match g(m, "draftId", line::DRAFT_ID, F)?.filter(py::truthy) {
        Some(v) => v,
        None => g(m, "id", line::DRAFT_ID, F)?.unwrap_or(Value::Null),
    };
    py_.print(&format!("Draft:   {}", s_(&id)));
    if let Some(h) = g(m, "hint", line::DRAFT_HINT, F)?.filter(py::truthy) {
        py_.print(&format!("Note:    {}", s_(&h)));
    }
    Ok(())
}

/// `print_message(m)`.
fn message(py_: &mut Py, m: &Value) -> X<()> {
    const F: &str = "print_message";
    let draft = p::truthy(g(m, "draft", line::MSG_DRAFT, F)?.as_ref());
    if draft {
        within(draft_head(py_, m), line::MSG_HEAD, F)?;
    }
    let from = within(
        who(g(m, "from", line::MSG_FROM, F)?.as_ref()),
        line::MSG_FROM,
        F,
    )?;
    py_.print(&format!("From:    {}", s_(&from)));
    if p::truthy(g(m, "to", line::MSG_TO_IF, F)?.as_ref()) {
        let to = raise(py_index(m, "to"), line::MSG_TO, F)?;
        let t = who_all(&to, line::MSG_TO, F)?;
        py_.print(&format!("To:      {t}"));
    }
    if p::truthy(g(m, "cc", line::MSG_CC_IF, F)?.as_ref()) {
        let cc = raise(py_index(m, "cc"), line::MSG_CC, F)?;
        let t = who_all(&cc, line::MSG_CC, F)?;
        py_.print(&format!("Cc:      {t}"));
    }
    let date = g(m, "date", line::MSG_DATE, F)?.unwrap_or_else(|| p::sv(""));
    py_.print(&format!("Date:    {}", s_(&date)));
    let subject = or_get(m, "subject", p::sv(""), line::MSG_SUBJECT, F)?;
    py_.print(&format!("Subject: {}", s_(&subject)));
    if !p::truthy(g(m, "draft", line::MSG_ID_IF, F)?.as_ref()) {
        let id = g(m, "id", line::MSG_ID, F)?.unwrap_or_else(|| p::sv(""));
        py_.print(&format!("Id:      {}", s_(&id)));
    }
    let ar = or_get(m, "authResults", empty_dict(), line::MSG_AUTH, F)?;
    let mut any = false;
    for k in ["spf", "dkim", "dmarc"] {
        let v = within(
            raise(attr_get(&ar, k), line::MSG_ANY, "<genexpr>"),
            line::MSG_ANY,
            F,
        )?;
        if p::truthy(v.as_ref()) {
            any = true;
            break;
        }
    }
    if any {
        // `ar` is a dict here: `any` read it.
        let one = |k: &str| s_(&p::or(attr_get(&ar, k).unwrap_or(None), p::sv("-")));
        py_.print(&format!(
            "Checks:  SPF {} · DKIM {} · DMARC {}",
            one("spf"),
            one("dkim"),
            one("dmarc")
        ));
    }
    let atts = or_get(m, "attachments", empty_list(), line::MSG_ATTS, F)?;
    for a in raise(iter(&atts), line::MSG_ATTS, F)? {
        let index = s_get(&a, "index", line::MSG_ATT, F)?;
        let name = or_get(&a, "filename", p::sv("(no name)"), line::MSG_ATT, F)?;
        let mime = s_get(&a, "mime", line::MSG_ATT, F)?;
        let size = s_get(&a, "size", line::MSG_ATT, F)?;
        py_.print(&format!(
            "Attach:  {index}. {} ({mime}, {size} bytes)",
            s_(&name)
        ));
    }
    py_.print("");
    let html = py_.env.on("MR_HTML") && p::truthy(g(m, "html", line::MSG_HTML_IF, F)?.as_ref());
    if html {
        let h = raise(py_index(m, "html"), line::MSG_HTML, F)?;
        py_.print_v(&h);
        if p::truthy(g(m, "remoteContentBlocked", line::MSG_BLOCKED, F)?.as_ref()) {
            py_.print("\n(Remote pictures and styles were taken out. Links are kept as data-k2-link, not href.)");
        }
    } else {
        let t = or_get(m, "text", p::sv("(no text part)"), line::MSG_TEXT, F)?;
        py_.print_v(&t);
    }
    Ok(())
}

/// `lst(k)`: the newline- and comma-separated list, each item trimmed,
/// empty ones dropped.
fn lst(py_: &Py, k: &str) -> Vec<Value> {
    py_.env(k)
        .replace(',', "\n")
        .split('\n')
        .map(|x| x.trim_matches(super::super::py_space))
        .filter(|x| !x.is_empty())
        .map(p::sv)
        .collect()
}

/// `print(dumps(d)); sys.exit(0)` for `--json`: true when it printed.
fn json_out(py_: &mut Py, d: &Value) -> bool {
    if py_.env.on("MR_JSON") {
        py_.print_json(d);
        true
    } else {
        false
    }
}

fn main(py_: &mut Py) -> X<()> {
    let sub = py_.env("MR_SUB").to_string();
    let reason = py_.env("MR_REASON").to_string();
    let a1 = py_.env("MR_A1").to_string();
    let a2 = py_.env("MR_A2").to_string();
    let rs = |r: &str| format!("{R_}{r}");
    match sub.as_str() {
        "inboxes" => {
            let d = call(
                py_,
                Method::Get,
                &rs("inboxes"),
                &[("reason", reason)],
                None,
                line::INBOXES_CALL,
            )?;
            if json_out(py_, &d) {
                return Ok(());
            }
            let rows = or_get(&d, "inboxes", empty_list(), line::INBOXES_ROWS, M)?;
            if !py::truthy(&rows) {
                let viewer = or_get(&d, "viewer", empty_dict(), line::INBOXES_NONE, M)?;
                let any = p::truthy(g(&viewer, "opensAny", line::INBOXES_NONE, M)?.as_ref());
                py_.print(&format!(
                    "No inboxes you can open.{}",
                    if any {
                        ""
                    } else {
                        " You see only your own mailbox; an Owner or Admin sees every inbox."
                    }
                ));
                return Ok(());
            }
            for i in raise(iter(&rows), line::INBOXES_FOR, M)? {
                let owner = match g(&i, "person", line::INBOXES_OWNER, M)?.filter(py::truthy) {
                    Some(v) => v,
                    None => or_get(&i, "workspace", p::sv("-"), line::INBOXES_OWNER, M)?,
                };
                let own = p::truthy(g(&i, "own", line::INBOXES_TAG, M)?.as_ref());
                let address = raise(py_index(&i, "address"), line::INBOXES_ROW, M)?;
                let provider = raise(p::get_or(&i, "provider", p::sv("")), line::INBOXES_ROW, M)?;
                let kind = raise(p::get_or(&i, "kind", p::sv("")), line::INBOXES_ROW, M)?;
                py_.print(&format!(
                    "{} {} {} {}{}",
                    p::ljust(&s_(&address), 40),
                    p::ljust(&s_(&provider), 9),
                    p::ljust(&s_(&kind), 6),
                    s_(&owner),
                    if own { " (you)" } else { "" }
                ));
            }
        }
        "folders" => {
            let params = [("inbox", a1.clone()), ("reason", reason)];
            let d = call(
                py_,
                Method::Get,
                &rs("folders"),
                &params,
                None,
                line::FOLDERS_CALL,
            )?;
            if json_out(py_, &d) {
                return Ok(());
            }
            let rows = or_get(&d, "folders", empty_list(), line::FOLDERS_FOR, M)?;
            for f in raise(iter(&rows), line::FOLDERS_FOR, M)? {
                let name = raise(py_index(&f, "name"), line::FOLDERS_ROW, M)?;
                let role = or_get(&f, "role", p::sv(""), line::FOLDERS_ROW, M)?;
                py_.print(&format!("{} {}", p::ljust(&s_(&name), 30), s_(&role)));
            }
        }
        "messages" => {
            let e = |k: &str| py_.env(k).to_string();
            let params = [
                ("inbox", a1.clone()),
                ("folder", e("MR_FOLDER")),
                (
                    "unread",
                    if py_.env.on("MR_UNREAD") {
                        "1".into()
                    } else {
                        String::new()
                    },
                ),
                ("query", e("MR_QUERY")),
                ("from", e("MR_FROM")),
                ("since", e("MR_SINCE")),
                ("before", e("MR_BEFORE")),
                ("limit", e("MR_LIMIT")),
                ("offset", e("MR_OFFSET")),
                ("reason", reason),
            ];
            let d = call(
                py_,
                Method::Get,
                &rs("messages"),
                &params,
                None,
                line::MESSAGES_CALL,
            )?;
            if json_out(py_, &d) {
                return Ok(());
            }
            let items = or_get(&d, "items", empty_list(), line::MESSAGES_ITEMS, M)?;
            if !py::truthy(&items) {
                py_.print("No messages.");
            }
            for m in raise(iter(&items), line::MESSAGES_FOR, M)? {
                let l = line::MESSAGES_ROW;
                let star = if p::truthy(g(&m, "unread", l, M)?.as_ref()) {
                    "*"
                } else {
                    " "
                };
                let id = s_get(&m, "id", l, M)?;
                let date = or_get(&m, "date", p::sv(""), l, M)?;
                let date = match raise(head(&date, 16), l, M)? {
                    Value::Str(t) => p::text(&t).replace('T', " "),
                    other => {
                        return Err(Ex::Raise(
                            vec![(l, M)],
                            format!(
                                "AttributeError: '{}' object has no attribute 'replace'",
                                py::type_name(&other)
                            ),
                        ))
                    }
                };
                let from = within(who(g(&m, "from", l + 1, M)?.as_ref()), l + 1, M)?;
                let from = raise(head(&from, 30), l + 1, M)?;
                let subject = or_get(&m, "subject", p::sv("(no subject)"), l + 1, M)?;
                py_.print(&format!(
                    "{star} {id}  {}  {}  {}",
                    p::ljust(&date, 20),
                    p::ljust(&s_(&from), 30),
                    s_(&subject)
                ));
            }
            let next = g(&d, "nextOffset", line::MESSAGES_NEXT, M)?;
            if let Some(n) = next.filter(|n| *n != Value::Null) {
                py_.print(&format!(
                    "more: {CLI_NAME} mail reader messages {a1} --offset {}",
                    s_(&n)
                ));
            }
        }
        "show" => {
            let params = [("id", a1.clone()), ("reason", reason)];
            let d = call(
                py_,
                Method::Get,
                &rs("message"),
                &params,
                None,
                line::SHOW_CALL,
            )?;
            if json_out(py_, &d) {
                return Ok(());
            }
            let m = raise(py_index(&d, "message"), line::SHOW_PRINT, M)?;
            within(message(py_, &m), line::SHOW_PRINT, M)?;
        }
        "thread" => {
            let params = [
                ("id", a1.clone()),
                ("folder", py_.env("MR_FOLDER").to_string()),
                ("reason", reason),
            ];
            let d = call(
                py_,
                Method::Get,
                &rs("thread"),
                &params,
                None,
                line::THREAD_CALL,
            )?;
            if json_out(py_, &d) {
                return Ok(());
            }
            let ms = or_get(&d, "messages", empty_list(), line::THREAD_FOR, M)?;
            for (n, m) in raise(iter(&ms), line::THREAD_FOR, M)?.iter().enumerate() {
                if n > 0 {
                    py_.print(&format!("\n{}\n", "─".repeat(60)));
                }
                within(message(py_, m), line::THREAD_PRINT, M)?;
            }
        }
        "attachment" => attachment(py_, &a1, &a2, reason)?,
        "log" => {
            let e = |k: &str| py_.env(k).to_string();
            let params = [
                ("inbox", e("MR_INBOX")),
                ("actor", e("MR_ACTOR")),
                ("since", e("MR_SINCE")),
                ("limit", e("MR_LIMIT")),
            ];
            let d = call(py_, Method::Get, &rs("log"), &params, None, line::LOG_CALL)?;
            if json_out(py_, &d) {
                return Ok(());
            }
            let rows = or_get(&d, "rows", empty_list(), line::LOG_ROWS, M)?;
            if !py::truthy(&rows) {
                py_.print("No rows.");
            }
            for r in raise(iter(&rows), line::LOG_FOR, M)? {
                let at = raise(local_minute(g(&r, "at", line::LOG_AT, M)?), line::LOG_AT, M)?;
                let l = line::LOG_ROW;
                let mut row = format!(
                    "{at}  {} {} {} {} {}",
                    p::ljust(&s_get(&r, "actor", l, M)?, 22),
                    p::ljust(&s_get(&r, "actorRole", l, M)?, 7),
                    p::ljust(&s_get(&r, "action", l, M)?, 18),
                    p::ljust(&s_get(&r, "address", l, M)?, 36),
                    s_get(&r, "outcome", l, M)?
                );
                if let Some(why) = g(&r, "reason", line::LOG_REASON, M)?.filter(py::truthy) {
                    row.push_str(&format!("  reason: {}", s_(&why)));
                }
                py_.print(&row);
            }
        }
        "drafts" => {
            let params = [("inbox", a1.clone()), ("reason", reason)];
            let d = call(
                py_,
                Method::Get,
                &rs("drafts"),
                &params,
                None,
                line::DRAFTS_CALL,
            )?;
            if json_out(py_, &d) {
                return Ok(());
            }
            let rows = or_get(&d, "drafts", empty_list(), line::DRAFTS_ROWS, M)?;
            if !py::truthy(&rows) {
                py_.print(&format!("No drafts {} tracks in {a1}.", product_name!()));
            }
            for r in raise(iter(&rows), line::DRAFTS_FOR, M)? {
                let a = or_get(&r, "author", empty_dict(), line::DRAFTS_AUTHOR, M)?;
                let edited = raise(
                    local_minute(g(&r, "editedAt", line::DRAFTS_EDITED, M)?),
                    line::DRAFTS_EDITED,
                    M,
                )?;
                let l = line::DRAFTS_ROW;
                let id = s_get(&r, "id", l, M)?;
                let status = s_get(&r, "status", l, M)?;
                let author = format!(
                    "{} ({})",
                    s_get(&a, "name", l, M)?,
                    s_get(&a, "kind", l, M)?
                );
                let mut row = format!(
                    "{id}  {} by {} edited {edited}",
                    p::ljust(&status, 9),
                    p::ljust(&author, 20)
                );
                if let Some(by) = g(&r, "editedBy", line::DRAFTS_EDITED_BY, M)?.filter(py::truthy) {
                    row.push_str(&format!(" by {}", s_(&by)));
                }
                let state_by = g(&r, "stateBy", line::DRAFTS_STATE, M)?;
                if p::truthy(state_by.as_ref()) {
                    let st = g(&r, "status", line::DRAFTS_STATE, M)?;
                    if st.as_ref().is_some_and(|s| {
                        ["sent", "held", "discarded"]
                            .iter()
                            .any(|x| p::eq_str(s, x))
                    }) {
                        let st = raise(py_index(&r, "status"), line::DRAFTS_STATE + 1, M)?;
                        let by = raise(py_index(&r, "stateBy"), line::DRAFTS_STATE + 1, M)?;
                        row.push_str(&format!("  · {} by {}", s_(&st), s_(&by)));
                    }
                }
                py_.print(&row);
            }
        }
        "draft-show" => {
            let params = [("id", a1.clone()), ("reason", reason)];
            let d = call(
                py_,
                Method::Get,
                &rs("draft-open"),
                &params,
                None,
                line::DRAFT_SHOW_CALL,
            )?;
            if json_out(py_, &d) {
                return Ok(());
            }
            let m = raise(py_index(&d, "draft"), line::DRAFT_SHOW_PRINT, M)?;
            within(message(py_, &m), line::DRAFT_SHOW_PRINT, M)?;
        }
        "draft-save" | "draft-send" | "draft-discard" => {
            let mut body: Vec<(&str, Value)> = vec![("id", p::sv(&a1))];
            if !reason.is_empty() {
                body.push(("reason", p::sv(&reason)));
            }
            if sub != "draft-discard" {
                let to = lst(py_, "MR_TO");
                if !to.is_empty() {
                    body.push(("to", Value::Array(to)));
                }
                let cc = lst(py_, "MR_CC");
                if !cc.is_empty() {
                    body.push(("cc", Value::Array(cc)));
                }
                if !py_.env("MR_SUBJECT").is_empty() {
                    body.push(("subject", p::sv(py_.env("MR_SUBJECT"))));
                }
                if py_.env.on("MR_BODY_SET") {
                    push_body(py_, &mut body, line::DRAFT_STATE_CALL)?;
                }
            }
            let body = core::obj(body);
            let d = call(
                py_,
                Method::Post,
                &rs(&sub),
                &[],
                Some(&body),
                line::DRAFT_STATE_CALL,
            )?;
            if json_out(py_, &d) {
                return Ok(());
            }
            match sub.as_str() {
                "draft-send" => {
                    let st = s_get(&d, "status", line::DRAFT_SEND_STATUS, M)?;
                    let l = line::DRAFT_SEND_PRINT;
                    let id = match g(&d, "id", l, M)?.filter(py::truthy) {
                        Some(_) => {
                            raise(p::concat(" — ", &raise(py_index(&d, "id"), l, M)?), l, M)?
                        }
                        None => String::new(),
                    };
                    let ds = s_get(&d, "draftStatus", l, M)?;
                    py_.print(&format!("send {st}{id} — draft {a1} {ds}"));
                }
                "draft-save" => py_.print(&format!("saved draft {a1}")),
                _ => py_.print(&format!("discarded draft {a1}")),
            }
        }
        _ => {
            let mut body: Vec<(&str, Value)> = Vec::new();
            if !reason.is_empty() {
                body.push(("reason", p::sv(&reason)));
            }
            if matches!(
                sub.as_str(),
                "flag" | "move" | "archive" | "delete" | "reply"
            ) || (sub == "draft" && !a1.is_empty())
            {
                body.push(("id", p::sv(&a1)));
            }
            if sub == "flag" {
                if !py_.env("MR_READ").is_empty() {
                    body.push(("read", Value::Bool(py_.env.on("MR_READ"))));
                }
                if !py_.env("MR_FLAGGED").is_empty() {
                    body.push(("flagged", Value::Bool(py_.env.on("MR_FLAGGED"))));
                }
            }
            if sub == "move" {
                body.push(("folder", p::sv(&a2)));
            }
            if matches!(sub.as_str(), "reply" | "draft" | "send") {
                push_body(py_, &mut body, line::ACT_CALL)?;
            }
            if sub == "send" || (sub == "draft" && a1.is_empty()) {
                body.push(("inbox", p::sv(py_.env("MR_INBOX"))));
                body.push(("to", Value::Array(lst(py_, "MR_TO"))));
                let cc = lst(py_, "MR_CC");
                if !cc.is_empty() {
                    body.push(("cc", Value::Array(cc)));
                }
                body.push(("subject", p::sv(py_.env("MR_SUBJECT"))));
            }
            let body = core::obj(body);
            let d = call(
                py_,
                Method::Post,
                &rs(&sub),
                &[],
                Some(&body),
                line::ACT_CALL,
            )?;
            if json_out(py_, &d) {
                return Ok(());
            }
            let status = g(&d, "status", line::ACT_STATUS, M)?;
            if sub == "send" && p::truthy(status.as_ref()) {
                let l = line::ACT_SEND;
                let id = match g(&d, "id", l, M)?.filter(py::truthy) {
                    Some(_) => raise(p::concat(" — ", &raise(py_index(&d, "id"), l, M)?), l, M)?,
                    None => String::new(),
                };
                py_.print(&format!("send {}{id}", s_(&status.unwrap_or(Value::Null))));
            } else if sub == "move" {
                let f = or_get(&d, "folder", p::sv(&a2), line::ACT_MOVE, M)?;
                py_.print(&format!("moved to {}", s_(&f)));
            } else if sub == "draft" && p::truthy(g(&d, "draftId", line::ACT_DRAFT_IF, M)?.as_ref())
            {
                let id = raise(py_index(&d, "draftId"), line::ACT_DRAFT, M)?;
                let f = or_get(&d, "folder", p::sv("Drafts"), line::ACT_DRAFT, M)?;
                py_.print(&format!("draft {} saved to {}", s_(&id), s_(&f)));
            } else {
                py_.print("ok");
            }
        }
    }
    Ok(())
}

/// `body["body"] = E("MR_BODY")`, for the call at module line `from`.
/// Stdin that is not UTF-8 is held as Python holds it (each bad byte a
/// lone surrogate), which `dumps(body).encode()` then refuses.
fn push_body(py_: &Py, body: &mut Vec<(&str, Value)>, from: u32) -> X<()> {
    match body_text(py_) {
        Ok(t) => {
            body.push(("body", p::sv(&t)));
            Ok(())
        }
        Err(bytes) => {
            let e = std::str::from_utf8(&bytes).expect_err("not UTF-8");
            let ok = &bytes[..e.valid_up_to()];
            let bad = bytes[e.valid_up_to()];
            // The code points before the first lone surrogate: the dict as
            // far as the body's text that is still UTF-8.
            let mut upto = body.clone();
            upto.push((
                "body",
                p::sv(std::str::from_utf8(ok).expect("valid up to here")),
            ));
            let at = core::dumps(&core::obj(upto)).chars().count() - 2;
            Err(Ex::Raise(
                vec![(from, M), (line::CALL_DATA, "call")],
                format!(
                    "UnicodeEncodeError: 'utf-8' codec can't encode character '\\udc{bad:02x}' in position {at}: surrogates not allowed"
                ),
            ))
        }
    }
}

/// `attachment`: the part number, the file that must not be there, the
/// call, the bytes written to a new file.
fn attachment(py_: &mut Py, a1: &str, a2: &str, reason: String) -> X<()> {
    let part = match p::py_int(&p::sv(a2)) {
        Ok(n) => n,
        Err(_) => {
            return Err(die_with(
                "usage",
                &format!("the attachment number is 1, 2, … (from '{CLI_NAME} mail reader show')"),
            ))
        }
    };
    let out_path = py_.env("MR_OUT").to_string();
    if std::fs::metadata(&out_path).is_ok() {
        return Err(die_with(
            "usage",
            &format!("'{out_path}' already exists — pick another --out"),
        ));
    }
    let params = [("id", a1.to_string()), ("part", part), ("reason", reason)];
    let d = call(
        py_,
        Method::Get,
        &format!("{R_}attachment"),
        &params,
        None,
        line::ATT_CALL,
    )?;
    let b = or_get(&d, "dataBase64", p::sv(""), line::ATT_DATA, M)?;
    let data = raise(b64decode(&b), line::ATT_DATA, M)?;
    raise(write_new(&out_path, &data), line::ATT_OPEN, M)?;
    if py_.env.on("MR_JSON") {
        let Value::Object(mut pairs) = d else {
            unreachable!("call read it as a dict")
        };
        pairs.retain(|(k, _)| k.as_str() != Some("dataBase64"));
        let saved = p::sv(&out_path);
        match pairs
            .iter_mut()
            .find(|(k, _)| k.as_str() == Some("savedTo"))
        {
            Some(slot) => slot.1 = saved,
            None => pairs.push((Text::from("savedTo"), saved)),
        }
        py_.print_json(&Value::Object(pairs));
        return Ok(());
    }
    let l = line::ATT_PRINT;
    let name = s_get(&d, "filename", l, M)?;
    let mime = s_get(&d, "mime", l, M)?;
    py_.print(&format!(
        "saved {name} ({mime}, {} bytes) to {out_path}",
        data.len()
    ));
    Ok(())
}

/// `open(path, "xb")` and the write: Python's error for it.
fn write_new(path: &str, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| pyjson::os_error(&e, path).last_line())?;
    f.write_all(data)
        .map_err(|e| pyjson::os_error(&e, path).last_line())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reader's script as python3 reads it from stdin: line 1 is the
    /// one after `python3 - <<'PYEOF'`.
    fn script() -> Vec<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../cli/src/mail-reader-and-help.sh");
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let body = text
            .split_once("python3 - <<'PYEOF'\n")
            .expect("the reader's command line")
            .1;
        let end = body.find("\nPYEOF\n").expect("the heredoc's end");
        body[..end].split('\n').map(str::to_string).collect()
    }

    #[test]
    fn the_lines_that_raise_are_the_shell_s() {
        let s = script();
        assert_eq!(s[0], "import base64, json, os, sys, time");
        let pins: &[(u32, &str)] = &[
            (line::CALL_DATA, "data = dumps(body).encode() if body"),
            (
                line::CALL_HTTP_ERROR,
                "if isinstance(d.get(\"error\"), str):",
            ),
            (line::CALL_ERROR, "if isinstance(d.get(\"error\"), dict):"),
            (line::CALL_OK, "if not d.get(\"ok\"):"),
            (line::WHO_NAME, "n, ad = (a.get(\"name\") or \"\").strip()"),
            (line::DRAFT_AUTHOR, "a = m.get(\"author\") or {}"),
            (line::DRAFT_BY, "by = a.get(\"name\") or \"?\""),
            (line::DRAFT_KIND, "kind = \"agent\" if a.get(\"kind\")"),
            (line::DRAFT_EDITED, "edited = time.strftime("),
            (line::DRAFT_EDITED_BY, "if m.get(\"editedBy\"):"),
            (
                line::DRAFT_ID,
                "print(\"Draft:   %s\" % (m.get(\"draftId\")",
            ),
            (line::DRAFT_HINT, "if m.get(\"hint\"):"),
            (line::MSG_DRAFT, "if m.get(\"draft\"):"),
            (line::MSG_HEAD, "print_draft_head(m)"),
            (
                line::MSG_FROM,
                "print(\"From:    %s\" % who(m.get(\"from\")))",
            ),
            (line::MSG_TO_IF, "if m.get(\"to\"):"),
            (line::MSG_TO, "print(\"To:      %s\" % \", \".join(who(x)"),
            (line::MSG_CC_IF, "if m.get(\"cc\"):"),
            (line::MSG_CC, "print(\"Cc:      %s\" % \", \".join(who(x)"),
            (
                line::MSG_DATE,
                "print(\"Date:    %s\" % m.get(\"date\", \"\"))",
            ),
            (
                line::MSG_SUBJECT,
                "print(\"Subject: %s\" % (m.get(\"subject\")",
            ),
            (line::MSG_ID_IF, "if not m.get(\"draft\"):"),
            (line::MSG_ID, "print(\"Id:      %s\" % m.get(\"id\", \"\"))"),
            (line::MSG_AUTH, "ar = m.get(\"authResults\") or {}"),
            (line::MSG_ANY, "if any(ar.get(k) for k in"),
            (line::MSG_CHECKS, "print(\"Checks:  SPF %s · DKIM %s"),
            (line::MSG_ATTS, "for a in m.get(\"attachments\") or []:"),
            (line::MSG_ATT, "print(\"Attach:  %s. %s (%s, %s bytes)\""),
            (
                line::MSG_HTML_IF,
                "if E(\"MR_HTML\") == \"true\" and m.get(\"html\"):",
            ),
            (line::MSG_HTML, "print(m[\"html\"])"),
            (line::MSG_BLOCKED, "if m.get(\"remoteContentBlocked\"):"),
            (
                line::MSG_TEXT,
                "print(m.get(\"text\") or \"(no text part)\")",
            ),
            (line::INBOXES_CALL, "d = call(\"GET\", R + \"inboxes\""),
            (line::INBOXES_ROWS, "rows = d.get(\"inboxes\") or []"),
            (line::INBOXES_NONE, "print(\"No inboxes you can open.\""),
            (line::INBOXES_FOR, "for i in rows:"),
            (
                line::INBOXES_OWNER,
                "owner = i.get(\"person\") or i.get(\"workspace\")",
            ),
            (
                line::INBOXES_TAG,
                "tag = \" (you)\" if i.get(\"own\") else \"\"",
            ),
            (
                line::INBOXES_ROW,
                "print(\"%-40s %-9s %-6s %s%s\" % (i[\"address\"]",
            ),
            (line::FOLDERS_CALL, "d = call(\"GET\", R + \"folders\""),
            (line::FOLDERS_FOR, "for f in d.get(\"folders\") or []:"),
            (line::FOLDERS_ROW, "print(\"%-30s %s\" % (f[\"name\"]"),
            (line::MESSAGES_CALL, "d = call(\"GET\", R + \"messages\""),
            (line::MESSAGES_ITEMS, "items = d.get(\"items\") or []"),
            (line::MESSAGES_FOR, "for m in items:"),
            (line::MESSAGES_ROW, "print(\"%s %s  %-20s  %-30s  %s\""),
            (line::MESSAGES_ROW + 1, "who(m.get(\"from\"))[:30]"),
            (line::MESSAGES_NEXT, "if d.get(\"nextOffset\") is not None:"),
            (
                line::MESSAGES_MORE,
                "print(\"more: k2 mail reader messages %s",
            ),
            (line::SHOW_CALL, "d = call(\"GET\", R + \"message\""),
            (line::SHOW_PRINT, "print_message(d[\"message\"])"),
            (line::THREAD_CALL, "d = call(\"GET\", R + \"thread\""),
            (
                line::THREAD_FOR,
                "for n, m in enumerate(d.get(\"messages\") or []):",
            ),
            (line::THREAD_PRINT, "print_message(m)"),
            (line::ATT_CALL, "d = call(\"GET\", R + \"attachment\""),
            (
                line::ATT_DATA,
                "data = base64.b64decode(d.get(\"dataBase64\") or \"\")",
            ),
            (line::ATT_OPEN, "with open(out, \"xb\") as f:"),
            (line::ATT_PRINT, "print(\"saved %s (%s, %d bytes) to %s\""),
            (line::LOG_CALL, "d = call(\"GET\", R + \"log\""),
            (line::LOG_ROWS, "rows = d.get(\"rows\") or []"),
            (line::LOG_FOR, "for r in rows:"),
            (line::LOG_AT, "at = time.strftime(\"%Y-%m-%d %H:%M\""),
            (line::LOG_ROW, "line = \"%s  %-22s %-7s %-18s %-36s %s\""),
            (line::LOG_REASON, "if r.get(\"reason\"):"),
            (line::DRAFTS_CALL, "d = call(\"GET\", R + \"drafts\""),
            (line::DRAFTS_ROWS, "rows = d.get(\"drafts\") or []"),
            (line::DRAFTS_FOR, "for r in rows:"),
            (line::DRAFTS_AUTHOR, "a = r.get(\"author\") or {}"),
            (line::DRAFTS_EDITED, "edited = time.strftime("),
            (line::DRAFTS_ROW, "line = \"%s  %-9s by %-20s edited %s\""),
            (line::DRAFTS_EDITED_BY, "if r.get(\"editedBy\"):"),
            (
                line::DRAFTS_STATE,
                "if r.get(\"stateBy\") and r.get(\"status\") in",
            ),
            (
                line::DRAFTS_STATE + 1,
                "line += \"  · %s by %s\" % (r[\"status\"]",
            ),
            (
                line::DRAFT_SHOW_CALL,
                "d = call(\"GET\", R + \"draft-open\"",
            ),
            (line::DRAFT_SHOW_PRINT, "print_message(d[\"draft\"])"),
            (
                line::DRAFT_STATE_CALL,
                "d = call(\"POST\", R + sub, None, body)",
            ),
            (line::DRAFT_SEND_STATUS, "st = d.get(\"status\")"),
            (line::DRAFT_SEND_PRINT, "print(\"send %s%s — draft %s %s\""),
            (line::ACT_CALL, "d = call(\"POST\", R + sub, None, body)"),
            (line::ACT_STATUS, "status = d.get(\"status\")"),
            (line::ACT_SEND, "print(\"send %s%s\" % (status"),
            (
                line::ACT_MOVE,
                "print(\"moved to %s\" % (d.get(\"folder\") or a2))",
            ),
            (
                line::ACT_DRAFT_IF,
                "elif sub == \"draft\" and d.get(\"draftId\"):",
            ),
            (line::ACT_DRAFT, "print(\"draft %s saved to %s\""),
        ];
        for (n, text) in pins {
            let got = s[*n as usize - 1].trim();
            assert!(
                got.starts_with(text) || got.contains(text),
                "line {n}: {got:?} is not {text:?}"
            );
        }
    }

    fn hint(words: &[&str]) -> String {
        match parse("reader", words) {
            Plan::Usage(h) => h,
            other => panic!("{words:?}: {other:?}"),
        }
    }

    fn env(words: &[&str]) -> Env {
        match parse("reader", words) {
            Plan::Run(e) => e,
            other => panic!("{words:?}: {other:?}"),
        }
    }

    #[test]
    fn the_words_are_cmd_mail_reader_s() {
        for w in [&[][..], &["help"], &[""], &["--help"], &["-h"]] {
            assert_eq!(parse("reader", w), Plan::Help("mail/reader"));
        }
        assert_eq!(
            hint(&["nope"]),
            format!("unknown command 'nope' — see {CLI_NAME} mail reader --help")
        );
        assert_eq!(
            hint(&["show", "--nope", "x"]),
            format!("unknown flag '--nope' for '{CLI_NAME} mail reader show' — see {CLI_NAME} mail reader --help")
        );
        assert_eq!(
            hint(&["show", "x", "--reason"]),
            format!("--reason needs a value — see {CLI_NAME} mail reader --help")
        );
        assert_eq!(
            hint(&["move", "x"]),
            format!("'{CLI_NAME} mail reader move' needs 2 argument(s) — see {CLI_NAME} mail reader --help")
        );
        assert_eq!(
            hint(&["inboxes", "x"]),
            format!("unexpected argument 'x' for '{CLI_NAME} mail reader inboxes' — see {CLI_NAME} mail reader --help")
        );
        assert_eq!(
            hint(&["attachment", "m", "1"]),
            "missing --out <file> — where to save the attachment"
        );
        assert_eq!(
            hint(&["flag", "m"]),
            "nothing to change — pass --read, --unread, --star or --unstar"
        );
        assert_eq!(
            hint(&["reply", "m"]),
            "missing --body <text> (or --body - to read stdin)"
        );
        assert_eq!(
            hint(&["send", "--body", "x"]),
            "missing --inbox <address> — the inbox to send from"
        );
        // A single dash and `-x` are words, not flags.
        let e = env(&["show", "-x"]);
        assert_eq!(e.get("MR_A1"), "-x");
        // `draft` with --inbox takes no id; without, one.
        assert_eq!(
            env(&["draft", "--inbox", "a@x", "--body", "b"]).get("MR_A1"),
            ""
        );
        assert_eq!(env(&["draft", "m1", "--body", "b"]).get("MR_A1"), "m1");
        // `--unread` is read=false for flag, the filter elsewhere.
        assert_eq!(env(&["flag", "m", "--unread"]).get("MR_READ"), "false");
        assert_eq!(env(&["messages", "a", "--unread"]).get("MR_UNREAD"), "true");
        // --to and --cc gather one a line.
        let e = env(&[
            "send", "--inbox", "a", "--body", "b", "--to", "x", "--to", "y,z",
        ]);
        assert_eq!(e.get("MR_TO"), "x\ny,z");
        // `--body -` reads stdin once; a second one reads nothing.
        let e = env(&["reply", "m", "--body", "-"]);
        assert!(e.on("MR_BODY_STDIN") && e.on("MR_BODY_SET"));
        let e = env(&["reply", "m", "--body", "-", "--body", "-"]);
        assert!(!e.on("MR_BODY_STDIN") && e.get("MR_BODY").is_empty());
        let e = env(&["reply", "m", "--body", "-", "--body", "x"]);
        assert!(!e.on("MR_BODY_STDIN") && e.get("MR_BODY") == "x");
        // A part number in digits of another script is the shell's.
        assert_eq!(
            parse("reader", &["attachment", "m", "٣", "--out", "f"]),
            Plan::Shell
        );
    }

    /// Under any brand: the usage lines name the brand's command, never
    /// another.
    #[test]
    fn usage_lines_name_the_brand_s_command() {
        for words in [
            &["nope"][..],
            &["show", "--nope"],
            &["show", "--reason"],
            &["move", "x"],
            &["inboxes", "x"],
        ] {
            let h = hint(words);
            assert!(
                h.contains(&format!("{CLI_NAME} mail reader")),
                "{words:?}: {h}"
            );
            if CLI_NAME != "k2" {
                assert!(!h.contains("k2 ") && !h.contains("k5 "), "{words:?}: {h}");
            }
        }
    }

    #[test]
    fn a_slice_is_python_s() {
        assert_eq!(
            head(&p::sv("2026-11-20T10:11:12Z"), 16),
            Ok(p::sv("2026-11-20T10:11"))
        );
        assert_eq!(
            head(&Value::Array(vec![p::sv("a"), p::sv("b")]), 1),
            Ok(Value::Array(vec![p::sv("a")]))
        );
        assert_eq!(
            head(&Value::Int("5".into()), 1),
            Err("TypeError: 'int' object is not subscriptable".into())
        );
        assert_eq!(
            head(&Value::Object(vec![]), 1),
            Err("TypeError: unhashable type: 'slice'".into())
        );
    }
}
