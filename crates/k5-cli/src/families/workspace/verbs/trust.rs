//! `workspace trust`: folder trust, as the shell CLI's
//! `cmd_workspace_trust` (`cli/src/wiki.sh`).
//!
//! - `status [<folder>] [--json]`: `resp=$(cli_send cli_request
//!   /cli/folder-trust/status [path=<abs>])`; `--json` prints it through
//!   `cli_print_or_fail` ([`print_plain`]), else `_workspace_trust_print`
//!   ([`text`]).
//! - `request <folder> --why "…"`: `cli_send cli_request_post
//!   /cli/folder-trust/request`, then its lines.
//! - The owner's verbs: `<folder> --yes|--always|--no [--fingerprint fp]`,
//!   `reset <folder>`, `forget <folder>`. With a passport (an agent or a
//!   K2 terminal) the status lookup for the app's place, three lines and
//!   exit 3. Else `reset` and `forget` POST and print; a decision without
//!   `--fingerprint` reads the status (`cli_request`), prints it, and with
//!   a fingerprint there stops with its line (exit 2) unless stdin and
//!   stdout are terminals; then the decision's POST.
//!
//! Folders are made absolute as `cli_abs_path` does. Usage lines are
//! exit 2. The shell answers: the help, a folder the binary can't make
//! absolute as bash would, and a decision with no fingerprint at a
//! terminal (its `[y/N]` prompt).

use super::{is_str, py_get, py_in, stop, str_plus, Ask as Top, Here, Py, Sent};
use crate::exit::Exit;
use crate::families::workspace::register::abs_path;
use crate::families::{get, ljust, py_d, py_iter, s, Raise, Shown};
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::transport::encode::quote;
use crate::transport::{Call, Channel};
use k2_client::brand::PRODUCT_NAME;
use k2_client::cli_name;

pub const STATUS: &str = "/cli/folder-trust/status";
pub const REQUEST: &str = "/cli/folder-trust/request";
pub const DECIDE: &str = "/cli/folder-trust/decide";
pub const RESET: &str = "/cli/folder-trust/reset";
pub const FORGET: &str = "/cli/folder-trust/forget";

const STATUS_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " workspace trust status [<folder>] [--json]"
);
const REQUEST_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " workspace trust request <folder> --why \"…\""
);
const DECIDE_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " workspace trust <folder> --yes|--always|--no [--fingerprint <fp>]   (or: reset <folder>, forget <folder>)"
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The folder made absolute (empty: none), and `--json`.
    Status(String, bool),
    /// The folder made absolute, and `--why`.
    Request(String, String),
    Decide(Decide),
}

impl Ask {
    pub fn sends_on_socket(&self) -> bool {
        // An owner's decision sends on the socket only for the lookup a
        // passport makes, and a passport is no session without one.
        matches!(self, Ask::Status(..) | Ask::Request(..))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decide {
    /// `once`, `always`, `deny`, `reset` or `forget`.
    pub decision: String,
    pub folder: String,
    pub fingerprint: String,
}

fn usage(lines: &[&str]) -> Top {
    stop(lines, Exit::Usage)
}

/// The words after `trust`.
pub fn parse(words: &[&str], here: &Here) -> Top {
    let sub = words.first().copied().unwrap_or("");
    let abs = |f: &str| abs_path(f, &here.pwd);
    match sub {
        "" | "-h" | "--help" | "help" => Top::Shell,
        "status" => {
            let (mut folder, mut json) = (String::new(), false);
            for w in &words[1..] {
                match *w {
                    "--json" => json = true,
                    w if w.starts_with('-') => {
                        return usage(&[&format!("Unknown flag: {w}"), STATUS_USAGE])
                    }
                    w => {
                        if !folder.is_empty() {
                            return usage(&[STATUS_USAGE]);
                        }
                        folder = w.to_string();
                    }
                }
            }
            match abs(&folder) {
                Some(f) => Top::Trust(Ask::Status(f, json)),
                None => Top::Shell,
            }
        }
        "request" => {
            let (mut folder, mut why) = (String::new(), String::new());
            let mut i = 1;
            while i < words.len() {
                match words[i] {
                    "--why" => {
                        let Some(v) = words.get(i + 1) else {
                            return usage(&[REQUEST_USAGE]);
                        };
                        why = (*v).to_string();
                        i += 2;
                        continue;
                    }
                    w if w.starts_with('-') => {
                        return usage(&[&format!("Unknown flag: {w}"), REQUEST_USAGE])
                    }
                    w => {
                        if !folder.is_empty() {
                            return usage(&[REQUEST_USAGE]);
                        }
                        folder = w.to_string();
                    }
                }
                i += 1;
            }
            if folder.is_empty() {
                return usage(&[REQUEST_USAGE]);
            }
            match abs(&folder) {
                Some(f) => Top::Trust(Ask::Request(f, why)),
                None => Top::Shell,
            }
        }
        _ => decide(words, here),
    }
}

fn decide(words: &[&str], here: &Here) -> Top {
    let mut decision = String::new();
    let mut words = words;
    if let Some(first @ ("reset" | "forget")) = words.first().copied() {
        decision = first.to_string();
        words = &words[1..];
    }
    let (mut folder, mut fingerprint) = (String::new(), String::new());
    let fixed = |d: &str| matches!(d, "reset" | "forget");
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--yes" | "--always" | "--no" | "--fingerprint" if fixed(&decision) => {
                return usage(&[DECIDE_USAGE])
            }
            "--yes" => decision = "once".into(),
            "--always" => decision = "always".into(),
            "--no" => decision = "deny".into(),
            "--fingerprint" => {
                let Some(v) = words.get(i + 1) else {
                    return usage(&[DECIDE_USAGE]);
                };
                fingerprint = (*v).to_string();
                i += 2;
                continue;
            }
            w if w.starts_with('-') => {
                return usage(&[&format!("Unknown flag: {w}"), DECIDE_USAGE])
            }
            w => {
                if !folder.is_empty() {
                    return usage(&[DECIDE_USAGE]);
                }
                folder = w.to_string();
            }
        }
        i += 1;
    }
    if folder.is_empty() || decision.is_empty() {
        return usage(&[DECIDE_USAGE]);
    }
    let Some(folder) = abs_path(&folder, &here.pwd) else {
        return Top::Shell;
    };
    // A decision with no fingerprint at a terminal asks `[y/N]`.
    if !fixed(&decision) && fingerprint.is_empty() && here.stdin_tty && here.stdout_tty {
        return Top::Shell;
    }
    Top::Trust(Ask::Decide(Decide {
        decision,
        folder,
        fingerprint,
    }))
}

/// The verb's requests and lines.
pub fn send(w: &mut Sent, a: &Ask) -> Exit {
    match a {
        Ask::Status(folder, json) => {
            let p = format!("path={}", quote(folder));
            let args: Vec<&str> = if folder.is_empty() {
                vec![]
            } else {
                vec![p.as_str()]
            };
            let r = w.client.send(STATUS, Call::Request(&args));
            let resp = match w.held(r, false) {
                Ok(b) => b,
                Err(exit) => return exit,
            };
            if *json {
                return print_plain(&resp);
            }
            text(&lossy(&resp), "status", "").print()
        }
        Ask::Request(folder, why) => {
            let p = format!("path={}", quote(folder));
            let y = format!("why={}", quote(why));
            let r = w.client.send(REQUEST, Call::RequestPost(&[&p, &y]));
            let resp = match w.held(r, false) {
                Ok(b) => b,
                Err(exit) => return exit,
            };
            text(&lossy(&resp), "request", "").print()
        }
        Ask::Decide(d) => owner(w, d),
    }
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn owner(w: &mut Sent, d: &Decide) -> Exit {
    let p = format!("path={}", quote(&d.folder));
    if w.client.passport().is_some() {
        return not_yours(w, &d.folder, &p);
    }
    let post = |w: &mut Sent, endpoint: &str, args: &[&str]| {
        let r = w
            .client
            .call(Channel::Tcp, endpoint, Call::RequestPost(args));
        w.held(r, false)
    };
    match d.decision.as_str() {
        "reset" => match post(w, RESET, &[&p]) {
            Ok(resp) => text(&lossy(&resp), "status", "").print(),
            Err(exit) => exit,
        },
        "forget" => match post(w, FORGET, &[&p]) {
            Ok(resp) => text(&lossy(&resp), "forget", "").print(),
            Err(exit) => exit,
        },
        _ => {
            let mut fingerprint = d.fingerprint.clone();
            let mut scope = String::new();
            if fingerprint.is_empty() {
                let r = w.client.call(Channel::Tcp, STATUS, Call::Request(&[&p]));
                let st = match w.held(r, false) {
                    Ok(b) => lossy(&b),
                    Err(exit) => return exit,
                };
                // `_workspace_trust_print "$st" status` under `set -e`.
                let shown = text(&st, "status", "");
                let exit = shown.print();
                if exit != Exit::Ok {
                    return exit;
                }
                let (fp, sc) = match fingerprint_of(&st) {
                    Ok(pair) => pair,
                    Err(r) => {
                        return crate::families::raised(String::new(), String::new(), &r).print()
                    }
                };
                fingerprint = fp;
                scope = sc;
                if fingerprint != "none" {
                    out::stderr_line(&format!(
                        concat!(
                            "Error: this folder has risky settings; decide only for the ones you read. Run: ",
                            cli_name!(),
                            " workspace trust status {}, read the findings, then pass the --fingerprint it shows (or run this in a terminal to confirm at a prompt)."
                        ),
                        bash_q(&d.folder)
                    ));
                    return Exit::Usage;
                }
            }
            let dec = format!("decision={}", d.decision);
            let f = format!("fingerprint={}", quote(&fingerprint));
            let sc = format!("scope={}", quote(&scope));
            let mut args: Vec<&str> = vec![p.as_str(), dec.as_str(), f.as_str()];
            if !scope.is_empty() {
                args.push(&sc);
            }
            match post(w, DECIDE, &args) {
                Ok(resp) => text(&lossy(&resp), "decide", &d.folder).print(),
                Err(exit) => exit,
            }
        }
    }
}

/// The Python that reads the status for its fingerprint and scope:
/// `print(d.get("fingerprint") or "none")`, `print(d.get("scope") or
/// "")`, then `sed -n 1p` and `2p` of what `$(…)` held.
fn fingerprint_of(st: &str) -> Result<(String, String), Raise> {
    // The status reader above ended with 0: the answer is a JSON object.
    let d = json::parse(st).unwrap_or(Value::Null);
    let pairs = match &d {
        Value::Object(p) => p.as_slice(),
        other => return Err(Raise::no_attribute(3, other, "get")),
    };
    let fp = get(pairs, "fingerprint")
        .filter(|v| py::truthy(v))
        .map_or_else(|| "none".to_string(), py::str_of);
    let scope = get(pairs, "scope")
        .filter(|v| py::truthy(v))
        .map_or_else(String::new, py::str_of);
    let held = format!("{fp}\n{scope}\n").replace('\0', "");
    let held = held.trim_end_matches('\n');
    let mut lines = held.split('\n');
    Ok((
        lines.next().unwrap_or("").to_string(),
        lines.next().unwrap_or("").to_string(),
    ))
}

/// `_workspace_trust_not_yours`: the status lookup for where the owner
/// decides in the app, then three lines, exit 3.
fn not_yours(w: &mut Sent, folder: &str, p: &str) -> Exit {
    let r = w.client.send(STATUS, Call::Request(&[p]));
    let mut place = String::new();
    match r {
        Ok(r) => {
            w.note(&r);
            let body = crate::families::piped_body(&r);
            if let Ok(Value::Object(pairs)) = json::parse(&lossy(&body)) {
                let v = get(&pairs, "decide_in_app").filter(|v| py::truthy(v));
                let printable = !matches!(v, Some(Value::Str(t)) if t.as_str().is_none());
                if let (Some(v), true) = (v, printable) {
                    place = py::str_of(v).replace('\0', "");
                    place = place.trim_end_matches('\n').to_string();
                }
            }
        }
        Err(
            e @ (crate::transport::Error::NoAnswer(_)
            | crate::transport::Error::MaybeApplied { .. }),
        ) => return w.failed_in_pipe(&e, true),
        Err(_) => {}
    }
    if place.is_empty() {
        place = "Settings → Workspaces / Agents → (that workspace) → Agent → Folder trust".into();
    }
    out::stderr_line(&format!(
        "Error: only the owner decides folder trust; an agent or a {PRODUCT_NAME} terminal can't."
    ));
    out::stderr_line(&format!(
        "The owner can also decide in the app: {place}, or by opening that agent."
    ));
    out::stderr_line(&format!(
        concat!(
            "Ask with: ",
            cli_name!(),
            " workspace trust request {} --why \"<what you need it for>\""
        ),
        bash_q(folder)
    ));
    Exit::Refused
}

/// `cli_print_or_fail "$resp"` with no endpoint: the answer and a
/// newline, or its error (exit 3 with a card, else 1).
pub fn print_plain(resp: &[u8]) -> Exit {
    let text = lossy(resp);
    let err = match json::parse(&text) {
        Ok(Value::Object(pairs)) => match super::super::error_of(&pairs) {
            None => None,
            Some(Ok(e)) => Some(e),
            Some(Err(())) => Some(format!("could not read {PRODUCT_NAME}'s server answer")),
        },
        _ => None,
    };
    let err = err.map(|e| e.replace('\0', "").trim_end_matches('\n').to_string());
    match err {
        Some(e) if !e.is_empty() => {
            out::stderr_line(&format!("Error: {e}"));
            super::super::refused_or(resp, Exit::Error)
        }
        _ => {
            let mut bytes = resp.to_vec();
            bytes.push(b'\n');
            match out::write_flushed(&mut std::io::stdout().lock(), &bytes) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
    }
}

/// Python's `shlex.quote`.
pub fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    let safe = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}

/// Bash's `printf '%q'`, as it quotes in the C locale: backslashes before
/// the shell's special characters, or `$'…'` when there is a byte that is
/// not printable ASCII.
pub fn bash_q(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.bytes().any(|b| !(0x20..0x7f).contains(&b)) {
        let mut out = String::from("$'");
        for b in s.bytes() {
            match b {
                0x07 => out.push_str("\\a"),
                0x08 => out.push_str("\\b"),
                0x0c => out.push_str("\\f"),
                b'\n' => out.push_str("\\n"),
                b'\r' => out.push_str("\\r"),
                b'\t' => out.push_str("\\t"),
                0x0b => out.push_str("\\v"),
                0x1b => out.push_str("\\E"),
                b'\\' => out.push_str("\\\\"),
                b'\'' => out.push_str("\\'"),
                0x20..=0x7e => out.push(b as char),
                b => out.push_str(&format!("\\{b:03o}")),
            }
        }
        out.push('\'');
        return out;
    }
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        let special = matches!(
            c,
            ' ' | '\''
                | '"'
                | '\\'
                | '|'
                | '&'
                | ';'
                | '('
                | ')'
                | '<'
                | '>'
                | '!'
                | '{'
                | '}'
                | '*'
                | '['
                | '?'
                | ']'
                | '^'
                | '$'
                | '`'
                | ','
        ) || (i == 0 && matches!(c, '~' | '#'));
        if special {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The Python lines a traceback names (`_workspace_trust_print`).
pub mod line {
    pub const ERR: u32 = 9;
    pub const MSG: u32 = 11;
    pub const ERR_DICT: u32 = 13;
    pub const ERR_LINE: u32 = 15;
    pub const SHOW_CALL: u32 = 55;
    pub const FOLDERS_FOR: u32 = 52;
    pub const FOLDERS_ROW: u32 = 53;
    pub const HELD: u32 = 63;
    pub const FINDINGS: u32 = 65;
    pub const FINDING: u32 = 66;
    pub const LEFTOVER: u32 = 67;
    pub const REMOVED_FOR: u32 = 79;
    pub const STILL: u32 = 84;
    pub const DONE_LEFTOVER: u32 = 88;
}

type R<T> = Result<T, Raise>;

fn attr(line: u32) -> impl Fn(String) -> Raise {
    move |e| Raise::at(line, format!("AttributeError: {e}"))
}

fn in_show(line: u32) -> impl Fn(String) -> Raise {
    move |e| {
        Raise::inside(
            line::SHOW_CALL,
            line,
            "show",
            format!("AttributeError: {e}"),
        )
    }
}

/// `_workspace_trust_print "$resp" <mode> [folder]` on what `$(…)` held.
pub fn text(raw: &str, mode: &str, folder: &str) -> Shown {
    let mut p = Py::default();
    let Ok(d) = json::parse(raw) else {
        p.eprint("Error: the daemon answered something that is not JSON");
        return p.done(Exit::Error);
    };
    match body(&d, mode, folder, &mut p) {
        Ok(exit) => p.done(exit),
        Err(r) => p.raised(&r),
    }
}

fn val(v: Option<&Value>) -> Value {
    v.cloned().unwrap_or(Value::Null)
}

fn body(d: &Value, mode: &str, folder: &str, p: &mut Py) -> R<Exit> {
    let err = val(py_get(d, "error").map_err(attr(line::ERR))?);
    if py::truthy(&err) {
        let or = crate::families::remind::shared::or;
        let mut msg = or(
            Some(&or(
                py_get(d, "message").map_err(attr(line::MSG))?,
                val(py_get(d, "hint").map_err(attr(line::MSG))?),
            )),
            s(""),
        );
        let mut err = err;
        if matches!(err, Value::Object(_)) {
            let hint = val(py_get(&err, "hint").map_err(attr(line::ERR_DICT))?);
            let message = val(py_get(&err, "message").map_err(attr(line::ERR_DICT))?);
            msg = or(Some(&or(Some(&hint), message)), msg);
            err = or(
                py_get(&err, "code").map_err(attr(line::ERR_DICT))?,
                s("refused"),
            );
        }
        let tail = if py::truthy(&msg) {
            str_plus(": ", &msg)
                .map_err(|e| Raise::at(line::ERR_LINE, format!("TypeError: {e}")))?
        } else {
            String::new()
        };
        p.eprint(&format!("Error: {}{tail}", py::str_of(&err)));
        if is_str(&err, "action_card_person_only") {
            if mode == "decide" && !folder.is_empty() {
                p.eprint(&format!(
                    concat!(
                        "Ask with: ",
                        cli_name!(),
                        " workspace trust request {} --why \"<what you need it for>\""
                    ),
                    shlex_quote(folder)
                ));
            } else {
                p.eprint("It can't be requested.");
            }
            return Ok(Exit::Refused);
        }
        return Ok(Exit::Error);
    }
    let g = |k: &str, line: u32| -> R<Value> { Ok(val(py_get(d, k).map_err(attr(line))?)) };
    match mode {
        "status" => {
            if py_in("folders", d).unwrap_or(false) {
                let folders = val(get(as_pairs(d), "folders"));
                for v in py_iter(&folders, line::FOLDERS_FOR)? {
                    let state = py_get(&v, "state").map_err(attr(line::FOLDERS_ROW))?;
                    let state = state.cloned().unwrap_or_else(|| s("?"));
                    let path = py_get(&v, "path").map_err(attr(line::FOLDERS_ROW))?;
                    let path = path.cloned().unwrap_or_else(|| s("?"));
                    p.print(&format!(
                        "{} {}",
                        ljust(&py::str_of(&state), 10),
                        py::str_of(&path)
                    ));
                }
            } else {
                show(d, p)?;
            }
        }
        "request" => {
            let st = g("state", 57)?;
            if is_str(&st, "clean") {
                p.print("Clean: no risky settings, so the folder is trusted as before.");
            } else if is_str(&st, "trusted") {
                p.print("Already trusted (a decision covers these settings).");
            } else if is_str(&st, "held") {
                let te = |e: String| Raise::at(line::HELD, format!("TypeError: {e}"));
                let place = if py::truthy(&g("card", line::HELD)?) {
                    str_plus("card ", &g("card", line::HELD)?).map_err(te)?
                } else if py::truthy(&g("ticket", line::HELD)?) {
                    str_plus("owner ticket ", &g("ticket", line::HELD)?).map_err(te)?
                } else {
                    "no Thread or ticket".to_string()
                };
                p.print(&format!(
                    "Asked the owner ({place}). {PRODUCT_NAME} starts no program there (but a plain shell tab) until they answer."
                ));
                let findings = crate::families::remind::shared::or(
                    Some(&g("findings", line::FINDINGS)?),
                    Value::Array(vec![]),
                );
                for f in head5(&findings)? {
                    let file = val(py_get(&f, "file").map_err(attr(line::FINDING))?);
                    let detail = val(py_get(&f, "detail").map_err(attr(line::FINDING))?);
                    p.print(&format!(
                        "  • {}: {}",
                        py::str_of(&file),
                        py::str_of(&detail)
                    ));
                }
                for t in iter_or_empty(&g("leftover", line::LEFTOVER)?, line::LEFTOVER)? {
                    p.print(&format!(
                        "  Still trusted outside {PRODUCT_NAME}: {}",
                        py::str_of(&t)
                    ));
                }
                let place = g("decide_in_app", 69)?;
                if py::truthy(&place) {
                    p.print(&format!(
                        "The owner can also decide in the app: {}, or by opening that agent.",
                        py::str_of(&place)
                    ));
                }
            } else if is_str(&st, "denied") {
                let place = crate::families::remind::shared::or(
                    Some(&g("decide_in_app", 72)?),
                    s("Settings → Workspaces / Agents → <workspace> → Agent → Folder trust"),
                );
                p.print(&format!(
                    "Not trusted: the owner chose \"Don't trust\". Only the owner can change it ({} → Reset).",
                    py::str_of(&place)
                ));
            } else {
                let reason = crate::families::remind::shared::or(Some(&g("reason", 74)?), st);
                p.print(&format!("Not gated: {}", py::str_of(&reason)));
            }
        }
        "forget" => {
            let removed =
                crate::families::remind::shared::or(Some(&g("removed", 76)?), Value::Array(vec![]));
            if py::truthy(&removed) {
                p.print("Removed:");
                for t in py_iter(&removed, line::REMOVED_FOR)? {
                    p.print(&format!("  {}", py::str_of(&t)));
                }
            } else {
                p.print(&format!(
                    "Nothing to remove: no tool trusts this folder outside {PRODUCT_NAME}'s own entries."
                ));
            }
            let st =
                crate::families::remind::shared::or(Some(&g("status", 83)?), Value::Object(vec![]));
            let by = val(py_get(&st, "trusted_by").map_err(attr(line::STILL))?);
            for t in iter_or_empty(&by, line::STILL)? {
                p.print(&format!("  Still trusted: {}", py::str_of(&t)));
            }
        }
        _ => {
            let m = crate::families::remind::shared::or(Some(&g("message", 87)?), s("Done."));
            p.print(&py::str_of(&m));
            for t in iter_or_empty(&g("leftover", line::DONE_LEFTOVER)?, line::DONE_LEFTOVER)? {
                p.print(&format!("  Still trusted elsewhere: {}", py::str_of(&t)));
            }
        }
    }
    Ok(Exit::Ok)
}

fn as_pairs(v: &Value) -> &[(json::Text, Value)] {
    match v {
        Value::Object(p) => p,
        _ => &[],
    }
}

/// `for x in v or []`.
fn iter_or_empty(v: &Value, line: u32) -> R<Vec<Value>> {
    if py::truthy(v) {
        py_iter(v, line)
    } else {
        Ok(Vec::new())
    }
}

/// `(v)[:5]`, walked.
fn head5(v: &Value) -> R<Vec<Value>> {
    match v {
        Value::Array(items) => Ok(items.iter().take(5).cloned().collect()),
        Value::Str(_) => Ok(py_iter(v, line::FINDINGS)?.into_iter().take(5).collect()),
        Value::Object(_) => Err(Raise::at(
            line::FINDINGS,
            "TypeError: unhashable type: 'slice'",
        )),
        other => Err(Raise::at(
            line::FINDINGS,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// `show(v)` on the status of one folder.
fn show(v: &Value, p: &mut Py) -> R<()> {
    let g = |k: &str, line: u32| -> R<Value> { Ok(val(py_get(v, k).map_err(in_show(line))?)) };
    let state = py_get(v, "state")
        .map_err(in_show(26))?
        .cloned()
        .unwrap_or_else(|| s("?"));
    let path = py_get(v, "path")
        .map_err(in_show(27))?
        .cloned()
        .unwrap_or_else(|| s("?"));
    let mut line = format!("{}: {}", py::str_of(&path), py::str_of(&state));
    let by = g("decided_by", 28)?;
    if py::truthy(&by)
        && ["trusted", "always", "denied", "changed"]
            .iter()
            .any(|w| is_str(&state, w))
    {
        line.push_str(&format!(" (by {})", py::str_of(&by)));
    }
    p.print(&line);
    let findings = g("findings", 32)?;
    let findings = if py::truthy(&findings) {
        py_iter(&findings, 32)
            .map_err(|r| Raise::inside(line::SHOW_CALL, 32, "show", r.exception))?
    } else {
        Vec::new()
    };
    for f in findings {
        let file = val(py_get(&f, "file").map_err(in_show(33))?);
        let detail = val(py_get(&f, "detail").map_err(in_show(33))?);
        p.print(&format!(
            "  • {}: {}",
            py::str_of(&file),
            py::str_of(&detail)
        ));
    }
    let more = g("more", 34)?;
    if py::truthy(&more) {
        let n =
            py_d(&more, 35).map_err(|r| Raise::inside(line::SHOW_CALL, 35, "show", r.exception))?;
        p.print(&format!("  (and {n} more)"));
    }
    for (key, label, at) in [
        ("card", "  Card waiting on Thread", 36),
        ("ticket", "  Owner ticket", 38),
    ] {
        let x = g(key, at)?;
        if py::truthy(&x) {
            p.print(&format!("{label}: {}", py::str_of(&x)));
        }
    }
    let scope = g("scope", 40)?;
    if py::truthy(&scope) && scope != path_of(v) {
        p.print(&format!("  Decision covers: {}", py::str_of(&scope)));
    }
    let fp = g("fingerprint", 42)?;
    if py::truthy(&fp) {
        p.print(&format!("  Fingerprint: {}", py::str_of(&fp)));
    }
    let by = g("trusted_by", 44)?;
    if py::truthy(&by) {
        let items = py_iter(&by, 44)
            .map_err(|r| Raise::inside(line::SHOW_CALL, 44, "show", r.exception))?;
        for t in items {
            p.print(&format!("  Trusted by: {}", py::str_of(&t)));
        }
    }
    let forget = g("forget", 46)?;
    if py::truthy(&forget) {
        p.print(&format!(
            "  Remove those entries (owner): {}",
            py::str_of(&forget)
        ));
    }
    if ["pending", "undecided", "changed", "denied"]
        .iter()
        .any(|w| is_str(&state, w))
    {
        let decide = py_get(v, "decide")
            .map_err(in_show(49))?
            .cloned()
            .unwrap_or_else(|| s(""));
        p.print(&format!("  Decide (owner): {}", py::str_of(&decide)));
    }
    Ok(())
}

/// `v.get("path")` (`None` when missing), for `!=`.
fn path_of(v: &Value) -> Value {
    val(get(as_pairs(v), "path"))
}
